//! The cinematics' checks against the shipped levels: the findings the
//! cinematics are built on, and the intros played on the real hierarchy.
//!
//! They skip (not fail) when `BFT_DATA_DIR` is unset or the cache has not
//! been built. Only names and numbers docs/data/anims.md and these tests
//! publish are asserted; no game data enters the repository.

use std::collections::{BTreeMap, BTreeSet};

use bevy::prelude::*;
use bft_assets::anim::{AnimDefDesc, AnimSet, EventDataDesc, parse_anim_set};
use bft_assets::axes::AxisConvention;
use bft_assets::level::{LevelData, parse_gamez};
use bft_sim::anim::handlers::si_pose;
use bft_sim::anim::{
    AnimInstance, AnimProgram, AnimScopes, CallAnim, PlayerLocation, apply_initial_node_states,
    spawn_anim_instances,
};
use bft_sim::cinematic::{
    CallbackMeaning, PlayerControl, SkipCinematic, callback_meaning, hand_off_pose,
    is_entry_animation,
};
use bft_sim::level::{LevelRoot, NodeActive, NodeIndex, NodeName};
use bft_sim::schedule::SIM_HZ;

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

fn anim(cache: &std::path::Path, level: u16) -> Option<AnimSet> {
    let bytes = std::fs::read(cache.join(format!("anim/m{level}-anim.anim.zip"))).ok()?;
    Some(parse_anim_set(&bytes).expect("a real anim archive parses"))
}

fn gamez(cache: &std::path::Path, level: u16) -> Option<LevelData> {
    let bytes = std::fs::read(cache.join(format!("gamez/m{level}-gamez.gamez.zip"))).ok()?;
    Some(parse_gamez(&bytes, "gamez.zip").expect("a real gamez archive parses"))
}

fn def<'a>(set: &'a AnimSet, name: &str) -> &'a AnimDefDesc {
    let index = *set.lookup(name).first().expect("the definition exists");
    &set.defs[index]
}

/// Every event of a definition's sequences, reset state excluded.
fn sequence_events(def: &AnimDefDesc) -> impl Iterator<Item = &EventDataDesc> {
    def.sequences
        .iter()
        .flat_map(|seq| seq.events.iter())
        .map(|event| &event.data)
}

/// The absolute `ObjectTranslateState` a reset gives a node.
fn reset_translation(def: &AnimDefDesc, node: &str) -> Option<Vec3> {
    def.reset_state.iter().find_map(|event| match &event.data {
        EventDataDesc::Presentation {
            kind: "ObjectTranslateState",
            value,
        } if value.get("node").and_then(|n| n.as_str()) == Some(node)
            && value.get("relative").and_then(|r| r.as_bool()) == Some(false) =>
        {
            let state = value.get("state")?;
            let n = |key: &str| state.get(key)?.as_f64().map(|v| v as f32);
            Some(Vec3::new(n("x")?, n("y")?, n("z")?))
        }
        _ => None,
    })
}

/// The SI scripts a definition's sequences play on a node, in order.
fn si_scripts_on(def: &AnimDefDesc, node: &str) -> Vec<usize> {
    sequence_events(def)
        .filter_map(|data| match data {
            EventDataDesc::Presentation {
                kind: "ObjectMotionSiScript",
                value,
            } if value.get("name").and_then(|n| n.as_str()) == Some(node) => value
                .get("index")
                .and_then(|i| i.as_u64())
                .map(|i| i as usize),
            _ => None,
        })
        .collect()
}

fn script_end(set: &AnimSet, index: usize) -> Vec3 {
    let script = &set.si_scripts[index];
    si_pose(script, script.duration()).0.expect("a translation")
}

fn script_start(set: &AnimSet, index: usize) -> Vec3 {
    si_pose(&set.si_scripts[index], 0.0)
        .0
        .expect("a translation")
}

// ---------------------------------------------------------------------------
// What the data says about the cinematics
// ---------------------------------------------------------------------------

