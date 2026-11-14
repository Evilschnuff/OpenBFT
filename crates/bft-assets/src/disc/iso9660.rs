//! ISO 9660 (ECMA-119), as far as finding and copying files needs.
//!
//! The primary volume descriptor is sector 16; its root directory record
//! leads to directories of variable-length records (extent, size, flags,
//! name). Names are matched ignoring case and the `;1` version suffix, so
//! `data1.cab` finds `DATA1.CAB;1`. Joliet and Rock Ridge are not read: the
//! primary names are enough for what setup copies.

use std::io::Write as _;
use std::path::Path;

use super::{DataTrack, DiscError};

/// A file or directory on the volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Its name as recorded, without the version suffix.
    pub name: String,
    /// The first sector of its data.
    pub extent: u64,
    /// Its length in bytes.
    pub size: u64,
    /// Whether it is a directory.
    pub is_dir: bool,
}

/// An ISO 9660 volume on a data track.
pub struct Volume {
    track: DataTrack,
    root: Entry,
    /// The volume label.
    pub label: String,
}

impl Volume {
    /// Read the volume descriptor of the file system on `track`.
    pub fn open(mut track: DataTrack) -> Result<Volume, DiscError> {
        let mut pvd = [0u8; 2048];
        track.read_sector(16, &mut pvd)?;
        if pvd[0] != 1 || &pvd[1..6] != b"CD001" {
            return Err(DiscError::format(
                &track.path,
                "the data track holds no ISO 9660 file system",
            ));
        }
        let block = u16::from_le_bytes([pvd[128], pvd[129]]);
        if block != 2048 {
            return Err(DiscError::format(
                &track.path,
                format!("the file system uses {block}-byte blocks"),
            ));
        }

        let root = record(&pvd[156..190]).ok_or_else(|| {
            DiscError::format(&track.path, "the root directory record is damaged")
        })?;
        let label = String::from_utf8_lossy(&pvd[40..72])
            .trim_matches([' ', '\0'])
            .to_string();

        Ok(Volume { track, root, label })
    }

    /// The entries of directory `dir`.
    pub fn list(&mut self, dir: &Entry) -> Result<Vec<Entry>, DiscError> {
        let mut entries = Vec::new();
        let mut sector = [0u8; 2048];

        for n in 0..dir.size.div_ceil(2048) {
            self.track.read_sector(dir.extent + n, &mut sector)?;
            let mut at = 0usize;
            // Records never cross a sector; a zero length pads to the next.
            while at < 2048 && sector[at] != 0 {
                let len = sector[at] as usize;
                let Some(bytes) = sector.get(at..at + len) else {
                    break;
                };
                if let Some(entry) = record(bytes)
                    && entry.name != "."
                    && entry.name != ".."
                {
                    entries.push(entry);
                }
                at += len;
            }
        }

        Ok(entries)
    }

    /// The entry at `path` (`/`-separated, any case), if there is one.
    pub fn find(&mut self, path: &str) -> Result<Option<Entry>, DiscError> {
        let mut current = self.root.clone();

        for part in path.split('/').filter(|p| !p.is_empty()) {
            if !current.is_dir {
                return Ok(None);
            }
            let found = self
                .list(&current)?
                .into_iter()
                .find(|entry| entry.name.eq_ignore_ascii_case(part));
            match found {
                Some(entry) => current = entry,
                None => return Ok(None),
            }
        }

        Ok(Some(current))
    }

    /// Copy file `entry` to `dest`, calling `progress` with the bytes done.
    ///
    /// A file that runs past the end of the track is
    /// [`DiscError::Truncated`], checked before anything is written.
    pub fn copy(
        &mut self,
        entry: &Entry,
        dest: &Path,
        mut progress: impl FnMut(u64),
    ) -> Result<(), DiscError> {
        let needs = entry.extent + entry.size.div_ceil(2048);
        if needs > self.track.sectors() {
            return Err(DiscError::Truncated {
                name: entry.name.clone(),
                needs,
                has: self.track.sectors(),
            });
        }

        let partial = dest.with_extension("partial");
        let file = std::fs::File::create(&partial).map_err(DiscError::io(&partial))?;
        let mut out = std::io::BufWriter::new(file);
        let mut sector = [0u8; 2048];
        let mut left = entry.size;

        for n in 0.. {
            if left == 0 {
                break;
            }
            self.track.read_sector(entry.extent + n, &mut sector)?;
            let take = left.min(2048) as usize;
            out.write_all(&sector[..take])
                .map_err(DiscError::io(&partial))?;
            left -= take as u64;
            progress(entry.size - left);
        }
        out.flush().map_err(DiscError::io(&partial))?;
        drop(out);

        std::fs::rename(&partial, dest).map_err(DiscError::io(dest))
    }
}

