//! The mission and VM checks that need a real Recoil install.
//!
//! They skip (not fail) when `BFT_DATA_DIR` is unset or the cache has not
//! been built, so `cargo test --workspace` stays green for a contributor
//! without the game. No game data enters the repository: only counts,
//! invariants and the handful of values docs/data/anims.md and READER-SCHEMA already
//! publish are asserted.
//!
//! Build the cache first with `open-bft extract`.

use bft_assets::anim::{AnimActivation, AnimSet, EventDataDesc, parse_anim_set};
use bft_assets::catalog::LevelId;
use bft_assets::level::{LevelData, parse_gamez};
use bft_assets::reader::{ReaderData, parse_reader_data};
use bft_sim::anim::MISSION_END_CALLBACK;
use bft_sim::mission::{
    Completion, DEATH_ANIM, MissionScript, MissionState, ObjectiveState, build_mission_script,
};

/// The resolved paths for a real install, or `None` when there is no data.
fn paths() -> Option<bft_assets::DataPaths> {
    let root = bft_assets::test_support::data_root()?;

    match bft_assets::DataPaths::discover(Some(&root), None, None) {
        Ok(paths) if paths.cache.path.is_dir() => Some(paths),
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

fn cached(paths: &bft_assets::DataPaths, relative: &str) -> Option<Vec<u8>> {
    let path = paths.cache.path.join(relative);
    if !path.is_file() {
        eprintln!("{} is not in the cache; skipping", path.display());
        return None;
    }
    Some(std::fs::read(&path).expect("the cache is readable"))
}

fn reader(paths: &bft_assets::DataPaths, level: u16) -> Option<ReaderData> {
    let bytes = cached(paths, &format!("reader/m{level}-zrdr.rdr.zip"))?;
    Some(parse_reader_data(&bytes).expect("a real reader archive parses"))
}

fn anim(paths: &bft_assets::DataPaths, level: u16) -> Option<AnimSet> {
    let bytes = cached(paths, &format!("anim/m{level}-anim.anim.zip"))?;
    Some(parse_anim_set(&bytes).expect("a real anim archive parses"))
}

fn gamez(paths: &bft_assets::DataPaths, level: u16) -> Option<LevelData> {
    let bytes = cached(paths, &format!("gamez/m{level}-gamez.gamez.zip"))?;
    Some(parse_gamez(&bytes, "gamez.zip").expect("a real gamez archive parses"))
}

fn script(paths: &bft_assets::DataPaths, level: u16) -> Option<MissionScript> {
    build_mission_script(&reader(paths, level)?)
}

/// Every campaign mission builds a script, with the counts §12 measured.
#[test]
fn every_campaign_mission_builds_a_script() {
    let Some(paths) = paths() else { return };

    // m1 4, m2 5, m3 6, m4 5, m5 6, m6 5 — 31 records.
    let counts = [(1, 4), (2, 5), (3, 6), (4, 5), (5, 6), (6, 5)];
    let mut total = 0usize;

    for (level, expected) in counts {
        let Some(script) = script(&paths, level) else {
            continue;
        };

        assert_eq!(script.objectives.len(), expected, "m{level}");
        assert_eq!(script.death_anim, DEATH_ANIM);
        assert_eq!(script.load_game_anim, "load_control");
        assert_eq!(script.final_mission, level == 6, "m{level} FINAL_MISSION");
        total += script.objectives.len();
    }

    if total > 0 {
        assert_eq!(total, 31, "objective records over the six missions");
    }
}

/// Every completion condition resolves against the level's own nodes.
///
/// The check that catches a wrong condition reading: a node name that is not
/// in the level means the objective can never complete, and the mission is
/// unfinishable without a single error being logged.
#[test]
fn every_completion_node_exists_in_its_level() {
    let Some(paths) = paths() else { return };

    for level in 1..=6u16 {
        let (Some(script), Some(data)) = (script(&paths, level), gamez(&paths, level)) else {
            continue;
        };

        let names: std::collections::HashSet<&str> = data
            .nodes
            .iter()
            .map(|node| node.common().name.as_str())
            .collect();

        for objective in &script.objectives {
            match &objective.completion {
                Completion::NodeStateChanged { node, child } => {
                    assert!(
                        names.contains(node.as_str()),
                        "m{level} objective {}: no node `{node}` in the level",
                        objective.index + 1
                    );
                    if let Some(child) = child {
                        assert!(
                            names.contains(child.as_str()),
                            "m{level} objective {}: no node `{child}` in the level",
                            objective.index + 1
                        );
                    }
                }
                // No shipped mission uses one, so finding one would mean the
                // record reading changed under us.
                Completion::PickupCollected { instance } => {
                    panic!("m{level} unexpectedly completes on pickup `{instance}`")
                }
                Completion::ExitAnim => {}
            }
        }
    }
}

/// Every objective node starts active, which is what makes "differs from the
/// start" a usable rule rather than an assumption about polarity.
#[test]
fn every_objective_node_starts_active() {
    let Some(paths) = paths() else { return };

    for level in 1..=6u16 {
        let (Some(script), Some(data)) = (script(&paths, level), gamez(&paths, level)) else {
            continue;
        };

        for objective in &script.objectives {
            let Completion::NodeStateChanged { node, .. } = &objective.completion else {
                continue;
            };
            for candidate in data.nodes.iter().filter(|n| n.common().name == *node) {
                assert!(
                    candidate.common().flags.active,
                    "m{level} objective {}: `{node}` starts inactive",
                    objective.index + 1
                );
            }
        }
    }
}

/// The exit objective is the last one in m1–m5, and m6 has none.
#[test]
fn the_exit_objective_is_last_where_there_is_one() {
    let Some(paths) = paths() else { return };

    for level in 1..=6u16 {
        let Some(script) = script(&paths, level) else {
            continue;
        };

        let exits: Vec<u8> = script
            .objectives
            .iter()
            .filter(|objective| objective.completion == Completion::ExitAnim)
            .map(|objective| objective.index)
            .collect();

        if level == 6 {
            assert!(exits.is_empty(), "m6 ends the campaign, not with a VTOL");
            continue;
        }

        assert_eq!(
            exits,
            vec![script.objectives.len() as u8 - 1],
            "m{level}'s exit is its last objective"
        );
    }
}

/// Every `INACTIVE` objective has an animation that flips its node.
///
/// This answers READER-SCHEMA §5.3. An `INACTIVE` objective is not activated
/// by the previous one completing: m2's objective 1 is `INACTIVE` with
/// nothing before it and m3's objective 3 sits `INACTIVE` between two `ACTIVE` ones, so briefing
/// order is not dependency order. What all 16 of them do have is a dedicated
/// definition that flips exactly their node (`check_get_amphib`,
/// `obj_trigger1`, `check_get_hover`, `tp6_teleport_access`, …), so
/// `INACTIVE` means "not worth briefing until it happens" and the flip both
/// reveals and completes the objective.
#[test]
fn every_inactive_objective_has_an_animation_that_completes_it() {
    let Some(paths) = paths() else { return };

    let mut checked = 0usize;

    for level in 1..=6u16 {
        let (Some(script), Some(set)) = (script(&paths, level), anim(&paths, level)) else {
            continue;
        };

        for objective in &script.objectives {
            if objective.initially_active {
                continue;
            }
            let Completion::NodeStateChanged { node, child } = &objective.completion else {
                continue;
            };
            // The watched node is the child where there is one.
            let watched = child.as_deref().unwrap_or(node.as_str());

            let flippers = set
                .defs
                .iter()
                .filter(|def| {
                    def.sequences
                        .iter()
                        .flat_map(|sequence| &sequence.events)
                        .any(|event| {
                            matches!(
                                &event.data,
                                EventDataDesc::ObjectActiveState { node, .. } if node == watched
                            )
                        })
                })
                .count();

            assert!(
                flippers > 0,
                "m{level} objective {}: nothing flips `{watched}`, so it can \
                 never complete",
                objective.index + 1
            );
            checked += 1;
        }
    }

    if checked > 0 {
        assert_eq!(checked, 16, "INACTIVE objectives over the six missions");
    }
}

/// Every mission can be finished: each objective has a way to complete, and
/// the exit opens once they have.
#[test]
fn every_mission_can_be_finished() {
    let Some(paths) = paths() else { return };

    for level in 1..=6u16 {
        let Some(script) = script(&paths, level) else {
            continue;
        };
        let mut state = MissionState::new(LevelId(level), &script);

        // Complete everything that is not the exit, whatever state it is in:
        // an `INACTIVE` objective is completed by its own animation.
        for (index, objective) in script.objectives.iter().enumerate() {
            if objective.completion != Completion::ExitAnim {
                state.objectives[index] = ObjectiveState::Complete;
            }
        }

        assert!(
            state.objectives_done(&script),
            "m{level}: {:?}",
            state.objectives
        );
    }
}

/// The entry animation, the death animation and the exit animation all
/// resolve in the level that names them.
#[test]
fn every_named_animation_resolves_in_its_level() {
    let Some(paths) = paths() else { return };

    for level in 1..=6u16 {
        let (Some(script), Some(set)) = (script(&paths, level), anim(&paths, level)) else {
            continue;
        };

        for name in [
            script.new_game_anim.as_str(),
            script.load_game_anim.as_str(),
            script.death_anim.as_str(),
        ] {
            assert!(
                set.find(name).is_some(),
                "m{level}: `{name}` is not a definition in this level"
            );
        }
    }
}

/// The definition that wins the mission exists in every campaign level, and
/// it is the one the VM's constant is keyed to.
#[test]
fn every_mission_has_a_definition_that_can_end_it() {
    let Some(paths) = paths() else { return };

    for level in 1..=6u16 {
        let Some(set) = anim(&paths, level) else {
            continue;
        };

        let enders: Vec<&str> = set
            .defs
            .iter()
            .filter(|def| {
                def.sequences.iter().any(|sequence| {
                    sequence.events.iter().any(|event| {
                        matches!(event.data, EventDataDesc::Callback(MISSION_END_CALLBACK))
                    })
                })
            })
            .map(|def| def.anim_name.as_str())
            .collect();

        assert_eq!(
            enders,
            vec![format!("m{level}_exit_animation").as_str()],
            "m{level} has exactly one way to win"
        );
    }
}

/// Almost every `CallAnimation` names a definition its level has.
///
/// Eighteen calls in the whole game do not, and they are the original's own
/// authoring slips — six distinct names, each a prop or effect that was
/// renamed or dropped before release (`lavbub.flt`, `cjunkdest02`,
/// `destroy_vtol1` from a definition that is itself in m1 only). The VM warns
/// once per name and carries on, which is why they cost an effect rather than
/// a mission. This test pins the set: a *new* dangling name means the loader
/// has started resolving something wrongly.
#[test]
fn calls_resolve_within_their_level() {
    let Some(paths) = paths() else { return };

    let mut dangling = Vec::new();
    let mut seen = false;

    for level in 1..=13u16 {
        let Some(set) = anim(&paths, level) else {
            continue;
        };
        seen = true;

        for def in &set.defs {
            let events = def
                .sequences
                .iter()
                .flat_map(|sequence| &sequence.events)
                .chain(&def.reset_state);
            for event in events {
                let EventDataDesc::CallAnimation { name, .. } = &event.data else {
                    continue;
                };
                if set.lookup(name).is_empty() {
                    dangling.push(format!("m{level}: {} calls {name}", def.anim_name));
                }
            }
        }
    }

    if !seen {
        return;
    }

    let mut names: Vec<&str> = dangling
        .iter()
        .map(|entry| entry.rsplit(' ').next().unwrap_or(entry))
        .collect();
    names.sort_unstable();
    names.dedup();

    assert_eq!(
        names,
        vec![
            "cjunkdest02",
            "destroy_vtol1",
            "frc_twirl",
            "lavbub.flt",
            "pspipe_destroy_07",
            "stop_the_belt",
        ],
        "the game's own dangling calls, and no new ones"
    );
    assert_eq!(dangling.len(), 18, "how many calls dangle in all");
}

/// Almost every `CallSequence` names a sequence its own definition has.
///
/// One name does not: `run_skin_sequence`, called by `regen_skin` and
/// `shock_vehicle` in all 13 levels — the same pair of player-effect
/// definitions each time, and another of the original's leftovers. The VM
/// ignores a call it cannot resolve, so the effect simply does not play.
#[test]
fn sequence_calls_resolve_within_their_definition() {
    let Some(paths) = paths() else { return };

    let mut dangling = Vec::new();
    let mut seen = false;

    for level in 1..=13u16 {
        let Some(set) = anim(&paths, level) else {
            continue;
        };
        seen = true;

        for def in &set.defs {
            let has = |name: &str| def.sequences.iter().any(|sequence| sequence.name == name);
            for event in def.sequences.iter().flat_map(|sequence| &sequence.events) {
                let name = match &event.data {
                    EventDataDesc::CallSequence(name) | EventDataDesc::StopSequence(name) => name,
                    _ => continue,
                };
                if !has(name) {
                    dangling.push(format!("m{level}: {} calls sequence {name}", def.anim_name));
                }
            }
        }
    }

    if !seen {
        return;
    }

    let mut names: Vec<&str> = dangling
        .iter()
        .map(|entry| entry.rsplit(' ').next().unwrap_or(entry))
        .collect();
    names.sort_unstable();
    names.dedup();

    assert_eq!(
        names,
        vec!["run_skin_sequence"],
        "the game's one dangling sequence name, and no new ones"
    );
    // Two definitions in each of the 13 levels.
    assert_eq!(dangling.len(), 26);
}

/// Every campaign level has something that starts without being called.
///
/// `OnStartup` is the only activation that needs nothing else, so a campaign
/// level with none would be one where nothing ever begins. **m9 has none** —
/// the race map is started by its countdown gates rather than by scripting —
/// which is why this is asserted for the campaign and counted for the rest.
#[test]
fn every_campaign_level_has_something_that_starts_by_itself() {
    let Some(paths) = paths() else { return };

    let startups = |set: &AnimSet| {
        set.defs
            .iter()
            .filter(|def| def.activation == AnimActivation::OnStartup)
            .count()
    };

    for level in 1..=6u16 {
        let Some(set) = anim(&paths, level) else {
            continue;
        };
        assert!(startups(&set) > 0, "m{level} has no OnStartup definition");
    }

    // The multiplayer maps have 0–10; only m9's zero is worth naming.
    if let Some(set) = anim(&paths, 9) {
        assert_eq!(
            startups(&set),
            0,
            "m9 is the race map and scripts nothing at level start"
        );
    }
}

/// The `movers.zrd` names are real nodes, which is what the anim VM moves.
#[test]
fn every_mover_is_a_node_in_its_level() {
    let Some(paths) = paths() else { return };

    for level in 1..=6u16 {
        let (Some(script), Some(data)) = (script(&paths, level), gamez(&paths, level)) else {
            continue;
        };

        let names: std::collections::HashSet<&str> = data
            .nodes
            .iter()
            .map(|node| node.common().name.as_str())
            .collect();

        for mover in &script.movers {
            assert!(
                names.contains(mover.as_str()),
                "m{level}: mover `{mover}` is not a node"
            );
        }
    }
}

/// A multiplayer level has no mission, whatever its placeholder objectives
/// file says.
#[test]
fn a_multiplayer_level_has_no_usable_mission_of_its_own() {
    let Some(paths) = paths() else { return };

    // m7–m13 ship a byte-for-byte copy of m2's `objectives.zrd`, so the file
    // parses; it is the catalog's level kind, checked by `mission_setup`,
    // that keeps it from becoming a mission. This asserts the copy for what
    // it is, so a change in the data shows up here.
    let (Some(m2), Some(m7)) = (script(&paths, 2), script(&paths, 7)) else {
        return;
    };

    assert_eq!(
        m2.objectives.len(),
        m7.objectives.len(),
        "m7's objectives are m2's"
    );
    assert_eq!(m2.objectives[0].summary_key, m7.objectives[0].summary_key);
}

/// **A mission played through, on a real level.**
///
/// m1's definitions, m1's node names, the real VM: the entry animation is
/// called, the chain of `CallAnimation`s it starts runs, and nothing panics
/// on the 24 event kinds that have no handler yet.
#[test]
fn m1s_entry_animation_runs_through_the_real_vm() {
    use bevy::prelude::*;
    use bft_sim::anim::{AnimInstance, AnimProgram, AnimScopes, CallAnim};
    use bft_sim::level::{NodeActive, NodeIndex, NodeName};
    use bft_sim::schedule::SIM_HZ;

    let Some(paths) = paths() else { return };
    let (Some(set), Some(data)) = (anim(&paths, 1), gamez(&paths, 1)) else {
        return;
    };

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bft_sim::SimPlugin);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / SIM_HZ),
    ));
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));

    // Just the nodes, not the geometry: the VM addresses nodes by name, and
    // spawning 4199 名 entities with their meshes would make this a renderer
    // test rather than a VM one.
    let mut index = NodeIndex::default();
    for (id, node) in data.nodes.iter().enumerate() {
        let common = node.common();
        let entity = app
            .world_mut()
            .spawn((
                NodeName(common.name.clone()),
                NodeActive(common.flags.active),
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        index.insert(id as u32, &common.name, entity);
    }

    // One instance per definition, with no subtree scoping: the hierarchy is
    // `bft-game`'s to build, and every name in m1 resolves level-wide anyway
    // (the run below produces no unresolved-name warning).
    let scopes = set
        .defs
        .iter()
        .enumerate()
        .map(|(def_index, def)| {
            let root = index.by_name(&def.name).first().copied();
            app.world_mut()
                .spawn(AnimInstance::new(def_index, def.activation, root));
            std::collections::HashMap::new()
        })
        .collect();

    app.insert_resource(index);
    app.insert_resource(AnimScopes(scopes));
    app.insert_resource(AnimProgram(set.clone()));
    app.finish();
    app.update();

    // Call m1's entry animation the way `start_mission` does.
    app.world_mut()
        .resource_mut::<Messages<CallAnim>>()
        .write(CallAnim::new("start_single_player"));

    // Two seconds of real ticks. The intro camera and the VTOL drop are
    // minutes long, so this is the opening of it, which is where the
    // `CallAnimation` fan-out and the first timed offsets are.
    for _ in 0..(SIM_HZ as usize * 2) {
        app.update();
    }

    let mut running = 0usize;
    let mut completed = 0usize;
    for instance in app.world_mut().query::<&AnimInstance>().iter(app.world()) {
        if instance.is_running() {
            running += 1;
        }
        if instance.completed {
            completed += 1;
        }
    }

    assert!(
        running + completed > 1,
        "the entry animation started something: {running} running, \
         {completed} completed"
    );

    // And the nine seagull flocks are armed from the start, waiting on their
    // 40-metre range rather than on a call.
    let startups = set
        .defs
        .iter()
        .filter(|def| def.activation == AnimActivation::OnStartup)
        .count();
    assert_eq!(startups, 9, "m1's OnStartup definitions");
}
