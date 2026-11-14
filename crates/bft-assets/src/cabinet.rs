//! Unpacking the disc's InstallShield cabinet into an install tree, by
//! running `unshield`.
//!
//! The cabinet's format is InstallShield's, and like the game's own archives
//! (which mech3ax reads) it is read by a separate tool, not by this crate.
//! What is unpacked, and where each file group goes, is docs/data/install.md,
//! "The disc and its installer cabinet".

use std::path::{Path, PathBuf};
use std::process::Command;

/// The file groups that make up the install tree, and the directory under
/// the install root each one's files go to (docs/data/install.md).
pub const GROUPS: [(&str, &str); 9] = [
    ("Game Files", ""),
    ("Hardware Textures", ""),
    ("Software Textures", ""),
    ("Software Textures Minimum", ""),
    ("AVI Files", ""),
    ("Sound Low Fidelity", "zbd"),
    ("Sound Medium Fidelity", "zbd"),
    ("Sound High Fidelity", "zbd"),
    ("Font Files", "fonts"),
];

/// The file groups that hold Windows system files rather than the game's,
/// and are not unpacked.
pub const SKIPPED: [&str; 4] = ["Uninstal Files", "A3D DLLs", "Indeo 5 codec", "Msvc DLLs"];

/// Why the cabinet could not be unpacked.
#[derive(Debug, thiserror::Error)]
pub enum CabinetError {
    /// `unshield` could not be started.
    #[error("cannot run {unshield}: {source}")]
    Spawn {
        /// The binary that was tried.
        unshield: PathBuf,
        /// What the OS said.
        #[source]
        source: std::io::Error,
    },
    /// `unshield` ran and failed.
    #[error("`{command}` failed:\n{output}")]
    Failed {
        /// The command, copy-pasteable.
        command: String,
        /// What it printed.
        output: String,
    },
    /// The file system said no.
    #[error("{path}: {source}")]
    Io {
        /// The path that failed.
        path: PathBuf,
        /// What the OS said.
        #[source]
        source: std::io::Error,
    },
    /// `unshield` reported success but wrote no files for a group.
    #[error("unshield wrote nothing for the file group \"{group}\"")]
    Empty {
        /// The group.
        group: String,
    },
    /// The unpacked tree is not an install.
    #[error("the unpacked cabinet is not a complete install; it is missing: {}", missing.join(", "))]
    Incomplete {
        /// What `root::validate` found missing.
        missing: Vec<String>,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> CabinetError + '_ {
    move |source| CabinetError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Progress through [`unpack`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnpackProgress {
    /// A group is being unpacked: its position, the count, and its name.
    Group(usize, usize, String),
    /// The cabinet has a group this table does not know; it is left out
    /// and worth reporting, since another edition of the game may need it.
    Unknown(String),
}

/// Find `unshield`: the CLI argument, `BFT_UNSHIELD`, then `PATH`.
pub fn find_unshield(cli: Option<&Path>) -> Option<PathBuf> {
    let named = cli
        .map(Path::to_path_buf)
        .or_else(|| std::env::var_os("BFT_UNSHIELD").map(PathBuf::from));
    if let Some(named) = named {
        return named.is_file().then_some(named);
    }

    let name = if cfg!(windows) {
        "unshield.exe"
    } else {
        "unshield"
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
}

/// How to get `unshield` on this platform, for the message when it is
/// missing.
pub fn install_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "brew install unshield"
    } else if cfg!(target_os = "linux") {
        "sudo apt install unshield (Debian, Ubuntu), sudo dnf install unshield \
         (Fedora), or sudo pacman -S unshield (Arch)"
    } else {
        "build it from https://github.com/twogood/unshield"
    }
}

/// Unpack `cabinet` into a fresh install tree at `install`.
///
/// Works in `<install>.partial` and renames it into place only once the
/// result validates as an install, so an interrupted run leaves nothing
/// that looks finished. An existing `install` is replaced.
pub fn unpack(
    unshield: &Path,
    cabinet: &Path,
    install: &Path,
    mut progress: impl FnMut(UnpackProgress),
) -> Result<(), CabinetError> {
    let partial = install.with_extension("partial");
    if partial.exists() {
        std::fs::remove_dir_all(&partial).map_err(io(&partial))?;
    }
    let groups_dir = partial.join("groups");
    let tree = partial.join("tree");
    std::fs::create_dir_all(&groups_dir).map_err(io(&groups_dir))?;
    std::fs::create_dir_all(&tree).map_err(io(&tree))?;

    let present = list_groups(unshield, cabinet)?;
    for name in &present {
        if !GROUPS.iter().any(|(group, _)| group == name) && !SKIPPED.contains(&name.as_str()) {
            progress(UnpackProgress::Unknown(name.clone()));
        }
    }

    let wanted: Vec<_> = GROUPS
        .iter()
        .filter(|(group, _)| present.iter().any(|name| name == group))
        .collect();
    for (n, (group, dest)) in wanted.iter().enumerate() {
        progress(UnpackProgress::Group(
            n + 1,
            wanted.len(),
            group.to_string(),
        ));

        // Each group into an empty directory of its own: `unshield` writes
        // the files under one directory named after the group, but escapes
        // the name (`Game Files` becomes `Game_Files`), and how may differ
        // between versions. Whatever single directory appears is the group.
        let out = groups_dir.join(n.to_string());
        std::fs::create_dir_all(&out).map_err(io(&out))?;
        let mut command = Command::new(unshield);
        command
            .arg("-d")
            .arg(&out)
            .arg("-g")
            .arg(group)
            .arg("x")
            .arg(cabinet);
        run(unshield, &mut command)?;

        let written: Vec<PathBuf> = std::fs::read_dir(&out)
            .map_err(io(&out))?
            .flatten()
            .map(|entry| entry.path())
            .collect();
        let unpacked = match written.as_slice() {
            [] => {
                return Err(CabinetError::Empty {
                    group: group.to_string(),
                });
            }
            [only] if only.is_dir() => only.clone(),
            _ => out,
        };
        merge_into(&unpacked, &tree.join(dest))?;
    }

    if let Err(missing) = crate::root::validate(&tree) {
        return Err(CabinetError::Incomplete { missing });
    }

    if install.exists() {
        std::fs::remove_dir_all(install).map_err(io(install))?;
    }
    std::fs::rename(&tree, install).map_err(io(install))?;
    std::fs::remove_dir_all(&partial).map_err(io(&partial))?;

    Ok(())
}

/// The cabinet's file group names, from `unshield g`.
fn list_groups(unshield: &Path, cabinet: &Path) -> Result<Vec<String>, CabinetError> {
    let mut command = Command::new(unshield);
    command.arg("g").arg(cabinet);
    let output = run(unshield, &mut command)?;

    // The names sit between the "Cabinet: <file>" line and the "-------"
    // line before the count.
    Ok(output
        .lines()
        .skip_while(|line| !line.starts_with("Cabinet:"))
        .skip(1)
        .take_while(|line| !line.starts_with("---"))
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .collect())
}

/// Run `command`, returning its standard output.
fn run(unshield: &Path, command: &mut Command) -> Result<String, CabinetError> {
    let output = command.output().map_err(|source| CabinetError::Spawn {
        unshield: unshield.to_path_buf(),
        source,
    })?;

    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }

    let mut printed = String::from_utf8_lossy(&output.stdout).into_owned();
    printed.push_str(&String::from_utf8_lossy(&output.stderr));
    Err(CabinetError::Failed {
        command: crate::extract::display_command(command),
        output: printed.trim().to_string(),
    })
}

/// Move everything under `from` into `to`, merging directories.
fn merge_into(from: &Path, to: &Path) -> Result<(), CabinetError> {
    std::fs::create_dir_all(to).map_err(io(to))?;

    for entry in std::fs::read_dir(from).map_err(io(from))? {
        let entry = entry.map_err(io(from))?;
        let source = entry.path();
        let target = to.join(entry.file_name());
        if source.is_dir() && target.is_dir() {
            merge_into(&source, &target)?;
        } else {
            std::fs::rename(&source, &target).map_err(io(&target))?;
        }
    }

    Ok(())
}

#[cfg(all(test, unix))]
pub(crate) mod tests {
    use super::*;
    use crate::test_support::TempDir;

