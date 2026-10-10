//! The AI's performance budget: "AI tick cost < 0.5 ms with 50 enemies".
//!
//! A test rather than a benchmark because the budget should fail CI when it
//! regresses, and because what matters
//! is the shape of the cost — 50 brains casting line-of-sight rays into a
//! real BVH — rather than a precise nanosecond count. The budget is generous
//! against the measured figure for that reason: a loaded CI runner is several
//! times slower than a laptop and the check is for an order-of-magnitude
//! regression, not for jitter.
//!
//! What is measured is `SimSet::Ai` alone: the whole `FixedUpdate` is stepped
//! and two stamp systems, first and last in the AI's own set, time the brains
//! and nothing else.

use avian3d::prelude::*;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bft_assets::materials::{MaterialDesc, Soil};
use bft_assets::models::{ModelDesc, ModelType, PolygonDesc, PolygonMaterialDesc};
use bft_sim::ai::{AiBrain, AiState, AttackStrategy, SteeringTuning};
use bft_sim::damage::{Health, Team};
use bft_sim::level::surface_info;
use bft_sim::physics::layers;
use bft_sim::schedule::{SIM_HZ, SimSet};
use bft_sim::vehicle::tuning::{
    ModeSounds, ModeTuning, PlayerConstants, SuspensionTuning, VehicleAiDefaults, VehicleDef,
    VehicleKindId, VehicleTable,
};
use bft_sim::vehicle::{Controlled, PlayerInput, Turret, Vehicle, VehicleForm};

/// How many enemies the criterion names.
const ENEMIES: usize = 50;

/// The budget, in microseconds per tick.
const BUDGET_US: u128 = 500;

/// How many ticks to average over. Enough to cross the perception cadence
/// several times, so the ray-casting ticks are represented.
const TICKS: u32 = 240;

fn app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bft_sim::SimPlugin);
    app.insert_resource(TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / SIM_HZ),
    ));
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.insert_resource(table());
    app.insert_resource(SteeringTuning::default());
    app.finish();
    app.update();

    app
}

fn mode() -> ModeTuning {
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

fn table() -> VehicleTable {
    let mut kinds = HashMap::default();
    kinds.insert(
        VehicleKindId("scou".to_string()),
        VehicleDef {
            name: "Scout".to_string(),
            health: 6.0,
            shield: None,
            modes: [(VehicleForm::Track, mode())].into_iter().collect(),
            camback: [Vec3::ZERO; 3],
            aim_y: [3.0, 3.0],
            turret_rate: None,
            weapons: Vec::new(),
            ai: VehicleAiDefaults {
                activation: Some(300.0),
                pursuit_range: Some([60.0, 90.0]),
                not_pursuit_dwell: Some(0.2),
                return_range: Some(175.0),
            },
            drops: Vec::new(),
            sounds: Default::default(),
            track_switch: None,
        },
    );

    VehicleTable {
        kinds,
        player: PlayerConstants::default(),
    }
}

/// A big slab plus a scatter of blocks, so the line-of-sight rays descend a
/// BVH with something in it rather than hitting an empty world.
fn spawn_world(app: &mut App) {
    let model = ModelDesc {
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
    };
    let axes = bft_assets::axes::AxisConvention::GAMEZ_DEFAULT;
    let triangulated = bft_assets::geometry::triangulate(&model, &axes);
    let materials = vec![MaterialDesc::Colored {
        color: [1.0, 1.0, 1.0],
        alpha: 255,
        soil: Soil::Default,
    }];

    app.world_mut().spawn((
        bft_sim::colliders::collider_from(ModelType::Default, &triangulated).expect("a surface"),
        surface_info(ModelType::Default, &triangulated, &materials).expect("soil"),
        RigidBody::Static,
        layers::level(),
        Transform::IDENTITY,
    ));

    // 64 blocks, so a ray has a real chance of being occluded and the
    // broad-phase has work to do.
    for i in 0..64 {
        let angle = i as f32 * 0.3;
        let radius = 40.0 + (i % 8) as f32 * 30.0;
        app.world_mut().spawn((
            Collider::cuboid(12.0, 20.0, 12.0),
            RigidBody::Static,
            layers::level(),
            Transform::from_xyz(angle.cos() * radius, 10.0, angle.sin() * radius),
        ));
    }
}

/// One brain, engaging, so it perceives, thinks, steers and aims every tick.
///
/// Spawned directly rather than through `spawn_enemy` so the cost measured is
/// the AI's and not the template cloning's, which happens once at load.
fn spawn_brains(app: &mut App, count: usize, target: Entity) {
    for index in 0..count {
        let angle = index as f32 * 0.7;
        let radius = 80.0 + (index % 10) as f32 * 12.0;
        let at = Vec3::new(angle.cos() * radius, 2.0, angle.sin() * radius);

        app.world_mut().spawn((
            Name::new(format!("scou_{index:02}")),
            bft_sim::ai::Enemy {
                instance: format!("scou_{index:02}"),
                kind: VehicleKindId("scou".to_string()),
            },
            Vehicle {
                kind: VehicleKindId("scou".to_string()),
                form: VehicleForm::Track,
                unlocked: bft_sim::vehicle::FormSet::of([VehicleForm::Track]),
            },
            AiBrain {
                net: None,
                // Engaging is the expensive state: it perceives, steers and
                // aims, where a dormant brain does almost nothing.
                state: AiState::Engage,
                target: Some(target),
                timer: 0.0,
                waypoint: None,
                forward: true,
                home: at,
                strategy: Some(AttackStrategy::HeadOn),
                defaults: VehicleAiDefaults {
                    activation: Some(300.0),
                    pursuit_range: Some([60.0, 90.0]),
                    not_pursuit_dwell: Some(0.2),
                    return_range: Some(175.0),
                },
                los_lost: 0,
                since_seen: 0.0,
                // Phased, as the real spawner phases them.
                phase: index as u32,
                steering: SteeringTuning::default(),
                weapon_range: 325.0,
            },
            Controlled::Ai,
            PlayerInput::default(),
            Turret::default(),
            bft_sim::vehicle::GroundState::airborne(),
            bft_sim::vehicle::Traction::default(),
            Transform::from_translation(at),
            (
                Team::ENEMY,
                Health::new(6.0),
                RigidBody::Dynamic,
                Collider::cuboid(5.7, 2.8, 7.4),
                Mass(2200.0),
                GravityScale(1.0),
                layers::vehicle(),
            ),
        ));
    }
}

/// Something for them all to hunt.
fn spawn_target(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            Team::PLAYER,
            Health::new(140.0),
            Transform::from_xyz(0.0, 2.0, 0.0),
            RigidBody::Dynamic,
            Collider::cuboid(5.7, 2.8, 7.4),
            Mass(2400.0),
            GravityScale(1.0),
            layers::vehicle(),
        ))
        .id()
}

