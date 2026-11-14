//! `open-bft setup` against a fake install, a stand-in `unzbd` and a
//! throwaway home directory.
//!
//! The home directory is the point: setup writes `config.ron`, and a test
//! must never touch the developer's own. `HOME` and `XDG_*` move the
//! platform directories on Linux and macOS alike. Unix only, because the
//! stand-in `unzbd` is a shell script.
#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A temporary directory that deletes itself when dropped.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let path = std::env::temp_dir().join(format!(
            "open-bft-setup-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("temp dir");

        TempDir { path }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// The smallest tree `validate` accepts as an install: empty files with
/// the real names, no game data.
fn fake_install() -> TempDir {
    let dir = TempDir::new("install");
    let touch = |path: PathBuf| {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, b"z").expect("write");
    };

    touch(dir.path.join("messages.dll"));
    touch(dir.path.join("zbd").join("interp.zbd"));
    touch(dir.path.join("zbd").join("m1").join("gamez.zbd"));
    touch(dir.path.join("maps").join("m1.zmap"));

    dir
}

/// A stand-in `unzbd`: answers `--version` and writes an empty output for
/// every `unzbd rc <kind> <in> <out>`.
fn fake_unzbd(dir: &TempDir) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;

    let path = dir.path.join("unzbd");
    std::fs::write(
        &path,
        "#!/bin/sh\n\
         if [ \"$1\" = --version ]; then echo 'unzbd fake'; exit 0; fi\n\
         : > \"$4\"\n",
    )
    .expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    path
}

/// Run `open-bft setup <install>` with `home` as the home directory.
fn setup(home: &Path, install: &Path, unzbd: &Path, cache: &Path) -> Output {
    setup_with(home, &[install], unzbd, cache)
}

/// Run `open-bft setup <sources...>` with `home` as both the home and the
/// working directory, so neither the search nor the config reaches outside.
fn setup_with(home: &Path, sources: &[&Path], unzbd: &Path, cache: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_open-bft"))
        .arg("setup")
        .args(sources)
        .arg("--unzbd")
        .arg(unzbd)
        .arg("--cache-dir")
        .arg(cache)
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env_remove("BFT_DATA_DIR")
        .env_remove("BFT_UNZBD")
        .output()
        .expect("the binary should run")
}

/// Where `config.ron` lands under a home directory, on this platform.
fn config_file(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/org.open-bft.open-bft/config.ron")
    } else {
        home.join(".config/open-bft/config.ron")
    }
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn setup_remembers_the_install_and_extracts() {
    let (home, install, bin, cache) = (
        TempDir::new("home"),
        fake_install(),
        TempDir::new("bin"),
        TempDir::new("cache"),
    );

    let out = setup(&home.path, &install.path, &fake_unzbd(&bin), &cache.path);

    let (stdout, stderr) = (text(&out.stdout), text(&out.stderr));
    assert!(out.status.success(), "{stdout}\n{stderr}");
    assert!(stdout.contains("Ready."), "{stdout}");

    let config = std::fs::read_to_string(config_file(&home.path)).expect("config.ron written");
    assert!(
        config.contains(&install.path.to_string_lossy().to_string()),
        "{config}"
    );
    assert!(cache.path.join("manifest.json").is_file());
    assert!(cache.path.join("interp.json").is_file());
}

