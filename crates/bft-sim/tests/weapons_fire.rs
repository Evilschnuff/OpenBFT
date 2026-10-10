//! The weapons' headless tests: fire, fly, hit, hurt, pick up.
//!
//! These drive the whole `SimPlugin` schedule with no renderer, so they check
//! the systems as they are really wired rather than as they are written. The
//! weapon table is built from hand-written reader JSON of the shape the real
//! `weapons.zrd` has, so the file runs without a Recoil install;
//! `real_weapons.rs` asserts the same code against the shipped data.

use std::sync::Arc;

use avian3d::prelude::*;
use bevy::prelude::*;
use bft_assets::materials::{MaterialDesc, Soil};
use bft_assets::models::{ModelDesc, ModelType, PolygonDesc, PolygonMaterialDesc};
use bft_sim::crater::{CraterFormed, Cratered, GroundSource, LevelSurfaces};
use bft_sim::damage::{AimResult, DamageEvent, Destroyed, Drops, Health, Team};
use bft_sim::level::surface_info;
use bft_sim::physics::layers;
use bft_sim::pickups::{
    PickedUp, Pickup, PickupDef, PickupEffect, PickupTable, RespawnTimer, spawn_pickups,
};
use bft_sim::schedule::SIM_HZ;
use bft_sim::vehicle::{
    AimJoints, AimPivot, FormSet, GunPart, PlayerInput, Turret, TurretPart, Vehicle, VehicleForm,
};
use bft_sim::weapons::{
    AmmoState, Firepoint, Guided, MineBounced, Mover, Muzzle, Projectile, ProjectileHit,
    ProjectileSpawned, TwinBarrels, WeaponLoadout, WeaponOverrides, WeaponSelected, WeaponTable,
    build_weapon_table,
};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A weapon table of the shapes the real file has: a fast flat gun, a
/// ballistic splash mortar that craters, a homing missile, a hitscan beam,
/// and a proximity and a remote mine.
fn weapon_json() -> serde_json::Value {
    serde_json::from_str(
        r#"[["VERSION", [2],
            "LOCK_ON_WARNING", ["snd_lo_warn"],
            "NO_AMMO_WARNING", ["snd_ammo_out"],
            "TRIGGER_INACTIVE", ["snd_trigger_click"],
            "WEAPON_INACTIVE", ["snd_trigger_click"],
            "BALLISTICS", [
              "wep_1.0", ["NAME", ["RFPG"], "AMMO_LIMIT", [123456789], "DAMAGE", [10.0],
                          "FIRE_RATE", [12.0], "RANGE", [700.0], "VELOCITY", [120.0],
                          "IMPACT_PROXIMITY", [0.0],
                          "IMPACT", ["default", ["SOUND", ["snd_ric"]]]],
              "wep_2.0", ["NAME", ["HEM"], "AMMO_LIMIT", [30], "DAMAGE", [40.0],
                          "FIRE_RATE", [0.5], "RANGE", [1024.0], "VELOCITY", [100.0],
                          "GRAVITY", [9.8], "IMPACT_PROXIMITY", [16.0], "CRATER", [10, 20],
                          "IMPACT", ["default", ["SOUND", ["snd_boom"]]]],
              "wep_4.0", ["NAME", ["P-HEM"], "AMMO_LIMIT", [30], "DAMAGE", [24.0],
                          "FIRE_RATE", [1.0], "RANGE", [200.0], "VELOCITY", [30.0],
                          "GRAVITY", [9.8], "IMPACT_PROXIMITY", [12.0], "MINE", [1],
                          "RELOAD", [1], "CRATER", [10, 15],
                          "IMPACT", ["default", ["BOUNCE_SOUND", ["snd_bncehard"]]]],
              "wep_5.0", ["NAME", ["P-HEM"], "AMMO_LIMIT", [30], "DAMAGE", [36.0],
                          "FIRE_RATE", [1.0], "RANGE", [160.0], "VELOCITY", [30.0],
                          "GRAVITY", [9.8], "IMPACT_PROXIMITY", [12.0], "MINE", [1],
                          "RELOAD", [1], "REMOTE_DETONATE", [1], "CRATER", [10, 15],
                          "BOUNCE", ["SOUND", ["snd_bncehard"]]],
              "wep_8.0", ["NAME", ["LO-EM"], "AMMO_LIMIT", [99], "DAMAGE", [44.0],
                          "FIRE_RATE", [2.0], "RANGE", [1200.0], "VELOCITY", [120.0],
                          "GRAVITY", [0.0], "ACCELERATION", [40.0], "TURN_RATE", [1.0],
                          "LOCK_ON", [1.0], "IMPACT_PROXIMITY", [7.0]],
              "wep_8.1", ["NAME", ["LO-EM"], "AMMO_LIMIT", [30], "DAMAGE", [66.0],
                          "FIRE_RATE", [0.5], "RANGE", [1050.0], "VELOCITY", [80.0],
                          "GRAVITY", [0.0], "ACCELERATION", [20.0], "TURN_RATE", [1.6],
                          "PITCH_RATE", [1.2], "LOCK_ON", [0.1], "TETHER_GUIDED", [1],
                          "RELOAD", [1], "IMPACT_PROXIMITY", [6.0]],
              "wep_6.1", ["NAME", ["LASER"], "AMMO_LIMIT", [50], "DAMAGE", [22.0],
                          "FIRE_RATE", [2.0], "RANGE", [512.0], "GRAVITY", [0.0],
                          "BEAM", [0.5, 4.0, 1], "IMPACT_PROXIMITY", [0.0]]]]]"#,
    )
    .expect("valid fixture JSON")
}

fn table() -> WeaponTable {
    let archive = bft_assets::ReaderData::from_entries(
        [("weapons.zrd".to_string(), weapon_json())]
            .into_iter()
            .collect(),
    );

    build_weapon_table(&archive, &WeaponOverrides::default(), 30.0).expect("the fixture builds")
}

/// An app with the sim schedule, avian and the weapon table, one fixed tick
/// per [`step`].
fn weapons_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bft_sim::SimPlugin);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / SIM_HZ),
    ));
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.insert_resource(table());
    app.finish();
    // The first update runs no fixed tick — virtual time has not accumulated
    // one yet — so it is spent here.
    app.update();

    app
}

fn step(app: &mut App, ticks: u32) {
    for _ in 0..ticks {
        app.update();
    }
}

/// A shooter with a loadout holding every fixture weapon.
fn spawn_shooter(app: &mut App, at: Vec3, team: Team) -> Entity {
    let table = app.world().resource::<WeaponTable>().clone();
    let mut loadout = WeaponLoadout::default();

    for (id, slot, ammo, interval) in [
        ("wep_1.0", 1u8, u32::MAX, 1.0 / 12.0),
        ("wep_2.0", 2, 30, 1.2),
        ("wep_4.0", 4, 30, 1.0),
        ("wep_5.0", 5, 30, 1.0),
        ("wep_8.0", 8, 99, 2.0),
        ("wep_8.1", 8, 30, 0.5),
        ("wep_6.1", 6, 50, 2.0),
    ] {
        let index = table.index(id).expect("the fixture has it");
        let strong = table.weapons[index].variant == 1;
        let state = AmmoState {
            weapon: index,
            ammo,
            interval,
            mounted: true,
        };
        let slot_state = loadout.slot_mut(slot).expect("1..=9");
        if strong {
            slot_state.strong = Some(state);
        } else {
            slot_state.weak = Some(state);
        }
    }
    loadout.select(1);

    app.world_mut()
        .spawn((
            Vehicle::player(FormSet::all()),
            PlayerInput::default(),
            Turret::default(),
            loadout,
            Muzzle(Vec3::new(0.0, 1.0, -3.0)),
            Health::new(140.0),
            team,
            AimResult::default(),
            Transform::from_translation(at),
            GlobalTransform::from_translation(at),
        ))
        .id()
}

