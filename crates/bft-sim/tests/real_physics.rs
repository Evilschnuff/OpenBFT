//! The physics' checks that need a real Recoil install.
//!
//! They skip (not fail) when `BFT_DATA_DIR` is unset or the cache has not
//! been built, so `cargo test --workspace` stays green for a contributor
//! without the game. No game data enters the repository: only counts and
//! invariants are asserted.
//!
//! Build the cache first with `open-bft extract`.
//!
//! These read the `gamez` archives straight off disk rather than through the
//! asset server: `bft-sim` has no renderer and needs none of Bevy's asset
//! plumbing to answer "what does this level's geometry say".

use std::time::Instant;

use avian3d::prelude::*;
use bevy::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::level::LevelData;
use bft_assets::materials::Soil;
use bft_sim::level::surface_info;
use bft_sim::physics::{GroundProbe, SurfaceQuery, is_liquid, layers, probe_ground};
use bft_sim::water::{LiquidKind, find_water};

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

    if !paths.cache.path.join("gamez").is_dir() {
        eprintln!(
            "no extraction cache at {}; run `open-bft extract`. Skipping.",
            paths.cache.path.display()
        );
        return None;
    }

    Some(paths.cache.path)
}

/// Parse one level's `gamez` archive.
fn level(cache: &std::path::Path, name: &str) -> Option<LevelData> {
    let path = cache.join(format!("gamez/{name}-gamez.gamez.zip"));
    let bytes = std::fs::read(&path).ok()?;

    match bft_assets::level::parse_gamez(&bytes, name) {
        Ok(level) => Some(level),
        Err(e) => panic!("{name} failed to parse: {e}"),
    }
}

/// Every level, for the sweeps.
const LEVELS: [&str; 13] = [
    "m1", "m2", "m3", "m4", "m5", "m6", "m7", "m8", "m9", "m10", "m11", "m12", "m13",
];

fn axes() -> AxisConvention {
    AxisConvention::GAMEZ_DEFAULT
}

/// docs/data/water.md's polygon-soil table, re-measured through the engine's
/// path: triangulation, collider merge, `material_per_triangle`, soil.
///
/// The point is not to re-verify the data — docs/data/water.md does that — but to
/// prove the soil survives the whole pipeline. A triangle order bug would
/// show up here as water on the wrong surfaces, and nowhere else until a
/// vehicle drove through a hillside expecting to swim.
#[test]
fn liquid_soil_survives_the_triangulation_in_every_level() {
    let Some(cache) = cache() else { return };
    let axes = axes();

    // docs/data/water.md names the levels that have water and lava. m10 has neither; m9 is
    // the only quicksand.
    let mut with_water = Vec::new();
    let mut with_lava = Vec::new();
    let mut with_quicksand = Vec::new();

    for name in LEVELS {
        let Some(level) = level(&cache, name) else {
            continue;
        };
        let mut counts = (0usize, 0usize, 0usize);

        for node in &level.nodes {
            let Some(model_index) = node.common().model_index else {
                continue;
            };
            let Some(model) = level.models.get(model_index) else {
                continue;
            };
            if model.polygons.is_empty() {
                continue;
            }

            let triangulated = bft_assets::geometry::triangulate(model, &axes);
            let Some(surface) = surface_info(model.model_type, &triangulated, &level.materials)
            else {
                continue;
            };

            for soil in &surface.soil_by_triangle {
                match soil {
                    Soil::Water => counts.0 += 1,
                    Soil::Lava => counts.1 += 1,
                    Soil::Quicksand => counts.2 += 1,
                    _ => {}
                }
            }
        }

        if counts.0 > 0 {
            with_water.push(name);
        }
        if counts.1 > 0 {
            with_lava.push(name);
        }
        if counts.2 > 0 {
            with_quicksand.push(name);
        }

        eprintln!(
            "{name}: {} water, {} lava, {} quicksand collider triangles",
            counts.0, counts.1, counts.2
        );
    }

    // docs/data/water.md's table, read through the engine's pipeline. Three levels have
    // no water at all: m3 and m8 are the lava levels (docs/data/water.md shows `—` for m3's
    // water column), and m10 has neither.
    assert_eq!(
        with_water,
        vec![
            "m1", "m2", "m4", "m5", "m6", "m7", "m9", "m11", "m12", "m13"
        ],
        "§8: every level but the two lava ones and m10 carries water soil"
    );
    assert_eq!(
        with_lava,
        vec!["m2", "m3", "m4", "m6", "m8", "m12"],
        "§8's lava levels"
    );
    assert_eq!(
        with_quicksand,
        vec!["m9"],
        "§8: quicksand exists only in m9"
    );
}

