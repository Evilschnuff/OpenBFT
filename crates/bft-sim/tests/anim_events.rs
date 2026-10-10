//! The anim handlers' headless tests: one or more per event handler.
//!
//! Each builds a two- or three-node level and a definition by hand, in the
//! shapes the compiled `anim.zbd` has (the JSON is copied from real events,
//! names kept, numbers simplified), runs the real `SimPlugin` and checks the
//! component, resource or message the handler is for. `real_anim_events.rs`
//! runs the same handlers over the shipped levels.

use avian3d::prelude::*;
use bevy::prelude::*;
use bft_assets::anim::{
    AnimActivation, AnimDefDesc, AnimSet, CallAt, EventDataDesc, EventDesc, EventStart, Execution,
    OffsetFrom, ResetTime, SeqDesc, SiFrame, SiScriptDesc,
};
use bft_sim::anim::{
    AnimCamera, AnimInstance, AnimLights, AnimOpacity, AnimOutput, AnimPlaced, AnimProgram,
    AnimScopes, AnimScreenFx, CallAnim, EventHandlers, PlayerModelRoot,
};
use bft_sim::level::{GameLayer, NodeActive, NodeIndex, NodeName, WorldEnv};
use bft_sim::schedule::SIM_HZ;
use serde_json::json;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn def(name: &str, anim_name: &str) -> AnimDefDesc {
    AnimDefDesc {
        name: name.to_string(),
        anim_name: anim_name.to_string(),
        anim_root_name: name.to_string(),
        activation: AnimActivation::OnStartup,
        execution: Execution::None,
        reset_time: ResetTime::Never,
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

fn at(from: OffsetFrom, time: f32, data: EventDataDesc) -> EventDesc {
    EventDesc {
        start: EventStart::Offset { from, time },
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

fn on_call(name: &str, events: Vec<EventDesc>) -> SeqDesc {
    SeqDesc {
        name: name.to_string(),
        on_call: true,
        events,
    }
}

/// A presentation event, as the loader carries it.
fn ev(kind: &'static str, value: serde_json::Value) -> EventDataDesc {
    EventDataDesc::Presentation { kind, value }
}

fn set_active(node: &str, state: bool) -> EventDataDesc {
    EventDataDesc::ObjectActiveState {
        node: node.to_string(),
        state,
    }
}

fn program(defs: Vec<AnimDefDesc>) -> AnimSet {
    let mut set = AnimSet {
        gravity: -9.8,
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

fn app_with(program: AnimSet) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bft_sim::SimPlugin);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / SIM_HZ),
    ));
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.insert_resource(AnimProgram(program));
    app.finish();
    app.update();
    app
}

/// Spawn named root nodes at positions, and index them.
fn spawn_nodes(app: &mut App, nodes: &[(&str, Vec3)]) -> Vec<Entity> {
    let entities: Vec<Entity> = nodes
        .iter()
        .map(|(name, position)| {
            app.world_mut()
                .spawn((
                    NodeName(name.to_string()),
                    NodeActive(true),
                    Transform::from_translation(*position),
                    GlobalTransform::from_translation(*position),
                ))
                .id()
        })
        .collect();

    let mut index = NodeIndex::default();
    for (id, ((name, _), entity)) in nodes.iter().zip(&entities).enumerate() {
        index.insert(id as u32, name, *entity);
    }
    app.insert_resource(index);
    entities
}

fn spawn_instances(app: &mut App) -> Vec<Entity> {
    let program = app.world().resource::<AnimProgram>().0.clone();
    let index = app.world().resource::<NodeIndex>().clone();

    let mut scopes = Vec::new();
    let mut instances = Vec::new();
    for (def_index, def) in program.defs.iter().enumerate() {
        let root = index.by_name(&def.name).first().copied();
        instances.push(
            app.world_mut()
                .spawn(AnimInstance::new(def_index, def.activation, root))
                .id(),
        );
        scopes.push(std::collections::HashMap::new());
    }
    app.insert_resource(AnimScopes(scopes));
    instances
}

/// A level of the given nodes running the given definitions.
fn level(defs: Vec<AnimDefDesc>, nodes: &[(&str, Vec3)]) -> (App, Vec<Entity>) {
    let mut app = app_with(program(defs));
    let entities = spawn_nodes(&mut app, nodes);
    spawn_instances(&mut app);
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

fn collect<M: Message + Clone>(app: &mut App, ticks: usize) -> Vec<M> {
    let mut out = Vec::new();
    for _ in 0..ticks {
        app.update();
        out.extend(
            app.world_mut()
                .resource_mut::<Messages<M>>()
                .drain()
                .collect::<Vec<M>>(),
        );
    }
    out
}

fn translation(app: &App, entity: Entity) -> Vec3 {
    app.world()
        .get::<Transform>(entity)
        .expect("a node")
        .translation
}

fn active(app: &App, entity: Entity) -> bool {
    app.world().get::<NodeActive>(entity).expect("a node").0
}

fn call(app: &mut App, name: &str) {
    app.world_mut()
        .resource_mut::<Messages<CallAnim>>()
        .write(CallAnim::new(name));
}

fn close(a: Vec3, b: Vec3, tolerance: f32) -> bool {
    (a - b).length() <= tolerance
}

// ---------------------------------------------------------------------------
// Coverage
// ---------------------------------------------------------------------------

/// Every one of the 37 kinds Recoil's anims use has a handler, apart from
/// the five the VM runs itself (`Loop` and the four `If` parts).
#[test]
fn every_kind_the_game_uses_has_a_handler() {
    let app = app_with(program(Vec::new()));
    let handlers = app.world().resource::<EventHandlers>();

    let kinds = [
        "ObjectActiveState",
        "CallAnimation",
        "StopAnimation",
        "InvalidateAnimation",
        "ResetAnimation",
        "CallSequence",
        "StopSequence",
        "Callback",
        "ObjectMotion",
        "ObjectMotionFromTo",
        "ObjectMotionSiScript",
        "ObjectTranslateState",
        "ObjectScaleState",
        "ObjectRotateState",
        "ObjectOpacityState",
        "ObjectOpacityFromTo",
        "ObjectAddChild",
        "ObjectDeleteChild",
        "ObjectCycleTexture",
        "ObjectConnector",
        "CallObjectConnector",
        "Sound",
        "SoundNode",
        "Effect",
        "LightState",
        "LightAnimation",
        "FogState",
        "FbfxColorFromTo",
        "FbfxCsinwaveFromTo",
        "CameraState",
        "CameraFromTo",
        "AnimVerbose",
    ];
    for kind in kinds {
        assert!(handlers.get(kind).is_some(), "no handler for {kind}");
    }
}

// ---------------------------------------------------------------------------
// Object state
// ---------------------------------------------------------------------------

/// Absolute, then relative: `vtol_destruction1`'s pieces and the elevator's
/// lift of the player.
#[test]
fn translate_state_sets_then_adds() {
    let mut d = def("prop", "place");
    d.sequences = vec![seq(
        "go",
        vec![
            now(ev(
                "ObjectTranslateState",
                json!({ "node": "piece", "relative": false,
                        "state": { "x": 1.0, "y": 2.0, "z": 3.0 }, "at_node": null }),
            )),
            now(ev(
                "ObjectTranslateState",
                json!({ "node": "piece", "relative": true,
                        "state": { "x": 0.0, "y": 10.0, "z": 0.0 }, "at_node": null }),
            )),
        ],
    )];
    let (mut app, nodes) = level(vec![d], &[("prop", Vec3::ZERO), ("piece", Vec3::ZERO)]);

    step(&mut app, 2);

    assert!(close(
        translation(&app, nodes[1]),
        Vec3::new(1.0, 12.0, 3.0),
        1e-4
    ));
}

/// Once `bft_00` is the player's model, what the scripts may do to it depends
/// on who drives ([`PlayerControl`](bft_sim::cinematic::PlayerControl)).
///
/// Driving, they cannot hide it or park it in the VTOL's cargo bay; an
/// elevator's relative lift moves the hull under it instead. During a
/// cinematic (`Callback 2`) they own it: the bay placement puts the *hull*
/// there, stopped, and the hide hides the model. After the hand-off the
/// driving rule holds again.
#[test]
fn the_players_model_is_left_to_the_engine() {
    let mut d = def("vtol1", "m2_start_animation");
    d.sequences = vec![seq(
        "go",
        vec![
            now(ev(
                "ObjectTranslateState",
                json!({ "node": "bft_00", "relative": false,
                        "state": { "x": 1697.0, "y": 36.6, "z": 656.5 }, "at_node": null }),
            )),
            now(ev(
                "ObjectRotateState",
                json!({ "name": "bft_00", "basis": "Absolute",
                        "state": { "x": 0.0, "y": -1.5707964, "z": 0.0 } }),
            )),
            now(set_active("bft_00", false)),
            now(ev(
                "ObjectTranslateState",
                json!({ "node": "bft_00", "relative": true,
                        "state": { "x": 0.0, "y": 10.0, "z": 0.0 }, "at_node": null }),
            )),
        ],
    )];
    let (mut app, nodes) = level(vec![d], &[("vtol1", Vec3::ZERO), ("bft_00", Vec3::ZERO)]);
    let hull = app
        .world_mut()
        .spawn(Transform::from_xyz(5.0, 1.0, 5.0))
        .id();
    app.world_mut()
        .entity_mut(nodes[1])
        .insert((ChildOf(hull), PlayerModelRoot { hull }));

    step(&mut app, 2);

    assert!(active(&app, nodes[1]), "the intro hid the player");
    assert_eq!(translation(&app, nodes[1]), Vec3::ZERO);
    assert_eq!(
        app.world().get::<Transform>(nodes[1]).unwrap().rotation,
        Quat::IDENTITY
    );
    assert!(close(
        translation(&app, hull),
        Vec3::new(5.0, 11.0, 5.0),
        1e-4
    ));

    // The cinematic half: the same placements, inside `2, 14`.
    let mut cinematic = def("vtol1", "m2_exit_animation");
    cinematic.activation = AnimActivation::OnCall;
    cinematic.sequences = vec![seq(
        "go",
        vec![
            now(set_active("bft_00", false)),
            now(EventDataDesc::Callback(2)),
            now(EventDataDesc::Callback(14)),
            now(ev(
                "ObjectTranslateState",
                json!({ "node": "bft_00", "relative": false,
                        "state": { "x": 1697.0, "y": 36.6, "z": 656.5 }, "at_node": null }),
            )),
            now(ev(
                "ObjectRotateState",
                json!({ "name": "bft_00", "basis": "Absolute",
                        "state": { "x": 0.0, "y": -1.5707964, "z": 0.0 } }),
            )),
        ],
    )];
    let mut hand_over = def("horizon", "load_control");
    hand_over.activation = AnimActivation::OnCall;
    hand_over.sequences = vec![seq(
        "give_player_control",
        vec![
            now(set_active("bft_00", true)),
            now(EventDataDesc::Callback(15)),
            // A tick later: a hand-over holds once the tick that made it has
            // applied its ops, which is what lets a reset place `bft_00` and
            // hand over in one breath.
            at(
                OffsetFrom::Sequence,
                0.05,
                ev(
                    "ObjectTranslateState",
                    json!({ "node": "bft_00", "relative": false,
                            "state": { "x": 0.0, "y": 0.0, "z": 0.0 }, "at_node": null }),
                ),
            ),
        ],
    )];
    let (mut app, nodes) = level(
        vec![cinematic, hand_over],
        &[
            ("vtol1", Vec3::ZERO),
            ("horizon", Vec3::ZERO),
            ("bft_00", Vec3::ZERO),
        ],
    );
    let hull = app
        .world_mut()
        .spawn((
            Transform::from_xyz(5.0, 1.0, 5.0),
            LinearVelocity(Vec3::new(4.0, 0.0, 0.0)),
        ))
        .id();
    app.world_mut()
        .entity_mut(nodes[2])
        .insert((ChildOf(hull), PlayerModelRoot { hull }));

    call(&mut app, "m2_exit_animation");
    step(&mut app, 2);
    assert!(
        !active(&app, nodes[2]),
        "hidden, although the hide came one event before `Callback 2`"
    );
    assert!(close(
        translation(&app, hull),
        Vec3::new(1697.0, 36.6, 656.5),
        1e-3
    ));
    let facing = app.world().get::<Transform>(hull).unwrap().rotation * Vec3::NEG_Z;
    assert!(close(facing, Vec3::X, 1e-4), "yawed −90°: {facing}");
    assert_eq!(
        app.world().get::<LinearVelocity>(hull).unwrap().0,
        Vec3::ZERO
    );
    assert_eq!(
        translation(&app, nodes[2]),
        Vec3::ZERO,
        "the model stays on it"
    );

    // The hand-over shows the model again — it comes before the callback, in
    // the same tick — and the placement after it is the driving rule's.
    call(&mut app, "load_control");
    step(&mut app, 6);
    assert!(active(&app, nodes[2]));
    assert!(close(
        translation(&app, hull),
        Vec3::new(1697.0, 36.6, 656.5),
        1e-3
    ));
    assert_eq!(
        *app.world().resource::<bft_sim::cinematic::PlayerControl>(),
        bft_sim::cinematic::PlayerControl::Driving
    );
}

/// `at_node: INPUT_NODE` puts an effect where it was called.
#[test]
fn translate_state_at_the_input_node_places_the_node_there() {
    let mut caller = def("gun", "fire");
    caller.sequences = vec![seq(
        "go",
        vec![now(EventDataDesc::CallAnimation {
            name: "burst".to_string(),
            operand_node: None,
            wait_for_completion: false,
            parameters: Some(CallAt {
                node: "muzzle".to_string(),
                position: Vec3::ZERO,
                place: false,
            }),
        })],
    )];
    let mut burst = def("burst.flt", "burst");
    burst.activation = AnimActivation::OnCall;
    burst.sequences = vec![seq(
        "go",
        vec![now(ev(
            "ObjectTranslateState",
            json!({ "node": "burst.flt", "relative": false,
                    "state": { "x": 0.0, "y": 0.0, "z": -1.0 }, "at_node": "INPUT_NODE" }),
        ))],
    )];
    let (mut app, nodes) = level(
        vec![caller, burst],
        &[
            ("gun", Vec3::ZERO),
            ("muzzle", Vec3::new(5.0, 2.0, 0.0)),
            ("burst.flt", Vec3::ZERO),
        ],
    );

    step(&mut app, 4);

    assert!(
        close(translation(&app, nodes[2]), Vec3::new(5.0, 2.0, -1.0), 1e-3),
        "{}",
        translation(&app, nodes[2])
    );
}

/// An absolute Euler rotation, in radians.
#[test]
fn rotate_state_turns_the_node() {
    let mut d = def("prop", "turn");
    d.sequences = vec![seq(
        "go",
        vec![now(ev(
            "ObjectRotateState",
            json!({ "name": "piece", "state": { "x": 0.0, "y": std::f32::consts::FRAC_PI_2, "z": 0.0 },
                    "basis": "Absolute" }),
        ))],
    )];
    let (mut app, nodes) = level(vec![d], &[("prop", Vec3::ZERO), ("piece", Vec3::ZERO)]);

    step(&mut app, 2);

    let rotation = app
        .world()
        .get::<Transform>(nodes[1])
        .expect("node")
        .rotation;
    assert!(close(rotation * Vec3::Z, Vec3::X, 1e-4), "{rotation:?}");
}

#[test]
fn scale_state_sets_the_scale() {
    let mut d = def("prop", "grow");
    d.sequences = vec![seq(
        "go",
        vec![now(ev(
            "ObjectScaleState",
            json!({ "name": "piece", "state": { "x": 3.0, "y": 3.0, "z": 3.0 } }),
        ))],
    )];
    let (mut app, nodes) = level(vec![d], &[("prop", Vec3::ZERO), ("piece", Vec3::ZERO)]);

    step(&mut app, 2);

    let scale = app.world().get::<Transform>(nodes[1]).expect("node").scale;
    assert_eq!(scale, Vec3::splat(3.0));
}

/// `smoke_destroy.flt`: switched on, fully transparent, ready to fade in.
#[test]
fn opacity_state_sets_opacity_and_the_active_flag() {
    let mut d = def("prop", "smoke");
    d.sequences = vec![seq(
        "go",
        vec![
            now(set_active("smoke", false)),
            now(ev(
                "ObjectOpacityState",
                json!({ "name": "smoke", "state": true, "opacity": 0.0 }),
            )),
        ],
    )];
    let (mut app, nodes) = level(vec![d], &[("prop", Vec3::ZERO), ("smoke", Vec3::ZERO)]);

    step(&mut app, 3);

    assert_eq!(
        app.world().get::<AnimOpacity>(nodes[1]),
        Some(&AnimOpacity(0.0))
    );
    assert!(active(&app, nodes[1]));
}

/// `bftmbrst.flt`: 1 → 0 over its run time, and gone at the end.
#[test]
fn opacity_fades_over_its_run_time_and_ends_inactive() {
    let mut d = def("prop", "fade");
    d.sequences = vec![seq(
        "go",
        vec![now(ev(
            "ObjectOpacityFromTo",
            json!({ "name": "burst",
                    "opacity_from": { "opacity": 1.0, "state": true },
                    "opacity_to": { "opacity": 0.0, "state": false },
                    "run_time": 1.0, "opacity_delta": null }),
        ))],
    )];
    let (mut app, nodes) = level(vec![d], &[("prop", Vec3::ZERO), ("burst", Vec3::ZERO)]);

    step(&mut app, seconds(0.5) + 1);
    let half = app.world().get::<AnimOpacity>(nodes[1]).expect("opacity").0;
    assert!((half - 0.5).abs() < 0.05, "half way: {half}");
    assert!(active(&app, nodes[1]));

    step(&mut app, seconds(0.6));
    assert_eq!(
        app.world().get::<AnimOpacity>(nodes[1]),
        Some(&AnimOpacity(0.0))
    );
    assert!(!active(&app, nodes[1]), "`state: false` at the end");
}

// ---------------------------------------------------------------------------
// Motion
// ---------------------------------------------------------------------------

/// m1's `dh_doors_open`: the right door slides from where `gamez` put it,
/// `(20, 2, −185.721)` — so `from`/`to` are absolute, not offsets — and the
/// event `{Event, 3.0}` after the motion, counted from the motion's *start*,
/// fires just as it ends (m2's gates are authored the same way).
#[test]
fn a_door_slides_between_absolute_positions_and_the_next_event_waits_for_it() {
    let rest = Vec3::new(20.0, 2.0, -185.721);
    let open = Vec3::new(28.0, 2.0, -185.721);
    let mut d = def("dock_house", "dh_doors_open");
    d.sequences = vec![seq(
        "right_door",
        vec![
            now(ev(
                "ObjectMotionFromTo",
                json!({ "name": "dh_door_right", "run_time": 3.0, "morph": null,
                        "translate": { "from": { "x": rest.x, "y": rest.y, "z": rest.z },
                                       "to": { "x": open.x, "y": open.y, "z": open.z } },
                        "rotate": null, "scale": null, "translate_delta": null,
                        "rotate_delta": null, "scale_delta": null }),
            )),
            at(OffsetFrom::Event, 3.0, set_active("dh_interior", true)),
        ],
    )];
    let (mut app, nodes) = level(
        vec![d],
        &[
            ("dock_house", Vec3::ZERO),
            ("dh_door_right", rest),
            ("dh_interior", Vec3::ZERO),
        ],
    );
    app.world_mut()
        .get_mut::<NodeActive>(nodes[2])
        .expect("node")
        .0 = false;

    step(&mut app, seconds(1.5) + 1);
    let half = translation(&app, nodes[1]);
    assert!(close(half, rest.lerp(open, 0.5), 0.1), "half way: {half}");
    assert!(!active(&app, nodes[2]), "not open while the door moves");

    step(&mut app, seconds(1.6));
    assert!(close(translation(&app, nodes[1]), open, 1e-3));
    assert!(active(&app, nodes[2]), "at three seconds, not six");
}

/// Scale grows between two absolute sizes (`chutes`' `stamp`).
#[test]
fn motion_from_to_scales_absolutely() {
    let mut d = def("prop", "stamp");
    d.sequences = vec![seq(
        "go",
        vec![now(ev(
            "ObjectMotionFromTo",
            json!({ "name": "stamp", "run_time": 0.5, "morph": null, "translate": null,
                    "rotate": null,
                    "scale": { "from": { "x": 0.1, "y": 0.5, "z": 0.1 },
                               "to": { "x": 1.0, "y": 1.0, "z": 1.0 } },
                    "translate_delta": null, "rotate_delta": null, "scale_delta": null }),
        ))],
    )];
    let (mut app, nodes) = level(vec![d], &[("prop", Vec3::ZERO), ("stamp", Vec3::ZERO)]);

    step(&mut app, 2);
    let start = app.world().get::<Transform>(nodes[1]).expect("node").scale;
    assert!(close(start, Vec3::new(0.1, 0.5, 0.1), 0.05), "{start}");

    step(&mut app, seconds(0.6));
    let end = app.world().get::<Transform>(nodes[1]).expect("node").scale;
    assert!(close(end, Vec3::ONE, 1e-4), "{end}");
}

/// A pickup pops up at 16 m/s for an eighth of a second: two metres, and
/// the sequence waits for it.
#[test]
fn an_object_motion_moves_for_its_run_time_and_holds_its_sequence() {
    let mut d = def("pu000", "pop");
    d.sequences = vec![seq(
        "go",
        vec![
            now(ev(
                "ObjectMotion",
                json!({ "node": "pu000", "impact_force": false, "morph": null, "gravity": null,
                        "translation_range": null,
                        "translation": { "initial": { "x": 0.0, "y": 16.0, "z": 0.0 },
                                         "delta": { "x": 0.0, "y": 0.0, "z": 0.0 },
                                         "rnd_xz": { "x": 0.0, "y": 0.0, "z": 0.0 } },
                        "forward_rotation": null, "xyz_rotation": null, "scale": null,
                        "bounce_sequence": null, "bounce_sound": null, "run_time": 0.125 }),
            )),
            now(set_active("done", false)),
        ],
    )];
    let (mut app, nodes) = level(vec![d], &[("pu000", Vec3::ZERO), ("done", Vec3::ZERO)]);

    step(&mut app, 4);
    assert!(active(&app, nodes[1]), "the sequence waits for the motion");

    step(&mut app, seconds(0.3));
    let y = translation(&app, nodes[0]).y;
    assert!((y - 2.0).abs() < 0.3, "popped about two metres: {y}");
    assert!(!active(&app, nodes[1]), "and then carried on");
}

/// Debris falls under gravity, lands on the level, plays its bounce sound
/// and calls its bounce sequence — then the event after it switches it off.
#[test]
fn debris_falls_lands_and_bounces() {
    let mut d = def("hulk", "blow_up");
    d.sequences = vec![
        seq(
            "fly_part1",
            vec![
                now(ev(
                    "ObjectMotion",
                    json!({ "node": "part1", "impact_force": false, "morph": null,
                            "gravity": { "value": -9.8, "complex": false, "no_altitude": false },
                            "translation_range": null, "translation": null,
                            "forward_rotation": { "Time": { "initial": 6.0, "delta": 0.0 } },
                            "xyz_rotation": null, "scale": null,
                            "bounce_sequence": { "default": "stop_trail", "water": null, "lava": null },
                            "bounce_sound": { "default": { "name": "snd_exp_hit3", "volume": 5.0 },
                                              "water": null, "lava": null },
                            "run_time": null }),
                )),
                now(set_active("part1", false)),
            ],
        ),
        on_call("stop_trail", vec![now(set_active("trail", false))]),
    ];
    let (mut app, nodes) = level(
        vec![d],
        &[
            ("hulk", Vec3::ZERO),
            ("part1", Vec3::new(0.0, 5.0, 0.0)),
            ("trail", Vec3::ZERO),
        ],
    );
    // The ground: a static slab on the level layer, top at y = 0.
    app.world_mut().spawn((
        RigidBody::Static,
        Collider::cuboid(100.0, 1.0, 100.0),
        Transform::from_xyz(0.0, -0.5, 0.0),
        CollisionLayers::new(GameLayer::Level, [GameLayer::Vehicle]),
    ));

    let outputs = collect::<AnimOutput>(&mut app, seconds(0.5));
    assert!(active(&app, nodes[1]), "still falling after half a second");
    assert!(outputs.iter().all(|o| o.kind != "Sound"));

    let outputs = collect::<AnimOutput>(&mut app, seconds(1.0));
    let y = translation(&app, nodes[1]).y;
    assert!(y.abs() < 0.3, "landed on the slab: {y}");
    assert!(!active(&app, nodes[1]), "switched off once it landed");
    assert!(!active(&app, nodes[2]), "the bounce sequence ran");
    assert!(
        outputs
            .iter()
            .any(|o| o.kind == "Sound" && o.value["name"] == "snd_exp_hit3"),
        "the bounce sound played"
    );
}

/// `xplflare.flt` shrinks at −20/s with no end: the scale bottoms out at a
/// tiny positive value rather than zero, and nothing goes NaN.
#[test]
fn a_node_shrunk_to_nothing_stays_finite() {
    let mut d = def("xplflare.flt", "xplflare.flt");
    d.sequences = vec![seq(
        "explosion_flare",
        vec![now(ev(
            "ObjectMotion",
            json!({ "node": "xplflare.flt", "impact_force": false, "morph": null,
                    "gravity": null, "translation_range": null, "translation": null,
                    "forward_rotation": null, "xyz_rotation": null,
                    "scale": { "initial": { "x": -20.0, "y": -20.0, "z": -20.0 },
                               "delta": { "x": -5.0, "y": -5.0, "z": -5.0 } },
                    "bounce_sequence": null, "bounce_sound": null, "run_time": 1.0 }),
        ))],
    )];
    let (mut app, nodes) = level(vec![d], &[("xplflare.flt", Vec3::new(3.0, 4.0, 5.0))]);

    step(&mut app, seconds(1.2));

    let transform = app.world().get::<Transform>(nodes[0]).expect("node");
    assert!(transform.translation.is_finite() && transform.rotation.is_finite());
    assert_eq!(
        transform.translation,
        Vec3::new(3.0, 4.0, 5.0),
        "a pure shrink does not move"
    );
    assert!(transform.scale.min_element() > 0.0, "{}", transform.scale);
    assert!(transform.scale.max_element() < 1e-3, "{}", transform.scale);
}

/// A rotor spins until its animation is stopped.
#[test]
fn an_endless_spin_stops_with_its_animation() {
    let mut spin = def("copter", "rotors");
    spin.sequences = vec![seq(
        "spin",
        vec![now(ev(
            "ObjectMotion",
            json!({ "node": "rotor", "impact_force": false, "morph": null, "gravity": null,
                    "translation_range": null, "translation": null, "forward_rotation": null,
                    "xyz_rotation": { "initial": { "x": 0.0, "y": 4.0, "z": 0.0 },
                                      "delta": { "x": 0.0, "y": 0.0, "z": 0.0 } },
                    "scale": null, "bounce_sequence": null, "bounce_sound": null,
                    "run_time": null }),
        ))],
    )];
    let mut stop = def("copter", "land");
    stop.activation = AnimActivation::OnCall;
    stop.sequences = vec![seq(
        "go",
        vec![now(EventDataDesc::StopAnimation("rotors".to_string()))],
    )];
    let (mut app, nodes) = level(
        vec![spin, stop],
        &[("copter", Vec3::ZERO), ("rotor", Vec3::ZERO)],
    );
    let rotation = |app: &App| {
        app.world()
            .get::<Transform>(nodes[1])
            .expect("node")
            .rotation
    };

    step(&mut app, seconds(0.5));
    let turning = rotation(&app);
    step(&mut app, 5);
    assert_ne!(rotation(&app), turning, "still turning");

    app.world_mut()
        .resource_mut::<Messages<CallAnim>>()
        .write(CallAnim::new("land"));
    step(&mut app, 3);
    let stopped = rotation(&app);
    step(&mut app, 10);
    assert_eq!(rotation(&app), stopped, "stopped with its animation");
}

/// An SI script moves its node along the keyframes, linearly.
#[test]
fn an_si_script_plays_its_keyframes() {
    let mut d = def("vtol1", "fly");
    d.sequences = vec![seq(
        "go",
        vec![now(ev(
            "ObjectMotionSiScript",
            json!({ "name": "vtol1", "index": 0 }),
        ))],
    )];
    let mut set = program(vec![d]);
    set.si_scripts = vec![SiScriptDesc {
        keys: vec![
            SiFrame {
                start: 0.0,
                end: 1.0,
                translate: Some((Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0))),
                rotate: Some(Quat::IDENTITY),
                scale: None,
            },
            SiFrame {
                start: 1.0,
                end: 2.0,
                translate: Some((Vec3::new(10.0, 0.0, 0.0), Vec3::new(0.0, 5.0, 0.0))),
                rotate: Some(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)),
                scale: None,
            },
        ],
        ..Default::default()
    }];
    let mut app = app_with(set);
    let nodes = spawn_nodes(&mut app, &[("vtol1", Vec3::new(-50.0, 0.0, 0.0))]);
    spawn_instances(&mut app);

    step(&mut app, seconds(0.5) + 1);
    assert!(close(
        translation(&app, nodes[0]),
        Vec3::new(5.0, 0.0, 0.0),
        0.2
    ));

    step(&mut app, seconds(1.0));
    assert!(close(
        translation(&app, nodes[0]),
        Vec3::new(10.0, 2.5, 0.0),
        0.2
    ));

    step(&mut app, seconds(1.0));
    assert!(close(
        translation(&app, nodes[0]),
        Vec3::new(10.0, 5.0, 0.0),
        1e-3
    ));
}

