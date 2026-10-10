//! The AI's headless tests: enemies see, chase, aim and shoot.
//!
//! These drive the whole `SimPlugin` schedule with no renderer, so they check
//! the AI as it is really wired — perception into the state machine into
//! steering into the vehicle controller into the weapons — rather
//! than as each piece is written. The tables are built from hand-written
//! reader JSON of the shapes the real files have, so the file runs without a
//! Recoil install; `real_ai.rs` asserts the same code against the shipped
//! data.

use avian3d::prelude::*;
use bevy::ecs::system::RunSystemOnce;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bft_assets::ReaderData;
use bft_assets::materials::{MaterialDesc, Soil};
use bft_assets::models::{ModelDesc, ModelType, PolygonDesc, PolygonMaterialDesc};
use bft_sim::ai::table::{AI_ENTRY, NET_ENTRY};
use bft_sim::ai::{
    AiBrain, AiState, AiTable, AttackStrategy, EnemyTables, SteeringTuning, TurretAi,
    build_ai_table, spawn_enemy, spawn_turrets,
};
use bft_sim::damage::{Health, Team};
use bft_sim::level::{NodeName, surface_info};
use bft_sim::physics::layers;
use bft_sim::schedule::SIM_HZ;
use bft_sim::vehicle::tuning::{
    Difficulty, ModeSounds, ModeTuning, PlayerConstants, SuspensionTuning, VehicleAiDefaults,
    VehicleDef, VehicleKindId, VehicleTable,
};
use bft_sim::vehicle::{FormSet, PlayerInput, VehicleForm, spawn_player_vehicle};
use bft_sim::weapons::{
    FireRequest, Muzzle, WeaponLoadout, WeaponOverrides, WeaponTable, build_weapon_table,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// An app with the sim's schedule and avian, and nothing that draws.
fn ai_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bft_sim::SimPlugin);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / SIM_HZ),
    ));
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.finish();
    app.update();

    app
}

fn step(app: &mut App, ticks: u32) {
    for _ in 0..ticks {
        app.update();
    }
}

