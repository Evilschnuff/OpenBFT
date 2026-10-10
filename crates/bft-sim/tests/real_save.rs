//! Save and load against a real mission: what goes in comes back out.
//!
//! The check is a round trip through a second, freshly loaded session: play
//! m1 for a while — past the intro, shooting, with a pier knocked down and
//! the enemies awake — capture it, restore the file into a new load of the
//! same level, and capture *that*. The two captures must agree. Anything a
//! restore forgets to put back shows up as a difference, which is how this
//! test keeps the restore honest as the sim grows new state.
//!
//! They skip (not fail) when `BFT_DATA_DIR` is unset or the cache has not
//! been built. No game data enters the repository.

use bevy::prelude::*;
use bft_assets::ReaderData;
use bft_assets::anim::{AnimSet, parse_anim_set};
use bft_assets::axes::AxisConvention;
use bft_assets::level::{LevelData, parse_gamez};
use bft_sim::ai::templates::LevelTemplates;
use bft_sim::ai::{
    AiTable, Enemy, EnemyTables, EnemyTemplates, SteeringTuning, build_ai_table, spawn_enemy,
    spawn_turrets,
};
use bft_sim::anim::{AnimProgram, CallAnim, apply_initial_node_states, spawn_anim_instances};
use bft_sim::cinematic::{PlayerControl, SkipCinematic};
use bft_sim::damage::{AimResult, Health, Team};
use bft_sim::level::{LevelRoot, NodeActive, NodeIndex, NodeName};
use bft_sim::mission::{MissionState, build_mission_script};
use bft_sim::pickups::{build_pickup_table, pickup_placements, spawn_pickups};
use bft_sim::save::format::{SaveFile, SaveRef};
use bft_sim::save::{
    CaptureMeta, RestoreGate, SaveBaseline, capture, read_save, restore, save_to_ron,
};
use bft_sim::schedule::SIM_HZ;
use bft_sim::vehicle::tuning::{Difficulty, VehicleTable, build_vehicle_table};
use bft_sim::vehicle::{
    FormSet, PlayerInput, PlayerVehicle, VehicleOverrides, apply_player_constants,
    spawn_player_vehicle,
};
use bft_sim::weapons::{
    DEFAULT_TETHER_MAX_ALTITUDE, Projectile, WeaponOverrides, WeaponTable, build_weapon_table,
    player_loadout,
};