/// Exactly six of the game's definitions carry control flow in their
/// `reset_state`, and they are the six `mN_start_animation`s.
#[test]
fn the_entry_animations_are_the_six_start_animations() {
    let Some(cache) = cache() else { return };

    let mut found = BTreeSet::new();
    let mut levels = 0;
    for level in 1..=13u16 {
        let Some(set) = anim(&cache, level) else {
            continue;
        };
        levels += 1;
        for def in set.defs.iter().filter(|def| is_entry_animation(def)) {
            found.insert((level, def.anim_name.clone()));
        }
    }
    if levels == 0 {
        return;
    }

    let expected: BTreeSet<_> = (1..=6u16)
        .map(|level| (level, format!("m{level}_start_animation")))
        .collect();
    assert_eq!(found, expected);
}

/// m2–m5's VTOL fly-in ends on the pose the `reset_state` gives it, so
/// finishing and skipping land in the same place.
#[test]
fn each_vtol_fly_in_ends_on_its_reset_pose() {
    let Some(cache) = cache() else { return };

    for level in 2..=5u16 {
        let Some(set) = anim(&cache, level) else {
            continue;
        };
        let intro = def(&set, &format!("m{level}_start_animation"));
        let fly_in = *si_scripts_on(intro, "vtol1").last().expect("a fly-in");
        let end = script_end(&set, fly_in);
        let reset = reset_translation(intro, "vtol1").expect("a reset pose");
        assert!(
            (end - reset).length() < 0.25,
            "m{level}: the fly-in ends at {end}, the reset says {reset}"
        );
    }
}

/// m6's reset parks the VTOL 720 m away, but the airdrop loop it calls
/// starts exactly where the fly-in ended: the parked pose lasts until the
/// loop's first tick and is never seen.
#[test]
fn m6s_hover_starts_where_its_fly_in_ends() {
    let Some(cache) = cache() else { return };
    let Some(set) = anim(&cache, 6) else { return };

    let intro = def(&set, "m6_start_animation");
    let fly_in_end = script_end(&set, *si_scripts_on(intro, "vtol1").last().unwrap());
    let parked = reset_translation(intro, "vtol1").unwrap();
    let hover = def(&set, "m6_airdrop_loop");
    let hover_start = script_start(&set, si_scripts_on(hover, "vtol1")[0]);

    assert!(
        (parked - fly_in_end).length() > 500.0,
        "the reset parks it away"
    );
    assert!(
        (hover_start - fly_in_end).length() < 0.05,
        "the hover picks up at {hover_start}, the fly-in ended at {fly_in_end}"
    );
}

/// m1's intro starts the truck at 14 s and ends at 22.95 s with the camera
/// behind the moving trailer: the BFT rides it out, so the engine must carry
/// it.
#[test]
fn m1s_intro_ends_looking_at_the_trailer_on_the_move() {
    let Some(cache) = cache() else { return };
    let Some(set) = anim(&cache, 1) else { return };

    let intro = def(&set, "m1_start_animation");
    let camera = si_scripts_on(intro, "camera1")[0];
    let intro_length = set.si_scripts[camera].duration();
    assert!((intro_length - 22.95).abs() < 0.01, "{intro_length}");

    let trailer = si_scripts_on(def(&set, "trailer_driving"), "trailr")[0];
    let start = script_start(&set, trailer);
    let at_hand_off = si_pose(&set.si_scripts[trailer], intro_length - 14.0)
        .0
        .unwrap();
    let eye = script_end(&set, camera);

    let driven = (at_hand_off - start).length();
    assert!(driven > 60.0, "the trailer has driven {driven} m");
    assert!(
        (eye - at_hand_off).length() < 25.0,
        "the camera ends {} m from the trailer",
        (eye - at_hand_off).length()
    );
}