// ---------------------------------------------------------------------------
// Hierarchy and connectors
// ---------------------------------------------------------------------------

#[test]
fn add_child_reparents_and_delete_child_detaches() {
    let mut d = def("lite_ref.flt", "sparks");
    d.sequences = vec![seq(
        "go",
        vec![
            now(ev(
                "ObjectAddChild",
                json!({ "parent": "lite_ref.flt", "child": "redsprks.flt" }),
            )),
            at(
                OffsetFrom::Event,
                0.1,
                ev(
                    "ObjectDeleteChild",
                    json!({ "parent": "lite_ref.flt", "child": "redsprks.flt" }),
                ),
            ),
        ],
    )];
    let (mut app, nodes) = level(
        vec![d],
        &[("lite_ref.flt", Vec3::X), ("redsprks.flt", Vec3::ZERO)],
    );
    app.world_mut().spawn((
        bft_sim::level::LevelRoot,
        Transform::default(),
        GlobalTransform::default(),
    ));

    step(&mut app, 2);
    assert_eq!(
        app.world().get::<ChildOf>(nodes[1]).map(ChildOf::parent),
        Some(nodes[0])
    );

    step(&mut app, seconds(0.2));
    assert_ne!(
        app.world().get::<ChildOf>(nodes[1]).map(ChildOf::parent),
        Some(nodes[0]),
        "detached"
    );
}

