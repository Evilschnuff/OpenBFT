//! The reader, messages and anim loaders against a real Recoil install.
//!
//! These skip (not fail) when `BFT_DATA_DIR` is unset or the cache has not
//! been built, so a contributor without the game still runs
//! `cargo test --workspace` green. No game data enters the repository: only
//! counts, invariants and the handful of values docs/data/anims.md and
//! READER-SCHEMA already publish are asserted.
//!
//! Build the cache first with `open-bft extract`.
//!
//! Every number below is a measurement recorded in docs/data/anims.md, run as a
//! test so a change to the loader — or a different mech3ax
//! build emitting a different shape — fails here rather than in a mission.

use std::path::PathBuf;

use bft_assets::DataPaths;
use bft_assets::anim::{
    AnimActivation, AnimSet, Condition, EventDataDesc, EventStart, Execution, OffsetFrom,
    ResetTime, parse_anim_set,
};
use bft_assets::messages::parse_messages;
use bft_assets::reader::{ReaderData, mission, parse_reader_data};

/// The resolved paths for a real install, or `None` when there is no data.
fn paths() -> Option<DataPaths> {
    let root = bft_assets::test_support::data_root()?;

    match DataPaths::discover(Some(&root), None, None) {
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

/// Read a cached file, or `None` when it is not there.
fn cached(paths: &DataPaths, relative: &str) -> Option<Vec<u8>> {
    let path: PathBuf = paths.cache.path.join(relative);
    if !path.is_file() {
        eprintln!("{} is not in the cache; skipping", path.display());
        return None;
    }
    Some(std::fs::read(&path).expect("the cache is readable"))
}

/// One level's animation definitions.
fn anim(paths: &DataPaths, level: u16) -> Option<AnimSet> {
    let bytes = cached(paths, &format!("anim/m{level}-anim.anim.zip"))?;
    Some(parse_anim_set(&bytes).expect("a real anim archive parses"))
}

/// One level's reader archive.
fn reader(paths: &DataPaths, level: u16) -> Option<ReaderData> {
    let bytes = cached(paths, &format!("reader/m{level}-zrdr.rdr.zip"))?;
    Some(parse_reader_data(&bytes).expect("a real reader archive parses"))
}

/// Every level's def count, from docs/data/anims.md's survey.
const DEFS_PER_LEVEL: [(u16, usize); 13] = [
    (1, 443),
    (2, 387),
    (3, 304),
    (4, 534),
    (5, 390),
    (6, 818),
    (7, 172),
    (8, 174),
    (9, 426),
    (10, 234),
    (11, 182),
    (12, 242),
    (13, 492),
];

/// The whole vocabulary parses, on every level, with no def lost.
///
/// 4798 defs is the total docs/data/anims.md measured; a mirror that silently
/// dropped one variant would show up here as a short count rather than as a
/// mission that quietly does nothing.
#[test]
fn every_definition_in_every_level_parses() {
    let Some(paths) = paths() else { return };

    let mut total = 0usize;
    for (level, expected) in DEFS_PER_LEVEL {
        let Some(set) = anim(&paths, level) else {
            continue;
        };

        assert_eq!(
            set.defs.len(),
            expected,
            "m{level} should have {expected} definitions"
        );
        assert_eq!(set.gravity, -9.8, "m{level}'s anim gravity");
        total += set.defs.len();
    }

    if total > 0 {
        assert_eq!(total, 4798, "every level's definitions together");
    }
}

/// The five activations, in the proportions docs/data/anims.md measured over all 13 levels.
#[test]
fn the_activation_mix_is_the_one_the_data_has() {
    let Some(paths) = paths() else { return };

    let mut counts = std::collections::HashMap::new();
    let mut seen = false;
    for (level, _) in DEFS_PER_LEVEL {
        let Some(set) = anim(&paths, level) else {
            continue;
        };
        seen = true;
        for def in &set.defs {
            *counts.entry(def.activation).or_insert(0usize) += 1;
        }
    }
    if !seen {
        return;
    }

    assert_eq!(counts.get(&AnimActivation::OnCall), Some(&2897));
    assert_eq!(counts.get(&AnimActivation::WeaponHit), Some(&1339));
    assert_eq!(counts.get(&AnimActivation::WeaponOrCollideHit), Some(&331));
    assert_eq!(counts.get(&AnimActivation::OnStartup), Some(&121));
    assert_eq!(counts.get(&AnimActivation::CollideHit), Some(&110));
}

/// **The finding that matters most.** Every `ByRange` bound and every
/// `PlayerRange` condition in the game is an exact square of a round number,
/// which is what says the field holds squared metres rather than metres.
///
/// Reading them as metres puts m1's seagulls 1600 m away instead of 40 and
/// the mission exit trigger at 30 m instead of 5.5 — both silently wrong.
#[test]
fn every_range_in_the_game_is_a_squared_distance() {
    let Some(paths) = paths() else { return };

    /// Whether a value is the square of a number that lands on a half metre,
    /// which is how every range in the game is authored (5.5, 12, 22.5, 400).
    fn is_round_square(value: f32) -> bool {
        let root = value.sqrt();
        (root * 2.0).fract().abs() < 1e-3
    }

    let mut ranges = 0usize;
    let mut conditions = 0usize;

    for (level, _) in DEFS_PER_LEVEL {
        let Some(set) = anim(&paths, level) else {
            continue;
        };

        for def in &set.defs {
            if let Execution::ByRange { min_sq, max_sq } = def.execution {
                assert_eq!(min_sq, 0.0, "m{level} {}: every min is 0", def.anim_name);
                assert!(
                    is_round_square(max_sq),
                    "m{level} {}: max {max_sq} is not a round square",
                    def.anim_name
                );
                ranges += 1;
            }

            for sequence in &def.sequences {
                for event in &sequence.events {
                    let condition = match &event.data {
                        EventDataDesc::If(c) | EventDataDesc::Elseif(c) => c,
                        _ => continue,
                    };
                    if let Condition::PlayerRange(range_sq) = condition {
                        assert!(
                            is_round_square(*range_sq),
                            "m{level} {}: PlayerRange {range_sq} is not a round square",
                            def.anim_name
                        );
                        conditions += 1;
                    }
                }
            }
        }
    }

    if ranges + conditions == 0 {
        return;
    }
    assert_eq!(ranges, 90, "ByRange executions in the game");
    assert_eq!(conditions, 666, "PlayerRange conditions in the game");
}

/// The resource lists are arrays of records; reading them as bare strings
/// empties every one of them without an error.
#[test]
fn resource_lists_carry_their_names() {
    let Some(paths) = paths() else { return };
    let Some(set) = anim(&paths, 1) else { return };

    // m1's player death animation names the camera, the shadow and the three
    // body parts, and three sounds.
    let def = set
        .find("bft_00")
        .expect("m1 has the player's bft_00 definition");

    assert!(
        def.objects.iter().any(|name| name == "turret"),
        "objects: {:?}",
        def.objects
    );
    assert!(
        def.nodes.iter().any(|name| name == "chassis"),
        "nodes: {:?}",
        def.nodes
    );
    assert!(
        def.static_sounds
            .iter()
            .any(|name| name.starts_with("snd_")),
        "static sounds: {:?}",
        def.static_sounds
    );
    assert_eq!(def.anim_root_name, "body");
}

/// `reset_state` is where a level's initial node visibility comes from, and
/// it is wrapped in an object the loader has to unwrap.
#[test]
fn reset_states_are_unwrapped_and_present_on_most_definitions() {
    let Some(paths) = paths() else { return };

    let mut with_reset = 0usize;
    let mut seen = false;
    for (level, _) in DEFS_PER_LEVEL {
        let Some(set) = anim(&paths, level) else {
            continue;
        };
        seen = true;
        with_reset += set
            .defs
            .iter()
            .filter(|def| !def.reset_state.is_empty())
            .count();
    }

    if seen {
        assert_eq!(with_reset, 3108, "definitions carrying a reset state");
    }
}

/// `anim_name` collides, so the index has to hold every def that carries one.
#[test]
fn repeated_anim_names_are_all_indexed() {
    let Some(paths) = paths() else { return };
    let Some(set) = anim(&paths, 1) else { return };

    // Six names repeat in m1: five effect templates (`spark_blue.flt` ×3)
    // and `fuel_station`, which is two copies of one prop's script.
    let repeated: Vec<&String> = set
        .by_anim_name
        .iter()
        .filter(|(_, defs)| defs.len() > 1)
        .map(|(name, _)| name)
        .collect();

    assert_eq!(repeated.len(), 6, "m1's repeated names: {repeated:?}");
    // Every index the multi-map holds points at a definition carrying that
    // name, which is the invariant `CallAnimation` relies on.
    for name in repeated {
        for &index in set.lookup(name) {
            assert_eq!(&set.defs[index].anim_name, name);
        }
    }

    // And the names that come from reader data never do, which is what makes
    // `find` safe for them.
    for name in ["start_single_player", "load_control", "m1_exit_animation"] {
        assert_eq!(set.lookup(name).len(), 1, "{name} should be unique");
    }
}

/// `Callback 99` is the win signal: exactly the six `mN_exit_animation`
/// definitions carry it, and nothing else in the game does.
///
/// `Callback 2` — which docs/data/anims.md first proposed — also fires in
/// `mN_start_animation`, so keying the win on it ends the mission during the
/// opening camera.
#[test]
fn the_mission_end_callback_is_only_in_the_exit_animations() {
    let Some(paths) = paths() else { return };

    let mut carriers = Vec::new();
    let mut seen = false;
    for (level, _) in DEFS_PER_LEVEL {
        let Some(set) = anim(&paths, level) else {
            continue;
        };
        seen = true;
        for def in &set.defs {
            let has_99 = def.sequences.iter().any(|sequence| {
                sequence
                    .events
                    .iter()
                    .any(|event| matches!(event.data, EventDataDesc::Callback(99)))
            });
            if has_99 {
                carriers.push(def.anim_name.clone());
            }
        }
    }
    if !seen {
        return;
    }

    carriers.sort();
    assert_eq!(
        carriers,
        vec![
            "m1_exit_animation",
            "m2_exit_animation",
            "m3_exit_animation",
            "m4_exit_animation",
            "m5_exit_animation",
            "m6_exit_animation",
        ]
    );
}

/// Every `OnStartup` definition is reachable and its sequences are `Initial`,
/// which is what makes "arm at level start, run at once" the right reading.
#[test]
fn on_startup_definitions_run_their_initial_sequences() {
    let Some(paths) = paths() else { return };
    let Some(set) = anim(&paths, 1) else { return };

    let startups: Vec<_> = set
        .defs
        .iter()
        .filter(|def| def.activation == AnimActivation::OnStartup)
        .collect();

    // m1's nine seagull flocks, at 40 m (1600 squared).
    assert_eq!(startups.len(), 9);
    for def in startups {
        assert_eq!(
            def.execution,
            Execution::ByRange {
                min_sq: 0.0,
                max_sq: 1600.0
            }
        );
        assert_eq!(def.reset_time, ResetTime::Never);
        assert!(
            def.sequences.iter().any(|sequence| !sequence.on_call),
            "{} has an Initial sequence to run",
            def.anim_name
        );
    }
}

/// The three start offsets survive, with the proportions docs/data/anims.md measured.
#[test]
fn event_start_offsets_keep_their_kind() {
    let Some(paths) = paths() else { return };

    let mut with_previous = 0usize;
    let mut offsets = std::collections::HashMap::new();
    let mut seen = false;

    for (level, _) in DEFS_PER_LEVEL {
        let Some(set) = anim(&paths, level) else {
            continue;
        };
        seen = true;
        for def in &set.defs {
            for sequence in &def.sequences {
                for event in &sequence.events {
                    match event.start {
                        EventStart::WithPrevious => with_previous += 1,
                        EventStart::Offset { from, .. } => {
                            *offsets.entry(from).or_insert(0usize) += 1
                        }
                    }
                }
            }
        }
    }
    if !seen {
        return;
    }

    assert_eq!(with_previous, 41_171, "events with no offset of their own");
    assert_eq!(offsets.get(&OffsetFrom::Animation), Some(&2459));
    assert_eq!(offsets.get(&OffsetFrom::Event), Some(&2262));
    assert_eq!(offsets.get(&OffsetFrom::Sequence), Some(&649));
}

/// The six campaign missions' objectives read, with the record variations the
/// shipped files actually have.
#[test]
fn every_campaign_mission_reads_its_objectives() {
    let Some(paths) = paths() else { return };

    // m1 4, m2 5, m3 6, m4 5, m5 6, m6 5 — 31 records for the 32 objective
    // message keys docs/data/messages.md counted, because m3's objective 5 has no
    // `COMP` key.
    let expected = [(1, 4), (2, 5), (3, 6), (4, 5), (5, 6), (6, 5)];

    for (level, count) in expected {
        let Some(data) = reader(&paths, level) else {
            continue;
        };

        let parsed = mission::parse(&data).expect("a campaign mission parses");

        assert_eq!(
            parsed.objectives.len(),
            count,
            "m{level} should have {count} objectives"
        );
        assert_eq!(parsed.read_time, 8.0, "m{level} read time");
        assert_eq!(parsed.review_sound, "snd_review");
        assert!(
            !parsed.new_game_anim.is_empty(),
            "m{level} names its new-game animation"
        );
        assert_eq!(parsed.load_game_anim, "load_control");

        // Only m6 is the last one.
        assert_eq!(parsed.final_mission, level == 6, "m{level} FINAL_MISSION");

        for (index, objective) in parsed.objectives.iter().enumerate() {
            assert!(
                !objective.picture.is_empty(),
                "m{level} objective {index} names a briefing picture"
            );
            assert!(
                objective.summary_key.starts_with("MSG_"),
                "m{level} objective {index} summary key: {}",
                objective.summary_key
            );
            assert!(
                objective.text_key.starts_with("MSG_"),
                "m{level} objective {index} text key: {}",
                objective.text_key
            );
        }
    }
}

/// The exit objective — no state word, no completion node — is the last one
/// in five of the six missions, and m6 has none because it ends differently.
#[test]
fn the_exit_objective_is_last_and_only_m6_has_none() {
    let Some(paths) = paths() else { return };

    for level in 1..=6u16 {
        let Some(data) = reader(&paths, level) else {
            continue;
        };
        let parsed = mission::parse(&data).expect("parses");

        let exits: Vec<usize> = parsed
            .objectives
            .iter()
            .enumerate()
            .filter(|(_, objective)| objective.is_exit())
            .map(|(index, _)| index)
            .collect();

        if level == 6 {
            assert!(exits.is_empty(), "m6 has no exit objective");
            continue;
        }

        assert_eq!(
            exits,
            vec![parsed.objectives.len() - 1],
            "m{level}'s exit objective is its last"
        );
    }
}

/// Every non-exit objective names at least one completion node, and the
/// two-element form is `[object, child]`.
#[test]
fn completion_conditions_name_nodes() {
    let Some(paths) = paths() else { return };

    let mut pairs = 0usize;
    let mut singles = 0usize;
    let mut seen = false;

    for level in 1..=6u16 {
        let Some(data) = reader(&paths, level) else {
            continue;
        };
        seen = true;
        let parsed = mission::parse(&data).expect("parses");

        for (index, objective) in parsed.objectives.iter().enumerate() {
            if objective.is_exit() {
                continue;
            }
            match objective.completion_nodes.len() {
                1 => singles += 1,
                2 => pairs += 1,
                other => panic!(
                    "m{level} objective {index} has {other} completion nodes: {:?}",
                    objective.completion_nodes
                ),
            }
        }
    }

    if seen {
        // 26 conditions over the six missions' 31 objective records: 21 name
        // one node, five name an `[object, child]` pair, and the remaining
        // five records are the exit objectives with no node at all.
        assert_eq!(singles, 21);
        assert_eq!(pairs, 5);
    }
}

/// `NEW_GAME_START` is `start_single_player` only in m1; defaulting to it
/// would leave five of the six missions with nothing to run.
#[test]
fn only_m1_starts_with_start_single_player() {
    let Some(paths) = paths() else { return };

    for level in 1..=6u16 {
        let Some(data) = reader(&paths, level) else {
            continue;
        };
        let parsed = mission::parse(&data).expect("parses");

        let expected = if level == 1 {
            "start_single_player".to_string()
        } else {
            format!("m{level}_start_animation")
        };
        assert_eq!(parsed.new_game_anim, expected, "m{level}");
    }
}

/// Every anim name a mission reaches for by name resolves in that mission's
/// archive — the entry animations, the exit animation and the death one.
#[test]
fn the_mission_entry_and_exit_animations_resolve() {
    let Some(paths) = paths() else { return };

    for level in 1..=6u16 {
        let (Some(data), Some(set)) = (reader(&paths, level), anim(&paths, level)) else {
            continue;
        };
        let parsed = mission::parse(&data).expect("parses");

        for name in [&parsed.new_game_anim, &parsed.load_game_anim] {
            assert!(
                set.find(name).is_some(),
                "m{level}: {name} is not a definition in this level"
            );
        }

        // The death animation is `bft_00`, not `deathmN`: that is the reader
        // *source* file's name, and the compiled definition is named for its
        // root object.
        assert!(
            set.find("bft_00").is_some(),
            "m{level} has the player's death animation"
        );
        assert!(
            set.find(&format!("m{level}_exit_animation")).is_some(),
            "m{level} has its exit animation"
        );
    }
}

/// The message table loads with the keys the mission and briefing screens ask
/// for by name.
#[test]
fn the_message_table_carries_the_mission_strings() {
    let Some(paths) = paths() else { return };
    let Some(bytes) = cached(&paths, "messages.json") else {
        return;
    };

    let messages = parse_messages(&bytes).expect("the real table parses");

    assert_eq!(messages.language_id, 1033, "US English");
    assert_eq!(messages.len(), 749);

    // The six campaign titles and every objective string the six missions
    // reference (docs/data/messages.md: 32 objectives over six missions).
    for campaign in 1..=6 {
        assert!(
            messages
                .get(&format!("MSG_CAMPAIGN{campaign}_NAME"))
                .is_some(),
            "campaign {campaign} has a title"
        );
    }

    for key in ["MSG_OBJECTIVES", "MSG_GAME_OVER", "MSG_LOAD_NEXT_MISSION"] {
        assert!(messages.get(key).is_some(), "{key} is in the table");
    }
}

/// Every `MSG_*` key the objectives name is in the table, which is what makes
/// the briefing screen buildable from data alone.
#[test]
fn every_objective_message_key_resolves() {
    let Some(paths) = paths() else { return };
    let Some(bytes) = cached(&paths, "messages.json") else {
        return;
    };
    let messages = parse_messages(&bytes).expect("parses");

    for level in 1..=6u16 {
        let Some(data) = reader(&paths, level) else {
            continue;
        };
        let parsed = mission::parse(&data).expect("parses");

        for (index, objective) in parsed.objectives.iter().enumerate() {
            for key in [&objective.summary_key, &objective.text_key] {
                assert!(
                    messages.get(key).is_some(),
                    "m{level} objective {index}: {key} is not in messages.dll"
                );
            }
            // `COMP` is absent on m3's objective 5, and empty is not a key.
            if !objective.complete_key.is_empty() {
                assert!(
                    messages.get(&objective.complete_key).is_some(),
                    "m{level} objective {index}: {} is not in messages.dll",
                    objective.complete_key
                );
            }
        }
    }
}

/// The reader archive holds what the vehicle, weapon and AI tables read out
/// of it, and the player start too.
#[test]
fn the_reader_archive_holds_the_tables_and_the_player_start() {
    let Some(paths) = paths() else { return };

    let Some(bytes) = cached(&paths, "reader/zrdr.rdr.zip") else {
        return;
    };
    let global = parse_reader_data(&bytes).expect("the global archive parses");
    assert_eq!(global.len(), 127, "global reader entries");
    for entry in ["vehicle.zrd", "player.zrd", "weapons.zrd", "pickup.zrd"] {
        assert!(global.entry(entry).is_some(), "{entry} is in the archive");
    }

    for level in 1..=13u16 {
        let Some(data) = reader(&paths, level) else {
            continue;
        };
        assert!(
            data.player_start().is_some(),
            "m{level} states a player start in aiv.zrd"
        );
    }
}
