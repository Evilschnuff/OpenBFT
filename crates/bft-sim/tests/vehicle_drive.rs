//! The vehicle's headless tests: a vehicle drives, turns, floats and
//! dives, with no renderer anywhere.
//!
//! These need a whole `App` stepping avian inside the sim schedule, which is
//! what makes them integration tests rather than units next to the code. They
//! use a flat test slab and a hand-built [`VehicleTable`] rather than the
//! real data, so they run for a contributor without the game; the checks that
//! need the real `vehicle.zrd` live in `real_vehicle.rs`.

use avian3d::prelude::*;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bft_assets::materials::{MaterialDesc, Soil};
use bft_assets::models::{ModelDesc, ModelType, PolygonDesc, PolygonMaterialDesc};
use bft_sim::level::surface_info;
use bft_sim::physics::layers;
use bft_sim::schedule::SIM_HZ;
use bft_sim::vehicle::tuning::{
    ModeSounds, ModeTuning, PlayerConstants, SuspensionTuning, VehicleAiDefaults, VehicleDef,
    VehicleKindId, VehicleTable,
};
use bft_sim::vehicle::{
    FormSet, GroundState, PlayerInput, PlayerVehicle, Traction, Turret, Vehicle, VehicleForm,
    spawn_player_vehicle,
};
use bft_sim::water::{LiquidKind, WaterVolume};

