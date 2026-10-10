//! `anim.zip` → [`AnimSet`]: the level-scripting VM's compiled definitions.
//!
//! In the GameZ engine "animation" means `anim.zbd`'s scripted sequences —
//! object motion, lights, fog, sounds, `CallAnimation`, `If`/`Else`, engine
//! callbacks — not skeletal animation. It is the level-scripting VM, and
//! mission logic runs on top of it (docs/data/anims.md).
//!
//! This module only *loads*. [`bft_sim::anim`] is the VM that steps the
//! result, and its handlers serve the presentation events this loader
//! carries but does not model.
//!
//! # What the archive holds
//!
//! One `<name>-<anim_name>[-<anim_root_name>].json` per definition (172–818
//! per level, 4798 in all), a `metadata.json` naming the mission and listing
//! every def, and 20-odd `si-script-NNN.zan.json` keyframe scripts that
//! `ObjectMotionSiScript` indexes into. The `.zan` files are **not** anim
//! defs and parsing them as one is the first trap.

use std::collections::HashMap;

use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::prelude::*;

use crate::mech3ax;

/// Every animation definition of one level, ready for the VM.
#[derive(Asset, TypePath, Debug, Clone, Default)]
pub struct AnimSet {
    /// The definitions, in archive order. The VM's `AnimInstance::def` is an
    /// index into this.
    pub defs: Vec<AnimDefDesc>,
    /// `anim_name` → every def that carries it.
    ///
    /// A `Vec` and not a single index because **`anim_name` is not unique**:
    /// 5–12 effect templates per level share one (`spark_blue.flt` ×3).
    /// `CallAnimation` starts every match, which is what a template wants.
    pub by_anim_name: HashMap<String, Vec<usize>>,
    /// The `.zan` keyframe scripts, indexed by `ObjectMotionSiScript::index`.
    pub si_scripts: Vec<SiScriptDesc>,
    /// The anim VM's own gravity, from `metadata.json`. −9.8 everywhere, and
    /// not the player's 40 m/s².
    pub gravity: f32,
    /// How many events of each kind the level has. `bft-game` warns at load
    /// about any kind the VM has no handler for.
    pub event_counts: HashMap<&'static str, usize>,
}

impl AnimSet {
    /// Every def carrying this `anim_name`, in archive order.
    pub fn lookup(&self, anim_name: &str) -> &[usize] {
        self.by_anim_name
            .get(anim_name)
            .map(Vec::as_slice)
            .unwrap_or_default()
    }

    /// The first def carrying this `anim_name`.
    ///
    /// For the names that come from reader data — `startanims.zrd`, the exit
    /// anims, `vehicle.zrd`'s transitions — which are unique in every level.
    pub fn find(&self, anim_name: &str) -> Option<&AnimDefDesc> {
        self.defs.get(*self.lookup(anim_name).first()?)
    }
}

/// One definition, as the VM wants it.
///
/// A flattened [`mech3ax::AnimDef`]: the resource lists become plain names,
/// `reset_state`'s wrapper object is unwrapped, and `reset_time`'s four
/// spellings become [`ResetTime`].
#[derive(Debug, Clone, Default)]
pub struct AnimDefDesc {
    /// The anim root object: the node this def hangs off.
    pub name: String,
    /// What `CallAnimation` and the reader refer to. Not unique.
    pub anim_name: String,
    /// The node under [`Self::name`] that scopes name lookups in this def.
    pub anim_root_name: String,
    pub activation: AnimActivation,
    pub execution: Execution,
    pub reset_time: ResetTime,
    /// Accumulated hit damage that arms a hit-activated def; 0 = any hit.
    pub health: f32,
    pub auto_reset_node_states: bool,
    pub proximity_damage: bool,
    pub network_log: bool,
    pub save_log: bool,
    pub has_callbacks: bool,
    /// Whether the def is enabled at all. True on all 4798 shipped defs.
    pub active: bool,
    pub objects: Vec<String>,
    pub nodes: Vec<String>,
    /// The `gamez` node each name in `objects` and `nodes` is bound to, as
    /// the original engine bound it at load: `(name, node id)`, from the
    /// entries' `ptr` (see [`node_id_from_ptr`]). Names whose pointer is not
    /// a `gamez` node — the sound nodes the engine creates at runtime — are
    /// left out.
    pub node_refs: Vec<(String, u32)>,
    pub lights: Vec<String>,
    pub static_sounds: Vec<String>,
    pub dynamic_sounds: Vec<String>,
    pub effects: Vec<String>,
    pub prereqs: Vec<PrereqDesc>,
    /// How many of the non-`required` prereqs must hold.
    pub prereq_min: u8,
    pub anim_refs: Vec<AnimRefDesc>,
    /// The initial node states, applied at load and on reset. **This is where
    /// a level's initial node visibility comes from** (docs/data/anims.md).
    pub reset_state: Vec<EventDesc>,
    pub sequences: Vec<SeqDesc>,
}

/// When a def arms.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum AnimActivation {
    /// Armed the moment the level starts.
    OnStartup,
    /// Only through `CallAnimation` or the engine.
    #[default]
    OnCall,
    /// Weapon damage on one of its nodes reaching `health`.
    WeaponHit,
    /// A collision doing so.
    CollideHit,
    /// Either.
    WeaponOrCollideHit,
}