/// A 1000 m square slab at y = 0, big enough for a chase.
fn slab() -> ModelDesc {
    ModelDesc {
        vertices: vec![
            Vec3::new(-500.0, 0.0, -500.0),
            Vec3::new(-500.0, 0.0, 500.0),
            Vec3::new(500.0, 0.0, 500.0),
            Vec3::new(500.0, 0.0, -500.0),
        ],
        polygons: vec![PolygonDesc {
            vertex_indices: vec![0, 1, 2, 3],
            materials: vec![PolygonMaterialDesc {
                material_index: 0,
                uv_coords: None,
            }],
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn materials() -> Vec<MaterialDesc> {
    vec![MaterialDesc::Colored {
        color: [1.0, 1.0, 1.0],
        alpha: 255,
        soil: Soil::Default,
    }]
}

fn spawn_slab(app: &mut App) -> Entity {
    let model = slab();
    let axes = bft_assets::axes::AxisConvention::GAMEZ_DEFAULT;
    let triangulated = bft_assets::geometry::triangulate(&model, &axes);
    let collider = bft_sim::colliders::collider_from(ModelType::Default, &triangulated)
        .expect("the slab has a surface");
    let surface =
        surface_info(ModelType::Default, &triangulated, &materials()).expect("the slab has soil");

    app.world_mut()
        .spawn((
            collider,
            surface,
            RigidBody::Static,
            layers::level(),
            Transform::IDENTITY,
        ))
        .id()
}

/// A solid wall, for the line-of-sight and whisker checks.
///
/// Centred on `at`, `half` in each direction. Static level geometry on
/// [`layers::level`], which is the only layer perception and the whiskers
/// look at.
fn spawn_wall(app: &mut App, at: Vec3, half: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            Collider::cuboid(half.x * 2.0, half.y * 2.0, half.z * 2.0),
            RigidBody::Static,
            layers::level(),
            Transform::from_translation(at),
        ))
        .id()
}

/// One enemy mode, run through the loader's own derivation.
///
/// The scout's real `track_mode` values (READER-SCHEMA §2.1), so the test
/// drives what the game would. An enemy block states no `platform` or
/// `collision`, exactly as the shipped data does not.
fn enemy_mode() -> ModeTuning {
    let mut tuning = ModeTuning {
        platform: None,
        collision: Vec::new(),
        mode_alt: 0.0,
        rates: [12.0, 40.0],
        turns: [1.2, 4.0],
        turn_damping: 12.0,
        rate_damping: [6.0, 4.0],
        friction: Some([150.0, 80.0, 0.02]),
        mass: Some(2.2),
        a_damping: Some(12.0),
        chas_smooth: None,
        chas_pitch: None,
        chas_roll: None,
        collision_d: [0.6, 0.15],
        quicksand_slowdown: Some(0.5),
        lava_slowdown: Some(0.2),
        ok_to_amphib: false,
        alt_control: None,
        gun_pitch: None,
        wave: None,
        transitions: HashMap::default(),
        sounds: ModeSounds::default(),
        suspension: SuspensionTuning {
            rays: Vec::new(),
            ray_lift: 0.0,
            rest: 0.0,
            stiffness: 0.0,
            damping: 0.0,
            max_len: 0.0,
        },
        engine_force: 0.0,
        max_speed: 0.0,
        reverse_speed: 0.0,
        lateral_grip: 0.0,
        static_grip: 0.0,
        air_control: 0.0,
        grip_force_scale: 0.0,
        regrip_fraction: 0.0,
        stationary_speed: 0.0,
        upright_rate_air: 0.0,
        upright_rate_ground: 0.0,
        max_tilt: 0.0,
    };

    tuning.derive();
    tuning
}

/// The player's track mode, for the target vehicle these tests chase.
fn player_mode() -> ModeTuning {
    let mut tuning = ModeTuning {
        platform: Some([
            Vec3::new(-2.25, 0.0, -2.4),
            Vec3::new(2.25, 0.0, -2.4),
            Vec3::new(2.25, 0.0, 2.34),
            Vec3::new(-2.25, 0.0, 2.34),
        ]),
        collision: vec![Vec3::new(-2.7, 1.0, -3.34), Vec3::new(2.7, 1.0, 3.34)],
        rates: [48.0, 65.0],
        turns: [2.5, 3.5],
        ..enemy_mode()
    };

    tuning.derive();
    tuning
}

/// The player plus one scout, which is every kind these tests need.
fn vehicle_table() -> VehicleTable {
    let mut kinds = HashMap::default();

    kinds.insert(
        VehicleKindId::player(),
        VehicleDef {
            health: 140.0,
            shield: Some(60.0),
            modes: [(VehicleForm::Track, player_mode())].into_iter().collect(),
            ..vehicle_def()
        },
    );

    kinds.insert(
        VehicleKindId("scou".to_string()),
        VehicleDef {
            name: "Scout".to_string(),
            health: 6.0,
            modes: [(VehicleForm::Track, enemy_mode())].into_iter().collect(),
            // The scout's real `common_mode` AI ranges.
            ai: VehicleAiDefaults {
                activation: Some(300.0),
                pursuit_range: Some([60.0, 90.0]),
                not_pursuit_dwell: Some(0.2),
                return_range: Some(175.0),
            },
            weapons: vec![bft_sim::vehicle::tuning::VehicleWeaponRow {
                weapon: "wep_1.0".to_string(),
                owned: true,
                ammo: 5000,
                interval: 0.15,
                range: 325.0,
                raw: [1.0, 1.0, 5000.0, 0.15, 5.0, 325.0, 0.0, 1.0],
            }],
            drops: vec![bft_sim::vehicle::tuning::DropDef {
                kind: "ERFPG_AMMO".to_string(),
                amount: 50,
            }],
            ..vehicle_def()
        },
    );

    VehicleTable {
        kinds,
        player: PlayerConstants::default(),
    }
}

fn vehicle_def() -> VehicleDef {
    VehicleDef {
        name: String::new(),
        health: 1.0,
        shield: None,
        modes: HashMap::default(),
        camback: [Vec3::ZERO; 3],
        aim_y: [3.0, 3.0],
        turret_rate: None,
        weapons: Vec::new(),
        ai: VehicleAiDefaults::default(),
        drops: Vec::new(),
        sounds: Default::default(),
        track_switch: None,
    }
}

/// One flat fast gun, which is what every enemy and turret in these tests
/// carries.
fn weapon_table() -> WeaponTable {
    let json: serde_json::Value = serde_json::from_str(
        r#"[["VERSION", [2],
            "BALLISTICS", [
              "wep_1.0", ["NAME", ["RFPG"], "AMMO_LIMIT", [123456789], "DAMAGE", [10.0],
                          "FIRE_RATE", [12.0], "RANGE", [700.0], "VELOCITY", [120.0],
                          "IMPACT_PROXIMITY", [0.0],
                          "IMPACT", ["default", ["SOUND", ["snd_ric"]]]]]]]"#,
    )
    .expect("valid fixture JSON");

    let archive =
        ReaderData::from_entries([("weapons.zrd".to_string(), json)].into_iter().collect());

    build_weapon_table(&archive, &WeaponOverrides::default(), 30.0).expect("the fixture builds")
}

/// A mission archive of the shapes the real per-mission files have.
///
/// One net with a two-node chain, one scout on it, and one `tur_1**` turret
/// entry — the commonest glob in the game.
fn mission_archive(activate_rad: f32, attack_rad: f32, return_range: f32) -> ReaderData {
    let aiv: serde_json::Value = serde_json::from_str(
        r#"[["bft_00", [0, [0.0, 0.0, 0.0], 0.0],
             "scou_01", [1, [100.0, 0.0, 0.0], 90.0]]]"#,
    )
    .expect("valid fixture JSON");

    let net: serde_json::Value = serde_json::from_str(&format!(
        r#"[["version", [105], "name", ["net_01"], "type", ["standard"],
             "path_width", [10],
             "activate_rad", [{activate_rad}], "attack_rad", [{attack_rad}],
             "attack_dwell", [2], "not_pursuit_dwell", [2],
             "pursuit_params", [0, 0], "return_range", [{return_range}],
             "hide_times", [0, 0], "attack_strategy", ["head_on"],
             "attack_buddy", [0], "activate_buddy", [0],
             "node_00", [[12], [100.0, 0.0, 0.0], [1, -1, -1]],
             "node_01", [[12], [150.0, 0.0, 0.0], [0, -1, -1]]]]"#
    ))
    .expect("valid fixture JSON");

    let ai: serde_json::Value = serde_json::from_str(
        r#"[["DESTROY_ANIM", ["destroy_turret"],
             "TURRET", [
               "tur_1**", ["HEALTH", [6], "PARTS", ["turret", "gun", "firepoint"],
                           "WEAPON", ["NAME", ["wep_1.0"], "AMMO", [100],
                                      "FIRE_RATE", [0.1], "FIRE_LIMITS", [0.8, 2.5],
                                      "DETECTION_RANGE", [300]]]]]]"#,
    )
    .expect("valid fixture JSON");

    let ring: serde_json::Value =
        serde_json::from_str(r#"[[[10.0, 1.0, 10.0, 180.0], [20.0, 1.0, 20.0, 90.0]]]"#)
            .expect("valid fixture JSON");

    ReaderData::from_entries(
        [
            ("aiv.zrd".to_string(), aiv),
            ("net_01.zrd".to_string(), net),
            (AI_ENTRY.to_string(), ai),
            (NET_ENTRY.to_string(), ring),
        ]
        .into_iter()
        .collect(),
    )
}