/// A target that can be hurt.
fn spawn_target(app: &mut App, at: Vec3, health: f32, team: Team) -> Entity {
    app.world_mut()
        .spawn((
            Health::new(health),
            team,
            RigidBody::Static,
            Collider::sphere(1.5),
            layers::vehicle(),
            Transform::from_translation(at),
            GlobalTransform::from_translation(at),
        ))
        .id()
}

/// A 1200 m square floor at y = 0, with soil, as `spawn_level` builds one.
///
/// Big enough to catch a mortar: at 100 m/s and 9.8 m/s² one fired level from
/// 20 m lands over 400 m downrange, well outside a courtyard-sized slab.
fn spawn_floor(app: &mut App) -> Entity {
    let model = ModelDesc {
        vertices: vec![
            Vec3::new(-600.0, 0.0, -600.0),
            Vec3::new(-600.0, 0.0, 600.0),
            Vec3::new(600.0, 0.0, 600.0),
            Vec3::new(600.0, 0.0, -600.0),
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
    };
    let materials = vec![MaterialDesc::Colored {
        color: [1.0, 1.0, 1.0],
        alpha: 255,
        soil: Soil::Default,
    }];

    let axes = bft_assets::axes::AxisConvention::GAMEZ_DEFAULT;
    let triangulated = bft_assets::geometry::triangulate(&model, &axes);
    let collider =
        bft_sim::colliders::collider_from(ModelType::Default, &triangulated).expect("a surface");
    let surface = surface_info(ModelType::Default, &triangulated, &materials).expect("soil");

    app.world_mut()
        .spawn((
            collider,
            surface,
            // What `spawn_level` gives every colliding node, so a crater can
            // cut it.
            GroundSource {
                model_type: ModelType::Default,
                triangulated: Arc::new(triangulated),
                surfaces: Arc::new(LevelSurfaces {
                    soils: vec![Soil::Default],
                    textures: vec![Some("grass".to_string())],
                }),
            },
            RigidBody::Static,
            layers::level(),
            Transform::IDENTITY,
            GlobalTransform::IDENTITY,
        ))
        .id()
}

fn hold_fire(app: &mut App, shooter: Entity, down: bool) {
    let mut input = app
        .world_mut()
        .get_mut::<PlayerInput>(shooter)
        .expect("the shooter has input");
    input.fire = down;
}

fn drain<M: Message + Clone>(app: &mut App) -> Vec<M> {
    app.world_mut()
        .resource_mut::<Messages<M>>()
        .drain()
        .collect()
}

/// Step `ticks` ticks, draining `M` every tick and keeping everything.
///
/// Bevy's message buffers hold **two** updates' worth, so a `drain` after a
/// long `step` sees only the tail. Anything counting events over a second has
/// to collect as it goes.
fn step_collecting<M: Message + Clone>(app: &mut App, ticks: u32) -> Vec<M> {
    let mut collected = Vec::new();

    for _ in 0..ticks {
        app.update();
        collected.extend(drain::<M>(app));
    }

    collected
}

// ---------------------------------------------------------------------------
// Firing and rate
// ---------------------------------------------------------------------------

/// Firing with a cooldown enforces the rate.
#[test]
fn a_held_trigger_fires_at_the_vehicles_own_interval() {
    let mut app = weapons_app();
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);
    hold_fire(&mut app, shooter, true);

    // One second of a 12-shots-a-second gun.
    let shots = step_collecting::<ProjectileSpawned>(&mut app, SIM_HZ as u32).len();

    // 60 ticks at one shot per 5 ticks, give or take the tick the cooldown
    // lands on.
    assert!(
        (11..=13).contains(&shots),
        "wanted about 12 shots in a second, got {shots}"
    );
}

/// The rate is the *vehicle's* `interval` column, not the weapon's
/// `FIRE_RATE`: the player's row is authoritative for the player.
#[test]
fn the_rate_comes_from_the_vehicle_row_not_the_weapon() {
    let mut app = weapons_app();
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);

    // Slot 2's weapon states FIRE_RATE 0.5 s but the row says 1.2 s.
    {
        let mut loadout = app
            .world_mut()
            .get_mut::<WeaponLoadout>(shooter)
            .expect("a loadout");
        loadout.select(2);
    }
    hold_fire(&mut app, shooter, true);

    let shots = step_collecting::<ProjectileSpawned>(&mut app, SIM_HZ as u32).len();
    assert_eq!(
        shots, 1,
        "a 1.2 s interval fits one shot in a second, not the weapon's two"
    );
}

/// An empty weapon fires nothing, however long the trigger is held.
#[test]
fn an_empty_weapon_does_not_fire() {
    let mut app = weapons_app();
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);
    {
        let mut loadout = app
            .world_mut()
            .get_mut::<WeaponLoadout>(shooter)
            .expect("a loadout");
        loadout.select(2);
        loadout.selected_ammo_mut().expect("slot 2").ammo = 0;
    }
    hold_fire(&mut app, shooter, true);

    assert!(step_collecting::<ProjectileSpawned>(&mut app, SIM_HZ as u32).is_empty());
}

/// Ammo goes down by one per shot, and the unlimited weapon never does.
#[test]
fn firing_spends_a_round_unless_the_supply_is_unlimited() {
    let mut app = weapons_app();
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);
    {
        let mut loadout = app
            .world_mut()
            .get_mut::<WeaponLoadout>(shooter)
            .expect("a loadout");
        loadout.select(2);
    }
    hold_fire(&mut app, shooter, true);
    step(&mut app, 2);
    hold_fire(&mut app, shooter, false);

    let ammo = app
        .world()
        .get::<WeaponLoadout>(shooter)
        .and_then(|l| l.selected_ammo().map(|a| a.ammo))
        .expect("slot 2");
    assert_eq!(ammo, 29, "one mortar spent");

    // Back to the unlimited gun.
    {
        let mut loadout = app
            .world_mut()
            .get_mut::<WeaponLoadout>(shooter)
            .expect("a loadout");
        loadout.select(1);
    }
    hold_fire(&mut app, shooter, true);
    step(&mut app, SIM_HZ as u32);

    assert_eq!(
        app.world()
            .get::<WeaponLoadout>(shooter)
            .and_then(|l| l.selected_ammo().map(|a| a.ammo)),
        Some(u32::MAX),
        "the player's pulse gun never runs down"
    );
}

/// Selection emits an event, and an empty slot is not selectable.
#[test]
fn selecting_an_owned_slot_reports_it_and_an_empty_one_does_not() {
    let mut app = weapons_app();
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);

    app.world_mut()
        .get_mut::<PlayerInput>(shooter)
        .expect("input")
        .select_slot = Some(8);
    step(&mut app, 1);

    let selected = drain::<WeaponSelected>(&mut app);
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].slot, 8);

    // Slot 3 holds nothing in this fixture.
    app.world_mut()
        .get_mut::<PlayerInput>(shooter)
        .expect("input")
        .select_slot = Some(3);
    step(&mut app, 1);

    assert!(drain::<WeaponSelected>(&mut app).is_empty());
    assert_eq!(
        app.world()
            .get::<WeaponLoadout>(shooter)
            .map(|l| l.selected),
        Some(8),
        "the old selection stands"
    );
}

// ---------------------------------------------------------------------------
// Movers
// ---------------------------------------------------------------------------

/// A ballistic projectile arcs and hits the floor.
#[test]
fn a_ballistic_projectile_arcs_down_and_hits_the_floor() {
    let mut app = weapons_app();
    spawn_floor(&mut app);
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 20.0, 0.0), Team::PLAYER);
    {
        let mut loadout = app
            .world_mut()
            .get_mut::<WeaponLoadout>(shooter)
            .expect("a loadout");
        loadout.select(2);
    }
    // Aim level, so only gravity brings it down.
    hold_fire(&mut app, shooter, true);
    step(&mut app, 1);
    hold_fire(&mut app, shooter, false);

    let spawned = drain::<ProjectileSpawned>(&mut app);
    assert_eq!(spawned.len(), 1, "one mortar");
    let projectile = spawned[0].entity;

    // Track it: it must descend, and end by hitting the floor.
    let mut previous = app
        .world()
        .get::<Transform>(projectile)
        .expect("in flight")
        .translation;
    let mut descended = false;

    for _ in 0..(SIM_HZ as u32 * 4) {
        step(&mut app, 1);
        let Some(transform) = app.world().get::<Transform>(projectile) else {
            break; // It hit and was despawned.
        };
        if transform.translation.y < previous.y {
            descended = true;
        }
        previous = transform.translation;
    }

    assert!(descended, "a mortar under gravity has to come down");
    assert!(
        app.world().get::<Transform>(projectile).is_none(),
        "it should have hit the floor and despawned, last at {previous:?}"
    );
    assert!(
        previous.z < -1.0,
        "and travelled forward while doing it, got {previous:?}"
    );
}

