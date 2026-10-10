//! The save file's types: the format `docs/SAVEGAME.md` specifies.
//!
//! These are the file, not the engine. They mirror the few engine enums the
//! file stores ([`Form`], [`AiMode`], [`Objective`], …) instead of
//! serialising the engine's own, so a rename in the sim cannot change what a
//! save means; the conversions sit next to each mirror. Vectors are arrays
//! and transforms are [`Xform`]s for the same reason — glam's and Bevy's
//! serde shapes are theirs to change.
//!
//! # Compatibility rules (SAVEGAME §4)
//!
//! - Nothing here denies unknown fields: a newer minor's additions are
//!   ignored by an older reader.
//! - Every field added after 1.0 is `#[serde(default)]`.
//! - `None` and empty lists are skipped when writing and default when read,
//!   which is both what keeps the file small and what lets a field be added
//!   without a migration.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::ai::AiState;
use crate::mission::{ObjectiveState, Outcome};
use crate::vehicle::VehicleForm;
use crate::vehicle::tuning::Difficulty;

/// What the `format` field of every save says.
pub const FORMAT_TAG: &str = "open-bft-save";

/// The version this build writes.
pub const CURRENT_VERSION: SaveVersion = SaveVersion { major: 1, minor: 1 };

/// A format version: a major bump breaks readers, a minor one does not.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SaveVersion {
    pub major: u16,
    pub minor: u16,
}

impl std::fmt::Display for SaveVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

// ---------------------------------------------------------------------------
// The file
// ---------------------------------------------------------------------------

/// A whole save.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SaveFile {
    /// Always [`FORMAT_TAG`].
    pub format: String,
    pub version: SaveVersion,
    pub header: SaveHeader,
    #[serde(default)]
    pub campaign: CampaignRecord,
    #[serde(default)]
    pub world: WorldRecord,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<NodeRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub craters: Vec<CraterNodeRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player: Option<PlayerRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub enemies: Vec<EnemyRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub turrets: Vec<TurretRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pickups: Vec<PickupRecord>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projectiles: Vec<ProjectileRecord>,
    #[serde(default)]
    pub anims: AnimsRecord,
}

/// The part of a save a reader looks at before deciding to read the rest.
#[derive(Debug, Clone, PartialEq)]
pub struct SaveProbe {
    pub format: String,
    pub version: SaveVersion,
    /// `None` when the header does not parse — a newer major's may not —
    /// and the list should still show the file.
    pub header: Option<SaveHeader>,
}

/// The two fields every version keeps. Parsing a whole file into this skips
/// every other field, so it reads a save of any version.
#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct SaveEnvelope {
    pub format: String,
    pub version: SaveVersion,
}

/// The header alone, parsed separately so a header that does not fit does
/// not hide the envelope.
#[derive(Deserialize, Debug, Clone, PartialEq)]
pub struct HeaderOnly {
    pub header: SaveHeader,
}

/// What the load list shows.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct SaveHeader {
    /// `"open-bft 0.1.0"`.
    pub written_by: String,
    /// Unix seconds, UTC.
    pub created: u64,
    /// The name the list shows.
    pub label: String,
    /// The mission number.
    pub level: u16,
    /// The catalog's directory name, `"m1"`.
    pub level_name: String,
    /// `MSG_CAMPAIGN1_NAME`.
    pub title_key: String,
    pub difficulty: DifficultyRecord,
    /// Seconds of mission time.
    pub play_time: f32,
    pub fingerprint: Fingerprint,
}

/// What game data a save was made against.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
#[serde(default)]
pub struct Fingerprint {
    pub nodes: u32,
    /// FNV-1a 64 over every `(node id, name)`, in id order.
    pub node_hash: u64,
    pub anims: u32,
    /// FNV-1a 64 over every definition's `anim_name`, in order.
    pub anim_hash: u64,
}

/// The campaign as it stood.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct CampaignRecord {
    pub completed: Vec<u16>,
    pub unlocked: Vec<Form>,
    pub mounted: Vec<String>,
}

/// Mission-wide state: clocks, randomness, objectives, the scripted look.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct WorldRecord {
    /// The anim VM's clock.
    pub clock: f32,
    pub ai_tick: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rng: Option<RngRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mission: Option<MissionRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fog: Option<FogRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub screen_tint: Option<[f32; 4]>,
}