/// Every callback in the game is one the protocol knows, `2`/`14` open
/// every intro and exit, and `99` is only in the exits.
#[test]
fn the_callbacks_fall_into_the_protocols_groups() {
    let Some(cache) = cache() else { return };

    let mut unknown = BTreeMap::new();
    for level in 1..=13u16 {
        let Some(set) = anim(&cache, level) else {
            continue;
        };
        for def in &set.defs {
            let values: Vec<i32> = sequence_events(def)
                .chain(def.reset_state.iter().map(|event| &event.data))
                .filter_map(|data| match data {
                    EventDataDesc::Callback(value) => Some(*value),
                    _ => None,
                })
                .collect();
            for value in &values {
                if callback_meaning(*value) == CallbackMeaning::Other
                    && ![25, 26, 27, 911, 912, 913, 914].contains(value)
                {
                    unknown.insert((level, def.anim_name.clone()), *value);
                }
                if *value == 99 {
                    assert!(
                        def.anim_name.ends_with("_exit_animation"),
                        "m{level}: 99 in {}",
                        def.anim_name
                    );
                }
            }
        }

        if level <= 6 {
            let intro = def(&set, &format!("m{level}_start_animation"));
            let fired: Vec<i32> = sequence_events(intro)
                .filter_map(|data| match data {
                    EventDataDesc::Callback(value) => Some(*value),
                    _ => None,
                })
                .collect();
            for value in [2, 14, 20] {
                assert!(fired.contains(&value), "m{level}'s intro fires {value}");
            }
            let exit = def(&set, &format!("m{level}_exit_animation"));
            assert!(
                sequence_events(exit).any(|data| matches!(data, EventDataDesc::Callback(14))),
                "m{level}'s exit enters a cinematic"
            );
        }
    }
    assert!(
        unknown.is_empty(),
        "callbacks outside the protocol: {unknown:?}"
    );
}

/// The hand-off pose is the reset's `bft_00`, and m1 has none.
#[test]
fn the_hand_off_poses_are_the_findings_table() {
    let Some(cache) = cache() else { return };

    let expected = [
        (1, None),
        (2, Some(Vec3::new(1697.0, 36.6, 656.5))),
        (3, Some(Vec3::new(3168.0, -30.95, 3995.0))),
        (4, Some(Vec3::new(2276.0, 4.918, 2965.0))),
        (5, Some(Vec3::new(1448.0, 28.2, 1938.0))),
        (6, Some(Vec3::new(1253.1, 25.1, 163.1))),
    ];
    for (level, position) in expected {
        let Some(set) = anim(&cache, level) else {
            continue;
        };
        let pose = hand_off_pose(&set);
        assert_eq!(
            pose.map(|pose| (pose.translation * 100.0).round() / 100.0),
            position.map(|p| (p * 100.0).round() / 100.0),
            "m{level}"
        );
    }

    // m2's is yawed −90°, facing down the ramp. Its 2.5° pitch and −2° roll
    // nearly cancel once composed (`Rz·Ry·Rx`, the order that reproduces
    // every VTOL fly-in's last quaternion to 0.11°): upright within a degree.
    if let Some(set) = anim(&cache, 2) {
        let pose = hand_off_pose(&set).unwrap();
        let facing = pose.rotation * Vec3::NEG_Z;
        assert!(facing.x > 0.99, "m2 faces +x: {facing}");
        let up = pose.rotation * Vec3::Y;
        assert!(up.y > 0.9998, "upright: {up}");
    }
}

// ---------------------------------------------------------------------------
// The intros, played
// ---------------------------------------------------------------------------

/// A headless app with a whole real level spawned, as `bft-game` builds it.
fn real_level(cache: &std::path::Path, level: u16) -> Option<App> {
    let (set, data) = (anim(cache, level)?, gamez(cache, level)?);

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    // World transforms propagate, as in the game: the VTOL's colliders hang
    // four levels under the node its scripts fly.
    app.add_plugins(bevy::transform::TransformPlugin);
    app.add_plugins(bft_sim::SimPlugin);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / SIM_HZ),
    ));
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.insert_resource(AxisConvention::GAMEZ_DEFAULT);
    app.finish();
    app.update();

    app.world_mut()
        .run_system_cached_with(
            |In(level): In<LevelData>, mut commands: Commands, axes: Res<AxisConvention>| {
                bft_sim::level::spawn_level_into(&mut commands, &level, &axes);
            },
            data,
        )
        .expect("the level spawns");

    app.world_mut()
        .run_system_cached_with(
            |In(set): In<AnimSet>,
             mut commands: Commands,
             nodes: Res<NodeIndex>,
             children: Query<&Children>,
             names: Query<&NodeName>,
             templates: Query<(), With<bft_sim::ai::Template>>,
             roots: Query<Entity, With<LevelRoot>>| {
                let Some(root) = roots.iter().next() else {
                    return;
                };
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
            set,
        )
        .expect("the instances spawn");

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

    for _ in 0..3 {
        app.update();
    }
    Some(app)
}