    /// A stand-in `unshield` that knows the retail groups and writes a
    /// file or two for each, under the group's own directory as the real
    /// one does.
    pub(crate) fn fake_unshield(dir: &Path) -> PathBuf {
        use std::os::unix::fs::PermissionsExt as _;

        let path = dir.join("unshield");
        std::fs::write(
            &path,
            r#"#!/bin/sh
if [ "$1" = g ]; then
  echo "Cabinet: $2"
  for g in "Hardware Textures" "Sound High Fidelity" "Font Files" "Uninstal Files" \
           "Game Files" "AVI Files" "Bonus Maps"; do echo "$g"; done
  echo "-------"
  echo "7 file groups"
  exit 0
fi
# -d DIR -g GROUP x CAB, under the group's name with spaces escaped as the
# real one does
out="$2/$(echo "$4" | tr ' ' _)"
case "$4" in
  "Game Files")
    mkdir -p "$out/zbd/m1" "$out/maps"
    echo z > "$out/messages.dll"; echo z > "$out/zbd/m1/gamez.zbd"
    echo z > "$out/zbd/interp.zbd"; echo z > "$out/maps/m1.zmap" ;;
  "Hardware Textures") mkdir -p "$out/zbd/m1"; echo z > "$out/zbd/m1/rtexture4.zbd" ;;
  "Sound High Fidelity") mkdir -p "$out"; echo z > "$out/soundsh.zbd" ;;
  "Font Files") mkdir -p "$out"; echo z > "$out/eurosti.ttf" ;;
  "AVI Files") mkdir -p "$out/video"; echo z > "$out/video/intro_01.avi" ;;
  *) echo "unexpected group $4" >&2; exit 1 ;;