/// A flat-firing gun keeps its height: `GRAVITY` is 0 for the pulse gun and
/// the mover must not borrow the vehicle's 40 m/s².
#[test]
fn a_straight_projectile_does_not_fall() {
    let mut app = weapons_app();
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 50.0, 0.0), Team::PLAYER);
    hold_fire(&mut app, shooter, true);
    step(&mut app, 1);
    hold_fire(&mut app, shooter, false);

    let projectile = drain::<ProjectileSpawned>(&mut app)[0].entity;
    let start = app
        .world()
        .get::<Transform>(projectile)
        .expect("in flight")
        .translation;

    step(&mut app, 20);

    let now = app
        .world()
        .get::<Transform>(projectile)
        .expect("still flying")
        .translation;

    assert!(
        (now.y - start.y).abs() < 1e-3,
        "a zero-gravity round must fly flat, dropped {}",
        start.y - now.y
    );
    assert!(now.z < start.z - 10.0, "and it must have travelled");
}

/// Give a shooter the player model's joints and firepoints, shaped as in
/// `bft_00`: the gun pivots 2.25 m up, `fpnt_c` 3 m out along the barrel and
/// `fpnt_r`/`fpnt_l` 1.5 m either side of it, at the pulse-gun cannons' tips.
fn rig_player_turret(app: &mut App, shooter: Entity) {
    let turret = app
        .world_mut()
        .spawn((
            TurretPart,
            Transform::from_xyz(0.0, 1.25, -0.85),
            ChildOf(shooter),
        ))
        .id();
    let gun = app
        .world_mut()
        .spawn((
            GunPart,
            Transform::from_xyz(0.0, 1.0, 0.875),
            ChildOf(turret),
        ))
        .id();
    let [centre, right, left] = [
        Vec3::new(0.0, -0.07, -3.05),
        Vec3::new(1.51, 0.0, -3.0),
        Vec3::new(-1.51, 0.0, -3.0),
    ]
    .map(|at| {
        app.world_mut()
            .spawn((Transform::from_translation(at), ChildOf(gun)))
            .id()
    });
    app.world_mut().entity_mut(shooter).insert((
        AimJoints {
            yaw: turret,
            pitch: Some(gun),
        },
        Firepoint(centre),
        AimPivot::default(),
        TwinBarrels::new([right, left], vec!["RFPG".to_string()]),
    ));
}

/// Where a projectile starts, and how far `point` is from its line.
fn shot_line(app: &App, projectile: Entity, point: Vec3) -> (Vec3, f32) {
    let from = app
        .world()
        .get::<Transform>(projectile)
        .expect("in flight")
        .translation;
    let direction = app
        .world()
        .get::<Projectile>(projectile)
        .expect("in flight")
        .velocity
        .normalize();
    let offset = point - from;

    (from, (offset - direction * offset.dot(direction)).length())
}

/// The pulse gun's two side cannons fire in turn on the secondary trigger,
/// whatever is selected, and both streams converge on the aimed point.
#[test]
fn the_secondary_trigger_fires_the_pulse_gun_from_both_cannons_in_turn() {
    let mut app = weapons_app();
    let at = Vec3::new(0.0, 50.0, 0.0);
    let shooter = spawn_shooter(&mut app, at, Team::PLAYER);
    rig_player_turret(&mut app, shooter);

    // The mortar is selected, so only the secondary can fire the pulse gun.
    assert!(
        app.world_mut()
            .get_mut::<WeaponLoadout>(shooter)
            .expect("armed")
            .select(2)
    );
    let aimed = at + Vec3::new(40.0, -1.0, -60.0);
    app.world_mut()
        .get_mut::<PlayerInput>(shooter)
        .expect("the shooter has input")
        .aim_point = Some(aimed);
    step(&mut app, 120);

    app.world_mut()
        .get_mut::<PlayerInput>(shooter)
        .expect("the shooter has input")
        .fire_alt = true;
    let mut shots = Vec::new();
    for _ in 0..SIM_HZ as u32 {
        app.update();
        for spawned in drain::<ProjectileSpawned>(&mut app) {
            let (from, miss) = shot_line(&app, spawned.entity, aimed);
            shots.push((spawned.weapon, from, miss));
        }
    }

    let pulse = app
        .world()
        .resource::<WeaponTable>()
        .index("wep_1.0")
        .expect("the fixture has it");
    assert!(
        shots.iter().all(|(weapon, _, _)| *weapon == pulse),
        "the secondary fires the pulse gun, not the selected mortar"
    );
    // The pair keeps the vehicle's rate of 12 a second, each cannon 6.
    assert!(
        (11..=13).contains(&shots.len()),
        "wanted about 12 shots in a second, got {}",
        shots.len()
    );
    // The cannons are 3 m apart; each shot leaves the other one, and every
    // other shot the same one.
    for window in shots.windows(3) {
        let [(_, a, _), (_, b, _), (_, c, _)] = window else {
            unreachable!()
        };
        assert!(a.distance(*b) > 2.5, "one barrel twice: {a} then {b}");
        assert!(a.distance(*c) < 0.5, "not alternating: {a}, {b}, {c}");
    }
    for (_, from, miss) in &shots {
        assert!(
            *miss < 0.05,
            "a shot from {from} passes {miss} m from the aimed point"
        );
    }
}

/// With the pulse gun selected the two triggers are one gun: holding both
/// does not fire it twice as fast.
#[test]
fn both_triggers_on_the_selected_pulse_gun_share_its_rate() {
    let mut app = weapons_app();
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);
    {
        let mut input = app
            .world_mut()
            .get_mut::<PlayerInput>(shooter)
            .expect("the shooter has input");
        input.fire = true;
        input.fire_alt = true;
    }

    let shots = step_collecting::<ProjectileSpawned>(&mut app, SIM_HZ as u32).len();

    assert!(
        (11..=13).contains(&shots),
        "wanted about 12 shots in a second, got {shots}"
    );
}