/// Strategy B on the real levels: every level with liquid soil produces
/// volumes, m1's deep pools are divable, and m10 produces none.
#[test]
fn strategy_b_finds_water_volumes_in_the_real_levels() {
    let Some(cache) = cache() else { return };
    let axes = axes();

    for name in LEVELS {
        let Some(level) = level(&cache, name) else {
            continue;
        };
        let found = find_water(&level, &axes);
        let divable = found
            .regions
            .iter()
            .filter(|region| region.volume.is_divable())
            .count();
        let kinds: Vec<LiquidKind> = {
            let mut kinds: Vec<LiquidKind> = found.regions.iter().map(|r| r.volume.kind).collect();
            kinds.sort_by_key(|kind| format!("{kind:?}"));
            kinds.dedup();
            kinds
        };

        eprintln!(
            "{name}: {} volume(s), {divable} divable, kinds {kinds:?}, \
             {} name warning(s)",
            found.regions.len(),
            found.name_mismatches.len()
        );

        if name == "m10" {
            assert!(
                found.regions.is_empty(),
                "§8 says m10 has no water, but strategy B found {} volumes",
                found.regions.len()
            );
            continue;
        }

        assert!(
            !found.regions.is_empty(),
            "{name} has liquid soil (§8) but produced no volumes"
        );

        // The name check is signal, not noise: with docs/data/water.md's floors excluded,
        // only m4's `water_level*` and m6's `a2lava` are genuinely named for
        // liquid and carry none. Any other level warning means the naming
        // convention or this reading has shifted.
        let expected_warnings = match name {
            "m4" => 2,
            "m6" => 1,
            _ => 0,
        };
        assert_eq!(
            found.name_mismatches.len(),
            expected_warnings,
            "{name}: unexpected water name warnings {:?}",
            found.name_mismatches
        );

        // Every volume's surface must be somewhere the level actually is,
        // and every divable one must have real depth.
        for region in &found.regions {
            assert!(
                region.volume.surface_y.is_finite(),
                "{name}: a volume with no finite surface"
            );
            assert!(
                !region.volume.footprint.is_empty(),
                "{name}: a volume with no footprint"
            );

            if let Some(floor) = region.volume.floor_y {
                assert!(
                    region.volume.surface_y - floor >= bft_sim::water::SHALLOW_DEPTH,
                    "{name}: a 'deep' volume only {} m deep",
                    region.volume.surface_y - floor
                );
            }
        }
    }
}

