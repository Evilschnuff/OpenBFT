//! Campaign missions: objectives, win and lose, progression, persistence.
//!
//! A mission is a list of objectives read from the level's `objectives.zrd`,
//! an entry animation from its `startanims.zrd`, and two ways to stop. All of
//! it is data (docs/data/anims.md, READER-SCHEMA §3.6): nothing here hard-codes a
//! mission, and a level without the files simply has no [`MissionScript`].
//!
//! # How an objective completes
//!
//! Each non-exit objective names a gamez node whose **active state changes**
//! when the objective is achieved. The polarity differs — `frcoff` is
//! switched *on* by the force-field animation, `nb_force` switched *off* by
//! the beach-access one — so the rule that fits all 26 shipped conditions is
//! "complete when the node differs from what it was at mission start". Which
//! node is flipped by which animation is the anim VM's business; this module
//! only watches [`NodeActiveChanged`].
//!
//! The last objective of m1–m5 has no node at all. It is the mission's exit:
//! the player drives into the pickup VTOL's range, `mN_exit_trigger` fires
//! `mN_exit_animation`, and that animation's `Callback 99` ends the mission.
//! m6 ends the campaign instead, which is what its `FINAL_MISSION` says.
//!
//! # Losing
//!
//! The player's destruction plays the `bft_00` animation — `deathmN.zrd` is
//! the source file's name, not the compiled definition's — and the mission is
//! lost when it finishes.

use std::collections::VecDeque;

use bevy::prelude::*;
use bft_assets::catalog::{LevelCatalog, LevelId, LevelKind};
use bft_assets::messages::Messages;
use bft_assets::reader::{ReaderData, mission as reader_mission};

use crate::anim::{AnimCallback, CallAnim, MISSION_END_CALLBACK, NodeActiveChanged};
use crate::damage::{Destroyed, Health, Team};
use crate::pickups::PickedUp;
use crate::schedule::SimSet;
use crate::vehicle::tuning::Difficulty;
use crate::vehicle::{FormSet, PlayerVehicle};
use crate::weapons::{WeaponLoadout, WeaponTable};

/// A mission's identity: the level it is played on.
///
/// The campaign is one mission per level (m1–m6), so the level id is the
/// mission id and there is nothing to map between.
pub type MissionId = LevelId;

/// The campaign: the six missions, in order.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct Campaign {
    pub missions: Vec<MissionDef>,
}

impl Campaign {
    /// The mission played on a level.
    pub fn mission(&self, id: MissionId) -> Option<&MissionDef> {
        self.missions.iter().find(|mission| mission.id == id)
    }

    /// The mission after this one, or `None` at the end of the campaign.
    pub fn next_after(&self, id: MissionId) -> Option<&MissionDef> {
        let index = self.missions.iter().position(|m| m.id == id)?;
        self.missions.get(index + 1)
    }
}

/// One campaign mission, as the catalog and the message table describe it.
#[derive(Debug, Clone, PartialEq)]
pub struct MissionDef {
    pub id: MissionId,
    /// The level it is played on. The same value as [`Self::id`]; both are
    /// kept so a future campaign that reuses a level does not have to change
    /// every caller.
    pub level: LevelId,
    /// `MSG_CAMPAIGN<N>_NAME` — "Island Storm", "Beach Rally", …
    pub title_key: String,
    /// The briefing video, `video/m0N_01.avi`.
    pub briefing_video: Option<String>,
    /// Which `briefing.zrd` layout describes this mission's briefing screen.
    pub briefing_layout: u8,
}

/// Build the campaign from the catalog.
///
/// One mission per campaign level, in level order. The titles come from the
/// catalog's `title_key` and the message table resolves them; a level whose
/// key is missing keeps the key, which shows up on screen rather than as a
/// blank.
pub fn build_campaign(catalog: &LevelCatalog) -> Campaign {
    let mut missions: Vec<MissionDef> = catalog
        .levels
        .iter()
        .filter_map(|entry| {
            let LevelKind::Campaign { campaign } = entry.kind else {
                return None;
            };
            Some(MissionDef {
                id: entry.id,
                level: entry.id,
                title_key: entry.title_key.clone(),
                briefing_video: Some(format!("video/m{campaign:02}_01.avi")),
                briefing_layout: campaign,
            })
        })
        .collect();

    missions.sort_by_key(|mission| mission.id);
    Campaign { missions }
}