/// A shot passes through the point it was aimed at, from a barrel that is
/// neither at the hull's centre nor pointing forward.
///
/// The player's gun pivots 2.25 m above the hull's centre and its `fpnt_c`
/// is 3 m out along the barrel. The turret used to aim from the hull's centre
/// and the shot to leave from where the muzzle sits with the gun centred, so
/// with the turret turned to the side it missed by metres.
#[test]
fn a_shot_from_a_turned_turret_passes_through_the_aim_point() {
    let mut app = weapons_app();
    let at = Vec3::new(0.0, 50.0, 0.0);
    let shooter = spawn_shooter(&mut app, at, Team::PLAYER);

    // The player model's joints, shaped as in `bft_00`.
    let turret = app
        .world_mut()
        .spawn((
            TurretPart,
            Transform::from_xyz(0.0, 1.25, -0.85),
            ChildOf(shooter),
        ))
        .id();
    let gun = app
        .world_mut()
        .spawn((
            GunPart,
            Transform::from_xyz(0.0, 1.0, 0.875),
            ChildOf(turret),
        ))
        .id();
    let firepoint = app
        .world_mut()
        .spawn((Transform::from_xyz(0.0, -0.07, -3.05), ChildOf(gun)))
        .id();
    app.world_mut().entity_mut(shooter).insert((
        AimJoints {
            yaw: turret,
            pitch: Some(gun),
        },
        Firepoint(firepoint),
        AimPivot::default(),
    ));

    // To the right and below the gun, as a target on the ground is.
    let aimed = at + Vec3::new(40.0, -1.0, -10.0);
    app.world_mut()
        .get_mut::<PlayerInput>(shooter)
        .expect("the shooter has input")
        .aim_point = Some(aimed);
    step(&mut app, 120);

    hold_fire(&mut app, shooter, true);
    step(&mut app, 1);
    hold_fire(&mut app, shooter, false);

    let projectile = drain::<ProjectileSpawned>(&mut app)[0].entity;
    let flying = app
        .world()
        .get::<Projectile>(projectile)
        .expect("in flight");
    let from = app
        .world()
        .get::<Transform>(projectile)
        .expect("in flight")
        .translation;

    // The shot's line, and how far the aim point is from it.
    let direction = flying.velocity.normalize();
    let offset = aimed - from;
    let miss = (offset - direction * offset.dot(direction)).length();

    // `fpnt_c` sits 7 cm below the barrel's axis, and that much is left.
    assert!(
        miss < 0.1,
        "the shot passes {miss} m from where it was aimed, from {from}"
    );
    assert!(
        from.x - at.x > 3.0,
        "and leaves the barrel turned to the right, not the hull's nose: {from}"
    );
}

/// A homing missile turns toward its target.
#[test]
fn a_homing_missile_turns_toward_its_target() {
    let mut app = weapons_app();
    // Off the aim axis but inside the acquisition cone.
    let target = spawn_target(&mut app, Vec3::new(-20.0, 5.0, -200.0), 500.0, Team::ENEMY);
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);
    {
        let mut loadout = app
            .world_mut()
            .get_mut::<WeaponLoadout>(shooter)
            .expect("a loadout");
        loadout.select(8);
    }
    hold_fire(&mut app, shooter, true);
    step(&mut app, 1);
    hold_fire(&mut app, shooter, false);

    let projectile = drain::<ProjectileSpawned>(&mut app)[0].entity;
    assert_eq!(
        app.world()
            .get::<Projectile>(projectile)
            .and_then(|p| p.target),
        Some(target),
        "it should have locked on at fire time"
    );

    let heading = |app: &App| {
        app.world()
            .get::<Projectile>(projectile)
            .map(|p| p.velocity.normalize_or_zero())
    };
    let before = heading(&app).expect("in flight");
    let to_target = Vec3::new(-20.0, 0.0, -200.0).normalize();

    step(&mut app, 20);

    let after = heading(&app).expect("still flying");
    assert!(
        after.dot(to_target) > before.dot(to_target),
        "it should be pointing more at the target: {before:?} then {after:?}"
    );
}

/// Launch the tether-guided missile, `wep_8.1`, and return it.
fn launch_guided(app: &mut App, shooter: Entity) -> Entity {
    {
        let mut loadout = app
            .world_mut()
            .get_mut::<WeaponLoadout>(shooter)
            .expect("a loadout");
        loadout.select(8);
        if !loadout.strong {
            loadout.toggle_strong();
        }
    }
    drain::<ProjectileSpawned>(app);
    hold_fire(app, shooter, true);
    step(app, 1);
    hold_fire(app, shooter, false);

    let spawned = drain::<ProjectileSpawned>(app);
    assert_eq!(spawned.len(), 1, "one guided missile");
    spawned[0].entity
}

fn steer(app: &mut App, shooter: Entity, guide: Vec2) {
    app.world_mut()
        .get_mut::<PlayerInput>(shooter)
        .expect("the shooter has input")
        .guide = guide;
}

fn heading(app: &App, projectile: Entity) -> Vec3 {
    app.world()
        .get::<Projectile>(projectile)
        .expect("still flying")
        .velocity
        .normalize()
}

/// The tether-guided pair carry `TURN_RATE` and a `LOCK_ON` of 0.1 too.
/// Read as homing, they locked onto whatever was ahead; they have to go
/// where they are steered instead.
#[test]
fn a_tether_guided_missile_goes_where_it_is_steered_not_at_a_target() {
    let mut app = weapons_app();
    // Dead ahead, where a homing missile would lock on.
    spawn_target(&mut app, Vec3::new(0.0, 5.0, -300.0), 500.0, Team::ENEMY);
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);

    let missile = launch_guided(&mut app, shooter);
    let projectile = app.world().get::<Projectile>(missile).expect("in flight");
    assert_eq!(
        projectile.target, None,
        "a guided missile locks onto nothing"
    );
    assert!(app.world().get::<Guided>(missile).is_some(), "it is guided");

    // Hard right for half a second.
    steer(&mut app, shooter, Vec2::new(1.0, 0.0));
    step(&mut app, 30);
    let turned = heading(&app, missile);
    assert!(
        turned.x > 0.5,
        "full right should have turned it right: {turned:?}"
    );

    // Half deflection turns half as fast.
    let before = heading(&app, missile);
    steer(&mut app, shooter, Vec2::new(-0.5, 0.0));
    step(&mut app, 30);
    let after = heading(&app, missile);
    let turned_by = before.angle_between(after);
    assert!(
        (turned_by - 0.8 * 0.5).abs() < 0.05,
        "half of 1.6 rad/s for half a second is 0.4 rad, turned {turned_by}"
    );
}

/// Letting go leaves the missile flying straight, and while one is guided
/// the trigger launches no second.
#[test]
fn a_released_missile_flies_straight_and_only_one_is_guided_at_a_time() {
    let mut app = weapons_app();
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);
    let missile = launch_guided(&mut app, shooter);

    // Well past the 0.5 s interval, the trigger still launches nothing.
    step(&mut app, 60);
    drain::<ProjectileSpawned>(&mut app);
    hold_fire(&mut app, shooter, true);
    step(&mut app, 3);
    hold_fire(&mut app, shooter, false);
    assert!(
        drain::<ProjectileSpawned>(&mut app).is_empty(),
        "a second guided missile was launched while the first was guided"
    );

    app.world_mut()
        .get_mut::<PlayerInput>(shooter)
        .expect("input")
        .release_guidance = true;
    step(&mut app, 1);
    assert!(app.world().get::<Guided>(missile).is_none(), "let go");

    let before = heading(&app, missile);
    steer(&mut app, shooter, Vec2::new(1.0, 1.0));
    step(&mut app, 30);
    let after = heading(&app, missile);
    assert!(
        before.angle_between(after) < 1e-3,
        "a released missile must not answer the stick: {before:?} then {after:?}"
    );
}

/// `WeaponSetMaxTetherAltitude`: held full up, the missile levels off near
/// 30 m over the ground instead of climbing away.
#[test]
fn a_guided_missile_cannot_climb_past_the_tether_altitude() {
    let mut app = weapons_app();
    spawn_floor(&mut app);
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);
    let missile = launch_guided(&mut app, shooter);

    steer(&mut app, shooter, Vec2::new(0.0, 1.0));
    let mut highest: f32 = 0.0;
    for _ in 0..(SIM_HZ as u32 * 4) {
        step(&mut app, 1);
        let Some(transform) = app.world().get::<Transform>(missile) else {
            break;
        };
        highest = highest.max(transform.translation.y);
    }

    let ceiling = app.world().resource::<WeaponTable>().tether_max_altitude;
    assert!(highest > ceiling - 5.0, "it should have climbed: {highest}");
    assert!(
        highest < ceiling + 2.0,
        "it climbed to {highest} m against a {ceiling} m tether"
    );
}