impl AnimActivation {
    /// Whether accumulated weapon damage arms this def.
    pub fn takes_weapon_hits(&self) -> bool {
        matches!(
            self,
            AnimActivation::WeaponHit | AnimActivation::WeaponOrCollideHit
        )
    }

    /// Whether a collision arms it.
    pub fn takes_collisions(&self) -> bool {
        matches!(
            self,
            AnimActivation::CollideHit | AnimActivation::WeaponOrCollideHit
        )
    }
}

/// The condition polled on top of activation.
///
/// **`ByRange` is in squared metres** — every one of the game's 90 `max`
/// values is an exact square of a round number — so the VM compares it
/// against `Vec3::distance_squared` and never takes a root.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum Execution {
    /// Activation alone decides.
    #[default]
    None,
    /// Also fires while the player is within the band, squared metres.
    ByRange { min_sq: f32, max_sq: f32 },
    /// Fires while the player is in the root object's zone.
    ByZone,
}

/// What happens after a def finishes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum ResetTime {
    /// `-1` (3368 defs): no timed reset. **Not "one-shot" for a hit
    /// definition**: 1762 of the 1780 carry it, and a destructible that must
    /// break only once says so itself — 933 invalidate themselves, 574 more
    /// hide the node that takes their hits. m1's generator emitters
    /// (`left_shoot`, `right_shoot`) do neither, and fire a shockwave per
    /// hit until one is shot.
    Never,
    /// `0`: re-armed at once (1173).
    Immediate,
    /// `t > 0`: re-armed `t` seconds later (34, all 0.1–10 s).
    After(f32),
    /// `null` (220) or the three `-99` defs: not applicable.
    #[default]
    Unset,
}

/// A gate on arming.
#[derive(Debug, Clone, PartialEq)]
pub enum PrereqDesc {
    /// That animation must have started (docs/data/anims.md).
    Animation { name: String, required: bool },
    /// That node must be in the given active state.
    Object {
        name: String,
        active: bool,
        required: bool,
    },
    /// The root's parent must be.
    Parent {
        name: String,
        active: bool,
        required: bool,
    },
}

impl PrereqDesc {
    /// Whether this one must hold regardless of `prereq_min`.
    pub fn required(&self) -> bool {
        match self {
            PrereqDesc::Animation { required, .. }
            | PrereqDesc::Object { required, .. }
            | PrereqDesc::Parent { required, .. } => *required,
        }
    }
}

/// A sub-animation the def instantiates by name.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimRefDesc {
    /// `"CallObjectConnector"` or `"CallAnimation"`.
    pub kind: &'static str,
    pub name: String,
    /// The connector's local end, where there is one.
    pub local_name: Option<String>,
}

/// One sequence of a def.
#[derive(Debug, Clone, Default)]
pub struct SeqDesc {
    pub name: String,
    /// True when `seq_state` is `OnCall`: it runs only via `CallSequence`.
    pub on_call: bool,
    pub events: Vec<EventDesc>,
}

/// One event: when it starts and what it does.
#[derive(Debug, Clone, Default)]
pub struct EventDesc {
    pub start: EventStart,
    pub data: EventDataDesc,
}

/// When an event fires relative to something already running.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum EventStart {
    /// `start: null` — the same tick as the previous event. 41 171 of 53 167.
    #[default]
    WithPrevious,
    /// `time` seconds after `from`.
    Offset { from: OffsetFrom, time: f32 },
}

/// What an [`EventStart::Offset`] counts from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum OffsetFrom {
    /// The animation's activation.
    #[default]
    Animation,
    /// The previous event's start.
    Event,
    /// The sequence's start.
    Sequence,
}

/// What an event does.
///
/// The thirteen the VM runs itself are modelled; the other 24 are
/// presentation ([`EventDataDesc::Presentation`]) and reach the VM's
/// handlers by kind name with their JSON intact. An event kind this loader
/// does not know at all becomes [`EventDataDesc::Unknown`] and is counted —
/// none exists in the shipped data, but mech3ax's output is unstable.
#[derive(Debug, Clone, Default)]
pub enum EventDataDesc {
    /// Toggle a node's active flag. What objectives watch.
    ObjectActiveState {
        node: String,
        state: bool,
    },
    /// Start every def with this `anim_name`.
    CallAnimation {
        name: String,
        operand_node: Option<String>,
        wait_for_completion: bool,
        /// Where to play it (`AtNode`/`WithNode`); `None` on its own root.
        parameters: Option<CallAt>,
    },
    StopAnimation(String),
    /// Disarm a def permanently.
    InvalidateAnimation(String),
    /// Re-arm one.
    ResetAnimation(String),
    /// Start one of this def's own sequences.
    CallSequence(String),
    StopSequence(String),
    /// An engine hook; 99 ends the mission.
    Callback(i32),
    /// Re-run the sequence from its first event. −1 loops for ever.
    Loop(i32),
    If(Condition),
    Elseif(Condition),
    Else,
    #[default]
    Endif,
    /// The kind name, and mech3ax's JSON untouched.
    Presentation {
        kind: &'static str,
        value: serde_json::Value,
    },
    /// A kind this loader has never seen. Counted, then skipped.
    Unknown(String, serde_json::Value),
}