/// One level's mission script, built at load from its reader archive.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct MissionScript {
    pub objectives: Vec<ObjectiveDef>,
    /// How long the briefing stays up, in seconds.
    pub read_time: f32,
    /// Delay before the objective review plays.
    pub review_delay: f32,
    pub review_sound: String,
    pub objective_sound: String,
    /// The anim to call on a new game. **Not** `start_single_player` outside
    /// m1: the other five are `mN_start_animation`.
    pub new_game_anim: String,
    /// The anim to call on a loaded game. `load_control` in all six.
    pub load_game_anim: String,
    /// Nodes that move at runtime (`movers.zrd`).
    pub movers: Vec<String>,
    /// The player's death animation. `bft_00` in all six.
    pub death_anim: String,
    /// Whether this mission ends the campaign (m6's `FINAL_MISSION`).
    pub final_mission: bool,
}

/// The `anim_name` of the player's death animation in every campaign level.
pub const DEATH_ANIM: &str = "bft_00";

/// One objective.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectiveDef {
    /// 0-based, and also the zmap feature's `objective` id, which is how the
    /// automap knows which outline to highlight.
    pub index: u8,
    /// The briefing picture's stem in `image.zbd`.
    pub picture: String,
    pub summary_key: String,
    pub text_key: String,
    /// Empty on m3's objective 5, which ships without a `COMP` key.
    pub complete_key: String,
    /// Whether the record said `ACTIVE`.
    pub initially_active: bool,
    pub completion: Completion,
    /// `(logical sound name, delay in seconds)`.
    pub read_sound: Option<(String, f32)>,
    pub hint: Option<HintDef>,
}

/// An objective's hint: a nudge after a while, or on entering a zone.
#[derive(Debug, Clone, PartialEq)]
pub struct HintDef {
    pub sound: String,
    /// Seconds before the hint plays.
    pub time: f32,
    /// The `zone_id` the player must be in, where the record names one.
    pub zone: Option<i32>,
    /// Whether the record carried `HINT_ACTIVE`.
    pub active: bool,
}

/// What completes an objective.
#[derive(Debug, Clone, PartialEq)]
pub enum Completion {
    /// A named node's active state changed from what it was at mission start.
    ///
    /// `child` is scoped to `node`'s subtree — m4's `[fntrocket, healthy]`
    /// means the `healthy` under `fntrocket`, not one of the level's other
    /// 185. Every one of the 26 shipped conditions is this.
    NodeStateChanged { node: String, child: Option<String> },
    /// A named pickup instance was collected.
    ///
    /// READER-SCHEMA §3.6 cites `["pu03700"]` for m2's first objective, but
    /// the shipped file says `["got_amphib"]` — a real gamez node. No
    /// campaign mission uses this; it is kept because `PickedUp.instance`
    /// makes it free and because the schema says the form exists.
    PickupCollected { instance: String },
    /// The mission's exit trigger: no node, `Callback 99` from the exit
    /// animation. The last objective of m1–m5.
    ExitAnim,
}

/// Build a level's mission script from its reader archive.
///
/// `None` for a level with no `objectives.zrd` (m13) or with a placeholder
/// copy of m2's (the multiplayer maps), which the caller distinguishes by the
/// catalog's [`LevelKind`] rather than by the file.
pub fn build_mission_script(reader: &ReaderData) -> Option<MissionScript> {
    let raw = match reader_mission::parse(reader) {
        Ok(raw) => raw,
        Err(e) => {
            warn!("this level has no usable mission script: {e}");
            return None;
        }
    };

    let objectives = raw
        .objectives
        .iter()
        .enumerate()
        .map(|(index, objective)| ObjectiveDef {
            index: index as u8,
            picture: objective.picture.clone(),
            summary_key: objective.summary_key.clone(),
            text_key: objective.text_key.clone(),
            complete_key: objective.complete_key.clone(),
            // An exit objective carries no state word. It is active from the
            // start: the player may drive to the landing site whenever they
            // like, and the mission simply does not end until the others are
            // done — which the win rule, not the objective, enforces.
            initially_active: objective.initially_active || objective.is_exit(),
            completion: completion_of(objective),
            read_sound: objective
                .read_sound
                .clone()
                .map(|sound| (sound, objective.read_delay.unwrap_or(0.0))),
            hint: objective.hint_sound.clone().map(|sound| HintDef {
                sound,
                time: objective.hint_time.unwrap_or(0.0),
                zone: objective.hint_zone,
                active: objective.hint_active,
            }),
        })
        .collect();

    Some(MissionScript {
        objectives,
        read_time: raw.read_time,
        review_delay: raw.review_delay,
        review_sound: raw.review_sound,
        objective_sound: raw.objective_sound,
        new_game_anim: raw.new_game_anim,
        load_game_anim: raw.load_game_anim,
        movers: raw.movers,
        death_anim: DEATH_ANIM.to_string(),
        final_mission: raw.final_mission,
    })
}