fn step(app: &mut App, seconds: f32) {
    for _ in 0..(seconds * SIM_HZ as f32).ceil() as usize {
        app.update();
    }
}

fn call(app: &mut App, name: &str) {
    app.world_mut()
        .resource_mut::<Messages<CallAnim>>()
        .write(CallAnim::new(name));
}

fn skip(app: &mut App) {
    app.world_mut()
        .resource_mut::<Messages<SkipCinematic>>()
        .write(SkipCinematic);
}

fn control(app: &App) -> PlayerControl {
    app.world().resource::<PlayerControl>().clone()
}

fn node(app: &App, name: &str) -> Entity {
    *app.world()
        .resource::<NodeIndex>()
        .by_name(name)
        .first()
        .expect("the node exists")
}

fn running(app: &mut App, anim_name: &str) -> bool {
    let program = app.world().resource::<AnimProgram>().0.clone();
    let targets = program.lookup(anim_name).to_vec();
    let mut query = app.world_mut().query::<&AnimInstance>();
    query
        .iter(app.world())
        .any(|instance| targets.contains(&instance.def) && instance.is_running())
}

fn translation(app: &App, name: &str) -> Vec3 {
    app.world()
        .get::<Transform>(node(app, name))
        .unwrap()
        .translation
}

/// m1: the intro plays under a cinematic, the chains hold until the
/// hand-off, and the intro disarms itself only then.
#[test]
fn m1s_intro_plays_then_hands_over_and_breaks_the_chains() {
    let Some(cache) = cache() else { return };
    let Some(mut app) = real_level(&cache, 1) else {
        return;
    };
    let chains = node(&app, "semichains");
    assert!(app.world().get::<NodeActive>(chains).unwrap().0);

    call(&mut app, "start_single_player");
    step(&mut app, 5.0);
    assert!(control(&app).is_cinematic(), "{:?}", control(&app));
    assert!(!control(&app).is_carried(), "m1 fires no 11: the BFT rides");
    assert!(running(&mut app, "m1_start_animation"));
    assert!(app.world().get::<NodeActive>(chains).unwrap().0);

    // 22.95 s, then `semi_chains` breaks them a second and a half later.
    step(&mut app, 20.0);
    assert_eq!(control(&app), PlayerControl::Driving);
    assert!(!running(&mut app, "m1_start_animation"));
    assert!(!app.world().get::<NodeActive>(chains).unwrap().0);
    assert!(running(&mut app, "trailer_driving"), "the truck drives on");
}

/// m2: the fly-in carries the player, the hand-off lands the VTOL on its
/// reset pose and starts the hover and the exit trigger. A skip at 5 s lands
/// in the same state. Driving 40 m off then sends the VTOL away.
#[test]
fn m2s_intro_lands_in_the_same_state_watched_or_skipped() {
    let Some(cache) = cache() else { return };
    let bay = Vec3::new(1697.0, 36.6, 656.5);

    let mut outcomes = Vec::new();
    for skip_at in [None, Some(5.0)] {
        let Some(mut app) = real_level(&cache, 2) else {
            return;
        };
        // The player sits in the bay, as `bft-game` reports it: with no
        // player at all, `PlayerRange` reads "far" and the VTOL leaves at
        // once.
        app.world_mut().resource_mut::<PlayerLocation>().position = Some(bay);
        call(&mut app, "m2_start_animation");
        step(&mut app, 2.0);
        assert!(control(&app).is_carried(), "{:?}", control(&app));

        match skip_at {
            Some(at) => {
                step(&mut app, at - 2.0);
                skip(&mut app);
                step(&mut app, 0.1);
            }
            None => step(&mut app, 24.0),
        }
        assert_eq!(control(&app), PlayerControl::Driving, "skip at {skip_at:?}");
        assert!(running(&mut app, "m2_airdrop_loop"));
        assert!(running(&mut app, "m2_do_exit_trigger"));

        let vtol = translation(&app, "vtol1");
        assert!(
            (vtol - Vec3::new(1694.3, 40.95, 656.4)).length() < 0.5,
            "landed: {vtol}"
        );
        let interior = app
            .world()
            .get::<NodeActive>(node(&app, "vtol_interior_on"))
            .unwrap()
            .0;
        let door = app
            .world()
            .get::<Transform>(node(&app, "cargodoor"))
            .unwrap()
            .rotation;
        outcomes.push((interior, door));

        // Out of the 27 m: the hover stops and the VTOL flies off.
        app.world_mut().resource_mut::<PlayerLocation>().position =
            Some(bay + Vec3::new(40.0, 0.0, 0.0));
        step(&mut app, 1.0);
        assert!(!running(&mut app, "m2_airdrop_loop"));
        assert!(running(&mut app, "m2_do_exit_animation"));
    }

    let (watched, skipped) = (outcomes[0], outcomes[1]);
    assert!(watched.0 && skipped.0, "the bay is lit");
    assert!(
        watched.1.angle_between(skipped.1) < 0.02,
        "the cargo door is open the same way: {:?} vs {:?}",
        watched.1,
        skipped.1
    );
}