impl EventDataDesc {
    /// The kind name handlers are registered under.
    pub fn kind(&self) -> &str {
        match self {
            EventDataDesc::ObjectActiveState { .. } => "ObjectActiveState",
            EventDataDesc::CallAnimation { .. } => "CallAnimation",
            EventDataDesc::StopAnimation(_) => "StopAnimation",
            EventDataDesc::InvalidateAnimation(_) => "InvalidateAnimation",
            EventDataDesc::ResetAnimation(_) => "ResetAnimation",
            EventDataDesc::CallSequence(_) => "CallSequence",
            EventDataDesc::StopSequence(_) => "StopSequence",
            EventDataDesc::Callback(_) => "Callback",
            EventDataDesc::Loop(_) => "Loop",
            EventDataDesc::If(_) => "If",
            EventDataDesc::Elseif(_) => "Elseif",
            EventDataDesc::Else => "Else",
            EventDataDesc::Endif => "Endif",
            EventDataDesc::Presentation { kind, .. } => kind,
            EventDataDesc::Unknown(kind, _) => kind,
        }
    }
}

/// A `CallAnimation`'s `AtNode` or `WithNode` parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct CallAt {
    /// The node, named in the *calling* definition's scope. Often
    /// `INPUT_NODE`: the caller's own input, passed on.
    pub node: String,
    /// An offset in that node's frame.
    pub position: Vec3,
    /// `AtNode` moves the called definition's root object to the node;
    /// `WithNode` only hands the node over as its input.
    pub place: bool,
}

/// The four conditions `If`/`Elseif` test.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Condition {
    /// The LOD level. Presentation: the VM takes the branch.
    AnimationLod(u32),
    /// **Squared** metres from the player to the def's root.
    PlayerRange(f32),
    /// Probability in 0–1.
    RandomWeight(f32),
    /// Whether the node at this index is under cover.
    NodeUndercover(u32),
}

/// One `.zan` keyframe script, for `ObjectMotionSiScript`.
#[derive(Debug, Clone, Default)]
pub struct SiScriptDesc {
    pub script_name: String,
    pub object_name: String,
    /// The frames, as mech3ax writes them.
    pub frames: Vec<serde_json::Value>,
    /// The same frames with a shape: what the VM plays back.
    pub keys: Vec<SiFrame>,
}

impl SiScriptDesc {
    /// When the last frame ends, in seconds from the script's start.
    pub fn duration(&self) -> f32 {
        self.keys.iter().map(|key| key.end).fold(0.0, f32::max)
    }
}

/// One `.zan` frame: a time span and up to three channels.
///
/// A channel a frame leaves out holds whatever the frames before it set —
/// 99 of the 176 scripts carry `scale` only on their first frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SiFrame {
    pub start: f32,
    pub end: f32,
    /// Position at `start`, and its velocity in metres per second.
    ///
    /// **`delta` is a rate, not a span**: `base + delta * (end - start)` lands
    /// on the next frame's `base` to within a millimetre across all 176
    /// scripts.
    pub translate: Option<(Vec3, Vec3)>,
    /// Orientation at `start`.
    ///
    /// mech3ax writes the quaternion's **scalar in `x`** and the vector part
    /// in `y`/`z`/`w`: the mean |x| over every frame is 0.64 and the mean |w|
    /// 0.05, and read the other way m1's VTOL flies upside down.
    pub rotate: Option<Quat>,
    /// Scale at `start`, and its rate per second.
    pub scale: Option<(Vec3, Vec3)>,
}

/// What can go wrong reading an `anim.zip`.
#[derive(Debug, thiserror::Error)]
pub enum AnimError {
    #[error("cannot read the anim archive: {0}")]
    Io(#[from] std::io::Error),
    #[error("the anim archive is not a readable ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("{0} is not the JSON the anim loader expects: {1}")]
    Json(String, serde_json::Error),
    /// Every level's archive has one; without it there is no def order.
    #[error("no metadata.json in the anim archive")]
    NoMetadata,
}

/// Loads `cache://anim/<level>-anim.anim.zip`.
#[derive(TypePath, Debug, Default)]
pub struct AnimLoader;

impl AssetLoader for AnimLoader {
    type Asset = AnimSet;
    type Settings = ();
    type Error = AnimError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _context: &mut LoadContext<'_>,
    ) -> Result<AnimSet, AnimError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        parse_anim_set(&bytes)
    }

    fn extensions(&self) -> &[&str] {
        // The extraction names anim outputs `<mission>-anim.anim.zip`, which
        // keeps this loader off `.gamez.zip`, `.tex.zip` and `.rdr.zip`.
        &["anim.zip"]
    }
}

/// The `.zan.json` suffix that marks an SI script rather than a def.
const SI_SUFFIX: &str = ".zan.json";