/// When the current tick's `SimSet::Ai` started, and the total spent in it.
#[derive(Resource, Default)]
struct AiClock {
    started: Option<std::time::Instant>,
    spent: std::time::Duration,
}

fn ai_started(mut clock: ResMut<AiClock>) {
    clock.started = Some(std::time::Instant::now());
}

fn ai_finished(mut clock: ResMut<AiClock>) {
    if let Some(started) = clock.started.take() {
        clock.spent += started.elapsed();
    }
}

/// Bracket the AI's own chain with two stamps, first and last in its set.
///
/// `SimSet` is chained, so nothing from another set runs between them: what
/// is timed is the brains, and not the physics and vehicle controller that
/// 50 extra bodies also cost.
fn clock_the_ai(app: &mut App) {
    app.init_resource::<AiClock>().add_systems(
        FixedUpdate,
        (
            ai_started
                .in_set(SimSet::Ai)
                .before(bft_sim::ai::brain::tick_ai),
            ai_finished
                .in_set(SimSet::Ai)
                .after(bft_sim::ai::brain::drive_turrets),
        ),
    );
}

/// Average microseconds per tick spent in `SimSet::Ai` over [`TICKS`], after
/// a warm-up.
fn ai_cost_per_tick(app: &mut App) -> u128 {
    // Warm up: the first ticks build the BVH and touch every system's caches.
    for _ in 0..60 {
        app.update();
    }

    app.world_mut().resource_mut::<AiClock>().spent = std::time::Duration::ZERO;
    for _ in 0..TICKS {
        app.update();
    }

    app.world().resource::<AiClock>().spent.as_micros() / TICKS as u128
}

/// The budget's figure: 50 engaging brains, timed inside their own set.
///
/// Not measured by difference — the same world stepped with and without the
/// brains — because the world without them also has no *vehicles*, so the
/// difference would carry avian and the vehicle controller for 50 extra
/// dynamic bodies. That is not the AI's to account for, and it would be most
/// of the figure, leaving the test near the budget on a busy machine.
#[test]
fn fifty_engaging_enemies_cost_under_half_a_millisecond_a_tick() {
    let mut app = app();
    clock_the_ai(&mut app);
    spawn_world(&mut app);
    let target = spawn_target(&mut app);
    spawn_brains(&mut app, ENEMIES, target);

    let ai_cost = ai_cost_per_tick(&mut app);

    println!("{ENEMIES} engaging enemies: the AI costs about {ai_cost} µs a tick");

    assert!(
        ai_cost < BUDGET_US,
        "the AI costs {ai_cost} µs a tick with {ENEMIES} enemies, over the \
         {BUDGET_US} µs budget"
    );
}
/// The perception cadence is what keeps that figure down, so it is worth
/// pinning that it is actually spreading the work.
///
/// With 50 brains on a 6-tick cadence, at most 9 should perceive on any one
/// tick. A regression that made every brain perceive every tick would still
/// pass the budget on a fast machine and would not pass this.
#[test]
fn the_perception_cadence_spreads_the_work_across_ticks() {
    let tuning = SteeringTuning::default();
    let cadence = tuning.reaction_ticks as usize;

    for tick in 0..cadence as u64 {
        let perceiving = (0..ENEMIES)
            .filter(|phase| tuning.perceives(tick, *phase as u32))
            .count();

        let most = ENEMIES.div_ceil(cadence);
        assert!(
            perceiving <= most,
            "tick {tick}: {perceiving} of {ENEMIES} brains perceive, over the \
             {most} a {cadence}-tick cadence allows"
        );
    }

    // And every brain does get a turn within one cadence, or some would
    // never see anything.
    for phase in 0..ENEMIES {
        let turns = (0..cadence as u64)
            .filter(|tick| tuning.perceives(*tick, phase as u32))
            .count();
        assert_eq!(turns, 1, "phase {phase} perceives {turns} times a cadence");
    }
}