/// Every exit takes control away and ends the mission on `99` — m1–m5
/// carrying the player off in the VTOL after the exit camera, m6 at once,
/// with no camera flight.
#[test]
fn every_exit_is_a_cinematic_that_ends_on_99() {
    let Some(cache) = cache() else { return };
    for level in 1..=6u16 {
        let Some(mut app) = real_level(&cache, level) else {
            continue;
        };
        call(&mut app, &format!("m{level}_exit_animation"));

        let mut won_at = None;
        let mut control_at_99 = None;
        for tick in 0..(SIM_HZ as usize * 30) {
            app.update();
            let fired: Vec<i32> = app
                .world_mut()
                .resource_mut::<Messages<bft_sim::anim::AnimCallback>>()
                .drain()
                .map(|callback| callback.value)
                .collect();
            if fired.contains(&99) {
                won_at = Some(tick as f32 / SIM_HZ as f32);
                control_at_99 = Some(control(&app));
                break;
            }
        }
        let won_at = won_at.unwrap_or_else(|| panic!("m{level}'s exit never fired 99"));
        let control = control_at_99.unwrap();
        assert!(control.is_cinematic(), "m{level}: {control:?}");
        if level == 6 {
            assert!(won_at < 0.1, "m6 has no exit camera: {won_at} s");
        } else {
            assert!(control.is_carried(), "m{level} carries the player off");
            assert!(
                won_at > 3.0,
                "m{level} flies its exit camera first: {won_at} s"
            );
        }
    }
}

/// All six intros finish, hand over and leave no entry animation running.
#[test]
fn every_intro_hands_over_when_it_ends() {
    let Some(cache) = cache() else { return };
    let entry = [
        (1, "start_single_player"),
        (2, "m2_start_animation"),
        (3, "m3_start_animation"),
        (4, "m4_start_animation"),
        (5, "m5_start_animation"),
        (6, "m6_start_animation"),
    ];
    for (level, name) in entry {
        let Some(mut app) = real_level(&cache, level) else {
            continue;
        };
        call(&mut app, name);
        step(&mut app, 1.0);
        assert!(control(&app).is_cinematic(), "m{level} starts a cinematic");
        step(&mut app, 26.0);
        assert_eq!(control(&app), PlayerControl::Driving, "m{level} hands over");
        assert!(
            !running(&mut app, &format!("m{level}_start_animation")),
            "m{level}'s intro is over"
        );
    }
}

// ---------------------------------------------------------------------------
// The player, handed over
// ---------------------------------------------------------------------------

fn reader(cache: &std::path::Path, name: &str) -> Option<bft_assets::ReaderData> {
    let bytes = std::fs::read(cache.join(format!("reader/{name}"))).ok()?;
    bft_assets::reader::parse_reader_data(&bytes).ok()
}