/// A connector stretches its node between two others along z.
#[test]
fn a_connector_stretches_between_its_ends() {
    let mut d = def("boat", "wake");
    d.sequences = vec![seq(
        "go",
        vec![now(ev(
            "ObjectConnector",
            json!({ "name": "trail", "from_node": "a", "to_node": "b",
                    "from_pos": null, "to_pos": null, "from_t": null, "to_t": null,
                    "run_time": 0.0, "max_length": null }),
        ))],
    )];
    let (mut app, nodes) = level(
        vec![d],
        &[
            ("boat", Vec3::ZERO),
            ("a", Vec3::new(0.0, 0.0, 0.0)),
            ("b", Vec3::new(4.0, 0.0, 0.0)),
            ("trail", Vec3::ZERO),
        ],
    );

    step(&mut app, 3);

    let trail = app.world().get::<Transform>(nodes[3]).expect("node");
    assert!(close(trail.translation, Vec3::ZERO, 1e-4));
    assert!((trail.scale.z - 4.0).abs() < 1e-3, "{}", trail.scale);
    assert!(close(trail.rotation * Vec3::Z, Vec3::X, 1e-4));
}

/// `CallObjectConnector` starts the named definition with both ends, and
/// its `"Input"` connector uses them.
#[test]
fn call_object_connector_hands_its_ends_to_the_called_definition() {
    let mut caller = def("boat", "boat_wake");
    caller.sequences = vec![seq(
        "go",
        vec![now(ev(
            "CallObjectConnector",
            json!({ "name": "fadeout_wake", "save_index": null,
                    "from_node": { "name": "a", "pos": true },
                    "to_node": { "name": "b", "pos": true },
                    "from_pos": null, "to_pos": { "Pos": { "x": 0.0, "y": 0.0, "z": 0.0 } } }),
        ))],
    )];
    let mut called = def("wake_fade", "fadeout_wake");
    called.activation = AnimActivation::OnCall;
    called.sequences = vec![seq(
        "go",
        vec![now(ev(
            "ObjectConnector",
            json!({ "name": "wake_fade", "from_node": null, "to_node": null,
                    "from_pos": "Input", "to_pos": "Input", "from_t": null, "to_t": null,
                    "run_time": 0.0, "max_length": null }),
        ))],
    )];
    let (mut app, nodes) = level(
        vec![caller, called],
        &[
            ("boat", Vec3::ZERO),
            ("a", Vec3::new(0.0, 0.0, 2.0)),
            ("b", Vec3::new(0.0, 0.0, 5.0)),
            ("wake_fade", Vec3::ZERO),
        ],
    );

    step(&mut app, 4);

    let fade = app.world().get::<Transform>(nodes[3]).expect("node");
    assert!(close(fade.translation, Vec3::new(0.0, 0.0, 2.0), 1e-3));
    assert!((fade.scale.z - 3.0).abs() < 1e-3, "{}", fade.scale);
}