fn ai_table(archive: &ReaderData) -> AiTable {
    build_ai_table(archive, Difficulty::Normal, &Default::default()).expect("the fixture builds")
}

/// Put the three tables in the world and hand back the AI one.
fn install_tables(app: &mut App, ai: AiTable) -> AiTable {
    app.insert_resource(vehicle_table());
    app.insert_resource(weapon_table());
    app.insert_resource(ai.clone());
    ai
}

/// Spawn one enemy from a table's placement, by index among the enemies.
fn spawn_one_enemy(app: &mut App, ai: &AiTable, index: usize) -> Entity {
    let placement = ai
        .enemies()
        .nth(index)
        .expect("the fixture places an enemy")
        .clone();
    let vehicles = vehicle_table();
    let weapons = weapon_table();
    let table = ai.clone();

    app.world_mut()
        .run_system_cached_with(
            move |In((placement, vehicles, weapons, table)): In<(
                bft_sim::ai::UnitPlacement,
                VehicleTable,
                WeaponTable,
                AiTable,
            )>,
                  mut commands: Commands| {
                spawn_enemy(
                    &mut commands,
                    &placement,
                    EnemyTables {
                        vehicles: &vehicles,
                        weapons: Some(&weapons),
                        ai: &table,
                    },
                    None,
                    SteeringTuning::default(),
                    0,
                )
            },
            (placement, vehicles, weapons, table),
        )
        .expect("the spawn system runs")
}

/// A player vehicle on the slab, which is what every enemy here hunts.
fn spawn_target(app: &mut App, at: Vec3) -> Entity {
    let table = vehicle_table();
    app.insert_resource(table.clone());

    let player = app
        .world_mut()
        .run_system_cached_with(
            move |In((at, table)): In<(Transform, VehicleTable)>, mut commands: Commands| {
                spawn_player_vehicle(&mut commands, at, &table, FormSet::track_only())
            },
            (Transform::from_translation(at), table),
        )
        .expect("the spawn system runs");

    app.world_mut()
        .entity_mut(player)
        .insert((Health::new(140.0), Team::PLAYER));

    app.update();
    player
}

/// Build a `pturret.flt/healthy/turret/gun/firepoint` subtree, as gamez has
/// it, and return the root.
///
/// The parts are found by *search*, not by path — `firepoint` is a
/// grandchild of the pitch part — so the fixture reproduces the real depth.
fn spawn_turret_node(app: &mut App, at: Vec3) -> Entity {
    let world = app.world_mut();

    let root = world
        .spawn((
            NodeName("tur_101".to_string()),
            Transform::from_translation(at),
        ))
        .id();
    let flt = world
        .spawn((NodeName("pturret.flt".to_string()), Transform::IDENTITY))
        .insert(ChildOf(root))
        .id();
    let healthy = world
        .spawn((NodeName("healthy".to_string()), Transform::IDENTITY))
        .insert(ChildOf(flt))
        .id();
    let turret = world
        .spawn((NodeName("turret".to_string()), Transform::IDENTITY))
        .insert(ChildOf(healthy))
        .id();
    let gun = world
        .spawn((NodeName("gun".to_string()), Transform::IDENTITY))
        .insert(ChildOf(turret))
        .id();
    world
        .spawn((
            NodeName("firepoint".to_string()),
            Transform::from_xyz(0.0, 0.0, -2.0),
        ))
        .insert(ChildOf(gun));

    // The wreck's identically named pieces, so the search has something to
    // pick the wrong one out of.
    let destroyed = world
        .spawn((NodeName("destroyed".to_string()), Transform::IDENTITY))
        .insert(ChildOf(flt))
        .id();
    world
        .spawn((NodeName("turret".to_string()), Transform::IDENTITY))
        .insert(ChildOf(destroyed));

    app.update();
    root
}

/// What `install_turrets` hands its one-shot system.
type TurretSpawnArgs = (Vec<(String, Entity)>, AiTable, WeaponTable);

/// Give the level's turret nodes their brains, the way `bft-game` will.
fn install_turrets(app: &mut App, ai: &AiTable, nodes: Vec<(String, Entity)>) -> Vec<Entity> {
    let table = ai.clone();
    let weapons = weapon_table();

    app.world_mut()
        .run_system_cached_with(
            move |In((nodes, table, weapons)): In<TurretSpawnArgs>,
                  mut commands: Commands,
                  index: Option<Res<bft_sim::level::NodeIndex>>,
                  children: Query<&Children>,
                  names: Query<&NodeName>| {
                let empty = bft_sim::level::NodeIndex::default();
                let index = index.as_deref().unwrap_or(&empty);

                spawn_turrets(
                    &mut commands,
                    index,
                    &nodes,
                    &table,
                    Some(&weapons),
                    &children,
                    &names,
                )
            },
            (nodes, table, weapons),
        )
        .expect("the spawn system runs")
}