/// A directory record: extent at 2, size at 10 (both-endian, the
/// little-endian half read), flags at 25, the name's length at 32 and the
/// name from 33. `.` and `..` are recorded as the bytes 0 and 1.
fn record(bytes: &[u8]) -> Option<Entry> {
    let extent = u32::from_le_bytes(bytes.get(2..6)?.try_into().ok()?);
    let size = u32::from_le_bytes(bytes.get(10..14)?.try_into().ok()?);
    let flags = *bytes.get(25)?;
    let name_len = *bytes.get(32)? as usize;
    let raw = bytes.get(33..33 + name_len)?;

    let name = match raw {
        [0] => ".".to_string(),
        [1] => "..".to_string(),
        _ => {
            let name = String::from_utf8_lossy(raw);
            let name = name.split(';').next().unwrap_or_default();
            // A file without an extension is recorded as `NAME.`.
            name.strip_suffix('.').unwrap_or(name).to_string()
        }
    };

    Some(Entry {
        name,
        extent: u64::from(extent),
        size: u64::from(size),
        is_dir: flags & 2 != 0,
    })
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::disc::{Track, TrackKind};
    use crate::test_support::TempDir;

    /// A directory record for `name` at `extent`.
    fn dir_record(name: &[u8], extent: u32, size: u32, is_dir: bool) -> Vec<u8> {
        let mut r = vec![0u8; 33];
        r[2..6].copy_from_slice(&extent.to_le_bytes());
        r[6..10].copy_from_slice(&extent.to_be_bytes());
        r[10..14].copy_from_slice(&size.to_le_bytes());
        r[14..18].copy_from_slice(&size.to_be_bytes());
        r[25] = if is_dir { 2 } else { 0 };
        r[32] = name.len() as u8;
        r.extend_from_slice(name);
        if r.len() % 2 == 1 {
            r.push(0);
        }
        r[0] = r.len() as u8;
        r
    }

    /// A small volume: `/DATA1.CAB;1` (sector 20) and `/VIDEO/INTRO.AVI;1`
    /// (sector 21), as 2048-byte sectors. `sectors` cuts the image.
    pub(crate) fn tiny_iso(cab: &[u8], sectors: usize) -> Vec<u8> {
        let mut img = vec![0u8; 24 * 2048];
        let pvd = 16 * 2048;
        img[pvd] = 1;
        img[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        img[pvd + 40..pvd + 46].copy_from_slice(b"RECOIL");
        img[pvd + 128..pvd + 130].copy_from_slice(&2048u16.to_le_bytes());
        let root = dir_record(&[0], 18, 2048, true);
        img[pvd + 156..pvd + 156 + root.len()].copy_from_slice(&root);

        let mut dir = Vec::new();
        dir.extend(dir_record(&[0], 18, 2048, true));
        dir.extend(dir_record(&[1], 18, 2048, true));
        dir.extend(dir_record(b"DATA1.CAB;1", 20, cab.len() as u32, false));
        dir.extend(dir_record(b"VIDEO", 19, 2048, true));
        img[18 * 2048..18 * 2048 + dir.len()].copy_from_slice(&dir);

        let mut video = Vec::new();
        video.extend(dir_record(&[0], 19, 2048, true));
        video.extend(dir_record(&[1], 18, 2048, true));
        video.extend(dir_record(b"INTRO.AVI;1", 22, 5, false));
        img[19 * 2048..19 * 2048 + video.len()].copy_from_slice(&video);

        img[20 * 2048..20 * 2048 + cab.len()].copy_from_slice(cab);
        img[22 * 2048..22 * 2048 + 5].copy_from_slice(b"movie");
        img.truncate(sectors * 2048);
        img
    }

    fn volume(dir: &TempDir, bytes: &[u8]) -> Volume {
        let path = dir.path().join("disc.iso");
        std::fs::write(&path, bytes).unwrap();
        let track = Track {
            number: 1,
            kind: TrackKind::Data,
            file: path,
            start: 0,
            sectors: bytes.len() as u64 / 2048,
            stride: 2048,
        };
        Volume::open(DataTrack::open(&track).unwrap()).unwrap()
    }

    #[test]
    fn files_are_found_in_any_case_and_copied() {
        let dir = TempDir::new();
        let mut volume = volume(&dir, &tiny_iso(b"ISc(cabinet", 24));
        assert_eq!(volume.label, "RECOIL");

        let cab = volume.find("data1.cab").unwrap().expect("found");
        assert_eq!((cab.name.as_str(), cab.size), ("DATA1.CAB", 11));
        let dest = dir.path().join("data1.cab");
        volume.copy(&cab, &dest, |_| {}).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"ISc(cabinet");

        let intro = volume.find("/Video/intro.avi").unwrap().expect("found");
        assert_eq!(intro.size, 5);
        assert!(volume.find("video/missing.avi").unwrap().is_none());
        assert!(volume.find("data1.cab/inside").unwrap().is_none());
    }

    #[test]
    fn a_file_past_the_end_of_the_track_is_reported_not_copied() {
        let dir = TempDir::new();
        // The image stops before INTRO.AVI's sector 22.
        let mut volume = volume(&dir, &tiny_iso(b"cab", 22));

        let intro = volume.find("video/intro.avi").unwrap().expect("listed");
        let dest = dir.path().join("intro.avi");
        let err = volume.copy(&intro, &dest, |_| {}).unwrap_err();

        assert!(
            matches!(
                err,
                DiscError::Truncated {
                    needs: 23,
                    has: 22,
                    ..
                }
            ),
            "{err}"
        );
        assert!(!dest.exists());
    }

    #[test]
    fn a_track_without_a_file_system_says_so() {
        let dir = TempDir::new();
        let path = dir.path().join("blank.iso");
        std::fs::write(&path, vec![0u8; 20 * 2048]).unwrap();
        let track = Track {
            number: 1,
            kind: TrackKind::Data,
            file: path,
            start: 0,
            sectors: 20,
            stride: 2048,
        };

        let err = Volume::open(DataTrack::open(&track).unwrap())
            .err()
            .unwrap();

        assert!(err.to_string().contains("no ISO 9660"), "{err}");
    }
}