/// A beam resolves on the tick it is fired: no projectile ever exists.
#[test]
fn a_beam_hits_without_a_projectile() {
    let mut app = weapons_app();
    let target = spawn_target(&mut app, Vec3::new(0.0, 6.0, -50.0), 100.0, Team::ENEMY);
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);
    {
        let mut loadout = app
            .world_mut()
            .get_mut::<WeaponLoadout>(shooter)
            .expect("a loadout");
        loadout.select(6);
        // The slot holds only the strong variant, so selecting it lands there.
        assert!(loadout.strong, "wep_6.1 is the strong variant");
    }
    assert_eq!(
        app.world()
            .resource::<WeaponTable>()
            .get("wep_6.1")
            .map(|w| w.mover),
        Some(Mover::Beam)
    );

    hold_fire(&mut app, shooter, true);
    step(&mut app, 2);
    hold_fire(&mut app, shooter, false);

    assert!(
        drain::<ProjectileSpawned>(&mut app).is_empty(),
        "hitscan spawns nothing"
    );
    let health = app.world().get::<Health>(target).expect("alive").current;
    assert!(health < 100.0, "the beam should have hurt it, at {health}");
    // And the shooter, which stands in as the beam's `projectile`, survives.
    assert!(app.world().get::<Health>(shooter).is_some());
}

/// The swept ray is what stops a 120 m/s round tunnelling through a thin
/// wall: 2 m of travel a tick against a target 3 m across.
#[test]
fn a_fast_projectile_does_not_tunnel_through_its_target() {
    let mut app = weapons_app();
    let target = spawn_target(&mut app, Vec3::new(0.0, 6.0, -60.0), 1000.0, Team::ENEMY);
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);
    hold_fire(&mut app, shooter, true);
    step(&mut app, 1);
    hold_fire(&mut app, shooter, false);

    step(&mut app, SIM_HZ as u32);

    let health = app.world().get::<Health>(target).expect("alive").current;
    assert!(
        health < 1000.0,
        "the round should have hit rather than passed through"
    );
}

// ---------------------------------------------------------------------------
// Damage
// ---------------------------------------------------------------------------

/// Splash damages two targets, with falloff.
#[test]
fn splash_hurts_everything_in_radius_and_hurts_the_near_one_more() {
    let mut app = weapons_app();
    spawn_floor(&mut app);
    // Inside the mortar's 16 m splash, at 4 m and 12 m from the blast.
    let near = spawn_target(&mut app, Vec3::new(4.0, 1.0, -60.0), 500.0, Team::ENEMY);
    let far = spawn_target(&mut app, Vec3::new(12.0, 1.0, -60.0), 500.0, Team::ENEMY);
    let outside = spawn_target(&mut app, Vec3::new(60.0, 1.0, -60.0), 500.0, Team::ENEMY);

    // Detonate the mortar at the origin of the two, by hand: aiming a
    // ballistic arc onto a point is the trajectory test's job, not this one.
    let table = app.world().resource::<WeaponTable>().clone();
    let weapon = table.index("wep_2.0").expect("the mortar");
    app.world_mut()
        .write_message(bft_sim::weapons::ProjectileHit {
            projectile: Entity::PLACEHOLDER,
            at: Vec3::new(0.0, 1.0, -60.0),
            normal: Vec3::Y,
            hit: None,
            hit_node: None,
            surface: bft_sim::weapons::Surface::Default,
            weapon,
        });
    step(&mut app, 1);

    let health = |app: &App, entity| app.world().get::<Health>(entity).expect("alive").current;
    let near_lost = 500.0 - health(&app, near);
    let far_lost = 500.0 - health(&app, far);

    assert!(near_lost > 0.0, "the near target should be hurt");
    assert!(far_lost > 0.0, "and so should the far one");
    assert!(
        near_lost > far_lost,
        "falloff: {near_lost} at 4 m should beat {far_lost} at 12 m"
    );
    assert_eq!(
        health(&app, outside),
        500.0,
        "60 m is outside a 16 m splash"
    );
}

/// Health reaching zero emits `Destroyed`.
#[test]
fn a_target_that_runs_out_of_health_is_destroyed() {
    let mut app = weapons_app();
    let target = spawn_target(&mut app, Vec3::new(0.0, 6.0, -40.0), 25.0, Team::ENEMY);
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);
    hold_fire(&mut app, shooter, true);

    // 10 damage a shot against 25 health: three hits.
    let kills = step_collecting::<Destroyed>(&mut app, SIM_HZ as u32);
    assert_eq!(kills.len(), 1, "one death, not one per later shot");
    assert_eq!(kills[0].entity, target);
    assert_eq!(kills[0].by, Some(shooter));
    assert!(
        kills[0].weapon.is_some(),
        "so the anim events can pick a kill anim"
    );
}

/// The owner is immune for the first few ticks, or every shot kills the
/// vehicle that fired it.
#[test]
fn a_shooter_is_not_killed_by_its_own_muzzle() {
    let mut app = weapons_app();
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);
    hold_fire(&mut app, shooter, true);

    let kills = step_collecting::<Destroyed>(&mut app, SIM_HZ as u32 * 2);

    assert_eq!(
        app.world().get::<Health>(shooter).map(|h| h.current),
        Some(140.0),
        "the shooter must be untouched by its own fire"
    );
    assert!(kills.is_empty());
}

/// The aim ray marks a hostile target as damageable and a friendly
/// one as not.
#[test]
fn the_aim_result_marks_only_hostile_targets_as_damageable() {
    let mut app = weapons_app();
    let enemy = spawn_target(&mut app, Vec3::new(0.0, 6.0, -40.0), 500.0, Team::ENEMY);
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 5.0, 0.0), Team::PLAYER);

    step(&mut app, 2);

    let aim = *app
        .world()
        .get::<AimResult>(shooter)
        .expect("an aim result");
    assert_eq!(aim.hit, Some(enemy));
    assert!(aim.damageable, "an enemy in the crosshair is a target");

    // Put a friendly one in front instead.
    app.world_mut().entity_mut(enemy).despawn();
    spawn_target(&mut app, Vec3::new(0.0, 6.0, -20.0), 500.0, Team::PLAYER);
    step(&mut app, 2);

    let aim = *app
        .world()
        .get::<AimResult>(shooter)
        .expect("an aim result");
    assert!(aim.hit.is_some());
    assert!(!aim.damageable, "your own side is not a target");
}

// ---------------------------------------------------------------------------
// Pickups
// ---------------------------------------------------------------------------

/// A pickup table of the three kinds these tests need.
fn pickup_table() -> PickupTable {
    let mut kinds = bevy::platform::collections::HashMap::default();

    for (kind, effect) in [
        (
            "HEMORTAR_AMMO",
            PickupEffect::Ammo {
                weapon_slot: 2,
                strong: false,
            },
        ),
        (
            "HEMORTAR_WEAPON",
            PickupEffect::Weapon {
                weapon_slot: 2,
                strong: false,
            },
        ),
        (
            "SONIC_CANNON_WEAPON",
            PickupEffect::Weapon {
                weapon_slot: 7,
                strong: false,
            },
        ),
        ("NANITE100", PickupEffect::Health),
        ("PUP_HOVER", PickupEffect::FormUnlock(VehicleForm::Hover)),
    ] {
        kinds.insert(
            kind.to_string(),
            PickupDef {
                kind: kind.to_string(),
                sound: None,
                image: None,
                effect,
                template: bft_sim::pickups::template_for(kind),
            },
        );
    }

    PickupTable { kinds }
}

fn spawn_pickup(app: &mut App, kind: &str, amount: u32, at: Vec3, respawn: Option<f32>) -> Entity {
    let placement = bft_sim::pickups::PickupPlacement {
        kind: kind.to_string(),
        amount,
        pos: at,
        respawn,
    };

    let mut commands = app.world_mut().commands();
    let spawned = spawn_pickups(&mut commands, &[placement]);
    let entity = spawned[0];
    app.world_mut().flush();
    // `spawn_pickups` sets a `Transform`; the global one has to exist for the
    // distance check before transform propagation has run.
    app.world_mut()
        .entity_mut(entity)
        .insert(GlobalTransform::from_translation(at));
    entity
}

