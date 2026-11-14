//! What `open-bft setup` can be pointed at, recognised by content.
//!
//! A user may have the game as an installed copy, as the disc's own files (a
//! mounted CD or image, or the disc copied to a folder), as a disc image
//! file, and the music as a folder of ripped tracks. Each enters the setup
//! chain at a different point:
//!
//! ```text
//! disc image ─► disc files ─► unpacked cabinet ─► install ─► cache
//!     └─ audio tracks, when the image has them ─► music folder
//! ```
//!
//! Recognition looks at what a path holds, never only at its name, so a
//! misnamed image still works and a wrong file is named as such.

use std::fs::File;
use std::io::{Read as _, Seek as _, SeekFrom};
use std::path::{Path, PathBuf};

/// Something setup can take the game or its music from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// An installed or extracted copy: a valid data root, used where it is.
    Install(PathBuf),
    /// The disc's own files: a mounted CD, a mounted image, or a copy.
    /// Recognised by the InstallShield cabinet, `DATA1.CAB`.
    DiscFolder(PathBuf),
    /// A disc image file.
    DiscImage {
        /// The file setup reads: the image, or the `.cue` sheet naming it.
        path: PathBuf,
        /// How its sectors are laid out.
        format: ImageFormat,
    },
    /// A folder of music files, for instance tracks ripped from the disc.
    Music(PathBuf),
}

/// The disc image formats setup reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    /// Nero: tracks described by a chunk list at the end of the file.
    /// May hold the CD-audio tracks.
    Nrg,
    /// A plain ISO 9660 image of the data track, 2048-byte sectors. Holds
    /// no audio.
    Iso,
    /// A `.cue` sheet and the raw `.bin` it names. May hold audio.
    Cue,
    /// A raw `.bin` with 2352-byte sectors and no sheet: the data track
    /// only, since only a sheet says where audio tracks begin.
    RawBin,
}

impl Source {
    /// The path this source was recognised at.
    pub fn path(&self) -> &Path {
        match self {
            Source::Install(path)
            | Source::DiscFolder(path)
            | Source::DiscImage { path, .. }
            | Source::Music(path) => path,
        }
    }

    /// Whether the game's data can come from here.
    pub fn has_data(&self) -> bool {
        !matches!(self, Source::Music(_))
    }

    /// What this is, in a few words, for setup's messages.
    pub fn describe(&self) -> &'static str {
        match self {
            Source::Install(_) => "an installed copy",
            Source::DiscFolder(_) => "the disc's files",
            Source::DiscImage {
                format: ImageFormat::Nrg,
                ..
            } => "a Nero disc image",
            Source::DiscImage {
                format: ImageFormat::Iso,
                ..
            } => "an ISO disc image",
            Source::DiscImage {
                format: ImageFormat::Cue,
                ..
            } => "a BIN/CUE disc image",
            Source::DiscImage {
                format: ImageFormat::RawBin,
                ..
            } => "a raw BIN disc image",
            Source::Music(_) => "a music folder",
        }
    }
}

/// The audio formats the game's music player reads (`bft-game`'s
/// `MUSIC_EXTENSIONS`; kept in step by hand, as this crate cannot see it).
const MUSIC_EXTENSIONS: [&str; 4] = ["ogg", "mp3", "wav", "flac"];

/// Recognise what `path` holds, or say why it is none of the above.
pub fn classify(path: &Path) -> Result<Source, String> {
    if path.is_dir() {
        return classify_dir(path);
    }
    if path.is_file() {
        return classify_file(path);
    }

    Err(format!("{}: no such file or directory", path.display()))
}

fn classify_dir(path: &Path) -> Result<Source, String> {
    let missing = match crate::root::validate(path) {
        Ok(_) => return Ok(Source::Install(path.to_path_buf())),
        Err(missing) => missing,
    };

    if find_file_ignoring_case(path, "data1.cab").is_some() {
        return Ok(Source::DiscFolder(path.to_path_buf()));
    }

    if has_music(path) {
        return Ok(Source::Music(path.to_path_buf()));
    }

    Err(format!(
        "{}: not a Recoil install, the disc's files, or a music folder. \
         As an install it is missing: {}",
        path.display(),
        missing.join(", ")
    ))
}