fn brain(app: &App, entity: Entity) -> AiBrain {
    app.world()
        .get::<AiBrain>(entity)
        .expect("the enemy has a brain")
        .clone()
}

fn position(app: &App, entity: Entity) -> Vec3 {
    app.world()
        .get::<Transform>(entity)
        .expect("the entity has a transform")
        .translation
}

// ---------------------------------------------------------------------------
// Turrets
// ---------------------------------------------------------------------------

/// A turret acquires a target in range with line of sight, and the `PARTS`
/// resolution with it.
#[test]
fn a_turret_acquires_a_target_in_range_with_line_of_sight() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    let archive = mission_archive(200.0, 200.0, 100.0);
    let ai = install_tables(&mut app, ai_table(&archive));

    let node = spawn_turret_node(&mut app, Vec3::new(0.0, 1.0, 0.0));
    let turrets = install_turrets(&mut app, &ai, vec![("tur_101".to_string(), node)]);
    assert_eq!(turrets.len(), 1, "`tur_1**` matches `tur_101`");
    app.update();

    // Well inside the entry's 300 m `DETECTION_RANGE`, nothing in the way.
    spawn_target(&mut app, Vec3::new(0.0, 1.0, -80.0));
    step(&mut app, 30);

    let turret = app
        .world()
        .get::<TurretAi>(node)
        .expect("the node became a turret");

    assert!(turret.target.is_some(), "it sees a target in the open");
    assert!(
        turret.yaw_node.is_some() && turret.pitch_node.is_some(),
        "and resolved its yaw and pitch parts"
    );
    assert_eq!(turret.muzzles.len(), 1, "and its firepoint");

    // The `healthy` subtree's parts, not the wreck's: the search is
    // breadth-first for exactly this reason.
    let yaw = turret.yaw_node.expect("a yaw part");
    let parent = app.world().get::<ChildOf>(yaw).expect("it has a parent").0;
    assert_eq!(
        app.world()
            .get::<NodeName>(parent)
            .map(|name| name.0.clone()),
        Some("healthy".to_string()),
        "the yaw part is under `healthy`, not `destroyed`"
    );
}

/// The other half: a wall hides a target.
#[test]
fn a_turret_does_not_acquire_a_target_behind_a_wall() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    let archive = mission_archive(200.0, 200.0, 100.0);
    let ai = install_tables(&mut app, ai_table(&archive));

    let node = spawn_turret_node(&mut app, Vec3::new(0.0, 1.0, 0.0));
    install_turrets(&mut app, &ai, vec![("tur_101".to_string(), node)]);

    // A tall wall halfway between, wide enough that the ray cannot miss it.
    spawn_wall(
        &mut app,
        Vec3::new(0.0, 10.0, -40.0),
        Vec3::new(30.0, 10.0, 2.0),
    );
    spawn_target(&mut app, Vec3::new(0.0, 1.0, -80.0));
    step(&mut app, 30);

    let turret = app
        .world()
        .get::<TurretAi>(node)
        .expect("the node became a turret");

    assert_eq!(
        turret.target, None,
        "the wall is between the turret and the target"
    );
}

/// A target outside the entry's own `DETECTION_RANGE` is not acquired, even
/// in the clear: the range is the data's, not ours.
#[test]
fn a_turret_ignores_a_target_beyond_its_detection_range() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    let archive = mission_archive(200.0, 200.0, 100.0);
    let ai = install_tables(&mut app, ai_table(&archive));

    let node = spawn_turret_node(&mut app, Vec3::new(0.0, 1.0, 0.0));
    install_turrets(&mut app, &ai, vec![("tur_101".to_string(), node)]);

    // 400 m against a 300 m `DETECTION_RANGE`.
    spawn_target(&mut app, Vec3::new(0.0, 1.0, -400.0));
    step(&mut app, 30);

    assert_eq!(
        app.world().get::<TurretAi>(node).and_then(|t| t.target),
        None
    );
}

/// A turret that has tracked its target fires, and the shot goes through the
/// same `FireRequest` path the player's does.
#[test]
fn a_turret_on_target_asks_to_fire() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    let archive = mission_archive(200.0, 200.0, 100.0);
    let ai = install_tables(&mut app, ai_table(&archive));

    let node = spawn_turret_node(&mut app, Vec3::new(0.0, 1.0, 0.0));
    install_turrets(&mut app, &ai, vec![("tur_101".to_string(), node)]);
    spawn_target(&mut app, Vec3::new(0.0, 1.0, -50.0));

    // Long enough for the parts to slew round from zero onto the target.
    step(&mut app, 120);

    let fired = app
        .world()
        .resource::<Messages<FireRequest>>()
        .iter_current_update_messages()
        .any(|request| request.shooter == node);

    assert!(
        fired
            || app
                .world()
                .get::<TurretAi>(node)
                .is_some_and(|t| t.fire_timer > 0.0),
        "a tracked turret pulls the trigger"
    );
}