/// The `pu<NNN><ii>` shape of a runtime pickup instance name.
///
/// A completion condition naming one means "complete when it is collected";
/// anything else is a gamez node whose active state flips.
fn is_pickup_instance(name: &str) -> bool {
    name.len() == 7 && name.starts_with("pu") && name[2..].chars().all(|c| c.is_ascii_digit())
}

fn completion_of(objective: &bft_assets::reader::mission::RawObjective) -> Completion {
    match objective.completion_nodes.as_slice() {
        [] => Completion::ExitAnim,
        [name] if is_pickup_instance(name) => Completion::PickupCollected {
            instance: name.clone(),
        },
        [node] => Completion::NodeStateChanged {
            node: node.clone(),
            child: None,
        },
        [node, child, ..] => Completion::NodeStateChanged {
            node: node.clone(),
            child: Some(child.clone()),
        },
    }
}

/// How the mission is going.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct MissionState {
    pub mission: MissionId,
    /// One per objective, in definition order.
    pub objectives: Vec<ObjectiveState>,
    /// `Some` once the mission has ended.
    pub outcome: Option<Outcome>,
    /// Lines for the HUD, oldest first. The HUD drains it.
    pub messages: VecDeque<MissionMessage>,
    /// Whether the mission's entry animation has been called.
    pub started: bool,
    /// Whether the death animation has been called.
    pub dying: bool,
    /// The mission was loaded from a save: its entry is `startanims.zrd`'s
    /// `load_game_anim` (`load_control`, which hands the player control),
    /// not the intro.
    pub resumed: bool,
}

impl MissionState {
    /// A fresh state for a script's objectives.
    pub fn new(mission: MissionId, script: &MissionScript) -> MissionState {
        MissionState {
            mission,
            objectives: script
                .objectives
                .iter()
                .map(|objective| {
                    if objective.initially_active {
                        ObjectiveState::Active
                    } else {
                        ObjectiveState::Inactive
                    }
                })
                .collect(),
            outcome: None,
            messages: VecDeque::new(),
            started: false,
            dying: false,
            resumed: false,
        }
    }

    /// Whether every objective that is not the exit has been completed.
    ///
    /// The exit objective never completes on its own — it is the *way* the
    /// mission ends, not a thing to do — so a mission whose other objectives
    /// are all done is ready for the player to leave.
    pub fn objectives_done(&self, script: &MissionScript) -> bool {
        script
            .objectives
            .iter()
            .zip(&self.objectives)
            .filter(|(objective, _)| objective.completion != Completion::ExitAnim)
            .all(|(_, state)| *state == ObjectiveState::Complete)
    }
}

/// One objective's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveState {
    /// Not shown to the player yet.
    ///
    /// Nothing activates these (READER-SCHEMA §5.3), not even the previous
    /// objective's completion: m2's objective 1 is `INACTIVE` with nothing
    /// before it, and m3's objective 3 sits `INACTIVE` between two `ACTIVE`
    /// ones, so briefing order is not dependency order and an in-order chain
    /// cannot reach either. What every one of the 16 `INACTIVE` objectives *does* have is a dedicated
    /// animation that flips its node — `check_get_amphib`, `obj_trigger1`,
    /// `check_get_hover` — so `INACTIVE` means "not worth showing until it
    /// happens", and the flip both reveals and completes the objective.
    Inactive,
    Active,
    Complete,
}

/// How a mission ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Won,
    Lost,
}

/// A line for the HUD.
#[derive(Debug, Clone, PartialEq)]
pub struct MissionMessage {
    /// The `MSG_*` key, so the HUD resolves it through [`Messages`].
    pub key: String,
    /// Which objective it is about, where it is about one.
    pub objective: Option<u8>,
}

/// Seconds since the mission started: the HUD's game clock.
///
/// Ticked with the sim, so it stops while [`Paused`](crate::states::Paused)
/// stops the sim. It also stops while a cinematic has the view — the
/// original's footage shows the clock holding still through them — and when
/// the mission ends. A resource of its
/// own rather than a field of [`MissionState`]: it changes every tick, and
/// the automap redraws whenever `MissionState` changes.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq)]
pub struct MissionClock(pub f32);

/// The player's points this mission: the HUD's score.
///
/// "You earn points for enemy kills" (the manual) is all the original says,
/// and no reader file gives a value per kill, so a kill scores the victim's
/// full health. // approximation: tougher enemies are worth more, which is
/// the one thing the manual's wording implies.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Score(pub u32);