/// Spawn the real player vehicle at `at`, with `bft_00` re-parented onto it
/// the way `bft_game::player_model` does: its physics stripped, marked
/// [`PlayerModelRoot`].
fn spawn_player(app: &mut App, cache: &std::path::Path, at: Transform) -> Option<Entity> {
    use avian3d::prelude::{Collider, RigidBody};
    use bft_sim::anim::PlayerModelRoot;
    use bft_sim::vehicle::{
        FormSet, VehicleOverrides, apply_player_constants, spawn_player_vehicle,
        tuning::{Difficulty, build_vehicle_table},
    };

    let global = reader(cache, "zrdr.rdr.zip")?;
    let table =
        build_vehicle_table(&global, Difficulty::Normal, &VehicleOverrides::default()).ok()?;
    app.insert_resource(table.clone());

    let hull = app
        .world_mut()
        .run_system_cached_with(
            move |In((at, table)): In<(Transform, bft_sim::vehicle::VehicleTable)>,
                  mut commands: Commands|
                  -> Entity {
                apply_player_constants(&mut commands, &table.player);
                spawn_player_vehicle(&mut commands, at, &table, FormSet::track_only())
            },
            (at, table),
        )
        .ok()?;

    let model = node(app, "bft_00");
    let mut subtree = vec![model];
    let mut index = 0;
    while index < subtree.len() {
        if let Some(children) = app.world().get::<Children>(subtree[index]) {
            subtree.extend(children.iter());
        }
        index += 1;
    }
    for entity in subtree {
        app.world_mut()
            .entity_mut(entity)
            .remove::<(RigidBody, Collider)>();
    }
    app.world_mut().entity_mut(model).insert((
        ChildOf(hull),
        Transform::IDENTITY,
        NodeActive(true),
        PlayerModelRoot { hull },
    ));
    Some(hull)
}

/// Step, reporting the hull's position to the VM as `bft-game` does, and
/// holding an input on it as the frame layer would.
fn step_player(app: &mut App, hull: Entity, seconds: f32, throttle: f32) {
    use bft_sim::vehicle::input::PlayerInput;
    for _ in 0..(seconds * SIM_HZ as f32).ceil() as usize {
        let at = app.world().get::<Transform>(hull).unwrap().translation;
        app.world_mut().resource_mut::<PlayerLocation>().position = Some(at);
        app.world_mut()
            .get_mut::<PlayerInput>(hull)
            .unwrap()
            .throttle = throttle;
        app.update();
    }
}

/// Full throttle, steering for `target`: `steer` +1 turns right. Stops on
/// arrival — within [`ARRIVED`] of it across the ground — or it would circle
/// the point for the rest of the time.
fn drive_towards(app: &mut App, hull: Entity, target: Vec3, seconds: f32) {
    use bft_sim::vehicle::input::PlayerInput;
    for _ in 0..(seconds * SIM_HZ as f32).ceil() as usize {
        let pose = hull_pose(app, hull);
        if (target - pose.translation).with_y(0.0).length() < ARRIVED {
            *app.world_mut().get_mut::<PlayerInput>(hull).unwrap() = PlayerInput::default();
            return;
        }
        let forward = (pose.rotation * Vec3::NEG_Z)
            .with_y(0.0)
            .normalize_or_zero();
        let to = (target - pose.translation).with_y(0.0).normalize_or_zero();
        // Positive when the target is to the left, which wants a left turn.
        let angle = forward.cross(to).y.atan2(forward.dot(to));
        app.world_mut().resource_mut::<PlayerLocation>().position = Some(pose.translation);
        *app.world_mut().get_mut::<PlayerInput>(hull).unwrap() = PlayerInput {
            throttle: 1.0,
            steer: (-angle * 3.0).clamp(-1.0, 1.0),
            ..Default::default()
        };
        app.update();
    }
}

/// How close across the ground counts as having reached a target, metres.
const ARRIVED: f32 = 5.0;

fn hull_pose(app: &App, hull: Entity) -> Transform {
    *app.world().get::<Transform>(hull).unwrap()
}

