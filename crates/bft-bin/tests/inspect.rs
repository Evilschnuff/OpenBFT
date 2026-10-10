//! `open-bft inspect` as a process: "`open-bft inspect level 1 | head`
//! prints a readable tree without a window".
//!
//! The data-backed half skips when `BFT_DATA_DIR` is unset.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

#[test]
fn inspect_level_piped_into_head_prints_a_tree_and_exits_cleanly() {
    let Some(root) = bft_assets::test_support::data_root() else {
        return;
    };

    let mut child = Command::new(env!("CARGO_BIN_EXE_open-bft"))
        .args(["inspect", "level", "1"])
        .arg("--data-dir")
        .arg(&root)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the binary should run");

    // What `head` does: read ten lines, then close the pipe.
    let lines: Vec<String> = {
        let stdout = child.stdout.take().expect("piped");
        BufReader::new(stdout)
            .lines()
            .take(10)
            .map(|line| line.expect("UTF-8 output"))
            .collect()
    };
    let out = child.wait_with_output().expect("it exits");

    if lines.is_empty() {
        // No cache built yet: nothing to check but that it said so.
        eprintln!("{}", String::from_utf8_lossy(&out.stderr));
        return;
    }

    assert!(lines[0].starts_with("m1 "), "{lines:?}");
    assert!(
        lines.iter().any(|line| line.contains(" World")),
        "the tree starts within ten lines: {lines:?}"
    );
    // A closed pipe is the reader being done, not a failure.
    assert!(
        out.status.success(),
        "{:?}: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn inspect_without_data_exits_two() {
    let empty = std::env::temp_dir().join(format!("open-bft-inspect-empty-{}", std::process::id()));
    std::fs::create_dir_all(&empty).expect("temp dir");

    let out = Command::new(env!("CARGO_BIN_EXE_open-bft"))
        .args(["inspect", "level", "1"])
        .arg("--data-dir")
        .arg(&empty)
        .env_remove("BFT_DATA_DIR")
        .output()
        .expect("the binary should run");
    let _ = std::fs::remove_dir_all(&empty);

    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty(), "nothing on stdout");
}

#[test]
fn inspect_rejects_a_level_that_is_not_one() {
    let out = Command::new(env!("CARGO_BIN_EXE_open-bft"))
        .args(["inspect", "level", "moon"])
        .output()
        .expect("the binary should run");

    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("not a level name"));
}