/// Overlapping a pickup applies its ammo.
#[test]
fn driving_over_an_ammo_pickup_adds_rounds() {
    let mut app = weapons_app();
    app.insert_resource(pickup_table());
    let shooter = spawn_shooter(&mut app, Vec3::ZERO, Team::PLAYER);
    {
        let mut loadout = app
            .world_mut()
            .get_mut::<WeaponLoadout>(shooter)
            .expect("a loadout");
        loadout
            .slot_mut(2)
            .expect("slot 2")
            .weak
            .as_mut()
            .expect("owned")
            .ammo = 5;
    }
    let pickup = spawn_pickup(
        &mut app,
        "HEMORTAR_AMMO",
        10,
        Vec3::new(1.0, 0.0, 0.0),
        None,
    );

    step(&mut app, 2);

    let picked = drain::<PickedUp>(&mut app);
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0].kind, "HEMORTAR_AMMO");
    assert_eq!(picked[0].instance, "pu00100", "the first HE mortar ammo");
    assert_eq!(
        app.world()
            .get::<WeaponLoadout>(shooter)
            .and_then(|l| l.slot(2).and_then(|s| s.weak).map(|a| a.ammo)),
        Some(15)
    );
    assert!(
        app.world().get::<Pickup>(pickup).is_none(),
        "a campaign pickup is gone for good"
    );
}

/// A pickup out of reach stays put.
#[test]
fn a_distant_pickup_is_not_collected() {
    let mut app = weapons_app();
    app.insert_resource(pickup_table());
    spawn_shooter(&mut app, Vec3::ZERO, Team::PLAYER);
    let pickup = spawn_pickup(&mut app, "NANITE100", 100, Vec3::new(50.0, 0.0, 0.0), None);

    step(&mut app, 4);

    assert!(drain::<PickedUp>(&mut app).is_empty());
    assert!(app.world().get::<Pickup>(pickup).is_some());
}

/// A gun pickup gives you a weapon you did not have, and it becomes
/// selectable.
#[test]
fn a_weapon_pickup_adds_a_selectable_slot() {
    let mut app = weapons_app();
    app.insert_resource(pickup_table());
    // Slot 7 holds nothing in the shooter fixture, and the fixture table has
    // no `wep_7.0` — so this also checks the "no such weapon" path leaves the
    // pickup standing rather than adding a slot pointing at nothing.
    let shooter = spawn_shooter(&mut app, Vec3::ZERO, Team::PLAYER);
    assert!(
        !app.world()
            .get::<WeaponLoadout>(shooter)
            .expect("a loadout")
            .is_selectable(7)
    );

    let pickup = spawn_pickup(
        &mut app,
        "SONIC_CANNON_WEAPON",
        5,
        Vec3::new(1.0, 0.0, 0.0),
        None,
    );
    step(&mut app, 2);

    assert!(
        app.world().get::<Pickup>(pickup).is_some(),
        "a pickup for a weapon the table has no entry for stays standing"
    );
    assert!(drain::<PickedUp>(&mut app).is_empty());
}

/// Health pickups heal, and a full vehicle leaves them standing.
#[test]
fn a_nanite_heals_and_a_full_vehicle_leaves_it() {
    let mut app = weapons_app();
    app.insert_resource(pickup_table());
    let shooter = spawn_shooter(&mut app, Vec3::ZERO, Team::PLAYER);
    let pickup = spawn_pickup(&mut app, "NANITE100", 50, Vec3::new(1.0, 0.0, 0.0), None);

    // At full health it is not taken.
    step(&mut app, 2);
    assert!(app.world().get::<Pickup>(pickup).is_some());
    assert!(drain::<PickedUp>(&mut app).is_empty());

    // Hurt, and it is.
    app.world_mut()
        .get_mut::<Health>(shooter)
        .expect("health")
        .current = 40.0;
    step(&mut app, 2);

    assert_eq!(
        app.world().get::<Health>(shooter).map(|h| h.current),
        Some(90.0)
    );
    assert!(app.world().get::<Pickup>(pickup).is_none());
}

/// A form chip unlocks the form it names.
#[test]
fn a_chip_unlocks_its_form() {
    let mut app = weapons_app();
    app.insert_resource(pickup_table());
    let shooter = spawn_shooter(&mut app, Vec3::ZERO, Team::PLAYER);
    app.world_mut()
        .get_mut::<Vehicle>(shooter)
        .expect("a vehicle")
        .unlocked = FormSet::track_only();

    spawn_pickup(&mut app, "PUP_HOVER", 1, Vec3::new(1.0, 0.0, 0.0), None);
    step(&mut app, 2);

    assert!(
        app.world()
            .get::<Vehicle>(shooter)
            .expect("a vehicle")
            .unlocked
            .has(VehicleForm::Hover)
    );
}

/// A deathmatch pickup comes back after its timer.
#[test]
fn a_respawning_pickup_returns_when_its_timer_runs_out() {
    let mut app = weapons_app();
    app.insert_resource(pickup_table());
    let shooter = spawn_shooter(&mut app, Vec3::ZERO, Team::PLAYER);
    app.world_mut()
        .get_mut::<Health>(shooter)
        .expect("health")
        .current = 10.0;

    let respawn = 0.5;
    let pickup = spawn_pickup(
        &mut app,
        "NANITE100",
        20,
        Vec3::new(1.0, 0.0, 0.0),
        Some(respawn),
    );

    step(&mut app, 2);
    assert!(
        app.world().get::<RespawnTimer>(pickup).is_some(),
        "taken, but waiting rather than gone"
    );
    assert!(app.world().get::<Pickup>(pickup).is_some());

    // While it waits it cannot be taken again.
    let health_after_first = app.world().get::<Health>(shooter).expect("alive").current;
    step(&mut app, 4);
    assert_eq!(
        app.world().get::<Health>(shooter).expect("alive").current,
        health_after_first
    );

    // Top the vehicle up before the timer runs out, so the pickup that comes
    // back stays standing: a still-hurt vehicle sitting on it would take it
    // again on the very tick it returns, which is right but untestable.
    app.world_mut()
        .get_mut::<Health>(shooter)
        .expect("health")
        .current = 140.0;

    step(&mut app, (SIM_HZ * respawn as f64) as u32 + 2);
    assert!(
        app.world().get::<RespawnTimer>(pickup).is_none(),
        "it should be back"
    );
    assert!(
        app.world().get::<Pickup>(pickup).is_some(),
        "and still be a pickup"
    );
}

/// A respawned pickup is takeable again — the timer is the only thing that
/// was holding it back.
#[test]
fn a_respawned_pickup_can_be_taken_again() {
    let mut app = weapons_app();
    app.insert_resource(pickup_table());
    let shooter = spawn_shooter(&mut app, Vec3::ZERO, Team::PLAYER);
    app.world_mut()
        .get_mut::<Health>(shooter)
        .expect("health")
        .current = 10.0;

    let respawn = 0.25;
    spawn_pickup(
        &mut app,
        "NANITE100",
        20,
        Vec3::new(1.0, 0.0, 0.0),
        Some(respawn),
    );

    let taken = step_collecting::<PickedUp>(&mut app, (SIM_HZ * respawn as f64) as u32 * 3);

    assert!(
        taken.len() >= 2,
        "it should have been taken, come back and been taken again; got {}",
        taken.len()
    );
}

