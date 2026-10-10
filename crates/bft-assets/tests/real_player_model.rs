//! The `bft_00` subtree against a real Recoil install.
//!
//! These are the player model's data-dependent facts (docs/data/nodes.md), run as
//! tests. They skip (not
//! fail) when `BFT_DATA_DIR` is unset or the cache has not been built, so a
//! contributor without the game still runs `cargo test --workspace` green.
//! Build the cache first with `open-bft extract`.
//!
//! The two that matter are [`the_turret_and_the_gun_are_separate_joints`] and
//! [`the_firepoints_are_transform_only_children_of_the_gun`]:
//! `bft_game::player_model` is built on the answers, and a gamez
//! change or a loader regression that moved either would otherwise show up as
//! a barrel pointing the wrong way in one level and nowhere else.

use std::path::PathBuf;

use bevy::prelude::*;
use bft_assets::DataPaths;
use bft_assets::level::{LevelData, NodeDesc, TransformDesc, parse_gamez};

/// The resolved paths for a real install, or `None` when there is no data.
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

fn gamez_path(paths: &DataPaths, level: u16) -> PathBuf {
    paths
        .cache
        .path
        .join(format!("gamez/m{level}-gamez.gamez.zip"))
}

fn load(paths: &DataPaths, level: u16) -> Option<LevelData> {
    let path = gamez_path(paths, level);
    if !path.is_file() {
        eprintln!("{} is not in the cache; skipping m{level}", path.display());
        return None;
    }

    let bytes = std::fs::read(&path).expect("the cache is readable");
    match parse_gamez(&bytes, &format!("m{level}")) {
        Ok(level) => Some(level),
        Err(e) => panic!("m{level} failed to load: {e}"),
    }
}

fn every_level(paths: &DataPaths) -> Vec<(u16, LevelData)> {
    (1..=13)
        .filter_map(|n| load(paths, n).map(|level| (n, level)))
        .collect()
}

/// The `bft_00` root of a level, which every level has.
fn player_root(level: &LevelData) -> u32 {
    (0..level.nodes.len() as u32)
        .find(|id| level.node(*id).is_some_and(|node| node.name() == "bft_00"))
        .expect("every level has a `bft_00` template root")
}

/// Every node id in a subtree, breadth-first.
fn subtree(level: &LevelData, root: u32) -> Vec<u32> {
    let mut queue = std::collections::VecDeque::from([root]);
    let mut out = Vec::new();

    while let Some(id) = queue.pop_front() {
        let Some(node) = level.node(id) else { continue };
        out.push(id);
        queue.extend(node.common().children.iter().copied());
    }

    out
}

/// The first node of this name in a subtree.
fn find(level: &LevelData, root: u32, name: &str) -> Option<u32> {
    subtree(level, root)
        .into_iter()
        .find(|id| level.node(*id).is_some_and(|node| node.name() == name))
}

/// A node's own transform, as the data states it.
fn transform(level: &LevelData, id: u32) -> TransformDesc {
    match level.node(id).expect("in the subtree") {
        NodeDesc::Object3d(object) => object.transform,
        _ => TransformDesc::Initial,
    }
}

/// Every level carries `bft_00`, and its node count grows through the
/// campaign as weapon models are added under `gun` (docs/data/nodes.md and docs/data/reader.md).
#[test]
fn every_level_has_the_player_subtree_and_it_grows() {
    let Some(paths) = paths() else { return };

    // level → nodes, from walking all 13.
    let expected = [
        (1, 49),
        (2, 57),
        (3, 65),
        (4, 63),
        (5, 72),
        (6, 72),
        (7, 72),
        (8, 72),
        (9, 72),
        (10, 72),
        (11, 72),
        (12, 72),
        (13, 72),
    ];

    for (n, level) in every_level(&paths) {
        let count = subtree(&level, player_root(&level)).len();
        let want = expected
            .iter()
            .find(|(m, _)| *m == n)
            .map(|(_, count)| *count)
            .expect("all 13 are listed");

        assert_eq!(count, want, "m{n}'s `bft_00` should have {want} nodes");
    }
}

