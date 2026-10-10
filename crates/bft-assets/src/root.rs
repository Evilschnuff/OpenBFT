//! Finding and validating the user's Recoil install (FR-D1, FR-D2).
//!
//! Nothing here reads game data; it only checks that the files the
//! extraction step will hand to `unzbd` are where they are expected to be.
//! The layout it checks against is `docs/data/install.md`, verified against
//! a retail install.

use std::fmt;
use std::path::{Path, PathBuf};

/// A validated Recoil install directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataRoot {
    /// The install directory itself, the one holding `zbd/` and `maps/`.
    pub path: PathBuf,
    /// Which release this looks like, as far as [`validate`] can tell.
    pub version: DataVersion,
}

/// Which Recoil release a data root holds.
///
/// Only the retail layout is described in docs/data/install.md; `Demo` is reserved
/// for a release we have not seen, and `Unknown` is what a root that
/// validates but does not match a known shape reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataVersion {
    /// The full retail release: 13 mission directories under `zbd/`.
    Retail,
    /// A cut-down release with fewer missions.
    Demo,
    /// Validates, but the mission count matches nothing we know.
    Unknown,
}

impl fmt::Display for DataVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            DataVersion::Retail => "retail",
            DataVersion::Demo => "demo",
            DataVersion::Unknown => "unknown",
        };
        f.write_str(name)
    }
}

/// Why [`discover`] found no usable data root.
///
/// Carries everything FR-D1 asks the error message to list: every path that
/// was tried, and — when a path existed but did not validate — what it was
/// missing.
#[derive(Debug, Clone)]
pub struct DataRootError {
    /// Every candidate path, in the order it was tried.
    pub looked_in: Vec<PathBuf>,
    /// Items missing from the most promising candidate, if there was one.
    pub missing: Vec<String>,
}

impl fmt::Display for DataRootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "no Recoil install found.")?;
        writeln!(f)?;
        writeln!(f, "Looked in:")?;
        for path in &self.looked_in {
            writeln!(f, "  {}", path.display())?;
        }
        if !self.missing.is_empty() {
            writeln!(f)?;
            writeln!(f, "The closest match was missing:")?;
            for item in &self.missing {
                writeln!(f, "  {item}")?;
            }
        }
        writeln!(f)?;
        write!(
            f,
            "Point the engine at an install with --data-dir <path>, the \
             BFT_DATA_DIR environment variable, or the data_root key in the \
             config file."
        )
    }
}

impl std::error::Error for DataRootError {}

/// Locate the data root per FR-D1: CLI argument, `BFT_DATA_DIR`, config
/// file, then the conventional install paths for this platform.
///
/// The config file is read by [`crate::config::Config::load`]; a root named
/// there is tried before the conventional paths. Every candidate that exists
/// is validated, and the first one that validates wins.
///
/// An explicit `cli` path is the exception: if it does not validate, the
/// search stops there rather than quietly using some other install the user
/// did not ask for.
pub fn discover(cli: Option<&Path>) -> Result<DataRoot, DataRootError> {
    let config_root = crate::config::Config::load().ok().and_then(|c| c.data_root);
    discover_in(
        cli,
        std::env::var_os("BFT_DATA_DIR").map(PathBuf::from),
        config_root,
        &conventional_paths(),
    )
}