fn classify_file(path: &Path) -> Result<Source, String> {
    let image = |format| {
        Ok(Source::DiscImage {
            path: path.to_path_buf(),
            format,
        })
    };
    let unreadable = |e: std::io::Error| format!("{}: {e}", path.display());

    let mut file = File::open(path).map_err(unreadable)?;
    let len = file.metadata().map_err(unreadable)?.len();

    // Nero writes its footer at the very end: "NER5" and a 64-bit offset
    // (version 2), or "NERO" and a 32-bit one (version 1).
    if len >= 12 {
        let mut tail = [0u8; 12];
        file.seek(SeekFrom::End(-12)).map_err(unreadable)?;
        file.read_exact(&mut tail).map_err(unreadable)?;
        if &tail[..4] == b"NER5" || &tail[4..8] == b"NERO" {
            return image(ImageFormat::Nrg);
        }
    }

    // A raw image starts with a sector's 12-byte sync pattern.
    let mut head = [0u8; 12];
    file.seek(SeekFrom::Start(0)).map_err(unreadable)?;
    if file.read_exact(&mut head).is_ok() && head == SYNC {
        // A sheet beside it says where the tracks are; read that instead.
        let sheet = path.with_extension("cue");
        if sheet.is_file() {
            return Ok(Source::DiscImage {
                path: sheet,
                format: ImageFormat::Cue,
            });
        }
        return image(ImageFormat::RawBin);
    }

    // ISO 9660's volume descriptor, at sector 16 of 2048 bytes.
    let mut magic = [0u8; 5];
    if file.seek(SeekFrom::Start(16 * 2048 + 1)).is_ok()
        && file.read_exact(&mut magic).is_ok()
        && &magic == b"CD001"
    {
        return image(ImageFormat::Iso);
    }

    if is_cue_sheet(path) {
        return image(ImageFormat::Cue);
    }

    Err(format!(
        "{}: not a disc image this setup reads (Nero .nrg, .iso, or .bin with \
         .cue)",
        path.display()
    ))
}

/// The sync pattern that opens every raw 2352-byte data sector.
pub(crate) const SYNC: [u8; 12] = [0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0];

/// A small text file with a `FILE` and a `TRACK` line.
fn is_cue_sheet(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if meta.len() > 64 * 1024 {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let has = |keyword: &str| {
        text.lines()
            .any(|line| line.trim_start().to_ascii_uppercase().starts_with(keyword))
    };

    has("FILE ") && has("TRACK ")
}

/// A file in `dir` whose name matches `name` in any case: a disc's ISO 9660
/// names are upper case, a copy's may not be.
pub fn find_file_ignoring_case(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case(name)
                && entry.path().is_file()
        })
        .map(|entry| entry.path())
}

/// Whether `dir` directly holds a file the music player reads.
fn has_music(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };

    entries.flatten().any(|entry| {
        entry.path().extension().is_some_and(|ext| {
            MUSIC_EXTENSIONS.contains(&ext.to_string_lossy().to_ascii_lowercase().as_str())
        })
    })
}

/// Places a copy of the game is likely to be, for setup with no arguments:
/// the conventional install paths, media the desktop mounted, and disc
/// images in the current directory and in the user's Downloads and Desktop.
/// Only places the system defines, so the search stays predictable.
///
/// Only paths that [`classify`] recognises as holding the game's data are
/// returned, in that order and without duplicates.
pub fn candidates() -> Vec<Source> {
    let mut paths: Vec<PathBuf> = crate::root::conventional_paths();
    paths.extend(mounted_media());

    let mut image_dirs: Vec<PathBuf> = std::env::current_dir().into_iter().collect();
    image_dirs.extend(download_and_desktop());
    for dir in image_dirs {
        paths.extend(image_files(&dir));
    }

    let mut found: Vec<Source> = Vec::new();
    for path in paths {
        if let Ok(source) = classify(&path)
            && source.has_data()
            && !found.iter().any(|s| s.path() == source.path())
        {
            found.push(source);
        }
    }

    found
}

/// The user's Downloads and Desktop folders, by the names the system gives
/// them: the XDG user directories on Linux (localised, e.g.
/// `~/Schreibtisch`), the standard folders on macOS. Falls back to
/// `~/Downloads` and `~/Desktop` when the system names none.
fn download_and_desktop() -> Vec<PathBuf> {
    let Some(dirs) = directories::UserDirs::new() else {
        return Vec::new();
    };
    let home = dirs.home_dir();

    vec![
        dirs.download_dir()
            .map_or_else(|| home.join("Downloads"), Path::to_path_buf),
        dirs.desktop_dir()
            .map_or_else(|| home.join("Desktop"), Path::to_path_buf),
    ]
}

/// The folders the desktop mounts removable media and disc images in:
/// `/Volumes` on macOS, `/media/$USER` (Debian, Ubuntu) and
/// `/run/media/$USER` (Fedora, Arch) on Linux. `/mnt` is left out: it is for
/// mounts an administrator makes by hand.
fn mounted_media() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    if cfg!(target_os = "macos") {
        roots.push(PathBuf::from("/Volumes"));
    } else if cfg!(target_os = "linux")
        && let Some(user) = std::env::var_os("USER")
    {
        roots.push(Path::new("/media").join(&user));
        roots.push(Path::new("/run/media").join(&user));
    }

    roots
        .iter()
        .filter_map(|root| std::fs::read_dir(root).ok())
        .flat_map(|entries| entries.flatten().map(|entry| entry.path()))
        .filter(|path| path.is_dir())
        .collect()
}

