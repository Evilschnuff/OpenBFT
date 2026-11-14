//! Cue sheets (`.cue`) and the raw `.bin` files they name.
//!
//! A sheet is text: `FILE "<name>" BINARY` (or `MOTOROLA`, big-endian
//! audio), then per track `TRACK <n> <mode>` and its `INDEX` points as
//! `mm:ss:ff` sector positions in that file, 75 sectors to the second.
//! A track runs from its `INDEX 01` to the next track's first index in the
//! same file, or to the file's end. Tracks kept as separate audio files
//! (`WAVE`, `MP3`) are already music and are left out here.

use std::path::{Path, PathBuf};

use super::{DiscError, Track, TrackKind};

/// The tracks the cue sheet at `path` describes.
pub fn tracks(path: &Path) -> Result<Vec<Track>, DiscError> {
    let text = std::fs::read_to_string(path).map_err(DiscError::io(path))?;
    let dir = path.parent().unwrap_or(Path::new("."));

    parse(&text, dir, &|file| {
        std::fs::metadata(file).map(|m| m.len()).ok()
    })
    .map_err(|reason| DiscError::format(path, reason))
}

/// One `TRACK` as the sheet gives it, before its length is known.
struct Entry {
    number: u8,
    kind: TrackKind,
    stride: u32,
    file: PathBuf,
    /// Its `INDEX` points in sectors, in order.
    indexes: Vec<(u8, u64)>,
}

/// The sheet's tracks. `file_len` names a file's length, so the last track
/// in each file knows where it ends; split out for the tests.
fn parse(
    text: &str,
    dir: &Path,
    file_len: &dyn Fn(&Path) -> Option<u64>,
) -> Result<Vec<Track>, String> {
    let mut entries: Vec<Entry> = Vec::new();
    // The current FILE, or `None` for one this reader leaves out.
    let mut file: Option<PathBuf> = None;
    let mut seen_file = false;

    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        let (keyword, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        let rest = rest.trim();
        let at = |what: &str| format!("line {}: {what}", n + 1);

        match keyword.to_ascii_uppercase().as_str() {
            "FILE" => {
                seen_file = true;
                let (name, kind) =
                    file_name_and_type(rest).ok_or_else(|| at("a FILE line without a name"))?;
                file = match kind.to_ascii_uppercase().as_str() {
                    "BINARY" | "MOTOROLA" => Some(dir.join(name)),
                    _ => None,
                };
            }
            "TRACK" => {
                let mut words = rest.split_whitespace();
                let number = words
                    .next()
                    .and_then(|w| w.parse::<u8>().ok())
                    .ok_or_else(|| at("a TRACK line without a number"))?;
                let mode = words.next().unwrap_or("").to_ascii_uppercase();
                let Some(current) = &file else {
                    continue;
                };
                let (kind, stride) = match mode.as_str() {
                    "AUDIO" => (TrackKind::Audio, 2352),
                    "MODE1/2048" => (TrackKind::Data, 2048),
                    "MODE1/2352" | "MODE2/2352" => (TrackKind::Data, 2352),
                    "MODE2/2336" => (TrackKind::Data, 2336),
                    other => {
                        return Err(at(&format!(
                            "track mode {other} is not one this reader knows"
                        )));
                    }
                };
                entries.push(Entry {
                    number,
                    kind,
                    stride,
                    file: current.clone(),
                    indexes: Vec::new(),
                });
            }
            "INDEX" => {
                if file.is_none() {
                    continue;
                }
                let mut words = rest.split_whitespace();
                let index = words.next().and_then(|w| w.parse::<u8>().ok());
                let position = words.next().and_then(msf);
                let (Some(index), Some(position)) = (index, position) else {
                    return Err(at("an INDEX line that is not `INDEX <n> mm:ss:ff`"));
                };
                let entry = entries
                    .last_mut()
                    .ok_or_else(|| at("an INDEX before any TRACK"))?;
                entry.indexes.push((index, position));
            }
            _ => {}
        }
    }

    if !seen_file {
        return Err("no FILE line".to_string());
    }

    // Byte positions: within one file, each index point is the previous
    // one plus the sectors between them at the stride of the track the
    // previous point belongs to.
    let mut tracks = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        let Some(&(_, start)) = entry.indexes.iter().find(|(index, _)| *index == 1) else {
            return Err(format!("track {} has no INDEX 01", entry.number));
        };

        let points = entries[..=i]
            .iter()
            .enumerate()
            .filter(|(_, e)| e.file == entry.file)
            .flat_map(|(j, e)| {
                e.indexes
                    .iter()
                    .map(move |&(index, at)| (j, index, at, e.stride))
            });
        let mut previous: Option<(u64, u32)> = None;
        let mut bytes = 0u64;
        for (j, index, at, stride) in points {
            bytes = match previous {
                Some((before, before_stride)) => {
                    bytes + at.saturating_sub(before) * u64::from(before_stride)
                }
                None => at * u64::from(stride),
            };
            previous = Some((at, stride));
            if j == i && index == 1 {
                break;
            }
        }
        let start_byte = bytes;

        let next = entries[i + 1..].iter().find(|e| e.file == entry.file);
        let sectors = match next.and_then(|e| e.indexes.first()) {
            Some(&(_, end)) => end.saturating_sub(start),
            None => {
                let len = file_len(&entry.file)
                    .ok_or_else(|| format!("{} is missing", entry.file.display()))?;
                len.saturating_sub(start_byte) / u64::from(entry.stride)
            }
        };

        tracks.push(Track {
            number: entry.number,
            kind: entry.kind,
            file: entry.file.clone(),
            start: start_byte,
            sectors,
            stride: entry.stride,
        });
    }

    Ok(tracks)
}