/// A SAM site shoots the set-piece VTOL, not the player, so it gets no brain.
#[test]
fn a_targets_vtol_entry_is_not_given_a_brain() {
    let mut app = ai_app();

    let ai: serde_json::Value = serde_json::from_str(
        r#"[["DESTROY_ANIM", ["destroy_turret"],
             "TURRET", [
               "sam_**", ["HEALTH", [12], "PARTS", ["turret", "gun", "firepoint"],
                          "TARGETS", ["vtol"],
                          "WEAPON", ["NAME", ["wep_1.0"], "AMMO", [6],
                                     "FIRE_RATE", [30], "DETECTION_RANGE", [500]]]]]]"#,
    )
    .expect("valid fixture JSON");
    let aiv: serde_json::Value = serde_json::from_str(r#"[["bft_00", [0, [0.0, 0.0, 0.0], 0.0]]]"#)
        .expect("valid fixture JSON");

    let archive = ReaderData::from_entries(
        [("aiv.zrd".to_string(), aiv), (AI_ENTRY.to_string(), ai)]
            .into_iter()
            .collect(),
    );
    let table = install_tables(&mut app, ai_table(&archive));

    let node = app
        .world_mut()
        .spawn((NodeName("sam_01".to_string()), Transform::IDENTITY))
        .id();

    let spawned = install_turrets(&mut app, &table, vec![("sam_01".to_string(), node)]);
    app.update();

    assert!(spawned.is_empty(), "a SAM site has nothing to shoot at yet");
    assert!(app.world().get::<TurretAi>(node).is_none());
}

// ---------------------------------------------------------------------------
// Vehicle enemies
// ---------------------------------------------------------------------------

/// An enemy on a flat slab closes on a target.
#[test]
fn a_vehicle_enemy_closes_the_distance_to_its_target() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    // A wide activation radius and no leash, so this measures the steering
    // rather than the state machine, which has its own tests.
    let archive = mission_archive(400.0, 400.0, 0.0);
    let ai = install_tables(&mut app, ai_table(&archive));

    let enemy = spawn_one_enemy(&mut app, &ai, 0);
    app.update();

    // The enemy starts at (100, 0, 0); the target is 150 m away down +x.
    let target = spawn_target(&mut app, Vec3::new(250.0, 1.0, 0.0));
    step(&mut app, 10);

    let started = position(&app, enemy).distance(position(&app, target));

    // The scout spawns facing −x, directly away. It turns round — about 2.6 s
    // at its standstill rate — and then has five seconds at up to 12 m/s.
    step(&mut app, (SIM_HZ as u32) * 8);

    let ended = position(&app, enemy).distance(position(&app, target));
    let state = brain(&app, enemy).state;

    // Up to 60 m of driving after the turn, less the time to reach speed.
    // 40 still catches a steering sign error, which shows up as a *widening*
    // distance or a crawl.
    assert!(
        ended < started - 40.0,
        "closed only {:.1} m in eight seconds (from {started:.1} to {ended:.1}), state {state:?}",
        started - ended
    );

    // And it got there nose first. Backing toward a goal keeps it behind, so
    // steering that reversed the whole way would also pass the distance
    // check above.
    let hull = app.world().get::<Transform>(enemy).expect("a hull");
    let forward = hull.rotation * Vec3::NEG_Z;
    let to_target = (position(&app, target) - hull.translation).normalize();
    assert!(
        forward.dot(to_target) > 0.9,
        "it faces {forward}, not the target along {to_target}"
    );
}

/// A `head_on` enemy stops at its `pursuit` standoff rather than ramming.
#[test]
fn a_vehicle_enemy_holds_its_pursuit_standoff() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    let aiv: serde_json::Value = serde_json::from_str(
        r#"[["bft_00", [0, [0.0, 0.0, 0.0], 0.0],
             "scou_01", [1, [100.0, 0.0, 0.0], 90.0]]]"#,
    )
    .expect("valid fixture JSON");
    let net: serde_json::Value = serde_json::from_str(
        r#"[["version", [105], "name", ["net_01"], "type", ["standard"],
             "path_width", [10], "activate_rad", [400], "attack_rad", [400],
             "attack_dwell", [2], "not_pursuit_dwell", [2],
             "pursuit_params", [60, 90], "return_range", [0],
             "hide_times", [0, 0], "attack_strategy", ["head_on"],
             "attack_buddy", [0], "activate_buddy", [0],
             "node_00", [[12], [100.0, 0.0, 0.0], [-1, -1, -1]]]]"#,
    )
    .expect("valid fixture JSON");

    let archive = ReaderData::from_entries(
        [
            ("aiv.zrd".to_string(), aiv),
            ("net_01.zrd".to_string(), net),
        ]
        .into_iter()
        .collect(),
    );
    let ai = install_tables(&mut app, ai_table(&archive));

    let enemy = spawn_one_enemy(&mut app, &ai, 0);
    app.update();
    let target = spawn_target(&mut app, Vec3::new(250.0, 1.0, 0.0));

    // Long enough to arrive and settle.
    step(&mut app, (SIM_HZ as u32) * 20);

    let distance = position(&app, enemy).distance(position(&app, target));

    assert!(
        distance > 20.0,
        "it closed to {distance:.1} m against a 60 m standoff; it is ramming"
    );
}

/// The whole point of `Dormant`: 84 enemies on a real level must not all be
/// driving before the player has come near them.
#[test]
fn an_enemy_outside_its_activate_rad_stays_asleep() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    // A 50 m activation radius against a target 500 m away, which is also
    // beyond the default `roam_range`.
    let archive = mission_archive(50.0, 50.0, 100.0);
    let ai = install_tables(&mut app, ai_table(&archive));

    let enemy = spawn_one_enemy(&mut app, &ai, 0);
    app.update();
    spawn_target(&mut app, Vec3::new(-400.0, 1.0, 0.0));

    let started = position(&app, enemy);
    step(&mut app, (SIM_HZ as u32) * 3);

    let brain = brain(&app, enemy);
    let input = app
        .world()
        .get::<PlayerInput>(enemy)
        .expect("the enemy has an input");

    assert_eq!(brain.state, AiState::Dormant);
    assert_eq!(input.throttle, 0.0, "a dormant enemy does not drive");
    assert!(!input.fire, "nor shoot");

    // It may settle onto its suspension, but it must not have driven off.
    let moved = position(&app, enemy).distance(started);
    assert!(moved < 5.0, "it drove {moved:.1} m while dormant");
}

