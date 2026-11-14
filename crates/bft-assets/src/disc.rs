//! Reading a disc image: its tracks, the data track's files, and the CD
//! audio as WAV.
//!
//! Only the containers are read here: ISO 9660 (ECMA-119), Nero's chunk
//! list and cue sheets. The installer cabinet on the data track is
//! InstallShield's format, which this crate leaves to `unshield`
//! ([`crate::cabinet`]), as it leaves the game's archives to mech3ax.
//!
//! What the retail disc holds is in docs/data/install.md, "The disc and its
//! installer cabinet".

pub mod cue;
pub mod iso9660;
pub mod nrg;

use std::fs::File;
use std::io::{BufWriter, Read as _, Seek as _, SeekFrom, Write as _};
use std::path::{Path, PathBuf};

use crate::source::ImageFormat;

/// Why an image could not be read.
#[derive(Debug, thiserror::Error)]
pub enum DiscError {
    /// The file system said no.
    #[error("{path}: {source}")]
    Io {
        /// The file that failed.
        path: PathBuf,
        /// What the OS said.
        #[source]
        source: std::io::Error,
    },
    /// The image's structure is not what its format promises.
    #[error("{path}: {reason}")]
    Format {
        /// The image or sheet.
        path: PathBuf,
        /// What was wrong, in words.
        reason: String,
    },
    /// A file on the data track runs past the end of the track: the image
    /// was cut short (docs/data/install.md has one such file).
    #[error(
        "{name} is cut short in the image: it needs sectors up to {needs}, the track has {has}"
    )]
    Truncated {
        /// The file's path on the disc.
        name: String,
        /// The sector its data runs to.
        needs: u64,
        /// How many sectors the track holds.
        has: u64,
    },
}

impl DiscError {
    fn io(path: &Path) -> impl FnOnce(std::io::Error) -> DiscError + '_ {
        move |source| DiscError::Io {
            path: path.to_path_buf(),
            source,
        }
    }

    fn format(path: &Path, reason: impl Into<String>) -> DiscError {
        DiscError::Format {
            path: path.to_path_buf(),
            reason: reason.into(),
        }
    }
}

/// Whether a track is data or CD audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    /// A data track: 2048 bytes of user data per sector.
    Data,
    /// CD audio: 2352 bytes per sector, 44.1 kHz 16-bit stereo.
    Audio,
}

/// One track of an image, as bytes in a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// The track number on the disc, from 1.
    pub number: u8,
    /// Data or audio.
    pub kind: TrackKind,
    /// The file the sectors are in.
    pub file: PathBuf,
    /// Where the track's first sector (index 1) starts in `file`.
    pub start: u64,
    /// How many sectors the track has.
    pub sectors: u64,
    /// Bytes per sector as stored: 2048, 2336, 2352, or 2448 with
    /// subchannel data.
    pub stride: u32,
}

/// Bytes of audio in a CD sector.
pub const AUDIO_SECTOR: u32 = 2352;
/// Bytes of user data in a data sector.
pub const DATA_SECTOR: u32 = 2048;

/// The tracks of the image at `path`, in disc order.
pub fn tracks(path: &Path, format: ImageFormat) -> Result<Vec<Track>, DiscError> {
    let tracks = match format {
        ImageFormat::Nrg => nrg::tracks(path)?,
        ImageFormat::Cue => cue::tracks(path)?,
        ImageFormat::Iso | ImageFormat::RawBin => {
            let len = std::fs::metadata(path).map_err(DiscError::io(path))?.len();
            let stride = if format == ImageFormat::Iso {
                DATA_SECTOR
            } else {
                AUDIO_SECTOR
            };
            vec![Track {
                number: 1,
                kind: TrackKind::Data,
                file: path.to_path_buf(),
                start: 0,
                sectors: len / u64::from(stride),
                stride,
            }]
        }
    };

    if tracks.is_empty() {
        return Err(DiscError::format(path, "the image lists no tracks"));
    }
    Ok(tracks)
}

/// A data track opened for reading 2048-byte sectors.
pub struct DataTrack {
    file: File,
    path: PathBuf,
    start: u64,
    sectors: u64,
    stride: u64,
    /// Where the 2048 user bytes begin inside a stored sector.
    payload: u64,
}

