//! The vehicle's checks that need a real Recoil install.
//!
//! They skip (not fail) when `BFT_DATA_DIR` is unset or the cache has not
//! been built, so `cargo test --workspace` stays green for a contributor
//! without the game. No game data enters the repository: only counts,
//! invariants and the handful of values docs/data/reader.md and READER-SCHEMA already
//! publish are asserted.
//!
//! Build the cache first with `open-bft extract`.
//!
//! These read the reader archive straight off disk rather than through the
//! asset server: `bft-sim` has no renderer and needs none of Bevy's asset
//! plumbing to answer "what does `vehicle.zrd` say".

use bft_assets::ReaderData;
use bft_sim::vehicle::tuning::{Difficulty, VehicleKindId, build_vehicle_table};
use bft_sim::vehicle::{VehicleForm, VehicleOverrides, stated_transitions, transition_allowed};

/// The extraction cache, or `None` when there is no data to test against.
fn cache() -> Option<std::path::PathBuf> {
    let root = bft_assets::test_support::data_root()?;

    let paths = match bft_assets::DataPaths::discover(Some(&root), None, None) {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("cannot resolve the data paths ({e}); skipping");
            return None;
        }
    };

    if !paths.cache.path.join("reader").is_dir() {
        eprintln!(
            "no extraction cache at {}; run `open-bft extract`. Skipping.",
            paths.cache.path.display()
        );
        return None;
    }

    Some(paths.cache.path)
}

/// The global reader archive, which holds `vehicle*.zrd` and `player.zrd`.
fn global_reader() -> Option<ReaderData> {
    let cache = cache()?;
    let path = cache.join("reader/zrdr.rdr.zip");

    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("cannot read {}: {e}; skipping", path.display());
            return None;
        }
    };

    match bft_assets::reader::parse_reader_data(&bytes) {
        Ok(archive) => Some(archive),
        Err(e) => {
            eprintln!("cannot parse the global reader archive: {e}; skipping");
            None
        }
    }
}

/// The 25 kinds READER-SCHEMA §2.1 counts, and the player among them.
#[test]
fn the_table_holds_every_vehicle_the_schema_counts() {
    let Some(reader) = global_reader() else {
        return;
    };

    let table = build_vehicle_table(&reader, Difficulty::Normal, &VehicleOverrides::default())
        .expect("the shipped vehicle.zrd builds a table");

    assert_eq!(table.kinds.len(), 25, "READER-SCHEMA §2.1 counts 25 kinds");
    assert!(table.player_def().is_some(), "the player is one of them");

    for name in ["scou", "rumv", "stealth", "rumv_easy", "dron_2"] {
        assert!(
            table.kind(&VehicleKindId(name.to_string())).is_some(),
            "`{name}` should be in the table"
        );
    }
}

/// The player has all four forms and nobody else has more than one shape of
/// mode block that this reads.
#[test]
fn the_player_has_all_four_forms_and_the_enemies_have_their_own() {
    let Some(reader) = global_reader() else {
        return;
    };

    let table = build_vehicle_table(&reader, Difficulty::Normal, &VehicleOverrides::default())
        .expect("the table builds");
    let player = table.player_def().expect("the player");

    for form in VehicleForm::ALL {
        assert!(
            player.mode(form).is_some(),
            "the player should have a {form:?} mode"
        );
    }

    // READER-SCHEMA §2.1's counts: track 19, hover 4, amphib 2, sub 2.
    let count = |form: VehicleForm| {
        table
            .kinds
            .values()
            .filter(|def| def.mode(form).is_some())
            .count()
    };

    assert_eq!(count(VehicleForm::Track), 19);
    assert_eq!(count(VehicleForm::Hover), 4);
    assert_eq!(count(VehicleForm::Amphib), 2);
    assert_eq!(count(VehicleForm::Sub), 2);
}

/// The transition graph, read out of the data rather than asserted: track↔amphib, track↔hover, amphib↔hover, amphib↔sub, and **no
/// track↔sub**.
#[test]
fn the_transition_graph_is_the_one_the_anim_keys_state() {
    let Some(reader) = global_reader() else {
        return;
    };

    let table = build_vehicle_table(&reader, Difficulty::Normal, &VehicleOverrides::default())
        .expect("the table builds");
    let stated = stated_transitions(table.player_def().expect("the player"));

    assert_eq!(stated.len(), 8, "eight `*_anims` keys, eight transitions");

    for (from, to) in stated.keys() {
        assert!(
            transition_allowed(*from, *to),
            "{from:?} -> {to:?} is in the data but not in TRANSITIONS"
        );
    }
    for (from, to) in bft_sim::vehicle::TRANSITIONS {
        assert!(
            stated.contains_key(&(from, to)),
            "{from:?} -> {to:?} is in TRANSITIONS but not in the data"
        );
    }

    assert!(
        !stated.contains_key(&(VehicleForm::Track, VehicleForm::Sub)),
        "there is no direct track-to-sub transition"
    );

    // The pairs name real anims: "leave the old form", "enter the new one".
    let to_sub = stated
        .get(&(VehicleForm::Amphib, VehicleForm::Sub))
        .expect("amphib -> sub");
    assert_eq!(to_sub[1], "body_to_sub");
}