fn cache() -> Option<std::path::PathBuf> {
    let root = bft_assets::test_support::data_root()?;
    match bft_assets::DataPaths::discover(Some(&root), None, None) {
        Ok(paths) if paths.cache.path.join("anim").is_dir() => Some(paths.cache.path),
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

fn read(cache: &std::path::Path, relative: &str) -> Vec<u8> {
    std::fs::read(cache.join(relative)).expect("the cache is readable")
}

fn reader(cache: &std::path::Path, file: &str) -> ReaderData {
    bft_assets::reader::parse_reader_data(&read(cache, &format!("reader/{file}")))
        .expect("a real reader archive parses")
}

/// Where the player is put to wake the enemies: next to m1's nearest group
/// (the spot `play.rs`'s `BFT_START` documents).
const NEAR_ENEMIES: Vec3 = Vec3::new(2131.0, 8.0, 1618.0);

/// A campaign session of mission `level`, loaded the way `bft-game` loads
/// one: level, tables, enemies, turrets, pickups, player, VM, mission — then
/// the save baseline. With `gated`, a [`RestoreGate`] holds the sim.
fn session(cache: &std::path::Path, level: u16, gated: bool) -> App {
    let global = reader(cache, "zrdr.rdr.zip");
    let mission = reader(cache, &format!("m{level}-zrdr.rdr.zip"));
    let data: LevelData = parse_gamez(
        &read(cache, &format!("gamez/m{level}-gamez.gamez.zip")),
        "gamez.zip",
    )
    .expect("a real gamez parses");
    let set: AnimSet = parse_anim_set(&read(cache, &format!("anim/m{level}-anim.anim.zip")))
        .expect("a real anim archive parses");

    let difficulty = Difficulty::Normal;
    let vehicles =
        build_vehicle_table(&global, difficulty, &VehicleOverrides::default()).expect("vehicles");
    let weapons = build_weapon_table(
        &global,
        &WeaponOverrides::default(),
        DEFAULT_TETHER_MAX_ALTITUDE,
    )
    .expect("weapons");
    let pickups = build_pickup_table(&global).expect("pickups");
    let ai = build_ai_table(&mission, difficulty, &Default::default()).expect("ai");
    let script = build_mission_script(&mission).expect("a campaign mission has a script");
    let placements = pickup_placements(&mission, difficulty).expect("pickups place");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bft_sim::SimPlugin);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / SIM_HZ),
    ));
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.insert_resource(AxisConvention::GAMEZ_DEFAULT);
    app.insert_resource(difficulty);
    if gated {
        app.insert_resource(RestoreGate);
    }
    app.finish();
    app.update();

    app.world_mut()
        .run_system_cached_with(
            |In(level): In<LevelData>, mut commands: Commands, axes: Res<AxisConvention>| {
                bft_sim::level::spawn_level_into(&mut commands, &level, &axes);
            },
            data.clone(),
        )
        .expect("the level spawns");
    app.update();

    apply_player_constants(&mut app.world_mut().commands(), &vehicles.player);
    app.insert_resource(weapons);
    app.insert_resource(pickups);
    app.insert_resource(vehicles);

    // The enemies, their models and the turrets: `bft-game`'s
    // `populate_level`.
    app.world_mut()
        .run_system_cached_with(
            |In((ai, data)): In<(AiTable, LevelData)>,
             mut commands: Commands,
             vehicles: Res<VehicleTable>,
             weapons: Res<WeaponTable>,
             axes: Res<AxisConvention>,
             nodes: Res<NodeIndex>,
             all: Query<(Entity, &NodeName)>,
             children: Query<&Children>,
             names: Query<&NodeName>| {
                let templates = LevelTemplates::from_level(&data);
                let enemy_templates = EnemyTemplates {
                    level: &data,
                    axes: &axes,
                    templates: &templates,
                };
                for (phase, placement) in ai.enemies().enumerate() {
                    spawn_enemy(
                        &mut commands,
                        placement,
                        EnemyTables {
                            vehicles: &vehicles,
                            weapons: Some(&weapons),
                            ai: &ai,
                        },
                        Some(&enemy_templates),
                        SteeringTuning::default(),
                        phase as u32,
                    );
                }
                let matched: Vec<(String, Entity)> = all
                    .iter()
                    .filter(|(_, name)| ai.turret_for(&name.0).is_some())
                    .map(|(entity, name)| (name.0.clone(), entity))
                    .collect();
                spawn_turrets(
                    &mut commands,
                    &nodes,
                    &matched,
                    &ai,
                    Some(&weapons),
                    &children,
                    &names,
                );
                commands.insert_resource(templates);
                commands.insert_resource(ai);
            },
            (ai, data),
        )
        .expect("the enemies spawn");

    // The player, armed with everything so the test can fire a mortar: the
    // `--unlock-all` loadout.
    app.world_mut()
        .run_system_cached_with(
            |In(at): In<Vec3>,
             mut commands: Commands,
             vehicles: Res<VehicleTable>,
             weapons: Res<WeaponTable>| {
                let hull = spawn_player_vehicle(
                    &mut commands,
                    Transform::from_translation(at),
                    &vehicles,
                    FormSet::track_only(),
                );
                let def = vehicles.player_def().expect("a player row");
                commands.entity(hull).insert((
                    player_loadout(def, &weapons, &[], true),
                    Health::new(def.health),
                    Team::PLAYER,
                    AimResult::default(),
                ));
            },
            NEAR_ENEMIES,
        )
        .expect("the player spawns");

    app.world_mut()
        .run_system_cached_with(
            |In(placements): In<Vec<bft_sim::pickups::PickupPlacement>>, mut commands: Commands| {
                spawn_pickups(&mut commands, &placements);
            },
            placements,
        )
        .expect("the pickups spawn");
    app.update();

    // The VM and the mission: `bft-game`'s `setup_mission`.
    app.world_mut()
        .run_system_cached_with(
            |In(set): In<AnimSet>,
             mut commands: Commands,
             nodes: Res<NodeIndex>,
             children: Query<&Children>,
             names: Query<&NodeName>,
             templates: Query<(), With<bft_sim::ai::Template>>,
             roots: Query<Entity, With<LevelRoot>>| {
                let root = roots.iter().next().expect("a level root");
                let scopes = spawn_anim_instances(
                    &mut commands,
                    &set,
                    &nodes,
                    &children,
                    &names,
                    &templates,
                    root,
                );
                commands.insert_resource(scopes);
                commands.insert_resource(AnimProgram(set));
            },
            set.clone(),
        )
        .expect("the instances spawn");
    app.world_mut().flush();
    app.world_mut()
        .run_system_cached_with(
            |In(set): In<AnimSet>,
             scopes: Res<bft_sim::anim::AnimScopes>,
             nodes: Res<NodeIndex>,
             mut actives: Query<&mut NodeActive>| {
                apply_initial_node_states(&set, &scopes, &nodes, &mut actives);
            },
            set,
        )
        .expect("the initial states apply");
    let state = MissionState::new(bft_assets::catalog::LevelId(level), &script);
    app.insert_resource(state);
    app.insert_resource(script);
    app.world_mut().flush();

    SaveBaseline::record(app.world_mut());
    app
}

