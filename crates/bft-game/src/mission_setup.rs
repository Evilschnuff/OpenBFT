//! Turning a spawned level into a running mission.
//!
//! The pieces come from four places and have to meet in the right order:
//! `bft-sim` spawns the node hierarchy, `bft-assets` loads the level's
//! `AnimSet` and its reader archive, and this module builds the anim
//! instances (which need the hierarchy, for subtree name scoping), the
//! [`MissionScript`] and the [`ObjectiveBaseline`] — the node states the
//! objectives are measured against.
//!
//! It lives in `bft-game` and not in `bft-sim` because it is the only place
//! that has both the asset server's loaded `AnimSet` and the spawned level;
//! `bft-sim` has the VM but no asset handles (NFR-4), and everything it does
//! here goes through functions `bft-sim` owns.
//!
//! # Why the baseline is taken here and not at the first tick
//!
//! An objective completes when its node's active state **differs from what it
//! was at mission start**. "Mission start" is before any definition's
//! `reset_state` has run, so the baseline has to be read in the same pass
//! that spawns the instances and before the VM's first step.

use bevy::prelude::*;
use bft_assets::anim::AnimSet;
use bft_assets::catalog::{LevelCatalog, LevelKind};
use bft_assets::reader::ReaderData;
use bft_sim::anim::{
    AnimProgram, EventHandlers, PlayerLocation, apply_initial_node_states, spawn_anim_instances,
};
use bft_sim::level::{NodeActive, NodeIndex, NodeName, NodeZone};
use bft_sim::mission::{
    Completion, MissionScript, MissionState, ObjectiveBaseline, build_mission_script,
};
use bft_sim::states::AppState;
use bft_sim::vehicle::PlayerVehicle;

use crate::level_view::LoadedLevel;

/// Whether this level's mission has been set up yet.
///
/// A marker resource rather than a flag on [`LoadedLevel`]: the level can be
/// spawned for the viewer, where there is no mission at all, and the two
/// states should not share a field.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissionReady(pub bft_assets::catalog::LevelId);

/// Mission and VM setup, and the player's position for the VM's conditions.
pub struct MissionSetupPlugin;

impl Plugin for MissionSetupPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            setup_mission
                .run_if(in_state(AppState::InGame))
                // After the level is spawned, or there is no hierarchy to
                // scope names against and no nodes to read a baseline from.
                .after(crate::level_view::poll_level_load),
        )
        .add_systems(
            FixedUpdate,
            // Before `SimSet::Anim`, so the ranges and zones the VM polls are
            // this tick's rather than last tick's.
            track_player_location.before(bft_sim::schedule::SimSet::Anim),
        )
        .add_systems(OnExit(AppState::InGame), tear_down_mission);
    }
}