/// A call made *at* a node moves the called template there and marks it.
/// A prototype with no execution condition plays a copy instead (`anim_vm`
/// covers that); one that waits for the player is armed where it was called.
/// A definition rooted in the level is not a template and stays put.
#[test]
fn a_call_at_a_node_places_the_template_there() {
    let mut caller = def("chutes", "land");
    caller.sequences = vec![seq(
        "go",
        vec![now(EventDataDesc::CallAnimation {
            name: "lg_impact.flt".to_string(),
            operand_node: None,
            wait_for_completion: false,
            parameters: Some(CallAt {
                node: "pch_flare".to_string(),
                position: Vec3::new(0.0, 1.0, 0.0),
                place: true,
            }),
        })],
    )];
    let mut impact = def("lg_impact.flt", "lg_impact.flt");
    impact.activation = AnimActivation::OnCall;
    impact.execution = Execution::ByZone;
    impact.sequences = vec![seq("go", vec![now(set_active("sparks", true))])];
    let (mut app, nodes) = level(
        vec![caller, impact],
        &[
            ("chutes", Vec3::ZERO),
            ("pch_flare", Vec3::new(30.0, 0.0, -7.0)),
            ("lg_impact.flt", Vec3::ZERO),
            ("sparks", Vec3::ZERO),
        ],
    );
    let mut instances = app.world_mut().query::<&mut AnimInstance>();
    for mut instance in instances.iter_mut(app.world_mut()) {
        instance.prototype |= instance.def == 1;
    }

    step(&mut app, 4);

    assert!(close(
        translation(&app, nodes[2]),
        Vec3::new(30.0, 1.0, -7.0),
        1e-3
    ));
    assert!(app.world().get::<AnimPlaced>(nodes[2]).is_some());
}