/// The sim's ChaCha8 stream, exactly.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct RngRecord {
    /// 64 hex digits.
    pub seed: String,
    pub stream: u64,
    /// A u128 in decimal: RON's integers stop at 64 bits without a feature.
    pub word_pos: String,
}

/// The objectives and how the mission is going.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct MissionRecord {
    pub objectives: Vec<Objective>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<MissionOutcome>,
    pub started: bool,
    pub dying: bool,
    /// The HUD's game clock, seconds (1.1). A 1.0 save has none, and the
    /// reader falls back to the anim VM's clock, which starts with the level
    /// too.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed: Option<f32>,
    /// The HUD's score (1.1).
    #[serde(skip_serializing_if = "is_zero")]
    pub score: u32,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// The fog, as the scripts left it.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct FogRecord {
    /// Linear RGBA.
    pub color: [f32; 4],
    pub range: [f32; 2],
    pub altitude: [f32; 2],
}

/// The scripted camera zoom and clip planes.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct CameraRecord {
    pub zoom: [f32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_near: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip_far: Option<f32>,
}

// ---------------------------------------------------------------------------
// References and small values
// ---------------------------------------------------------------------------

/// An entity, by something stable across runs (SAVEGAME §5).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Hash)]
pub enum SaveRef {
    /// A level node, by gamez node id.
    Node(u32),
    /// The player's hull.
    Player,
    /// An enemy's hull, by `aiv.zrd` instance name.
    Enemy(String),
    /// Node `index` of an enemy's model, breadth-first from its root.
    EnemyPart(String, u32),
    /// Definition `index`'s own instance.
    Anim(u32),
    /// Entry `index` of `anims.copies`.
    Copy(u32),
    /// Node `index` of a copy's cloned subtree.
    CopyPart(u32, u32),
    /// Entry `index` of `pickups`.
    Pickup(u32),
    /// Entry `index` of `projectiles`.
    Projectile(u32),
}

/// A local transform.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct Xform {
    /// Translation, metres.
    pub t: [f32; 3],
    /// Rotation quaternion, `x, y, z, w`.
    pub r: [f32; 4],
    /// Scale.
    pub s: [f32; 3],
}

impl From<Transform> for Xform {
    fn from(transform: Transform) -> Xform {
        Xform {
            t: transform.translation.to_array(),
            r: transform.rotation.to_array(),
            s: transform.scale.to_array(),
        }
    }
}

impl From<Xform> for Transform {
    fn from(xform: Xform) -> Transform {
        let rotation = Quat::from_array(xform.r);
        // A hand-edited or corrupted quaternion is normalised rather than
        // allowed to shear the node. One that is already unit is kept
        // bit for bit: normalising it again moves the last digit, and a
        // restore should put back exactly what was saved.
        let rotation = if (rotation.length_squared() - 1.0).abs() > 1e-4 {
            rotation.normalize()
        } else {
            rotation
        };
        Transform {
            translation: Vec3::from_array(xform.t),
            rotation,
            scale: Vec3::from_array(xform.s),
        }
    }
}

/// A difficulty, as the file spells it.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DifficultyRecord {
    Easy,
    #[default]
    Normal,
    Hard,
}

impl From<Difficulty> for DifficultyRecord {
    fn from(difficulty: Difficulty) -> DifficultyRecord {
        match difficulty {
            Difficulty::Easy => DifficultyRecord::Easy,
            Difficulty::Normal => DifficultyRecord::Normal,
            Difficulty::Hard => DifficultyRecord::Hard,
        }
    }
}

impl From<DifficultyRecord> for Difficulty {
    fn from(difficulty: DifficultyRecord) -> Difficulty {
        match difficulty {
            DifficultyRecord::Easy => Difficulty::Easy,
            DifficultyRecord::Normal => Difficulty::Normal,
            DifficultyRecord::Hard => Difficulty::Hard,
        }
    }
}

/// A vehicle form.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Form {
    Track,
    Hover,
    Amphib,
    Sub,
}

impl From<VehicleForm> for Form {
    fn from(form: VehicleForm) -> Form {
        match form {
            VehicleForm::Track => Form::Track,
            VehicleForm::Hover => Form::Hover,
            VehicleForm::Amphib => Form::Amphib,
            VehicleForm::Sub => Form::Sub,
        }
    }
}