/// Build the mission and the VM's instances, once per level.
// Eleven parameters because this is the junction: the level's two assets, the
// catalog that says whether it is a campaign, the spawned hierarchy and the
// queries that read it. Each is used exactly once, in order.
#[allow(clippy::too_many_arguments)]
pub fn setup_mission(
    mut commands: Commands,
    loaded: Option<Res<LoadedLevel>>,
    ready: Option<Res<MissionReady>>,
    catalog: Res<LevelCatalog>,
    anims: Res<Assets<AnimSet>>,
    readers: Res<Assets<ReaderData>>,
    nodes: Option<Res<NodeIndex>>,
    children: Query<&Children>,
    names: Query<&NodeName>,
    templates: Query<(), With<bft_sim::ai::Template>>,
    mut actives: Query<&mut NodeActive>,
    handlers: Res<EventHandlers>,
) {
    let (Some(loaded), Some(nodes)) = (loaded, nodes) else {
        return;
    };
    let Some(root) = loaded.root else {
        return;
    };
    // Already done for this level.
    if ready.is_some_and(|ready| ready.0 == loaded.id) {
        return;
    }

    // The level-scripting program. A level whose anim extraction failed has
    // none; it still plays, with no scripted animation and — because the
    // objectives' nodes are flipped by anim events — no objective that can
    // complete, which is why the warning says so plainly.
    let program = loaded
        .anim
        .as_ref()
        .and_then(|handle| anims.get(handle))
        .cloned();

    match &program {
        Some(set) => {
            info!(
                "level {}: {} animation definition(s), {} SI script(s)",
                loaded.id,
                set.defs.len(),
                set.si_scripts.len()
            );
            log_unhandled(set, &handlers);

            let scopes = spawn_anim_instances(
                &mut commands,
                set,
                &nodes,
                &children,
                &names,
                &templates,
                root,
            );

            // Before anything steps, and before the baseline below is read:
            // every node in `gamez` is active, and this is what turns off the
            // destroyed hulks, opened doors and debris that should not be
            // visible yet (docs/data/anims.md).
            let hidden = apply_initial_node_states(set, &scopes, &nodes, &mut actives);
            info!("level {}: {hidden} node(s) start inactive", loaded.id);

            commands.insert_resource(scopes);
            commands.insert_resource(AnimProgram(set.clone()));
        }
        None => warn!(
            "level {} has no animation definitions; nothing scripted will run \
             and no objective can complete",
            loaded.id
        ),
    }

    // Only a campaign level has a mission. The multiplayer maps ship a
    // byte-for-byte copy of m2's `objectives.zrd` (READER-SCHEMA §3.6), so
    // the catalog's kind — not the file's presence — is what decides.
    let is_campaign = catalog
        .level(loaded.id)
        .is_some_and(|entry| matches!(entry.kind, LevelKind::Campaign { .. }));

    if is_campaign
        && let Some(reader) = loaded
            .reader
            .as_ref()
            .and_then(|handle| readers.get(handle))
        && let Some(script) = build_mission_script(reader)
    {
        info!(
            "mission {}: {} objective(s), entry animation {}",
            loaded.id,
            script.objectives.len(),
            script.new_game_anim
        );

        // Read *after* the reset states above: "mission start" is the level
        // as the player first sees it, not as `gamez` stored it.
        commands.insert_resource(baseline(
            &script,
            &nodes,
            &children,
            &names,
            &actives.as_readonly(),
        ));
        commands.insert_resource(MissionState::new(loaded.id, &script));
        commands.insert_resource(script);
        // The HUD's clock and score start with the mission; a save restores
        // them afterwards.
        commands.insert_resource(bft_sim::mission::MissionClock::default());
        commands.insert_resource(bft_sim::mission::Score::default());
    }

    commands.insert_resource(MissionReady(loaded.id));
}

/// Read the objectives' nodes as they are before anything has run.
///
/// The two-element condition `[object, child]` is scoped to the object's
/// subtree: m4's `[fntrocket, healthy]` means the `healthy` under
/// `fntrocket`, and a level-wide lookup would pick one of the other 185.
fn baseline(
    script: &MissionScript,
    nodes: &NodeIndex,
    children: &Query<&Children>,
    names: &Query<&NodeName>,
    actives: &Query<&NodeActive>,
) -> ObjectiveBaseline {
    let states = script
        .objectives
        .iter()
        .enumerate()
        .map(|(index, objective)| {
            let Completion::NodeStateChanged { node, child } = &objective.completion else {
                return None;
            };

            let object = nodes.by_name(node).first().copied()?;
            let target = match child {
                Some(child) => match find_in_subtree(object, child, children, names) {
                    Some(entity) => entity,
                    None => {
                        warn!(
                            "objective {}: no `{child}` under `{node}`; watching `{node}` itself",
                            index + 1
                        );
                        object
                    }
                },
                None => object,
            };

            let state = actives.get(target).map(|active| active.0).unwrap_or(true);
            Some((target, state))
        })
        .collect();

    ObjectiveBaseline { states }
}

/// The nearest node with a name, inside a subtree.
fn find_in_subtree(
    root: Entity,
    name: &str,
    children: &Query<&Children>,
    names: &Query<&NodeName>,
) -> Option<Entity> {
    // Breadth-first: the node an objective means by `healthy` is the one
    // closest to the object it names, not the first one a depth-first walk
    // happens to reach.
    let mut queue = std::collections::VecDeque::from([root]);

    while let Some(entity) = queue.pop_front() {
        if names.get(entity).is_ok_and(|node| node.0 == name) {
            return Some(entity);
        }
        if let Ok(kids) = children.get(entity) {
            queue.extend(kids.iter());
        }
    }

    None
}

/// Say once if any of the level's scripting has no handler.
///
/// Every kind the shipped levels use has one, so this is a
/// warning: it fires only if the loader starts producing a kind nobody
/// handles. The kinds with the most events first.
fn log_unhandled(set: &AnimSet, handlers: &EventHandlers) {
    let mut counts: Vec<(&str, usize)> = set
        .event_counts
        .iter()
        .filter(|(kind, _)| !is_control_flow(kind) && handlers.get(kind).is_none())
        .map(|(kind, count)| (*kind, *count))
        .collect();
    if counts.is_empty() {
        return;
    }

    counts.sort_by_key(|(kind, count)| (std::cmp::Reverse(*count), *kind));
    let top: Vec<String> = counts
        .iter()
        .take(5)
        .map(|(kind, count)| format!("{kind} ×{count}"))
        .collect();

    warn!(
        "{} anim event kind(s) have no handler: {}",
        counts.len(),
        top.join(", ")
    );
}