#[test]
fn setup_keeps_the_players_other_settings() {
    let (home, install, bin, cache) = (
        TempDir::new("home"),
        fake_install(),
        TempDir::new("bin"),
        TempDir::new("cache"),
    );
    let file = config_file(&home.path);
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("mkdir");
    std::fs::write(&file, r#"(player_name: "Tester")"#).expect("write");

    let out = setup(&home.path, &install.path, &fake_unzbd(&bin), &cache.path);
    assert!(out.status.success(), "{}", text(&out.stderr));

    let config = std::fs::read_to_string(&file).expect("read");
    assert!(config.contains("\"Tester\""), "{config}");
    assert!(config.contains("data_root: Some("), "{config}");
}

#[test]
fn setup_leaves_a_broken_config_alone() {
    let (home, install, bin, cache) = (
        TempDir::new("home"),
        fake_install(),
        TempDir::new("bin"),
        TempDir::new("cache"),
    );
    let file = config_file(&home.path);
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("mkdir");
    std::fs::write(&file, "(player_name: ").expect("write");

    let out = setup(&home.path, &install.path, &fake_unzbd(&bin), &cache.path);

    assert!(!out.status.success());
    assert!(
        text(&out.stderr).contains("leaves it"),
        "{}",
        text(&out.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&file).expect("read"),
        "(player_name: "
    );
    // It stopped before extracting.
    assert!(!cache.path.join("manifest.json").exists());
}

#[test]
fn setup_rejects_a_directory_that_is_not_an_install() {
    let (home, empty, bin, cache) = (
        TempDir::new("home"),
        TempDir::new("empty"),
        TempDir::new("bin"),
        TempDir::new("cache"),
    );

    let out = setup(&home.path, &empty.path, &fake_unzbd(&bin), &cache.path);

    // FR-D1's failure: exit 2, nothing remembered.
    assert_eq!(out.status.code(), Some(2), "{}", text(&out.stderr));
    assert!(!config_file(&home.path).exists());
}

#[test]
fn setup_remembers_a_music_folder_given_with_the_install() {
    let (home, install, bin, cache, music) = (
        TempDir::new("home"),
        fake_install(),
        TempDir::new("bin"),
        TempDir::new("cache"),
        TempDir::new("music"),
    );
    std::fs::write(music.path.join("track02.flac"), b"fLaC").expect("write");

    let out = setup_with(
        &home.path,
        &[&music.path, &install.path],
        &fake_unzbd(&bin),
        &cache.path,
    );
    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", text(&out.stderr));
    assert!(stdout.contains("a music folder"), "{stdout}");

    let config = std::fs::read_to_string(config_file(&home.path)).expect("read");
    assert!(
        config.contains(&format!("music_dir: Some(\"{}\")", music.path.display())),
        "{config}"
    );
    assert!(config.contains("data_root: Some("), "{config}");
}

#[test]
fn setup_names_a_file_it_cannot_read() {
    let (home, bin, cache) = (
        TempDir::new("home"),
        TempDir::new("bin"),
        TempDir::new("cache"),
    );
    let notes = home.path.join("notes.txt");
    std::fs::write(&notes, b"not a disc").expect("write");

    let out = setup_with(&home.path, &[&notes], &fake_unzbd(&bin), &cache.path);

    assert_eq!(out.status.code(), Some(2));
    assert!(
        text(&out.stderr).contains("not a disc image"),
        "{}",
        text(&out.stderr)
    );
}

#[test]
fn setup_with_nothing_to_find_says_where_it_looked() {
    let (home, bin, cache) = (
        TempDir::new("home"),
        TempDir::new("bin"),
        TempDir::new("cache"),
    );

    let out = setup_with(&home.path, &[], &fake_unzbd(&bin), &cache.path);

    let stderr = text(&out.stderr);
    assert_eq!(out.status.code(), Some(2), "{stderr}");
    assert!(stderr.contains("no copy of Recoil found"), "{stderr}");
    assert!(stderr.contains("your Downloads"), "{stderr}");
}

#[cfg(target_os = "linux")]
#[test]
fn setup_finds_an_install_in_a_usual_place_when_told_yes() {
    let (home, bin, cache) = (
        TempDir::new("home"),
        TempDir::new("bin"),
        TempDir::new("cache"),
    );
    // Wine's default prefix is the usual place on Linux.
    let install = fake_install();
    let usual = home.path.join(".wine/drive_c/Program Files/Recoil");
    std::fs::create_dir_all(usual.parent().expect("a parent")).expect("mkdir");
    std::fs::rename(&install.path, &usual).expect("move");

    let out = Command::new(env!("CARGO_BIN_EXE_open-bft"))
        .args(["setup", "--yes", "--unzbd"])
        .arg(fake_unzbd(&bin))
        .arg("--cache-dir")
        .arg(&cache.path)
        .current_dir(&home.path)
        .env("HOME", &home.path)
        .env("XDG_CONFIG_HOME", home.path.join(".config"))
        .env("XDG_DATA_HOME", home.path.join(".local/share"))
        .env_remove("BFT_DATA_DIR")
        .env_remove("BFT_UNZBD")
        .output()
        .expect("the binary should run");

    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", text(&out.stderr));
    let config = std::fs::read_to_string(config_file(&home.path)).expect("read");
    assert!(
        config.contains(&usual.to_string_lossy().to_string()),
        "{config}"
    );
}

/// Where the project's data directory is under a home directory.
fn data_dir(home: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/org.open-bft.open-bft")
    } else {
        home.join(".local/share/open-bft")
    }
}

/// A stand-in `unshield`: lists two file groups and writes, for each, the
/// files of a minimal install under the group's directory as the real one
/// does. Fails unless the cabinet it is handed exists.
fn fake_unshield(dir: &TempDir) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;

    let path = dir.path.join("unshield");
    std::fs::write(
        &path,
        r#"#!/bin/sh
if [ "$1" = g ]; then
  [ -f "$2" ] || { echo "Failed to open $2" >&2; exit 1; }
  printf 'Cabinet: %s\nGame Files\nFont Files\nMsvc DLLs\n-------\n3 file groups\n' "$2"
  exit 0
fi
[ -f "$6" ] || { echo "Failed to open $6" >&2; exit 1; }
out="$2/$(echo "$4" | tr ' ' _)"
case "$4" in
  "Game Files")
    mkdir -p "$out/zbd/m1" "$out/maps"
    echo z > "$out/messages.dll"; echo z > "$out/zbd/m1/gamez.zbd"
    echo z > "$out/zbd/interp.zbd"; echo z > "$out/maps/m1.zmap" ;;
  "Font Files") mkdir -p "$out"; echo z > "$out/eurosti.ttf" ;;
  *) exit 1 ;;