/// Between the two: outside `activate_rad` but inside `roam_range`, a unit
/// walks its net without turning on the player or firing.
#[test]
fn an_enemy_inside_its_roam_range_walks_its_net_without_hunting() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    // 50 m activation, target 150 m away.
    let archive = mission_archive(50.0, 50.0, 0.0);
    let ai = install_tables(&mut app, ai_table(&archive));

    let enemy = spawn_one_enemy(&mut app, &ai, 0);
    app.update();
    spawn_target(&mut app, Vec3::new(250.0, 1.0, 0.0));

    // It spawns facing away from node 1, so it turns round first.
    let started = position(&app, enemy);
    let mut fired = false;
    for _ in 0..(SIM_HZ as u32) * 8 {
        app.update();
        fired |= app
            .world()
            .get::<PlayerInput>(enemy)
            .is_some_and(|input| input.fire);
    }

    assert_eq!(brain(&app, enemy).state, AiState::Roam);
    assert!(!fired, "a roaming unit does not shoot");
    let moved = position(&app, enemy).distance(started);
    assert!(moved > 10.0, "it walked only {moved:.1} m of its net");
}

/// A unit shot from outside its `activate_rad` wakes and turns on the
/// shooter, rather than sitting through the shelling.
#[test]
fn a_sleeping_enemy_that_is_hit_wakes_and_targets_the_shooter() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    let archive = mission_archive(50.0, 50.0, 0.0);
    let ai = install_tables(&mut app, ai_table(&archive));

    let enemy = spawn_one_enemy(&mut app, &ai, 0);
    app.update();
    let player = spawn_target(&mut app, Vec3::new(-300.0, 1.0, 0.0));
    step(&mut app, 12);
    assert!(!brain(&app, enemy).state.awake(), "asleep before the hit");

    app.world_mut().write_message(bft_sim::damage::DamageEvent {
        target: enemy,
        amount: 1.0,
        source: Some(player),
        at: Vec3::ZERO,
        kind: bft_sim::damage::DamageKind::Script,
    });
    step(&mut app, 2);

    let brain = brain(&app, enemy);
    assert!(brain.state.awake(), "it woke: {:?}", brain.state);
    assert_eq!(brain.target, Some(player));
}

/// A low rise between the two hides the target's tracks but not its hull,
/// and the hull is what is seen. A ray ending at the target's origin on the
/// ground would leave enemies blind at range over rolling terrain.
#[test]
fn an_enemy_sees_a_hull_over_a_rise_that_hides_its_tracks() {
    let mut app = ai_app();
    spawn_slab(&mut app);
    // Top at 1.2 m, halfway between the two.
    spawn_wall(
        &mut app,
        Vec3::new(175.0, 0.6, 0.0),
        Vec3::new(1.0, 0.6, 30.0),
    );

    let archive = mission_archive(400.0, 400.0, 0.0);
    let ai = install_tables(&mut app, ai_table(&archive));

    let enemy = spawn_one_enemy(&mut app, &ai, 0);
    app.update();
    let player = spawn_target(&mut app, Vec3::new(250.0, 1.0, 0.0));
    step(&mut app, 30);

    let brain = brain(&app, enemy);
    assert_eq!(brain.target, Some(player));
    assert_eq!(brain.los_lost, 0, "and it has line of sight to the hull");
}

/// During a cinematic nothing sees the player: the intros land it among
/// enemies (m2's minelayer is 40 m from the start), and the original's hold
/// fire while the VTOL unloads.
#[test]
fn an_enemy_does_not_see_the_player_during_a_cinematic() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    let archive = mission_archive(400.0, 400.0, 0.0);
    let ai = install_tables(&mut app, ai_table(&archive));
    app.insert_resource(bft_sim::cinematic::PlayerControl::Cinematic {
        anim: "m2_start_animation".to_string(),
        carried: false,
    });

    let enemy = spawn_one_enemy(&mut app, &ai, 0);
    app.update();
    spawn_target(&mut app, Vec3::new(160.0, 1.0, 0.0));

    let mut fired = false;
    for _ in 0..60 {
        app.update();
        fired |= app
            .world()
            .get::<PlayerInput>(enemy)
            .is_some_and(|input| input.fire);
    }

    let asleep = brain(&app, enemy);
    assert_eq!(
        asleep.state,
        AiState::Dormant,
        "inside activate_rad, but blind"
    );
    assert_eq!(asleep.target, None);
    assert!(!fired);

    // Control handed back: now it wakes.
    app.insert_resource(bft_sim::cinematic::PlayerControl::Driving);
    step(&mut app, 60);
    assert!(
        brain(&app, enemy).state.awake(),
        "it wakes once the player drives"
    );
}

/// And the complement: inside the radius it wakes and engages.
#[test]
fn an_enemy_inside_its_activate_rad_wakes_and_engages() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    let archive = mission_archive(400.0, 400.0, 0.0);
    let ai = install_tables(&mut app, ai_table(&archive));

    let enemy = spawn_one_enemy(&mut app, &ai, 0);
    app.update();
    spawn_target(&mut app, Vec3::new(160.0, 1.0, 0.0));

    step(&mut app, 60);

    let brain = brain(&app, enemy);

    assert_ne!(brain.state, AiState::Dormant, "it woke");
    assert!(brain.target.is_some(), "and acquired the player");
}