/// An objective changed state. The automap marker follows this.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ObjectiveChanged {
    pub index: u8,
    pub state: ObjectiveState,
}

/// The mission ended.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct MissionEnded(pub Outcome);

/// What the player has unlocked and finished, saved from one run of the
/// game to the next.
#[derive(Resource, Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Progress {
    /// Missions completed, in the order they were won.
    pub completed: Vec<u16>,
    /// The vehicle forms unlocked by the chips collected so far.
    pub unlocked: FormSet,
    pub difficulty: Difficulty,
    /// The weapons mounted on the turret by the last mission won, by
    /// `weapons.zrd` id. Defaulted so a save from before it loads.
    #[serde(default)]
    pub mounted: Vec<String>,
    /// Made by `--unlock-all`: every weapon mounted and full, and never
    /// written to the save, so a test run cannot overwrite a campaign.
    #[serde(skip)]
    pub unlock_all: bool,
}

impl Default for Progress {
    fn default() -> Progress {
        Progress {
            completed: Vec::new(),
            // The player starts on tracks; amphib, hover and sub are chips
            // found in m2, m3 and m5.
            unlocked: FormSet::track_only(),
            difficulty: Difficulty::default(),
            mounted: Vec::new(),
            unlock_all: false,
        }
    }
}

impl Progress {
    /// Everything unlocked, for `--unlock-all` (FR-M7).
    pub fn unlocked_all() -> Progress {
        Progress {
            completed: (1..=6).collect(),
            unlocked: FormSet::all(),
            unlock_all: true,
            ..Progress::default()
        }
    }

    /// Whether a mission may be played.
    ///
    /// The first always may; a later one once the mission before it is done.
    pub fn is_unlocked(&self, mission: MissionId) -> bool {
        mission.0 <= 1 || self.completed.contains(&(mission.0 - 1))
    }

    /// Record a win. Repeats do not stack.
    pub fn complete(&mut self, mission: MissionId) {
        if !self.completed.contains(&mission.0) {
            self.completed.push(mission.0);
        }
    }

    /// Serialise to RON, as `save.ron` holds it.
    pub fn to_ron(&self) -> Result<String, ron::Error> {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())
    }

    /// Read RON back.
    pub fn from_ron(text: &str) -> Result<Progress, ron::de::SpannedError> {
        ron::from_str(text)
    }

    /// Where the save file lives: the platform's config directory.
    ///
    /// `None` when the platform has no home directory, which is a headless
    /// CI runner — there, progress is in memory and goes no further.
    pub fn save_path() -> Option<std::path::PathBuf> {
        directories::ProjectDirs::from("org", "open-bft", "open-bft")
            .map(|dirs| dirs.config_dir().join("save.ron"))
    }

    /// Write the save file, creating its directory.
    pub fn save(&self) -> Result<(), SaveError> {
        let path = Progress::save_path().ok_or(SaveError::NoConfigDir)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, self.to_ron()?)?;
        Ok(())
    }

    /// Read the save file. A missing one is [`Progress::default`], not an
    /// error: a first run has no save and must not fail on its absence.
    pub fn load() -> Result<Progress, SaveError> {
        let Some(path) = Progress::save_path() else {
            return Ok(Progress::default());
        };
        match std::fs::read_to_string(&path) {
            Ok(text) => Ok(Progress::from_ron(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Progress::default()),
            Err(e) => Err(e.into()),
        }
    }
}

/// What can go wrong saving or loading progress.
#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("cannot read or write the save file: {0}")]
    Io(#[from] std::io::Error),
    #[error("the save file is not valid RON: {0}")]
    Parse(#[from] ron::de::SpannedError),
    #[error("cannot serialise the save: {0}")]
    Serialise(#[from] ron::Error),
    #[error("this platform has no config directory to save into")]
    NoConfigDir,
}

/// The active states of the objectives' nodes at mission start.
///
/// An objective completes when its node *differs* from this, whichever way it
/// went — the polarity is per objective and the file does not state it
/// (READER-SCHEMA §3.6).
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct ObjectiveBaseline {
    /// One entry per objective: the node's entity and its state at start.
    /// `None` for an objective whose node this level does not have.
    pub states: Vec<Option<(Entity, bool)>>,
}

/// Mission logic: the state, the events, the systems.
pub struct MissionPlugin;

impl Plugin for MissionPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ObjectiveChanged>()
            .add_message::<MissionEnded>()
            .init_resource::<ObjectiveBaseline>()
            .init_resource::<MissionClock>()
            .init_resource::<Score>()
            .add_systems(
                FixedUpdate,
                (
                    start_mission,
                    complete_objectives_from_nodes,
                    complete_objectives_from_pickups,
                    end_on_callback,
                    lose_on_player_death,
                    advance_objectives,
                    tick_mission_clock,
                    score_kills,
                )
                    .chain()
                    .in_set(SimSet::Mission),
            );
    }
}