impl DataTrack {
    /// Open `track`, working out where a sector's user data sits.
    pub fn open(track: &Track) -> Result<DataTrack, DiscError> {
        if track.kind != TrackKind::Data {
            return Err(DiscError::format(
                &track.file,
                format!("track {} is audio, not data", track.number),
            ));
        }

        let mut file = File::open(&track.file).map_err(DiscError::io(&track.file))?;
        let payload = match track.stride {
            2048 => 0,
            // Mode 2 without sync and header: an 8-byte subheader first.
            2336 => 8,
            // Raw: 12 bytes of sync, a 4-byte header whose last byte is the
            // mode, and in mode 2 an 8-byte subheader.
            2352 | 2448 => {
                let mut header = [0u8; 16];
                file.seek(SeekFrom::Start(track.start))
                    .and_then(|_| file.read_exact(&mut header))
                    .map_err(DiscError::io(&track.file))?;
                match header[15] {
                    1 => 16,
                    2 => 24,
                    mode => {
                        return Err(DiscError::format(
                            &track.file,
                            format!("track {} has data sectors of mode {mode}", track.number),
                        ));
                    }
                }
            }
            stride => {
                return Err(DiscError::format(
                    &track.file,
                    format!("track {} has {stride}-byte sectors", track.number),
                ));
            }
        };

        Ok(DataTrack {
            file,
            path: track.file.clone(),
            start: track.start,
            sectors: track.sectors,
            stride: u64::from(track.stride),
            payload,
        })
    }

    /// How many sectors the track holds.
    pub fn sectors(&self) -> u64 {
        self.sectors
    }

    /// Read sector `lba`'s 2048 bytes into `buf`.
    pub fn read_sector(&mut self, lba: u64, buf: &mut [u8; 2048]) -> Result<(), DiscError> {
        if lba >= self.sectors {
            return Err(DiscError::format(
                &self.path,
                format!("sector {lba} is past the track's {}", self.sectors),
            ));
        }
        self.file
            .seek(SeekFrom::Start(
                self.start + lba * self.stride + self.payload,
            ))
            .and_then(|_| self.file.read_exact(buf))
            .map_err(DiscError::io(&self.path))
    }
}

/// What [`write_wav`] wrote.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WavWritten {
    /// The track's length.
    pub seconds: f64,
    /// Whether the image stored the samples big-endian and they were
    /// swapped.
    pub swapped: bool,
}

/// Write audio `track` to `dest` as a 44.1 kHz 16-bit stereo WAV.
///
/// The byte order is measured, not assumed: images differ (a cue sheet's
/// `MOTOROLA` files and some Nero versions are big-endian), and music read
/// in the right order changes little from one sample to the next, while
/// the wrong order is noise (docs/data/install.md measures the retail
/// image this way).
pub fn write_wav(track: &Track, dest: &Path) -> Result<WavWritten, DiscError> {
    if track.kind != TrackKind::Audio {
        return Err(DiscError::format(
            &track.file,
            format!("track {} is data, not audio", track.number),
        ));
    }

    let mut file = File::open(&track.file).map_err(DiscError::io(&track.file))?;
    let swapped = is_big_endian(&mut file, track).map_err(DiscError::io(&track.file))?;

    let data_len = track.sectors * u64::from(AUDIO_SECTOR);
    let data_len32 = u32::try_from(data_len)
        .ok()
        .filter(|len| *len <= u32::MAX - 36)
        .ok_or_else(|| {
            DiscError::format(
                &track.file,
                format!("track {} is too long for a WAV", track.number),
            )
        })?;

    // Written beside the destination and renamed at the end, so an
    // interrupted run never leaves a short file that looks finished.
    let partial = dest.with_extension("wav.partial");
    let out = File::create(&partial).map_err(DiscError::io(&partial))?;
    let mut out = BufWriter::new(out);
    let write_err = DiscError::io(&partial);

    let result = (|| -> std::io::Result<()> {
        out.write_all(&wav_header(data_len32))?;
        let mut sector = vec![0u8; track.stride as usize];
        for n in 0..track.sectors {
            file.seek(SeekFrom::Start(track.start + n * u64::from(track.stride)))?;
            file.read_exact(&mut sector)?;
            let audio = &mut sector[..AUDIO_SECTOR as usize];
            if swapped {
                for pair in audio.as_chunks_mut::<2>().0 {
                    pair.swap(0, 1);
                }
            }
            out.write_all(audio)?;
        }
        out.flush()
    })();
    result.map_err(write_err)?;
    drop(out);

    std::fs::rename(&partial, dest).map_err(DiscError::io(dest))?;

    Ok(WavWritten {
        seconds: track.sectors as f64 / 75.0,
        swapped,
    })
}

