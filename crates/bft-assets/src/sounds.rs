//! `sounds/soundsh.snd.zip` → [`SoundBank`]: the game's sound effects.
//!
//! `unzbd rc sounds` writes one `.wav` per archive entry plus a
//! `manifest.json`. The WAVs are ordinary mono PCM, 8- or 16-bit, at
//! 11–44 kHz (docs/data/sounds.md), which Bevy's own decoder plays, so each one
//! becomes a labelled `AudioSource` sub-asset keyed by its lower-cased stem.
//! What a logical name like `snd_exp_hit1` means is
//! [`reader::sounds`](crate::reader::sounds)'s business, not this module's.
//!
//! # Only one bank
//!
//! The install ships three archives, `soundsl`/`m`/`h`, holding the same
//! files at three fidelities with nothing unique to any of them
//! (docs/data/sounds.md). Only `soundsh` is extracted, so there is
//! one bank and no quality switch.
//!
//! # Why the WAVs are checked here
//!
//! `bevy_audio`'s decoder `unwrap`s: a file it cannot decode panics the audio
//! system the first time something plays it, which could be minutes into a
//! mission. So every entry's RIFF header is read at load time and anything
//! that is not PCM WAV is dropped with a warning, where a broken cache shows
//! up at once and costs one sound instead of the process.
//!
//! The asset types need Bevy's audio and so sit behind this crate's `audio`
//! feature; [`read_sound_archive`] and [`WavInfo`] do not, so a headless
//! build can still inspect an archive.

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use crate::reader::sounds::wav_stem;

/// The entry mech3ax writes beside the WAVs.
pub const MANIFEST_ENTRY: &str = "manifest.json";

/// The format fields of a PCM WAV's `fmt ` chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WavInfo {
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
}

/// Read a WAV header, or `None` when the bytes are not PCM WAV the decoder
/// can play.
///
/// Walks the RIFF chunks for `fmt ` rather than assuming it sits at byte 12:
/// the format allows other chunks first. Accepts integer PCM of 8, 16, 24 or
/// 32 bits, which is what rodio's WAV decoder handles.
pub fn wav_info(bytes: &[u8]) -> Option<WavInfo> {
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return None;
    }

    let mut at = 12usize;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes(bytes[at + 4..at + 8].try_into().ok()?) as usize;
        let body = at + 8;

        if id == b"fmt " {
            let fmt = bytes.get(body..body + 16)?;
            let format = u16::from_le_bytes([fmt[0], fmt[1]]);
            let info = WavInfo {
                channels: u16::from_le_bytes([fmt[2], fmt[3]]),
                sample_rate: u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]),
                bits_per_sample: u16::from_le_bytes([fmt[14], fmt[15]]),
            };
            // 1 = integer PCM. Anything compressed (ADPCM is 2) the decoder
            // refuses.
            let playable = format == 1
                && info.channels > 0
                && info.sample_rate > 0
                && matches!(info.bits_per_sample, 8 | 16 | 24 | 32);
            return playable.then_some(info);
        }

        // Chunks are padded to an even length.
        at = body.checked_add(size)?.checked_add(size & 1)?;
    }

    None
}

/// The WAVs of one sound archive, keyed by lower-cased stem.
#[derive(Debug, Default)]
pub struct SoundArchive {
    /// Every playable WAV.
    pub wavs: BTreeMap<String, Vec<u8>>,
    /// Entries that were `.wav` but would not decode, by archive name.
    pub rejected: Vec<String>,
}

/// Why a sound archive would not load.
#[derive(Debug, thiserror::Error)]
pub enum SoundBankError {
    #[error("cannot read the archive: {0}")]
    Read(#[from] std::io::Error),
    #[error("not a readable ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("no {MANIFEST_ENTRY} and no .wav entries; is this a sound archive?")]
    NotSounds,
}

/// Read every WAV out of a `.snd.zip`.
///
/// The manifest is not needed — the entries are the WAVs themselves and
/// mech3ax's per-entry metadata is opaque — but an archive with neither a
/// manifest nor a single WAV is the wrong file, and says so.
pub fn read_sound_archive(bytes: &[u8]) -> Result<SoundArchive, SoundBankError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut out = SoundArchive::default();
    let mut has_manifest = false;

    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_string();

        if name == MANIFEST_ENTRY {
            has_manifest = true;
            continue;
        }
        if !name.to_ascii_lowercase().ends_with(".wav") {
            continue;
        }

        let mut wav = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut wav)?;

        if wav_info(&wav).is_some() {
            out.wavs.insert(wav_stem(&name), wav);
        } else {
            out.rejected.push(name);
        }
    }

    if !has_manifest && out.wavs.is_empty() && out.rejected.is_empty() {
        return Err(SoundBankError::NotSounds);
    }

    Ok(out)
}

#[cfg(feature = "audio")]
pub use bank::{SoundBank, SoundBankLoader};

#[cfg(feature = "audio")]
mod bank {
    use std::collections::HashMap;