impl From<Form> for VehicleForm {
    fn from(form: Form) -> VehicleForm {
        match form {
            Form::Track => VehicleForm::Track,
            Form::Hover => VehicleForm::Hover,
            Form::Amphib => VehicleForm::Amphib,
            Form::Sub => VehicleForm::Sub,
        }
    }
}

/// One objective's state.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Objective {
    Inactive,
    Active,
    Complete,
}

impl From<ObjectiveState> for Objective {
    fn from(state: ObjectiveState) -> Objective {
        match state {
            ObjectiveState::Inactive => Objective::Inactive,
            ObjectiveState::Active => Objective::Active,
            ObjectiveState::Complete => Objective::Complete,
        }
    }
}

impl From<Objective> for ObjectiveState {
    fn from(state: Objective) -> ObjectiveState {
        match state {
            Objective::Inactive => ObjectiveState::Inactive,
            Objective::Active => ObjectiveState::Active,
            Objective::Complete => ObjectiveState::Complete,
        }
    }
}

/// How a mission ended.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissionOutcome {
    Won,
    Lost,
}

impl From<Outcome> for MissionOutcome {
    fn from(outcome: Outcome) -> MissionOutcome {
        match outcome {
            Outcome::Won => MissionOutcome::Won,
            Outcome::Lost => MissionOutcome::Lost,
        }
    }
}

impl From<MissionOutcome> for Outcome {
    fn from(outcome: MissionOutcome) -> Outcome {
        match outcome {
            MissionOutcome::Won => Outcome::Won,
            MissionOutcome::Lost => Outcome::Lost,
        }
    }
}

/// What an enemy brain is doing.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiMode {
    Dormant,
    Patrol,
    Engage,
    Return,
    Hide,
    Dead,
}

impl From<AiState> for AiMode {
    fn from(state: AiState) -> AiMode {
        match state {
            // Roaming is re-derived from the player's distance on the first
            // perception pass after a load, so it saves as asleep and the
            // format needs no new variant.
            AiState::Dormant | AiState::Roam => AiMode::Dormant,
            AiState::Patrol => AiMode::Patrol,
            AiState::Engage => AiMode::Engage,
            AiState::Return => AiMode::Return,
            AiState::Hide => AiMode::Hide,
            AiState::Dead => AiMode::Dead,
        }
    }
}

impl From<AiMode> for AiState {
    fn from(mode: AiMode) -> AiState {
        match mode {
            AiMode::Dormant => AiState::Dormant,
            AiMode::Patrol => AiState::Patrol,
            AiMode::Engage => AiState::Engage,
            AiMode::Return => AiState::Return,
            AiMode::Hide => AiState::Hide,
            AiMode::Dead => AiState::Dead,
        }
    }
}

/// The surface a mine lies on.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRecord {
    Default,
    Water,
    Quicksand,
}

impl From<crate::weapons::Surface> for SurfaceRecord {
    fn from(surface: crate::weapons::Surface) -> SurfaceRecord {
        match surface {
            crate::weapons::Surface::Default => SurfaceRecord::Default,
            crate::weapons::Surface::Water => SurfaceRecord::Water,
            crate::weapons::Surface::Quicksand => SurfaceRecord::Quicksand,
        }
    }
}

impl From<SurfaceRecord> for crate::weapons::Surface {
    fn from(surface: SurfaceRecord) -> crate::weapons::Surface {
        match surface {
            SurfaceRecord::Default => crate::weapons::Surface::Default,
            SurfaceRecord::Water => crate::weapons::Surface::Water,
            SurfaceRecord::Quicksand => crate::weapons::Surface::Quicksand,
        }
    }
}

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

/// A node whose state differs from the freshly loaded level.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct NodeRecord {
    /// A [`SaveRef::Node`] or a [`SaveRef::EnemyPart`].
    pub node: SaveRef,
    /// The node's name, on part records, so a model that changed shape is
    /// noticed rather than written into the wrong node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t: Option<Xform>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<Parent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rest: Option<Xform>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placed: Option<bool>,
}

impl NodeRecord {
    /// A record that changes nothing yet.
    pub fn new(node: SaveRef) -> NodeRecord {
        NodeRecord {
            node,
            name: None,
            t: None,
            active: None,
            parent: None,
            opacity: None,
            rest: None,
            placed: None,
        }
    }

