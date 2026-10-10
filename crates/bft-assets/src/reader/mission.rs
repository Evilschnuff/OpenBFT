//! `objectives.zrd` + `startanims.zrd` + `movers.zrd` → [`RawMission`].
//!
//! A typed reader view this crate owns. Most others — `vehicle.zrd`,
//! `weapons.zrd`, `ai.zrd` — are read by the `bft-sim` module that needs
//! them (READER-SCHEMA §2, §3).
//!
//! # The record shape, and why it is walked rather than indexed
//!
//! READER-SCHEMA §3.6 gives `OBJECTIVEn` as a fixed positional record:
//! picture, three `MSG_*` keys, state, then keyword/value pairs. The shipped
//! files are looser than that in three ways, all of which break a parser that
//! counts fields:
//!
//! - **The state is optional.** The last objective of m1, m2, m3 and m4 — the
//!   "get to the landing site" one — has no state and no completion node; it
//!   ends the mission through the exit anim instead.
//! - **The `COMP` key is optional.** m3's objective 5 carries only `SUMMARY`
//!   and `OBJ`.
//! - **New keywords.** m6 adds `AUTOPLAY` inside two records and
//!   `FINAL_MISSION` at file level; `HINT_SOUND` is also spelled `HINT_SND`.
//!
//! So [`parse_objective`] reads the leading strings up to the first known
//! keyword or state word, and then pairs each keyword with the next unclaimed
//! list. That tolerates a record this file has not seen.

use crate::reader_value::ReaderValue;

use super::ReaderData;

/// The `objectives.zrd` entry.
pub const OBJECTIVES_ENTRY: &str = "objectives.zrd";
/// The `startanims.zrd` entry — the mission script's entry point.
pub const STARTANIMS_ENTRY: &str = "startanims.zrd";
/// The `movers.zrd` entry — node names that move at runtime.
pub const MOVERS_ENTRY: &str = "movers.zrd";

/// Everything the mission systems read out of one level's reader archive.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawMission {
    /// The objectives, in `OBJECTIVE1`…`OBJECTIVEn` order. The index is also
    /// the zmap feature's `objective` id (0-based).
    pub objectives: Vec<RawObjective>,
    /// How long the briefing text stays up, in seconds. 8 in all six.
    pub read_time: f32,
    /// Delay before the objective review plays, in seconds. 1–5.
    pub review_delay: f32,
    /// The logical sound name for the review cue.
    pub review_sound: String,
    /// The logical sound name played when an objective changes.
    pub objective_sound: String,
    /// `FINAL_MISSION [1]`: m6 alone. The campaign ends rather than loading
    /// a next mission.
    pub final_mission: bool,
    /// `NEW_GAME_START`'s anim name. **Read, never defaulted**:
    /// `start_single_player` in m1 and `mN_start_animation` in m2–m6.
    pub new_game_anim: String,
    /// `LOAD_GAME_START`'s anim name. `load_control` in all six.
    pub load_game_anim: String,
    /// `movers.zrd`: nodes that move and so need per-frame partition updates.
    pub movers: Vec<String>,
}

/// One objective, straight out of the file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RawObjective {
    /// Briefing picture stem in `image.zbd` (`obj1fgate`).
    pub picture: String,
    /// `MSG_M<n>_O<k>_SUMMARY`: the one-line automap label.
    pub summary_key: String,
    /// `MSG_M<n>_O<k>_OBJ`: the briefing paragraph.
    pub text_key: String,
    /// `MSG_M<n>_O<k>_COMP`: shown on completion. Absent on m3's objective 5,
    /// so empty rather than assumed.
    pub complete_key: String,
    /// Whether the record said `ACTIVE`. `INACTIVE` and *absent* are both
    /// false; [`RawObjective::has_state`] tells them apart.
    pub initially_active: bool,
    /// Whether the record carried a state word at all. The four exit
    /// objectives do not.
    pub has_state: bool,
    /// The completion node: `[node]` or `[object, child]`, where `child` is
    /// scoped to `object`'s subtree. Empty on an exit objective.
    pub completion_nodes: Vec<String>,
    /// `READ_SOUND [snd]` or `[snd, delay_s]`.
    pub read_sound: Option<String>,
    /// The delay on `READ_SOUND`, where the record gives one.
    pub read_delay: Option<f32>,
    /// `HINT_SOUND`/`HINT_SND [snd]`.
    pub hint_sound: Option<String>,
    /// `HINT_TIME [s]`: how long before the hint plays.
    pub hint_time: Option<f32>,
    /// `HINT_ZONE [id]`: the zone the player must be in for the hint.
    pub hint_zone: Option<i32>,
    /// `HINT_ACTIVE` was present.
    pub hint_active: bool,
    /// `AUTOPLAY` was present (m6's first two objectives).
    pub autoplay: bool,
}