/// Call the mission's entry animation, once, on the first tick.
pub fn start_mission(
    script: Option<Res<MissionScript>>,
    mut state: Option<ResMut<MissionState>>,
    mut calls: MessageWriter<CallAnim>,
) {
    let (Some(script), Some(state)) = (script, state.as_mut()) else {
        return;
    };
    if state.started {
        return;
    }
    state.started = true;

    // What the original called on a loaded game: `load_control`, whose
    // callbacks hand the player control. The world it hands over is the
    // save's, already in place.
    let entry = if state.resumed {
        &script.load_game_anim
    } else {
        &script.new_game_anim
    };
    if entry.is_empty() {
        return;
    }
    info!("mission start: calling {entry}");
    calls.write(CallAnim::new(entry.clone()));
}

/// Complete the objectives whose nodes have changed from their baseline.
pub fn complete_objectives_from_nodes(
    mut changed: MessageReader<NodeActiveChanged>,
    script: Option<Res<MissionScript>>,
    baseline: Res<ObjectiveBaseline>,
    mut state: Option<ResMut<MissionState>>,
    mut objective_changed: MessageWriter<ObjectiveChanged>,
) {
    let (Some(script), Some(state)) = (script, state.as_mut()) else {
        return;
    };
    if state.outcome.is_some() {
        return;
    }

    for event in changed.read() {
        for (index, objective) in script.objectives.iter().enumerate() {
            if !matches!(objective.completion, Completion::NodeStateChanged { .. }) {
                continue;
            }
            let Some(Some((entity, was))) = baseline.states.get(index) else {
                continue;
            };
            // The node has to be *this* objective's, and it has to have moved
            // away from where it started — a definition that re-asserts the
            // starting state must not complete anything.
            if *entity != event.node || event.active == *was {
                continue;
            }

            complete(state, &script, index, &mut objective_changed);
        }
    }
}

/// Complete the objectives whose pickup instance was collected.
pub fn complete_objectives_from_pickups(
    mut collected: MessageReader<PickedUp>,
    script: Option<Res<MissionScript>>,
    mut state: Option<ResMut<MissionState>>,
    mut objective_changed: MessageWriter<ObjectiveChanged>,
) {
    let (Some(script), Some(state)) = (script, state.as_mut()) else {
        return;
    };
    if state.outcome.is_some() {
        return;
    }

    for event in collected.read() {
        for (index, objective) in script.objectives.iter().enumerate() {
            let Completion::PickupCollected { instance } = &objective.completion else {
                continue;
            };
            if *instance == event.instance {
                complete(state, &script, index, &mut objective_changed);
            }
        }
    }
}

/// Mark an objective complete, once.
fn complete(
    state: &mut MissionState,
    script: &MissionScript,
    index: usize,
    changed: &mut MessageWriter<ObjectiveChanged>,
) {
    let Some(slot) = state.objectives.get_mut(index) else {
        return;
    };
    if *slot == ObjectiveState::Complete {
        return;
    }
    // From `Inactive` as readily as from `Active`: an `INACTIVE` objective is
    // one the briefing does not mention until its animation fires, and that
    // same animation is what completes it.
    *slot = ObjectiveState::Complete;

    if let Some(objective) = script.objectives.get(index)
        && !objective.complete_key.is_empty()
    {
        state.messages.push_back(MissionMessage {
            key: objective.complete_key.clone(),
            objective: Some(index as u8),
        });
    }

    info!("objective {} complete", index + 1);
    changed.write(ObjectiveChanged {
        index: index as u8,
        state: ObjectiveState::Complete,
    });
}