/// Parse a whole extracted `anim.zip`.
///
/// Definitions are read in `metadata.json`'s `anim_def_names` order rather
/// than ZIP order, because that is the order the original engine assigned
/// indices in and `ObjectMotionSiScript` is not the only thing that could
/// come to depend on it.
///
/// A def that will not parse is logged and skipped rather than failing the
/// level: an unparsed prop animation costs an effect, while a hard error
/// costs the whole mission.
pub fn parse_anim_set(bytes: &[u8]) -> Result<AnimSet, AnimError> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;

    let metadata: mech3ax::AnimMetadata = {
        let entry = zip
            .by_name("metadata.json")
            .map_err(|_| AnimError::NoMetadata)?;
        serde_json::from_reader(entry)
            .map_err(|e| AnimError::Json("metadata.json".to_string(), e))?
    };

    let mut set = AnimSet {
        gravity: metadata.gravity,
        ..default()
    };

    for stem in &metadata.anim_def_names {
        let name = format!("{stem}.json");
        let raw: mech3ax::AnimDef = match zip.by_name(&name) {
            Ok(entry) => match serde_json::from_reader(entry) {
                Ok(def) => def,
                Err(e) => {
                    warn!("anim def {name} will not parse, skipping it: {e}");
                    continue;
                }
            },
            Err(e) => {
                warn!("anim def {name} is listed in metadata.json but missing: {e}");
                continue;
            }
        };

        let def = anim_def_desc(raw, &mut set.event_counts);
        set.by_anim_name
            .entry(def.anim_name.clone())
            .or_default()
            .push(set.defs.len());
        set.defs.push(def);
    }

    // The SI scripts, in `si-script-NNN` order so the number in the name is
    // the index `ObjectMotionSiScript` uses.
    let mut si_names: Vec<String> = zip
        .file_names()
        .filter(|name| name.ends_with(SI_SUFFIX))
        .map(str::to_string)
        .collect();
    si_names.sort();

    for name in si_names {
        let entry = zip.by_name(&name)?;
        match serde_json::from_reader::<_, mech3ax::SiScript>(entry) {
            Ok(script) => set.si_scripts.push(SiScriptDesc {
                keys: script.frames.iter().filter_map(si_frame).collect(),
                script_name: script.script_name,
                object_name: script.object_name,
                frames: script.frames,
            }),
            // Keep the index alignment: a dropped script would shift every
            // later one and move the wrong object.
            Err(e) => {
                warn!("si script {name} will not parse; keeping an empty slot: {e}");
                set.si_scripts.push(SiScriptDesc::default());
            }
        }
    }

    Ok(set)
}

/// Flatten one mech3ax definition into the shape the VM steps.
fn anim_def_desc(raw: mech3ax::AnimDef, counts: &mut HashMap<&'static str, usize>) -> AnimDefDesc {
    AnimDefDesc {
        name: raw.name,
        anim_name: raw.anim_name,
        anim_root_name: raw.anim_root_name,
        activation: match raw.activation {
            mech3ax::AnimActivation::OnStartup => AnimActivation::OnStartup,
            mech3ax::AnimActivation::OnCall => AnimActivation::OnCall,
            mech3ax::AnimActivation::WeaponHit => AnimActivation::WeaponHit,
            mech3ax::AnimActivation::CollideHit => AnimActivation::CollideHit,
            mech3ax::AnimActivation::WeaponOrCollideHit => AnimActivation::WeaponOrCollideHit,
        },
        execution: match raw.execution {
            mech3ax::AnimExecution::None => Execution::None,
            mech3ax::AnimExecution::ByRange(range) => Execution::ByRange {
                min_sq: range.min_sq,
                max_sq: range.max_sq,
            },
            mech3ax::AnimExecution::ByZone => Execution::ByZone,
        },
        reset_time: reset_time(raw.reset_time),
        health: raw.health,
        auto_reset_node_states: raw.auto_reset_node_states,
        proximity_damage: raw.proximity_damage,
        network_log: raw.network_log.unwrap_or(false),
        save_log: raw.save_log.unwrap_or(false),
        has_callbacks: raw.has_callbacks,
        active: raw.active,
        node_refs: node_refs(raw.objects.iter().chain(raw.nodes.iter()).flatten()),
        objects: names(raw.objects),
        nodes: names(raw.nodes),
        lights: names(raw.lights),
        static_sounds: names(raw.static_sounds),
        dynamic_sounds: names(raw.dynamic_sounds),
        effects: names(raw.effects),
        prereqs: raw
            .activ_prereqs
            .unwrap_or_default()
            .into_iter()
            .map(prereq_desc)
            .collect(),
        prereq_min: raw.activ_prereq_min_to_satisfy,
        anim_refs: raw
            .anim_refs
            .unwrap_or_default()
            .into_iter()
            .map(anim_ref_desc)
            .collect(),
        reset_state: raw
            .reset_state
            .map(|seq| events(seq.events, counts))
            .unwrap_or_default(),
        sequences: raw
            .sequences
            .unwrap_or_default()
            .into_iter()
            .map(|seq| SeqDesc {
                name: seq.name,
                on_call: seq.seq_state == Some(mech3ax::SeqState::OnCall),
                events: events(seq.events, counts),
            })
            .collect(),
    }
}

/// The four `reset_time` spellings (docs/data/anims.md).
///
/// `-99` — three defs in the whole game, meaning unknown — is folded into
/// `Unset` rather than given a variant nothing can act on.
fn reset_time(value: Option<f32>) -> ResetTime {
    match value {
        None => ResetTime::Unset,
        Some(-1.0) => ResetTime::Never,
        Some(0.0) => ResetTime::Immediate,
        Some(t) if t > 0.0 => ResetTime::After(t),
        Some(_) => ResetTime::Unset,
    }
}