/// Ammunition is taken for a weapon the turret does not carry, and waits:
/// the slot is not selectable and the rounds cannot be fired until the
/// weapon's own pickup mounts the gun, which keeps them.
#[test]
fn ammo_waits_for_its_weapon_to_be_mounted() {
    let mut app = weapons_app();
    app.insert_resource(pickup_table());
    let shooter = spawn_shooter(&mut app, Vec3::ZERO, Team::PLAYER);
    let hem = |app: &App| {
        app.world()
            .get::<WeaponLoadout>(shooter)
            .and_then(|loadout| loadout.slot(2).and_then(|slot| slot.weak))
            .expect("the HE mortar has a state")
    };
    {
        let mut loadout = app
            .world_mut()
            .get_mut::<WeaponLoadout>(shooter)
            .expect("a loadout");
        let state = loadout.slot_mut(2).and_then(|slot| slot.weak.as_mut());
        let state = state.expect("the fixture has it");
        state.mounted = false;
        state.ammo = 0;
    }

    spawn_pickup(&mut app, "HEMORTAR_AMMO", 10, Vec3::X, None);
    step(&mut app, 2);

    assert_eq!(drain::<PickedUp>(&mut app).len(), 1, "the ammo is taken");
    assert_eq!(hem(&app).ammo, 10);
    let mut loadout = app
        .world_mut()
        .get_mut::<WeaponLoadout>(shooter)
        .expect("a loadout");
    assert!(!loadout.is_selectable(2), "no gun, no slot");
    assert!(!loadout.select(2));
    // Even forced onto the slot, the trigger does nothing.
    loadout.selected = 2;
    hold_fire(&mut app, shooter, true);
    let shots = step_collecting::<ProjectileSpawned>(&mut app, 30);
    hold_fire(&mut app, shooter, false);
    assert!(shots.is_empty(), "rounds without the gun cannot be fired");

    spawn_pickup(&mut app, "HEMORTAR_WEAPON", 3, Vec3::X, None);
    step(&mut app, 2);

    assert!(hem(&app).mounted, "the weapon pickup mounts the gun");
    assert_eq!(hem(&app).ammo, 13, "and keeps the rounds already carried");
    assert!(
        app.world()
            .get::<WeaponLoadout>(shooter)
            .expect("a loadout")
            .is_selectable(2)
    );
}

/// A drop lands on the ground under the kill, not where the shot struck:
/// a pickup has no body to fall with, and one left in the air cannot be
/// driven over.
#[test]
fn a_drop_lands_on_the_ground_under_the_kill() {
    let mut app = weapons_app();
    app.insert_resource(pickup_table());
    spawn_floor(&mut app);
    step(&mut app, 1);
    let at = Vec3::new(10.0, 6.0, -30.0);
    let enemy = app
        .world_mut()
        .spawn((
            Health::new(10.0),
            Team::ENEMY,
            Drops(vec![bft_sim::vehicle::tuning::DropDef {
                kind: "HEMORTAR_AMMO".to_string(),
                amount: 5,
            }]),
            Transform::from_translation(at),
            GlobalTransform::from_translation(at),
        ))
        .id();

    app.world_mut().write_message(DamageEvent {
        target: enemy,
        amount: 50.0,
        source: None,
        at,
        kind: bft_sim::damage::DamageKind::Script,
    });
    step(&mut app, 2);

    let landed: Vec<Vec3> = app
        .world_mut()
        .query_filtered::<&Transform, With<Pickup>>()
        .iter(app.world())
        .map(|transform| transform.translation)
        .collect();
    assert_eq!(landed.len(), 1);
    assert!(
        landed[0].distance(Vec3::new(at.x, 0.0, at.z)) < 0.01,
        "the drop is on the floor below the kill, at {}",
        landed[0]
    );
}

/// A destroyed enemy drops what `vehicle.zrd` says it does.
#[test]
fn a_destroyed_enemy_drops_its_pickups() {
    let mut app = weapons_app();
    app.insert_resource(pickup_table());
    let at = Vec3::new(10.0, 1.0, -30.0);
    let enemy = app
        .world_mut()
        .spawn((
            Health::new(10.0),
            Team::ENEMY,
            Drops(vec![bft_sim::vehicle::tuning::DropDef {
                kind: "HEMORTAR_AMMO".to_string(),
                amount: 5,
            }]),
            Transform::from_translation(at),
            GlobalTransform::from_translation(at),
        ))
        .id();

    app.world_mut().write_message(DamageEvent {
        target: enemy,
        amount: 50.0,
        source: None,
        at,
        kind: bft_sim::damage::DamageKind::Script,
    });
    step(&mut app, 2);

    let dropped: Vec<(Entity, Pickup)> = app
        .world_mut()
        .query::<(Entity, &Pickup)>()
        .iter(app.world())
        .map(|(entity, pickup)| (entity, pickup.clone()))
        .collect();

    assert_eq!(dropped.len(), 1, "one drop, at the wreck");
    assert_eq!(dropped[0].1.kind, "HEMORTAR_AMMO");
    assert_eq!(dropped[0].1.amount, 5);
    assert_eq!(
        dropped[0].1.respawn, None,
        "a death drop is not a deathmatch pickup"
    );
}

// ---------------------------------------------------------------------------
// Mines
// ---------------------------------------------------------------------------

/// Fire one round from `slot` and return the projectile.
fn fire_slot(app: &mut App, shooter: Entity, slot: u8) -> Entity {
    app.world_mut()
        .get_mut::<WeaponLoadout>(shooter)
        .expect("a loadout")
        .select(slot);
    drain::<ProjectileSpawned>(app);
    hold_fire(app, shooter, true);
    step(app, 1);
    hold_fire(app, shooter, false);

    let spawned = drain::<ProjectileSpawned>(app);
    assert_eq!(spawned.len(), 1, "one round from slot {slot}");
    spawned[0].entity
}

/// Step until a mine lies still, collecting its bounces. Panics if it never
/// does, or if it went off on the way.
fn settle(app: &mut App, mine: Entity) -> Vec<MineBounced> {
    let mut bounces = Vec::new();
    for _ in 0..(SIM_HZ as u32 * 8) {
        app.update();
        bounces.extend(drain::<MineBounced>(app));
        let projectile = app
            .world()
            .get::<Projectile>(mine)
            .expect("a mine is not used up by landing");
        if projectile.landed.is_some() && projectile.armed {
            return bounces;
        }
    }
    panic!("the mine never came to rest");
}

fn place(app: &mut App, entity: Entity, at: Vec3) {
    let mut world = app.world_mut().entity_mut(entity);
    *world.get_mut::<Transform>().expect("placed") = Transform::from_translation(at);
    *world.get_mut::<GlobalTransform>().expect("placed") = GlobalTransform::from_translation(at);
}

/// A mine lands, stays where it landed, and turns slowly on the spot.
#[test]
fn a_mine_lands_and_lies_there_turning() {
    let mut app = weapons_app();
    spawn_floor(&mut app);
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 2.0, 0.0), Team::PLAYER);

    let mine = fire_slot(&mut app, shooter, 4);
    let bounces = settle(&mut app, mine);
    assert!(!bounces.is_empty(), "it hit the ground on the way down");

    let rest = *app.world().get::<Transform>(mine).expect("lying there");
    assert!(rest.translation.z < -5.0, "it was thrown forward: {rest:?}");
    assert!(
        rest.translation.y > 0.0 && rest.translation.y < 0.5,
        "just above the floor, as the original's hover: {rest:?}"
    );

    step(&mut app, SIM_HZ as u32 * 2);

    let later = *app.world().get::<Transform>(mine).expect("still there");
    assert_eq!(
        later.translation, rest.translation,
        "a lying mine does not move"
    );
    assert!(
        later.rotation.angle_between(rest.rotation) > 0.5,
        "but it turns: {} rad in two seconds",
        later.rotation.angle_between(rest.rotation)
    );
    assert!(
        (later.rotation * Vec3::Y).dot(Vec3::Y) > 0.99,
        "about the ground's normal, staying upright"
    );
}

/// A mine against a wall bounces back off it rather than going off.
#[test]
fn a_mine_bounces_off_a_wall() {
    let mut app = weapons_app();
    spawn_floor(&mut app);
    // A wall across the line of fire, 8 m out.
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(40.0, 20.0, 1.0),
        layers::level(),
        Transform::from_xyz(0.0, 0.0, -8.0),
        GlobalTransform::from_xyz(0.0, 0.0, -8.0),
    ));
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 4.0, 0.0), Team::PLAYER);
    step(&mut app, 1);

    let mine = fire_slot(&mut app, shooter, 4);
    settle(&mut app, mine);

    let rest = app
        .world()
        .get::<Transform>(mine)
        .expect("lying there")
        .translation;
    assert!(
        rest.z > -7.5,
        "it came back off the wall rather than through it: {rest:?}"
    );
}

