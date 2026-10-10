//! Craters against the real data: every mission's `declient.zrd`, and m1's
//! own terrain taking a mortar shell.
//!
//! They skip (not fail) when `BFT_DATA_DIR` is unset or the cache has not
//! been built. No game data enters the repository.

use avian3d::prelude::*;
use bevy::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::level::{LevelData, parse_gamez};
use bft_sim::crater::{CraterConfig, CraterFormed, Cratered, GroundSource};
use bft_sim::level::{GameLayer, NodeIndex, NodeName};
use bft_sim::schedule::SIM_HZ;
use bft_sim::weapons::{
    DEFAULT_TETHER_MAX_ALTITUDE, ProjectileHit, Surface, WeaponOverrides, WeaponTable,
    build_weapon_table,
};

fn cache() -> Option<std::path::PathBuf> {
    let root = bft_assets::test_support::data_root()?;
    match bft_assets::DataPaths::discover(Some(&root), None, None) {
        Ok(paths) if paths.cache.path.join("reader").is_dir() => Some(paths.cache.path),
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

fn reader(cache: &std::path::Path, file: &str) -> bft_assets::ReaderData {
    let bytes = std::fs::read(cache.join("reader").join(file)).expect("the archive is readable");
    bft_assets::reader::parse_reader_data(&bytes).expect("a real reader archive parses")
}

/// Every mission has a `CRATER` block, and every one names a default
/// texture and anim; m3 and m6 send lava to their own dirt.
#[test]
fn every_mission_configures_its_craters() {
    let Some(cache) = cache() else { return };

    for mission in 1..=13 {
        let config =
            CraterConfig::from_reader(&reader(&cache, &format!("m{mission}-zrdr.rdr.zip")))
                .unwrap_or_else(|| panic!("m{mission} has a CRATER block"));

        assert!(config.default_texture.is_some(), "m{mission}");
        assert!(config.default_anim.is_some(), "m{mission}");
        assert_eq!(config.max_radius, 25.0, "m{mission}");
        assert!((config.max_depth - 1.7).abs() < 1e-6, "m{mission}");
        assert_eq!(config.slope_deg, 15.0, "m{mission}");
    }

    let m3 = CraterConfig::from_reader(&reader(&cache, "m3-zrdr.rdr.zip")).expect("m3");
    assert_eq!(
        m3.look_for(Some("lavhot1")),
        (Some("lavhot1".into()), Some("lav_dirt.flt".into()))
    );
    let m1 = CraterConfig::from_reader(&reader(&cache, "m1-zrdr.rdr.zip")).expect("m1");
    assert_eq!(
        m1.look_for(Some("mnsroad3")).0.as_deref(),
        Some("craterroad")
    );
    assert_eq!(
        m1.look_for(Some("anything")).0.as_deref(),
        Some("cratersand")
    );
}

/// m1, headless, with its colliders and crater settings.
fn m1(cache: &std::path::Path) -> (App, bft_assets::ReaderData) {
    let weapons: WeaponTable = build_weapon_table(
        &reader(cache, "zrdr.rdr.zip"),
        &WeaponOverrides::default(),
        DEFAULT_TETHER_MAX_ALTITUDE,
    )
    .expect("the weapon table builds");
    let mission = reader(cache, "m1-zrdr.rdr.zip");
    let bytes = std::fs::read(cache.join("gamez/m1-gamez.gamez.zip")).expect("readable");
    let level: LevelData = parse_gamez(&bytes, "gamez.zip").expect("a real gamez parses");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bft_sim::SimPlugin);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / SIM_HZ),
    ));
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.insert_resource(AxisConvention::GAMEZ_DEFAULT);
    app.insert_resource(weapons);
    app.insert_resource(CraterConfig::from_reader(&mission).expect("m1 has one"));
    app.finish();
    app.update();

    app.world_mut()
        .run_system_cached_with(
            |In(level): In<LevelData>, mut commands: Commands, axes: Res<AxisConvention>| {
                bft_sim::level::spawn_level_into(&mut commands, &level, &axes);
            },
            level,
        )
        .expect("the level spawns");
    for _ in 0..5 {
        app.update();
    }
    (app, mission)
}

