//! The physics' headless tests: a body falls, lands and stops, and a
//! ground probe names the soil under it — with no renderer anywhere.
//!
//! These need a whole `App` with avian's schedule running, which is what
//! makes them integration tests rather than unit tests next to the code.
//! `MinimalPlugins` is the point: if any of this needed `bevy_render`, NFR-4
//! would already be broken and this file would not link.

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::time::TimeUpdateStrategy;
use bft_assets::materials::{MaterialDesc, Soil};
use bft_assets::models::{ModelDesc, ModelType, PolygonDesc, PolygonMaterialDesc};
use bft_sim::level::surface_info;
use bft_sim::physics::{
    DebugFlags, GroundProbe, PhysicsTuning, SimRng, SimSeed, SurfaceQuery, layers, probe_ground,
};
use bft_sim::schedule::SIM_HZ;

/// An app with the sim's schedule and avian, and nothing that draws.
fn physics_app() -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bft_sim::SimPlugin);
    // One fixed tick per `App::update`, deterministically. See `step`.
    app.insert_resource(TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / SIM_HZ),
    ));
    // `SimSet` only runs while playing; a test that drives
    // `FixedUpdate` directly has to say so.
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.finish();
    app.update();

    app
}

/// Step `FixedUpdate` `ticks` times, one tick per `App::update`.
///
/// `TimeUpdateStrategy::ManualDuration` rather than nudging `Time<Virtual>`:
/// `TimePlugin` recomputes virtual time from real time every update, so a
/// hand-set virtual delta is discarded before `FixedUpdate` ever sees it and
/// the schedule silently never runs.
fn step(app: &mut App, ticks: u32) {
    for _ in 0..ticks {
        app.update();
    }
}

