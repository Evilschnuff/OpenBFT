//! The cinematics' headless tests: the entry animations, the control state and
//! the player's hull during a cinematic.
//!
//! The fixtures copy the shipped shapes (`real_cinematics.rs`) with the
//! numbers simplified: an intro whose `reset_state` hands over and disarms
//! itself, a camera flown by an SI script, m1's fade in from black. The real
//! levels are `real_cinematics.rs`'s.

use std::time::Duration;

use avian3d::prelude::*;
use bevy::prelude::*;
use bft_assets::anim::{
    AnimActivation, AnimDefDesc, AnimSet, EventDataDesc, EventDesc, EventStart, Execution,
    ResetTime, SeqDesc, SiFrame, SiScriptDesc,
};
use bft_sim::anim::{
    AnimCallback, AnimInstance, AnimProgram, AnimScopes, AnimScreenFx, CallAnim,
    apply_initial_node_states,
};
use bft_sim::cinematic::{
    Parked, PlayerControl, PlayerTeleported, SkipCinematic, SupportCarry, carry_on_support,
};
use bft_sim::level::{NodeActive, NodeIndex, NodeName};
use bft_sim::schedule::SIM_HZ;
use bft_sim::vehicle::input::PlayerInput;
use bft_sim::vehicle::{GroundState, PlayerVehicle};
use serde_json::json;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn def(name: &str, anim_name: &str) -> AnimDefDesc {
    AnimDefDesc {
        name: name.to_string(),
        anim_name: anim_name.to_string(),
        anim_root_name: name.to_string(),
        activation: AnimActivation::OnCall,
        execution: Execution::None,
        reset_time: ResetTime::Unset,
        active: true,
        ..Default::default()
    }
}

fn now(data: EventDataDesc) -> EventDesc {
    EventDesc {
        start: EventStart::WithPrevious,
        data,
    }
}

fn seq(name: &str, events: Vec<EventDesc>) -> SeqDesc {
    SeqDesc {
        name: name.to_string(),
        on_call: false,
        events,
    }
}

fn ev(kind: &'static str, value: serde_json::Value) -> EventDataDesc {
    EventDataDesc::Presentation { kind, value }
}

fn set_active(node: &str, state: bool) -> EventDataDesc {
    EventDataDesc::ObjectActiveState {
        node: node.to_string(),
        state,
    }
}

/// A one-second flight along +x at 1 m/s, in two frames.
fn camera_script() -> SiScriptDesc {
    let frame = |start: f32| SiFrame {
        start,
        end: start + 0.5,
        translate: Some((Vec3::new(start, 0.0, 0.0), Vec3::X)),
        rotate: None,
        scale: None,
    };
    SiScriptDesc {
        script_name: "camera".into(),
        object_name: "camera1".into(),
        frames: Vec::new(),
        keys: vec![frame(0.0), frame(0.5)],
    }
}

/// `m1_start_animation`'s shape: callbacks `20, 2, 14`, a fade in from
/// black, the camera's flight; a `reset_state` that hands over, breaks the
/// chains and disarms the intro.
fn m1_intro() -> AnimDefDesc {
    let mut intro = def("camera1", "m1_start_animation");
    intro.sequences = vec![
        seq(
            "introsnd_callbacks",
            vec![
                now(EventDataDesc::Callback(20)),
                now(EventDataDesc::Callback(2)),
                now(EventDataDesc::Callback(14)),
            ],
        ),
        seq(
            "fade_in",
            vec![now(ev(
                "FbfxColorFromTo",
                json!({ "from": { "r": 0.0, "g": 0.0, "b": 0.0, "a": 1.0 },
                        "to": { "r": 1.0, "g": 1.0, "b": 1.0, "a": 0.0 },
                        "run_time": 0.5, "alpha_delta": null }),
            ))],
        ),
        seq(
            "camera_start_script",
            vec![now(ev(
                "ObjectMotionSiScript",
                json!({ "name": "camera1", "index": 0 }),
            ))],
        ),
    ];
    intro.reset_state = vec![
        now(EventDataDesc::Callback(15)),
        now(EventDataDesc::Callback(10)),
        now(EventDataDesc::Callback(1)),
        now(set_active("semichains", false)),
        now(EventDataDesc::InvalidateAnimation(
            "m1_start_animation".into(),
        )),
    ];
    intro
}