/// The ground straight down from `above`.
fn ground(app: &mut App, above: Vec3) -> Option<(Vec3, Vec3, Entity)> {
    app.world_mut()
        .run_system_cached_with(
            |In(above): In<Vec3>, spatial: SpatialQuery| {
                let filter = SpatialQueryFilter::from_mask(GameLayer::Level);
                spatial
                    .cast_ray(above, Dir3::NEG_Y, 500.0, true, &filter)
                    .map(|hit| (above + Vec3::NEG_Y * hit.distance, hit.normal, hit.entity))
            },
            above,
        )
        .expect("the query runs")
}

/// Hit the ground at `at` with the HE mortar, and return what formed.
fn shell(app: &mut App, at: Vec3, normal: Vec3, node: Entity) -> Vec<CraterFormed> {
    let hem = app
        .world()
        .resource::<WeaponTable>()
        .index("wep_2.0")
        .expect("the HE mortar");
    app.world_mut().write_message(ProjectileHit {
        projectile: Entity::PLACEHOLDER,
        at,
        normal,
        hit: Some(node),
        hit_node: None,
        surface: Surface::Default,
        weapon: hem,
    });
    app.update();
    let formed = app
        .world_mut()
        .resource_mut::<Messages<CraterFormed>>()
        .drain()
        .collect();
    // The cut colliders into the spatial index.
    app.update();
    formed
}

/// Level ground around m1's start, on a 60 m grid: where a shell would land,
/// its normal, the node, and whether that node is deformable ground.
fn level_ground(app: &mut App, start: Vec3) -> Vec<(Vec3, Vec3, Entity, bool)> {
    let mut found = Vec::new();
    for x in -5..=5 {
        for z in -5..=5 {
            let probe = start + Vec3::new(x as f32 * 60.0, 150.0, z as f32 * 60.0);
            let Some((at, normal, node)) = ground(app, probe) else {
                continue;
            };
            // Only ground a shell could land on flat: steep slopes refusing
            // is a different rule from the one these tests are about.
            if normal.y < 0.9 {
                continue;
            }
            let deformable = app.world().get::<GroundSource>(node).is_some();
            found.push((at, normal, node, deformable));
        }
    }
    found
}

/// A shell on m1's sand digs a hole a vehicle can sit in: the ground under
/// its centre is `DEPTH` lower than it was.
#[test]
fn a_shell_on_m1s_sand_digs_a_crater() {
    let Some(cache) = cache() else { return };
    let (mut app, mission) = m1(&cache);
    let start = mission.player_start().expect("m1 has a start").position;

    let mut dug = None;
    for (at, normal, node, deformable) in level_ground(&mut app, start) {
        if !deformable {
            continue;
        }
        if let Some(formed) = shell(&mut app, at, normal, node).into_iter().next() {
            dug = Some((at, formed));
            break;
        }
    }
    let (at, formed) = dug.expect("m1's deformable ground takes a crater");

    assert!(!formed.nodes.is_empty());
    assert_eq!(formed.crater.radius, 10.0, "HEM's 10 m");
    assert_eq!(formed.crater.sides, 6, "a hexagon");
    assert!(formed.crater.texture.is_some(), "m1 textures every crater");
    for node in &formed.nodes {
        assert!(app.world().get::<Cratered>(*node).is_some());
        assert!(
            app.world().get::<GroundSource>(*node).is_some(),
            "only deformable ground is cut"
        );
    }

    let (bottom, ..) = ground(&mut app, at + Vec3::Y * 20.0).expect("still ground under it");
    assert!(
        (at.y - bottom.y - formed.crater.depth).abs() < 0.05,
        "the floor is {} m down, wanted {}",
        at.y - bottom.y,
        formed.crater.depth
    );
    // And the rim is where the ground was.
    let beyond = at + Vec3::new(formed.crater.radius + 2.0, 20.0, 0.0);
    let (outside, ..) = ground(&mut app, beyond).expect("ground beyond the rim");
    assert!(
        (outside.y - at.y).abs() < 3.0,
        "past the rim the ground is where it was: {} vs {}",
        outside.y,
        at.y
    );
}

