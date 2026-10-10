//! Headless tests of the anim VM and the objective machine.
//!
//! These drive the real `AnimPlugin`/`MissionPlugin` schedules with no
//! renderer and no game data, so they check the systems as they are wired
//! rather than as they are written. The definitions are hand-built in the
//! shapes the compiled `anim.zbd` has; `real_mission.rs` asserts the same
//! code against the shipped 4798.

use bevy::prelude::*;
use bft_assets::anim::{
    AnimActivation, AnimDefDesc, AnimSet, Condition, EventDataDesc, EventDesc, EventStart,
    Execution, OffsetFrom, PrereqDesc, ResetTime, SeqDesc,
};
use bft_assets::catalog::LevelId;
use bft_sim::anim::{
    AnimCallback, AnimClock, AnimInstance, AnimOutput, AnimProgram, AnimScopes, CallAnim,
    EventHandlers, MISSION_END_CALLBACK, NodeActiveChanged, PlayerLocation,
};
use bft_sim::level::{NodeActive, NodeIndex, NodeName, NodeZone};
use bft_sim::mission::{
    Completion, MissionEnded, MissionScript, MissionState, ObjectiveBaseline, ObjectiveDef,
    ObjectiveState, Outcome, Progress,
};
use bft_sim::schedule::SIM_HZ;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A definition with nothing set, for a test to fill in what it cares about.
fn def(name: &str, anim_name: &str) -> AnimDefDesc {
    AnimDefDesc {
        name: name.to_string(),
        anim_name: anim_name.to_string(),
        anim_root_name: name.to_string(),
        activation: AnimActivation::OnCall,
        execution: Execution::None,
        reset_time: ResetTime::Never,
        active: true,
        ..Default::default()
    }
}

/// An event that fires with the previous one.
fn now(data: EventDataDesc) -> EventDesc {
    EventDesc {
        start: EventStart::WithPrevious,
        data,
    }
}

/// An event that fires `time` seconds after `from`.
fn at(from: OffsetFrom, time: f32, data: EventDataDesc) -> EventDesc {
    EventDesc {
        start: EventStart::Offset { from, time },
        data,
    }
}

/// A sequence that runs on activation.
fn initial(name: &str, events: Vec<EventDesc>) -> SeqDesc {
    SeqDesc {
        name: name.to_string(),
        on_call: false,
        events,
    }
}

/// A sequence that runs only through `CallSequence`.
fn on_call(name: &str, events: Vec<EventDesc>) -> SeqDesc {
    SeqDesc {
        name: name.to_string(),
        on_call: true,
        events,
    }
}

/// Toggle a node's active state.
fn set_active(node: &str, state: bool) -> EventDataDesc {
    EventDataDesc::ObjectActiveState {
        node: node.to_string(),
        state,
    }
}

/// Build an [`AnimSet`] from definitions, with the name index they imply.
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

/// A headless app with the VM, its clock and the node scaffolding.
///
/// The real `AnimPlugin` and `MissionPlugin`, not a convenient subset: the
/// point is that the systems run in the order `SimSet` puts them in.
fn app_with(program: AnimSet) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    // The whole `SimPlugin`, not a convenient subset: the point of these
    // headless tests is the wiring, and `SimSet::Anim` running before
    // `SimSet::Mission` is exactly what makes an objective see the node flip
    // in the tick it happened.
    app.add_plugins(bft_sim::SimPlugin);
    // One fixed tick per `step`, rather than whatever wall-clock time the
    // test machine happens to take.
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / SIM_HZ),
    ));
    // `sim_running` needs the state machine to be in play, or nothing steps.
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.insert_resource(AnimProgram(program));
    app.finish();
    // The first update runs no fixed tick — virtual time has not accumulated
    // one yet — so it is spent here.
    app.update();
    app
}