/// m2–m6: the hull waits out the fly-in parked, comes to rest in the landed
/// VTOL's bay at hand-off, and drives out down the ramp — which sends the
/// VTOL away. The interior colliders must not trap it.
#[test]
fn the_bft_drives_out_of_every_vtol() {
    let Some(cache) = cache() else { return };

    for level in 2..=6u16 {
        let Some(mut app) = real_level(&cache, level) else {
            continue;
        };
        let bay = hand_off_pose(&app.world().resource::<AnimProgram>().0).unwrap();
        let Some(hull) = spawn_player(&mut app, &cache, bay) else {
            return;
        };

        call(&mut app, &format!("m{level}_start_animation"));
        step_player(&mut app, hull, 1.0, 1.0);
        assert!(control(&app).is_carried(), "m{level}: {:?}", control(&app));
        // m3–m6 carry the player from 0.5 s in; the hull drops until then,
        // off camera, and the reset puts it back in the bay at hand-off.
        let parked = hull_pose(&app, hull).translation;
        step_player(&mut app, hull, 1.0, 1.0);
        let held = hull_pose(&app, hull).translation;
        assert!(
            (held - parked).length() < 0.01,
            "m{level}: parked, not falling: {parked} → {held}"
        );

        // The throttle held through the intro is ignored until the hand-off.
        let mut waited = 0.0;
        while control(&app).is_cinematic() && waited < 30.0 {
            step_player(&mut app, hull, 0.1, 1.0);
            waited += 0.1;
        }
        assert_eq!(control(&app), PlayerControl::Driving, "m{level}");
        let handed = hull_pose(&app, hull).translation;
        assert!(
            (handed - bay.translation).length() < 0.5,
            "m{level}: handed over in the bay: {handed}"
        );
        step_player(&mut app, hull, 2.0, 0.0);
        let rest = hull_pose(&app, hull);
        let up = rest.rotation * Vec3::Y;
        eprintln!(
            "m{level}: at rest {:.2} (bay {:.2}), up {up:.3}",
            rest.translation, bay.translation
        );
        assert!(
            (rest.translation - bay.translation).length() < 2.0,
            "m{level}: settles in the bay: {} vs {}",
            rest.translation,
            bay.translation
        );
        assert!(up.y > 0.95, "m{level}: upright in the bay: {up}");

        // Full throttle out of the bay and down the ramp, steering for a
        // point 35 m out along the hand-off heading, as a player would.
        let facing = (bay.rotation * Vec3::NEG_Z).with_y(0.0).normalize();
        let target = bay.translation + facing * 35.0;
        drive_towards(&mut app, hull, target, 15.0);
        let out = hull_pose(&app, hull);
        let driven = (out.translation - bay.translation).length();
        eprintln!("m{level}: drove {driven:.1} m to {:.1}", out.translation);
        assert!(driven > 27.0, "m{level}: trapped, only {driven} m out");
        assert!((out.rotation * Vec3::Y).y > 0.7, "m{level}: rolled over");
        assert!(
            !running(&mut app, &format!("m{level}_airdrop_loop")),
            "m{level}: the VTOL is sent away"
        );
    }
}

/// m1: the BFT rides the trailer through the intro and can drive off it.
#[test]
fn m1s_bft_rides_the_trailer_and_drives_off() {
    let Some(cache) = cache() else { return };
    let Some(mut app) = real_level(&cache, 1) else {
        return;
    };
    let Some(start) = reader(&cache, "m1-zrdr.rdr.zip").and_then(|r| r.player_start()) else {
        return;
    };
    let at =
        Transform::from_translation(start.position + Vec3::Y * 2.0).with_rotation(start.rotation());
    let Some(hull) = spawn_player(&mut app, &cache, at) else {
        return;
    };

    call(&mut app, "start_single_player");
    step_player(&mut app, hull, 13.0, 1.0);
    let before = hull_pose(&app, hull).translation;
    assert!(
        (before - start.position).length() < 3.0,
        "input is locked and the truck has not left: {before}"
    );

    // The truck leaves at 14 s; the intro hands over at 22.95 s.
    step_player(&mut app, hull, 11.0, 0.0);
    assert_eq!(control(&app), PlayerControl::Driving);
    let trailer = translation(&app, "trailr");
    let riding = hull_pose(&app, hull).translation;
    eprintln!("m1: hull {riding:.1}, trailer {trailer:.1}");
    assert!(
        (riding - before).length() > 50.0,
        "the BFT rode out with the truck: {riding}"
    );
    assert!(
        (riding - trailer).length() < 10.0,
        "and is still on the trailer: {riding} vs {trailer}"
    );

    // Off the back: reverse at full throttle for a few seconds.
    step_player(&mut app, hull, 6.0, -1.0);
    let off = hull_pose(&app, hull);
    let trailer = translation(&app, "trailr");
    eprintln!(
        "m1: hull {:.1}, trailer {trailer:.1}, up {:.2}",
        off.translation,
        off.rotation * Vec3::Y
    );
    assert!(
        (off.translation - trailer).length() > 15.0,
        "the BFT got off the trailer"
    );
    assert!((off.rotation * Vec3::Y).y > 0.7, "and landed on its tracks");
}