/// Reveal the exit objective once everything else is done.
///
/// The only objective this engine activates rather than reads from the file.
/// Every other `INACTIVE` objective is revealed by its own animation flipping
/// its node, which completes it in the same breath (see [`ObjectiveState`]);
/// the exit has no node to flip, so something has to say when the player may
/// leave, and "when the rest are done" is what the missions mean by it.
pub fn advance_objectives(
    script: Option<Res<MissionScript>>,
    mut state: Option<ResMut<MissionState>>,
    mut changed: MessageWriter<ObjectiveChanged>,
) {
    let (Some(script), Some(state)) = (script, state.as_mut()) else {
        return;
    };
    if !state.objectives_done(&script) {
        return;
    }

    // Scanned from the state rather than driven by `ObjectiveChanged`: a
    // system that both read and wrote that message would take `Res` and
    // `ResMut` on one buffer, which Bevy rejects outright.
    let exits: Vec<usize> = script
        .objectives
        .iter()
        .enumerate()
        .filter(|(index, objective)| {
            objective.completion == Completion::ExitAnim
                && state.objectives.get(*index) == Some(&ObjectiveState::Inactive)
        })
        .map(|(index, _)| index)
        .collect();

    for index in exits {
        state.objectives[index] = ObjectiveState::Active;
        changed.write(ObjectiveChanged {
            index: index as u8,
            state: ObjectiveState::Active,
        });
    }
}

/// End the mission when the exit animation says so.
#[allow(clippy::too_many_arguments)]
pub fn end_on_callback(
    mut callbacks: MessageReader<AnimCallback>,
    script: Option<Res<MissionScript>>,
    mut state: Option<ResMut<MissionState>>,
    mut progress: Option<ResMut<Progress>>,
    mut ended: MessageWriter<MissionEnded>,
    mut objective_changed: MessageWriter<ObjectiveChanged>,
    players: Query<&WeaponLoadout, With<PlayerVehicle>>,
    weapons: Option<Res<WeaponTable>>,
) {
    let (Some(script), Some(state)) = (script, state.as_mut()) else {
        return;
    };

    for callback in callbacks.read() {
        match callback.value {
            MISSION_END_CALLBACK => {
                if state.outcome.is_some() {
                    continue;
                }

                // The exit objective completes with the mission rather than
                // before it: reaching the VTOL *is* achieving it.
                if let Some(index) = script
                    .objectives
                    .iter()
                    .position(|objective| objective.completion == Completion::ExitAnim)
                {
                    complete(state, &script, index, &mut objective_changed);
                }

                state.outcome = Some(Outcome::Won);
                info!("mission {} won", state.mission);
                ended.write(MissionEnded(Outcome::Won));

                if let Some(progress) = progress.as_mut().filter(|p| !p.unlock_all) {
                    progress.complete(state.mission);
                    // The weapons found carry on to the next mission.
                    if let (Ok(loadout), Some(weapons)) = (players.single(), weapons.as_ref()) {
                        progress.mounted = loadout
                            .mounted_weapons()
                            .filter_map(|index| weapons.by_index(index))
                            .map(|def| def.id.clone())
                            .collect();
                    }
                    if let Err(e) = progress.save() {
                        // A save that fails costs the next session's
                        // progress, not this mission's ending.
                        warn!("cannot save progress: {e}");
                    }
                }
            }
            // The cinematic protocol (2, 11, 14, 15/10/1, 17, 20) is
            // `crate::cinematic`'s; the anim VM applies it as it steps.
            value
                if crate::cinematic::callback_meaning(value)
                    == crate::cinematic::CallbackMeaning::Other =>
            {
                debug!("anim {} fired callback {value}", callback.def)
            }
            _ => {}
        }
    }
}

/// Lose the mission when the player is destroyed.
///
/// The death animation plays first — it is the camera-shake cinematic — and
/// the mission ends when it does. Calling it and ending in the same tick
/// would cut it off, so the state records that it is playing and the win/lose
/// screen waits for the animation to finish.
pub fn lose_on_player_death(
    mut destroyed: MessageReader<Destroyed>,
    players: Query<Entity, With<PlayerVehicle>>,
    script: Option<Res<MissionScript>>,
    mut state: Option<ResMut<MissionState>>,
    mut calls: MessageWriter<CallAnim>,
    mut ended: MessageWriter<MissionEnded>,
) {
    let (Some(script), Some(state)) = (script, state.as_mut()) else {
        return;
    };
    if state.outcome.is_some() || state.dying {
        return;
    }

    for event in destroyed.read() {
        if !players.contains(event.entity) {
            continue;
        }

        state.dying = true;
        state.outcome = Some(Outcome::Lost);
        info!("mission {} lost", state.mission);
        calls.write(CallAnim::new(script.death_anim.clone()));
        ended.write(MissionEnded(Outcome::Lost));
        break;
    }
}

/// Advance the game clock while a mission is running.
pub fn tick_mission_clock(
    time: Res<Time>,
    state: Option<Res<MissionState>>,
    control: Option<Res<crate::cinematic::PlayerControl>>,
    mut clock: ResMut<MissionClock>,
) {
    let cinematic = control.is_some_and(|control| control.is_cinematic());
    if !cinematic && state.is_some_and(|state| state.outcome.is_none()) {
        clock.0 += time.delta_secs();
    }
}