/// docs/data/water.md's m1 deep-water pattern, end to end: the `waterup*` surface is a
/// `Soil::Water` polygon with **no** collision flags over an `underwater*`
/// floor that has them, and strategy B turns the pair into a divable volume.
#[test]
fn m1s_deep_pools_come_out_divable_and_its_open_sea_does_not() {
    let Some(cache) = cache() else { return };
    let Some(level) = level(&cache, "m1") else {
        return;
    };

    // The flag pattern docs/data/water.md describes, read off the nodes rather than assumed.
    let flagged = |prefix: &str| -> Vec<(String, bool, bool)> {
        level
            .nodes
            .iter()
            .filter(|node| node.common().name.starts_with(prefix))
            .map(|node| {
                let common = node.common();
                (
                    common.name.clone(),
                    common.flags.altitude_surface,
                    common.flags.intersect_surface,
                )
            })
            .collect()
    };

    let waterup = flagged("waterup");
    let underwater = flagged("underwater");

    assert!(!waterup.is_empty(), "m1 should have waterup nodes");
    assert!(!underwater.is_empty(), "m1 should have underwater nodes");

    for (name, altitude, intersect) in &waterup {
        assert!(
            !altitude && !intersect,
            "§8: {name} is the visual surface and carries no collision flags"
        );
    }
    for (name, altitude, intersect) in &underwater {
        assert!(
            *altitude && *intersect,
            "§8: {name} is the sea floor and carries both flags"
        );
    }

    let found = find_water(&level, &axes());
    let divable = found
        .regions
        .iter()
        .filter(|region| region.volume.is_divable())
        .count();
    let shallow = found.regions.len() - divable;

    eprintln!("m1: {divable} divable volume(s), {shallow} shallow");

    assert!(
        divable > 0,
        "m1's zone-14 pools should be divable: the sub has to fit between \
         waterup and underwater"
    );
    assert!(
        shallow > 0,
        "m1's open sea rides on the polygon itself and has no floor below"
    );
}

/// Does `intersect_surface` without
/// `altitude_surface` mean "wall, not driveable"?
///
/// The answer the engine implements is yes — a wall collides but is not a
/// riding surface — and this measures how much geometry that describes, so
/// the reading is on record rather than assumed.
#[test]
fn walls_are_intersect_only_and_are_a_real_class_of_geometry() {
    let Some(cache) = cache() else { return };

    for name in ["m1", "m6", "m13"] {
        let Some(level) = level(&cache, name) else {
            continue;
        };

        let mut both = 0usize;
        let mut walls = 0usize;
        let mut ride_only = 0usize;
        let mut neither = 0usize;

        for node in &level.nodes {
            let flags = &node.common().flags;

            match (flags.altitude_surface, flags.intersect_surface) {
                (true, true) => both += 1,
                (false, true) => walls += 1,
                (true, false) => ride_only += 1,
                (false, false) => neither += 1,
            }
        }

        eprintln!(
            "{name}: {both} ride+collide, {walls} collide-only (walls), \
             {ride_only} ride-only, {neither} decorative"
        );

        assert!(
            walls > 0,
            "{name}: `intersect_surface` without `altitude_surface` should be \
             a real class of node, or the distinction is meaningless"
        );
        assert!(
            both > walls,
            "{name}: most collidable geometry should be rideable terrain"
        );
    }
}

/// Acceptance: the physics set on the largest level, with no dynamic bodies,
/// and the trimesh build time the open question asks about.
///
/// The numbers are printed for the PR rather than asserted tightly: a CI
/// runner is not the reference machine. The assertion is the loose ceiling
/// that would catch an accidental O(n²).
#[test]
fn the_biggest_level_builds_and_steps_inside_the_budget() {
    let Some(cache) = cache() else { return };
    // m6 is the biggest: 4955 nodes, 1927 colliders (docs/data/nodes.md).
    let Some(level) = level(&cache, "m6") else {
        return;
    };
    let axes = axes();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bft_sim::SimPlugin);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / bft_sim::schedule::SIM_HZ),
    ));
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.insert_resource(axes);
    app.finish();
    app.update();

    let spawn_start = Instant::now();
    app.world_mut()
        .run_system_cached_with(
            |In(level): In<LevelData>, mut commands: Commands, axes: Res<AxisConvention>| {
                bft_sim::level::spawn_level_into(&mut commands, &level, &axes);
            },
            level,
        )
        .expect("the spawn runs");
    app.update();
    let spawn_time = spawn_start.elapsed();

    let colliders = app
        .world_mut()
        .query_filtered::<Entity, With<Collider>>()
        .iter(app.world())
        .count();
    let volumes = app
        .world_mut()
        .query::<&bft_sim::water::WaterVolume>()
        .iter(app.world())
        .count();

    // Five warm-up ticks so the broad phase has built its tree, then measure.
    for _ in 0..5 {
        app.update();
    }

    let tick_start = Instant::now();
    const TICKS: u32 = 60;
    for _ in 0..TICKS {
        app.update();
    }
    let per_tick = tick_start.elapsed() / TICKS;

    eprintln!(
        "m6: {colliders} colliders + {volumes} water volumes built in {:.2?}; \
         {:.3} ms per tick with no dynamic bodies",
        spawn_time,
        per_tick.as_secs_f64() * 1000.0
    );

    assert!(
        colliders > 1900,
        "m6 should build about 1927 colliders plus its water sensors, got {colliders}"
    );
    // The open question's threshold: "if trimesh build time exceeds ~1 s,
    // consider caching". A generous ceiling here catches a regression an
    // order of magnitude past that without failing on a slow CI box.
    assert!(
        spawn_time.as_secs_f64() < 30.0,
        "spawning m6 took {spawn_time:.2?}, which is far past the budget"
    );
    // Acceptance says under 2 ms on the reference machine; 20 ms is the
    // "something is quadratic" line for any machine.
    assert!(
        per_tick.as_secs_f64() < 0.020,
        "a tick took {:.3} ms with no dynamic bodies",
        per_tick.as_secs_f64() * 1000.0
    );
}