fn program(defs: Vec<AnimDefDesc>) -> AnimSet {
    let mut set = AnimSet {
        gravity: -9.8,
        si_scripts: vec![camera_script()],
        ..Default::default()
    };
    for (index, def) in defs.into_iter().enumerate() {
        set.by_anim_name
            .entry(def.anim_name.clone())
            .or_default()
            .push(index);
        set.defs.push(def);
    }
    set
}

/// The whole `SimPlugin` with a hand-built level, and the level's initial
/// node states applied the way `bft-game` applies them at load.
fn level(defs: Vec<AnimDefDesc>, names: &[&str]) -> (App, Vec<Entity>) {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bft_sim::SimPlugin);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        Duration::from_secs_f64(1.0 / SIM_HZ),
    ));
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.insert_resource(AnimProgram(program(defs)));
    app.finish();
    app.update();

    let mut index = NodeIndex::default();
    let mut entities = Vec::new();
    for (id, name) in names.iter().enumerate() {
        let entity = app
            .world_mut()
            .spawn((
                NodeName(name.to_string()),
                NodeActive(true),
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        index.insert(id as u32, name, entity);
        entities.push(entity);
    }
    app.insert_resource(index);

    let set = app.world().resource::<AnimProgram>().0.clone();
    let mut scopes = Vec::new();
    for (def_index, def) in set.defs.iter().enumerate() {
        let root = app
            .world()
            .resource::<NodeIndex>()
            .by_name(&def.name)
            .first()
            .copied();
        app.world_mut()
            .spawn(AnimInstance::new(def_index, def.activation, root));
        scopes.push(Default::default());
    }
    app.insert_resource(AnimScopes(scopes));

    app.world_mut()
        .run_system_cached(
            |program: Res<AnimProgram>,
             scopes: Res<AnimScopes>,
             nodes: Res<NodeIndex>,
             mut actives: Query<&mut NodeActive>| {
                apply_initial_node_states(&program.0, &scopes, &nodes, &mut actives);
            },
        )
        .expect("the initial states apply");

    (app, entities)
}

fn step(app: &mut App, ticks: usize) {
    for _ in 0..ticks {
        app.update();
    }
}

fn seconds(secs: f32) -> usize {
    (secs * SIM_HZ as f32).round() as usize
}

fn call(app: &mut App, name: &str) {
    app.world_mut()
        .resource_mut::<Messages<CallAnim>>()
        .write(CallAnim::new(name));
}

fn callbacks(app: &mut App, ticks: usize) -> Vec<i32> {
    let mut out = Vec::new();
    for _ in 0..ticks {
        app.update();
        out.extend(
            app.world_mut()
                .resource_mut::<Messages<AnimCallback>>()
                .drain()
                .map(|callback| callback.value),
        );
    }
    out
}

fn active(app: &App, entity: Entity) -> bool {
    app.world().get::<NodeActive>(entity).expect("a node").0
}

fn control(app: &App) -> PlayerControl {
    app.world().resource::<PlayerControl>().clone()
}

fn intro_instance(app: &mut App) -> AnimInstance {
    let mut query = app.world_mut().query::<&AnimInstance>();
    query
        .iter(app.world())
        .find(|instance| instance.def == 0)
        .expect("the intro")
        .clone()
}

// ---------------------------------------------------------------------------
// Entry animations
// ---------------------------------------------------------------------------

/// m1's intro must not disarm itself on its first tick, before it is even
/// called: it plays, and its `reset_state` lands when it ends.
#[test]
fn m1s_intro_plays_and_invalidates_itself_at_the_end_not_at_load() {
    let (mut app, nodes) = level(vec![m1_intro()], &["camera1", "semichains"]);
    let chains = nodes[1];

    assert!(active(&app, chains), "not broken at load");
    step(&mut app, 3);
    assert!(
        !intro_instance(&mut app).invalidated,
        "not disarmed at load"
    );

    call(&mut app, "m1_start_animation");
    let fired = callbacks(&mut app, seconds(0.5));
    assert_eq!(
        fired,
        vec![20, 2, 14],
        "the intro's own callbacks, no hand-over"
    );
    assert_eq!(
        control(&app),
        PlayerControl::Cinematic {
            anim: "m1_start_animation".into(),
            carried: false
        }
    );
    assert!(active(&app, chains), "the chains hold during the intro");

    let fired = callbacks(&mut app, seconds(0.75));
    assert_eq!(fired, vec![15, 10, 1], "the hand-over, once, at the end");
    assert_eq!(control(&app), PlayerControl::Driving);
    assert!(!active(&app, chains), "the reset broke the chains");
    let intro = intro_instance(&mut app);
    assert!(intro.invalidated && intro.completed && !intro.is_running());
    let camera = app.world().get::<Transform>(nodes[0]).unwrap().translation;
    assert!(
        (camera - Vec3::X).length() < 1e-3,
        "the flight ended: {camera}"
    );

    // Disarmed: calling it again does nothing.
    call(&mut app, "m1_start_animation");
    assert!(callbacks(&mut app, 5).is_empty());
}

/// What the player sees after a skip and after watching it out.
#[derive(Debug, PartialEq)]
struct HandOff {
    control: PlayerControl,
    chains: bool,
    camera: Vec3,
    tint: Option<LinearRgba>,
    invalidated: bool,
}

fn hand_off(app: &mut App, nodes: &[Entity]) -> HandOff {
    let camera = app.world().get::<Transform>(nodes[0]).unwrap().translation;
    HandOff {
        control: control(app),
        chains: active(app, nodes[1]),
        // Millimetres: the full play's last tick lands on the frame, the
        // skip computes it.
        camera: (camera * 1000.0).round() / 1000.0,
        tint: app.world().resource::<AnimScreenFx>().tint,
        invalidated: intro_instance(app).invalidated,
    }
}

/// A skip mid-intro — half-way through the fade in from black, a fifth of
/// the way along the camera's flight — lands where watching it out does.
#[test]
fn a_skip_mid_intro_lands_in_the_same_state_as_a_full_play() {
    let (mut watched, watched_nodes) = level(vec![m1_intro()], &["camera1", "semichains"]);
    call(&mut watched, "m1_start_animation");
    step(&mut watched, seconds(1.5));

    let (mut skipped, skipped_nodes) = level(vec![m1_intro()], &["camera1", "semichains"]);
    call(&mut skipped, "m1_start_animation");
    step(&mut skipped, seconds(0.2));
    assert!(control(&skipped).is_cinematic());
    assert!(
        skipped.world().resource::<AnimScreenFx>().tint.is_some(),
        "mid fade"
    );
    skipped
        .world_mut()
        .resource_mut::<Messages<SkipCinematic>>()
        .write(SkipCinematic);
    step(&mut skipped, 2);

    let watched = hand_off(&mut watched, &watched_nodes);
    assert_eq!(watched.control, PlayerControl::Driving);
    assert_eq!(watched.tint, None, "faded in fully");
    assert_eq!(hand_off(&mut skipped, &skipped_nodes), watched);
}

/// Only an intro is skippable: a definition with a plain `reset_state` (an
/// exit) runs on.
#[test]
fn a_skip_with_no_intro_running_changes_nothing() {
    let mut exit = def("vtol1", "m2_exit_animation");
    exit.sequences = vec![seq(
        "camera_exit_script",
        vec![
            now(EventDataDesc::Callback(2)),
            now(ev(
                "ObjectMotionSiScript",
                json!({ "name": "camera1", "index": 0 }),
            )),
            now(EventDataDesc::Callback(99)),
        ],
    )];
    let (mut app, _) = level(vec![exit], &["vtol1", "camera1"]);
    call(&mut app, "m2_exit_animation");
    step(&mut app, seconds(0.2));
    app.world_mut()
        .resource_mut::<Messages<SkipCinematic>>()
        .write(SkipCinematic);

    let fired = callbacks(&mut app, seconds(0.2));
    assert!(!fired.contains(&99), "the exit was not cut short");
    assert!(intro_instance(&mut app).is_running());
}

// ---------------------------------------------------------------------------
// The player during a cinematic
// ---------------------------------------------------------------------------

fn player(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            PlayerVehicle,
            PlayerInput {
                throttle: 1.0,
                fire: true,
                ..Default::default()
            },
            Transform::from_xyz(0.0, 10.0, 0.0),
            RigidBody::Dynamic,
            Collider::cuboid(2.0, 1.0, 3.0),
            LinearVelocity(Vec3::new(3.0, 0.0, 0.0)),
        ))
        .id()
}