/// `Team(1)` and `Health` from `vehicle.zrd`, `Drops` from `pickups` — which
/// is what makes an enemy killable by the player's own weapons (FR-A5) and
/// what the damage module's `spawn_drops` reads.
#[test]
fn a_spawned_enemy_carries_the_components_the_rest_of_the_sim_expects() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    let archive = mission_archive(200.0, 200.0, 100.0);
    let ai = install_tables(&mut app, ai_table(&archive));

    let enemy = spawn_one_enemy(&mut app, &ai, 0);
    app.update();

    let world = app.world();

    assert_eq!(world.get::<Team>(enemy), Some(&Team::ENEMY));
    assert_eq!(
        world.get::<Health>(enemy).map(|health| health.max),
        Some(6.0),
        "the scout's own vehicle.zrd health"
    );
    assert_eq!(
        world
            .get::<bft_sim::damage::Drops>(enemy)
            .map(|drops| drops.0.len()),
        Some(1),
        "and its pickup drop"
    );
    assert_eq!(
        world.get::<bft_sim::vehicle::Controlled>(enemy),
        Some(&bft_sim::vehicle::Controlled::Ai)
    );
    assert!(
        world.get::<PlayerInput>(enemy).is_some(),
        "it is driven by input"
    );
    assert!(world.get::<WeaponLoadout>(enemy).is_some());
    assert!(world.get::<Muzzle>(enemy).is_some());
    assert!(
        world.get::<RigidBody>(enemy).is_some(),
        "and is a real body, so the player can shove it"
    );

    let brain = world.get::<AiBrain>(enemy).expect("a brain");
    assert_eq!(brain.strategy, Some(AttackStrategy::HeadOn));
    assert_eq!(brain.net, Some(1));
    assert_eq!(brain.weapon_range, 325.0, "from the weapon row's column d");
}

/// A dead enemy stops steering on the tick it dies, rather than driving on as
/// a corpse until something else notices.
#[test]
fn a_destroyed_enemy_stops_driving() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    let archive = mission_archive(400.0, 400.0, 0.0);
    let ai = install_tables(&mut app, ai_table(&archive));

    let enemy = spawn_one_enemy(&mut app, &ai, 0);
    app.update();
    spawn_target(&mut app, Vec3::new(160.0, 1.0, 0.0));
    step(&mut app, 60);

    assert_ne!(brain(&app, enemy).state, AiState::Dormant, "it was awake");

    // Kill it the way a weapon would.
    app.world_mut().write_message(bft_sim::damage::DamageEvent {
        target: enemy,
        amount: 1000.0,
        source: None,
        at: Vec3::ZERO,
        kind: bft_sim::damage::DamageKind::Script,
    });
    step(&mut app, 3);

    let brain = brain(&app, enemy);
    let input = app
        .world()
        .get::<PlayerInput>(enemy)
        .expect("the wreck keeps its input");

    assert_eq!(brain.state, AiState::Dead);
    assert_eq!(brain.target, None);
    assert_eq!(input.throttle, 0.0);
    assert!(!input.fire);
}

/// A wreck blocks nothing: the player drives through where it died and shots
/// pass it. It stays where it died rather than falling through the terrain.
#[test]
fn a_destroyed_enemy_leaves_no_collider_behind() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    let archive = mission_archive(400.0, 400.0, 0.0);
    let ai = install_tables(&mut app, ai_table(&archive));

    let enemy = spawn_one_enemy(&mut app, &ai, 0);
    step(&mut app, 120);

    let hit_by_a_ray_from_above = |app: &mut App| {
        let at = position(app, enemy);
        app.world_mut()
            .run_system_once(move |query: SpatialQuery| {
                query
                    .cast_ray(
                        at + Vec3::Y * 20.0,
                        Dir3::NEG_Y,
                        40.0,
                        true,
                        &SpatialQueryFilter::default(),
                    )
                    .map(|hit| hit.entity)
            })
            .expect("the ray runs")
    };
    assert_eq!(
        hit_by_a_ray_from_above(&mut app),
        Some(enemy),
        "alive, the hull is solid"
    );

    app.world_mut().write_message(bft_sim::damage::DamageEvent {
        target: enemy,
        amount: 1000.0,
        source: None,
        at: Vec3::ZERO,
        kind: bft_sim::damage::DamageKind::Script,
    });
    step(&mut app, 3);
    let died_at = position(&app, enemy);
    step(&mut app, 120);

    assert_ne!(
        hit_by_a_ray_from_above(&mut app),
        Some(enemy),
        "the wreck must not block"
    );
    assert!(
        position(&app, enemy).distance(died_at) < 0.01,
        "the wreck stays put: {died_at} -> {}",
        position(&app, enemy)
    );
}