/// Spawn named nodes and index them, the way a level's spawn does.
fn spawn_nodes(app: &mut App, names: &[&str]) -> Vec<Entity> {
    let mut entities = Vec::new();
    for name in names {
        let entity = app
            .world_mut()
            .spawn((
                NodeName(name.to_string()),
                NodeActive(true),
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        entities.push(entity);
    }

    let mut index = NodeIndex::default();
    for (id, (name, entity)) in names.iter().zip(&entities).enumerate() {
        index.insert(id as u32, name, *entity);
    }
    app.insert_resource(index);
    entities
}

/// One instance per definition, with no subtree scoping.
fn spawn_instances(app: &mut App) {
    let program = app.world().resource::<AnimProgram>().0.clone();
    let index = app.world().resource::<NodeIndex>().clone();

    let mut scopes = Vec::new();
    for (def_index, def) in program.defs.iter().enumerate() {
        let root = index.by_name(&def.name).first().copied();
        app.world_mut()
            .spawn(AnimInstance::new(def_index, def.activation, root));
        // No subtree scoping: these fixtures give every node a unique name,
        // so a level-wide lookup is the right one. `real_mission.rs` checks
        // the scoped path against the levels that need it.
        scopes.push(std::collections::HashMap::new());
    }

    app.insert_resource(AnimScopes(scopes));
}

/// Run `ticks` fixed updates.
fn step(app: &mut App, ticks: usize) {
    for _ in 0..ticks {
        app.update();
    }
}

/// Step `ticks` ticks, draining `M` every one and keeping the lot.
///
/// Bevy's message buffers hold **two** updates' worth, so reading after a
/// long `step` sees only the tail. Anything counting events over a run has to
/// collect as it goes.
fn step_collecting<M: Message + Clone>(app: &mut App, ticks: usize) -> Vec<M> {
    let mut collected = Vec::new();
    for _ in 0..ticks {
        app.update();
        collected.extend(
            app.world_mut()
                .resource_mut::<Messages<M>>()
                .drain()
                .collect::<Vec<M>>(),
        );
    }
    collected
}

/// Every `NodeActiveChanged` the run produced.
fn active_changes(events: &[NodeActiveChanged]) -> Vec<(String, bool)> {
    events
        .iter()
        .map(|event| (event.name.clone(), event.active))
        .collect()
}

// ---------------------------------------------------------------------------
// The VM
// ---------------------------------------------------------------------------

/// An `OnStartup` definition runs its sequence through the core handlers
/// without a call.
#[test]
fn an_on_startup_definition_runs_at_level_start() {
    let mut startup = def("sign", "sign_loop");
    startup.activation = AnimActivation::OnStartup;
    startup.sequences = vec![initial(
        "go",
        vec![now(set_active("lit", true)), now(set_active("dark", false))],
    )];

    let mut app = app_with(program(vec![startup]));
    spawn_nodes(&mut app, &["sign", "lit", "dark"]);
    spawn_instances(&mut app);
    // The data has every node active at load, so `lit` has to start false for
    // turning it on to be a change.
    let lit = app.world().resource::<NodeIndex>().by_name("lit")[0];
    app.world_mut()
        .get_mut::<NodeActive>(lit)
        .expect("spawned")
        .0 = false;

    let events = step_collecting::<NodeActiveChanged>(&mut app, 2);

    let changes = active_changes(&events);
    assert!(
        changes.contains(&("lit".to_string(), true)),
        "the startup definition turned its node on: {changes:?}"
    );
    assert!(changes.contains(&("dark".to_string(), false)));
}

/// Two events with the timing the real sequences use: one at once, one a
/// second later, in that order and not before.
#[test]
fn a_two_event_sequence_runs_in_order_with_its_timing() {
    let mut timed = def("gate", "open_gate");
    timed.activation = AnimActivation::OnStartup;
    timed.sequences = vec![initial(
        "open",
        vec![
            now(set_active("closed", false)),
            at(OffsetFrom::Event, 1.0, set_active("open", true)),
        ],
    )];

    let mut app = app_with(program(vec![timed]));
    spawn_nodes(&mut app, &["gate", "closed", "open"]);
    spawn_instances(&mut app);
    let open = app.world().resource::<NodeIndex>().by_name("open")[0];
    app.world_mut()
        .get_mut::<NodeActive>(open)
        .expect("spawned")
        .0 = false;

    // Half a second in: the first event has fired, the second has not.
    step(&mut app, (SIM_HZ / 2.0) as usize);
    let closed = app.world().resource::<NodeIndex>().by_name("closed")[0];
    assert!(
        !app.world().get::<NodeActive>(closed).expect("spawned").0,
        "the first event fires at once"
    );
    assert!(
        !app.world().get::<NodeActive>(open).expect("spawned").0,
        "the second waits its second out"
    );

    // Past the offset.
    step(&mut app, (SIM_HZ * 0.6) as usize);
    assert!(
        app.world().get::<NodeActive>(open).expect("spawned").0,
        "the second event fires once its offset has elapsed"
    );
}

/// `Callback` reaches the mission layer as an `AnimCallback`.
#[test]
fn a_callback_event_produces_an_anim_callback() {
    let mut caller = def("vtol1", "m1_exit_animation");
    caller.activation = AnimActivation::OnStartup;
    caller.sequences = vec![initial(
        "leave",
        vec![now(EventDataDesc::Callback(MISSION_END_CALLBACK))],
    )];

    let mut app = app_with(program(vec![caller]));
    spawn_nodes(&mut app, &["vtol1"]);
    spawn_instances(&mut app);

    let fired = step_collecting::<AnimCallback>(&mut app, 2);

    assert_eq!(fired.len(), 1, "one callback: {fired:?}");
    assert_eq!(fired[0].value, MISSION_END_CALLBACK);
    assert_eq!(fired[0].def, "m1_exit_animation");
}

/// `CallSequence` starts an `OnCall` sequence, which does not run on its own.
#[test]
fn an_on_call_sequence_runs_only_when_called() {
    let mut caller = def("trail", "trail_anim");
    caller.activation = AnimActivation::OnStartup;
    caller.sequences = vec![
        initial(
            "start",
            vec![at(
                OffsetFrom::Sequence,
                0.5,
                EventDataDesc::CallSequence("puff".to_string()),
            )],
        ),
        on_call("puff", vec![now(set_active("smoke", true))]),
    ];

    let mut app = app_with(program(vec![caller]));
    spawn_nodes(&mut app, &["trail", "smoke"]);
    spawn_instances(&mut app);
    let smoke = app.world().resource::<NodeIndex>().by_name("smoke")[0];
    app.world_mut()
        .get_mut::<NodeActive>(smoke)
        .expect("spawned")
        .0 = false;

    // Before the call.
    step(&mut app, (SIM_HZ * 0.3) as usize);
    assert!(
        !app.world().get::<NodeActive>(smoke).expect("spawned").0,
        "an OnCall sequence does not start with the definition"
    );

    step(&mut app, (SIM_HZ * 0.5) as usize);
    assert!(
        app.world().get::<NodeActive>(smoke).expect("spawned").0,
        "CallSequence started it"
    );
}

/// `CallAnimation` starts a definition by name, and `StopAnimation` stops it.
#[test]
fn one_definition_calls_and_stops_another() {
    let mut caller = def("horizon", "start_single_player");
    caller.activation = AnimActivation::OnStartup;
    caller.sequences = vec![initial(
        "go",
        vec![now(EventDataDesc::CallAnimation {
            name: "prop_spin".to_string(),
            operand_node: None,
            wait_for_completion: false,
            parameters: None,
        })],
    )];

    let mut called = def("prop", "prop_spin");
    called.sequences = vec![initial(
        "spin",
        vec![at(OffsetFrom::Sequence, 0.2, set_active("blade", true))],
    )];

    let mut app = app_with(program(vec![caller, called]));
    spawn_nodes(&mut app, &["horizon", "prop", "blade"]);
    spawn_instances(&mut app);
    let blade = app.world().resource::<NodeIndex>().by_name("blade")[0];
    app.world_mut()
        .get_mut::<NodeActive>(blade)
        .expect("spawned")
        .0 = false;

    let calls = step_collecting::<CallAnim>(&mut app, (SIM_HZ * 0.4) as usize);

    assert!(
        app.world().get::<NodeActive>(blade).expect("spawned").0,
        "the called definition ran"
    );

    // And it was not echoed onto the bus, where `start_called_animations`
    // would read it next tick and run the definition a second time.
    assert!(
        calls.iter().all(|call| call.name != "prop_spin"),
        "an internal call stays inside the VM: {calls:?}"
    );
}

/// `InvalidateAnimation` disarms a definition for good.
#[test]
fn an_invalidated_definition_does_not_run_again() {
    let mut killer = def("switch", "kill_it");
    killer.activation = AnimActivation::OnStartup;
    killer.sequences = vec![initial(
        "go",
        vec![now(EventDataDesc::InvalidateAnimation("spin".to_string()))],
    )];

    let mut victim = def("fan", "spin");
    // Immediate reset, so it would re-arm every tick if it were not dead.
    victim.reset_time = ResetTime::Immediate;
    victim.activation = AnimActivation::OnStartup;
    victim.sequences = vec![initial("turn", vec![now(set_active("blade", false))])];

    let mut app = app_with(program(vec![killer, victim]));
    spawn_nodes(&mut app, &["switch", "fan", "blade"]);
    spawn_instances(&mut app);

    step(&mut app, 10);

    let invalidated = app
        .world_mut()
        .query::<&AnimInstance>()
        .iter(app.world())
        .any(|instance| instance.invalidated);
    assert!(invalidated, "the victim was invalidated");
}

/// `Loop { Count: -1 }` restarts the sequence rather than ending it.
#[test]
fn a_forever_loop_keeps_the_sequence_running() {
    let mut looper = def("beacon", "blink");
    looper.activation = AnimActivation::OnStartup;
    looper.sequences = vec![initial(
        "blink",
        vec![
            at(OffsetFrom::Sequence, 0.1, set_active("lamp", true)),
            now(EventDataDesc::Loop(-1)),
        ],
    )];

    let mut app = app_with(program(vec![looper]));
    spawn_nodes(&mut app, &["beacon", "lamp"]);
    spawn_instances(&mut app);

    step(&mut app, (SIM_HZ * 2.0) as usize);

    let running = app
        .world_mut()
        .query::<&AnimInstance>()
        .iter(app.world())
        .any(|instance| instance.is_running());
    assert!(running, "a forever loop never finishes");
}

/// A `PlayerRange` condition compares squared distances against the player.
#[test]
fn a_player_range_condition_reads_squared_metres() {
    let mut gated = def("trigger", "near_only");
    gated.activation = AnimActivation::OnStartup;
    gated.sequences = vec![initial(
        "check",
        vec![
            // 400 is 20 metres squared.
            now(EventDataDesc::If(Condition::PlayerRange(400.0))),
            now(set_active("near", true)),
            now(EventDataDesc::Else),
            now(set_active("far", true)),
            now(EventDataDesc::Endif),
        ],
    )];

    for (distance, expected_near) in [(10.0f32, true), (30.0f32, false)] {
        let mut app = app_with(program(vec![gated.clone()]));
        spawn_nodes(&mut app, &["trigger", "near", "far"]);
        spawn_instances(&mut app);
        for node in ["near", "far"] {
            let entity = app.world().resource::<NodeIndex>().by_name(node)[0];
            app.world_mut()
                .get_mut::<NodeActive>(entity)
                .expect("spawned")
                .0 = false;
        }
        app.insert_resource(PlayerLocation {
            position: Some(Vec3::new(distance, 0.0, 0.0)),
            zone: None,
        });

        step(&mut app, 3);

        let near = app.world().resource::<NodeIndex>().by_name("near")[0];
        let far = app.world().resource::<NodeIndex>().by_name("far")[0];
        assert_eq!(
            app.world().get::<NodeActive>(near).expect("spawned").0,
            expected_near,
            "at {distance} m the near branch should be {expected_near}"
        );
        assert_eq!(
            app.world().get::<NodeActive>(far).expect("spawned").0,
            !expected_near,
            "at {distance} m the else branch should be {}",
            !expected_near
        );
    }
}

/// `Execution::ByRange` is a squared band the player has to be inside.
#[test]
fn a_by_range_definition_waits_for_the_player() {
    let mut trigger = def("vtol1", "m1_exit_trigger");
    trigger.activation = AnimActivation::OnStartup;
    // 30.25 is 5.5 metres squared: the real exit trigger's range.
    trigger.execution = Execution::ByRange {
        min_sq: 0.0,
        max_sq: 30.25,
    };
    trigger.sequences = vec![initial("fire", vec![now(set_active("flag", false))])];

    let mut app = app_with(program(vec![trigger]));
    spawn_nodes(&mut app, &["vtol1", "flag"]);
    spawn_instances(&mut app);
    app.insert_resource(PlayerLocation {
        position: Some(Vec3::new(20.0, 0.0, 0.0)),
        zone: None,
    });

    step(&mut app, 5);
    let flag = app.world().resource::<NodeIndex>().by_name("flag")[0];
    assert!(
        app.world().get::<NodeActive>(flag).expect("spawned").0,
        "20 m is outside 5.5 m, so nothing fires"
    );

    // Drive up to it. 5 m is inside; 25 (the squared distance) is under 30.25.
    app.insert_resource(PlayerLocation {
        position: Some(Vec3::new(5.0, 0.0, 0.0)),
        zone: None,
    });
    step(&mut app, 3);

    assert!(
        !app.world().get::<NodeActive>(flag).expect("spawned").0,
        "inside 5.5 m the trigger fires"
    );
}

/// An unhandled event kind is skipped rather than stopping the sequence.
#[test]
fn an_unsupported_event_does_not_stall_the_sequence() {
    let mut mixed = def("prop", "mixed");
    mixed.activation = AnimActivation::OnStartup;
    mixed.sequences = vec![initial(
        "go",
        vec![
            // A kind nothing handles.
            now(EventDataDesc::Unknown(
                "NoSuchEvent".to_string(),
                serde_json::json!({ "node": "prop" }),
            )),
            now(set_active("after", false)),
        ],
    )];

    let mut app = app_with(program(vec![mixed]));
    spawn_nodes(&mut app, &["prop", "after"]);
    spawn_instances(&mut app);

    step(&mut app, 3);

    let after = app.world().resource::<NodeIndex>().by_name("after")[0];
    assert!(
        !app.world().get::<NodeActive>(after).expect("spawned").0,
        "the event after an unsupported one still runs"
    );
}

/// A presentation event's `at_node` reaches the sound layer as an entity,
/// resolved in the definition's own scope before the level's.
#[test]
fn a_sound_event_carries_its_node_resolved_in_scope() {
    let mut noisy = def("tank", "noisy");
    noisy.activation = AnimActivation::OnStartup;
    noisy.sequences = vec![initial(
        "go",
        vec![
            now(EventDataDesc::Presentation {
                kind: "Sound",
                value: serde_json::json!({
                    "name": "snd_exp_hit1",
                    "at_node": { "name": "turret", "pos": { "x": 0.0, "y": 1.0, "z": 0.0 } }
                }),
            }),
            now(EventDataDesc::Presentation {
                kind: "Sound",
                value: serde_json::json!({ "name": "snd_plain", "at_node": null }),
            }),
        ],
    )];

    let mut app = app_with(program(vec![noisy]));
    // Two `turret`s level-wide; the definition's scope names the second.
    let nodes = spawn_nodes(&mut app, &["tank", "turret", "turret"]);
    spawn_instances(&mut app);
    app.insert_resource(AnimScopes(vec![std::collections::HashMap::from([(
        "turret".to_string(),
        nodes[2],
    )])]));

    let outputs = step_collecting::<AnimOutput>(&mut app, 3);
    let sounds: Vec<_> = outputs.iter().filter(|o| o.kind == "Sound").collect();
    assert_eq!(sounds.len(), 2);
    assert_eq!(sounds[0].at, Some(nodes[0]), "`at` stays the root object");
    assert_eq!(
        sounds[0].node,
        Some(nodes[2]),
        "the scoped turret, not the first"
    );
    assert_eq!(sounds[1].node, None, "no `at_node`, no node");
}

/// A `required` prerequisite gates arming until it holds.
#[test]
fn a_required_prerequisite_gates_activation() {
    let mut gated = def("door", "open_door");
    gated.activation = AnimActivation::OnStartup;
    gated.prereqs = vec![PrereqDesc::Object {
        name: "window".to_string(),
        active: false,
        required: true,
    }];
    gated.sequences = vec![initial("open", vec![now(set_active("door", false))])];

    let mut app = app_with(program(vec![gated]));
    spawn_nodes(&mut app, &["door", "window"]);
    spawn_instances(&mut app);

    // The window is still whole, so the door stays shut.
    step(&mut app, 5);
    let door = app.world().resource::<NodeIndex>().by_name("door")[0];
    assert!(
        app.world().get::<NodeActive>(door).expect("spawned").0,
        "the prerequisite is not met yet"
    );

    // Break it.
    let window = app.world().resource::<NodeIndex>().by_name("window")[0];
    app.world_mut()
        .get_mut::<NodeActive>(window)
        .expect("spawned")
        .0 = false;
    step(&mut app, 3);

    assert!(
        !app.world().get::<NodeActive>(door).expect("spawned").0,
        "the door opens once the window is broken"
    );
}

/// Call another definition by name.
fn call_anim(name: &str) -> EventDataDesc {
    EventDataDesc::CallAnimation {
        name: name.to_string(),
        operand_node: None,
        wait_for_completion: false,
        parameters: None,
    }
}

/// A call whose prerequisites fail is ignored, not remembered, and each call
/// sees the ones made before it.
///
/// m1's `rocket_count` runs on every rocket kill and calls `three_rocket`,
/// `two_rocket`, `one_rocket` in that order; an `Animation` prerequisite
/// holds once that definition has started, so each kill passes exactly one.
#[test]
fn a_call_honours_prerequisites_so_a_counter_counts() {
    let mut count = def("horizon", "rocket_count");
    count.reset_time = ResetTime::Immediate;
    count.sequences = vec![initial(
        "call_voice",
        vec![
            now(call_anim("three_rocket")),
            now(call_anim("two_rocket")),
            now(call_anim("one_rocket")),
        ],
    )];
    let voice = |anim: &str, node: &str, after: &[&str]| {
        let mut voice = def(node, anim);
        voice.prereqs = after
            .iter()
            .map(|name| PrereqDesc::Animation {
                name: name.to_string(),
                required: true,
            })
            .collect();
        voice.sequences = vec![initial("say", vec![now(set_active(node, false))])];
        voice
    };
    let defs = vec![
        count,
        voice("one_rocket", "site1", &[]),
        voice("two_rocket", "site2", &["one_rocket"]),
        voice("three_rocket", "site3", &["one_rocket", "two_rocket"]),
    ];

    let mut app = app_with(program(defs));
    spawn_nodes(&mut app, &["horizon", "site1", "site2", "site3"]);
    spawn_instances(&mut app);
    let said = |app: &App| -> Vec<bool> {
        ["site1", "site2", "site3"]
            .iter()
            .map(|name| {
                let node = app.world().resource::<NodeIndex>().by_name(name)[0];
                !app.world().get::<NodeActive>(node).expect("spawned").0
            })
            .collect()
    };

    for kills in 1..=3 {
        app.world_mut()
            .resource_mut::<Messages<CallAnim>>()
            .write(CallAnim::new("rocket_count"));
        step(&mut app, 3);
        let expected: Vec<bool> = (1..=3).map(|site| site <= kills).collect();
        assert_eq!(said(&app), expected, "after {kills} kill(s)");
    }
}

/// A called definition with a `ByRange` condition is armed by the call and
/// starts only when the player comes in range: m1's `m1_exit_trigger`.
#[test]
fn a_called_by_range_definition_waits_for_the_player() {
    let mut trigger = def("vtol1", "m1_exit_trigger");
    trigger.execution = Execution::ByRange {
        min_sq: 0.0,
        max_sq: 30.25,
    };
    trigger.sequences = vec![initial("fire", vec![now(set_active("flag", false))])];

    let mut app = app_with(program(vec![trigger]));
    spawn_nodes(&mut app, &["vtol1", "flag"]);
    spawn_instances(&mut app);
    app.insert_resource(PlayerLocation {
        position: Some(Vec3::new(20.0, 0.0, 0.0)),
        zone: None,
    });

    app.world_mut()
        .resource_mut::<Messages<CallAnim>>()
        .write(CallAnim::new("m1_exit_trigger"));
    step(&mut app, 5);
    let flag = app.world().resource::<NodeIndex>().by_name("flag")[0];
    assert!(
        app.world().get::<NodeActive>(flag).expect("spawned").0,
        "called, but 20 m is outside 5.5 m"
    );

    app.insert_resource(PlayerLocation {
        position: Some(Vec3::new(5.0, 0.0, 0.0)),
        zone: None,
    });
    step(&mut app, 3);
    assert!(
        !app.world().get::<NodeActive>(flag).expect("spawned").0,
        "inside 5.5 m the called trigger fires"
    );
}

/// `ByZone` compares the player's zone with the definition root's.
#[test]
fn a_by_zone_definition_waits_for_the_players_zone() {
    let mut ambient = def("dock", "ocean_sound_dock");
    ambient.activation = AnimActivation::OnStartup;
    ambient.execution = Execution::ByZone;
    ambient.sequences = vec![initial("play", vec![now(set_active("speaker", false))])];

    let mut app = app_with(program(vec![ambient]));
    spawn_nodes(&mut app, &["dock", "speaker"]);
    let dock = app.world().resource::<NodeIndex>().by_name("dock")[0];
    app.world_mut().entity_mut(dock).insert(NodeZone(14));
    spawn_instances(&mut app);

    app.insert_resource(PlayerLocation {
        position: Some(Vec3::ZERO),
        zone: Some(3),
    });
    step(&mut app, 4);
    let speaker = app.world().resource::<NodeIndex>().by_name("speaker")[0];
    assert!(
        app.world().get::<NodeActive>(speaker).expect("spawned").0,
        "the player is in another zone"
    );

    app.insert_resource(PlayerLocation {
        position: Some(Vec3::ZERO),
        zone: Some(14),
    });
    step(&mut app, 3);

    assert!(
        !app.world().get::<NodeActive>(speaker).expect("spawned").0,
        "the ambient runs in its own zone"
    );
}

/// A node re-asserting the state it already has announces nothing, which is
/// what keeps an objective from completing on a no-op.
#[test]
fn only_a_real_change_is_announced() {
    let mut noop = def("prop", "noop");
    noop.activation = AnimActivation::OnStartup;
    // Every node starts active, so setting it active again changes nothing.
    noop.sequences = vec![initial("go", vec![now(set_active("lamp", true))])];

    let mut app = app_with(program(vec![noop]));
    spawn_nodes(&mut app, &["prop", "lamp"]);
    spawn_instances(&mut app);

    let events = step_collecting::<NodeActiveChanged>(&mut app, 3);

    assert!(
        active_changes(&events).is_empty(),
        "re-asserting a state is not a change"
    );
}

// ---------------------------------------------------------------------------
// The objective machine
// ---------------------------------------------------------------------------

fn objective(index: u8, active: bool, completion: Completion) -> ObjectiveDef {
    ObjectiveDef {
        index,
        picture: format!("pic{index}"),
        summary_key: format!("MSG_O{index}_SUMMARY"),
        text_key: format!("MSG_O{index}_OBJ"),
        complete_key: format!("MSG_O{index}_COMP"),
        initially_active: active,
        completion,
        read_sound: None,
        hint: None,
    }
}

fn node_condition(name: &str) -> Completion {
    Completion::NodeStateChanged {
        node: name.to_string(),
        child: None,
    }
}

/// m1's shape: one active objective, two that follow, and an exit.
fn m1_script() -> MissionScript {
    MissionScript {
        objectives: vec![
            objective(0, true, node_condition("frcoff")),
            objective(1, false, node_condition("nb_force")),
            objective(2, false, node_condition("rocket_gull")),
            objective(3, true, Completion::ExitAnim),
        ],
        read_time: 8.0,
        review_delay: 4.0,
        review_sound: "snd_review".to_string(),
        objective_sound: "snd_objective".to_string(),
        new_game_anim: "start_single_player".to_string(),
        load_game_anim: "load_control".to_string(),
        movers: Vec::new(),
        death_anim: "bft_00".to_string(),
        final_mission: false,
    }
}

/// A mission app: the script, the state and the baseline the nodes imply.
fn mission_app(script: MissionScript, nodes: &[&str]) -> App {
    let mut app = app_with(program(Vec::new()));
    let entities = spawn_nodes(&mut app, nodes);
    spawn_instances(&mut app);

    let baseline = ObjectiveBaseline {
        states: script
            .objectives
            .iter()
            .map(|objective| match &objective.completion {
                Completion::NodeStateChanged { node, .. } => nodes
                    .iter()
                    .position(|name| name == node)
                    .map(|index| (entities[index], true)),
                _ => None,
            })
            .collect(),
    };

    app.insert_resource(MissionState::new(LevelId(1), &script));
    app.insert_resource(script);
    app.insert_resource(baseline);
    app
}

/// Flip a node, the way an `ObjectActiveState` event does.
fn flip(app: &mut App, name: &str) {
    let entity = app.world().resource::<NodeIndex>().by_name(name)[0];
    let was = app.world().get::<NodeActive>(entity).expect("spawned").0;
    app.world_mut()
        .get_mut::<NodeActive>(entity)
        .expect("spawned")
        .0 = !was;
    app.world_mut()
        .resource_mut::<Messages<NodeActiveChanged>>()
        .write(NodeActiveChanged {
            node: entity,
            name: name.to_string(),
            root: None,
            active: !was,
        });
}

fn objective_states(app: &App) -> Vec<ObjectiveState> {
    app.world().resource::<MissionState>().objectives.clone()
}

/// The state machine: the file's own initial states, and a node flip
/// completing an objective whatever state it was in.
///
/// An `INACTIVE` objective is one the briefing does not mention until its
/// animation fires; the same flip that reveals it completes it, which is what
/// the shipped data says (m2's objective 1 is `INACTIVE` with nothing before
/// it to activate it).
#[test]
fn an_inactive_objective_completes_on_its_own_node_flip() {
    let mut app = mission_app(m1_script(), &["frcoff", "nb_force", "rocket_gull"]);

    assert_eq!(
        objective_states(&app),
        vec![
            ObjectiveState::Active,
            ObjectiveState::Inactive,
            ObjectiveState::Inactive,
            // The exit objective is inactive until the rest are done.
            ObjectiveState::Active,
        ],
        "the file's own initial states"
    );

    // Out of order, and on an objective the briefing has not shown yet.
    flip(&mut app, "rocket_gull");
    step(&mut app, 2);

    assert_eq!(
        objective_states(&app)[2],
        ObjectiveState::Complete,
        "an INACTIVE objective completes when its node flips"
    );
    assert_eq!(
        objective_states(&app)[1],
        ObjectiveState::Inactive,
        "and completing it does not touch its neighbours"
    );

    flip(&mut app, "frcoff");
    flip(&mut app, "nb_force");
    step(&mut app, 2);

    assert_eq!(
        objective_states(&app)[..3],
        [
            ObjectiveState::Complete,
            ObjectiveState::Complete,
            ObjectiveState::Complete
        ]
    );
}

/// The exit objective opens once the rest are done, and not before.
#[test]
fn the_exit_objective_opens_when_the_others_are_complete() {
    let mut script = m1_script();
    // The file gives m1's exit no state word; this test is about the engine
    // revealing it, so it starts hidden.
    script.objectives[3].initially_active = false;

    let mut app = mission_app(script, &["frcoff", "nb_force", "rocket_gull"]);

    step(&mut app, 2);
    assert_eq!(
        objective_states(&app)[3],
        ObjectiveState::Inactive,
        "there is still work to do"
    );

    for node in ["frcoff", "nb_force", "rocket_gull"] {
        flip(&mut app, node);
    }
    step(&mut app, 3);

    assert_eq!(
        objective_states(&app)[3],
        ObjectiveState::Active,
        "with the rest done, the player may leave"
    );
}

/// The polarity is per objective: a node switched *off* completes just as one
/// switched on does, because the rule is "differs from the start".
#[test]
fn an_objective_completes_whichever_way_its_node_moves() {
    for start_active in [true, false] {
        let script = MissionScript {
            objectives: vec![objective(0, true, node_condition("gate"))],
            ..m1_script()
        };
        let mut app = app_with(program(Vec::new()));
        let entities = spawn_nodes(&mut app, &["gate"]);
        spawn_instances(&mut app);
        app.world_mut()
            .get_mut::<NodeActive>(entities[0])
            .expect("spawned")
            .0 = start_active;
        app.insert_resource(ObjectiveBaseline {
            states: vec![Some((entities[0], start_active))],
        });
        app.insert_resource(MissionState::new(LevelId(1), &script));
        app.insert_resource(script);

        flip(&mut app, "gate");
        step(&mut app, 2);

        assert_eq!(
            objective_states(&app)[0],
            ObjectiveState::Complete,
            "starting {start_active}, the flip completes it"
        );
    }
}

/// A node returning to its starting state does not complete the objective.
#[test]
fn a_node_that_returns_to_its_start_state_completes_nothing() {
    let script = MissionScript {
        objectives: vec![objective(0, true, node_condition("gate"))],
        ..m1_script()
    };
    let mut app = app_with(program(Vec::new()));
    let entities = spawn_nodes(&mut app, &["gate"]);
    spawn_instances(&mut app);
    app.insert_resource(ObjectiveBaseline {
        states: vec![Some((entities[0], true))],
    });
    app.insert_resource(MissionState::new(LevelId(1), &script));
    app.insert_resource(script);

    // A definition announcing the state it already had.
    app.world_mut()
        .resource_mut::<Messages<NodeActiveChanged>>()
        .write(NodeActiveChanged {
            node: entities[0],
            name: "gate".to_string(),
            root: None,
            active: true,
        });
    step(&mut app, 2);

    assert_eq!(objective_states(&app)[0], ObjectiveState::Active);
}

/// `Callback 99` from the exit animation wins the mission and completes the
/// exit objective with it.
#[test]
fn the_exit_callback_wins_the_mission() {
    let mut app = mission_app(m1_script(), &["frcoff", "nb_force", "rocket_gull"]);
    app.insert_resource(Progress::default());

    app.world_mut()
        .resource_mut::<Messages<AnimCallback>>()
        .write(AnimCallback {
            def: "m1_exit_animation".to_string(),
            value: MISSION_END_CALLBACK,
        });
    let ended = step_collecting::<MissionEnded>(&mut app, 2);

    let state = app.world().resource::<MissionState>();
    assert_eq!(state.outcome, Some(Outcome::Won));
    assert_eq!(state.objectives[3], ObjectiveState::Complete);
    assert_eq!(ended, vec![MissionEnded(Outcome::Won)]);
}

/// `Callback 2` also fires in every `mN_start_animation`, so it must not end
/// the mission during the intro.
#[test]
fn the_intro_callbacks_do_not_end_the_mission() {
    let mut app = mission_app(m1_script(), &["frcoff", "nb_force", "rocket_gull"]);

    for value in [1, 2, 10, 14, 15, 20] {
        app.world_mut()
            .resource_mut::<Messages<AnimCallback>>()
            .write(AnimCallback {
                def: "m1_start_animation".to_string(),
                value,
            });
    }
    step(&mut app, 2);

    assert_eq!(
        app.world().resource::<MissionState>().outcome,
        None,
        "only 99 wins"
    );
}

/// The mission's entry animation is called once, on the first tick, and it is
/// the one the file names.
#[test]
fn the_mission_calls_its_own_entry_animation() {
    let script = MissionScript {
        new_game_anim: "m4_start_animation".to_string(),
        ..m1_script()
    };
    let mut app = mission_app(script, &["frcoff", "nb_force", "rocket_gull"]);

    let calls: Vec<String> = step_collecting::<CallAnim>(&mut app, 3)
        .into_iter()
        .map(|call| call.name)
        .collect();

    assert_eq!(
        calls
            .iter()
            .filter(|name| *name == "m4_start_animation")
            .count(),
        1,
        "called once, not every tick: {calls:?}"
    );
}

/// An objective that watches a pickup completes when it is collected.
#[test]
fn a_pickup_objective_completes_on_collection() {
    let script = MissionScript {
        objectives: vec![objective(
            0,
            true,
            Completion::PickupCollected {
                instance: "pu03700".to_string(),
            },
        )],
        ..m1_script()
    };
    let mut app = app_with(program(Vec::new()));
    spawn_nodes(&mut app, &["anything"]);
    spawn_instances(&mut app);
    app.insert_resource(ObjectiveBaseline { states: vec![None] });
    app.insert_resource(MissionState::new(LevelId(1), &script));
    app.insert_resource(script);

    let player = app.world_mut().spawn_empty().id();
    app.world_mut()
        .resource_mut::<Messages<bft_sim::pickups::PickedUp>>()
        .write(bft_sim::pickups::PickedUp {
            by: player,
            kind: "PUP_AMPHIB".to_string(),
            amount: 1,
            instance: "pu03700".to_string(),
        });
    step(&mut app, 2);

    assert_eq!(objective_states(&app)[0], ObjectiveState::Complete);
}

/// The clock advances with the fixed tick, which is what every offset counts
/// against.
#[test]
fn the_anim_clock_follows_the_fixed_tick() {
    let mut app = app_with(program(Vec::new()));
    spawn_nodes(&mut app, &["x"]);
    spawn_instances(&mut app);

    step(&mut app, SIM_HZ as usize);

    let clock = app.world().resource::<AnimClock>().0;
    assert!(
        (clock - 1.0).abs() < 0.05,
        "a second of ticks is about a second: {clock}"
    );
}

/// The core handlers are registered, and the presentation ones beside them: the
/// eight mission logic needs are all there whatever else is.
#[test]
fn the_core_handlers_are_the_eight_mission_logic_needs() {
    let core = bft_sim::anim::core_handlers();
    let mut kinds: Vec<&str> = core.0.keys().copied().collect();
    kinds.sort();

    assert_eq!(
        kinds,
        vec![
            "CallAnimation",
            "CallSequence",
            "Callback",
            "InvalidateAnimation",
            "ObjectActiveState",
            "ResetAnimation",
            "StopAnimation",
            "StopSequence",
        ]
    );

    let app = app_with(program(Vec::new()));
    let installed = app.world().resource::<EventHandlers>();
    for kind in kinds {
        assert!(installed.get(kind).is_some(), "{kind} is installed");
    }
}

/// A `CallAnim` message from outside the VM starts the animation.
///
/// This is how the mission's entry animation, a form change's transition and
/// an enemy's death animation all reach the VM. Writing the message and
/// having nothing read it is a silent failure — the mission simply never
/// starts — so it is worth a test of its own.
#[test]
fn a_call_from_outside_the_vm_starts_an_animation() {
    let mut called = def("prop", "prop_spin");
    called.sequences = vec![initial("spin", vec![now(set_active("blade", false))])];

    let mut app = app_with(program(vec![called]));
    spawn_nodes(&mut app, &["prop", "blade"]);
    spawn_instances(&mut app);

    // Nothing has called it, so nothing runs.
    step(&mut app, 3);
    let blade = app.world().resource::<NodeIndex>().by_name("blade")[0];
    assert!(
        app.world().get::<NodeActive>(blade).expect("spawned").0,
        "an OnCall definition waits to be called"
    );

    app.world_mut()
        .resource_mut::<Messages<CallAnim>>()
        .write(CallAnim::new("prop_spin"));
    step(&mut app, 3);

    assert!(
        !app.world().get::<NodeActive>(blade).expect("spawned").0,
        "the call started it"
    );
}

/// A chain of `CallAnimation`s runs in one tick, not one link per tick.
///
/// The VM applies its own calls from the stepping pass's outbox; only calls
/// from outside go through the message bus. A chain that took a tick per link
/// would stretch m1's six-call opening over six ticks.
#[test]
fn an_internal_call_chain_does_not_cost_a_tick_per_link() {
    let mut first = def("a", "first");
    first.activation = AnimActivation::OnStartup;
    first.sequences = vec![initial(
        "go",
        vec![now(EventDataDesc::CallAnimation {
            name: "second".to_string(),
            operand_node: None,
            wait_for_completion: false,
            parameters: None,
        })],
    )];

    let mut second = def("b", "second");
    second.sequences = vec![initial("go", vec![now(set_active("end", false))])];

    let mut app = app_with(program(vec![first, second]));
    spawn_nodes(&mut app, &["a", "b", "end"]);
    spawn_instances(&mut app);

    // Two ticks: one to arm and run the first, one for the second. A chain
    // routed through the message bus would need three.
    step(&mut app, 2);

    let end = app.world().resource::<NodeIndex>().by_name("end")[0];
    assert!(
        !app.world().get::<NodeActive>(end).expect("spawned").0,
        "the chain reached its end"
    );
}

/// A node change names the definition's scope, so a log or an objective can
/// tell m4's 186 `healthy` nodes apart.
#[test]
fn a_node_change_carries_the_definitions_scope() {
    let mut flipper = def("fntrocket", "blow_it_up");
    flipper.anim_root_name = "healthy".to_string();
    flipper.activation = AnimActivation::OnStartup;
    flipper.sequences = vec![initial("go", vec![now(set_active("target", false))])];

    let mut app = app_with(program(vec![flipper]));
    spawn_nodes(&mut app, &["fntrocket", "target"]);
    spawn_instances(&mut app);

    let events = step_collecting::<NodeActiveChanged>(&mut app, 3);

    let change = events
        .iter()
        .find(|event| event.name == "target")
        .expect("the node changed");
    assert_eq!(change.root.as_deref(), Some("healthy"));
}

/// The level's initial node states come from the definitions' `reset_state`.
///
/// `gamez` has `active: true` on every node of every level, so a level
/// spawned without this shows its destroyed hulks on top of the intact
/// versions. 3108 of the game's 4798 definitions carry one.
#[test]
fn reset_states_set_the_levels_initial_visibility() {
    use bft_sim::anim::apply_initial_node_states;

    let mut prop = def("crate1", "smash_crate");
    prop.reset_state = vec![
        now(set_active("healthy", true)),
        now(set_active("destroyed", false)),
        now(set_active("piece1", false)),
    ];

    let mut app = app_with(program(vec![prop]));
    spawn_nodes(&mut app, &["crate1", "healthy", "destroyed", "piece1"]);
    spawn_instances(&mut app);

    // As `gamez` has it: everything on, debris and all.
    for node in ["healthy", "destroyed", "piece1"] {
        let entity = app.world().resource::<NodeIndex>().by_name(node)[0];
        assert!(app.world().get::<NodeActive>(entity).expect("spawned").0);
    }

    let hidden = app
        .world_mut()
        .run_system_cached(
            |program: Res<AnimProgram>,
             scopes: Res<AnimScopes>,
             nodes: Res<NodeIndex>,
             mut actives: Query<&mut NodeActive>| {
                apply_initial_node_states(&program.0, &scopes, &nodes, &mut actives)
            },
        )
        .expect("runs");

    assert_eq!(hidden, 2, "the two that should start off");
    let state = |node: &str| {
        let entity = app.world().resource::<NodeIndex>().by_name(node)[0];
        app.world().get::<NodeActive>(entity).expect("spawned").0
    };
    assert!(state("healthy"), "the intact model stays visible");
    assert!(!state("destroyed"), "the wreck starts hidden");
    assert!(!state("piece1"), "so does the debris");
}

// ---------------------------------------------------------------------------
// Copies
// ---------------------------------------------------------------------------

/// A prototype `fx.flt/piece1` under a level root, and one instance for a
/// definition rooted at it. Returns (prototype root, piece1).
fn spawn_prototype(app: &mut App, fx: AnimDefDesc) -> (Entity, Entity) {
    use bft_sim::ai::Template;
    use bft_sim::level::LevelRoot;

    let level = app
        .world_mut()
        .spawn((LevelRoot, Transform::default(), GlobalTransform::default()))
        .id();
    let root = app
        .world_mut()
        .spawn((
            NodeName("fx.flt".into()),
            NodeActive(true),
            Template,
            Transform::default(),
            GlobalTransform::default(),
            ChildOf(level),
        ))
        .id();
    let piece = app
        .world_mut()
        .spawn((
            NodeName("piece1".into()),
            NodeActive(true),
            Transform::from_xyz(0.0, 1.0, 0.0),
            GlobalTransform::from_xyz(0.0, 1.0, 0.0),
            ChildOf(root),
        ))
        .id();

    let mut index = NodeIndex::default();
    index.insert(0, "fx.flt", root);
    index.insert(1, "piece1", piece);
    app.insert_resource(index);

    let mut scope = std::collections::HashMap::new();
    scope.insert("fx.flt".to_string(), root);
    scope.insert("piece1".to_string(), piece);
    app.insert_resource(AnimScopes(vec![scope]));

    let mut instance = AnimInstance::new(0, fx.activation, Some(root));
    instance.prototype = true;
    app.world_mut().spawn((instance, ChildOf(level)));
    (root, piece)
}

/// Every copy of `piece1`: the nodes of that name that are not the
/// prototype's.
fn copied_pieces(app: &mut App, prototype: Entity) -> Vec<(Entity, bool, Vec3)> {
    let mut query = app
        .world_mut()
        .query::<(Entity, &NodeName, &NodeActive, &GlobalTransform)>();
    query
        .iter(app.world())
        .filter(|(entity, name, ..)| name.0 == "piece1" && *entity != prototype)
        .map(|(entity, _, active, global)| (entity, active.0, global.translation()))
        .collect()
}

/// An effect called at two places plays twice, once at each, and each copy
/// goes when it is done. The prototype is never touched.
///
/// One instance per definition could only move the one prototype from shot
/// to shot: m1's `lg_impact.flt` at every shell, 83 enemies' death anims.
#[test]
fn a_placed_call_on_a_prototype_plays_a_copy_there() {
    let mut fx = def("fx.flt", "fx.flt");
    fx.nodes = vec!["piece1".into()];
    fx.sequences = vec![initial(
        "burst",
        vec![at(OffsetFrom::Animation, 0.5, set_active("piece1", false))],
    )];

    let mut app = app_with(program(vec![fx.clone()]));
    let (_, piece) = spawn_prototype(&mut app, fx);
    let target = |app: &mut App, at: Vec3| {
        app.world_mut()
            .spawn((
                Transform::from_translation(at),
                GlobalTransform::from_translation(at),
            ))
            .id()
    };
    let a = target(&mut app, Vec3::new(10.0, 0.0, 0.0));
    let b = target(&mut app, Vec3::new(-20.0, 0.0, 5.0));

    for node in [a, b] {
        app.world_mut()
            .resource_mut::<Messages<CallAnim>>()
            .write(CallAnim::at("fx.flt", node, Vec3::ZERO));
    }
    step(&mut app, 2);

    let mut copies = copied_pieces(&mut app, piece);
    copies.sort_by(|x, y| x.2.x.total_cmp(&y.2.x));
    assert_eq!(copies.len(), 2, "one copy per call");
    // `piece1` sits a metre above its root, wherever the root was placed.
    assert!(
        copies[0].2.distance(Vec3::new(-20.0, 1.0, 5.0)) < 1e-4,
        "{copies:?}"
    );
    assert!(
        copies[1].2.distance(Vec3::new(10.0, 1.0, 0.0)) < 1e-4,
        "{copies:?}"
    );
    assert!(copies.iter().all(|copy| copy.1), "not switched off yet");

    step(&mut app, (SIM_HZ * 0.6) as usize);
    let copies = copied_pieces(&mut app, piece);
    assert!(copies.is_empty(), "a finished copy goes: {copies:?}");
    assert!(
        app.world().get::<NodeActive>(piece).expect("spawned").0,
        "the prototype itself never plays"
    );
    let mut instances = app.world_mut().query::<&AnimInstance>();
    assert_eq!(
        instances.iter(app.world()).count(),
        1,
        "only the prototype's instance is left"
    );
}

/// A call made *on* a node starts only the definition rooted there, not
/// every definition of that name.
#[test]
fn a_call_on_a_node_starts_only_the_definition_rooted_there() {
    let destroy = |turret: &str, part: &str| {
        let mut destroy = def(turret, "destroy_turret");
        destroy.sequences = vec![initial("fall", vec![now(set_active(part, false))])];
        destroy
    };
    let mut app = app_with(program(vec![
        destroy("tur_101", "healthy_101"),
        destroy("tur_102", "healthy_102"),
    ]));
    spawn_nodes(
        &mut app,
        &["tur_101", "healthy_101", "tur_102", "healthy_102"],
    );
    spawn_instances(&mut app);
    let node = |app: &App, name: &str| app.world().resource::<NodeIndex>().by_name(name)[0];

    let turret = node(&app, "tur_102");
    app.world_mut()
        .resource_mut::<Messages<CallAnim>>()
        .write(CallAnim::on("destroy_turret", turret));
    step(&mut app, 3);

    let active = |app: &App, name: &str| {
        app.world()
            .get::<NodeActive>(node(app, name))
            .expect("spawned")
            .0
    };
    assert!(
        active(&app, "healthy_101"),
        "the other turret is left alone"
    );
    assert!(!active(&app, "healthy_102"), "the destroyed one falls");
}

/// A call on a node no definition of that name is rooted at plays the
/// definition with the node as its operand.
///
/// Each level has one `destroy_turret`, rooted at its first turret; every
/// other turret's `DESTROY_ANIM` has to play it on itself.
#[test]
fn a_call_on_another_node_plays_the_definition_on_it() {
    let mut destroy = def("tur_101", "destroy_turret");
    destroy.objects = vec!["healthy".into()];
    destroy.sequences = vec![initial("fall", vec![now(set_active("healthy", false))])];
    let mut app = app_with(program(vec![destroy]));

    let turret = |app: &mut App, name: &str| {
        let root = app
            .world_mut()
            .spawn((
                NodeName(name.into()),
                NodeActive(true),
                Transform::default(),
            ))
            .id();
        let healthy = app
            .world_mut()
            .spawn((
                NodeName("healthy".into()),
                NodeActive(true),
                Transform::default(),
                ChildOf(root),
            ))
            .id();
        (root, healthy)
    };
    let (first, first_healthy) = turret(&mut app, "tur_101");
    let (other, other_healthy) = turret(&mut app, "tur_102");
    let mut index = NodeIndex::default();
    index.insert(0, "tur_101", first);
    index.insert(1, "healthy", first_healthy);
    index.insert(2, "tur_102", other);
    index.insert(3, "healthy", other_healthy);
    app.insert_resource(index);
    let mut scope = std::collections::HashMap::new();
    scope.insert("healthy".to_string(), first_healthy);
    app.insert_resource(AnimScopes(vec![scope]));
    app.world_mut()
        .spawn(AnimInstance::new(0, AnimActivation::OnCall, Some(first)));

    app.world_mut()
        .resource_mut::<Messages<CallAnim>>()
        .write(CallAnim::on("destroy_turret", other));
    step(&mut app, 3);

    let active = |app: &App, node: Entity| app.world().get::<NodeActive>(node).expect("spawned").0;
    assert!(
        !active(&app, other_healthy),
        "the turret it was called on falls"
    );
    assert!(
        active(&app, first_healthy),
        "the one it is rooted at does not"
    );
}