/// The `ptr` of the `objects`/`nodes` entry bound to gamez node 0.
///
/// An anim def's `objects`/`nodes` entries carry a `ptr` that identifies the
/// node each name is bound to: this base plus [`NODE_PTR_STRIDE`] per node
/// id. Measured from the data, it is the same in all 13 levels and accounts
/// for every entry but the sound nodes created at runtime, whose values lie
/// past the last node (docs/data/anims.md).
pub const NODE_PTR_BASE: u32 = 39_845_900;

/// The step between the `ptr` values of consecutive gamez node ids.
pub const NODE_PTR_STRIDE: u32 = 196;

/// The `gamez` node id an `objects`/`nodes` pointer refers to, if it is on
/// the node grid. Whether that node exists and carries the name is the
/// caller's to check against the level.
pub fn node_id_from_ptr(ptr: u32) -> Option<u32> {
    let offset = ptr.checked_sub(NODE_PTR_BASE)?;
    (offset % NODE_PTR_STRIDE == 0).then_some(offset / NODE_PTR_STRIDE)
}

fn node_refs<'a>(list: impl Iterator<Item = &'a mech3ax::AnimNamed>) -> Vec<(String, u32)> {
    let mut refs: Vec<(String, u32)> = Vec::new();
    for named in list {
        if let Some(id) = node_id_from_ptr(named.ptr)
            && !refs.iter().any(|(name, _)| *name == named.name)
        {
            refs.push((named.name.clone(), id));
        }
    }
    refs
}

/// Pull the names out of one of a def's resource lists.
fn names(list: Option<Vec<mech3ax::AnimNamed>>) -> Vec<String> {
    list.unwrap_or_default()
        .into_iter()
        .map(|named| named.name)
        .collect()
}

fn prereq_desc(raw: mech3ax::ActivationPrereq) -> PrereqDesc {
    match raw {
        mech3ax::ActivationPrereq::Animation(a) => PrereqDesc::Animation {
            name: a.name,
            required: a.required,
        },
        mech3ax::ActivationPrereq::Object(o) => PrereqDesc::Object {
            name: o.name,
            active: o.active,
            required: o.required,
        },
        mech3ax::ActivationPrereq::Parent(o) => PrereqDesc::Parent {
            name: o.name,
            active: o.active,
            required: o.required,
        },
    }
}

fn anim_ref_desc(raw: mech3ax::AnimRef) -> AnimRefDesc {
    match raw {
        mech3ax::AnimRef::CallObjectConnector(c) => AnimRefDesc {
            kind: "CallObjectConnector",
            name: c.name,
            local_name: c.local_name,
        },
        mech3ax::AnimRef::CallAnimation(a) => AnimRefDesc {
            kind: "CallAnimation",
            name: a.name,
            local_name: None,
        },
    }
}

fn events(
    raw: Vec<mech3ax::AnimEvent>,
    counts: &mut HashMap<&'static str, usize>,
) -> Vec<EventDesc> {
    raw.into_iter()
        .map(|event| {
            let data = event_data_desc(event.data);
            // `kind()` borrows, and the counter key has to outlive it, so the
            // static name is taken from the same match rather than the value.
            if let Some(kind) = static_kind(&data) {
                *counts.entry(kind).or_default() += 1;
            }
            EventDesc {
                start: match event.start {
                    None => EventStart::WithPrevious,
                    Some(start) => EventStart::Offset {
                        from: match start.offset {
                            mech3ax::OffsetFrom::Animation => OffsetFrom::Animation,
                            mech3ax::OffsetFrom::Event => OffsetFrom::Event,
                            mech3ax::OffsetFrom::Sequence => OffsetFrom::Sequence,
                        },
                        time: start.time,
                    },
                },
                data,
            }
        })
        .collect()
}

/// The `'static` kind name, for the counter. `None` for an unknown kind,
/// whose name is owned by the event and cannot key a `&'static str` map.
fn static_kind(data: &EventDataDesc) -> Option<&'static str> {
    match data {
        EventDataDesc::Unknown(..) => None,
        EventDataDesc::Presentation { kind, .. } => Some(kind),
        // Every other variant's `kind()` is a literal, so this is the same
        // string with the lifetime the map needs.
        other => match other.kind() {
            "ObjectActiveState" => Some("ObjectActiveState"),
            "CallAnimation" => Some("CallAnimation"),
            "StopAnimation" => Some("StopAnimation"),
            "InvalidateAnimation" => Some("InvalidateAnimation"),
            "ResetAnimation" => Some("ResetAnimation"),
            "CallSequence" => Some("CallSequence"),
            "StopSequence" => Some("StopSequence"),
            "Callback" => Some("Callback"),
            "Loop" => Some("Loop"),
            "If" => Some("If"),
            "Elseif" => Some("Elseif"),
            "Else" => Some("Else"),
            "Endif" => Some("Endif"),
            _ => None,
        },
    }
}