fn meta() -> CaptureMeta {
    CaptureMeta {
        label: "test".to_string(),
        level_name: "m1".to_string(),
        title_key: "MSG_CAMPAIGN1_NAME".to_string(),
        written_by: "open-bft test".to_string(),
        created: 1,
    }
}

fn tick(app: &mut App, ticks: usize) {
    for _ in 0..ticks {
        app.update();
    }
}

fn set_input(app: &mut App, input: PlayerInput) {
    let mut players = app
        .world_mut()
        .query_filtered::<&mut PlayerInput, With<PlayerVehicle>>();
    for mut current in players.iter_mut(app.world_mut()) {
        *current = input;
    }
}

/// A save with the parts that cannot match across two loaded games made equal:
/// the order copies were numbered in (entity allocation order differs), and
/// `started`, which a restore clears on purpose so the load entry animation
/// runs.
fn canonical(save: &SaveFile) -> String {
    let mut save = save.clone();
    if let Some(mission) = save.world.mission.as_mut() {
        mission.started = true;
    }
    let text = save_to_ron(&save).expect("serialises");
    // Copy numbers are the file's own and follow entity order; compare the
    // copies as a set.
    let text = regex_lite_replace(&text);
    let mut lines: Vec<&str> = text.lines().collect();
    lines.sort_unstable();
    lines.join("\n")
}

/// `Copy(7)` → `Copy(_)`, `CopyPart(7, 2)` → `CopyPart(_, 2)`.
fn regex_lite_replace(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("Copy") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let (prefix, after) = if let Some(after) = tail.strip_prefix("CopyPart(") {
            ("CopyPart(", after)
        } else if let Some(after) = tail.strip_prefix("Copy(") {
            ("Copy(", after)
        } else {
            out.push_str("Copy");
            rest = &tail[4..];
            continue;
        };
        let digits = after.chars().take_while(char::is_ascii_digit).count();
        out.push_str(prefix);
        out.push('_');
        rest = &after[digits..];
    }
    out.push_str(rest);
    out
}