/// The candidate list behind [`discover`], with every source injected.
///
/// Split out so tests can drive discovery without touching the environment,
/// the user's config file or the real filesystem layout.
pub(crate) fn discover_in(
    cli: Option<&Path>,
    env: Option<PathBuf>,
    config: Option<PathBuf>,
    conventional: &[PathBuf],
) -> Result<DataRoot, DataRootError> {
    // An explicit path is a decision, not a suggestion: when it is wrong the
    // user wants to hear about *that* path, not to be handed a different
    // install that happened to be in a conventional place.
    if let Some(cli) = cli {
        return match validate(cli) {
            Ok(version) => Ok(DataRoot {
                path: cli.to_path_buf(),
                version,
            }),
            Err(missing) => Err(DataRootError {
                looked_in: vec![cli.to_path_buf()],
                missing,
            }),
        };
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    candidates.extend(env);
    candidates.extend(config);
    candidates.extend_from_slice(conventional);

    // Remember the missing list of the candidate that got furthest, so the
    // error can say "you have an install here, but it is incomplete" rather
    // than only "nothing found".
    let mut best: Option<Vec<String>> = None;

    for candidate in &candidates {
        match validate(candidate) {
            Ok(version) => {
                return Ok(DataRoot {
                    path: candidate.clone(),
                    version,
                });
            }
            Err(missing) => {
                let better = best.as_ref().is_none_or(|b| missing.len() < b.len());
                // A path that does not exist at all is not a near miss; it
                // reports every item as missing and would drown out a real
                // install with one file gone.
                if better && candidate.is_dir() {
                    best = Some(missing);
                }
            }
        }
    }

    Err(DataRootError {
        looked_in: candidates,
        missing: best.unwrap_or_default(),
    })
}

/// Check a directory for the files FR-D2 requires, returning what is missing.
///
/// The list is docs/data/install.md's: the `zbd/` and `maps/` directories, the
/// message DLL, and `zbd/m1/gamez.zbd` as proof that at least one mission is
/// installed. The DLL is `messages.dll` on the retail release; REQUIREMENTS
/// FR-D2 also allows `strings.dll`, so either satisfies the check.
pub fn validate(path: &Path) -> Result<DataVersion, Vec<String>> {
    let mut missing = Vec::new();

    if !path.join("zbd").is_dir() {
        missing.push("zbd/ (the archive directory)".to_string());
    }
    if !path.join("maps").is_dir() {
        missing.push("maps/ (the automap directory)".to_string());
    }
    if !path.join("messages.dll").is_file() && !path.join("strings.dll").is_file() {
        missing.push("messages.dll or strings.dll (localised strings)".to_string());
    }
    if !path.join("zbd").join("m1").join("gamez.zbd").is_file() {
        missing.push("zbd/m1/gamez.zbd (the first mission)".to_string());
    }

    if missing.is_empty() {
        Ok(version_of(path))
    } else {
        Err(missing)
    }
}

/// Guess the release from how many mission directories are installed.
fn version_of(path: &Path) -> DataVersion {
    let missions = (1..=MAX_MISSION)
        .filter(|n| path.join("zbd").join(format!("m{n}")).is_dir())
        .count();

    match missions {
        // docs/data/install.md: retail ships m1-m13, six campaigns and seven
        // multiplayer maps.
        13 => DataVersion::Retail,
        1..=6 => DataVersion::Demo,
        _ => DataVersion::Unknown,
    }
}

/// The highest mission number the game ships (docs/data/install.md).
pub const MAX_MISSION: u16 = 13;

/// Where a Recoil install conventionally lives on this platform (FR-D1).
pub fn conventional_paths() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut paths = Vec::new();

    if cfg!(target_os = "macos") {
        if let Some(home) = &home {
            paths.push(home.join("Games").join("Recoil"));
        }
        // Mounted CDs and disk images land under /Volumes with the volume
        // name, which for the retail disc is "Recoil".
        paths.push(PathBuf::from("/Volumes/Recoil"));
    } else if cfg!(target_os = "linux") {
        if let Some(home) = &home {
            paths.push(
                home.join(".wine")
                    .join("drive_c")
                    .join("Program Files")
                    .join("Recoil"),
            );
            paths.push(
                home.join(".wine")
                    .join("drive_c")
                    .join("Program Files (x86)")
                    .join("Recoil"),
            );
            paths.push(home.join("Games").join("Recoil"));
        }
    } else if cfg!(target_os = "windows") {
        paths.push(PathBuf::from(r"C:\Program Files (x86)\Recoil"));
        paths.push(PathBuf::from(r"C:\Program Files\Recoil"));
        paths.push(PathBuf::from(r"C:\Recoil"));
    }

    paths
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::FakeRoot;

    #[test]
    fn a_complete_tree_validates_as_retail() {
        let fake = FakeRoot::retail();

        assert_eq!(validate(fake.path()), Ok(DataVersion::Retail));
    }

    #[test]
    fn six_missions_look_like_a_demo() {
        let fake = FakeRoot::with_missions(1..=6);

        assert_eq!(validate(fake.path()), Ok(DataVersion::Demo));
    }

    #[test]
    fn strings_dll_satisfies_the_message_check() {
        let fake = FakeRoot::retail();
        std::fs::remove_file(fake.path().join("messages.dll")).unwrap();
        std::fs::write(fake.path().join("strings.dll"), []).unwrap();

        assert_eq!(validate(fake.path()), Ok(DataVersion::Retail));
    }

    #[test]
    fn validate_names_every_missing_item() {
        let fake = FakeRoot::empty();

        let missing = validate(fake.path()).unwrap_err();

        assert_eq!(missing.len(), 4, "{missing:?}");
        assert!(missing.iter().any(|m| m.starts_with("zbd/ ")));
        assert!(missing.iter().any(|m| m.starts_with("maps/")));
        assert!(missing.iter().any(|m| m.contains("messages.dll")));
        assert!(missing.iter().any(|m| m.contains("gamez.zbd")));
    }

    #[test]
    fn a_missing_directory_does_not_validate() {
        let missing = validate(Path::new("/nonexistent/recoil")).unwrap_err();

        assert_eq!(missing.len(), 4);
    }

    #[test]
    fn discovery_prefers_the_cli_path() {
        let cli = FakeRoot::retail();
        let env = FakeRoot::retail();

        let root =
            discover_in(Some(cli.path()), Some(env.path().to_path_buf()), None, &[]).unwrap();

        assert_eq!(root.path, cli.path());
    }

    #[test]
    fn discovery_falls_through_to_the_environment_then_config() {
        let env = FakeRoot::retail();
        let config = FakeRoot::retail();

        let root = discover_in(
            None,
            Some(env.path().to_path_buf()),
            Some(config.path().to_path_buf()),
            &[],
        )
        .unwrap();
        assert_eq!(root.path, env.path());

        let root = discover_in(None, None, Some(config.path().to_path_buf()), &[]).unwrap();
        assert_eq!(root.path, config.path());
    }

    #[test]
    fn an_invalid_candidate_is_skipped_for_a_valid_one() {
        let broken = FakeRoot::empty();
        let good = FakeRoot::retail();

        // Among the paths the user did not name explicitly, a broken one is
        // just skipped.
        let root = discover_in(
            None,
            Some(broken.path().to_path_buf()),
            None,
            &[good.path().to_path_buf()],
        )
        .unwrap();

        assert_eq!(root.path, good.path());
    }

    #[test]
    fn an_explicit_path_that_does_not_validate_stops_the_search() {
        let broken = FakeRoot::empty();
        let good = FakeRoot::retail();

        // Falling through to `good` here would silently run a different
        // install than the one the user asked for.
        let err = discover_in(
            Some(broken.path()),
            None,
            None,
            &[good.path().to_path_buf()],
        )
        .unwrap_err();

        assert_eq!(err.looked_in, vec![broken.path().to_path_buf()]);
        assert!(!err.missing.is_empty());
    }

    #[test]
    fn the_error_lists_every_path_and_the_near_misses_gaps() {
        // An install with everything but the mission archive: the error
        // should name that one file, not all four checks.
        let nearly = FakeRoot::retail();
        std::fs::remove_file(nearly.path().join("zbd/m1/gamez.zbd")).unwrap();

        let err = discover_in(
            None,
            None,
            None,
            &[
                PathBuf::from("/nonexistent/recoil"),
                nearly.path().to_path_buf(),
            ],
        )
        .unwrap_err();

        assert_eq!(err.looked_in.len(), 2);
        assert_eq!(err.missing.len(), 1);
        assert!(err.missing[0].contains("gamez.zbd"));

        let text = err.to_string();
        assert!(text.contains("/nonexistent/recoil"));
        assert!(text.contains("BFT_DATA_DIR"));
    }

    #[test]
    fn the_real_install_validates_when_one_is_configured() {
        let Some(root) = crate::test_support::data_root() else {
            return;
        };

        let version = validate(&root).expect("BFT_DATA_DIR should be a valid Recoil install");
        assert_eq!(version, DataVersion::Retail);
    }
}