fn event_data_desc(raw: mech3ax::EventData) -> EventDataDesc {
    use mech3ax::EventData as E;

    /// A presentation event: kept whole for its handler, named for the
    /// handler registry.
    macro_rules! presentation {
        ($kind:literal, $value:expr) => {
            EventDataDesc::Presentation {
                kind: $kind,
                value: $value,
            }
        };
    }

    match raw {
        E::ObjectActiveState(state) => EventDataDesc::ObjectActiveState {
            node: state.node,
            state: state.state,
        },
        E::CallAnimation(call) => EventDataDesc::CallAnimation {
            name: call.name,
            operand_node: call.operand_node,
            wait_for_completion: call.wait_for_completion.is_some(),
            parameters: call.parameters.map(|parameters| {
                let (at, place) = match parameters {
                    mech3ax::CallParameters::AtNode(at) => (at, true),
                    mech3ax::CallParameters::WithNode(at) => (at, false),
                };
                CallAt {
                    node: at.node,
                    position: at
                        .position
                        .map(|p| Vec3::new(p.x, p.y, p.z))
                        .unwrap_or(Vec3::ZERO),
                    place,
                }
            }),
        },
        E::StopAnimation(e) => EventDataDesc::StopAnimation(e.name),
        E::InvalidateAnimation(e) => EventDataDesc::InvalidateAnimation(e.name),
        E::ResetAnimation(e) => EventDataDesc::ResetAnimation(e.name),
        E::CallSequence(e) => EventDataDesc::CallSequence(e.name),
        E::StopSequence(e) => EventDataDesc::StopSequence(e.name),
        E::Callback(e) => EventDataDesc::Callback(e.value),
        E::Loop(e) => EventDataDesc::Loop(e.count),
        E::If(e) => EventDataDesc::If(condition(e.condition)),
        E::Elseif(e) => EventDataDesc::Elseif(condition(e.condition)),
        E::Else(_) => EventDataDesc::Else,
        E::Endif(_) => EventDataDesc::Endif,

        E::ObjectMotion(v) => presentation!("ObjectMotion", v),
        E::ObjectMotionFromTo(v) => presentation!("ObjectMotionFromTo", v),
        E::ObjectMotionSiScript(si) => presentation!(
            "ObjectMotionSiScript",
            serde_json::json!({ "name": si.name, "index": si.index })
        ),
        E::ObjectTranslateState(v) => presentation!("ObjectTranslateState", v),
        E::ObjectScaleState(v) => presentation!("ObjectScaleState", v),
        E::ObjectRotateState(v) => presentation!("ObjectRotateState", v),
        E::ObjectOpacityState(v) => presentation!("ObjectOpacityState", v),
        E::ObjectOpacityFromTo(v) => presentation!("ObjectOpacityFromTo", v),
        E::ObjectAddChild(v) => presentation!("ObjectAddChild", v),
        E::ObjectDeleteChild(v) => presentation!("ObjectDeleteChild", v),
        E::ObjectCycleTexture(v) => presentation!("ObjectCycleTexture", v),
        E::ObjectConnector(v) => presentation!("ObjectConnector", v),
        E::CallObjectConnector(v) => presentation!("CallObjectConnector", v),
        E::Sound(v) => presentation!("Sound", v),
        E::SoundNode(v) => presentation!("SoundNode", v),
        E::Effect(v) => presentation!("Effect", v),
        E::LightState(v) => presentation!("LightState", v),
        E::LightAnimation(v) => presentation!("LightAnimation", v),
        E::FogState(v) => presentation!("FogState", v),
        E::FbfxColorFromTo(v) => presentation!("FbfxColorFromTo", v),
        E::FbfxCsinwaveFromTo(v) => presentation!("FbfxCsinwaveFromTo", v),
        E::CameraState(v) => presentation!("CameraState", v),
        E::CameraFromTo(v) => presentation!("CameraFromTo", v),
        E::AnimVerbose(v) => presentation!("AnimVerbose", v),
    }
}

/// Give one `.zan` frame its shape. `None` for a frame with no time span.
pub fn si_frame(value: &serde_json::Value) -> Option<SiFrame> {
    let number = |v: &serde_json::Value, key: &str| v.get(key)?.as_f64().map(|f| f as f32);
    let vec3 = |v: &serde_json::Value| -> Option<Vec3> {
        Some(Vec3::new(number(v, "x")?, number(v, "y")?, number(v, "z")?))
    };
    let channel = |key: &str| -> Option<(Vec3, Vec3)> {
        let channel = value.get(key)?;
        Some((vec3(channel.get("base")?)?, vec3(channel.get("delta")?)?))
    };

    let rotate = value.get("rotate").and_then(|rotate| {
        let q = rotate.get("base")?;
        // Scalar first, in the field mech3ax calls `x` (see `SiFrame::rotate`).
        let quat = Quat::from_xyzw(
            number(q, "y")?,
            number(q, "z")?,
            number(q, "w")?,
            number(q, "x")?,
        );
        (quat.length_squared() > 1e-6).then(|| quat.normalize())
    });

    Some(SiFrame {
        start: number(value, "start_time")?,
        end: number(value, "end_time")?,
        translate: channel("translate"),
        rotate,
        scale: channel("scale"),
    })
}

