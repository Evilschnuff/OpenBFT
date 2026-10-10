//! The extraction cache: where `unzbd` output lives and what is in it.
//!
//! The cache is derived game data and never enters the repository (FR-D7).
//! It defaults to the platform cache directory, keyed by a hash of the data
//! root so two installs do not share one cache, and is overridable from the
//! config file.
//!
//! # Layout
//!
//! Paths are relative to the cache directory. The suffixes are what the
//! asset loaders' `AssetLoader::extensions()` key on, so they are fixed here:
//!
//! ```text
//! manifest.json
//! gamez/m1-gamez.gamez.zip
//! anim/m1-anim.anim.zip
//! textures/m1-rtexture4.tex.zip, textures/image.tex.zip
//! reader/zrdr.rdr.zip, reader/m1-zrdr.rdr.zip
//! sounds/soundsh.snd.zip
//! interp.json
//! messages.json
//! zmap/m1.zmap.json
//! ```

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The directory holding extracted game data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheDir {
    /// The directory itself. Absolute, so Bevy's `FileAssetReader` resolves
    /// it independently of the executable's location.
    pub path: PathBuf,
}

impl CacheDir {
    /// Name a cache directory without touching the filesystem.
    pub fn new(path: impl Into<PathBuf>) -> CacheDir {
        CacheDir { path: path.into() }
    }

    /// The manifest's path inside this cache.
    pub fn manifest_path(&self) -> PathBuf {
        self.path.join(MANIFEST_NAME)
    }
}

/// What a previous extraction run produced (FR-D3, FR-D4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheManifest {
    /// `unzbd --version` output, so a later run can warn about a mismatch.
    pub mech3ax_version: String,
    /// The mech3ax commit, when the caller knows it. `unzbd --version` does
    /// not print one on the pinned build, so this is usually `None`.
    pub mech3ax_commit: Option<String>,
    /// When the run finished, RFC 3339 in UTC.
    pub created: String,
    /// One entry per output that was written successfully.
    pub entries: Vec<CacheEntry>,
}

/// One successfully extracted file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheEntry {
    /// What kind of archive this came from.
    pub kind: ExtractKind,
    /// The `.zbd` (or `.dll`/`.zmap`) it came from, absolute.
    pub source: PathBuf,
    /// The output, relative to the cache directory.
    pub output: PathBuf,
    /// The source's length when it was extracted.
    pub source_len: u64,
    /// The source's mtime, whole seconds since the Unix epoch.
    pub source_mtime: u64,
}

/// Which `unzbd rc` subcommand produces a kind of output (docs/data/install.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractKind {
    /// `soundsh.zbd` and friends: one WAV per entry.
    Sounds,
    /// `interp.zbd`: the build-time scene scripts, kept for their texture
    /// animation bindings (docs/data/interp.md).
    Interp,
    /// `zrdr.zbd`, global or per mission: the gameplay database.
    Reader,
    /// `messages.dll`: the localised string table.
    Messages,
    /// `rtexture4.zbd`, `image.zbd`: one PNG per texture.
    Textures,
    /// `gamez.zbd`: the level's nodes, models, materials and textures.
    Gamez,
    /// `anim.zbd`: the level's animation definitions.
    Anim,
    /// `mN.zmap`: the automap.
    // Named explicitly: `snake_case` would split this into `z_map`, and the
    // manifest is a file the catalog reads.
    #[serde(rename = "zmap")]
    ZMap,
}

impl ExtractKind {
    /// The `unzbd rc` subcommand that extracts this kind.
    pub fn subcommand(self) -> &'static str {
        match self {
            ExtractKind::Sounds => "sounds",
            ExtractKind::Interp => "interp",
            ExtractKind::Reader => "reader",
            ExtractKind::Messages => "messages",
            ExtractKind::Textures => "textures",
            ExtractKind::Gamez => "gamez",
            ExtractKind::Anim => "anim",
            ExtractKind::ZMap => "zmap",
        }
    }

    /// Whether a failure to extract this kind should abort the run.
    ///
    /// Nothing does: one broken level must not stop the other twelve, so
    /// `extract::run` collects failures and writes a manifest of what did
    /// work. Kept as a method so a future "this one is essential" case has
    /// somewhere to live.
    pub fn is_fatal(self) -> bool {
        false
    }
}

impl fmt::Display for ExtractKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.subcommand())
    }
}

/// The manifest's file name inside a cache directory.
pub const MANIFEST_NAME: &str = "manifest.json";

/// Why the cache could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum CacheError {
    /// The platform has no cache directory (no `HOME`, typically).
    #[error("no cache directory is available on this platform")]
    NoCacheDir,
    /// A file could not be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file that failed.
        path: PathBuf,
        /// What the filesystem said.
        #[source]
        source: std::io::Error,
    },
    /// The manifest is not valid JSON, or not a manifest.
    #[error("{path}: {source}")]
    Parse {
        /// The manifest that failed.
        path: PathBuf,
        /// What the parser said, with line and column.
        #[source]
        source: serde_json::Error,
    },
}

/// The platform directories this project writes to.
pub fn project_dirs() -> Option<directories::ProjectDirs> {
    directories::ProjectDirs::from("org", "open-bft", "open-bft")
}