/// An app with the sim's schedule and avian, and nothing that draws.
fn vehicle_app() -> App {
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

/// A 400 m square slab at y = 0, big enough to reach top speed on.
fn slab(soil_material: u32) -> ModelDesc {
    ModelDesc {
        vertices: vec![
            Vec3::new(-200.0, 0.0, -200.0),
            Vec3::new(-200.0, 0.0, 200.0),
            Vec3::new(200.0, 0.0, 200.0),
            Vec3::new(200.0, 0.0, -200.0),
        ],
        polygons: vec![PolygonDesc {
            vertex_indices: vec![0, 1, 2, 3],
            materials: vec![PolygonMaterialDesc {
                material_index: soil_material,
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
    let model = slab(0);
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

/// A mode with the player's real `track_mode` values, run through the same
/// derivation the loader uses.
fn mode(form: VehicleForm) -> ModeTuning {
    // The reader's own numbers for the player's four modes (READER-SCHEMA
    // §2.1), so the test drives what the game would.
    let (rates, turns, turn_damping, rate_damping, mode_alt) = match form {
        VehicleForm::Track => ([48.0, 65.0], [2.5, 3.5], 12.0, [4.0, 1.25], 0.0),
        VehicleForm::Amphib => ([22.0, 50.0], [2.0, 3.0], 10.0, [0.01, 0.5], -0.1),
        VehicleForm::Hover => ([20.0, 55.0], [1.0, 3.0], 4.0, [1.0, 1.0], 0.5),
        VehicleForm::Sub => ([25.0, 55.0], [1.0, 3.0], 12.0, [4.0, 2.0], -0.5),
    };

    // Per form, not one set for all four: `friction[1]` is what the grip
    // model reads, and giving hover the tracks' 80 hides the difference the
    // data draws between them.
    let friction = match form {
        VehicleForm::Track => [150.0, 80.0, 0.02],
        VehicleForm::Amphib => [140.0, 90.0, 0.02],
        VehicleForm::Hover => [80.0, 40.0, 0.05],
        VehicleForm::Sub => [300.0, 200.0, 0.001],
    };

    let mut tuning = ModeTuning {
        platform: Some([
            Vec3::new(-2.25, 0.0, -2.4),
            Vec3::new(2.25, 0.0, -2.4),
            Vec3::new(2.25, 0.0, 2.34),
            Vec3::new(-2.25, 0.0, 2.34),
        ]),
        collision: vec![Vec3::new(-2.7, 1.0, -3.34), Vec3::new(2.7, 1.0, 3.34)],
        mode_alt,
        rates,
        turns,
        turn_damping,
        rate_damping,
        friction: Some(friction),
        mass: Some(2.2),
        a_damping: Some(40.0),
        chas_smooth: None,
        chas_pitch: None,
        chas_roll: None,
        collision_d: [0.35, 0.0],
        quicksand_slowdown: Some(0.4),
        lava_slowdown: Some(0.2),
        ok_to_amphib: form == VehicleForm::Track,
        alt_control: None,
        gun_pitch: (form == VehicleForm::Sub).then_some([-0.1736, 0.5]),
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

    // The loader's derivation, reached the same way the real path does: by
    // building a table from reader JSON. Doing it by hand here would let the
    // test and the code disagree about what is derived.
    tuning.derive();
    tuning
}

/// Every form, so a test can switch between them.
fn table() -> VehicleTable {
    let mut modes = HashMap::default();
    for form in VehicleForm::ALL {
        modes.insert(form, mode(form));
    }

    let mut kinds = HashMap::default();
    kinds.insert(
        VehicleKindId::player(),
        VehicleDef {
            name: String::new(),
            health: 140.0,
            shield: Some(60.0),
            modes,
            camback: [
                Vec3::new(0.0, 4.0, 12.75),
                Vec3::new(0.0, 3.5, 2.25),
                Vec3::new(0.0, 3.5, 2.25),
            ],
            aim_y: [3.0, 3.0],
            turret_rate: None,
            weapons: Vec::new(),
            ai: VehicleAiDefaults::default(),
            drops: Vec::new(),
            sounds: Default::default(),
            track_switch: Some([10.0, 25.0, 45.0]),
        },
    );

    VehicleTable {
        kinds,
        player: PlayerConstants::default(),
    }
}

/// Put a player vehicle on the slab, with every form unlocked.
fn spawn_player(app: &mut App, at: Transform) -> Entity {
    spawn_player_with(app, at, FormSet::all())
}

/// Put a player vehicle down with a chosen set of unlocks.
fn spawn_player_with(app: &mut App, at: Transform, unlocked: FormSet) -> Entity {
    let table = table();
    app.insert_resource(table.clone());

    app.world_mut()
        .run_system_cached_with(
            move |In((at, table, unlocked)): In<(Transform, VehicleTable, FormSet)>,
                  mut commands: Commands|
                  -> Entity {
                spawn_player_vehicle(&mut commands, at, &table, unlocked)
            },
            (at, table, unlocked),
        )
        .expect("the spawn system runs")
}

/// Set the player's input for the ticks that follow.
fn set_input(app: &mut App, entity: Entity, input: PlayerInput) {
    *app.world_mut()
        .get_mut::<PlayerInput>(entity)
        .expect("the vehicle has an input") = input;
}

/// Hold an input across several ticks, re-applying it each tick.
///
/// The sim clears the edge-triggered fields every tick
/// (`consume_input`), and the frame layer is what re-supplies the held ones —
/// so a test that wants the throttle down for two seconds has to act as that
/// layer.
fn drive_for(app: &mut App, entity: Entity, input: PlayerInput, ticks: u32) {
    for _ in 0..ticks {
        set_input(app, entity, input);
        app.update();
    }
}

fn speed(app: &App, entity: Entity) -> f32 {
    app.world()
        .get::<LinearVelocity>(entity)
        .expect("a velocity")
        .0
        .length()
}

fn position(app: &App, entity: Entity) -> Vec3 {
    app.world()
        .get::<Transform>(entity)
        .expect("a transform")
        .translation
}

/// Full throttle for 120 ticks reaches within
/// 10 % of `max_speed`.
#[test]
fn full_throttle_reaches_top_speed() {
    let mut app = vehicle_app();
    spawn_slab(&mut app);
    let player = spawn_player(&mut app, Transform::from_xyz(-150.0, 2.0, 0.0));

    // A second to settle onto the suspension before the run, so the spring is
    // not still absorbing the drop when the throttle goes down.
    step(&mut app, SIM_HZ as u32);

    drive_for(
        &mut app,
        player,
        PlayerInput {
            throttle: 1.0,
            ..default()
        },
        120,
    );

    let max_speed = table()
        .player_def()
        .and_then(|def| def.mode(VehicleForm::Track))
        .expect("track")
        .max_speed;
    let reached = speed(&app, player);

    assert!(
        reached > max_speed * 0.9,
        "after 2 s of full throttle the vehicle should be near its {max_speed} m/s \
         top speed, not at {reached}"
    );
    assert!(
        reached < max_speed * 1.15,
        "it should not exceed the top speed by more than the clamp's overshoot: {reached}"
    );
}

/// Grip is a force limit, so a hard turn at speed slides: the velocity
/// vector lags the heading, which is what a skid *is*.
///
/// Measured as the angle between where the hull points and where it is
/// actually going. A model that cancels a fixed fraction of the sideways
/// velocity keeps this near zero at every speed, which is the behaviour this
/// replaced.
#[test]
fn a_hard_turn_at_speed_slides_rather_than_tracking_true() {
    let mut app = vehicle_app();
    spawn_slab(&mut app);
    let player = spawn_player(&mut app, Transform::from_xyz(-150.0, 2.0, 0.0));
    step(&mut app, SIM_HZ as u32);

    // Up to speed in a straight line first.
    drive_for(
        &mut app,
        player,
        PlayerInput {
            throttle: 1.0,
            ..default()
        },
        150,
    );
    let entry_speed = speed(&app, player);
    assert!(
        entry_speed > 30.0,
        "the turn has to be entered fast to slide; got {entry_speed}"
    );

    // Then throw it into a full-rate turn.
    drive_for(
        &mut app,
        player,
        PlayerInput {
            throttle: 1.0,
            steer: 1.0,
            ..default()
        },
        30,
    );

    let slip = slip_angle(&app, player);
    assert!(
        slip > 0.08,
        "a full-rate turn above the grip limit should slide: slip is only \
         {:.1} degrees",
        slip.to_degrees()
    );
}

/// And below the limit it does not: a gentle turn at low speed tracks true,
/// or the vehicle would feel like it was on ice everywhere.
#[test]
fn a_gentle_turn_at_low_speed_keeps_its_grip() {
    let mut app = vehicle_app();
    spawn_slab(&mut app);
    let player = spawn_player(&mut app, Transform::from_xyz(-150.0, 2.0, 0.0));
    step(&mut app, SIM_HZ as u32);

    // A third of the throttle, so the speed stays well under the ~27 m/s
    // where the tracks let go.
    let gentle = PlayerInput {
        throttle: 0.35,
        steer: 0.5,
        ..default()
    };
    drive_for(&mut app, player, gentle, 120);

    let cruising = speed(&app, player);
    assert!(
        cruising < 27.0,
        "this test is only meaningful below the grip limit; got {cruising}"
    );

    let slip = slip_angle(&app, player);
    assert!(
        slip < 0.08,
        "a gentle turn should track true, not slide {:.1} degrees",
        slip.to_degrees()
    );
}

/// The whole point of two thresholds: once traction goes, it **stays** gone
/// until the slide has decayed well below what broke it.
///
/// A single-threshold model re-grips the moment lateral speed dips under its
/// limit, which with a turn still being held happens within a tick or two —
/// so the vehicle catches itself continuously and stays fully pointable
/// through what should be a slide. That is the "still feels very steerable"
/// symptom, and it is what this test would catch coming back.
#[test]
fn a_slide_stays_broken_rather_than_catching_itself_every_tick() {
    let mut app = vehicle_app();
    spawn_slab(&mut app);
    let player = spawn_player(&mut app, Transform::from_xyz(-150.0, 2.0, 0.0));
    step(&mut app, SIM_HZ as u32);

    // Up to speed, then hard over.
    drive_for(
        &mut app,
        player,
        PlayerInput {
            throttle: 1.0,
            ..default()
        },
        150,
    );
    let turning = PlayerInput {
        throttle: 1.0,
        steer: 1.0,
        ..default()
    };

    // Find the tick it breaks loose on.
    let mut broke = None;
    for tick in 0..120 {
        drive_for(&mut app, player, turning, 1);
        if traction(&app, player).sliding {
            broke = Some(tick);
            break;
        }
    }
    let broke = broke.expect("a full-rate turn at speed should break traction");

    // Then hold the same input and count how much of the next stretch it
    // spends sliding. With hysteresis it should be nearly all of it; without
    // it, the state flickers.
    let mut sliding_ticks = 0;
    const WINDOW: u32 = 30;
    for _ in 0..WINDOW {
        drive_for(&mut app, player, turning, 1);
        if traction(&app, player).sliding {
            sliding_ticks += 1;
        }
    }

    assert!(
        sliding_ticks > WINDOW * 3 / 4,
        "after breaking loose on tick {broke} the vehicle should stay sliding \
         while the turn is held, but slid on only {sliding_ticks} of the next \
         {WINDOW} ticks"
    );
}

/// And the slide ends when the vehicle slows: a skid that never recovers
/// would be worse than one that never starts.
#[test]
fn a_slide_recovers_once_the_vehicle_stops_turning() {
    let mut app = vehicle_app();
    spawn_slab(&mut app);
    let player = spawn_player(&mut app, Transform::from_xyz(-150.0, 2.0, 0.0));
    step(&mut app, SIM_HZ as u32);

    drive_for(
        &mut app,
        player,
        PlayerInput {
            throttle: 1.0,
            ..default()
        },
        150,
    );
    drive_for(
        &mut app,
        player,
        PlayerInput {
            throttle: 1.0,
            steer: 1.0,
            ..default()
        },
        45,
    );
    assert!(
        traction(&app, player).sliding,
        "the turn should have broken traction"
    );

    // Straighten up and coast.
    drive_for(&mut app, player, PlayerInput::default(), 120);

    assert!(
        !traction(&app, player).sliding,
        "the tracks should bite again once the slide decays"
    );
}

/// Each form lets go at its own limit, and hover's is the lowest.
///
/// Comparing the two forms' slip at one speed does **not** work and was the
/// first thing tried: hover turns at 3.0 rad/s against the tracks' 3.5, so
/// at any speed where both still grip, the tracks show the larger slip
/// simply because they are rotating faster. That measures `turns[1]`, not
/// grip.
///
/// What the grip model actually claims is a *threshold* per form — the speed
/// above which a full-rate turn can no longer be held, `lateral_grip ×
/// grip_force_scale / turns[1]`. So each form is checked against its own,
/// and the thresholds are asserted to differ in the direction the data says.
#[test]
fn each_form_lets_go_above_its_own_grip_limit() {
    // The speed at which a full-rate turn needs exactly the lateral
    // acceleration this form can supply.
    let breakaway = |form: VehicleForm| {
        let table = table();
        let mode = table
            .player_def()
            .and_then(|def| def.mode(form))
            .expect("a mode")
            .clone();

        mode.static_grip * mode.grip_force_scale / mode.turns[1]
    };

    let hover = breakaway(VehicleForm::Hover);
    let track = breakaway(VehicleForm::Track);

    // Hover's weaker `friction[1]` has to show up as an earlier breakaway,
    // which is the whole of "hover drifts".
    assert!(
        hover < track,
        "hover should let go at a lower speed ({hover:.1}) than tracks ({track:.1})"
    );
    // And hover's limit has to sit inside its own speed range, or it never
    // drifts in play however weak its grip is.
    let hover_top = table()
        .player_def()
        .and_then(|def| def.mode(VehicleForm::Hover))
        .expect("hover")
        .max_speed;
    assert!(
        hover < hover_top,
        "hover breaks away at {hover:.1} m/s but only reaches {hover_top:.1}"
    );

    // The same for the tracks: a limit above top speed would mean no skid.
    let track_top = table()
        .player_def()
        .and_then(|def| def.mode(VehicleForm::Track))
        .expect("track")
        .max_speed;
    assert!(
        track < track_top,
        "tracks break away at {track:.1} m/s but only reach {track_top:.1}"
    );

    // The two water forms deliberately do **not** slide, and that is the
    // data's call rather than an oversight: amphib's `friction[1]` is 90 and
    // the sub's is 200, against 80 on the tracks, so both have more grip than
    // the form that skids while being much slower. Their breakaway sits above
    // their own top speed and they hold any turn they can make. A boat
    // carving out from under itself would be the surprising behaviour here.
    for form in [VehicleForm::Amphib, VehicleForm::Sub] {
        let limit = breakaway(form);
        let top = table()
            .player_def()
            .and_then(|def| def.mode(form))
            .expect("a mode")
            .max_speed;

        assert!(
            limit > top,
            "{form:?} should hold every turn it can make, but breaks away at \
             {limit:.1} m/s against a top speed of {top:.1}"
        );
    }
}

/// The angle between where a vehicle points and where it is moving, in
/// radians. Zero means it tracks true; a skid is a non-zero slip.
fn slip_angle(app: &App, entity: Entity) -> f32 {
    let transform = app.world().get::<Transform>(entity).expect("a transform");
    let velocity = app
        .world()
        .get::<LinearVelocity>(entity)
        .expect("a velocity")
        .0;

    // Flat: a slope would show up as slip that is really just gravity.
    let heading = transform
        .forward()
        .as_vec3()
        .with_y(0.0)
        .normalize_or_zero();
    let travel = velocity.with_y(0.0);

    let Some(travel) = travel.try_normalize() else {
        return 0.0;
    };

    heading.angle_between(travel)
}

/// Steering with no throttle turns the vehicle on the spot: `turns[0]` is the
/// stationary rate, and FR-V1 asks for "turns in place when stationary".
#[test]
fn steering_from_a_standstill_turns_in_place() {
    let mut app = vehicle_app();
    spawn_slab(&mut app);
    let player = spawn_player(&mut app, Transform::from_xyz(0.0, 2.0, 0.0));
    step(&mut app, SIM_HZ as u32);

    let before = app
        .world()
        .get::<Transform>(player)
        .expect("a transform")
        .rotation;
    let start = position(&app, player);

    drive_for(
        &mut app,
        player,
        PlayerInput {
            steer: 1.0,
            ..default()
        },
        60,
    );

    let after = app
        .world()
        .get::<Transform>(player)
        .expect("a transform")
        .rotation;
    let turned = before.angle_between(after);

    assert!(
        turned > 0.5,
        "a second of full steer should have turned it well past half a radian, not {turned}"
    );
    // In place: it may drift a little as the suspension settles, but it must
    // not drive off.
    let moved = (position(&app, player) - start).length();
    assert!(moved < 5.0, "it should turn, not drive: moved {moved} m");
}

/// The suspension holds the hull up rather than letting it sink into the
/// slab or bounce off it.
#[test]
fn the_suspension_holds_the_hull_at_its_rest_height() {
    let mut app = vehicle_app();
    spawn_slab(&mut app);
    let player = spawn_player(&mut app, Transform::from_xyz(0.0, 6.0, 0.0));

    // Long enough to fall 6 m at 40 m/s² (0.55 s) and settle.
    step(&mut app, (SIM_HZ * 3.0) as u32);

    let rest = table()
        .player_def()
        .and_then(|def| def.mode(VehicleForm::Track))
        .expect("track")
        .suspension
        .rest;
    let y = position(&app, player).y;

    assert!(
        (y - rest).abs() < 0.6,
        "the hull should sit near its {rest} m rest height, not at {y}"
    );
    assert!(
        speed(&app, player) < 1.0,
        "and it should have stopped bouncing"
    );

    let ground = app
        .world()
        .get::<GroundState>(player)
        .expect("a ground state");
    assert!(ground.grounded, "the rays should have found the slab");
    assert_eq!(ground.soil, Some(Soil::Default));
    assert!(
        ground.normal.dot(Vec3::Y) > 0.99,
        "a flat slab's normal is up, not {:?}",
        ground.normal
    );
}

/// A hard landing must not lose the suspension.
///
/// Rays cast from the `platform` corners, which sit a metre *below* the
/// collider box, would fail here. On a landing hard enough to compress past
/// the travel, the solver stops the box at the terrain while the hull keeps
/// going, putting those origins under the surface — and a downward ray from
/// below the ground finds nothing. The spring would then read as fully
/// extended, `grounded` go false, and the hull sit dead on its collider with
/// no suspension at all.
///
/// Dropped from high enough to bottom the springs out, then checked for
/// that state.
#[test]
fn a_hard_landing_keeps_its_suspension() {
    let mut app = vehicle_app();
    spawn_slab(&mut app);

    // 40 m up: about 1.4 s of falling at the reader's 40 m/s², arriving at
    // roughly 40 m/s, which is far more compression than the 1.2 m travel.
    let player = spawn_player(&mut app, Transform::from_xyz(0.0, 40.0, 0.0));

    // Step through the landing, and fail if the suspension is ever lost
    // while the hull is at or below its resting height — that is exactly
    // the "origin underground" state.
    let rest = table()
        .player_def()
        .and_then(|def| def.mode(VehicleForm::Track))
        .expect("track")
        .suspension
        .rest;

    let mut lost_while_low = 0;
    for _ in 0..(SIM_HZ * 4.0) as u32 {
        app.update();

        let y = position(&app, player).y;
        let grounded = app
            .world()
            .get::<GroundState>(player)
            .expect("a ground state")
            .grounded;

        if y <= rest && !grounded {
            lost_while_low += 1;
        }
    }

    assert_eq!(
        lost_while_low, 0,
        "the rays lost the ground on {lost_while_low} tick(s) while the hull \
         was at or below its {rest} m rest height"
    );

    // And it settles where it should rather than resting on the box.
    let y = position(&app, player).y;
    assert!(
        (y - rest).abs() < 0.6,
        "after a hard landing the hull should settle at its {rest} m rest \
         height, not at {y}"
    );
}

/// The whole cast has to lie inside or above the collider, or the origin can
/// be pushed underground again by some landing harder than the one the test
/// above happens to use.
#[test]
fn the_suspension_rays_start_inside_the_collider() {
    let mode = table()
        .player_def()
        .and_then(|def| def.mode(VehicleForm::Track))
        .expect("track")
        .clone();

    // The rays are at the contact patch, and the cast starts `ray_lift`
    // above it — which has to be at least the collider's floor.
    assert!(
        mode.suspension.ray_lift >= bft_sim::vehicle::HULL_FLOOR_Y,
        "the cast starts {} above the patch but the box floor is at {}",
        mode.suspension.ray_lift,
        bft_sim::vehicle::HULL_FLOOR_Y
    );

    // And lifting it must not have shortened the reach below the hull, or
    // the vehicle would lose contact on ordinary bumps.
    let reach_below_patch = mode.suspension.max_len - mode.suspension.ray_lift;
    assert!(
        reach_below_patch >= mode.suspension.rest,
        "the cast reaches only {reach_below_patch} below the contact patch, \
         less than the {} rest length",
        mode.suspension.rest
    );
}

/// A surface form in a `WaterVolume` settles at
/// the surface.
#[test]
fn an_amphib_settles_at_the_water_surface() {
    let mut app = vehicle_app();
    // No slab: open water, the shallow regime (docs/data/water.md).
    spawn_water(&mut app, 0.0, None);

    let player = spawn_player(&mut app, Transform::from_xyz(0.0, 8.0, 0.0));
    app.world_mut()
        .get_mut::<Vehicle>(player)
        .expect("a vehicle")
        .form = VehicleForm::Amphib;

    step(&mut app, (SIM_HZ * 4.0) as u32);

    let y = position(&app, player).y;
    let mode_alt = table()
        .player_def()
        .and_then(|def| def.mode(VehicleForm::Amphib))
        .expect("amphib")
        .mode_alt;

    assert!(
        (y - mode_alt).abs() < 0.5,
        "an amphib should ride at the surface plus its {mode_alt} m offset, not at {y}"
    );
}

/// The sub cannot leave its volume at either
/// end.
#[test]
fn the_sub_stays_between_the_floor_and_the_surface() {
    let mut app = vehicle_app();
    // A deep pool: surface at 0, floor 24 m down, as m1's zone-14 pair is.
    spawn_water(&mut app, 0.0, Some(-24.0));

    let player = spawn_player(&mut app, Transform::from_xyz(0.0, -10.0, 0.0));
    app.world_mut()
        .get_mut::<Vehicle>(player)
        .expect("a vehicle")
        .form = VehicleForm::Sub;

    // Full climb for four seconds: without the clamp this leaves the water.
    drive_for(
        &mut app,
        player,
        PlayerInput {
            pitch: 1.0,
            ..default()
        },
        (SIM_HZ * 4.0) as u32,
    );

    let top = position(&app, player).y;
    assert!(
        top < 0.0,
        "the sub must not surface through the water: it reached {top}"
    );

    // And full dive for four seconds must not go through the floor.
    drive_for(
        &mut app,
        player,
        PlayerInput {
            pitch: -1.0,
            ..default()
        },
        (SIM_HZ * 6.0) as u32,
    );

    let bottom = position(&app, player).y;
    assert!(
        bottom > -24.0,
        "the sub must not sink through the floor at -24: it reached {bottom}"
    );
}

/// Spawn a water volume the way the water pass does, with a footprint big
/// enough to hold the test vehicle.
fn spawn_water(app: &mut App, surface_y: f32, floor_y: Option<f32>) -> Entity {
    let footprint = vec![
        [
            Vec2::new(-100.0, -100.0),
            Vec2::new(-100.0, 100.0),
            Vec2::new(100.0, 100.0),
        ],
        [
            Vec2::new(-100.0, -100.0),
            Vec2::new(100.0, 100.0),
            Vec2::new(100.0, -100.0),
        ],
    ];

    app.world_mut()
        .spawn((
            WaterVolume {
                surface_y,
                floor_y,
                footprint,
                kind: LiquidKind::Water,
            },
            Transform::IDENTITY,
        ))
        .id()
}

/// Entering water in Track form with Amphib unlocked switches automatically,
/// and leaving it switches back — "transformations function as barriers".
#[test]
fn entering_water_switches_a_tank_to_amphib_and_leaving_switches_back() {
    let mut app = vehicle_app();
    spawn_slab(&mut app);
    spawn_water(&mut app, 0.0, None);

    let player = spawn_player(&mut app, Transform::from_xyz(0.0, 2.0, 0.0));

    // The hull origin starts above the surface, so nothing has happened yet.
    step(&mut app, 2);
    assert_eq!(form(&app, player), VehicleForm::Track);

    // Put it under the surface: `water_at` answers for a position at or below
    // the surface height.
    place(&mut app, player, Vec3::new(0.0, -1.0, 0.0));
    step(&mut app, 2);
    assert_eq!(
        form(&app, player),
        VehicleForm::Amphib,
        "entering water with amphib unlocked should transform"
    );

    // Out of the water and back on the slab, within the tracks' reach: the
    // hull rides about 0.2 m up on its 0.4 m of travel
    // (`SUSPENSION_TRAVEL`).
    place(&mut app, player, Vec3::new(0.0, 0.3, 0.0));
    step(&mut app, 4);
    assert_eq!(
        form(&app, player),
        VehicleForm::Track,
        "leaving the water should transform back"
    );
}

/// The sub is reachable only from amphib, and only over deep water.
#[test]
fn the_sub_is_only_reachable_from_amphib_over_deep_water() {
    let mut app = vehicle_app();
    spawn_water(&mut app, 0.0, Some(-24.0));
    let player = spawn_player(&mut app, Transform::from_xyz(0.0, -5.0, 0.0));

    // From Track, `next_form` must not reach Sub: there is no track↔sub pair.
    set_form(&mut app, player, VehicleForm::Track);
    drive_for(
        &mut app,
        player,
        PlayerInput {
            next_form: true,
            ..default()
        },
        1,
    );
    app.update();
    assert_ne!(form(&app, player), VehicleForm::Sub);

    // From Amphib over a divable volume it is.
    set_form(&mut app, player, VehicleForm::Amphib);
    let mut seen_sub = false;
    for _ in 0..4 {
        drive_for(
            &mut app,
            player,
            PlayerInput {
                next_form: true,
                ..default()
            },
            1,
        );
        if form(&app, player) == VehicleForm::Sub {
            seen_sub = true;
            break;
        }
    }
    assert!(
        seen_sub,
        "cycling from amphib over deep water should reach the sub"
    );
}

/// Shallow water has no floor, so there is nothing to dive into
/// (docs/data/water.md).
#[test]
fn the_sub_is_not_reachable_over_shallow_water() {
    let mut app = vehicle_app();
    spawn_water(&mut app, 0.0, None);
    let player = spawn_player(&mut app, Transform::from_xyz(0.0, -0.5, 0.0));
    set_form(&mut app, player, VehicleForm::Amphib);

    for _ in 0..8 {
        drive_for(
            &mut app,
            player,
            PlayerInput {
                next_form: true,
                ..default()
            },
            1,
        );
        assert_ne!(
            form(&app, player),
            VehicleForm::Sub,
            "the shallow sea has no floor to dive to"
        );
    }
}

/// Turret pitch is clamped by the mode's `gun_pitch` where it states one.
#[test]
fn the_turret_pitch_is_clamped_by_the_modes_limits() {
    let mut app = vehicle_app();
    spawn_water(&mut app, 0.0, Some(-24.0));
    let player = spawn_player(&mut app, Transform::from_xyz(0.0, -5.0, 0.0));
    set_form(&mut app, player, VehicleForm::Sub);

    // Far more aim than the limits allow, over many ticks.
    drive_for(
        &mut app,
        player,
        PlayerInput {
            turret_delta: Vec2::new(0.0, 1.0),
            ..default()
        },
        60,
    );

    let turret = *app.world().get::<Turret>(player).expect("a turret");
    assert!(
        (turret.pitch - 0.5).abs() < 1e-4,
        "the sub's upper gun_pitch limit is 0.5 rad, not {}",
        turret.pitch
    );

    drive_for(
        &mut app,
        player,
        PlayerInput {
            turret_delta: Vec2::new(0.0, -1.0),
            ..default()
        },
        60,
    );

    let turret = *app.world().get::<Turret>(player).expect("a turret");
    assert!(
        (turret.pitch + 0.1736).abs() < 1e-4,
        "the sub's lower limit is -0.1736 rad, not {}",
        turret.pitch
    );
}

/// Turret yaw is not clamped — the turret turns all the way round — but it
/// must stay in a range that does not lose precision.
#[test]
fn the_turret_yaw_wraps_rather_than_growing() {
    let mut app = vehicle_app();
    spawn_slab(&mut app);
    let player = spawn_player(&mut app, Transform::from_xyz(0.0, 2.0, 0.0));

    drive_for(
        &mut app,
        player,
        PlayerInput {
            turret_delta: Vec2::new(0.5, 0.0),
            ..default()
        },
        240,
    );

    let turret = *app.world().get::<Turret>(player).expect("a turret");
    assert!(
        turret.yaw.abs() <= std::f32::consts::PI + 1e-5,
        "the yaw should have wrapped, not grown to {}",
        turret.yaw
    );
}

/// A vehicle in water falls at `wat_gravity`, not `nom_gravity`: the reader
/// keys all three gravities off one global (READER-SCHEMA §2.2).
///
/// Amphib is deliberately **not** unlocked: with it unlocked the tank
/// transforms on entering the water and floats instead, which is the other
/// test's subject.
#[test]
fn a_tank_in_water_falls_at_the_water_gravity() {
    let mut app = vehicle_app();
    spawn_water(&mut app, 0.0, Some(-40.0));
    let player = spawn_player_with(
        &mut app,
        Transform::from_xyz(0.0, -2.0, 0.0),
        FormSet::track_only(),
    );
    set_form(&mut app, player, VehicleForm::Track);

    step(&mut app, 4);

    let scale = app
        .world()
        .get::<GravityScale>(player)
        .expect("a gravity scale")
        .0;
    let tuning = app.world().resource::<bft_sim::physics::PhysicsTuning>();

    assert!(
        (scale - tuning.water_gravity / tuning.gravity).abs() < 1e-5,
        "a tank in water should fall at 9/40 of the global gravity, not {scale}"
    );
}

/// The player's spawn faces the way `aiv.zrd` says, which is the whole point
/// of reading the record rather than the node.
#[test]
fn the_spawn_keeps_the_heading_it_was_given() {
    let mut app = vehicle_app();
    spawn_slab(&mut app);

    let heading = Quat::from_rotation_y(166.5f32.to_radians());
    let player = spawn_player(
        &mut app,
        Transform {
            translation: Vec3::new(0.0, 4.0, 0.0),
            rotation: heading,
            ..default()
        },
    );
    step(&mut app, 2);

    let actual = app
        .world()
        .get::<Transform>(player)
        .expect("a transform")
        .rotation;

    assert!(
        actual.angle_between(heading) < 0.05,
        "the vehicle should still face its spawn heading"
    );
}

/// The spawn carries everything the controller needs, so a later
/// system does not have to remember to insert a component.
#[test]
fn the_spawn_carries_the_whole_controller_contract() {
    let mut app = vehicle_app();
    let player = spawn_player(&mut app, Transform::from_xyz(0.0, 4.0, 0.0));
    app.update();

    let world = app.world();
    assert!(world.get::<Vehicle>(player).is_some());
    assert!(world.get::<PlayerVehicle>(player).is_some());
    assert!(world.get::<PlayerInput>(player).is_some());
    assert!(world.get::<Turret>(player).is_some());
    assert!(world.get::<GroundState>(player).is_some());
    assert!(world.get::<Collider>(player).is_some());
    assert!(world.get::<RigidBody>(player).is_some());
    // A turret child the camera and the player model can hang off.
    assert_eq!(
        world.get::<Children>(player).map(|kids| kids.len()),
        Some(1)
    );
}

fn form(app: &App, entity: Entity) -> VehicleForm {
    app.world().get::<Vehicle>(entity).expect("a vehicle").form
}

fn set_form(app: &mut App, entity: Entity, form: VehicleForm) {
    app.world_mut()
        .get_mut::<Vehicle>(entity)
        .expect("a vehicle")
        .form = form;
}

/// Teleport a body, which needs avian's `Position` as well as the transform:
/// the solver reads `Position` and writes `Transform` back from it.
fn place(app: &mut App, entity: Entity, to: Vec3) {
    if let Some(mut position) = app.world_mut().get_mut::<Position>(entity) {
        position.0 = to;
    }
    if let Some(mut transform) = app.world_mut().get_mut::<Transform>(entity) {
        transform.translation = to;
    }
    if let Some(mut velocity) = app.world_mut().get_mut::<LinearVelocity>(entity) {
        velocity.0 = Vec3::ZERO;
    }
}

/// A vehicle's current traction state.
fn traction(app: &App, entity: Entity) -> Traction {
    *app.world()
        .get::<Traction>(entity)
        .expect("the vehicle has a traction state")
}

// ---------------------------------------------------------------------------
// Staying upright
// ---------------------------------------------------------------------------

/// How far a hull leans from world up, in radians.
fn tilt(app: &App, entity: Entity) -> f32 {
    let rotation = app
        .world()
        .get::<Transform>(entity)
        .expect("a transform")
        .rotation;
    (rotation * Vec3::Y).y.clamp(-1.0, 1.0).acos()
}

/// The track mode's tilt limit, as the loader derives it.
fn max_tilt() -> f32 {
    mode(VehicleForm::Track).max_tilt
}

/// The slab, turned about x so it climbs toward −z at `angle`.
fn spawn_slope(app: &mut App, angle: f32) -> Entity {
    let slab = spawn_slab(app);
    app.world_mut()
        .get_mut::<Transform>(slab)
        .expect("the slab has a transform")
        .rotation = Quat::from_rotation_x(angle);
    slab
}

/// Dropped on its back, a hull lands on its tracks: a tank on its roof
/// cannot drive, and the original never leaves one there.
#[test]
fn a_hull_dropped_upside_down_lands_on_its_tracks() {
    let mut app = vehicle_app();
    spawn_slab(&mut app);
    let player = spawn_player(
        &mut app,
        Transform::from_xyz(0.0, 12.0, 0.0).with_rotation(Quat::from_rotation_z(3.0)),
    );

    step(&mut app, (SIM_HZ * 3.0) as u32);

    let tilt = tilt(&app, player);
    assert!(tilt < 0.1, "the hull should be level, not {tilt} rad over");
    let ground = app
        .world()
        .get::<GroundState>(player)
        .expect("a ground state");
    assert!(ground.grounded, "and on its suspension");
}

/// A hull that leaves the ground leaning comes level in the air, as the
/// original's tank does off a ramp.
#[test]
fn a_leaning_hull_levels_out_in_the_air() {
    let mut app = vehicle_app();
    // No ground at all: whatever levels it is the controller.
    let player = spawn_player(
        &mut app,
        Transform::from_xyz(0.0, 500.0, 0.0).with_rotation(Quat::from_rotation_x(0.7)),
    );

    step(&mut app, (SIM_HZ * 1.5) as u32);

    let tilt = tilt(&app, player);
    assert!(
        tilt < 0.05,
        "the hull should be level after 1.5 s, not {tilt} rad"
    );
}

/// However hard it is spun, a hull never leans past its limit.
///
/// The spin is far more than any spring answers within a tick — the limit
/// after the solver is what has to hold it.
#[test]
fn a_violent_roll_never_passes_the_tilt_limit() {
    let mut app = vehicle_app();
    spawn_slab(&mut app);
    let player = spawn_player(&mut app, Transform::from_xyz(0.0, 3.0, 0.0));
    step(&mut app, (SIM_HZ * 1.0) as u32);

    app.world_mut()
        .get_mut::<AngularVelocity>(player)
        .expect("an angular velocity")
        .0 = Vec3::new(0.0, 0.0, 40.0);

    let limit = max_tilt();
    let mut worst = 0.0f32;
    for _ in 0..(SIM_HZ * 2.0) as u32 {
        app.update();
        worst = worst.max(tilt(&app, player));
    }

    assert!(
        worst <= limit + 1e-3,
        "the hull leaned {worst} rad, past its {limit} rad limit"
    );
    let tilt = tilt(&app, player);
    assert!(
        tilt < 0.1,
        "and should be back on its tracks, not {tilt} rad over"
    );
}

/// On a slope, level is the slope: the righting follows the ground's normal
/// rather than dragging the hull off it toward world up, and the limit sits
/// above the steepest slopes the levels ask the player to climb.
#[test]
fn a_hull_on_a_steep_slope_lies_along_it() {
    let slope = 40f32.to_radians();
    assert!(
        slope < max_tilt(),
        "the limit has to allow the slope at all"
    );

    let mut app = vehicle_app();
    spawn_slope(&mut app, slope);
    let player = spawn_player(
        &mut app,
        Transform::from_xyz(0.0, 4.0, 0.0).with_rotation(Quat::from_rotation_x(slope)),
    );

    step(&mut app, (SIM_HZ * 0.75) as u32);

    let ground = *app
        .world()
        .get::<GroundState>(player)
        .expect("a ground state");
    assert!(ground.grounded, "the hull should be on the slope");
    let tilt = tilt(&app, player);
    assert!(
        (tilt - slope).abs() < 0.1,
        "and lie along it at {slope} rad, not {tilt}"
    );
}

/// The centre of mass is down in the tracks, not the collider's.
#[test]
fn the_centre_of_mass_is_in_the_tracks() {
    let mut app = vehicle_app();
    let player = spawn_player(&mut app, Transform::from_xyz(0.0, 4.0, 0.0));
    app.update();

    let com = app
        .world()
        .get::<ComputedCenterOfMass>(player)
        .expect("a computed centre of mass")
        .0;
    assert!(
        (com.y - bft_sim::vehicle::CENTER_OF_MASS_Y).abs() < 1e-4,
        "the centre of mass should be at the tracks' middle, not {com:?}"
    );
}

// ---------------------------------------------------------------------------
// Ledges, ramps and craters
// ---------------------------------------------------------------------------

/// A step across the whole width of the way, `height` tall, from 30 m ahead
/// (the hull faces −z) to 80 m.
fn spawn_ledge(app: &mut App, height: f32) {
    app.world_mut().spawn((
        Collider::cuboid(100.0, height, 50.0),
        RigidBody::Static,
        layers::level(),
        Transform::from_xyz(0.0, height / 2.0, -55.0),
    ));
}

/// The slab with a mortar crater cut into it at `center`: the HE mortar's
/// 10 m radius, `declient.zrd`'s 1.7 m depth and 15° walls, six sides — the
/// same numbers and the same cut the game uses.
fn spawn_cratered_slab(app: &mut App, center: Vec3) {
    let model = slab(0);
    let axes = bft_assets::axes::AxisConvention::GAMEZ_DEFAULT;
    let triangulated = bft_assets::geometry::triangulate(&model, &axes);
    let (radius, depth, slope) = (10.0, 1.7, 15f32.to_radians());
    let crater = bft_sim::crater::Crater {
        center,
        radius,
        floor_radius: radius - depth / slope.tan(),
        depth,
        sides: 6,
        texture: None,
        uv_scale: Vec2::ONE,
    };
    let groups = vec![None; triangulated.groups.len()];
    let cut = bft_sim::crater::cut_crater(
        &triangulated,
        &groups,
        &bevy::math::Affine3A::IDENTITY,
        &crater,
        0,
    )
    .expect("the slab takes a crater");
    let collider = bft_sim::colliders::collider_from(ModelType::Default, &cut.triangulated)
        .expect("the cut slab has a surface");

    app.world_mut().spawn((
        collider,
        RigidBody::Static,
        layers::level(),
        Transform::IDENTITY,
    ));
}

/// Full throttle at a step for four seconds; where the hull got to.
fn charge_at_ledge(height: f32) -> Vec3 {
    let mut app = vehicle_app();
    spawn_slab(&mut app);
    spawn_ledge(&mut app, height);
    let player = spawn_player(&mut app, Transform::from_xyz(0.0, 1.0, 0.0));
    step(&mut app, SIM_HZ as u32);

    let throttle = PlayerInput {
        throttle: 1.0,
        ..Default::default()
    };
    // Four seconds reaches top speed, and 48 m/s would carry a hull that
    // got over the step well past its far end at z = −80.
    let mut furthest = Vec3::ZERO;
    for _ in 0..(SIM_HZ * 4.0) as u32 {
        drive_for(&mut app, player, throttle, 1);
        let at = position(&app, player);
        if at.z < furthest.z {
            furthest = at;
        }
    }
    furthest
}

/// The tracks' rounded nose rides up a kerb.
#[test]
fn a_low_step_is_driven_over() {
    let furthest = charge_at_ledge(0.3);
    assert!(
        furthest.z < -80.0,
        "a 0.3 m step should be driven over and off its far end at z = −80, \
         but the hull got to {furthest}"
    );
}

/// A ledge the height of the tracks stops the tank, even at full speed.
///
/// The original's tank runs into low ledges that ours used to walk up: the
/// suspension rays found the ledge's top and the springs lifted the hull
/// onto it, because nothing solid reached below the hull floor. Some places
/// in the levels are meant to be reached only by blowing a crater and
/// jumping from it.
#[test]
fn a_ledge_as_high_as_the_tracks_stops_the_tank() {
    let furthest = charge_at_ledge(0.8);
    assert!(
        furthest.z > -31.0,
        "a 0.8 m ledge 30 m ahead should stop the hull, but it got to {furthest}"
    );
}

/// A mortar crater is a ramp: driven through at speed, its far wall throws
/// the tank into the air.
///
/// The hull used to hit the 15° far wall with the square front of its
/// collider and stop dead, then crawl out.
#[test]
fn a_crater_driven_through_at_speed_launches_the_tank() {
    let mut app = vehicle_app();
    spawn_cratered_slab(&mut app, Vec3::ZERO);
    let player = spawn_player(&mut app, Transform::from_xyz(0.0, 1.0, 60.0));
    step(&mut app, SIM_HZ as u32);

    let throttle = PlayerInput {
        throttle: 1.0,
        ..Default::default()
    };
    let (mut highest, mut airborne) = (f32::MIN, 0u32);
    for _ in 0..(SIM_HZ * 3.0) as u32 {
        drive_for(&mut app, player, throttle, 1);
        let at = position(&app, player);
        // Past the far rim, over the flat.
        if at.z < -10.0 && at.z > -60.0 {
            highest = highest.max(at.y);
            let ground = app
                .world()
                .get::<GroundState>(player)
                .expect("a ground state");
            airborne += u32::from(!ground.grounded);
        }
    }

    assert!(
        highest > 1.2,
        "the hull should clear the rim by a metre or more, but peaked at {highest}"
    );
    assert!(
        f64::from(airborne) > SIM_HZ * 0.3,
        "and fly for a good part of a second, not {airborne} ticks"
    );
}

/// Climbing out of flat ground onto a steep slope does not catch the nose
/// in the corner.
#[test]
fn a_steep_slope_is_climbed_from_the_flat() {
    let slope = 30f32.to_radians();
    let mut app = vehicle_app();
    spawn_slab(&mut app);
    // The same plane through the origin, rising toward −z: flat ground
    // behind, the slope from z = 0 on.
    spawn_slope(&mut app, slope);
    let player = spawn_player(&mut app, Transform::from_xyz(0.0, 1.0, 40.0));
    step(&mut app, SIM_HZ as u32);

    let throttle = PlayerInput {
        throttle: 1.0,
        ..Default::default()
    };
    drive_for(&mut app, player, throttle, (SIM_HZ * 4.0) as u32);

    let at = position(&app, player);
    assert!(
        at.z < -20.0 && at.y > 20.0 * slope.tan() - 1.0,
        "the hull should be well up the slope, not at {at}"
    );
}