impl RawObjective {
    /// Whether this is the mission's exit objective: no state, no node.
    ///
    /// The four missions that have one (m1, m2, m3, m4 — m5 too) end when
    /// the exit animation fires `Callback 99`, not when a node flips.
    pub fn is_exit(&self) -> bool {
        !self.has_state && self.completion_nodes.is_empty()
    }
}

/// What can go wrong reading a level's mission data.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum MissionError {
    /// Multiplayer maps ship a placeholder copy of m2's, and m13 ships none;
    /// a campaign level always has one.
    #[error("no {OBJECTIVES_ENTRY} in this level's reader archive")]
    NoObjectives,
    /// Every campaign mission has one, and without it nothing starts.
    #[error("no {STARTANIMS_ENTRY} in this level's reader archive")]
    NoStartAnims,
    #[error("{STARTANIMS_ENTRY} has no {0}")]
    NoStartAnim(&'static str),
    #[error("{OBJECTIVES_ENTRY} defines no objectives")]
    NoObjectiveRecords,
}

/// The keywords an `OBJECTIVEn` record may carry after its leading strings.
///
/// Order does not matter — each is paired with the next unclaimed list — but
/// membership does: a string that is not one of these and not a state word is
/// still part of the leading `MSG_*` run.
const KEYWORDS: &[&str] = &[
    "READ_SOUND",
    "HINT_SOUND",
    "HINT_SND",
    "HINT_TIME",
    "HINT_ZONE",
    "HINT_ACTIVE",
    "AUTOPLAY",
];