/// A 20 m square slab of one material, centred on the origin at y = 0.
fn slab(soil_material: u32) -> ModelDesc {
    ModelDesc {
        vertices: vec![
            Vec3::new(-10.0, 0.0, -10.0),
            Vec3::new(-10.0, 0.0, 10.0),
            Vec3::new(10.0, 0.0, 10.0),
            Vec3::new(10.0, 0.0, -10.0),
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
    vec![
        MaterialDesc::Colored {
            color: [1.0, 1.0, 1.0],
            alpha: 255,
            soil: Soil::Default,
        },
        MaterialDesc::Colored {
            color: [0.0, 0.0, 1.0],
            alpha: 255,
            soil: Soil::Lava,
        },
    ]
}

/// Spawn a static slab collider with its soil, as `spawn_level` does.
fn spawn_slab(app: &mut App, soil_material: u32) -> Entity {
    let model = slab(soil_material);
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

/// A dropped body lands on the terrain and stays there.
#[test]
fn a_falling_sphere_lands_on_a_static_collider_and_stops() {
    let mut app = physics_app();
    spawn_slab(&mut app, 0);

    let sphere = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            layers::vehicle(),
            Transform::from_xyz(0.0, 8.0, 0.0),
        ))
        .id();

    // 8 m at 40 m/s² is 0.63 s, so a second of ticks is landing plus
    // settling, with no dependence on how fast the solver converges.
    step(&mut app, (SIM_HZ * 1.5) as u32);

    let transform = app.world().get::<Transform>(sphere).expect("a transform");
    let velocity = app
        .world()
        .get::<LinearVelocity>(sphere)
        .expect("a velocity");

    assert!(
        (transform.translation.y - 0.5).abs() < 0.1,
        "the sphere should rest on the slab at its own radius, not at {}",
        transform.translation.y
    );
    assert!(
        velocity.0.length() < 0.5,
        "it should have stopped, not still be moving at {:?}",
        velocity.0
    );
}

/// Gravity is the reader's, not avian's: a body must fall at 40 m/s².
#[test]
fn gravity_is_the_tuning_resource_not_avians_default() {
    let app = physics_app();

    let gravity = app.world().resource::<Gravity>().0;
    let tuning = *app.world().resource::<PhysicsTuning>();

    assert_eq!(gravity, Vec3::NEG_Y * tuning.gravity);
    assert_eq!(gravity.y, -40.0);
}

/// A paused sim must not step the solver: `SimSet::Physics` carries the run
/// condition and avian's sets are nested inside it.
#[test]
fn pausing_the_sim_stops_the_physics_step() {
    let mut app = physics_app();
    spawn_slab(&mut app, 0);

    let sphere = app
        .world_mut()
        .spawn((
            RigidBody::Dynamic,
            Collider::sphere(0.5),
            layers::vehicle(),
            Transform::from_xyz(0.0, 50.0, 0.0),
        ))
        .id();

    app.world_mut()
        .insert_resource(bft_sim::states::Paused(true));
    step(&mut app, 30);

    let height = app
        .world()
        .get::<Transform>(sphere)
        .expect("a transform")
        .translation
        .y;

    assert_eq!(height, 50.0, "a paused sim should not have moved it");
}

/// A probe names the triangle's soil, through avian's ray cast and
/// parry's feature id.
#[test]
fn the_ground_probe_reports_the_soil_of_the_triangle_it_hit() {
    let mut app = physics_app();
    // Material 1 is `Soil::Lava`, so a correct lookup cannot be the default.
    let slab = spawn_slab(&mut app, 1);

    // One tick so avian builds its spatial index.
    step(&mut app, 1);

    let hit = app
        .world_mut()
        .run_system_cached(|spatial: SpatialQuery, surfaces: SurfaceQuery| {
            probe_ground(
                &spatial,
                &surfaces,
                GroundProbe::down(Vec3::new(1.0, 5.0, 2.0), 20.0),
            )
        })
        .expect("the system runs")
        .expect("the probe should hit the slab");

    assert_eq!(hit.entity, slab);
    assert_eq!(hit.soil, Soil::Lava, "the slab's material is lava");
    assert!((hit.distance - 5.0).abs() < 1e-3, "5 m down to y = 0");
    assert!((hit.point - Vec3::new(1.0, 0.0, 2.0)).length() < 1e-3);
    assert!(hit.normal.dot(Vec3::Y) > 0.9, "a floor faces up");
}

/// The probe must miss when there is nothing within range, rather than
/// reporting the nearest thing at any distance.
#[test]
fn the_ground_probe_misses_when_the_ground_is_out_of_range() {
    let mut app = physics_app();
    spawn_slab(&mut app, 0);
    step(&mut app, 1);

    let hit = app
        .world_mut()
        .run_system_cached(|spatial: SpatialQuery, surfaces: SurfaceQuery| {
            probe_ground(
                &spatial,
                &surfaces,
                GroundProbe::down(Vec3::new(0.0, 100.0, 0.0), 10.0),
            )
        })
        .expect("the system runs");

    assert!(hit.is_none());
}

/// Water is a sensor on its own layer, so it must not answer a ground probe:
/// a sub dives *through* the surface it would otherwise stand on.
#[test]
fn the_ground_probe_ignores_water_sensors() {
    let mut app = physics_app();

    app.world_mut().spawn((
        Collider::trimesh(
            vec![
                Vec3::new(-10.0, 0.0, -10.0),
                Vec3::new(-10.0, 0.0, 10.0),
                Vec3::new(10.0, 0.0, 10.0),
            ],
            vec![[0, 1, 2]],
        ),
        RigidBody::Static,
        Sensor,
        layers::water(),
        Transform::IDENTITY,
    ));
    step(&mut app, 1);

    let hit = app
        .world_mut()
        .run_system_cached(|spatial: SpatialQuery, surfaces: SurfaceQuery| {
            probe_ground(
                &spatial,
                &surfaces,
                GroundProbe::down(Vec3::new(-5.0, 5.0, 0.0), 20.0),
            )
        })
        .expect("the system runs");

    assert!(hit.is_none(), "water is not ground");
}

/// The stream is seeded from the resource, and the debug flag exists
/// for the dev tools to toggle.
#[test]
fn the_plugin_installs_the_seeded_rng_and_the_debug_flags() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    // Before the plugins, the way `bft-bin` and a multiplayer client set it:
    // the stream is seeded in `PreStartup`, once the value is final.
    app.insert_resource(SimSeed(1234));
    app.add_plugins(bft_sim::SimPlugin);
    app.finish();
    app.update();

    assert!(app.world().get_resource::<SimRng>().is_some());
    assert_eq!(app.world().resource::<SimSeed>().0, 1234);
    assert!(!app.world().resource::<DebugFlags>().physics);
}