/// A canonical 44-byte header for 44.1 kHz 16-bit stereo PCM.
fn wav_header(data_len: u32) -> [u8; 44] {
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36 + data_len).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes()); // integer PCM
    h[22..24].copy_from_slice(&2u16.to_le_bytes()); // stereo
    h[24..28].copy_from_slice(&44_100u32.to_le_bytes());
    h[28..32].copy_from_slice(&(44_100u32 * 4).to_le_bytes());
    h[32..34].copy_from_slice(&4u16.to_le_bytes()); // block align
    h[34..36].copy_from_slice(&16u16.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_len.to_le_bytes());
    h
}

/// Whether the track's samples are big-endian: read one second from the
/// middle both ways and keep the smoother.
fn is_big_endian(file: &mut File, track: &Track) -> std::io::Result<bool> {
    let take = track.sectors.min(75);
    let first = track.sectors.saturating_sub(take) / 2;
    let mut bytes = Vec::with_capacity((take * u64::from(AUDIO_SECTOR)) as usize);
    let mut sector = vec![0u8; track.stride as usize];
    for n in first..first + take {
        file.seek(SeekFrom::Start(track.start + n * u64::from(track.stride)))?;
        file.read_exact(&mut sector)?;
        bytes.extend_from_slice(&sector[..AUDIO_SECTOR as usize]);
    }

    let little = roughness(&bytes, i16::from_le_bytes);
    let big = roughness(&bytes, i16::from_be_bytes);

    // Silence reads the same both ways; keep the usual order.
    Ok(big < little)
}