esac
"#,
    )
    .expect("write");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    path
}

/// Run `open-bft setup <source> --unshield <unshield>` in `home`.
fn setup_disc(home: &Path, source: &Path, unshield: &Path, unzbd: &Path, cache: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_open-bft"))
        .arg("setup")
        .arg(source)
        .arg("--unshield")
        .arg(unshield)
        .arg("--unzbd")
        .arg(unzbd)
        .arg("--cache-dir")
        .arg(cache)
        .current_dir(home)
        .env("HOME", home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env_remove("BFT_DATA_DIR")
        .env_remove("BFT_UNZBD")
        .env_remove("BFT_UNSHIELD")
        .output()
        .expect("the binary should run")
}

/// Three quarters of a second of a quiet tone, as CD audio sectors.
fn tone() -> Vec<u8> {
    let mut bytes = Vec::new();
    for n in 0..(56 * 2352 / 4) {
        let s = (3000.0 * (n as f64 * 0.0627).sin()) as i16;
        bytes.extend_from_slice(&s.to_le_bytes());
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    bytes
}

#[test]
fn setup_unpacks_the_discs_files_with_unshield() {
    let (home, disc, bin, cache) = (
        TempDir::new("home"),
        TempDir::new("disc"),
        TempDir::new("bin"),
        TempDir::new("cache"),
    );
    std::fs::write(disc.path.join("DATA1.CAB"), b"cabinet").expect("write");

    let out = setup_disc(
        &home.path,
        &disc.path,
        &fake_unshield(&bin),
        &fake_unzbd(&bin),
        &cache.path,
    );

    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", text(&out.stderr));
    assert!(stdout.contains("the disc's files"), "{stdout}");
    assert!(
        stdout.contains("Music is not read from a mounted disc"),
        "{stdout}"
    );

    let install = data_dir(&home.path).join("recoil");
    assert!(install.join("zbd/m1/gamez.zbd").is_file());
    assert!(
        install.join("fonts/eurosti.ttf").is_file(),
        "Font Files go to fonts/"
    );
    let config = std::fs::read_to_string(config_file(&home.path)).expect("read");
    assert!(
        config.contains(&install.to_string_lossy().to_string()),
        "{config}"
    );
}

#[test]
fn setup_takes_the_data_and_the_music_from_a_nero_image() {
    let (home, bin, cache) = (
        TempDir::new("home"),
        TempDir::new("bin"),
        TempDir::new("cache"),
    );
    let iso = bft_assets::test_support::iso_image(&[("DATA1.CAB;1", b"cabinet")]);
    let image = home.path.join("Recoil.nrg");
    std::fs::write(&image, bft_assets::test_support::nrg_image(&iso, &[tone()])).expect("write");
    let (unshield, unzbd) = (fake_unshield(&bin), fake_unzbd(&bin));

    let out = setup_disc(&home.path, &image, &unshield, &unzbd, &cache.path);

    let stdout = text(&out.stdout);
    assert!(out.status.success(), "{stdout}\n{}", text(&out.stderr));
    assert!(stdout.contains("a Nero disc image"), "{stdout}");
    let data = data_dir(&home.path);
    assert!(data.join("recoil/messages.dll").is_file());
    assert!(
        !data.join("recoil.disc").exists(),
        "the copied cabinet is cleaned up"
    );

    let wav = data.join("music/track02.wav");
    assert_eq!(
        std::fs::metadata(&wav).expect("track02.wav").len(),
        44 + 56 * 2352
    );
    let config = std::fs::read_to_string(config_file(&home.path)).expect("read");
    assert!(
        config.contains(&format!(
            "music_dir: Some(\"{}\")",
            data.join("music").display()
        )),
        "{config}"
    );

    // Again: nothing is redone.
    let again = setup_disc(&home.path, &image, &unshield, &unzbd, &cache.path);
    let stdout = text(&again.stdout);
    assert!(again.status.success(), "{stdout}\n{}", text(&again.stderr));
    assert!(stdout.contains("Already unpacked"), "{stdout}");
    assert!(stdout.contains("track 02: already there"), "{stdout}");
    assert!(stdout.contains("Nothing to do"), "{stdout}");
}

#[test]
fn setup_without_unshield_says_how_to_get_it() {
    let (home, disc, bin, cache) = (
        TempDir::new("home"),
        TempDir::new("disc"),
        TempDir::new("bin"),
        TempDir::new("cache"),
    );
    std::fs::write(disc.path.join("data1.cab"), b"cabinet").expect("write");

    let out = setup_disc(
        &home.path,
        &disc.path,
        Path::new("/nonexistent/unshield"),
        &fake_unzbd(&bin),
        &cache.path,
    );

    let stderr = text(&out.stderr);
    assert!(!out.status.success());
    assert!(stderr.contains("needs `unshield`"), "{stderr}");
    assert!(!data_dir(&home.path).join("recoil").exists());
}