    /// Whether it records any change at all.
    pub fn is_empty(&self) -> bool {
        self.t.is_none()
            && self.active.is_none()
            && self.parent.is_none()
            && self.opacity.is_none()
            && self.rest.is_none()
            && self.placed.is_none()
    }
}

/// What a node hangs under, when a script moved it.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum Parent {
    LevelRoot,
    Of(SaveRef),
}

/// A ground node's craters, in the order they were cut.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CraterNodeRecord {
    pub node: u32,
    pub craters: Vec<CraterRecord>,
}

/// One crater.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CraterRecord {
    pub center: [f32; 3],
    pub radius: f32,
    pub floor_radius: f32,
    pub depth: f32,
    pub sides: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub texture: Option<String>,
    pub uv_scale: [f32; 2],
}

// ---------------------------------------------------------------------------
// Vehicles and turrets
// ---------------------------------------------------------------------------

/// Anything with a hull: the player and the enemy vehicles.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct VehicleRecord {
    pub t: Xform,
    #[serde(default)]
    pub velocity: [f32; 3],
    /// Angular velocity, rad/s.
    #[serde(default)]
    pub spin: [f32; 3],
    pub form: Form,
    #[serde(default)]
    pub unlocked: Vec<Form>,
    /// Yaw and pitch, radians.
    #[serde(default)]
    pub turret: [f32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<HealthRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burning: Option<BurningRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loadout: Option<LoadoutRecord>,
}

/// Hit points.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct HealthRecord {
    pub current: f32,
    pub max: f32,
}

/// Burning: seconds left and damage per second.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct BurningRecord {
    pub remaining: f32,
    pub dps: f32,
}

/// What a vehicle or turret carries.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LoadoutRecord {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub slots: Vec<SlotRecord>,
    pub selected: u8,
    #[serde(default)]
    pub strong: bool,
    #[serde(default)]
    pub cooldown: f32,
    #[serde(default)]
    pub secondary_strong: bool,
    #[serde(default)]
    pub secondary_cooldown: f32,
}

/// One weapon slot, 1–9.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SlotRecord {
    pub slot: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weak: Option<AmmoRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub strong: Option<AmmoRecord>,
}

/// One weapon's rounds.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AmmoRecord {
    /// The `weapons.zrd` id, `"wep_2.0"`.
    pub weapon: String,
    /// `u32::MAX` is unlimited.
    pub ammo: u32,
    pub mounted: bool,
}

/// The player.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PlayerRecord {
    pub vehicle: VehicleRecord,
    /// The side cannon that fires next.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twin_next: Option<u8>,
}

/// An enemy vehicle.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct EnemyRecord {
    pub instance: String,
    pub kind: String,
    pub vehicle: VehicleRecord,
    pub brain: BrainRecord,
}

/// An enemy's decisions in progress.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct BrainRecord {
    pub mode: AiMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<SaveRef>,
    #[serde(default)]
    pub timer: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waypoint: Option<u32>,
    #[serde(default)]
    pub forward: bool,
    #[serde(default)]
    pub los_lost: u32,
    #[serde(default)]
    pub since_seen: f32,
}

/// A gun emplacement, by its node.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct TurretRecord {
    pub node: u32,
    #[serde(default)]
    pub next_muzzle: u32,
    #[serde(default)]
    pub fire_timer: f32,
    #[serde(default)]
    pub burst_left: f32,
    #[serde(default)]
    pub dwell_left: f32,
    /// Yaw and pitch, radians.
    #[serde(default)]
    pub aim: [f32; 2],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<SaveRef>,
    #[serde(default)]
    pub deactivated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<HealthRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub burning: Option<BurningRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loadout: Option<LoadoutRecord>,
}

// ---------------------------------------------------------------------------
// Pickups and projectiles
// ---------------------------------------------------------------------------

/// A pickup standing in the level.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PickupRecord {
    pub kind: String,
    pub amount: u32,
    pub instance: String,
    pub at: [f32; 3],
    /// Seconds a deathmatch pickup takes to come back; absent in a campaign.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub respawn: Option<f32>,
    /// Taken, and coming back in this many seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub respawn_in: Option<f32>,
}