esac
"#,
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn the_groups_land_where_the_install_has_them() {
        let dir = TempDir::new();
        let unshield = fake_unshield(dir.path());
        let install = dir.path().join("recoil");
        let mut seen = Vec::new();

        unpack(&unshield, Path::new("/disc/data1.cab"), &install, |p| {
            seen.push(p)
        })
        .unwrap();

        for file in [
            "messages.dll",
            "zbd/m1/gamez.zbd",
            "zbd/m1/rtexture4.zbd",
            "zbd/soundsh.zbd",
            "fonts/eurosti.ttf",
            "video/intro_01.avi",
        ] {
            assert!(install.join(file).is_file(), "{file}");
        }
        assert!(!dir.path().join("recoil.partial").exists());

        // Only the known groups the cabinet has, in the table's order; the
        // unknown one reported, the skipped one not.
        let groups: Vec<_> = seen
            .iter()
            .filter_map(|p| match p {
                UnpackProgress::Group(_, _, name) => Some(name.as_str()),
                UnpackProgress::Unknown(_) => None,
            })
            .collect();
        assert_eq!(
            groups,
            [
                "Game Files",
                "Hardware Textures",
                "AVI Files",
                "Sound High Fidelity",
                "Font Files"
            ]
        );
        assert!(seen.contains(&UnpackProgress::Unknown("Bonus Maps".into())));
        assert!(!seen.contains(&UnpackProgress::Unknown("Uninstal Files".into())));
    }

    #[test]
    fn a_group_that_writes_nothing_is_an_error() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = TempDir::new();
        let unshield = dir.path().join("unshield");
        std::fs::write(
            &unshield,
            "#!/bin/sh\n[ \"$1\" = g ] && printf 'Cabinet: c\\nGame Files\\n---\\n'\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&unshield, std::fs::Permissions::from_mode(0o755)).unwrap();

        let err = unpack(
            &unshield,
            Path::new("data1.cab"),
            &dir.path().join("recoil"),
            |_| {},
        )
        .unwrap_err();

        assert!(
            matches!(&err, CabinetError::Empty { group } if group == "Game Files"),
            "{err}"
        );
    }

    #[test]
    fn a_failing_unshield_is_reported_with_its_output_and_nothing_is_kept() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = TempDir::new();
        let unshield = dir.path().join("unshield");
        std::fs::write(
            &unshield,
            "#!/bin/sh\necho 'Failed to open data1.cab' >&2\nexit 1\n",
        )
        .unwrap();
        std::fs::set_permissions(&unshield, std::fs::Permissions::from_mode(0o755)).unwrap();
        let install = dir.path().join("recoil");

        let err = unpack(&unshield, Path::new("data1.cab"), &install, |_| {}).unwrap_err();

        let text = err.to_string();
        assert!(text.contains("Failed to open data1.cab"), "{text}");
        assert!(text.contains("unshield g data1.cab"), "{text}");
        assert!(!install.exists());
    }

    #[test]
    fn the_table_matches_the_install_doc() {
        let doc = include_str!("../../../docs/data/install.md");

        for (group, dest) in GROUPS {
            let dest = if dest.is_empty() {
                "`.`".to_string()
            } else {
                format!("`{dest}/`")
            };
            assert!(
                doc.lines()
                    .any(|line| line.contains(group) && line.contains(&dest)),
                "docs/data/install.md does not send {group} to {dest}"
            );
        }
    }
}