/// `"name with spaces" TYPE` or `name TYPE`.
fn file_name_and_type(rest: &str) -> Option<(&str, &str)> {
    if let Some(quoted) = rest.strip_prefix('"') {
        let (name, after) = quoted.split_once('"')?;
        return Some((name, after.trim()));
    }
    let (name, kind) = rest.rsplit_once(char::is_whitespace)?;
    Some((name.trim(), kind.trim()))
}

/// `mm:ss:ff` as a sector count.
fn msf(text: &str) -> Option<u64> {
    let mut parts = text.split(':').map(|p| p.parse::<u64>().ok());
    let (m, s, f) = (parts.next()??, parts.next()??, parts.next()??);
    (s < 60 && f < 75 && parts.next().is_none()).then_some((m * 60 + s) * 75 + f)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lengths<'a>(files: &'a [(&'a str, u64)]) -> impl Fn(&Path) -> Option<u64> + 'a {
        move |path| {
            files
                .iter()
                .find(|(name, _)| path.ends_with(name))
                .map(|(_, len)| *len)
        }
    }

    #[test]
    fn one_file_with_data_then_audio() {
        let sheet = "FILE \"Recoil Disc.bin\" BINARY\n\
                     \x20 TRACK 01 MODE1/2352\n\
                     \x20   INDEX 01 00:00:00\n\
                     \x20 TRACK 02 AUDIO\n\
                     \x20   INDEX 00 00:02:00\n\
                     \x20   INDEX 01 00:04:00\n";
        // 2 s of data, then 2 s of pregap and 3 s of audio.
        let len = (2 + 2 + 3) * 75 * 2352;

        let tracks = parse(
            sheet,
            Path::new("/d"),
            &lengths(&[("Recoil Disc.bin", len)]),
        )
        .unwrap();

        assert_eq!(tracks.len(), 2);
        assert_eq!(tracks[0].file, Path::new("/d/Recoil Disc.bin"));
        assert_eq!(
            (tracks[0].kind, tracks[0].start, tracks[0].sectors),
            (TrackKind::Data, 0, 150)
        );
        assert_eq!(
            (tracks[1].kind, tracks[1].start, tracks[1].sectors),
            (TrackKind::Audio, 300 * 2352, 225)
        );
    }

    #[test]
    fn one_file_per_track() {
        let sheet = "FILE \"t1.bin\" BINARY\n  TRACK 01 MODE1/2048\n    INDEX 01 00:00:00\n\
                     FILE \"t2.bin\" BINARY\n  TRACK 02 AUDIO\n    INDEX 00 00:00:00\n    INDEX 01 00:02:00\n";

        let tracks = parse(
            sheet,
            Path::new("/d"),
            &lengths(&[("t1.bin", 10 * 2048), ("t2.bin", (150 + 75) * 2352)]),
        )
        .unwrap();

        assert_eq!((tracks[0].stride, tracks[0].sectors), (2048, 10));
        assert_eq!((tracks[1].start, tracks[1].sectors), (150 * 2352, 75));
    }

    #[test]
    fn mixed_strides_in_one_file_add_up() {
        let sheet = "FILE disc.bin BINARY\n  TRACK 01 MODE1/2048\n    INDEX 01 00:00:00\n\
                     \x20 TRACK 02 AUDIO\n    INDEX 01 00:01:00\n";

        let tracks = parse(
            sheet,
            Path::new("/d"),
            &lengths(&[("disc.bin", 75 * 2048 + 75 * 2352)]),
        )
        .unwrap();

        assert_eq!((tracks[1].start, tracks[1].sectors), (75 * 2048, 75));
    }

    #[test]
    fn tracks_in_audio_files_are_left_out() {
        let sheet = "FILE data.bin BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n\
                     FILE \"Track 02.wav\" WAVE\n  TRACK 02 AUDIO\n    INDEX 01 00:00:00\n";

        let tracks = parse(sheet, Path::new("/d"), &lengths(&[("data.bin", 2352 * 10)])).unwrap();

        assert_eq!(tracks.len(), 1);
    }

    #[test]
    fn a_track_without_index_01_is_an_error() {
        let sheet = "FILE d.bin BINARY\n  TRACK 01 MODE1/2352\n    INDEX 00 00:00:00\n";

        let err = parse(sheet, Path::new("/d"), &lengths(&[("d.bin", 2352)])).unwrap_err();

        assert!(err.contains("INDEX 01"), "{err}");
    }

    #[test]
    fn msf_counts_75_frames_a_second() {
        assert_eq!(msf("00:00:00"), Some(0));
        assert_eq!(msf("01:02:03"), Some((62 * 75) + 3));
        assert_eq!(msf("00:60:00"), None);
        assert_eq!(msf("00:00:75"), None);
    }
}