/// Files in `dir` named like disc images. The name only nominates them;
/// [`classify`] decides by content.
fn image_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path.extension().is_some_and(|ext| {
                    ["nrg", "iso", "cue", "bin"]
                        .contains(&ext.to_string_lossy().to_ascii_lowercase().as_str())
                })
        })
        .collect();
    files.sort();
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeRoot, TempDir};

    fn write(path: &Path, bytes: &[u8]) {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, bytes).expect("write");
    }

    #[test]
    fn an_install_is_used_in_place() {
        let fake = FakeRoot::retail();

        assert_eq!(
            classify(fake.path()),
            Ok(Source::Install(fake.path().to_path_buf()))
        );
    }

    #[test]
    fn the_discs_files_are_recognised_by_the_cabinet_in_any_case() {
        let dir = TempDir::new();
        write(&dir.path().join("DATA1.CAB"), b"ISc(");

        assert_eq!(
            classify(dir.path()),
            Ok(Source::DiscFolder(dir.path().to_path_buf()))
        );

        let copy = TempDir::new();
        write(&copy.path().join("data1.cab"), b"ISc(");
        assert!(matches!(classify(copy.path()), Ok(Source::DiscFolder(_))));
    }

    #[test]
    fn a_folder_of_tracks_is_music() {
        let dir = TempDir::new();
        write(&dir.path().join("Track02.FLAC"), b"fLaC");

        assert_eq!(
            classify(dir.path()),
            Ok(Source::Music(dir.path().to_path_buf()))
        );
        assert!(!classify(dir.path()).unwrap().has_data());
    }

    #[test]
    fn an_unrelated_folder_says_what_an_install_would_need() {
        let dir = TempDir::new();

        let err = classify(dir.path()).unwrap_err();

        assert!(err.contains("missing"), "{err}");
        assert!(err.contains("zbd/"), "{err}");
    }

    #[test]
    fn a_nero_image_is_recognised_by_its_footer_whatever_its_name() {
        let dir = TempDir::new();
        let mut bytes = vec![0u8; 4096];
        bytes.extend_from_slice(b"NER5");
        bytes.extend_from_slice(&4096u64.to_be_bytes());
        let path = dir.path().join("recoil.img");
        write(&path, &bytes);

        assert_eq!(
            classify(&path),
            Ok(Source::DiscImage {
                path,
                format: ImageFormat::Nrg
            })
        );
    }

    #[test]
    fn a_version_one_nero_footer_is_recognised_too() {
        let dir = TempDir::new();
        let mut bytes = vec![0u8; 4096];
        bytes.extend_from_slice(b"NERO");
        bytes.extend_from_slice(&4096u32.to_be_bytes());
        let path = dir.path().join("old.nrg");
        write(&path, &bytes);

        assert!(matches!(
            classify(&path),
            Ok(Source::DiscImage {
                format: ImageFormat::Nrg,
                ..
            })
        ));
    }

    #[test]
    fn an_iso_is_recognised_by_its_volume_descriptor() {
        let dir = TempDir::new();
        let mut bytes = vec![0u8; 17 * 2048];
        bytes[16 * 2048] = 1;
        bytes[16 * 2048 + 1..16 * 2048 + 6].copy_from_slice(b"CD001");
        let path = dir.path().join("recoil.iso");
        write(&path, &bytes);

        assert!(matches!(
            classify(&path),
            Ok(Source::DiscImage {
                format: ImageFormat::Iso,
                ..
            })
        ));
    }

    #[test]
    fn a_raw_bin_prefers_the_sheet_beside_it() {
        let dir = TempDir::new();
        let mut bytes = SYNC.to_vec();
        bytes.resize(2352, 0);
        let bin = dir.path().join("recoil.bin");
        write(&bin, &bytes);

        assert!(matches!(
            classify(&bin),
            Ok(Source::DiscImage {
                format: ImageFormat::RawBin,
                ..
            })
        ));

        let sheet = dir.path().join("recoil.cue");
        write(
            &sheet,
            b"FILE \"recoil.bin\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n",
        );

        assert_eq!(
            classify(&bin),
            Ok(Source::DiscImage {
                path: sheet.clone(),
                format: ImageFormat::Cue
            })
        );
        assert!(matches!(
            classify(&sheet),
            Ok(Source::DiscImage {
                format: ImageFormat::Cue,
                ..
            })
        ));
    }

    #[test]
    fn any_other_file_is_named_as_unreadable() {
        let dir = TempDir::new();
        let path = dir.path().join("notes.txt");
        write(&path, b"not a disc");

        let err = classify(&path).unwrap_err();

        assert!(err.contains("not a disc image"), "{err}");
    }

    #[test]
    fn a_missing_path_is_an_error() {
        let err = classify(Path::new("/nonexistent/recoil.nrg")).unwrap_err();

        assert!(err.contains("no such file"), "{err}");
    }
}