/// Score the player's kills of hostile things.
pub fn score_kills(
    mut destroyed: MessageReader<Destroyed>,
    players: Query<Option<&Team>, With<PlayerVehicle>>,
    victims: Query<(&Health, Option<&Team>)>,
    state: Option<Res<MissionState>>,
    mut score: ResMut<Score>,
) {
    let running = state.is_some_and(|state| state.outcome.is_none());
    for event in destroyed.read() {
        if !running {
            continue;
        }
        let Some(player_team) = event.by.and_then(|by| players.get(by).ok()) else {
            continue;
        };
        let player_team = player_team.copied().unwrap_or(Team::PLAYER);
        let Ok((health, team)) = victims.get(event.entity) else {
            continue;
        };
        if team.is_some_and(|team| team.is_hostile_to(player_team)) {
            score.0 = score.0.saturating_add(health.max.max(0.0).round() as u32);
        }
    }
}

/// The mission's title, resolved through the message table.
pub fn mission_title(mission: &MissionDef, messages: &Messages) -> String {
    messages.text(&mission.title_key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::reader::mission::RawObjective;

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
            death_anim: DEATH_ANIM.to_string(),
            final_mission: false,
        }
    }

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

    fn node(name: &str) -> Completion {
        Completion::NodeStateChanged {
            node: name.to_string(),
            child: None,
        }
    }

    /// A world with just the clock and score systems and a running mission.
    fn stats_app() -> App {
        let mut app = App::new();
        app.add_message::<Destroyed>()
            .init_resource::<MissionClock>()
            .init_resource::<Score>()
            .insert_resource(MissionState::new(
                LevelId(1),
                &script(vec![objective(0, true, node("a"))]),
            ))
            .add_systems(Update, (tick_mission_clock, score_kills));
        app
    }

    fn step(app: &mut App, seconds: f32) {
        app.world_mut()
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(seconds));
        app.update();
    }

    #[test]
    fn the_clock_runs_with_the_mission_and_stops_for_cinematics_and_the_end() {
        let mut app = stats_app();
        app.init_resource::<Time>();

        step(&mut app, 0.5);
        step(&mut app, 0.25);
        assert!((app.world().resource::<MissionClock>().0 - 0.75).abs() < 1e-6);

        // A cinematic holds it still.
        app.insert_resource(crate::cinematic::PlayerControl::Cinematic {
            anim: "intro".to_string(),
            carried: false,
        });
        step(&mut app, 1.0);
        assert!((app.world().resource::<MissionClock>().0 - 0.75).abs() < 1e-6);
        app.insert_resource(crate::cinematic::PlayerControl::Driving);

        app.world_mut().resource_mut::<MissionState>().outcome = Some(Outcome::Won);
        step(&mut app, 1.0);
        assert!((app.world().resource::<MissionClock>().0 - 0.75).abs() < 1e-6);
    }

    #[test]
    fn the_player_scores_hostile_kills_only() {
        let mut app = stats_app();
        app.init_resource::<Time>();
        let player = app
            .world_mut()
            .spawn((PlayerVehicle, Team::PLAYER, Health::new(100.0)))
            .id();
        let enemy = app.world_mut().spawn((Team::ENEMY, Health::new(60.0))).id();
        let friend = app
            .world_mut()
            .spawn((Team::PLAYER, Health::new(40.0)))
            .id();
        let other_enemy = app.world_mut().spawn((Team::ENEMY, Health::new(80.0))).id();

        let kill = |entity, by| Destroyed {
            entity,
            by,
            at: Vec3::ZERO,
            weapon: None,
        };
        app.world_mut().write_message(kill(enemy, Some(player)));
        app.world_mut().write_message(kill(friend, Some(player)));
        // An enemy killed by something else — another enemy's splash — is
        // not the player's.
        app.world_mut()
            .write_message(kill(other_enemy, Some(enemy)));
        step(&mut app, 0.1);

        assert_eq!(app.world().resource::<Score>().0, 60);
    }

    #[test]
    fn a_fresh_state_activates_what_the_file_said_and_nothing_else() {
        let script = script(vec![
            objective(0, true, node("a")),
            objective(1, false, node("b")),
            objective(2, false, Completion::ExitAnim),
        ]);

        let state = MissionState::new(LevelId(1), &script);

        assert_eq!(
            state.objectives,
            vec![
                ObjectiveState::Active,
                ObjectiveState::Inactive,
                // The exit objective has no state word, and
                // `build_mission_script` makes it active.
                ObjectiveState::Inactive,
            ]
        );
        assert!(state.outcome.is_none());
    }

    #[test]
    fn the_exit_objective_does_not_count_toward_being_done() {
        let script = script(vec![
            objective(0, true, node("a")),
            objective(1, true, Completion::ExitAnim),
        ]);
        let mut state = MissionState::new(LevelId(1), &script);

        assert!(!state.objectives_done(&script));

        state.objectives[0] = ObjectiveState::Complete;

        assert!(
            state.objectives_done(&script),
            "the exit is how you leave, not a thing to do"
        );
    }

    /// A single-node condition and an `[object, child]` pair both read; a
    /// record with no node is the exit.
    #[test]
    fn completion_conditions_come_from_the_record_shape() {
        let mut raw = RawObjective {
            completion_nodes: vec!["frcoff".to_string()],
            has_state: true,
            ..Default::default()
        };
        assert_eq!(completion_of(&raw), node("frcoff"));

        raw.completion_nodes = vec!["powstation".to_string(), "powerbad".to_string()];
        assert_eq!(
            completion_of(&raw),
            Completion::NodeStateChanged {
                node: "powstation".to_string(),
                child: Some("powerbad".to_string()),
            }
        );

        raw.completion_nodes = Vec::new();
        raw.has_state = false;
        assert_eq!(completion_of(&raw), Completion::ExitAnim);
    }

    /// READER-SCHEMA §3.6's `pu03700` form. No shipped mission uses it, but
    /// the shape has to be distinguishable from a node name.
    #[test]
    fn a_pickup_instance_is_told_from_a_node_name() {
        assert!(is_pickup_instance("pu03700"));
        assert!(!is_pickup_instance("got_amphib"));
        assert!(
            !is_pickup_instance("pu037"),
            "a template is not an instance"
        );
        assert!(!is_pickup_instance("powstation"));

        let raw = RawObjective {
            completion_nodes: vec!["pu03700".to_string()],
            has_state: true,
            ..Default::default()
        };
        assert_eq!(
            completion_of(&raw),
            Completion::PickupCollected {
                instance: "pu03700".to_string()
            }
        );
    }

    /// A save written before mounts were saved still loads, with none.
    #[test]
    fn a_save_without_mounts_loads() {
        let text = Progress::default().to_ron().expect("serialises");
        let old: String = text
            .lines()
            .filter(|line| !line.contains("mounted"))
            .collect::<Vec<_>>()
            .join("\n");
        assert_ne!(old, text, "the field was there to drop");

        let progress = Progress::from_ron(&old).expect("an old save still loads");

        assert!(progress.mounted.is_empty());
    }

    #[test]
    fn progress_round_trips_through_ron() {
        let mut progress = Progress {
            completed: vec![1, 2],
            unlocked: FormSet::of([crate::vehicle::VehicleForm::Amphib]),
            difficulty: Difficulty::Hard,
            mounted: vec!["wep_1.0".to_string(), "wep_2.0".to_string()],
            unlock_all: false,
        };
        progress.complete(LevelId(3));

        let text = progress.to_ron().expect("serialises");
        let read = Progress::from_ron(&text).expect("parses");

        assert_eq!(read, progress);
        assert_eq!(read.completed, vec![1, 2, 3]);
    }

    #[test]
    fn completing_a_mission_twice_does_not_record_it_twice() {
        let mut progress = Progress::default();

        progress.complete(LevelId(1));
        progress.complete(LevelId(1));

        assert_eq!(progress.completed, vec![1]);
    }

    #[test]
    fn missions_unlock_in_order_and_unlock_all_opens_everything() {
        let mut progress = Progress::default();

        assert!(progress.is_unlocked(LevelId(1)));
        assert!(!progress.is_unlocked(LevelId(2)));

        progress.complete(LevelId(1));
        assert!(progress.is_unlocked(LevelId(2)));
        assert!(!progress.is_unlocked(LevelId(3)));

        let all = Progress::unlocked_all();
        for mission in 1..=6 {
            assert!(all.is_unlocked(LevelId(mission)), "m{mission}");
        }
        assert_eq!(all.unlocked, FormSet::all());
    }

    /// The default save is the start of a campaign, not everything unlocked.
    #[test]
    fn a_missing_save_is_a_fresh_campaign() {
        let progress = Progress::default();

        assert!(progress.completed.is_empty());
        assert_eq!(progress.unlocked, FormSet::track_only());
    }
}