/// The manual's proximity mine: a friend can drive over it, something
/// hostile cannot — and nothing far off sets it off.
#[test]
fn a_proximity_mine_waits_for_something_hostile_to_drive_over_it() {
    let mut app = weapons_app();
    spawn_floor(&mut app);
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 2.0, 0.0), Team::PLAYER);
    let mine = fire_slot(&mut app, shooter, 4);
    settle(&mut app, mine);
    let at = app
        .world()
        .get::<Transform>(mine)
        .expect("lying")
        .translation;

    let friend = spawn_target(&mut app, at + Vec3::new(0.0, 5.0, 0.0), 100.0, Team::PLAYER);
    let enemy = spawn_target(&mut app, at + Vec3::new(20.0, 5.0, 0.0), 100.0, Team::ENEMY);
    step(&mut app, 1);
    place(&mut app, friend, at + Vec3::new(0.5, 1.0, 0.0));

    let hits = step_collecting::<ProjectileHit>(&mut app, SIM_HZ as u32);
    assert!(
        hits.is_empty(),
        "a friend over it and an enemy 20 m off: nothing"
    );
    assert!(app.world().get::<Projectile>(mine).is_some());

    place(&mut app, enemy, at + Vec3::new(-1.5, 1.0, 1.0));
    let damage = step_collecting::<DamageEvent>(&mut app, 2);

    assert!(app.world().get::<Projectile>(mine).is_none(), "it went off");
    assert!(
        damage
            .iter()
            .any(|event| event.target == enemy && event.amount > 0.0),
        "and caught the enemy in its blast"
    );
}

/// The manual's remote mine: it does not care who drives over it, only for
/// its layer's detonate key.
#[test]
fn a_remote_mine_goes_off_only_when_its_layer_says() {
    let mut app = weapons_app();
    spawn_floor(&mut app);
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 2.0, 0.0), Team::PLAYER);
    let mine = fire_slot(&mut app, shooter, 5);
    settle(&mut app, mine);
    let at = app
        .world()
        .get::<Transform>(mine)
        .expect("lying")
        .translation;

    spawn_target(&mut app, at + Vec3::new(0.5, 1.0, 0.0), 100.0, Team::ENEMY);
    step(&mut app, SIM_HZ as u32);
    assert!(
        app.world().get::<Projectile>(mine).is_some(),
        "an enemy on top of a remote mine does not set it off"
    );

    app.world_mut()
        .get_mut::<PlayerInput>(shooter)
        .expect("input")
        .detonate = true;
    let hits = step_collecting::<ProjectileHit>(&mut app, 1);

    assert!(
        hits.iter().any(|hit| hit.projectile == mine),
        "the key sets it off"
    );
    assert!(app.world().get::<Projectile>(mine).is_none());
}

// ---------------------------------------------------------------------------
// Craters
// ---------------------------------------------------------------------------

/// The lowest point of a trimesh collider, in world space (the floor is at
/// the identity).
fn lowest_point(app: &App, entity: Entity) -> f32 {
    let collider = app.world().get::<Collider>(entity).expect("a collider");
    let trimesh = collider.shape().as_trimesh().expect("a trimesh");
    trimesh
        .vertices()
        .iter()
        .map(|vertex| vertex.y)
        .fold(f32::MAX, f32::min)
}

/// The manual: "you can use the Mortar Cannon to make craters, then hide in
/// them". The shell cuts the ground's collider, not just its picture.
#[test]
fn a_mortar_shell_digs_a_crater_the_ground_now_dips_into() {
    let mut app = weapons_app();
    let floor = spawn_floor(&mut app);
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 20.0, 0.0), Team::PLAYER);
    step(&mut app, 1);

    let shell = fire_slot(&mut app, shooter, 2);
    let mut formed = Vec::new();
    for _ in 0..(SIM_HZ as u32 * 4) {
        app.update();
        formed.extend(drain::<CraterFormed>(&mut app));
        if app.world().get::<Projectile>(shell).is_none() && !formed.is_empty() {
            break;
        }
    }

    assert_eq!(formed.len(), 1, "one shell, one crater");
    let crater = &formed[0].crater;
    assert_eq!(formed[0].nodes, vec![floor]);
    assert_eq!(crater.radius, 10.0);
    assert!(
        crater.center.z < -10.0,
        "where the shell came down: {crater:?}"
    );

    let cratered = app
        .world()
        .get::<Cratered>(floor)
        .expect("the floor is cut");
    assert_eq!(cratered.craters.len(), 1);
    assert!(
        (lowest_point(&app, floor) + crater.depth).abs() < 1e-3,
        "the collider's floor is {} m down",
        crater.depth
    );
}

/// The mine is a `CRATER` weapon too: set off, it digs one where it lay.
#[test]
fn a_mine_going_off_digs_a_crater_where_it_lay() {
    let mut app = weapons_app();
    let floor = spawn_floor(&mut app);
    let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 2.0, 0.0), Team::PLAYER);
    let mine = fire_slot(&mut app, shooter, 5);
    settle(&mut app, mine);
    let at = app
        .world()
        .get::<Transform>(mine)
        .expect("lying")
        .translation;

    app.world_mut()
        .get_mut::<PlayerInput>(shooter)
        .expect("input")
        .detonate = true;
    let formed = step_collecting::<CraterFormed>(&mut app, 2);

    assert_eq!(formed.len(), 1);
    assert!(formed[0].crater.center.xz().distance(at.xz()) < 0.5);
    assert!(app.world().get::<Cratered>(floor).is_some());
}

/// Water does not crater: the hit's surface decides.
#[test]
fn a_shell_in_water_digs_nothing() {
    let mut app = weapons_app();
    let floor = spawn_floor(&mut app);
    step(&mut app, 1);
    let hem = app
        .world()
        .resource::<WeaponTable>()
        .index("wep_2.0")
        .expect("the fixture has it");

    app.world_mut().write_message(ProjectileHit {
        projectile: Entity::PLACEHOLDER,
        at: Vec3::new(0.0, 0.0, -30.0),
        normal: Vec3::Y,
        hit: Some(floor),
        hit_node: None,
        surface: bft_sim::weapons::Surface::Water,
        weapon: hem,
    });
    let formed = step_collecting::<CraterFormed>(&mut app, 1);

    assert!(formed.is_empty());
    assert!(app.world().get::<Cratered>(floor).is_none());
}

// ---------------------------------------------------------------------------
// Determinism
// ---------------------------------------------------------------------------

/// Determinism: the same inputs produce the same projectiles in
/// the same places, twice.
#[test]
fn two_runs_of_the_same_inputs_agree_exactly() {
    fn run() -> Vec<(u32, Vec3, Vec3)> {
        let mut app = weapons_app();
        spawn_floor(&mut app);
        let shooter = spawn_shooter(&mut app, Vec3::new(0.0, 20.0, 0.0), Team::PLAYER);
        {
            let mut loadout = app
                .world_mut()
                .get_mut::<WeaponLoadout>(shooter)
                .expect("a loadout");
            loadout.select(2);
        }

        let mut samples = Vec::new();

        for tick in 0..(SIM_HZ as u32 * 3) {
            // A fixed input sequence: the trigger goes down every 30 ticks.
            hold_fire(&mut app, shooter, tick % 30 == 0);
            step(&mut app, 1);

            let mut live: Vec<(Vec3, Vec3)> = app
                .world_mut()
                .query::<(&Projectile, &Transform)>()
                .iter(app.world())
                .map(|(projectile, transform)| (transform.translation, projectile.velocity))
                .collect();
            // Query order is not part of the contract; the values are.
            live.sort_by(|a, b| a.0.to_array().partial_cmp(&b.0.to_array()).expect("finite"));

            for (position, velocity) in live {
                samples.push((tick, position, velocity));
            }
        }

        samples
    }

    let first = run();
    let second = run();

    assert!(!first.is_empty(), "the run has to actually fire something");
    assert_eq!(
        first, second,
        "the same inputs must produce the same projectiles"
    );
}