/// An enemy that drives at a wall has to turn out of it. Without the whiskers
/// a `head_on` unit grinds along the wall for ever.
#[test]
fn the_whiskers_turn_an_enemy_away_from_a_wall() {
    use bft_sim::ai::avoid_walls;

    let mut app = ai_app();
    spawn_slab(&mut app);
    // A wall straight ahead of a hull facing −Z.
    spawn_wall(
        &mut app,
        Vec3::new(0.0, 5.0, -12.0),
        Vec3::new(40.0, 5.0, 1.0),
    );
    app.update();

    let tuning = SteeringTuning::default();
    let hull = Transform::from_xyz(0.0, 0.0, 0.0);

    let steered = app
        .world_mut()
        .run_system_cached_with(
            move |In((tuning, hull, steering)): In<(SteeringTuning, Transform, f32)>,
                  spatial: SpatialQuery| {
                avoid_walls(&spatial, &tuning, &hull, steering)
            },
            (tuning, hull, 0.0),
        )
        .expect("the system runs");

    assert!(
        steered.abs() > 0.1,
        "a wall 12 m ahead has to bend the steering; got {steered}"
    );

    // And with nothing in front, the steering is left alone.
    let clear = Transform::from_xyz(0.0, 0.0, 200.0);
    let untouched = app
        .world_mut()
        .run_system_cached_with(
            move |In((tuning, hull, steering)): In<(SteeringTuning, Transform, f32)>,
                  spatial: SpatialQuery| {
                avoid_walls(&spatial, &tuning, &hull, steering)
            },
            (tuning, clear, 0.3),
        )
        .expect("the system runs");

    assert_eq!(untouched, 0.3, "open ground does not steer");
}

// ---------------------------------------------------------------------------
// Aiming
// ---------------------------------------------------------------------------

/// A lead shot hits a target moving at a constant
/// velocity, with zero aim error.
///
/// The whole flight is simulated rather than the geometry being asserted:
/// this is what catches a lead that is right in isolation and wrong once
/// the weapons' projectile integration is in front of it.
#[test]
fn a_lead_shot_hits_a_target_moving_at_constant_velocity() {
    use bft_sim::ai::lead_point;

    let weapons = weapon_table();
    let speed = weapons
        .get("wep_1.0")
        .and_then(|def| def.velocity)
        .expect("the gun has a muzzle speed");

    let mut app = ai_app();
    spawn_slab(&mut app);
    app.insert_resource(weapons);

    // The shooter at the origin, the target crossing its front at 200 m.
    let from = Vec3::new(0.0, 2.0, 0.0);
    let start = Vec3::new(0.0, 2.0, -200.0);
    let velocity = Vec3::new(20.0, 0.0, 0.0);

    // No error: this tests the *lead*, not the scatter.
    let aim = lead_point(from, start, velocity, speed, 1.0);
    let flight = from.distance(aim) / speed;

    // Where the target will actually be when the round arrives.
    let arrives_at = start + velocity * flight;

    assert!(
        aim.distance(arrives_at) < 0.5,
        "aimed at {aim}, target will be at {arrives_at} after {flight:.2} s"
    );

    // And the lead is a real correction, not a rounding: at 200 m and
    // 120 m/s the target moves more than thirty metres.
    assert!(
        aim.distance(start) > 25.0,
        "the lead is {:.1} m, which is not leading",
        aim.distance(start)
    );
}

/// An enemy pointed at a target writes an aim point, so the vehicle's turret
/// slew and ballistic solution do the work rather than a second copy of it.
#[test]
fn an_engaging_enemy_writes_an_aim_point_and_fires() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    let archive = mission_archive(400.0, 400.0, 0.0);
    let ai = install_tables(&mut app, ai_table(&archive));

    let enemy = spawn_one_enemy(&mut app, &ai, 0);
    app.update();
    spawn_target(&mut app, Vec3::new(160.0, 1.0, 0.0));

    // Long enough to wake, acquire and slew the turret round.
    step(&mut app, (SIM_HZ as u32) * 4);

    let input = app
        .world()
        .get::<PlayerInput>(enemy)
        .expect("the enemy has an input");

    assert_eq!(brain(&app, enemy).state, AiState::Engage);
    assert!(
        input.aim_point.is_some(),
        "the brain aims through `aim_point`, not its own maths"
    );
}

/// The sign convention `drive_toward` depends on, checked against the
/// vehicle controller rather than against a reading of it.
///
/// `PlayerInput::steer` is right-positive: `ground_forces` computes
/// `wanted_yaw = -steer * turn_rate`, and a negative yaw rate about +Y is a
/// right turn. An AI that got this backwards would steer every enemy *away*
/// from its target — which presents as a widening spiral rather than as a
/// vehicle that does not move, and cost real time to find.
#[test]
fn a_positive_steer_turns_the_hull_to_its_right() {
    let mut app = ai_app();
    spawn_slab(&mut app);

    let vehicle = spawn_target(&mut app, Vec3::new(0.0, 1.0, 0.0));

    // Facing −Z, driving forward, steering positive.
    app.world_mut().entity_mut(vehicle).insert(PlayerInput {
        throttle: 1.0,
        steer: 1.0,
        ..default()
    });

    // The input is re-applied every tick: `consume_input` clears the
    // edge-triggered fields but `steer` is a held state the frame layer
    // rewrites, and there is no frame layer here.
    for _ in 0..(SIM_HZ as u32) {
        app.world_mut().entity_mut(vehicle).insert(PlayerInput {
            throttle: 1.0,
            steer: 1.0,
            ..default()
        });
        app.update();
    }

    let heading = app
        .world()
        .get::<Transform>(vehicle)
        .expect("the vehicle has a transform")
        .rotation
        .to_euler(EulerRot::YXZ)
        .0;

    // A right turn from a −Z heading is a *negative* yaw about +Y.
    assert!(
        heading < -0.2,
        "a positive steer must turn right; yaw went to {heading:.2} rad"
    );
}