/// Where a data root's cache goes by default.
///
/// `<platform cache dir>/<hash of the root path>`: one cache per install, so
/// switching between a retail and a demo tree does not mix their output.
/// Falls back to a `.open-bft-cache` directory beside the data root when the
/// platform will not name a cache directory.
pub fn default_cache_dir(root: &crate::root::DataRoot) -> PathBuf {
    let key = path_hash(&root.path);

    match project_dirs() {
        Some(dirs) => dirs.cache_dir().join(key),
        // No HOME: keep the engine usable rather than failing at startup.
        None => root.path.join(".open-bft-cache").join(key),
    }
}

/// A short, stable, filesystem-safe digest of a path.
///
/// FNV-1a rather than a cryptographic hash: this only has to avoid accidental
/// collisions between the handful of install trees one user has, and adding a
/// hashing crate for it would be a dependency the workspace does not need.
fn path_hash(path: &Path) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;

    let mut hash = OFFSET;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }

    format!("{hash:016x}")
}

/// Read the cache's manifest, or `None` when the cache has never been built.
///
/// A manifest that exists but does not parse is an error, not a `None`: it
/// means a half-written or foreign file is in the way, and silently
/// re-extracting 450 MB over it would hide that.
pub fn load_manifest(cache: &CacheDir) -> Result<Option<CacheManifest>, CacheError> {
    let path = cache.manifest_path();

    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(CacheError::Io { path, source }),
    };

    serde_json::from_str(&text)
        .map(Some)
        .map_err(|source| CacheError::Parse { path, source })
}

/// Write the cache's manifest, creating the cache directory.
pub fn save_manifest(cache: &CacheDir, manifest: &CacheManifest) -> Result<(), CacheError> {
    std::fs::create_dir_all(&cache.path).map_err(|source| CacheError::Io {
        path: cache.path.clone(),
        source,
    })?;

    let path = cache.manifest_path();
    let text = serde_json::to_string_pretty(manifest).map_err(|source| CacheError::Parse {
        path: path.clone(),
        source,
    })?;

    std::fs::write(&path, text).map_err(|source| CacheError::Io { path, source })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::root::{DataRoot, DataVersion};
    use crate::test_support::TempDir;

    fn a_manifest() -> CacheManifest {
        CacheManifest {
            mech3ax_version: "unzbd v0.0.0-test".to_string(),
            mech3ax_commit: None,
            created: "2000-01-01T00:00:00Z".to_string(),
            entries: vec![CacheEntry {
                kind: ExtractKind::Gamez,
                source: PathBuf::from("/games/recoil/zbd/m1/gamez.zbd"),
                output: PathBuf::from("gamez/m1-gamez.gamez.zip"),
                source_len: 42,
                source_mtime: 1_700_000_000,
            }],
        }
    }

    #[test]
    fn an_unbuilt_cache_has_no_manifest() {
        let dir = TempDir::new();

        let manifest = load_manifest(&CacheDir::new(dir.path())).unwrap();

        assert!(manifest.is_none());
    }

    #[test]
    fn a_manifest_round_trips() {
        let dir = TempDir::new();
        let cache = CacheDir::new(dir.path());

        save_manifest(&cache, &a_manifest()).unwrap();

        assert_eq!(load_manifest(&cache).unwrap(), Some(a_manifest()));
    }

    #[test]
    fn a_broken_manifest_is_an_error_not_a_rebuild() {
        let dir = TempDir::new();
        let cache = CacheDir::new(dir.path());
        std::fs::write(cache.manifest_path(), "{ not json").unwrap();

        let err = load_manifest(&cache).unwrap_err();

        assert!(err.to_string().contains(MANIFEST_NAME), "{err}");
    }

    #[test]
    fn the_cache_dir_is_stable_per_root_and_differs_between_roots() {
        let one = DataRoot {
            path: PathBuf::from("/games/recoil"),
            version: DataVersion::Retail,
        };
        let two = DataRoot {
            path: PathBuf::from("/games/recoil-demo"),
            version: DataVersion::Demo,
        };

        assert_eq!(default_cache_dir(&one), default_cache_dir(&one));
        assert_ne!(default_cache_dir(&one), default_cache_dir(&two));
    }

    #[test]
    fn the_hash_is_hex_and_short_enough_for_any_filesystem() {
        let hash = path_hash(Path::new("/games/recoil"));

        assert_eq!(hash.len(), 16);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn kinds_serialise_as_their_subcommand_name() {
        // The manifest is read back by the catalog, so these strings are
        // fixed. `ZMap` is the one that needs help: `snake_case` alone
        // would write `z_map`.
        for kind in crate::extract::ALL_KINDS {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.subcommand()));

            let back: ExtractKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, kind);
        }
    }

    #[test]
    fn every_kind_names_a_real_unzbd_subcommand() {
        // docs/data/install.md: the subcommand set `unzbd rc` accepts.
        const SUBCOMMANDS: [&str; 8] = [
            "sounds", "interp", "reader", "messages", "textures", "gamez", "anim", "zmap",
        ];

        for kind in crate::extract::ALL_KINDS {
            assert!(SUBCOMMANDS.contains(&kind.subcommand()), "{kind:?}");
        }
    }
}