/// The mean step between consecutive left-channel samples over their mean
/// level: small for sound, near or above 1 for noise.
fn roughness(bytes: &[u8], sample: fn([u8; 2]) -> i16) -> f64 {
    let left: Vec<f64> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|frame| f64::from(sample([frame[0], frame[1]])))
        .collect();
    if left.len() < 2 {
        return 0.0;
    }

    let level = left.iter().map(|s| s.abs()).sum::<f64>() / left.len() as f64;
    if level == 0.0 {
        return 0.0;
    }
    let step = left.windows(2).map(|w| (w[1] - w[0]).abs()).sum::<f64>() / (left.len() - 1) as f64;

    step / level
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// One second of a 440 Hz tone as CD audio, in the given byte order.
    pub(crate) fn tone(sectors: usize, big_endian: bool) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(sectors * AUDIO_SECTOR as usize);
        for n in 0..sectors * AUDIO_SECTOR as usize / 4 {
            let t = n as f64 / 44_100.0;
            let s = (8000.0 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as i16;
            let pair = if big_endian {
                s.to_be_bytes()
            } else {
                s.to_le_bytes()
            };
            bytes.extend_from_slice(&pair);
            bytes.extend_from_slice(&pair);
        }
        bytes
    }

    fn audio_track(path: &Path, sectors: u64) -> Track {
        Track {
            number: 2,
            kind: TrackKind::Audio,
            file: path.to_path_buf(),
            start: 0,
            sectors,
            stride: AUDIO_SECTOR,
        }
    }

    #[test]
    fn a_wav_has_a_header_the_music_player_accepts() {
        let dir = TempDir::new();
        let image = dir.path().join("audio.bin");
        std::fs::write(&image, tone(75, false)).unwrap();
        let dest = dir.path().join("track02.wav");

        let written = write_wav(&audio_track(&image, 75), &dest).unwrap();

        let wav = std::fs::read(&dest).unwrap();
        let info = crate::sounds::wav_info(&wav).expect("a playable WAV");
        assert_eq!(
            (info.channels, info.sample_rate, info.bits_per_sample),
            (2, 44_100, 16)
        );
        assert_eq!(wav.len(), 44 + 75 * 2352);
        assert!((written.seconds - 1.0).abs() < 1e-9);
        assert!(!written.swapped);
        assert!(!dir.path().join("track02.wav.partial").exists());
    }

    #[test]
    fn big_endian_audio_is_detected_and_swapped() {
        let dir = TempDir::new();
        let image = dir.path().join("audio.bin");
        std::fs::write(&image, tone(75, true)).unwrap();
        let dest = dir.path().join("track02.wav");

        let written = write_wav(&audio_track(&image, 75), &dest).unwrap();

        assert!(written.swapped);
        let wav = std::fs::read(&dest).unwrap();
        assert_eq!(&wav[44..], &tone(75, false)[..]);
    }

    #[test]
    fn subchannel_bytes_are_dropped() {
        let dir = TempDir::new();
        let image = dir.path().join("audio.bin");
        let audio = tone(3, false);
        let mut stored = Vec::new();
        for sector in audio.chunks(AUDIO_SECTOR as usize) {
            stored.extend_from_slice(sector);
            stored.extend_from_slice(&[0xAA; 96]);
        }
        std::fs::write(&image, stored).unwrap();
        let track = Track {
            stride: 2448,
            ..audio_track(&image, 3)
        };
        let dest = dir.path().join("track02.wav");

        write_wav(&track, &dest).unwrap();

        assert_eq!(&std::fs::read(&dest).unwrap()[44..], &audio[..]);
    }

    /// The retail disc image from `BFT_DISC_IMAGE`, checked against
    /// docs/data/install.md. Skips when the variable is unset.
    #[test]
    fn the_retail_image_reads_as_documented() {
        let Some(path) = std::env::var_os("BFT_DISC_IMAGE").map(PathBuf::from) else {
            return;
        };
        let Ok(crate::source::Source::DiscImage { path, format }) = crate::source::classify(&path)
        else {
            panic!("BFT_DISC_IMAGE is not a disc image");
        };

        let tracks = tracks(&path, format).unwrap();
        let kinds: Vec<_> = tracks.iter().map(|t| t.kind).collect();
        assert_eq!(
            kinds,
            [
                TrackKind::Data,
                TrackKind::Audio,
                TrackKind::Audio,
                TrackKind::Audio,
                TrackKind::Audio
            ]
        );
        assert_eq!(tracks[0].sectors, 216_497);
        // 3:35, 9:11, 8:47 and 7:20.
        let lengths: Vec<_> = tracks[1..]
            .iter()
            .map(|t| (t.sectors / 75) as u32)
            .collect();
        assert_eq!(lengths, [215, 550, 527, 440]);

        let mut volume = iso9660::Volume::open(DataTrack::open(&tracks[0]).unwrap()).unwrap();
        assert_eq!(volume.label, "RECOIL");
        let cab = volume.find("data1.cab").unwrap().expect("DATA1.CAB");
        assert_eq!(cab.size, 245_410_644);
        assert!(volume.find("data1.hdr").unwrap().is_none());

        let dir = TempDir::new();
        let wav = volume.find("setup1.wav").unwrap().expect("SETUP1.WAV");
        let err = volume
            .copy(&wav, &dir.path().join("setup1.wav"), |_| {})
            .unwrap_err();
        assert!(
            matches!(
                err,
                DiscError::Truncated {
                    needs: 216_646,
                    has: 216_497,
                    ..
                }
            ),
            "{err}"
        );

        let written = write_wav(&tracks[1], &dir.path().join("track02.wav")).unwrap();
        assert!(
            !written.swapped,
            "the retail image stores audio little-endian"
        );
    }

    #[test]
    fn a_raw_mode_1_sector_yields_its_user_data() {
        let dir = TempDir::new();
        let image = dir.path().join("data.bin");
        let mut sector = vec![0u8; 2352];
        sector[..12].copy_from_slice(&crate::source::SYNC);
        sector[15] = 1;
        sector[16..16 + 2048].fill(7);
        std::fs::write(&image, &sector).unwrap();
        let track = Track {
            number: 1,
            kind: TrackKind::Data,
            file: image,
            start: 0,
            sectors: 1,
            stride: 2352,
        };

        let mut data = DataTrack::open(&track).unwrap();
        let mut buf = [0u8; 2048];
        data.read_sector(0, &mut buf).unwrap();

        assert!(buf.iter().all(|b| *b == 7));
        assert!(data.read_sector(1, &mut buf).is_err());
    }
}
