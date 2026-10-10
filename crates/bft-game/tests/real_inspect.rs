//! `open-bft inspect` against a real Recoil install.
//!
//! Skips (not fails) when `BFT_DATA_DIR` is unset or the cache has not been
//! built, like every data-backed test. Only shapes are asserted, never game
//! text.

use bft_assets::DataPaths;
use bft_assets::axes::AxisConvention;
use bft_assets::catalog::LevelId;
use bft_game::devtools::inspect::{self, InspectError, InspectTarget};

/// The resolved paths for a real install with a cache, or `None`.
fn paths() -> Option<DataPaths> {
    let root = bft_assets::test_support::data_root()?;

    match DataPaths::discover(Some(&root), None, None) {
        Ok(paths) if paths.cache.path.is_dir() => Some(paths),
        Ok(paths) => {
            eprintln!(
                "no extraction cache at {}; run `open-bft extract`. Skipping.",
                paths.cache.path.display()
            );
            None
        }
        Err(e) => {
            eprintln!("cannot resolve the data paths ({e}); skipping");
            None
        }
    }
}

/// Run one target and return what it printed.
fn inspect(paths: &DataPaths, target: InspectTarget) -> Result<String, InspectError> {
    let mut out = Vec::new();
    inspect::run(
        paths.clone(),
        AxisConvention::GAMEZ_DEFAULT,
        &target,
        &mut out,
    )?;
    Ok(String::from_utf8(out).expect("the output is UTF-8"))
}

/// `inspect level 1 | head` is a readable tree.
#[test]
fn level_1_prints_statistics_then_a_tree_rooted_at_the_world() {
    let Some(paths) = paths() else {
        return;
    };

    let text = inspect(&paths, InspectTarget::Level(LevelId(1))).expect("m1 inspects");
    let lines: Vec<&str> = text.lines().collect();

    assert!(lines[0].starts_with("m1 "), "{}", lines[0]);
    assert!(lines[1].starts_with("nodes "), "{}", lines[1]);
    assert!(lines[2].starts_with("models "), "{}", lines[2]);
    assert!(lines[3].starts_with("materials "), "{}", lines[3]);

    // The first tree line is the world node, unindented; its children are
    // indented under it.
    let tree: Vec<&str> = lines
        .iter()
        .skip_while(|line| !line.is_empty())
        .skip(1)
        .copied()
        .collect();
    assert!(tree[0].contains(" World"), "{}", tree[0]);
    assert!(!tree[0].starts_with(' '));
    assert!(tree[1].starts_with("  "), "{}", tree[1]);
    // Every node is printed once: one line per node.
    let nodes: usize = lines[1]
        .split_whitespace()
        .nth(1)
        .and_then(|count| count.parse().ok())
        .expect("a node count");
    assert_eq!(tree.len(), nodes);
}

#[test]
fn every_other_target_prints_for_m1() {
    let Some(paths) = paths() else {
        return;
    };

    let textures = inspect(&paths, InspectTarget::Textures("m1".into())).expect("textures");
    assert!(
        textures
            .lines()
            .next()
            .is_some_and(|l| l.contains("textures"))
    );

    let reader = inspect(
        &paths,
        InspectTarget::Reader {
            level: Some(LevelId(1)),
            entry: None,
        },
    )
    .expect("reader list");
    assert!(reader.contains("aiv.zrd"), "{reader}");

    let aiv = inspect(
        &paths,
        InspectTarget::Reader {
            level: Some(LevelId(1)),
            entry: Some("aiv".into()),
        },
    )
    .expect("one reader entry");
    assert!(aiv.trim_start().starts_with(['{', '[']), "JSON");

    let interp = inspect(
        &paths,
        InspectTarget::Interp {
            level: Some(LevelId(1)),
        },
    )
    .expect("interp");
    assert!(interp.contains("RuntimeBindings"), "{interp}");

    let messages = inspect(&paths, InspectTarget::Messages { filter: None }).expect("messages");
    assert!(messages.contains("MSG_"), "the keys are listed");

    let anim = inspect(
        &paths,
        InspectTarget::Anim {
            level: LevelId(1),
            name: None,
        },
    )
    .expect("anim");
    assert!(
        anim.lines()
            .next()
            .is_some_and(|l| l.contains("definitions"))
    );
}

#[test]
fn an_unknown_level_is_an_error_not_a_panic() {
    let Some(paths) = paths() else {
        return;
    };

    assert!(matches!(
        inspect(&paths, InspectTarget::Level(LevelId(99))),
        Err(InspectError::UnknownLevel(_))
    ));
}