/// Read one level's mission data.
pub fn parse(level: &ReaderData) -> Result<RawMission, MissionError> {
    let objectives = level
        .read(OBJECTIVES_ENTRY)
        .ok_or(MissionError::NoObjectives)?;
    let start = level
        .read(STARTANIMS_ENTRY)
        .ok_or(MissionError::NoStartAnims)?;

    let new_game_anim = start
        .get("NEW_GAME_START")
        .and_then(|value| value.string())
        .ok_or(MissionError::NoStartAnim("NEW_GAME_START"))?
        .to_string();
    let load_game_anim = start
        .get("LOAD_GAME_START")
        .and_then(|value| value.string())
        .ok_or(MissionError::NoStartAnim("LOAD_GAME_START"))?
        .to_string();

    // `OBJECTIVE1`, `OBJECTIVE2`, … in file order, stopping at the first gap
    // rather than scanning every key: the numbering is dense in all six and a
    // gap would mean a file this parser does not understand.
    let mut records = Vec::new();
    for index in 1.. {
        let Some(record) = objectives.get(&format!("OBJECTIVE{index}")) else {
            break;
        };
        records.push(parse_objective(record));
    }

    if records.is_empty() {
        return Err(MissionError::NoObjectiveRecords);
    }

    Ok(RawMission {
        objectives: records,
        read_time: objectives
            .get("READ_TIME")
            .and_then(|value| value.float())
            .unwrap_or(0.0),
        review_delay: objectives
            .get("REVIEW_DELAY")
            .and_then(|value| value.float())
            .unwrap_or(0.0),
        review_sound: string_or_empty(objectives.get("REVIEW_SOUND")),
        objective_sound: string_or_empty(objectives.get("OBJECTIVE_SOUND")),
        final_mission: objectives
            .get("FINAL_MISSION")
            .and_then(|value| value.flag())
            .unwrap_or(false),
        new_game_anim,
        load_game_anim,
        movers: level
            .read(MOVERS_ENTRY)
            .map(|value| {
                value
                    .strings()
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
    })
}

fn string_or_empty(value: Option<ReaderValue<'_>>) -> String {
    value
        .and_then(|value| value.string())
        .unwrap_or_default()
        .to_string()
}

/// Read one `OBJECTIVEn` positional record.
///
/// Three passes over the elements, because the record mixes three kinds of
/// thing in one flat list:
///
/// 1. the leading bare strings (picture, `MSG_*` keys) up to the state word
///    or the first keyword;
/// 2. the state word, if there is one;
/// 3. keyword strings, each taking the next list that has not been claimed.
///
/// The completion condition is the first list in the record, claimed before
/// any keyword gets one — which is what makes an exit objective (no list
/// before `READ_SOUND`) come out with no nodes rather than stealing the
/// read sound.
pub fn parse_objective(record: ReaderValue<'_>) -> RawObjective {
    let items = record.items();

    // The strings, in order, and the lists, in order: the record interleaves
    // them but each sequence is read independently.
    let mut leading: Vec<&str> = Vec::new();
    let mut keywords: Vec<&str> = Vec::new();
    let mut state: Option<bool> = None;
    let mut lists: Vec<ReaderValue<'_>> = Vec::new();
    // Keywords whose value was an inline `null` rather than a trailing list.
    let mut valueless: Vec<&str> = Vec::new();

    for item in &items {
        // The *JSON* type, not `ReaderValue::string`: the collapse that reads
        // `["frcoff"]` as the string `"frcoff"` is right everywhere else and
        // exactly wrong here, because it turns the completion condition into
        // a fourth `MSG_*` key.
        match item.raw() {
            serde_json::Value::String(text) if KEYWORDS.contains(&text.as_str()) => {
                keywords.push(text)
            }
            serde_json::Value::String(text) if text == "ACTIVE" => state = Some(true),
            serde_json::Value::String(text) if text == "INACTIVE" => state = Some(false),
            // A bare string before any keyword is one of the leading fields.
            // After the keywords start, a loose string would be a record
            // shape this parser has not seen; it is ignored rather than
            // shifting everything after it.
            serde_json::Value::String(text) if keywords.is_empty() && state.is_none() => {
                leading.push(text)
            }
            serde_json::Value::String(_) => {}
            // A `null` **inline** after a valueless keyword (`HINT_ACTIVE`,
            // null / `AUTOPLAY`, null) is that keyword's own empty value and
            // is consumed where it stands. The trailing values that follow
            // the keyword run are a separate sequence, and folding the null
            // into it would shift every one of them by a place — which is
            // what turned m1's objective 2 into an objective with no node.
            serde_json::Value::Null => valueless.extend(keywords.last().copied()),
            _ => lists.push(*item),
        }
    }

    // Each keyword that took an inline null is dropped from the run that
    // claims trailing values, having already been satisfied.
    keywords.retain(|keyword| !valueless.contains(keyword));

    let field = |index: usize| leading.get(index).copied().unwrap_or_default().to_string();

    // The completion condition is the first list, when the record has a state
    // word — an exit objective has none and its first list is the read sound.
    let mut next_list = 0usize;
    let completion_nodes = if state.is_some() {
        let nodes = lists
            .get(next_list)
            .map(|value| {
                value
                    .strings()
                    .into_iter()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        next_list += 1;
        nodes
    } else {
        Vec::new()
    };

    let mut objective = RawObjective {
        picture: field(0),
        summary_key: field(1),
        text_key: field(2),
        complete_key: field(3),
        initially_active: state.unwrap_or(false),
        has_state: state.is_some(),
        completion_nodes,
        ..Default::default()
    };

    for keyword in keywords {
        let value = lists.get(next_list).copied();
        next_list += 1;

        match keyword {
            "READ_SOUND" => {
                objective.read_sound = value
                    .and_then(|v| v.nth(0).or(Some(v)))
                    .and_then(|v| v.string())
                    .map(str::to_string);
                // `["snd_m1obj1", 24]`: the second element is a delay.
                objective.read_delay = value.and_then(|v| v.nth(1)).and_then(|v| v.float());
            }
            "HINT_SOUND" | "HINT_SND" => {
                objective.hint_sound = value.and_then(|v| v.string()).map(str::to_string);
            }
            "HINT_TIME" => objective.hint_time = value.and_then(|v| v.float()),
            "HINT_ZONE" => objective.hint_zone = value.and_then(|v| v.int()).map(|v| v as i32),
            "HINT_ACTIVE" => objective.hint_active = true,
            "AUTOPLAY" => objective.autoplay = true,
            _ => {}
        }
    }

    // The valueless ones are flags and nothing else, so they only set theirs.
    for keyword in valueless {
        match keyword {
            "HINT_ACTIVE" => objective.hint_active = true,
            "AUTOPLAY" => objective.autoplay = true,
            _ => {}
        }
    }

    objective
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn value(json: &str) -> Value {
        serde_json::from_str(json).expect("valid test JSON")
    }

    fn objective(json: &str) -> RawObjective {
        let value = value(json);
        parse_objective(ReaderValue::new(&value))
    }

    fn level(objectives: &str, startanims: &str) -> ReaderData {
        ReaderData::from_entries(
            [
                (OBJECTIVES_ENTRY.to_string(), value(objectives)),
                (STARTANIMS_ENTRY.to_string(), value(startanims)),
            ]
            .into_iter()
            .collect(),
        )
    }

    const STARTANIMS: &str =
        r#"[["NEW_GAME_START", ["start_single_player"], "LOAD_GAME_START", ["load_control"]]]"#;

    /// m1's objective 1, verbatim from the shipped file.
    #[test]
    fn a_full_record_reads_every_field() {
        let o = objective(
            r#"["obj1fgate", "MSG_M1_O1_SUMMARY", "MSG_M1_O1_OBJ", "MSG_M1_O1_COMP",
                "ACTIVE", "READ_SOUND", "HINT_SOUND", "HINT_TIME",
                ["frcoff"], ["snd_m1obj1", 24], ["snd_m1hint1"], [180]]"#,
        );

        assert_eq!(o.picture, "obj1fgate");
        assert_eq!(o.summary_key, "MSG_M1_O1_SUMMARY");
        assert_eq!(o.text_key, "MSG_M1_O1_OBJ");
        assert_eq!(o.complete_key, "MSG_M1_O1_COMP");
        assert!(o.initially_active);
        assert!(o.has_state);
        assert_eq!(o.completion_nodes, vec!["frcoff"]);
        assert_eq!(o.read_sound.as_deref(), Some("snd_m1obj1"));
        assert_eq!(o.read_delay, Some(24.0));
        assert_eq!(o.hint_sound.as_deref(), Some("snd_m1hint1"));
        assert_eq!(o.hint_time, Some(180.0));
        assert!(!o.is_exit());
    }

    /// m1's objective 2: the `HINT_SND` spelling, `HINT_ACTIVE` with a null
    /// value, and `HINT_ZONE` after it.
    #[test]
    fn the_hint_snd_spelling_and_a_valueless_keyword_are_handled() {
        let o = objective(
            r#"["comandlw", "MSG_M1_O2_SUMMARY", "MSG_M1_O2_OBJ", "MSG_M1_O2_COMP",
                "INACTIVE", "READ_SOUND", "HINT_SND", "HINT_TIME", "HINT_ACTIVE", null,
                "HINT_ZONE", ["nb_force"], ["snd_m1obj2"], ["snd_m1hint2"], [120], [2]]"#,
        );

        assert!(!o.initially_active);
        assert!(o.has_state);
        assert_eq!(o.completion_nodes, vec!["nb_force"]);
        assert_eq!(o.hint_sound.as_deref(), Some("snd_m1hint2"));
        assert_eq!(o.hint_time, Some(120.0));
        assert_eq!(o.hint_zone, Some(2));
        assert!(o.hint_active);
    }

    /// The exit objective (m1's 4th): no state word and no completion list,
    /// so the read sound must not be misread as a node name.
    #[test]
    fn an_exit_objective_has_no_state_and_no_nodes() {
        let o = objective(
            r#"["helolw", "MSG_M1_O4_SUMMARY", "MSG_M1_O4_OBJ", "MSG_M1_O4_COMP",
                "READ_SOUND", ["snd_m1_site3"]]"#,
        );

        assert!(!o.has_state);
        assert!(o.completion_nodes.is_empty());
        assert_eq!(o.read_sound.as_deref(), Some("snd_m1_site3"));
        assert!(o.is_exit());
    }

    /// m3's objective 5 has only three `MSG_*` keys.
    #[test]
    fn a_record_without_a_completion_key_leaves_it_empty() {
        let o = objective(
            r#"["vconelw", "MSG_M3_O5_SUMMARY", "MSG_M3_O5_OBJ", "INACTIVE",
                "READ_SOUND", ["force"], ["snd_m3obj5"]]"#,
        );

        assert_eq!(o.summary_key, "MSG_M3_O5_SUMMARY");
        assert_eq!(o.text_key, "MSG_M3_O5_OBJ");
        assert_eq!(o.complete_key, "");
        assert_eq!(o.completion_nodes, vec!["force"]);
        assert_eq!(o.read_sound.as_deref(), Some("snd_m3obj5"));
    }

    /// m2's objective 2: `[object, child]`, where the child is scoped to the
    /// object's subtree.
    #[test]
    fn a_two_element_condition_keeps_both_names() {
        let o = objective(
            r#"["powextlw", "MSG_M2_O2_SUMMARY", "MSG_M2_O2_OBJ", "MSG_M2_O2_COMP",
                "ACTIVE", "READ_SOUND", ["powstation", "powerbad"], ["snd_m2obj2"]]"#,
        );

        assert_eq!(o.completion_nodes, vec!["powstation", "powerbad"]);
    }

    /// m6's objective 1: `AUTOPLAY` with a null value.
    #[test]
    fn autoplay_is_recognised() {
        let o = objective(
            r#"["obj1elv", "MSG_M6_O1_SUMMARY", "MSG_M6_O1_OBJ", "MSG_M6_O1_COMP",
                "ACTIVE", "READ_SOUND", "AUTOPLAY", null,
                ["btrigdownoff"], ["snd_m6obj1", 23]]"#,
        );

        assert!(o.autoplay);
        assert_eq!(o.completion_nodes, vec!["btrigdownoff"]);
        assert_eq!(o.read_sound.as_deref(), Some("snd_m6obj1"));
        assert_eq!(o.read_delay, Some(23.0));
    }

    #[test]
    fn a_whole_level_reads_its_objectives_in_order() {
        let level = level(
            r#"[["READ_TIME", [8], "REVIEW_DELAY", [4],
                "REVIEW_SOUND", ["snd_review"], "OBJECTIVE_SOUND", ["snd_objective"],
                "OBJECTIVE1", ["a", "MSG_A_S", "MSG_A_O", "MSG_A_C", "ACTIVE",
                               "READ_SOUND", ["n1"], ["snd_a"]],
                "OBJECTIVE2", ["b", "MSG_B_S", "MSG_B_O", "MSG_B_C",
                               "READ_SOUND", ["snd_b"]]]]"#,
            STARTANIMS,
        );

        let mission = parse(&level).expect("parses");

        assert_eq!(mission.objectives.len(), 2);
        assert_eq!(mission.read_time, 8.0);
        assert_eq!(mission.review_delay, 4.0);
        assert_eq!(mission.review_sound, "snd_review");
        assert_eq!(mission.objective_sound, "snd_objective");
        assert!(!mission.final_mission);
        assert_eq!(mission.new_game_anim, "start_single_player");
        assert_eq!(mission.load_game_anim, "load_control");
        assert!(mission.objectives[1].is_exit());
    }

    /// m2–m6 do not use `start_single_player`; defaulting to it would leave
    /// five of the six missions with nothing to run.
    #[test]
    fn the_new_game_anim_comes_from_the_file_not_a_default() {
        let level = level(
            r#"[["OBJECTIVE1", ["a", "s", "o", "c", "ACTIVE", "READ_SOUND", ["n"], ["x"]]]]"#,
            r#"[["NEW_GAME_START", ["m4_start_animation"],
                 "LOAD_GAME_START", ["load_control"]]]"#,
        );

        let mission = parse(&level).expect("parses");

        assert_eq!(mission.new_game_anim, "m4_start_animation");
    }

    /// m6 is the last mission and says so.
    #[test]
    fn final_mission_is_read() {
        let level = level(
            r#"[["FINAL_MISSION", [1],
                "OBJECTIVE1", ["a", "s", "o", "c", "ACTIVE", "READ_SOUND", ["n"], ["x"]]]]"#,
            STARTANIMS,
        );

        assert!(parse(&level).expect("parses").final_mission);
    }

    /// m13 ships no `objectives.zrd` and the multiplayer maps ship a
    /// placeholder; both must be a clean error, not a panic.
    #[test]
    fn a_level_without_mission_data_is_an_error_not_a_panic() {
        assert_eq!(
            parse(&ReaderData::default()).unwrap_err(),
            MissionError::NoObjectives
        );

        let no_start = ReaderData::from_entries(
            [(OBJECTIVES_ENTRY.to_string(), value("[[]]"))]
                .into_iter()
                .collect(),
        );
        assert_eq!(parse(&no_start).unwrap_err(), MissionError::NoStartAnims);
    }

    #[test]
    fn movers_are_a_flat_name_list_and_optional() {
        let mut entries = std::collections::BTreeMap::new();
        entries.insert(
            OBJECTIVES_ENTRY.to_string(),
            value(
                r#"[["OBJECTIVE1", ["a", "s", "o", "c", "ACTIVE", "READ_SOUND", ["n"], ["x"]]]]"#,
            ),
        );
        entries.insert(STARTANIMS_ENTRY.to_string(), value(STARTANIMS));
        entries.insert(MOVERS_ENTRY.to_string(), value(r#"[["vtol1", "trailr"]]"#));

        let mission = parse(&ReaderData::from_entries(entries)).expect("parses");

        assert_eq!(mission.movers, vec!["vtol1", "trailr"]);
    }
}
