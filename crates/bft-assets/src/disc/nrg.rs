//! Nero images (`.nrg`): the tracks from the chunk list at the end of the
//! file.
//!
//! The footer is "NER5" and a 64-bit big-endian offset (version 2), or
//! "NERO" and a 32-bit one (version 1). From that offset, chunks run as a
//! 4-byte id, a 32-bit big-endian length and the body, up to `END!`. The
//! track table is a `DAOX`/`DAOI` chunk for a disc written at once, or
//! `ETN2`/`ETNF` for one written track at a time. The layout follows
//! libcdio's `nrg.h`; docs/data/install.md records what the retail image
//! holds.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::Path;

use super::{AUDIO_SECTOR, DATA_SECTOR, DiscError, Track, TrackKind};

/// The tracks of the Nero image at `path`.
pub fn tracks(path: &Path) -> Result<Vec<Track>, DiscError> {
    let mut file = File::open(path).map_err(DiscError::io(path))?;
    let len = file.metadata().map_err(DiscError::io(path))?.len();

    let offset = footer(&mut file, len)
        .map_err(DiscError::io(path))?
        .ok_or_else(|| {
            DiscError::format(path, "no Nero footer (NER5 or NERO) at the end of the file")
        })?;
    if offset >= len {
        return Err(DiscError::format(
            path,
            "the Nero footer points past the end of the file",
        ));
    }

    let mut list = vec![0u8; (len - offset) as usize];
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(&mut list))
        .map_err(DiscError::io(path))?;

    let chunks =
        chunks(&list).ok_or_else(|| DiscError::format(path, "the Nero chunk list is cut short"))?;

    for (id, body) in &chunks {
        match id {
            b"DAOX" => return dao(path, body, true),
            b"DAOI" => return dao(path, body, false),
            b"ETN2" => return etn(path, body, true),
            b"ETNF" => return etn(path, body, false),
            _ => {}
        }
    }

    Err(DiscError::format(
        path,
        "the Nero image has no track table (DAOX, DAOI, ETN2 or ETNF)",
    ))
}

/// The chunk list's offset, from a version 2 or version 1 footer.
fn footer(file: &mut File, len: u64) -> std::io::Result<Option<u64>> {
    if len < 12 {
        return Ok(None);
    }
    let mut tail = [0u8; 12];
    file.seek(SeekFrom::End(-12))?;
    file.read_exact(&mut tail)?;

    if &tail[..4] == b"NER5" {
        return Ok(Some(u64::from_be_bytes(
            tail[4..12].try_into().unwrap_or_default(),
        )));
    }
    if &tail[4..8] == b"NERO" {
        return Ok(Some(u64::from(u32::from_be_bytes(
            tail[8..12].try_into().unwrap_or_default(),
        ))));
    }
    Ok(None)
}

/// The chunks up to `END!`, or `None` when one runs past the list.
fn chunks(list: &[u8]) -> Option<Vec<([u8; 4], &[u8])>> {
    let mut out = Vec::new();
    let mut at = 0usize;

    loop {
        let id: [u8; 4] = list.get(at..at + 4)?.try_into().ok()?;
        if &id == b"END!" {
            return Some(out);
        }
        let size = u32::from_be_bytes(list.get(at + 4..at + 8)?.try_into().ok()?) as usize;
        let body = list.get(at + 8..at + 8 + size)?;
        out.push((id, body));
        at += 8 + size;
    }
}

fn be_u64(bytes: &[u8], wide: bool) -> u64 {
    if wide {
        u64::from_be_bytes(bytes[..8].try_into().unwrap_or_default())
    } else {
        u64::from(u32::from_be_bytes(
            bytes[..4].try_into().unwrap_or_default(),
        ))
    }
}

/// Whether a Nero mode byte is CD audio: 7, or 0x10 with subchannel data.
fn is_audio(mode: u8) -> bool {
    matches!(mode, 0x07 | 0x10)
}

/// A disc-at-once table: a 22-byte header (ending in the first and last
/// track numbers), then per track a 12-byte ISRC, the sector size, the
/// mode, two unknown bytes and three positions (index 0, index 1, end),
/// 64-bit in `DAOX` and 32-bit in `DAOI`.
fn dao(path: &Path, body: &[u8], wide: bool) -> Result<Vec<Track>, DiscError> {
    let bad = || DiscError::format(path, "the Nero DAO track table is cut short");
    let (first, last) = (
        *body.get(20).ok_or_else(bad)?,
        *body.get(21).ok_or_else(bad)?,
    );
    let width = if wide { 8 } else { 4 };
    let record = 18 + 3 * width;

    let mut tracks = Vec::new();
    for (n, number) in (first..=last).enumerate() {
        let r = body
            .get(22 + n * record..22 + (n + 1) * record)
            .ok_or_else(bad)?;
        let stride = u32::from(u16::from_be_bytes([r[12], r[13]]));
        let mode = r[14];
        let index1 = be_u64(&r[18 + width..], wide);
        let end = be_u64(&r[18 + 2 * width..], wide);

        if stride == 0 || end < index1 {
            return Err(DiscError::format(
                path,
                format!("track {number} has an impossible layout"),
            ));
        }
        tracks.push(Track {
            number,
            kind: if is_audio(mode) {
                TrackKind::Audio
            } else {
                TrackKind::Data
            },
            file: path.to_path_buf(),
            start: index1,
            sectors: (end - index1) / u64::from(stride),
            stride,
        });
    }

    Ok(tracks)
}