/// m2's intro: `2, 14`, the BFT hidden, `11`. The input is ignored, the hull
/// parked; the hand-off releases both and stands the hull still.
#[test]
fn a_carried_player_is_parked_and_its_input_ignored_until_the_hand_off() {
    let mut intro = def("vtol1", "m2_start_animation");
    intro.sequences = vec![seq(
        "camera_start_script",
        vec![
            now(EventDataDesc::Callback(2)),
            now(EventDataDesc::Callback(14)),
            now(EventDataDesc::Callback(11)),
            now(ev(
                "ObjectMotionSiScript",
                json!({ "name": "camera1", "index": 0 }),
            )),
        ],
    )];
    intro.reset_state = vec![now(EventDataDesc::Callback(15))];
    let (mut app, _) = level(vec![intro], &["vtol1", "camera1"]);
    let hull = player(&mut app);

    call(&mut app, "m2_start_animation");
    step(&mut app, 3);
    assert!(control(&app).is_carried());
    assert!(app.world().get::<Parked>(hull).is_some());
    assert!(app.world().get::<ColliderDisabled>(hull).is_some());
    let input = app.world().get::<PlayerInput>(hull).unwrap();
    assert!(input.is_idle(), "input is locked: {input:?}");

    // The frame layer keeps writing; the sim keeps ignoring.
    app.world_mut()
        .get_mut::<PlayerInput>(hull)
        .unwrap()
        .throttle = 1.0;
    step(&mut app, 1);
    assert!(app.world().get::<PlayerInput>(hull).unwrap().is_idle());

    // Held in the air for the fly-in, not falling out of it.
    let parked = app.world().get::<Transform>(hull).unwrap().translation;
    step(&mut app, seconds(0.5));
    let held = app.world().get::<Transform>(hull).unwrap().translation;
    assert!((held - parked).length() < 1e-3, "{parked} → {held}");

    step(&mut app, seconds(1.0));
    assert_eq!(control(&app), PlayerControl::Driving);
    assert!(app.world().get::<Parked>(hull).is_none());
    assert!(app.world().get::<ColliderDisabled>(hull).is_none());
    app.world_mut()
        .get_mut::<PlayerInput>(hull)
        .unwrap()
        .throttle = 1.0;
    step(&mut app, 1);
    assert_eq!(
        app.world().get::<PlayerInput>(hull).unwrap().throttle,
        1.0,
        "the player drives again"
    );
}