// ---------------------------------------------------------------------------
// World
// ---------------------------------------------------------------------------

/// `vtol_destruction1`'s `turret_light`: on at a node, then its range
/// animated by +10 over half a second.
#[test]
fn a_light_is_created_and_animated_by_deltas() {
    let mut d = def("vtol1", "burn");
    d.sequences = vec![seq(
        "go",
        vec![
            now(ev(
                "LightState",
                json!({ "name": "turret_light", "active_state": true, "type_": "PointSource",
                        "translate": { "AtNode": { "name": "dbase", "pos": { "x": 0.0, "y": 1.0, "z": 0.0 } } },
                        "range": { "min": 1.0, "max": 20.5 },
                        "color": { "r": 0.8, "g": 0.5, "b": 0.5 } }),
            )),
            now(ev(
                "LightAnimation",
                json!({ "name": "turret_light", "range": { "min": 0.0, "max": 10.0 },
                        "color": { "r": 0.0, "g": 0.0, "b": 0.0 }, "run_time": 0.5,
                        "range_alt": null }),
            )),
            now(set_active("after", false)),
        ],
    )];
    let (mut app, nodes) = level(
        vec![d],
        &[
            ("vtol1", Vec3::ZERO),
            ("dbase", Vec3::ZERO),
            ("after", Vec3::ZERO),
        ],
    );

    step(&mut app, 3);
    let lights = app.world().resource::<AnimLights>();
    let light = lights.0.values().next().expect("a light");
    assert!(light.active);
    assert_eq!(light.at, Some((nodes[1], Vec3::Y)));
    assert!(active(&app, nodes[2]), "the animation holds the sequence");

    step(&mut app, seconds(0.6));
    let lights = app.world().resource::<AnimLights>();
    let light = lights.0.values().next().expect("a light");
    assert!((light.light.far - 30.5).abs() < 0.05, "{}", light.light.far);
    assert!(!active(&app, nodes[2]));
}