/// **`bft_00` hangs off the World node, it is not a template root.**
///
/// This corrects docs/data/reader.md, and is the reason
/// the player's model is re-parented rather than cloned: `LevelTemplates`
/// walks `root_nodes`, so a clone-based attach silently finds nothing. m1's
/// `bft` twin is the only parentless one, and it is deliberately unused.
#[test]
fn the_player_subtree_is_a_child_of_the_world_node() {
    let Some(paths) = paths() else { return };

    for (n, level) in every_level(&paths) {
        let root = player_root(&level);

        assert_eq!(
            level.node(root).expect("found").common().parent,
            Some(level.world_node),
            "m{n}: `bft_00` should hang off the World node"
        );
        assert!(
            !level.root_nodes.contains(&root),
            "m{n}: `bft_00` should not be a parentless template root"
        );
    }
}

/// **Nearly every node of the subtree collides**, so the level spawner gives each
/// one a static body and a trimesh collider.
///
/// `bft_game::player_model` strips them as it re-parents. Left on, they make
/// the model lie flat, float, then tip and spin: a static body does not
/// follow its parent, and avian resolves it against the hull's own box.
#[test]
fn the_subtree_is_almost_entirely_collidable() {
    let Some(paths) = paths() else { return };

    for (n, level) in every_level(&paths) {
        let nodes = subtree(&level, player_root(&level));
        let collides = nodes
            .iter()
            .filter(|id| {
                level
                    .node(**id)
                    .is_some_and(|node| node.common().flags.collides())
            })
            .count();

        // 48 of 49 in m1, 71 of 72 in m5. Exactly one node in each subtree
        // does not collide, which is enough of a pattern to pin.
        assert_eq!(
            collides,
            nodes.len() - 1,
            "m{n}: all but one of the {} nodes should collide",
            nodes.len()
        );
    }
}

/// **The finding the player model is built on.** `turret` and `gun` are two
/// separate joints: transform-only nodes with translation-only transforms,
/// one under the other.
///
/// If this fails, `bft_game::player_model` is marking the wrong nodes and
/// `aim_turret` will pitch the whole turret ring — the doors and the turret
/// polys tipping with the barrel.
#[test]
fn the_turret_and_the_gun_are_separate_joints() {
    let Some(paths) = paths() else { return };

    for (n, level) in every_level(&paths) {
        let root = player_root(&level);

        let turret = find(&level, root, "turret").unwrap_or_else(|| panic!("m{n} has a turret"));
        let gun = find(&level, root, "gun").unwrap_or_else(|| panic!("m{n} has a gun"));

        // The gun is *under* the turret, not a sibling: that is what makes
        // pitching it independent of the ring's yaw.
        assert_eq!(
            level.node(gun).expect("found").common().parent,
            Some(turret),
            "m{n}: `gun` should be a child of `turret`"
        );

        for (id, name) in [(turret, "turret"), (gun, "gun")] {
            let node = level.node(id).expect("found");

            // No mesh: a joint is a transform, and its children carry the
            // geometry.
            assert!(
                node.common().model_index.is_none(),
                "m{n}: `{name}` should be transform-only"
            );

            // Translation only: a joint's own rotation is the game's to
            // write, so the data states none.
            match transform(&level, id) {
                TransformDesc::Matrix { rotation, .. } => assert!(
                    (rotation - Mat3::IDENTITY)
                        .to_cols_array()
                        .iter()
                        .all(|v| v.abs() < 1e-6),
                    "m{n}: `{name}` should carry no rotation of its own"
                ),
                other => panic!("m{n}: `{name}` has an unexpected transform: {other:?}"),
            }
        }
    }
}