/// `Callback 17` is announced so the view can snap to the moved hull.
#[test]
fn a_script_moving_the_player_is_announced() {
    let mut lift = def("belev", "belev_start");
    lift.sequences = vec![seq(
        "move_eup",
        vec![
            now(ev(
                "ObjectTranslateState",
                json!({ "node": "bft_00", "relative": true,
                        "state": { "x": 0.0, "y": 161.87, "z": 0.0 }, "at_node": null }),
            )),
            now(EventDataDesc::Callback(17)),
        ],
    )];
    let (mut app, _) = level(vec![lift], &["belev", "bft_00"]);
    call(&mut app, "belev_start");

    let mut moved = Vec::new();
    for _ in 0..3 {
        app.update();
        moved.extend(
            app.world_mut()
                .resource_mut::<Messages<PlayerTeleported>>()
                .drain(),
        );
    }
    assert_eq!(
        moved,
        vec![PlayerTeleported {
            by: "belev_start".into()
        }]
    );
    assert_eq!(control(&app), PlayerControl::Driving, "not a cinematic");
}

// ---------------------------------------------------------------------------
// Riding a scripted support
// ---------------------------------------------------------------------------

/// A world with only what `carry_on_support` reads: a hull standing on a
/// node, both moved by hand between runs.
fn carry_world() -> (World, Entity, Entity) {
    let mut world = World::new();
    let mut time = Time::<Fixed>::from_hz(SIM_HZ);
    time.advance_by(Duration::from_secs_f64(1.0 / SIM_HZ));
    world.insert_resource(time);
    world.insert_resource(SupportCarry::default());
    world.insert_resource(PlayerControl::Cinematic {
        anim: "m1_start_animation".into(),
        carried: false,
    });

    let trailer = world.spawn(Transform::from_xyz(2223.0, 2.0, 1290.0)).id();
    let hull = world
        .spawn((
            PlayerVehicle,
            Transform::from_xyz(2223.0, 4.0, 1290.0),
            GroundState {
                support: Some(trailer),
                ..GroundState::airborne()
            },
            LinearVelocity::ZERO,
        ))
        .id();
    (world, trailer, hull)
}

