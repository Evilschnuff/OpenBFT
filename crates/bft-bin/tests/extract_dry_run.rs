//! `open-bft extract --dry-run` against a fake install.
//!
//! The command list is the contract between this binary and mech3ax, and the
//! output paths are the contract between the cache and every asset loader's
//! `AssetLoader::extensions()`. Both are snapshotted here so a rename shows
//! up as a test diff rather than as a loader that silently finds nothing.
//!
//! The tree is empty files with the real names; no game data is involved.

use std::path::{Path, PathBuf};
use std::process::Command;

/// A temporary directory that deletes itself when dropped.
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(tag: &str) -> TempDir {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);

        let path = std::env::temp_dir().join(format!(
            "open-bft-{tag}-{}-{}",
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

/// Build the retail file tree of docs/data/install.md out of empty files.
fn fake_install() -> TempDir {
    let dir = TempDir::new("install");
    let root = &dir.path;

    let touch = |path: PathBuf| {
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("mkdir");
        std::fs::write(path, b"z").expect("write");
    };

    touch(root.join("messages.dll"));
    for name in [
        "image.zbd",
        "interp.zbd",
        "zrdr.zbd",
        "soundsl.zbd",
        "soundsm.zbd",
        "soundsh.zbd",
    ] {
        touch(root.join("zbd").join(name));
    }

    for n in 1..=13 {
        let mission = root.join("zbd").join(format!("m{n}"));
        for name in [
            "gamez.zbd",
            "anim.zbd",
            "zrdr.zbd",
            "rtexture2.zbd",
            "rtexture4.zbd",
        ] {
            touch(mission.join(name));
        }
        // The software renderer's tiers, which must not be planned (docs/data/materials.md).
        touch(mission.join("texture2.zbd"));
        touch(mission.join("texture4.zbd"));
        // m13 ships no automap (docs/data/install.md).
        if n != 13 {
            touch(root.join("maps").join(format!("m{n}.zmap")));
        }
    }

    dir
}

/// A stand-in for `unzbd` that is never executed, only named.
///
/// `--dry-run` prints commands rather than running them, but the path still
/// has to exist: a named binary that is not there is a mistake worth
/// reporting, so `find_unzbd` rejects it.
fn fake_unzbd(dir: &TempDir) -> PathBuf {
    let path = dir.path.join("unzbd");
    std::fs::write(&path, b"#!/bin/sh\nexit 1\n").expect("write");

    path
}

/// Run the binary with a fake install and a fresh cache.
fn dry_run(install: &Path, cache: &Path, unzbd: &Path) -> (String, bool) {
    let out = Command::new(env!("CARGO_BIN_EXE_open-bft"))
        .args(["extract", "--dry-run"])
        .arg("--data-dir")
        .arg(install)
        .arg("--cache-dir")
        .arg(cache)
        .arg("--unzbd")
        .arg(unzbd)
        // The test must not pick up the developer's own install or config.
        .env_remove("BFT_DATA_DIR")
        .env_remove("BFT_UNZBD")
        .output()
        .expect("the binary should run");

    (
        String::from_utf8_lossy(&out.stdout).to_string(),
        out.status.success(),
    )
}

/// The command lines only, with the temp paths replaced so the snapshot is
/// stable across machines and runs.
fn normalise(stdout: &str, install: &Path, cache: &Path, unzbd: &Path) -> Vec<String> {
    stdout
        .lines()
        .filter(|l| l.contains(" rc "))
        .map(|l| {
            l.replace(&unzbd.to_string_lossy().to_string(), "unzbd")
                .replace(&install.to_string_lossy().to_string(), "<install>")
                .replace(&cache.to_string_lossy().to_string(), "<cache>")
        })
        .collect()
}

#[test]
fn dry_run_prints_the_expected_command_list() {
    let install = fake_install();
    let cache = TempDir::new("cache");
    let bin = TempDir::new("bin");
    let unzbd = fake_unzbd(&bin);

    let (stdout, ok) = dry_run(&install.path, &cache.path, &unzbd);
    assert!(ok, "--dry-run should exit zero:\n{stdout}");

    let commands = normalise(&stdout, &install.path, &cache.path, &unzbd);

    // 5 globals + 12 automaps + 13 missions x (gamez, anim, reader, two
    // hardware texture archives).
    assert_eq!(commands.len(), 5 + 12 + 13 * 5, "{commands:#?}");

    // The globals, in the order `plan` emits them.
    assert_eq!(
        commands[0],
        "unzbd rc interp <install>/zbd/interp.zbd <cache>/interp.json"
    );
    assert_eq!(
        commands[1],
        "unzbd rc messages <install>/messages.dll <cache>/messages.json"
    );
    assert_eq!(
        commands[2],
        "unzbd rc reader <install>/zbd/zrdr.zbd <cache>/reader/zrdr.rdr.zip"
    );
    assert_eq!(
        commands[3],
        "unzbd rc textures <install>/zbd/image.zbd <cache>/textures/image.tex.zip"
    );
    assert_eq!(
        commands[4],
        "unzbd rc sounds <install>/zbd/soundsh.zbd <cache>/sounds/soundsh.snd.zip"
    );

    // The automaps: m1-m12, never m13.
    assert_eq!(
        commands[5],
        "unzbd rc zmap <install>/maps/m1.zmap <cache>/zmap/m1.zmap.json"
    );
    assert!(!commands.iter().any(|c| c.contains("m13.zmap")));

    // One mission's five jobs, in order.
    assert_eq!(
        commands[17..22],
        [
            "unzbd rc gamez <install>/zbd/m1/gamez.zbd <cache>/gamez/m1-gamez.gamez.zip",
            "unzbd rc anim <install>/zbd/m1/anim.zbd <cache>/anim/m1-anim.anim.zip",
            "unzbd rc reader <install>/zbd/m1/zrdr.zbd <cache>/reader/m1-zrdr.rdr.zip",
            "unzbd rc textures <install>/zbd/m1/rtexture4.zbd <cache>/textures/m1-rtexture4.tex.zip",
            "unzbd rc textures <install>/zbd/m1/rtexture2.zbd <cache>/textures/m1-rtexture2.tex.zip",
        ]
    );

    // The software renderer's tiers are never extracted (docs/data/materials.md).
    assert!(!commands.iter().any(|c| c.contains("/texture2.zbd")));
    assert!(!commands.iter().any(|c| c.contains("/texture4.zbd")));
    // Nor the low and medium sound tiers (docs/data/sounds.md).
    assert!(!commands.iter().any(|c| c.contains("soundsl")));
    assert!(!commands.iter().any(|c| c.contains("soundsm")));
}

#[test]
fn dry_run_summarises_the_plan_before_the_commands() {
    let install = fake_install();
    let cache = TempDir::new("cache");
    let bin = TempDir::new("bin");

    let (stdout, _) = dry_run(&install.path, &cache.path, &fake_unzbd(&bin));

    assert!(stdout.contains("(retail)"), "{stdout}");
    assert!(
        stdout.contains("82 job(s), 82 to run, 0 up to date"),
        "{stdout}"
    );
}

#[test]
fn no_data_root_exits_two_and_says_where_it_looked() {
    let empty = TempDir::new("empty");

    let out = Command::new(env!("CARGO_BIN_EXE_open-bft"))
        .args(["extract", "--dry-run"])
        .arg("--data-dir")
        .arg(&empty.path)
        .env_remove("BFT_DATA_DIR")
        .output()
        .expect("the binary should run");

    // FR-D1: the message lists every path tried, and the exit code is 2.
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("Looked in:"), "{stderr}");
    assert!(
        stderr.contains(&empty.path.to_string_lossy().to_string()),
        "{stderr}"
    );
    assert!(stderr.contains("BFT_DATA_DIR"), "{stderr}");
}
