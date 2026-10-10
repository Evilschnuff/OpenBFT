//! The on-disk config file, as far as the data paths and audio need it.
//!
//! DESIGN §9 puts `config.ron` in the platform config directory and gives it
//! the data root, cache directory, unzbd path, video, audio, keybinds and
//! player name. The three path keys and [`AudioSettings`] are here; the
//! settings UI in `bft-game` adds the rest to this same struct so everything
//! reads one file.
//!
//! Every key is optional and unknown keys are ignored, so a file written by
//! a later version still loads.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The user's persisted settings.
// Unknown keys are ignored by default in serde, which is what FR-D6 wants: a
// file written by a later version still loads here.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The Recoil install to read, tried after the CLI argument and
    /// `BFT_DATA_DIR` (FR-D1).
    pub data_root: Option<PathBuf>,
    /// Where to put the extraction cache, overriding the platform default.
    pub cache_dir: Option<PathBuf>,
    /// The `unzbd` binary to drive extraction with.
    pub unzbd: Option<PathBuf>,
    /// Volumes, mute and the music directory.
    pub audio: AudioSettings,
}

/// The audio half of the settings (FR-S1–S3).
///
/// A `Resource` as well, so the running game reads the live values and a
/// settings screen can change them without a restart. Volumes are linear
/// gains, 0–1.
#[derive(bevy::prelude::Resource, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// Scales everything.
    pub master: f32,
    /// Sound effects, engines and speech.
    pub effects: f32,
    /// Music from [`Self::music_dir`].
    pub music: f32,
    /// Silences everything without losing the volumes.
    pub muted: bool,
    /// Where the user's music is: `.ogg`, `.mp3`, `.wav` or `.flac`, for
    /// instance the four CD-audio tracks ripped from the original disc.
    /// `None` plays no music.
    pub music_dir: Option<PathBuf>,
}

impl Default for AudioSettings {
    fn default() -> AudioSettings {
        AudioSettings {
            master: 1.0,
            effects: 1.0,
            music: 0.6,
            muted: false,
            music_dir: None,
        }
    }
}

impl AudioSettings {
    /// The gain effects play at: master × effects, or 0 when muted.
    pub fn effects_gain(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            (self.master * self.effects).clamp(0.0, 1.0)
        }
    }

    /// The gain music plays at: master × music, or 0 when muted.
    pub fn music_gain(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            (self.master * self.music).clamp(0.0, 1.0)
        }
    }
}

/// Why a config file could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The platform has no config directory (no `HOME`, typically).
    #[error("no config directory is available on this platform")]
    NoConfigDir,
    /// The file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file that failed.
        path: PathBuf,
        /// What the filesystem said.
        #[source]
        source: std::io::Error,
    },
    /// The file is not valid RON.
    ///
    /// The parse error is boxed: `SpannedError` is over 100 bytes, which
    /// would make every `Result` in this module that wide.
    #[error("{path}: {source}")]
    Parse {
        /// The file that failed.
        path: PathBuf,
        /// What the parser said, with line and column.
        #[source]
        source: Box<ron::error::SpannedError>,
    },
    /// The config could not be serialised.
    #[error("cannot serialise the config: {0}")]
    Serialise(#[from] Box<ron::Error>),
}

impl Config {
    /// Read the config from the platform config directory.
    ///
    /// A missing file is not an error: it yields the default config, because
    /// running with no config at all is the normal first-launch case.
    pub fn load() -> Result<Config, ConfigError> {
        let path = Self::default_path().ok_or(ConfigError::NoConfigDir)?;
        Self::load_from(&path)
    }

    /// Read the config from a specific file, defaulting when it is absent.
    pub fn load_from(path: &Path) -> Result<Config, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Config::default()),
            Err(source) => {
                return Err(ConfigError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };

        ron::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source: Box::new(source),
        })
    }

    /// Write the config to a specific file, creating parent directories.
    pub fn save_to(&self, path: &Path) -> Result<(), ConfigError> {
        let text = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
            .map_err(Box::new)?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| ConfigError::Io {
                path: parent.to_path_buf(),
                source,
            })?;
        }

        std::fs::write(path, text).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })
    }

    /// `config.ron` in the platform config directory, or `None` when the
    /// platform will not name one.
    pub fn default_path() -> Option<PathBuf> {
        crate::cache::project_dirs().map(|d| d.config_dir().join("config.ron"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_is_the_default_config() {
        let dir = crate::test_support::TempDir::new();

        let config = Config::load_from(&dir.path().join("config.ron")).unwrap();

        assert_eq!(config, Config::default());
    }

    #[test]
    fn a_config_round_trips() {
        let dir = crate::test_support::TempDir::new();
        let path = dir.path().join("config.ron");
        let config = Config {
            data_root: Some(PathBuf::from("/games/recoil")),
            cache_dir: None,
            unzbd: Some(PathBuf::from("/usr/local/bin/unzbd")),
            audio: AudioSettings {
                effects: 0.5,
                music_dir: Some(PathBuf::from("/music/recoil")),
                ..AudioSettings::default()
            },
        };

        config.save_to(&path).unwrap();

        assert_eq!(Config::load_from(&path).unwrap(), config);
    }

    #[test]
    fn a_partial_file_leaves_the_rest_default() {
        let dir = crate::test_support::TempDir::new();
        let path = dir.path().join("config.ron");
        std::fs::write(&path, r#"(data_root: Some("/games/recoil"))"#).unwrap();

        let config = Config::load_from(&path).unwrap();

        assert_eq!(config.data_root, Some(PathBuf::from("/games/recoil")));
        assert_eq!(config.unzbd, None);
        assert_eq!(config.audio, AudioSettings::default());
    }

    #[test]
    fn muting_zeroes_both_gains_and_keeps_the_volumes() {
        let mut audio = AudioSettings {
            master: 0.5,
            effects: 0.8,
            music: 1.0,
            ..AudioSettings::default()
        };
        assert!((audio.effects_gain() - 0.4).abs() < 1e-6);
        assert!((audio.music_gain() - 0.5).abs() < 1e-6);

        audio.muted = true;
        assert_eq!(audio.effects_gain(), 0.0);
        assert_eq!(audio.music_gain(), 0.0);
        assert_eq!(audio.effects, 0.8);
    }

    #[test]
    fn a_broken_file_names_itself_in_the_error() {
        let dir = crate::test_support::TempDir::new();
        let path = dir.path().join("config.ron");
        std::fs::write(&path, "(data_root: Some(").unwrap();

        let err = Config::load_from(&path).unwrap_err();

        assert!(err.to_string().contains("config.ron"), "{err}");
    }
}