/// Sand, dirt and dirt roads crater; the docks and everything else that is
/// not `can_modify` never does. Printed so a change in the rule shows up in
/// the numbers.
#[test]
fn only_m1s_deformable_ground_takes_craters() {
    let Some(cache) = cache() else { return };
    let (mut app, mission) = m1(&cache);
    let start = mission.player_start().expect("m1 has a start").position;

    let (mut soft, mut soft_dug, mut hard, mut hard_dug) = (0, 0, 0, 0);
    for (at, normal, node, deformable) in level_ground(&mut app, start) {
        let dug = !shell(&mut app, at, normal, node).is_empty();
        if deformable {
            soft += 1;
            soft_dug += usize::from(dug);
        } else {
            hard += 1;
            hard_dug += usize::from(dug);
        }
    }

    eprintln!("m1: {soft_dug} of {soft} deformable and {hard_dug} of {hard} other level hits dug");
    assert!(
        soft >= 10 && hard >= 5,
        "the grid found both kinds of ground"
    );
    assert_eq!(hard_dug, 0, "concrete, metal and roofs never crater");
    assert!(
        soft_dug * 4 >= soft * 3,
        "most deformable level ground should crater: {soft_dug} of {soft}"
    );
}

/// `craters.ron`'s m1 entry: the pier road `g943`, made deformable, takes a
/// crater together with the sand `g32` it lies on. The two interleave by a
/// few centimetres (here the sand is the higher); both sink by the same
/// amount at every point, so whichever is on top stays on top.
#[test]
fn the_pier_road_and_the_sand_under_it_sink_together() {
    let Some(cache) = cache() else { return };
    let (mut app, _) = m1(&cache);
    let bytes = std::fs::read(cache.join("gamez/m1-gamez.gamez.zip")).expect("readable");
    let level: LevelData = parse_gamez(&bytes, "gamez.zip").expect("parses");

    // What `bft-game` does for a `craters.ron` name.
    let road = app.world().resource::<NodeIndex>().by_name("g943").to_vec();
    assert_eq!(road.len(), 1, "one pier road");
    let model = app
        .world()
        .get::<bft_sim::level::ModelRef>(road[0])
        .expect("it draws a model")
        .0;
    let surfaces = std::sync::Arc::new(bft_sim::crater::LevelSurfaces::from_materials(
        &level.materials,
    ));
    app.world_mut()
        .entity_mut(road[0])
        .insert(GroundSource::for_model(
            &level.models[model],
            &AxisConvention::GAMEZ_DEFAULT,
            surfaces,
        ));
    app.update();

    // Where the road runs over the sand.
    let (at, normal, hit) = ground(&mut app, Vec3::new(2220.0, 50.0, 1500.0)).expect("ground");
    assert!(
        road[0] == hit
            || app
                .world()
                .get::<NodeName>(hit)
                .is_some_and(|n| n.0 == "g32")
    );
    let formed = shell(&mut app, at, normal, hit);
    assert_eq!(formed.len(), 1, "the road craters");

    let names: Vec<&str> = formed[0]
        .nodes
        .iter()
        .map(|node| {
            app.world()
                .get::<NodeName>(*node)
                .expect("named")
                .0
                .as_str()
        })
        .collect();
    assert!(
        names.contains(&"g943") && names.contains(&"g32"),
        "{names:?}"
    );

    // Straight down through the centre: the top layer is `DEPTH` lower.
    let (road_floor, ..) = ground(&mut app, at + Vec3::Y * 20.0).expect("road");
    assert!((at.y - road_floor.y - formed[0].crater.depth).abs() < 0.05);
}