#[test]
fn a_mission_saved_and_loaded_captures_the_same() {
    let Some(cache) = cache() else { return };

    // Session A: past the intro, then make things happen.
    let mut first = session(&cache, 1, false);
    tick(&mut first, 30);
    first.world_mut().write_message(SkipCinematic);
    tick(&mut first, 120);
    assert_eq!(
        *first.world().resource::<PlayerControl>(),
        PlayerControl::Driving,
        "the intro was skipped"
    );

    // Break a pier: a shared destruction script on an operand, with debris
    // flying (as `real_anim_events.rs`'s `m1s_pier_pillar_breaks_alone`).
    first
        .world_mut()
        .write_message(CallAnim::new("oldpiergone_01"));
    // Lob mortars: shells in flight, maybe craters.
    set_input(
        &mut first,
        PlayerInput {
            select_slot: Some(2),
            ..default()
        },
    );
    tick(&mut first, 2);
    set_input(
        &mut first,
        PlayerInput {
            fire: true,
            throttle: 0.6,
            aim_point: Some(NEAR_ENEMIES + Vec3::new(60.0, 0.0, -40.0)),
            ..default()
        },
    );
    tick(&mut first, 150);

    let world = first.world_mut();
    let projectiles = world.query::<&Projectile>().iter(world).count();
    let saved = capture(world, meta()).expect("driving in a campaign mission saves");
    eprintln!(
        "captured: {} node(s), {} enemies, {} turrets, {} pickups, {} projectiles ({} live), \
         {} instance(s), {} copies, {} motions, {} lights, {} crater node(s)",
        saved.nodes.len(),
        saved.enemies.len(),
        saved.turrets.len(),
        saved.pickups.len(),
        saved.projectiles.len(),
        projectiles,
        saved.anims.instances.len(),
        saved.anims.copies.len(),
        saved.anims.motions.len(),
        saved.anims.lights.len(),
        saved.craters.len(),
    );
    assert!(saved.enemies.len() > 50, "m1's enemies are in the save");
    assert!(!saved.anims.instances.is_empty(), "the VM ran");
    assert!(
        saved.nodes.iter().any(|node| node.active.is_some()),
        "the pier broke"
    );

    let text = save_to_ron(&saved).expect("serialises");
    eprintln!("save is {} KiB", text.len() / 1024);
    // `BFT_SAVE_OUT=/tmp/m1.ron` keeps the file, for `open-bft play --load`
    // or for reading.
    if let Some(out) = std::env::var_os("BFT_SAVE_OUT") {
        std::fs::write(&out, &text).expect("the save is writable");
    }
    let parsed = read_save(&text).expect("reads back");
    assert_eq!(parsed, saved, "the file round-trips");

    // Session B: a fresh load, held, restored.
    let mut second = session(&cache, 1, true);
    let report = restore(second.world_mut(), &parsed);
    assert!(report.fingerprint_matches);
    assert!(report.skipped.is_empty(), "skipped: {:#?}", report.skipped);
    assert!(second.world().get_resource::<RestoreGate>().is_none());
    second.world_mut().flush();

    let world = second.world_mut();
    let again = capture(world, meta()).expect("a restored mission saves");

    let (a, b) = (canonical(&saved), canonical(&again));
    if a != b {
        let only_a: Vec<&str> = a
            .lines()
            .filter(|line| !b.contains(line))
            .take(20)
            .collect();
        let only_b: Vec<&str> = b
            .lines()
            .filter(|line| !a.contains(line))
            .take(20)
            .collect();
        panic!(
            "the restored session differs:\n  only saved: {only_a:#?}\n  only restored: {only_b:#?}"
        );
    }

    // The first tick puts back what the VM's level-change reset would have
    // wiped: the debris still in the air.
    tick(&mut second, 1);
    assert!(
        second
            .world()
            .get_resource::<bft_sim::save::PendingAnimRestore>()
            .is_none()
    );
    if !saved.anims.motions.is_empty() {
        assert!(
            !second
                .world()
                .resource::<bft_sim::anim::AnimMotions>()
                .0
                .is_empty(),
            "the motions survived the first tick"
        );
    }

    // It plays on: the load entry animation hands over, nothing panics.
    tick(&mut second, 120);
    let world = second.world_mut();
    assert_eq!(
        *world.resource::<PlayerControl>(),
        PlayerControl::Driving,
        "load_control handed the player control"
    );
    assert!(world.resource::<MissionState>().started);
    assert_eq!(
        world.query::<&Enemy>().iter(world).count(),
        saved.enemies.len()
    );
}

/// Refs name what they say: a node record's id is a level node, a part
/// record carries its name.
#[test]
fn a_capture_names_nodes_and_parts_stably() {
    let Some(cache) = cache() else { return };

    let mut app = session(&cache, 1, false);
    tick(&mut app, 30);
    app.world_mut().write_message(SkipCinematic);
    tick(&mut app, 60);

    let saved = capture(app.world_mut(), meta()).expect("saves");
    for node in &saved.nodes {
        match &node.node {
            SaveRef::Node(_) => assert!(node.name.is_none()),
            SaveRef::EnemyPart(_, _) => assert!(node.name.is_some()),
            other => panic!("a node record names {other:?}"),
        }
    }
}