/// A track-at-once table: per track its offset and length in the file
/// (64-bit in `ETN2`, 32-bit in `ETNF`), the mode and the start sector.
/// The sector size follows from the mode.
fn etn(path: &Path, body: &[u8], wide: bool) -> Result<Vec<Track>, DiscError> {
    let width = if wide { 8 } else { 4 };
    let record = if wide { 32 } else { 20 };

    body.chunks_exact(record)
        .enumerate()
        .map(|(n, r)| {
            let offset = be_u64(r, wide);
            let length = be_u64(&r[width..], wide);
            let mode =
                u32::from_be_bytes(r[2 * width..2 * width + 4].try_into().unwrap_or_default());
            let number = u8::try_from(n + 1).unwrap_or(u8::MAX);

            let (kind, stride) = match mode {
                0 | 2 => (TrackKind::Data, DATA_SECTOR),
                3 => (TrackKind::Data, 2336),
                6 => (TrackKind::Data, AUDIO_SECTOR),
                7 => (TrackKind::Audio, AUDIO_SECTOR),
                other => {
                    return Err(DiscError::format(
                        path,
                        format!(
                            "track {number} has Nero mode {other}, which this reader does not know"
                        ),
                    ));
                }
            };

            Ok(Track {
                number,
                kind,
                file: path.to_path_buf(),
                start: offset,
                sectors: length / u64::from(stride),
                stride,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// An image of `payload` followed by `chunks` and a version 2 footer.
    fn image(payload: &[u8], chunks: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
        let mut bytes = payload.to_vec();
        let offset = bytes.len() as u64;
        for (id, body) in chunks {
            bytes.extend_from_slice(*id);
            bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
            bytes.extend_from_slice(body);
        }
        bytes.extend_from_slice(b"END!\0\0\0\0");
        bytes.extend_from_slice(b"NER5");
        bytes.extend_from_slice(&offset.to_be_bytes());
        bytes
    }

    /// A `DAOX` body: (sector size, mode, index 1, end) per track.
    fn daox(tracks: &[(u16, u8, u64, u64)]) -> Vec<u8> {
        let mut body = vec![0u8; 20];
        body.push(1);
        body.push(tracks.len() as u8);
        for (size, mode, index1, end) in tracks {
            body.extend_from_slice(&[0u8; 12]);
            body.extend_from_slice(&size.to_be_bytes());
            body.push(*mode);
            body.extend_from_slice(&[0u8; 3]);
            body.extend_from_slice(&0u64.to_be_bytes());
            body.extend_from_slice(&index1.to_be_bytes());
            body.extend_from_slice(&end.to_be_bytes());
        }
        body
    }

    #[test]
    fn a_dao_image_lists_its_data_and_audio_tracks() {
        // The retail image's shape (docs/data/install.md): a 2048-byte data
        // track after a 150-sector pregap, then audio.
        let pregap = 150 * 2048;
        let data_end = pregap + 4 * 2048;
        let audio_end = data_end + 2 * 2352;
        let payload = vec![0u8; audio_end as usize];
        let dir = TempDir::new();
        let path = dir.path().join("disc.nrg");
        std::fs::write(
            &path,
            image(
                &payload,
                &[
                    (b"CUEX", vec![0; 16]),
                    (
                        b"DAOX",
                        daox(&[(2048, 0, pregap, data_end), (2352, 7, data_end, audio_end)]),
                    ),
                ],
            ),
        )
        .unwrap();

        let tracks = tracks(&path).unwrap();

        assert_eq!(tracks.len(), 2);
        assert_eq!(
            (
                tracks[0].number,
                tracks[0].kind,
                tracks[0].start,
                tracks[0].sectors,
                tracks[0].stride
            ),
            (1, TrackKind::Data, pregap, 4, 2048)
        );
        assert_eq!(
            (
                tracks[1].number,
                tracks[1].kind,
                tracks[1].start,
                tracks[1].sectors,
                tracks[1].stride
            ),
            (2, TrackKind::Audio, data_end, 2, 2352)
        );
    }

    #[test]
    fn a_tao_image_lists_its_tracks_by_mode() {
        let mut etn2 = Vec::new();
        for (offset, length, mode) in [(0u64, 4 * 2048u64, 0u32), (4 * 2048, 2 * 2352, 7)] {
            etn2.extend_from_slice(&offset.to_be_bytes());
            etn2.extend_from_slice(&length.to_be_bytes());
            etn2.extend_from_slice(&mode.to_be_bytes());
            etn2.extend_from_slice(&[0u8; 12]);
        }
        let dir = TempDir::new();
        let path = dir.path().join("disc.nrg");
        std::fs::write(
            &path,
            image(&vec![0u8; 4 * 2048 + 2 * 2352], &[(b"ETN2", etn2)]),
        )
        .unwrap();

        let tracks = tracks(&path).unwrap();

        assert_eq!(tracks.len(), 2);
        assert_eq!((tracks[0].kind, tracks[0].sectors), (TrackKind::Data, 4));
        assert_eq!(
            (tracks[1].kind, tracks[1].start, tracks[1].sectors),
            (TrackKind::Audio, 8192, 2)
        );
    }

    #[test]
    fn an_image_without_a_track_table_says_so() {
        let dir = TempDir::new();
        let path = dir.path().join("disc.nrg");
        std::fs::write(&path, image(&[0u8; 64], &[(b"SINF", vec![0; 4])])).unwrap();

        let err = tracks(&path).unwrap_err().to_string();

        assert!(err.contains("no track table"), "{err}");
    }

    #[test]
    fn a_chunk_list_cut_short_is_an_error_not_a_panic() {
        let dir = TempDir::new();
        let path = dir.path().join("disc.nrg");
        let mut bytes = vec![0u8; 64];
        bytes.extend_from_slice(b"DAOX\0\0\x10\0");
        bytes.extend_from_slice(b"NER5");
        bytes.extend_from_slice(&64u64.to_be_bytes());
        std::fs::write(&path, bytes).unwrap();

        assert!(tracks(&path).is_err());
    }
}