/// m1's trailer drives 0.14 m a tick with the BFT chained on it. The hull
/// rides along, and takes the trailer's speed with it when it leaves.
#[test]
fn the_hull_rides_the_node_it_stands_on_and_keeps_its_speed_off_it() {
    let (mut world, trailer, hull) = carry_world();
    world.run_system_cached(carry_on_support).unwrap();

    for _ in 0..10 {
        world.get_mut::<Transform>(trailer).unwrap().translation.z += 0.14;
        world.run_system_cached(carry_on_support).unwrap();
    }
    let at = world.get::<Transform>(hull).unwrap().translation;
    assert!(
        (at - Vec3::new(2223.0, 4.0, 1291.4)).length() < 1e-3,
        "rode 1.4 m: {at}"
    );

    // Driven off: the support is the ground now, which does not move.
    *world.get_mut::<GroundState>(hull).unwrap() = GroundState::default();
    world.run_system_cached(carry_on_support).unwrap();
    let speed = world.get::<LinearVelocity>(hull).unwrap().0;
    assert!(
        (speed - Vec3::new(0.0, 0.0, 0.14 * SIM_HZ as f32)).length() < 1e-2,
        "the trailer's 8.4 m/s: {speed}"
    );
}

/// An elevator or teleport jumps its support tens of metres in a tick and
/// moves `bft_00` itself; carrying the hull too would move it twice.
#[test]
fn a_support_that_jumps_does_not_carry() {
    let (mut world, trailer, hull) = carry_world();
    world.run_system_cached(carry_on_support).unwrap();

    world.get_mut::<Transform>(trailer).unwrap().translation.y += 161.87;
    world.run_system_cached(carry_on_support).unwrap();
    assert_eq!(
        world.get::<Transform>(hull).unwrap().translation,
        Vec3::new(2223.0, 4.0, 1290.0)
    );
}