#[test]
fn fog_state_changes_the_levels_fog() {
    let mut d = def("sky", "liteupsky");
    d.sequences = vec![seq(
        "go",
        vec![now(ev(
            "FogState",
            json!({ "type_": null, "color": { "r": 0.6, "g": 0.5, "b": 0.0 },
                    "altitude": { "min": 31.0, "max": 48.0 },
                    "range": { "min": 2000.0, "max": 2500.0 } }),
        ))],
    )];
    let (mut app, _) = level(vec![d], &[("sky", Vec3::ZERO)]);
    app.insert_resource(WorldEnv {
        fog_color: Color::BLACK,
        fog_range: (100.0, 200.0),
        fog_altitude: (48.0, 24.0),
        area: (0, 0, 1, 1),
    });

    step(&mut app, 2);

    let env = app.world().resource::<WorldEnv>();
    assert_eq!(env.fog_color, Color::linear_rgb(0.6, 0.5, 0.0));
    assert_eq!(env.fog_range, (2000.0, 2500.0));
    assert_eq!(env.fog_altitude, (31.0, 48.0));
}

// ---------------------------------------------------------------------------
// Presentation cues
// ---------------------------------------------------------------------------

/// `bft_00`'s death: the camera eases from 1 to 0.85 over a second.
#[test]
fn camera_state_and_from_to_drive_the_zoom() {
    let mut d = def("bft_00", "bft_00");
    d.reset_state = vec![now(ev(
        "CameraState",
        json!({ "name": "camera1", "clip_near": null, "clip_far": 3000.0,
                "lod_multiplier": null, "fov_h": null, "fov_v": null,
                "zoom_h": 1.0, "zoom_v": 1.0 }),
    ))];
    d.sequences = vec![seq(
        "go",
        vec![now(ev(
            "CameraFromTo",
            json!({ "name": "camera1", "clip_near": null, "clip_far": null,
                    "lod_multiplier": null, "fov_h": null, "fov_v": null,
                    "zoom_h": { "from": 1.0, "to": 0.85 }, "zoom_v": { "from": 1.0, "to": 0.85 },
                    "run_time": 1.0 }),
        ))],
    )];
    let (mut app, _) = level(vec![d], &[("bft_00", Vec3::ZERO)]);

    step(&mut app, 2);
    assert_eq!(app.world().resource::<AnimCamera>().clip_far, Some(3000.0));

    step(&mut app, seconds(1.1));
    let camera = app.world().resource::<AnimCamera>();
    assert!(close(
        camera.zoom.extend(0.0),
        Vec3::new(0.85, 0.85, 0.0),
        1e-4
    ));
}