    use bevy::asset::io::Reader;
    use bevy::asset::{AssetLoader, LoadContext};
    use bevy::audio::AudioSource;
    use bevy::prelude::*;

    use super::{SoundBankError, read_sound_archive};

    /// Every sound effect, as playable audio.
    #[derive(Asset, TypePath, Debug, Default)]
    pub struct SoundBank {
        /// Keyed by WAV stem, lower-cased (`"exp_hit1"`).
        pub sounds: HashMap<String, Handle<AudioSource>>,
    }

    impl SoundBank {
        /// One sound by WAV stem, any case.
        pub fn get(&self, stem: &str) -> Option<&Handle<AudioSource>> {
            self.sounds
                .get(stem)
                .or_else(|| self.sounds.get(&stem.to_ascii_lowercase()))
        }
    }

    /// Loads `cache://sounds/soundsh.snd.zip`.
    ///
    /// Claims `snd.zip`, which nothing else in the cache uses.
    #[derive(TypePath, Debug, Default)]
    pub struct SoundBankLoader;

    impl AssetLoader for SoundBankLoader {
        type Asset = SoundBank;
        type Settings = ();
        type Error = SoundBankError;

        async fn load(
            &self,
            reader: &mut dyn Reader,
            _settings: &(),
            load_context: &mut LoadContext<'_>,
        ) -> Result<SoundBank, SoundBankError> {
            let mut bytes = Vec::new();
            reader.read_to_end(&mut bytes).await?;

            let archive = read_sound_archive(&bytes)?;
            if !archive.rejected.is_empty() {
                warn!(
                    "{}: {} entr(ies) are not PCM WAV and will be silent: {:?}",
                    load_context.path(),
                    archive.rejected.len(),
                    archive.rejected
                );
            }

            let mut bank = SoundBank::default();
            for (stem, wav) in archive.wavs {
                let handle =
                    load_context.add_labeled_asset(stem.clone(), AudioSource { bytes: wav.into() });
                bank.sounds.insert(stem, handle);
            }

            Ok(bank)
        }

        fn extensions(&self) -> &[&str] {
            &["snd.zip"]
        }
    }
}

/// A minimal PCM WAV: mono, 16-bit, `samples` of a quiet sine.
///
/// For tests here and in `bft-game` — synthesised so no test depends on
/// game data.
pub fn synth_wav(sample_rate: u32, samples: usize) -> Vec<u8> {
    let data_len = (samples * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..samples {
        let t = i as f32 / sample_rate as f32;
        let sample = ((t * 440.0 * std::f32::consts::TAU).sin() * 3000.0) as i16;
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_with(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buffer);
            let options = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, bytes) in files {
                zip.start_file(*name, options).expect("start");
                zip.write_all(bytes).expect("write");
            }
            zip.finish().expect("finish");
        }
        buffer.into_inner()
    }

    #[test]
    fn a_synthesised_wav_reads_back_its_format() {
        let wav = synth_wav(22050, 2205);
        assert_eq!(
            wav_info(&wav),
            Some(WavInfo {
                channels: 1,
                sample_rate: 22050,
                bits_per_sample: 16,
            })
        );
    }

    #[test]
    fn a_chunk_before_fmt_is_skipped() {
        let wav = synth_wav(11025, 10);
        // Splice an odd-sized `LIST` chunk (padded to even) in after `WAVE`.
        let mut spliced = wav[..12].to_vec();
        spliced.extend_from_slice(b"LIST");
        spliced.extend_from_slice(&3u32.to_le_bytes());
        spliced.extend_from_slice(&[1, 2, 3, 0]);
        spliced.extend_from_slice(&wav[12..]);
        assert_eq!(wav_info(&spliced).map(|i| i.sample_rate), Some(11025));
    }

    #[test]
    fn compressed_and_non_wav_data_are_refused() {
        let mut adpcm = synth_wav(22050, 10);
        adpcm[20] = 2;
        assert_eq!(wav_info(&adpcm), None);
        assert_eq!(wav_info(b"OggS\0\0\0\0\0\0\0\0\0\0"), None);
        assert_eq!(wav_info(b"RIFF"), None);
    }

    #[test]
    fn the_archive_is_keyed_by_lower_case_stem_and_drops_the_unplayable() {
        let wav = synth_wav(22050, 2205);
        let bytes = zip_with(&[
            (MANIFEST_ENTRY, b"[]"),
            ("Exp_Hit1.wav", &wav),
            ("broken.wav", b"not a wav"),
        ]);

        let archive = read_sound_archive(&bytes).expect("reads");
        assert_eq!(archive.wavs.keys().collect::<Vec<_>>(), vec!["exp_hit1"]);
        assert_eq!(archive.rejected, vec!["broken.wav".to_string()]);
    }

    #[test]
    fn an_archive_of_something_else_is_an_error() {
        let bytes = zip_with(&[("nodes.json", b"[]")]);
        assert!(matches!(
            read_sound_archive(&bytes),
            Err(SoundBankError::NotSounds)
        ));
    }
}