/// `player.zrd`'s gravity is 40 m/s², not 9.81 — the single number the whole
/// feel of the game rests on (docs/data/water.md).
#[test]
fn the_player_constants_are_the_shipped_ones() {
    let Some(reader) = global_reader() else {
        return;
    };

    let table = build_vehicle_table(&reader, Difficulty::Normal, &VehicleOverrides::default())
        .expect("the table builds");
    let player = &table.player;

    assert_eq!(player.gravity, 40.0);
    assert_eq!(player.water_gravity, 9.0);
    assert_eq!(player.quicksand_gravity, 3.0);
    assert!((player.max_slope_cos - 0.8192).abs() < 1e-6);
    assert!((player.camera.zone - 0.7).abs() < 1e-6);
    assert!((player.camera.max_yaw_rate - 0.87).abs() < 1e-6);
    assert_eq!(player.camera.max_tether_angle_deg, 20.0);
}

/// The player's own `track_mode` numbers, as READER-SCHEMA §2.1 publishes
/// them. If mech3ax's output shape changes, this is what says so.
#[test]
fn the_players_track_mode_is_the_one_the_schema_publishes() {
    let Some(reader) = global_reader() else {
        return;
    };

    let table = build_vehicle_table(&reader, Difficulty::Normal, &VehicleOverrides::default())
        .expect("the table builds");
    let track = table
        .player_def()
        .and_then(|def| def.mode(VehicleForm::Track))
        .expect("the player's track mode");

    assert_eq!(track.rates, [48.0, 65.0]);
    assert_eq!(track.turns, [2.5, 3.5]);
    assert_eq!(track.mode_alt, 0.0);
    assert!(track.ok_to_amphib, "the player may transform in water");
    assert_eq!(track.mass, Some(2.2));
    assert_eq!(track.quicksand_slowdown, Some(0.4));
    assert_eq!(track.lava_slowdown, Some(0.2));

    // The suspension quad is the ±2.25 x -2.4..2.34 the schema names, and it
    // is what the derived rays are.
    let platform = track.platform.expect("the player's modes have a platform");
    assert_eq!(platform[0].x, -2.25);
    assert_eq!(platform[2].z, 2.34);
    assert_eq!(track.suspension.rays.len(), 4);

    // And the modes' ride heights are the four the schema lists.
    let alt = |form| {
        table
            .player_def()
            .and_then(|def| def.mode(form))
            .map(|mode| mode.mode_alt)
    };
    assert_eq!(alt(VehicleForm::Amphib), Some(-0.1));
    assert_eq!(alt(VehicleForm::Hover), Some(0.5));
    assert_eq!(alt(VehicleForm::Sub), Some(-0.5));
}

/// Difficulty selects a whole file, and all three of them parse.
#[test]
fn every_difficulty_builds_a_table() {
    let Some(reader) = global_reader() else {
        return;
    };

    for difficulty in [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard] {
        let table = build_vehicle_table(&reader, difficulty, &VehicleOverrides::default())
            .unwrap_or_else(|e| panic!("{difficulty:?} should build a table: {e}"));

        assert_eq!(table.kinds.len(), 25, "{difficulty:?}");
        assert!(table.player_def().is_some(), "{difficulty:?}");
    }
}

/// Every mode of every kind derives usable controller constants: a zero top
/// speed or a zero spring is a vehicle that cannot move, and the fallbacks
/// exist so the 23 of 28 mode blocks without a `platform` still work.
#[test]
fn every_mode_of_every_kind_derives_a_drivable_controller() {
    let Some(reader) = global_reader() else {
        return;
    };

    let table = build_vehicle_table(&reader, Difficulty::Normal, &VehicleOverrides::default())
        .expect("the table builds");

    let mut modes = 0;
    let mut with_platform = 0;

    for (id, def) in &table.kinds {
        for form in VehicleForm::ALL {
            let Some(mode) = def.mode(form) else {
                continue;
            };
            modes += 1;
            if mode.platform.is_some() {
                with_platform += 1;
            }

            let where_ = format!("{}/{form:?}", id.as_str());
            assert!(mode.max_speed > 0.0, "{where_}: no top speed");
            assert!(mode.engine_force > 0.0, "{where_}: no engine");
            assert!(mode.lateral_grip > 0.0, "{where_}: no grip");
            assert_eq!(mode.suspension.rays.len(), 4, "{where_}: ray count");
            assert!(mode.suspension.stiffness > 0.0, "{where_}: no spring");
            assert!(mode.suspension.damping > 0.0, "{where_}: no damper");
            assert!(
                mode.suspension.max_len > mode.suspension.rest,
                "{where_}: no travel"
            );
        }
    }

    // 27 of the 28 mode blocks are drivable forms; the 28th is `stealth`'s
    // `basic_mode`, which is not a form and is skipped.
    assert_eq!(modes, 27, "one drivable mode per non-basic mode block");
    // Only the player's four carry a `platform`; `stealth`'s fifth is in the
    // block this table does not keep.
    assert_eq!(with_platform, 4, "only the player's modes state a quad");
}