/// `expnuke`'s three fades chain one after another: in, hold, out.
#[test]
fn screen_tints_chain_and_clear() {
    let fade = |from: f32, to: f32, run: f32| {
        now(ev(
            "FbfxColorFromTo",
            json!({ "from": { "r": 1.0, "g": 1.0, "b": 1.0, "a": from },
                    "to": { "r": 1.0, "g": 1.0, "b": 1.0, "a": to },
                    "run_time": run, "alpha_delta": null }),
        ))
    };
    let mut d = def("expnuke.flt", "expnuke");
    d.sequences = vec![seq(
        "frame_buffer_effects",
        vec![
            fade(0.25, 0.9, 0.2),
            fade(0.9, 0.9, 0.5),
            fade(0.9, 0.0, 0.5),
        ],
    )];
    let (mut app, _) = level(vec![d], &[("expnuke.flt", Vec3::ZERO)]);

    step(&mut app, seconds(0.5));
    let held = app.world().resource::<AnimScreenFx>().tint.expect("a tint");
    assert!((held.alpha - 0.9).abs() < 1e-3, "holding: {}", held.alpha);

    step(&mut app, seconds(0.95));
    assert_eq!(
        app.world().resource::<AnimScreenFx>().tint,
        None,
        "faded out"
    );
}

/// The screen wave is timing only: it holds its sequence for its run time.
#[test]
fn csinwave_holds_its_sequence_for_its_run_time() {
    let mut d = def("csinwave", "csinwave");
    d.sequences = vec![seq(
        "go",
        vec![
            now(ev(
                "FbfxCsinwaveFromTo",
                json!({ "at_node": null, "screen_pos": null, "world_radius": null,
                        "screen_radius": null, "csin": {}, "run_time": 0.5 }),
            )),
            now(set_active("after", false)),
        ],
    )];
    let (mut app, nodes) = level(vec![d], &[("csinwave", Vec3::ZERO), ("after", Vec3::ZERO)]);

    step(&mut app, seconds(0.3));
    assert!(active(&app, nodes[1]));
    step(&mut app, seconds(0.3));
    assert!(!active(&app, nodes[1]));
}

/// Sound, effect and texture-cycle events reach presentation with their
/// node resolved, and do not count as unsupported.
#[test]
fn sounds_effects_and_texture_cycles_are_passed_on() {
    let mut d = def("prop", "noisy");
    d.sequences = vec![seq(
        "go",
        vec![
            now(ev(
                "Sound",
                json!({ "name": "snd_stone", "at_node": { "name": "locked", "pos": { "x": 0.0, "y": 0.0, "z": 0.0 } } }),
            )),
            now(ev(
                "Effect",
                json!({ "name": "exp_crate", "at_node": { "name": "locked", "pos": { "x": 0.0, "y": 0.0, "z": 0.0 } } }),
            )),
            now(ev(
                "ObjectCycleTexture",
                json!({ "name": "wakepoly", "reset": 0 }),
            )),
        ],
    )];
    let (mut app, nodes) = level(
        vec![d],
        &[
            ("prop", Vec3::ZERO),
            ("locked", Vec3::ZERO),
            ("wakepoly", Vec3::ZERO),
        ],
    );

    let outputs = collect::<AnimOutput>(&mut app, 3);
    let find = |kind: &str| outputs.iter().find(|o| o.kind == kind).map(|o| o.node);
    assert_eq!(find("Sound"), Some(Some(nodes[1])));
    assert_eq!(find("Effect"), Some(Some(nodes[1])));
    assert_eq!(find("ObjectCycleTexture"), Some(Some(nodes[2])));
}

#[test]
fn anim_verbose_is_logged_and_passed() {
    let mut d = def("prop", "start_clones");
    d.sequences = vec![seq(
        "go",
        vec![
            now(ev("AnimVerbose", json!({ "on": true }))),
            now(set_active("after", false)),
        ],
    )];
    let (mut app, nodes) = level(vec![d], &[("prop", Vec3::ZERO), ("after", Vec3::ZERO)]);

    step(&mut app, 2);
    assert!(!active(&app, nodes[1]));
}

// ---------------------------------------------------------------------------
// Reset state and collision
// ---------------------------------------------------------------------------

/// A definition's `reset_state` places its nodes at load, not only their
/// active flags: `vtol_destruction1` puts its pieces where they start.
#[test]
fn reset_state_transforms_apply_on_the_first_tick() {
    let mut d = def("vtol1", "vtol_destruction1");
    d.activation = AnimActivation::OnCall;
    d.reset_state = vec![now(ev(
        "ObjectTranslateState",
        json!({ "node": "piece1", "relative": false,
                "state": { "x": -32.5, "y": 1.75, "z": 7.875 }, "at_node": null }),
    ))];
    let (mut app, nodes) = level(vec![d], &[("vtol1", Vec3::ZERO), ("piece1", Vec3::ZERO)]);

    step(&mut app, 2);

    assert!(close(
        translation(&app, nodes[1]),
        Vec3::new(-32.5, 1.75, 7.875),
        1e-4
    ));
}

/// Scope the named nodes for definition `def`, as `spawn_anim_instances`
/// would from a real hierarchy.
fn scope(app: &mut App, def: usize, names: &[(&str, Entity)]) {
    let mut scopes = app.world_mut().resource_mut::<AnimScopes>();
    for (name, entity) in names {
        scopes.0[def].insert(name.to_string(), *entity);
    }
}