/// The muzzle is `fpnt_c`, a transform-only child of `gun`, and it is in the
/// same place in all 13 levels.
///
/// `bft_game::player_model` composes this down to hull space for the
/// weapons' `Muzzle`, in place of a guessed −3.5 m.
#[test]
fn the_firepoints_are_transform_only_children_of_the_gun() {
    let Some(paths) = paths() else { return };

    let mut centres = Vec::new();

    for (n, level) in every_level(&paths) {
        let root = player_root(&level);
        let gun = find(&level, root, "gun").expect("there is a gun");

        for name in ["fpnt_l", "fpnt_c", "fpnt_r"] {
            let id = find(&level, root, name).unwrap_or_else(|| panic!("m{n} has `{name}`"));

            assert_eq!(
                level.node(id).expect("found").common().parent,
                Some(gun),
                "m{n}: `{name}` should hang off the gun"
            );
            assert!(
                level
                    .node(id)
                    .expect("found")
                    .common()
                    .model_index
                    .is_none(),
                "m{n}: `{name}` is a point, not a mesh"
            );
        }

        let centre = match transform(&level, find(&level, root, "fpnt_c").expect("found")) {
            TransformDesc::Matrix { translation, .. } => translation,
            other => panic!("m{n}: `fpnt_c` has an unexpected transform: {other:?}"),
        };

        // Forward of the gun's origin, which is −Z in gamez space.
        assert!(
            centre.z < -2.0,
            "m{n}: the muzzle should be out at the end of the barrel, got {centre:?}"
        );

        centres.push((n, centre));
    }

    // All 13 agree, so a single constant would have been correct — but it is
    // read from the data rather than hard-coded, and this is what says the
    // reading is stable.
    if let Some(((_, first), rest)) = centres.split_first() {
        for (n, centre) in rest {
            assert!(
                centre.distance(*first) < 1e-4,
                "m{n}'s muzzle sits at {centre:?}, m1's at {first:?}"
            );
        }
    }
}

/// From m5 the subtree carries every form's geometry at once, and the engine
/// picks by visibility rather than by swapping models.
///
/// `bft_game::player_model` hides the non-track set at spawn. If a name here
/// changed, the player would drive a tank wearing its pontoons.
#[test]
fn the_late_levels_carry_every_form_at_once() {
    let Some(paths) = paths() else { return };

    // The nodes that only exist once the vehicle can change form.
    let form_nodes = [
        "sub_chassis",
        "s_turret",
        "wings_hover",
        "wings_sub",
        "hover_shadow",
        "amphib_shadow",
        "rpont",
        "lpont",
    ];

    for (n, level) in every_level(&paths) {
        let root = player_root(&level);
        let present = form_nodes
            .iter()
            .filter(|name| find(&level, root, name).is_some())
            .count();

        if n >= 5 {
            assert_eq!(
                present,
                form_nodes.len(),
                "m{n} should carry all four forms' geometry"
            );
        } else {
            // The early levels are the tank set only, which is why the
            // subtree grows. m1 has none of them at all.
            assert!(
                present < form_nodes.len(),
                "m{n} should not have the full set yet, got {present}"
            );
        }
    }
}

/// The tank tracks are there to hang the texture cycle on, in every level.
#[test]
fn both_track_nodes_exist_everywhere() {
    let Some(paths) = paths() else { return };

    for (n, level) in every_level(&paths) {
        let root = player_root(&level);

        for name in ["rtracks", "ltracks"] {
            assert!(
                find(&level, root, name).is_some(),
                "m{n} should have `{name}` for the track cycle"
            );
        }
    }
}

/// m1 alone carries a second copy named `bft`, and it is the same tree.
///
/// `player_model` uses `bft_00`, the name `playerName` binds and the only one
/// all 13 levels have. This pins the twin so its appearing elsewhere — or the
/// two drifting apart — is noticed rather than silently picked up.
#[test]
fn only_m1_has_the_bft_twin_and_it_matches() {
    let Some(paths) = paths() else { return };

    for (n, level) in every_level(&paths) {
        let twin = (0..level.nodes.len() as u32)
            .find(|id| level.node(*id).is_some_and(|node| node.name() == "bft"));

        match (n, twin) {
            (1, Some(twin)) => {
                let mine = subtree(&level, twin).len();
                let theirs = subtree(&level, player_root(&level)).len();
                assert_eq!(mine, theirs, "m1's `bft` twin should match `bft_00`");
            }
            (1, None) => panic!("m1 should have its `bft` twin"),
            (_, Some(_)) => panic!("m{n} unexpectedly has a `bft` root as well as `bft_00`"),
            (_, None) => {}
        }
    }
}