/// There is no `bft_multi` build: every level, multiplayer included, has the
/// same `bft_00` subtree.
///
/// Recorded so nobody goes looking for a node that is not there.
#[test]
fn there_is_no_separate_multiplayer_player_model() {
    let Some(cache) = cache() else {
        return;
    };

    for level in 1..=13 {
        let path = cache.join(format!("gamez/m{level}-gamez.gamez.zip"));
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let Ok(data) = bft_assets::level::parse_gamez(&bytes, &path.to_string_lossy()) else {
            continue;
        };

        let names: Vec<&str> = data
            .nodes
            .iter()
            .map(|node| node.common().name.as_str())
            .collect();

        assert!(
            names.contains(&"bft_00"),
            "m{level} should have the player's subtree"
        );
        assert!(
            !names.iter().any(|name| name.starts_with("bft_multi")),
            "m{level} has a bft_multi node after all"
        );
        // And the turret the model needs is under it, on every level, which
        // is what the player model's turret joint relies on.
        assert!(names.contains(&"turret"), "m{level} has no turret node");
        assert!(names.contains(&"gun"), "m{level} has no gun node");

        // The subtree grows through the campaign as weapon models are added
        // under `gun` (docs/data/reader.md): 49 nodes in m1 up to 72 in m5-m13.
        // The player model attaches whichever it finds, so the point is only that
        // it is a whole vehicle and not a stub.
        let subtree = subtree_size(&data, "bft_00");
        assert!(
            (49..=72).contains(&subtree),
            "m{level}'s bft_00 subtree is {subtree} nodes, outside the measured 49-72"
        );
    }
}

/// How many nodes a named subtree holds, the root included.
fn subtree_size(data: &bft_assets::level::LevelData, name: &str) -> usize {
    let Some(root) = data
        .nodes
        .iter()
        .position(|node| node.common().name == name)
    else {
        return 0;
    };

    let mut stack = vec![root];
    let mut seen = 0;

    while let Some(index) = stack.pop() {
        seen += 1;
        let Some(node) = data.nodes.get(index) else {
            continue;
        };
        stack.extend(node.common().children.iter().map(|child| *child as usize));
    }

    seen
}

/// **The `collision` ring is a hand-authored outline, and the collider keeps
/// its shape.**
///
/// The player's four modes each state twelve points at
/// `y = 1`, and they are not a rectangle — the waist bulges past the corners,
/// and the four forms differ. An axis-aligned box over them (which is what
/// `hull_box` returns) makes the vehicle fatter at its ends than the
/// original drew it.
///
/// This pins the shape so a future change back to a box is noticed.
#[test]
fn the_players_collision_rings_are_outlines_rather_than_boxes() {
    let Some(reader) = global_reader() else {
        return;
    };

    let table = build_vehicle_table(&reader, Difficulty::Normal, &VehicleOverrides::default())
        .expect("the shipped vehicle.zrd builds a table");
    let def = table.player_def().expect("the player is in the table");

    // Every form the player can take states a ring, and they are not the
    // same ring: the shape is per form, so a collider built once from the
    // spawn form is wrong after a transformation.
    let mut widths = Vec::new();

    for form in [
        VehicleForm::Track,
        VehicleForm::Amphib,
        VehicleForm::Hover,
        VehicleForm::Sub,
    ] {
        let mode = def.mode(form).unwrap_or_else(|| panic!("{form:?} exists"));
        let ring = &mode.collision;

        assert_eq!(ring.len(), 12, "{form:?} should state twelve points");

        // All at one height: the ring is a flat outline and says nothing
        // about how tall the hull is, which is why `HULL_HEIGHT` is an
        // approximation rather than a reading.
        assert!(
            ring.iter().all(|point| (point.y - 1.0).abs() < 1e-6),
            "{form:?}'s ring should be flat at y = 1"
        );

        // The waist is wider than the corners, which is the whole point: a
        // box over the extent would push the corners out to the waist.
        let waist = ring
            .iter()
            .filter(|point| point.z.abs() < 0.001)
            .map(|point| point.x.abs())
            .fold(0.0f32, f32::max);
        let corner = ring
            .iter()
            .filter(|point| point.z.abs() > 3.0)
            .map(|point| point.x.abs())
            .fold(0.0f32, f32::max);

        assert!(
            waist > corner,
            "{form:?} should bulge at the waist: {waist} vs {corner} at the corners"
        );

        widths.push((form, waist));
    }

    // Hover is the widest and sub the narrowest, so a collider that ignored
    // the form would be wrong by more than a metre on each side.
    let hover = widths
        .iter()
        .find(|(form, _)| *form == VehicleForm::Hover)
        .expect("hover is there")
        .1;
    let sub = widths
        .iter()
        .find(|(form, _)| *form == VehicleForm::Sub)
        .expect("sub is there")
        .1;

    assert!(
        hover - sub > 1.0,
        "the forms should differ by more than a metre: hover {hover}, sub {sub}"
    );
}