/// Something fired and not yet spent.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ProjectileRecord {
    /// The `weapons.zrd` id.
    pub weapon: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<SaveRef>,
    pub team: u8,
    pub t: Xform,
    pub velocity: [f32; 3],
    #[serde(default)]
    pub age: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<SaveRef>,
    #[serde(default)]
    pub armed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landed: Option<LandingRecord>,
}

/// Where a mine came to rest.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct LandingRecord {
    pub normal: [f32; 3],
    pub surface: SurfaceRecord,
}

// ---------------------------------------------------------------------------
// The anim VM
// ---------------------------------------------------------------------------

/// The level-scripting VM's state.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AnimsRecord {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub instances: Vec<AnimRecord>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub copies: Vec<CopyRecord>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub motions: Vec<MotionRecord>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub lights: Vec<LightRecord>,
}

/// One definition instance's run state.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct AnimRecord {
    /// Index into the level's definitions.
    pub def: u32,
    /// The definition's `anim_name`, checked against `def` on load.
    pub name: String,
    pub armed: bool,
    #[serde(default)]
    pub invalidated: bool,
    #[serde(default)]
    pub started: bool,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub damage: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reset_at: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub running: Vec<CursorRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input: Option<SaveRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_ends: Option<[EndRecord; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub played_on: Option<SaveRef>,
    /// The names an operand or copy run resolves, and what to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<Vec<ScopeEntry>>,
    #[serde(default)]
    pub skip: bool,
    #[serde(default)]
    pub ran_out: bool,
    #[serde(default)]
    pub pending_reset: bool,
}

/// One running sequence.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CursorRecord {
    pub seq: u32,
    pub event: u32,
    pub t_anim: f32,
    pub t_seq: f32,
    pub t_event: f32,
    #[serde(default)]
    pub started: bool,
    #[serde(default)]
    pub t_running: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loops_left: Option<i32>,
    /// The open `If` blocks: `(taking, satisfied)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conds: Vec<(bool, bool)>,
}

/// One end of a connector.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct EndRecord {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<SaveRef>,
    pub offset: [f32; 3],
    #[serde(default)]
    pub frozen: bool,
}

/// A name a run resolves, and to what.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ScopeEntry {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<SaveRef>,
}

/// A copy of a prototype's definition, playing somewhere.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CopyRecord {
    pub instance: AnimRecord,
    pub subtree: CopySubtree,
}

/// What a copy plays on.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum CopySubtree {
    /// An existing subtree: a dead enemy's model, a destroyed turret.
    On(SaveRef),
    /// A clone the VM made, breadth-first, root first.
    Owned(Vec<CloneNode>),
}

/// One node of a copy's clone.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct CloneNode {
    /// The parent's index in the same list; `None` hangs it off the level
    /// root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lod: Option<LodRecord>,
    pub t: Xform,
    pub active: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opacity: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rest: Option<Xform>,
    #[serde(default)]
    pub placed: bool,
}

/// A LOD band.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct LodRecord {
    pub near: f32,
    pub far: f32,
    pub level: bool,
}

/// An `ObjectMotion` in flight.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct MotionRecord {
    pub node: SaveRef,
    /// The instance whose event started it.
    pub owner: SaveRef,
    /// `None` for a motion no sequence owns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u32>,
    #[serde(default)]
    pub event: u32,
    pub velocity: [f32; 3],
    pub acceleration: [f32; 3],
    pub gravity: f32,
    pub lands: bool,
    pub spin: [f32; 3],
    pub spin_rate: [f32; 3],
    pub tumble: f32,
    pub tumble_rate: f32,
    pub growth: [f32; 3],
    pub growth_rate: [f32; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_time: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounce_sequence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bounce_sound: Option<String>,
    pub age: f32,
    /// World position and velocity, once launched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub world: Option<([f32; 3], [f32; 3])>,
}

/// A definition's own light.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct LightRecord {
    pub owner: SaveRef,
    pub name: String,
    pub active: bool,
    /// Linear RGBA.
    pub color: [f32; 4],
    pub near: f32,
    pub far: f32,
    /// The node it rides on and the offset in that node's frame.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<(SaveRef, [f32; 3])>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub animation: Option<LightAnimationRecord>,
}

/// A `LightAnimation` in progress.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub struct LightAnimationRecord {
    pub range_rate: [f32; 2],
    pub color_rate: [f32; 3],
    pub remaining: f32,
}