/// A body dropped into a real level lands on the terrain.
#[test]
fn a_dropped_body_lands_on_m1s_terrain_at_the_player_start() {
    let Some(cache) = cache() else { return };
    let Some(level) = level(&cache, "m1") else {
        return;
    };
    let axes = axes();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bft_sim::SimPlugin);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / bft_sim::schedule::SIM_HZ),
    ));
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.insert_resource(axes);
    app.finish();
    app.update();

    app.world_mut()
        .run_system_cached_with(
            |In(level): In<LevelData>, mut commands: Commands, axes: Res<AxisConvention>| {
                bft_sim::level::spawn_level_into(&mut commands, &level, &axes);
            },
            level,
        )
        .expect("the spawn runs");
    app.update();

    // m1's player start, from `aiv.zrd` (docs/data/nodes.md): on the pier by
    // the hangar, at y = 3.9. Dropped from 30 m above it.
    let start = Vec3::new(2222.1, 3.9, 1294.2);

    // The probe first: there has to be ground under the start, or the level
    // has not been spawned the way the vehicle will expect.
    let hit = app
        .world_mut()
        .run_system_cached_with(
            |In(from): In<Vec3>, spatial: SpatialQuery, surfaces: SurfaceQuery| {
                probe_ground(&spatial, &surfaces, GroundProbe::down(from, 100.0))
            },
            start + Vec3::Y * 30.0,
        )
        .expect("the system runs")
        .expect("there should be ground under m1's player start");

    eprintln!(
        "m1 start: ground at y = {:.2}, soil {:?}, normal {:?}",
        hit.point.y, hit.soil, hit.normal
    );

    assert!(
        (hit.point.y - start.y).abs() < 10.0,
        "the ground under the start should be near the start's own height, \
         found {:.2} against {:.2}",
        hit.point.y,
        start.y
    );
    assert!(
        !is_liquid(&hit.soil),
        "§6's note says the reader start is on the pier, not over water, \
         but the soil reads {:?}",
        hit.soil
    );

    let body = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(1.0),
            layers::vehicle(),
            Transform::from_translation(start + Vec3::Y * 30.0),
        ))
        .id();

    // 30 m at 40 m/s² is 1.2 s; three seconds of ticks covers the fall and
    // the settle on a trimesh.
    for _ in 0..(bft_sim::schedule::SIM_HZ as u32 * 3) {
        app.update();
    }

    let resting = app
        .world()
        .get::<Transform>(body)
        .expect("a transform")
        .translation;

    eprintln!("dropped body came to rest at {resting:?}");

    assert!(
        resting.y < start.y + 30.0,
        "the body never fell: still at {:.2}",
        resting.y
    );
    assert!(
        (resting.y - hit.point.y).abs() < 5.0,
        "the body should rest on the ground the probe found ({:.2}), not at {:.2}",
        hit.point.y,
        resting.y
    );
}