/// Whether a kind is control flow the VM steps itself rather than a handler.
fn is_control_flow(kind: &str) -> bool {
    matches!(kind, "Loop" | "If" | "Elseif" | "Else" | "Endif")
}

/// Keep the VM's view of the player up to date.
///
/// `ByRange` and `PlayerRange` want a position and `ByZone` a zone id. The
/// zone is the one the nearest zoned node holds, which is how the original
/// decided it: zones are volumes described by the nodes standing in them
/// (docs/data/nodes.md).
pub fn track_player_location(
    players: Query<&GlobalTransform, With<PlayerVehicle>>,
    zoned: Query<(&GlobalTransform, &NodeZone)>,
    mut location: ResMut<PlayerLocation>,
) {
    let Ok(player) = players.single() else {
        // No player: a viewer session, or the frame before one spawns. The
        // range conditions then read as false rather than as true everywhere.
        *location = PlayerLocation::default();
        return;
    };
    let position = player.translation();

    let zone = zoned
        .iter()
        .map(|(transform, zone)| (transform.translation().distance_squared(position), zone.0))
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, zone)| zone);

    *location = PlayerLocation {
        position: Some(position),
        zone,
    };
}

/// Drop the mission's resources when the level does.
///
/// `ObjectiveBaseline` and `AnimScopes` are **reset**, not removed: the sim
/// creates them at startup and reads them as required resources, and
/// [`setup_mission`] only puts them back in `Update` — after the next
/// session's first `FixedUpdate` ticks have already run. Removing them made
/// every second entry into a level panic on its first tick.
pub fn tear_down_mission(mut commands: Commands) {
    commands.remove_resource::<MissionReady>();
    commands.remove_resource::<MissionScript>();
    commands.remove_resource::<MissionState>();
    commands.remove_resource::<AnimProgram>();
    commands.insert_resource(ObjectiveBaseline::default());
    commands.insert_resource(bft_sim::anim::AnimScopes::default());
    commands.insert_resource(bft_sim::mission::MissionClock::default());
    commands.insert_resource(bft_sim::mission::Score::default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_sim::mission::{ObjectiveDef, ObjectiveState};

    fn objective(completion: Completion) -> ObjectiveDef {
        ObjectiveDef {
            index: 0,
            picture: "pic".to_string(),
            summary_key: "MSG_S".to_string(),
            text_key: "MSG_O".to_string(),
            complete_key: "MSG_C".to_string(),
            initially_active: true,
            completion,
            read_sound: None,
            hint: None,
        }
    }

    fn script(objectives: Vec<ObjectiveDef>) -> MissionScript {
        MissionScript {
            objectives,
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

    /// Build the hierarchy m4's objective 4 has: two `healthy` nodes, one
    /// under `fntrocket` and one under something else.
    fn app_with_two_healthies() -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins);

        let mine = app
            .world_mut()
            .spawn((NodeName("healthy".to_string()), NodeActive(true)))
            .id();
        let theirs = app
            .world_mut()
            .spawn((NodeName("healthy".to_string()), NodeActive(false)))
            .id();
        let rocket = app
            .world_mut()
            .spawn((NodeName("fntrocket".to_string()), NodeActive(true)))
            .id();
        app.world_mut().entity_mut(rocket).add_child(mine);

        let mut index = NodeIndex::default();
        // `theirs` first, so a level-wide lookup would find the wrong one.
        index.insert(0, "healthy", theirs);
        index.insert(1, "healthy", mine);
        index.insert(2, "fntrocket", rocket);
        app.insert_resource(index);

        (app, mine, theirs)
    }

    /// The whole point of the scoped lookup: `[fntrocket, healthy]` has to
    /// find the `healthy` inside `fntrocket`, not the level's first.
    #[test]
    fn a_child_condition_resolves_inside_its_objects_subtree() {
        let (mut app, mine, theirs) = app_with_two_healthies();
        let script = script(vec![objective(Completion::NodeStateChanged {
            node: "fntrocket".to_string(),
            child: Some("healthy".to_string()),
        })]);

        let built = app
            .world_mut()
            .run_system_cached_with(
                |script: In<MissionScript>,
                 nodes: Res<NodeIndex>,
                 children: Query<&Children>,
                 names: Query<&NodeName>,
                 actives: Query<&NodeActive>| {
                    baseline(&script, &nodes, &children, &names, &actives)
                },
                script,
            )
            .expect("runs");

        assert_eq!(built.states[0], Some((mine, true)));
        assert_ne!(built.states[0].map(|(entity, _)| entity), Some(theirs));
    }

    /// A single-name condition watches that node level-wide.
    #[test]
    fn a_single_name_condition_watches_the_named_node() {
        let (mut app, _, theirs) = app_with_two_healthies();
        let script = script(vec![objective(Completion::NodeStateChanged {
            node: "healthy".to_string(),
            child: None,
        })]);

        let built = app
            .world_mut()
            .run_system_cached_with(
                |script: In<MissionScript>,
                 nodes: Res<NodeIndex>,
                 children: Query<&Children>,
                 names: Query<&NodeName>,
                 actives: Query<&NodeActive>| {
                    baseline(&script, &nodes, &children, &names, &actives)
                },
                script,
            )
            .expect("runs");

        // The first level-wide match, and its state as spawned.
        assert_eq!(built.states[0], Some((theirs, false)));
    }

    /// An exit objective has no node, so it has no baseline either.
    #[test]
    fn an_exit_objective_has_no_baseline() {
        let (mut app, _, _) = app_with_two_healthies();
        let script = script(vec![objective(Completion::ExitAnim)]);

        let built = app
            .world_mut()
            .run_system_cached_with(
                |script: In<MissionScript>,
                 nodes: Res<NodeIndex>,
                 children: Query<&Children>,
                 names: Query<&NodeName>,
                 actives: Query<&NodeActive>| {
                    baseline(&script, &nodes, &children, &names, &actives)
                },
                script,
            )
            .expect("runs");

        assert_eq!(built.states[0], None);
    }

    /// Tearing down removes every mission resource, so the next level does
    /// not inherit the last one's objectives.
    #[test]
    fn tear_down_clears_every_mission_resource() {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins);

        let script = script(vec![objective(Completion::ExitAnim)]);
        app.insert_resource(MissionState::new(bft_assets::catalog::LevelId(1), &script));
        app.insert_resource(script);
        app.insert_resource(ObjectiveBaseline::default());
        app.insert_resource(MissionReady(bft_assets::catalog::LevelId(1)));
        app.insert_resource(AnimProgram::default());

        app.world_mut()
            .run_system_cached(tear_down_mission)
            .expect("runs");

        assert!(app.world().get_resource::<MissionScript>().is_none());
        assert!(app.world().get_resource::<MissionState>().is_none());
        assert!(app.world().get_resource::<MissionReady>().is_none());
        assert!(app.world().get_resource::<AnimProgram>().is_none());
        // Reset rather than removed: the sim requires both.
        let baseline = app.world().resource::<ObjectiveBaseline>();
        assert!(baseline.states.is_empty());
        assert!(
            app.world()
                .resource::<bft_sim::anim::AnimScopes>()
                .0
                .is_empty()
        );
    }

    /// Leaving a level and entering the next one ticks the sim before
    /// `setup_mission` has run: that first tick must not find a required
    /// resource such as `AnimScopes` missing.
    #[test]
    fn the_sim_ticks_after_a_level_is_torn_down() {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins);
        app.add_plugins(bft_sim::SimPlugin);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(1.0 / bft_sim::schedule::SIM_HZ),
        ));
        app.insert_resource(State::new(AppState::InGame));
        app.finish();
        app.update();

        app.world_mut()
            .run_system_cached(tear_down_mission)
            .expect("runs");
        for _ in 0..3 {
            app.update();
        }
    }

    /// The mission starts with the file's own objective states, which is
    /// what the automap marker reads on the first frame.
    #[test]
    fn a_fresh_mission_state_matches_the_script() {
        let mut first = objective(Completion::ExitAnim);
        first.initially_active = true;
        let mut second = objective(Completion::ExitAnim);
        second.initially_active = false;

        let script = script(vec![first, second]);
        let state = MissionState::new(bft_assets::catalog::LevelId(1), &script);

        assert_eq!(
            state.objectives,
            vec![ObjectiveState::Active, ObjectiveState::Inactive]
        );
    }
}