fn condition(raw: mech3ax::AnimCondition) -> Condition {
    match raw {
        mech3ax::AnimCondition::AnimationLod(lod) => Condition::AnimationLod(lod),
        mech3ax::AnimCondition::PlayerRange(range_sq) => Condition::PlayerRange(range_sq),
        mech3ax::AnimCondition::RandomWeight(weight) => Condition::RandomWeight(weight),
        mech3ax::AnimCondition::NodeUndercover(node) => Condition::NodeUndercover(node.node_index),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A def with the fields the real files carry, values invented.
    const DEF: &str = r#"{
      "name": "vtol1",
      "anim_name": "m1_exit_trigger",
      "anim_root_name": "healthy",
      "has_callbacks": false,
      "auto_reset_node_states": true,
      "local_nodes_only": false,
      "proximity_damage": false,
      "active": true,
      "low_priority": false,
      "activation": "OnStartup",
      "execution": { "ByRange": { "min": 0.0, "max": 30.25 } },
      "network_log": null,
      "save_log": true,
      "reset_time": -1.0,
      "health": 5.0,
      "activ_prereq_min_to_satisfy": 1,
      "objects": [{ "name": "healthy", "ptr": 1, "flags": 0 }],
      "nodes": [{ "name": "destroyed", "ptr": 2 }],
      "lights": null,
      "puffers": null,
      "dynamic_sounds": null,
      "static_sounds": [{ "name": "snd_exp_hit1", "pad": "" }],
      "effects": null,
      "activ_prereqs": [
        { "Animation": { "name": "broken_win_01", "required": true } },
        { "Object": { "name": "rock1", "required": false, "active": true, "ptr": 3 } }
      ],
      "anim_refs": [
        { "CallObjectConnector": { "name": "boat_wake", "local_name": "right_wake" } }
      ],
      "reset_state": {
        "events": [
          { "start": null, "data": { "ObjectActiveState": { "node": "healthy", "state": true } } }
        ],
        "pointer": 9
      },
      "sequences": [
        {
          "name": "go",
          "seq_state": "Initial",
          "reset_state": "Initial",
          "events": [
            { "start": null, "data": { "CallSequence": { "name": "trail" } } },
            { "start": { "offset": "Event", "time": 1.5 },
              "data": { "Callback": { "value": 99 } } }
          ],
          "pointer": 10
        },
        {
          "name": "trail",
          "seq_state": "OnCall",
          "reset_state": "Initial",
          "events": [
            { "start": null, "data": { "Loop": { "Count": -1 } } }
          ],
          "pointer": 11
        }
      ],
      "ptrs": {}
    }"#;

    fn parse(json: &str) -> AnimDefDesc {
        let raw: mech3ax::AnimDef = serde_json::from_str(json).expect("the def parses");
        anim_def_desc(raw, &mut HashMap::new())
    }

    #[test]
    fn a_definition_flattens_into_the_shape_the_vm_steps() {
        let def = parse(DEF);

        assert_eq!(def.name, "vtol1");
        assert_eq!(def.anim_name, "m1_exit_trigger");
        assert_eq!(def.anim_root_name, "healthy");
        assert_eq!(def.activation, AnimActivation::OnStartup);
        assert_eq!(def.reset_time, ResetTime::Never);
        assert_eq!(def.health, 5.0);
        assert!(def.auto_reset_node_states);
        // `network_log: null` is false, not a missing field.
        assert!(!def.network_log);
        assert!(def.save_log);
    }

    /// The resource lists are arrays of records in the real data, and reading
    /// them as `Vec<String>` is the mistake that silently empties them.
    #[test]
    fn resource_lists_are_records_with_a_name_not_bare_strings() {
        let def = parse(DEF);

        assert_eq!(def.objects, vec!["healthy"]);
        assert_eq!(def.nodes, vec!["destroyed"]);
        assert_eq!(def.static_sounds, vec!["snd_exp_hit1"]);
        assert!(def.lights.is_empty());
    }

    /// `ByRange` is squared metres: 30.25 is 5.5 m, the exit trigger's range.
    #[test]
    fn by_range_keeps_the_squared_distance_the_file_holds() {
        let def = parse(DEF);

        assert_eq!(
            def.execution,
            Execution::ByRange {
                min_sq: 0.0,
                max_sq: 30.25
            }
        );
        match def.execution {
            Execution::ByRange { max_sq, .. } => assert!((max_sq.sqrt() - 5.5).abs() < 1e-5),
            other => panic!("expected ByRange, got {other:?}"),
        }
    }

    /// `reset_state` wraps its events in an object; reading it as a bare list
    /// loses every level's initial node visibility.
    #[test]
    fn reset_state_is_unwrapped_from_its_object() {
        let def = parse(DEF);

        assert_eq!(def.reset_state.len(), 1);
        assert!(matches!(
            def.reset_state[0].data,
            EventDataDesc::ObjectActiveState {
                ref node,
                state: true
            } if node == "healthy"
        ));
    }

    #[test]
    fn sequences_carry_their_on_call_flag_and_their_event_timing() {
        let def = parse(DEF);

        assert_eq!(def.sequences.len(), 2);
        assert!(
            !def.sequences[0].on_call,
            "an Initial sequence is not OnCall"
        );
        assert!(def.sequences[1].on_call);

        assert_eq!(def.sequences[0].events[0].start, EventStart::WithPrevious);
        assert_eq!(
            def.sequences[0].events[1].start,
            EventStart::Offset {
                from: OffsetFrom::Event,
                time: 1.5
            }
        );
    }

    #[test]
    fn prereqs_and_anim_refs_keep_their_kind() {
        let def = parse(DEF);

        assert_eq!(def.prereq_min, 1);
        assert_eq!(
            def.prereqs[0],
            PrereqDesc::Animation {
                name: "broken_win_01".to_string(),
                required: true
            }
        );
        assert!(def.prereqs[0].required());
        assert!(!def.prereqs[1].required());
        assert_eq!(def.anim_refs[0].kind, "CallObjectConnector");
        assert_eq!(def.anim_refs[0].local_name.as_deref(), Some("right_wake"));
    }

    #[test]
    fn the_four_reset_time_spellings_are_distinguished() {
        assert_eq!(reset_time(Some(-1.0)), ResetTime::Never);
        assert_eq!(reset_time(Some(0.0)), ResetTime::Immediate);
        assert_eq!(reset_time(Some(2.5)), ResetTime::After(2.5));
        assert_eq!(reset_time(None), ResetTime::Unset);
        // The three `-99` defs mean nothing anyone can act on.
        assert_eq!(reset_time(Some(-99.0)), ResetTime::Unset);
    }

    /// m1's `fuel_station` binds `healthy` to node 1727 and `destroyed1` to
    /// 1746; a runtime sound node's address is off the grid.
    #[test]
    fn a_listed_names_pointer_is_its_node_id() {
        assert_eq!(node_id_from_ptr(40_184_392), Some(1727));
        assert_eq!(node_id_from_ptr(40_188_116), Some(1746));
        assert_eq!(node_id_from_ptr(40_670_472 + 4), None);
        assert_eq!(node_id_from_ptr(1), None);
    }

    #[test]
    fn a_presentation_event_keeps_its_json_for_package_14() {
        let json = r#"{ "start": null,
          "data": { "Sound": { "name": "snd_x", "at_node": null } } }"#;
        let raw: mech3ax::AnimEvent = serde_json::from_str(json).expect("parses");

        let desc = event_data_desc(raw.data);

        assert_eq!(desc.kind(), "Sound");
        match desc {
            EventDataDesc::Presentation { value, .. } => {
                assert_eq!(value["name"], "snd_x");
            }
            other => panic!("expected Presentation, got {other:?}"),
        }
    }

    #[test]
    fn the_four_conditions_survive_the_flattening() {
        let cases = [
            (r#"{ "AnimationLod": 2 }"#, Condition::AnimationLod(2)),
            (
                r#"{ "PlayerRange": 1024.0 }"#,
                Condition::PlayerRange(1024.0),
            ),
            (r#"{ "RandomWeight": 0.25 }"#, Condition::RandomWeight(0.25)),
            (
                r#"{ "NodeUndercover": { "node_index": 7 } }"#,
                Condition::NodeUndercover(7),
            ),
        ];

        for (json, expected) in cases {
            let raw: mech3ax::AnimCondition = serde_json::from_str(json).expect("parses");
            assert_eq!(condition(raw), expected);
        }
    }

    /// The quaternion's scalar is in `x`, and `delta` is a per-second rate.
    #[test]
    fn an_si_frame_reads_its_scalar_first_quaternion() {
        let frame = si_frame(&serde_json::json!({
            "start_time": 0.0, "end_time": 0.25,
            "translate": { "base": { "x": 1.0, "y": 2.0, "z": 3.0 },
                           "delta": { "x": 4.0, "y": 0.0, "z": 0.0 } },
            "rotate": { "base": { "x": 0.70710677, "y": 0.0, "z": 0.70710677, "w": 0.0 },
                        "delta": { "x": 0.0, "y": 0.0, "z": 0.0 } }
        }))
        .expect("a frame");

        assert_eq!(
            frame.translate,
            Some((Vec3::new(1.0, 2.0, 3.0), Vec3::X * 4.0))
        );
        let rotation = frame.rotate.expect("a rotation");
        // 90° about y: +z turns to +x.
        assert!(
            (rotation * Vec3::Z - Vec3::X).length() < 1e-5,
            "{rotation:?}"
        );
        assert_eq!(
            frame.scale, None,
            "a channel the frame leaves out is absent"
        );
    }

    /// `anim_name` collides 5–12 times per level, so the index has to be a
    /// multi-map or `CallAnimation` silently starts one of three sparks.
    #[test]
    fn a_repeated_anim_name_indexes_every_def_that_carries_it() {
        let mut set = AnimSet::default();
        for (index, name) in ["spark_blue.flt", "spark_blue.flt", "other"]
            .into_iter()
            .enumerate()
        {
            set.by_anim_name
                .entry(name.to_string())
                .or_default()
                .push(index);
            set.defs.push(AnimDefDesc {
                anim_name: name.to_string(),
                ..default()
            });
        }

        assert_eq!(set.lookup("spark_blue.flt"), &[0, 1]);
        assert_eq!(set.lookup("nothing"), &[] as &[usize]);
        assert_eq!(
            set.find("other").map(|def| def.anim_name.as_str()),
            Some("other")
        );
    }
}