fn shoot(app: &mut App, node: Entity, name: &str, amount: f32) {
    use bft_sim::damage::{DamageKind, NodeDamage};

    app.world_mut()
        .resource_mut::<Messages<NodeDamage>>()
        .write(NodeDamage {
            node,
            name: name.to_string(),
            amount,
            kind: DamageKind::Projectile(0),
            at: Vec3::ZERO,
        });
    step(app, 2);
}

/// A destructible breaks once its hits add up to `health`, and a hit on a
/// child of its `anim_root_name` counts: m1's `drain_01` is rooted at
/// `drhealth_01` and the surface that gets shot is its child `o205`. Hit
/// definitions start armed — they used not to, and so no prop in the game
/// could be destroyed.
#[test]
fn a_destructible_breaks_when_its_hits_reach_its_health() {
    let mut d = def("drain_01", "drain_01");
    d.anim_root_name = "drhealth_01".to_string();
    d.activation = AnimActivation::WeaponHit;
    d.health = 6.0;
    d.nodes = vec!["drhealth_01".to_string()];
    d.sequences = vec![seq("break", vec![now(set_active("drhealth_01", false))])];
    let (mut app, nodes) = level(
        vec![d],
        &[
            ("drain_01", Vec3::ZERO),
            ("drhealth_01", Vec3::ZERO),
            ("o205", Vec3::ZERO),
        ],
    );
    app.world_mut()
        .entity_mut(nodes[1])
        .insert(ChildOf(nodes[0]));
    app.world_mut()
        .entity_mut(nodes[2])
        .insert(ChildOf(nodes[1]));
    scope(
        &mut app,
        0,
        &[("drhealth_01", nodes[1]), ("o205", nodes[2])],
    );

    shoot(&mut app, nodes[2], "o205", 2.0);
    shoot(&mut app, nodes[2], "o205", 2.0);
    assert!(active(&app, nodes[1]), "4 of 6: still standing");
    shoot(&mut app, nodes[2], "o205", 2.0);
    step(&mut app, 2);
    assert!(!active(&app, nodes[1]), "6 of 6: broken");
}

/// A hit belongs to the definition rooted where it lands, not to every
/// definition that lists a node above it. m1's `fuel_station` has one
/// definition per roof post, both listing the `healthy` group the posts
/// share; shooting one post has to drop the roof its way only.
#[test]
fn a_hit_breaks_only_the_definition_rooted_where_it_lands() {
    let post = |root: &str, wreck: &str| {
        let mut d = def("fuel_station", "fuel_station");
        d.anim_root_name = root.to_string();
        d.activation = AnimActivation::WeaponHit;
        d.health = 14.0;
        d.objects = vec!["healthy".to_string(), wreck.to_string()];
        d.sequences = vec![seq(
            "healthy_to_destroyed",
            vec![
                now(set_active("healthy", false)),
                now(set_active(wreck, true)),
            ],
        )];
        d
    };
    let (mut app, nodes) = level(
        vec![
            post("postright", "destroyed"),
            post("postwrong", "destroyed1"),
        ],
        &[
            ("fuel_station", Vec3::ZERO),
            ("healthy", Vec3::ZERO),
            ("postright", Vec3::ZERO),
            ("postwrong", Vec3::ZERO),
            ("destroyed", Vec3::ZERO),
            ("destroyed1", Vec3::ZERO),
        ],
    );
    for (child, parent) in [(1, 0), (2, 1), (3, 1), (4, 0), (5, 0)] {
        app.world_mut()
            .entity_mut(nodes[child])
            .insert(ChildOf(nodes[parent]));
    }
    for def in 0..2 {
        scope(
            &mut app,
            def,
            &[
                ("healthy", nodes[1]),
                ("postright", nodes[2]),
                ("postwrong", nodes[3]),
                ("destroyed", nodes[4]),
                ("destroyed1", nodes[5]),
            ],
        );
    }
    app.world_mut()
        .entity_mut(nodes[4])
        .insert(NodeActive(false));
    app.world_mut()
        .entity_mut(nodes[5])
        .insert(NodeActive(false));

    shoot(&mut app, nodes[3], "postwrong", 20.0);
    step(&mut app, 2);

    assert!(!active(&app, nodes[1]), "the roof fell");
    assert!(active(&app, nodes[5]), "the way of the post shot");
    assert!(!active(&app, nodes[4]), "and not the other way too");
}

/// A definition whose `anim_root_name` its object lacks takes no hits,
/// even on a node it lists: m6's `hit_wall_102` names `wall1` on a wall of
/// `wall2` and `wall3`, and must not break `wall2` for `hit_wall_202`.
#[test]
fn a_definition_rooted_at_a_missing_node_takes_no_hits() {
    let mut d = def("dwall02", "hit_wall_102");
    d.anim_root_name = "wall1".to_string();
    d.activation = AnimActivation::WeaponHit;
    d.health = 1.0;
    d.objects = vec!["healthy".to_string()];
    d.sequences = vec![seq("break", vec![now(set_active("healthy", false))])];
    let (mut app, nodes) = level(
        vec![d],
        &[
            ("dwall02", Vec3::ZERO),
            ("wall2", Vec3::ZERO),
            ("healthy", Vec3::ZERO),
        ],
    );
    app.world_mut()
        .entity_mut(nodes[1])
        .insert(ChildOf(nodes[0]));
    app.world_mut()
        .entity_mut(nodes[2])
        .insert(ChildOf(nodes[1]));
    scope(&mut app, 0, &[("wall2", nodes[1]), ("healthy", nodes[2])]);

    shoot(&mut app, nodes[2], "healthy", 10.0);
    step(&mut app, 2);
    assert!(active(&app, nodes[2]), "wall2 is not this definition's");
}

/// A node switched off stops colliding, its subtree with it, and a node
/// switched back on collides again.
#[test]
fn an_inactive_node_stops_colliding() {
    let mut d = def("gate", "open");
    d.sequences = vec![seq(
        "go",
        vec![
            now(set_active("locked", false)),
            at(OffsetFrom::Event, 0.2, set_active("locked", true)),
        ],
    )];
    let (mut app, nodes) = level(vec![d], &[("gate", Vec3::ZERO), ("locked", Vec3::ZERO)]);
    let child = app
        .world_mut()
        .spawn((
            RigidBody::Static,
            Collider::cuboid(1.0, 1.0, 1.0),
            Transform::default(),
            ChildOf(nodes[1]),
        ))
        .id();

    step(&mut app, 3);
    assert!(app.world().get::<ColliderDisabled>(child).is_some());

    step(&mut app, seconds(0.3));
    assert!(app.world().get::<ColliderDisabled>(child).is_none());
}
