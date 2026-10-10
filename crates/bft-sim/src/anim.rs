//! The `anim.zbd` virtual machine: Recoil's level-scripting engine.
//!
//! Every level ships 172–818 compiled *animation definitions* — scripts that
//! open doors, destroy props, fly seagulls, run the intro camera, flip the
//! node states objectives watch, and hand the player control. This module
//! loads them into entities, arms them, steps their sequences on the fixed
//! tick, and dispatches each event to a handler.
//!
//! # What is core and what is a handler
//!
//! The VM itself owns control flow — activation, prerequisites, sequence
//! timing, `Loop`, the four `If` conditions, reset — and eight handlers whose
//! behaviour mission logic depends on ([`core_handlers`]). Everything else is
//! presentation — sound, effects, lights, fog, motion — and the
//! [`handlers`] cover every kind the shipped levels use; they push
//! [`AnimOp`]s that [`apply`] and [`motion`] carry out. A kind with no
//! handler reports [`HandlerResult::Unsupported`], is logged once and
//! skipped, and a mission stays playable without any of them.
//!
//! A timed event (a motion, a fade, a light animation) answers
//! [`HandlerResult::Running`] and **blocks its sequence** until it is done;
//! parallel motion is authored as parallel sequences (docs/data/anims.md).
//!
//! # Name resolution
//!
//! A definition names nodes that are not unique level-wide: m4 has 186
//! `healthy` nodes and m5 36 `dbase`. The def's `anim_root_name` scopes the
//! lookup — the match inside that subtree wins, and only if there is none
//! does the search widen to the level ([`AnimCtx::resolve`]).
//!
//! # Copies
//!
//! Most definitions run as one instance each, on the node they are rooted
//! at. A definition rooted at a *prototype* — an effect, a wreck, an enemy
//! kind — plays a copy wherever a call places it ([`AnimCopy`]): its own
//! instance on a clone of the prototype's subtree, or on an existing subtree
//! such as a dead enemy's model, gone when it completes. That is what lets
//! one `lg_impact.flt` play at every shell and 83 enemies die at once.
//!
//! # Timing
//!
//! Sequences step on `Time<Fixed>` at 60 Hz. An event with
//! `EventStart::WithPrevious` — 41 171 of the game's 53 167 — fires in the
//! same tick as the one before it, which is how a sequence flips ten node
//! states at once. An offset counts from the animation's activation, the
//! previous event's *start*, or the sequence's start (docs/data/anims.md).

use std::collections::{HashMap, HashSet};

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bft_assets::anim::{
    AnimActivation, AnimDefDesc, AnimSet, Condition, EventDataDesc, EventDesc, EventStart,
    Execution, OffsetFrom, PrereqDesc, ResetTime, SiScriptDesc,
};

pub mod apply;
pub mod handlers;
pub mod motion;

pub use apply::{
    AnimCamera, AnimLight, AnimLights, AnimOp, AnimOpacity, AnimPlaced, AnimRest, AnimScreenFx,
    Connector, ConnectorEnd, PendingAnimOps,
};
pub use motion::{AnimMotion, AnimMotions, MotionOwner};

use crate::cinematic::{
    PendingHandOver, PlayerControl, SkipCinematic, apply_callbacks, is_entry_animation,
};
use crate::damage::NodeDamage;
use crate::level::{LevelRoot, LodSwitch, ModelRef, NodeActive, NodeIndex, NodeName, NodeZone};
use crate::physics::SimRng;
use crate::schedule::SimSet;

/// The `Callback` value that ends a mission in victory.
///
/// It fires in exactly the six `mN_exit_animation` definitions and nowhere else
/// in the game. `Callback 2`, which docs/data/anims.md first proposed, also
/// fires in every `mN_start_animation` — keying the win on it ends the mission
/// during the opening camera.
pub const MISSION_END_CALLBACK: i32 = 99;

/// `Callback` values that hand the player control (`load_control`,
/// `mN_start_animation`).
pub const CONTROL_CALLBACKS: [i32; 3] = [15, 10, 1];

/// The level's loaded definitions, for the systems that step them.
///
/// A resource rather than a handle so a headless test can build one by hand
/// without an asset server.
#[derive(Resource, Debug, Clone, Default)]
pub struct AnimProgram(pub AnimSet);

/// One definition's running state.
///
/// One entity per definition, spawned under the level root at load, so a
/// query steps them all and despawning the level takes them with it.
#[derive(Component, Debug, Clone)]
pub struct AnimInstance {
    /// Index into [`AnimProgram`]'s definitions.
    pub def: usize,
    /// Whether the definition is armed: a hit or a call can start it.
    pub armed: bool,
    /// The sequences currently stepping.
    pub running: Vec<SeqCursor>,
    /// Damage accumulated toward `def.health` by the hit activations.
    pub damage: f32,
    /// Whether this tick's prerequisite evaluation passed.
    pub prereqs_met: bool,
    /// When to re-arm, in seconds of fixed time. `None` when nothing is due.
    pub reset_at: Option<f32>,
    /// `InvalidateAnimation` set this: the definition never runs again.
    pub invalidated: bool,
    /// Whether the definition has started at least once, which is what an
    /// `Animation` prerequisite tests.
    ///
    /// Started, not completed: m1's `gate_lower` and `rocket_hit11`–`13` are
    /// `m1pickup`'s prerequisites and each calls it *while still running*, so
    /// with "completed" whichever objective fell last would never bring the
    /// VTOL.
    pub started: bool,
    /// Whether the definition has completed at least once.
    pub completed: bool,
    /// The entity of the definition's anim root object, resolved at spawn.
    /// `None` when the level has no node of that name.
    pub root: Option<Entity>,
    /// Where that root was last tick, in world space.
    ///
    /// Cached by [`arm_animations`] so `PlayerRange` — 666 conditions,
    /// evaluated inside the stepping loop — costs a field read rather than a
    /// second `GlobalTransform` query the stepper would have to hold open
    /// alongside its mutable one.
    pub root_position: Option<Vec3>,
    /// The node this run was called *at* or *with* — what the definition's
    /// `INPUT_NODE` means (2522 references, all in effect templates).
    pub input: Option<Entity>,
    /// The two ends a `CallObjectConnector` handed over: what the called
    /// definition's `ObjectConnector`s mean by `"Input"`.
    pub input_ends: Option<(ConnectorEnd, ConnectorEnd)>,
    /// Whether the definition's `reset_state` still has to be applied beyond
    /// its active states, which [`apply_initial_node_states`] already did.
    /// Cleared by the first tick.
    pub pending_reset: bool,
    /// The names this run resolves on another node's subtree, when it was
    /// called with an `operand_node`; `None` inside means the prop has no such
    /// node. See [`operand_scope`].
    pub operand_scope: Option<HashMap<String, Option<Entity>>>,
    /// The node an operand run plays on, which is then its root.
    pub played_on: Option<Entity>,
    /// Whether the root object is a prototype ([`Template`]): a call that
    /// says where to play the definition plays a copy instead of this one.
    ///
    /// [`Template`]: crate::ai::templates::Template
    pub prototype: bool,
    /// Set on a copy: the subtree it plays on. A copy is despawned when it
    /// completes. See [`AnimCopy`].
    pub copy: Option<AnimCopy>,
    /// A [`SkipCinematic`] asked this entry animation to end now; the next
    /// step finishes it ([`finish_animation`]).
    pub skip: bool,
    /// Its own `StopSequence` stopped the last sequence it had running, so
    /// the next step completes it as if that sequence had ended. m1's
    /// generator emitters end this way, by stopping the looping light that
    /// followed the shockwave; without it they never completed, and never
    /// re-armed.
    pub ran_out: bool,
}

/// What a copy of a prototype definition plays on.
///
/// The level carries one of each effect and vehicle prototype
/// (`lg_impact.flt`, `kill_laser`, `hulk_small`, `scou_easy`), and a
/// definition rooted at one is meant to play wherever it is called: at each
/// shell's impact, on each of 83 dead enemies. A call that places it
/// (`AtNode`, a connector's ends, [`CallAnim::on`]) spawns a transient
/// [`AnimInstance`] with one of these, and the prototype's own instance stays
/// what it was — the unplaced calls (`m1_start_animation` on `camera1`,
/// `countdown_from_9`) are scripts, not effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimCopy {
    /// The root of the subtree the copy's names resolve in.
    pub root: Entity,
    /// Whether the VM cloned that subtree from the prototype, and so
    /// despawns it with the copy. False for a subtree that was already
    /// there — a dead enemy's model, a destroyed turret.
    pub owned: bool,
}

impl AnimInstance {
    /// A fresh instance of a definition, armed per its activation.
    pub fn new(def: usize, activation: AnimActivation, root: Option<Entity>) -> AnimInstance {
        AnimInstance {
            def,
            // `OnStartup` is armed when the level starts, and so are the hit
            // activations: `armed` is "a hit can start it", and a destructible
            // prop has to be hittable from the first shot. Arming only
            // `OnStartup` left all 1670 `WeaponHit`/`WeaponOrCollideHit`
            // definitions — every destructible in the game — accumulating
            // damage they could never act on. `OnCall`
            // waits for its call, which starts it armed or not.
            armed: activation != AnimActivation::OnCall,
            running: Vec::new(),
            damage: 0.0,
            prereqs_met: true,
            reset_at: None,
            invalidated: false,
            started: false,
            completed: false,
            root,
            root_position: None,
            input: None,
            input_ends: None,
            pending_reset: true,
            operand_scope: None,
            played_on: None,
            prototype: false,
            copy: None,
            skip: false,
            ran_out: false,
        }
    }

    /// The node this run plays on: the operand, the copy, else the
    /// definition's root object.
    pub fn run_root(&self) -> Option<Entity> {
        self.played_on.or(self.root)
    }

    /// Whether any sequence is stepping.
    pub fn is_running(&self) -> bool {
        !self.running.is_empty()
    }
}

/// Where one running sequence has got to.
#[derive(Debug, Clone, PartialEq)]
pub struct SeqCursor {
    /// Index into the definition's sequences.
    pub seq: usize,
    /// The next event to consider.
    pub event: usize,
    /// Seconds since the animation activated.
    pub t_anim: f32,
    /// Seconds since this sequence started.
    pub t_seq: f32,
    /// Seconds since the previous event *started* — what `OffsetFrom::Event`
    /// counts from. A timed event that is still running is "the previous
    /// event" for the one after it, so this runs on under it.
    pub t_event: f32,
    /// Whether the event at [`Self::event`] has started and is running.
    pub started: bool,
    /// How long it has been running.
    pub t_running: f32,
    /// Iterations left on a `Loop`. −1 loops for ever; `None` means the
    /// sequence has not met a `Loop` yet.
    pub loops_left: Option<i32>,
    /// One entry per open `If` block: whether its branch is being taken.
    pub cond_stack: Vec<CondFrame>,
}

impl SeqCursor {
    /// A cursor at the start of a sequence.
    pub fn new(seq: usize, t_anim: f32) -> SeqCursor {
        SeqCursor {
            seq,
            event: 0,
            t_anim,
            t_seq: 0.0,
            t_event: 0.0,
            started: false,
            t_running: 0.0,
            loops_left: None,
            cond_stack: Vec::new(),
        }
    }

    /// Whether events are being executed, or skipped inside a false branch.
    pub fn executing(&self) -> bool {
        self.cond_stack.iter().all(|frame| frame.taking)
    }
}

/// One open `If`/`Elseif`/`Else`/`Endif` block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CondFrame {
    /// Whether this branch is the one being executed.
    pub taking: bool,
    /// Whether an earlier branch of this block already matched, which is what
    /// stops `Elseif` and `Else` from running a second one.
    pub satisfied: bool,
}

/// What a handler did with an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandlerResult {
    /// Finished; the sequence moves on.
    Done,
    /// Still going. The VM re-runs it next tick and the sequence waits.
    Running,
    /// No handler for this kind. Logged once, then skipped.
    Unsupported,
}

/// A behaviour for one event kind.
///
/// [`handlers`] implements the 24 presentation kinds; the eight mission logic
/// needs are [`core_handlers`].
pub trait EventHandler: Send + Sync + 'static {
    /// The event kind this handles, as [`EventDataDesc::kind`] spells it.
    fn kind(&self) -> &'static str;
    /// Run it.
    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult;
}

/// The registry: event kind → handler.
#[derive(Resource, Default)]
pub struct EventHandlers(pub HashMap<&'static str, Box<dyn EventHandler>>);

impl EventHandlers {
    /// Register a handler, replacing any already on its kind.
    pub fn insert(&mut self, handler: impl EventHandler) {
        self.0.insert(handler.kind(), Box::new(handler));
    }

    /// The handler for a kind, if there is one.
    pub fn get(&self, kind: &str) -> Option<&dyn EventHandler> {
        self.0.get(kind).map(Box::as_ref)
    }
}

/// What an event handler may read and change.
///
/// A struct rather than a `&mut World` so a handler cannot reach past what
/// the VM guarantees: the node lookups are already scoped to the definition,
/// and every output goes through a message so the sim stays replayable.
pub struct AnimCtx<'a> {
    /// The definition being stepped.
    pub def: &'a AnimDefDesc,
    /// Its instance's entity, so a handler can address it.
    pub instance: Entity,
    /// How long this event has been running, in seconds. 0 on the tick it
    /// starts, which is what lets a `Running` handler interpolate.
    pub elapsed: f32,
    /// True on the tick the event starts, false on the re-runs a `Running`
    /// result asks for.
    pub first: bool,
    /// Which sequence and event of the definition this is, so a handler that
    /// hands work to another system can tell whether it is still wanted.
    pub seq: usize,
    pub event: usize,
    /// What `INPUT_NODE` means for this run: the node the definition was
    /// called at or with.
    pub input: Option<Entity>,
    /// What `"Input"` means to an `ObjectConnector`.
    pub input_ends: Option<(ConnectorEnd, ConnectorEnd)>,
    /// The definition's root object.
    root: Option<Entity>,
    /// Seconds since the animation activated.
    pub t_anim: f32,
    /// The level's node index.
    pub nodes: &'a NodeIndex,
    /// Node name → entity, already resolved within the definition's
    /// `anim_root_name` subtree.
    scoped: &'a HashMap<String, Entity>,
    /// This run's operand subtree, which is looked in first.
    operand: Option<&'a HashMap<String, Option<Entity>>>,
    /// Where the player is, for `PlayerRange` and `ByRange`.
    pub player: Option<Vec3>,
    /// Randomness for `RandomWeight`, seeded so a replay matches.
    pub rng: &'a mut SimRng,
    /// The level's `.zan` keyframe scripts, for `ObjectMotionSiScript`.
    pub si_scripts: &'a [SiScriptDesc],
    /// The `ObjectMotion`s in flight, so their handler knows when one ends.
    pub motions: &'a AnimMotions,
    /// Everything the VM has to apply after the borrow ends.
    pub out: &'a mut AnimOutbox,
}

impl AnimCtx<'_> {
    /// The definition's anim root object, where the level has it.
    pub fn root(&self) -> Option<Entity> {
        self.root
    }

    /// The entity for a node this definition names.
    ///
    /// The definition's own subtree first, then the level: m4's 186 `healthy`
    /// nodes are all called the same, and only the scope tells them apart.
    /// Returns the first level-wide match when the name is not in scope,
    /// which is the conservative reading.
    pub fn resolve(&self, name: &str) -> Option<Entity> {
        if name == INPUT_NODE {
            return self.input;
        }
        if let Some(found) = self.operand.and_then(|operand| operand.get(name)) {
            return *found;
        }
        if let Some(entity) = self.scoped.get(name) {
            return Some(*entity);
        }
        self.nodes.by_name(name).first().copied()
    }

    /// Every entity with this name, scoped the same way.
    ///
    /// A binding that names a duplicate applies to all of them unless the
    /// definition's own subtree claims one.
    pub fn resolve_all(&self, name: &str) -> Vec<Entity> {
        if name == INPUT_NODE {
            return self.input.into_iter().collect();
        }
        if let Some(found) = self.operand.and_then(|operand| operand.get(name)) {
            return found.iter().copied().collect();
        }
        match self.scoped.get(name) {
            Some(entity) => vec![*entity],
            None => self.nodes.by_name(name).to_vec(),
        }
    }

    /// Set a node's active state, and tell everyone who cares.
    pub fn set_active(&mut self, name: &str, active: bool) {
        for entity in self.resolve_all(name) {
            self.out.active.push(ActiveChange {
                node: entity,
                name: name.to_string(),
                root: Some(self.def.anim_root_name.clone()),
                active,
            });
        }
    }
}

/// The name a definition uses for the node it was called at or with.
pub const INPUT_NODE: &str = "INPUT_NODE";

/// What a tick's handlers asked for, applied once the VM's borrows end.
///
/// Handlers cannot hold `Commands` or a `MessageWriter` — the VM already
/// holds the queries they would conflict with — so they push here and the
/// stepping system drains it.
#[derive(Debug, Default)]
pub struct AnimOutbox {
    /// Node active-state changes from `ObjectActiveState`.
    pub active: Vec<ActiveChange>,
    /// Animations to start, by `anim_name`.
    pub call: Vec<CallAnim>,
    /// Animations to stop, by `anim_name`.
    pub stop: Vec<String>,
    /// Animations to disarm permanently.
    pub invalidate: Vec<String>,
    /// Animations to re-arm.
    pub reset: Vec<String>,
    /// `(instance, sequence name)` to start.
    pub call_sequence: Vec<(Entity, String)>,
    /// `(instance, sequence name)` to stop.
    pub stop_sequence: Vec<(Entity, String)>,
    /// Engine callbacks: `(definition anim name, value)`.
    pub callback: Vec<(String, i32)>,
    /// Presentation events, passed through untouched.
    pub presentation: Vec<AnimOutput>,
    /// Transforms, opacity, motion, lights, fog, camera and screen changes,
    /// for [`apply::apply_anim_ops`] to carry out.
    pub ops: Vec<AnimOp>,
}

/// One pending node active-state change.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveChange {
    pub node: Entity,
    pub name: String,
    /// The `anim_root_name` of the definition making the change, so an
    /// objective can tell m4's 186 `healthy` nodes apart in a log.
    pub root: Option<String>,
    pub active: bool,
}

/// A request to start an animation by name.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct CallAnim {
    /// The `anim_name` to start. Every definition carrying it starts.
    pub name: String,
    /// Run it on this node instead of its own root (`operand_node`).
    pub at_node: Option<String>,
    /// The entity that asked, where one did.
    pub operand: Option<Entity>,
    /// The node the called definition sees as `INPUT_NODE`.
    pub input: Option<Entity>,
    /// Move the called definition's root object here first: a node, and an
    /// offset in its frame (`AtNode`, and `operand_node`).
    pub place_at: Option<(Entity, Vec3)>,
    /// The two ends of a `CallObjectConnector`.
    pub input_ends: Option<(ConnectorEnd, ConnectorEnd)>,
    /// Play it on this node: the definitions rooted there start, else the
    /// node is the run's operand, and a prototype's definition plays a copy
    /// on this node's subtree. How a dead enemy plays its kind's death
    /// animation on its own model, and a destroyed turret its
    /// `DESTROY_ANIM` — one `destroy_turret` per level, rooted at the first
    /// turret — on itself.
    pub on: Option<Entity>,
}

impl CallAnim {
    /// Start an animation by name, on its own root.
    pub fn new(name: impl Into<String>) -> CallAnim {
        CallAnim {
            name: name.into(),
            at_node: None,
            operand: None,
            input: None,
            place_at: None,
            input_ends: None,
            on: None,
        }
    }

    /// Start an animation by name on a node ([`Self::on`]).
    pub fn on(name: impl Into<String>, node: Entity) -> CallAnim {
        CallAnim {
            on: Some(node),
            ..CallAnim::new(name)
        }
    }

    /// Start an animation by name at a node, offset in its frame: the
    /// `AtNode` form, which is how an effect plays where a shot lands.
    pub fn at(name: impl Into<String>, node: Entity, offset: Vec3) -> CallAnim {
        CallAnim {
            input: Some(node),
            place_at: Some((node, offset)),
            ..CallAnim::new(name)
        }
    }

    /// Whether the call says where to play: what makes a prototype's
    /// definition play a copy (see [`AnimCopy`]).
    fn places(&self) -> bool {
        self.place_at.is_some() || self.on.is_some() || self.input_ends.is_some()
    }
}

/// A request from outside the VM to stop an animation by name: every
/// instance carrying it, copies included, as `StopAnimation` does.
///
/// How the engine's own effects end — the player's dust and wake trails
/// loop for ever and nothing in the scripts stops them.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct StopAnim {
    /// The `anim_name` to stop.
    pub name: String,
}

/// A node's active state changed.
///
/// Mission objectives subscribe to this: an objective completes when the node
/// it names changes from the state it had at mission start (READER-SCHEMA
/// §3.6). `bft-game` mirrors it onto `Visibility`, and colliders follow it.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct NodeActiveChanged {
    /// The node's entity.
    pub node: Entity,
    /// Its name, which is what the reader and the anim defs use.
    pub name: String,
    /// The `anim_root_name` of the definition that changed it, when one did.
    pub root: Option<String>,
    pub active: bool,
}

/// An engine callback fired.
///
/// [`MISSION_END_CALLBACK`] ends the mission; [`CONTROL_CALLBACKS`] hand the
/// player control. Everything else is logged (docs/data/anims.md).
#[derive(Message, Debug, Clone, PartialEq)]
pub struct AnimCallback {
    /// The `anim_name` of the definition that fired it.
    pub def: String,
    pub value: i32,
}

/// A presentation event, for the handlers and the sound layer.
#[derive(Message, Debug, Clone)]
pub struct AnimOutput {
    /// The event kind (`Sound`, `Effect`, `LightState`, …).
    pub kind: &'static str,
    /// mech3ax's JSON for it, untouched.
    pub value: serde_json::Value,
    /// The definition that produced it.
    pub def: String,
    /// The definition's root object — the node it hangs off.
    pub at: Option<Entity>,
    /// The event's **own** node, where it names one: `at_node` on
    /// `Sound`/`Effect`, `translate.AtNode` on `SoundNode`. Resolved in the
    /// definition's scope first, the way the VM resolves every name, so
    /// `turret` means this vehicle's turret; then level-wide. `None` when
    /// the event names no node or the name matches nothing.
    ///
    /// The scope lives here, so a presentation layer
    /// holding only [`Self::value`] cannot resolve `healthy` or `turret`
    /// correctly. 97 of the 3093 `Sound` events also carry a non-zero offset
    /// from this node, which stays in [`Self::value`].
    pub node: Option<Entity>,
}

/// The node name a presentation event places itself at, if any.
///
/// `at_node.name` on `Sound` and `Effect`; `translate.AtNode.name` on
/// `SoundNode` (docs/data/sounds.md's addendum).
pub fn presentation_node_name(value: &serde_json::Value) -> Option<&str> {
    value
        .get("at_node")
        .and_then(|at| at.get("name"))
        .or_else(|| {
            value
                .get("translate")
                .and_then(|t| t.get("AtNode"))
                .and_then(|at| at.get("name"))
        })
        .and_then(|name| name.as_str())
}

// ---------------------------------------------------------------------------
// The core handlers
// ---------------------------------------------------------------------------

/// Every handler: the eight below plus the presentation ones
/// ([`handlers::register`]).
///
/// What [`AnimPlugin`] installs. [`core_handlers`] alone is still a working
/// VM — every presentation event then reaches [`AnimOutput`] untouched.
pub fn all_handlers() -> EventHandlers {
    let mut registry = core_handlers();
    handlers::register(&mut registry);
    registry
}

/// The eight handlers mission logic depends on.
///
/// They are here and not in [`handlers`] because objectives watch node active
/// states, the mission ends on a `Callback`, and the entry animations are
/// nothing but `CallAnimation` chains — a VM without these runs no mission.
pub fn core_handlers() -> EventHandlers {
    let mut handlers = EventHandlers::default();
    handlers.insert(ObjectActiveStateHandler);
    handlers.insert(CallAnimationHandler);
    handlers.insert(StopAnimationHandler);
    handlers.insert(InvalidateAnimationHandler);
    handlers.insert(ResetAnimationHandler);
    handlers.insert(CallSequenceHandler);
    handlers.insert(StopSequenceHandler);
    handlers.insert(CallbackHandler);
    handlers
}

/// Toggles a node's active flag: the workhorse, 8937 events.
pub struct ObjectActiveStateHandler;

impl EventHandler for ObjectActiveStateHandler {
    fn kind(&self) -> &'static str {
        "ObjectActiveState"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        if let EventDataDesc::ObjectActiveState { node, state } = &event.data {
            ctx.set_active(node, *state);
        }
        HandlerResult::Done
    }
}

/// Starts every definition carrying an `anim_name`.
pub struct CallAnimationHandler;

impl EventHandler for CallAnimationHandler {
    fn kind(&self) -> &'static str {
        "CallAnimation"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        if let EventDataDesc::CallAnimation {
            name,
            operand_node,
            parameters,
            ..
        } = &event.data
        {
            let operand = operand_node.as_deref().and_then(|node| ctx.resolve(node));
            // Three forms (docs/data/anims.md): `AtNode` plays
            // the called definition *at* a node — 3365 calls, two thirds of
            // them effect templates such as `lg_impact.flt`; `WithNode` hands
            // it a node as its input without moving it (13); `operand_node`
            // plays a generic template on a prop, the way m1's
            // `oldpiergone_01` plays `generic_6` on `oldpier_01` (410): the
            // template's names resolve in the prop's subtree (see
            // `operand_scope`), so there is nothing to move.
            let (input, place_at) = match parameters {
                Some(at) => {
                    let node = ctx.resolve(&at.node);
                    let place = if at.place {
                        node.map(|node| (node, at.position))
                    } else {
                        None
                    };
                    (node, place)
                }
                None => (operand, None),
            };
            ctx.out.call.push(CallAnim {
                name: name.clone(),
                at_node: operand_node.clone(),
                operand,
                input,
                place_at,
                input_ends: None,
                on: None,
            });
        }
        // `wait_for_completion` is not honoured yet: it is set on 0.4% of
        // calls and waiting needs the called definition's own completion,
        // which the outbox does not carry back. Timed events would want the
        // same plumbing; until then a waiting call runs concurrently, which is how a non-waiting one already behaves.
        HandlerResult::Done
    }
}

/// Stops a running definition.
pub struct StopAnimationHandler;

impl EventHandler for StopAnimationHandler {
    fn kind(&self) -> &'static str {
        "StopAnimation"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        if let EventDataDesc::StopAnimation(name) = &event.data {
            ctx.out.stop.push(name.clone());
        }
        HandlerResult::Done
    }
}

/// Disarms a definition for the rest of the level.
pub struct InvalidateAnimationHandler;

impl EventHandler for InvalidateAnimationHandler {
    fn kind(&self) -> &'static str {
        "InvalidateAnimation"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        if let EventDataDesc::InvalidateAnimation(name) = &event.data {
            ctx.out.invalidate.push(name.clone());
        }
        HandlerResult::Done
    }
}

/// Re-arms an invalidated or finished definition.
pub struct ResetAnimationHandler;

impl EventHandler for ResetAnimationHandler {
    fn kind(&self) -> &'static str {
        "ResetAnimation"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        if let EventDataDesc::ResetAnimation(name) = &event.data {
            ctx.out.reset.push(name.clone());
        }
        HandlerResult::Done
    }
}

/// Starts one of the definition's own sequences.
pub struct CallSequenceHandler;

impl EventHandler for CallSequenceHandler {
    fn kind(&self) -> &'static str {
        "CallSequence"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        if let EventDataDesc::CallSequence(name) = &event.data {
            ctx.out.call_sequence.push((ctx.instance, name.clone()));
        }
        HandlerResult::Done
    }
}

/// Stops one of the definition's own sequences.
pub struct StopSequenceHandler;

impl EventHandler for StopSequenceHandler {
    fn kind(&self) -> &'static str {
        "StopSequence"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        if let EventDataDesc::StopSequence(name) = &event.data {
            ctx.out.stop_sequence.push((ctx.instance, name.clone()));
        }
        HandlerResult::Done
    }
}

/// Fires an engine callback.
pub struct CallbackHandler;

impl EventHandler for CallbackHandler {
    fn kind(&self) -> &'static str {
        "Callback"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        if let EventDataDesc::Callback(value) = &event.data {
            ctx.out.callback.push((ctx.def.anim_name.clone(), *value));
        }
        HandlerResult::Done
    }
}

// ---------------------------------------------------------------------------
// Spawning
// ---------------------------------------------------------------------------

/// Names the VM has already complained about, so it complains once.
#[derive(Resource, Debug, Default)]
pub struct AnimWarnings {
    /// Event kinds with no handler, and sequences that hit the per-tick bound.
    unsupported: HashSet<String>,
    /// Names `CallAnimation` asked for that this level does not define.
    missing_anim: HashSet<String>,
}

/// One instance per definition, plus the scoped name tables.
///
/// The per-definition name scope is built once here rather than on every
/// event: resolving `healthy` inside m4's `fntrocket` means walking that
/// subtree, and 534 definitions doing it per tick would be the VM's whole
/// cost.
#[derive(Resource, Debug, Default)]
pub struct AnimScopes(pub Vec<HashMap<String, Entity>>);

/// Build the instances for a level's definitions.
///
/// Call once, after the level hierarchy is spawned, and follow it with
/// [`apply_initial_node_states`] — the two are separate because this one only
/// reads the hierarchy while that one writes to it.
///
/// A definition's scope is its root object's whole subtree, with the names
/// under `anim_root_name` winning a collision. `anim_root_name`'s subtree
/// alone is not enough, because `destroyed` is usually its *sibling*: m1's
/// `generic6/healthy` and `generic6/destroyed`, `pturret.flt/healthy` and
/// `pturret.flt/destroyed`. Out of scope, such a name would fall back to the
/// level-wide lookup and mean all 91 of m1's `destroyed` nodes — 686
/// definitions across the game declare a name that would. The World root
/// is the exception: its subtree is the level, where the level-wide lookup
/// (every match) is already the right reading.
///
/// **Names the definition lists are bound exactly**, over all of that: each
/// `objects`/`nodes` entry carries the node the original engine bound it to
/// (`AnimDefDesc::node_refs`), and that node wins. The scope stays for the
/// names a script uses without listing them. Where the two disagree the
/// binding is the data's meaning: m1's `hshit_1` gate resets a `dbase` its
/// own model lacks, bound to the VTOL wreck's; resolved level-wide instead,
/// it hid every wreck base in m1 at load, the fuses' burnt bases among them.
pub fn spawn_anim_instances(
    commands: &mut Commands,
    program: &AnimSet,
    nodes: &NodeIndex,
    children: &Query<&Children>,
    names: &Query<&NodeName>,
    templates: &Query<(), With<crate::ai::templates::Template>>,
    level_root: Entity,
) -> AnimScopes {
    let mut scopes = Vec::with_capacity(program.defs.len());
    let top_level: HashSet<Entity> = children
        .get(level_root)
        .map(|kids| kids.iter().collect())
        .unwrap_or_default();
    let is_world = |entity: Entity| top_level.contains(&entity) && !templates.contains(entity);
    // Every node of every prototype, not just the roots: `smoke1` — the puff
    // `exp_medium`'s debris trails call every 0.2 s, 13 levels over — and
    // `smoke2` are rooted at the model node *under* `smoke1.flt` and
    // `smoke2.flt`. Keyed on the roots alone they were not prototypes, so a
    // placed call replayed the hidden original where it was parked instead
    // of copying it to the debris, and no trail was ever seen.
    let in_template: HashSet<Entity> = top_level
        .iter()
        .copied()
        .filter(|entity| templates.contains(*entity))
        .flat_map(|root| subtree_entities(root, children))
        .collect();

    for (index, def) in program.defs.iter().enumerate() {
        // The nodes the original engine bound this definition's names to.
        let bound: HashMap<String, Entity> = def
            .node_refs
            .iter()
            .filter_map(|(name, id)| {
                let entity = nodes.get(*id)?;
                names
                    .get(entity)
                    .is_ok_and(|found| found.0 == *name)
                    .then(|| (name.clone(), entity))
            })
            .collect();

        // The anim root object is the node the definition hangs off: its
        // binding where it lists itself (m6 has two trees of `belev`, and
        // only one is the level's), else the first of that name.
        let root = bound
            .get(&def.name)
            .copied()
            .or_else(|| nodes.by_name(&def.name).first().copied());
        let scope_root = scope_root(root, &def.anim_root_name, children, names);
        let mut scope = match root {
            Some(root) if !is_world(root) => subtree_names(root, children, names),
            _ => HashMap::new(),
        };
        if let Some(entity) = scope_root {
            scope.extend(subtree_names(entity, children, names));
        }
        // The bindings win: they are what the scripts mean, where the scope
        // is a guess. m1's `hshit_1` gate resets a `dbase` it does not have;
        // guessed level-wide, that hid every wreck base in the level.
        scope.extend(bound);

        let mut instance = AnimInstance::new(index, def.activation, root);
        instance.prototype = root.is_some_and(|root| in_template.contains(&root));
        commands.spawn((
            instance,
            Name::new(format!("anim:{}", def.anim_name)),
            ChildOf(level_root),
        ));

        scopes.push(scope);
    }

    AnimScopes(scopes)
}

/// Apply every definition's `reset_state`: the level's initial node states.
///
/// **This is where a level's starting visibility comes from**
/// (docs/data/anims.md). The `gamez` data has `active: true` on every node of
/// every level, so a level spawned without this shows its destroyed hulks, its
/// opened doors and its explosion debris all at once, on top of the intact
/// versions. 3108 of the game's 4798 definitions carry one.
///
/// Call once, after [`spawn_anim_instances`] and before the first tick.
/// Returns how many nodes it turned off, which is what a level load logs.
pub fn apply_initial_node_states(
    program: &AnimSet,
    scopes: &AnimScopes,
    nodes: &NodeIndex,
    actives: &mut Query<&mut NodeActive>,
) -> usize {
    let mut changed = 0usize;

    for (index, def) in program.defs.iter().enumerate() {
        // An entry animation's reset is its *end* state — the VTOL landed,
        // `bft_00` in the bay — and waits for the intro to finish or be
        // skipped ([`finish_animation`]).
        if is_entry_animation(def) {
            continue;
        }
        let empty = HashMap::new();
        let scope = scopes.0.get(index).unwrap_or(&empty);

        for event in &def.reset_state {
            // Only the active states: the rest of a `reset_state` is initial
            // transforms and camera setup, which the definition applies on
            // its first tick (`pending_reset`).
            let EventDataDesc::ObjectActiveState { node, state } = &event.data else {
                continue;
            };

            // The definition's own subtree first, exactly as the VM resolves
            // a name while stepping — `healthy` means this definition's.
            let targets = match scope.get(node) {
                Some(entity) => vec![*entity],
                None => nodes.by_name(node).to_vec(),
            };

            for target in targets {
                let Ok(mut active) = actives.get_mut(target) else {
                    continue;
                };
                if active.0 != *state {
                    active.0 = *state;
                    changed += 1;
                }
            }
        }
    }

    changed
}

/// The node a definition's names are resolved against.
///
/// `anim_root_name` is a node *under* the root object, and often the root
/// object itself (`horizon`/`horizon`). Falling back to the root object
/// keeps a definition whose scope node is missing addressable.
fn scope_root(
    root: Option<Entity>,
    anim_root_name: &str,
    children: &Query<&Children>,
    names: &Query<&NodeName>,
) -> Option<Entity> {
    let root = root?;
    if anim_root_name.is_empty() {
        return Some(root);
    }

    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        if names.get(entity).is_ok_and(|name| name.0 == anim_root_name) {
            return Some(entity);
        }
        if let Ok(kids) = children.get(entity) {
            stack.extend(kids.iter());
        }
    }

    Some(root)
}

/// `root` and every node under it.
fn subtree_entities(root: Entity, children: &Query<&Children>) -> Vec<Entity> {
    let mut all = Vec::new();
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        all.push(entity);
        if let Ok(kids) = children.get(entity) {
            stack.extend(kids.iter());
        }
    }
    all
}

/// Every named node in a subtree, nearest first.
///
/// Nearest wins on a collision inside one subtree, because the node a
/// definition means by `healthy` is the one closest to its own root.
fn subtree_names(
    root: Entity,
    children: &Query<&Children>,
    names: &Query<&NodeName>,
) -> HashMap<String, Entity> {
    let mut found = HashMap::new();
    let mut queue = std::collections::VecDeque::from([root]);

    while let Some(entity) = queue.pop_front() {
        if let Ok(name) = names.get(entity) {
            found.entry(name.0.clone()).or_insert(entity);
        }
        if let Ok(kids) = children.get(entity) {
            queue.extend(kids.iter());
        }
    }

    found
}

/// The names a run called with an `operand_node` resolves on the operand.
///
/// `generic_6`, `destroy_turret`, `destroy_bunker` and the rest are shared
/// destruction scripts: `oldpiergone_01` calls `generic_6` on `oldpier_01`,
/// and `healthy`, `destroyed` and `part1`–`part6` then mean the pier's. A
/// name the script declares but the prop lacks — a pier has five parts,
/// `generic_6` animates six — maps to `None`, not to the level-wide lookup:
/// that fallback would break every `destroyed` node in m1 when one pier
/// fell.
fn operand_scope(
    def: &AnimDefDesc,
    operand: Entity,
    children: &Query<&Children>,
    names: &Query<&NodeName>,
) -> HashMap<String, Option<Entity>> {
    let mut scope: HashMap<String, Option<Entity>> = subtree_names(operand, children, names)
        .into_iter()
        .map(|(name, entity)| (name, Some(entity)))
        .collect();
    for name in def.objects.iter().chain(&def.nodes) {
        scope.entry(name.clone()).or_insert(None);
    }
    scope
}

// ---------------------------------------------------------------------------
// Stepping
// ---------------------------------------------------------------------------

/// How long the level has been running, in fixed-tick seconds.
///
/// The VM's own clock rather than `Time<Fixed>::elapsed`, so a mission
/// restart starts the animations over without touching the app's time.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct AnimClock(pub f32);

/// Where the player is, for `ByRange`, `ByZone` and `PlayerRange`.
///
/// Set by the play state each tick. `None` before a player exists, which is
/// every tick of a viewer session — the range conditions then read as false
/// and the zone ones as "not in it", which is why a viewer shows no scripted
/// triggers rather than all of them at once.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct PlayerLocation {
    pub position: Option<Vec3>,
    /// The `zone_id` the player is standing in, for `Execution::ByZone`.
    pub zone: Option<i32>,
}

/// The level's `bft_00` node once it has become the player's model.
///
/// The scripts treat `bft_00` as the player, and in the original the player
/// *is* `bft_00`. Here it hangs off the hull, and who may move it depends on
/// [`PlayerControl`]:
///
/// - **Driving**: the engine owns the player. The VM leaves this node's active
///   state alone and drops absolute placements, which would otherwise park
///   the model away from the hull it rides on. A relative translate moves the
///   hull instead: the elevators in m3 and m6 and the teleport screens in m6
///   and m13 carry the player that way.
/// - **Cinematic**: the scripts own the player. An absolute translate or
///   rotate places the *hull* (and stops it), and `ObjectActiveState` shows
///   or hides the model — the intros hide `bft_00` for the fly-in, the resets
///   put it in the landed VTOL's bay, the exits hide it in the VTOL.
///
/// An unconditional "leave it to the engine" rule would be right while the
/// player drives and wrong during a cinematic.
///
/// [`PlayerControl`]: crate::cinematic::PlayerControl
#[derive(Component, Debug, Clone, Copy)]
pub struct PlayerModelRoot {
    /// The player's vehicle, which the scripts' placements move instead.
    pub hull: Entity,
}

/// The node states the VM may write. The player's model among them only
/// during a cinematic, which [`apply_outbox`] checks.
type ScriptedActives<'w, 's> = Query<'w, 's, &'static mut NodeActive>;

/// The VM's plugin: the registry, the clock and the two systems.
pub struct AnimPlugin;

impl Plugin for AnimPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<CallAnim>()
            .add_message::<StopAnim>()
            .add_message::<NodeActiveChanged>()
            .add_message::<AnimCallback>()
            .add_message::<AnimOutput>()
            .init_resource::<AnimWarnings>()
            .init_resource::<AnimClock>()
            .init_resource::<PlayerLocation>()
            .init_resource::<AnimScopes>()
            .init_resource::<PendingAnimOps>()
            .init_resource::<AnimMotions>()
            .init_resource::<AnimLights>()
            .init_resource::<AnimCamera>()
            .init_resource::<AnimScreenFx>()
            // The stepper reads and writes the control state, so a VM added
            // without the rest of the sim still has one.
            .init_resource::<PlayerControl>()
            .init_resource::<PendingHandOver>()
            .add_message::<SkipCinematic>()
            .insert_resource(all_handlers())
            .add_systems(
                FixedUpdate,
                (
                    apply::forget_previous_level,
                    accumulate_hit_damage,
                    call_destruction_animations,
                    call_impact_animations,
                    call_fire_animations,
                    stop_requested_animations,
                    start_called_animations,
                    arm_animations,
                    skip_entry_animations,
                    step_animations,
                    apply::apply_anim_ops,
                    motion::advance_motions,
                    apply::advance_light_animations,
                    apply::sync_node_colliders,
                )
                    .chain()
                    .in_set(SimSet::Anim),
            );
    }
}

/// Start the animations other systems asked for by name.
///
/// [`CallAnim`] is how everything outside the VM starts one: the mission's
/// entry animation, a form change's transition, an enemy's death animation,
/// a weapon's kill animation. A call from *inside* the VM never comes through
/// here — [`step_animations`] applies those from its outbox in the same tick,
/// so a `CallAnimation` chain does not take one tick per link.
///
/// A separate system rather than a branch of the stepper because the stepper
/// *writes* `CallAnim` — one system doing both would take `Res` and `ResMut`
/// on the buffer, which Bevy rejects outright.
// Eight parameters: what [`start_by_name`] reads and writes, plus the copier
// for the copies it asks for.
#[allow(clippy::too_many_arguments)]
pub fn start_called_animations(
    mut calls: MessageReader<CallAnim>,
    program: Option<Res<AnimProgram>>,
    mut warnings: ResMut<AnimWarnings>,
    mut pending: ResMut<PendingAnimOps>,
    mut instances: Query<(Entity, &mut AnimInstance)>,
    nodes: Option<Res<NodeIndex>>,
    // What prerequisites and operand scopes read, grouped.
    (actives, children, names): (Query<&NodeActive>, Query<&Children>, Query<&NodeName>),
    mut copier: AnimCopier,
) {
    let Some(program) = program else { return };
    // No index yet means no nodes, so an `Object` prerequisite fails rather
    // than the call being lost.
    let node_active = |name: &str| {
        nodes
            .as_deref()
            .and_then(|nodes| node_active(nodes, name, |entity| actives.get(entity).ok()))
    };

    let mut copies = Vec::new();
    for call in calls.read() {
        start_by_name(
            call,
            &program.0,
            &mut instances,
            node_active,
            (&children, &names),
            &mut warnings,
            &mut pending.0,
            &mut copies,
        );
    }
    for request in copies {
        copier.spawn(request, &program.0, (&children, &names), |entity| {
            actives.get(entity).ok().map(|active| active.0)
        });
    }
}

/// Carry out [`StopAnim`] requests: clear every matching instance's running
/// sequences, so a copy retires and a singleton idles until called again.
pub fn stop_requested_animations(
    mut stops: MessageReader<StopAnim>,
    program: Option<Res<AnimProgram>>,
    mut instances: Query<(Entity, &mut AnimInstance)>,
) {
    let Some(program) = program else {
        stops.clear();
        return;
    };
    for stop in stops.read() {
        for_each_named(&stop.name, &program.0, &mut instances, |instance| {
            instance.running.clear();
            // Stopped, not finished: no reset rule applies.
            instance.ran_out = false;
        });
    }
}

/// Play what a destruction calls for: the victim's own death animation and
/// the killing weapon's `KILL_ANIMATION`.
///
/// - An enemy plays the definition named after its kind (`scou_easy`),
///   rooted at the prototype its model was cloned from, **on that model**:
///   hull fling, `hulk_small` wreck, `fire_40`, the gunner. No reader key
///   names it; the kind and the definition share the name in every level.
/// - A turret plays `ai.zrd`'s `DESTROY_ANIM` on itself. Each level has one
///   `destroy_turret`, rooted at its first turret; every other turret plays
///   it as an operand.
/// - The weapon's `IMPACT/default/KILL_ANIMATION` (`kill_laser`, …) plays
///   at the victim.
pub fn call_destruction_animations(
    mut destroyed: MessageReader<crate::damage::Destroyed>,
    mut calls: MessageWriter<CallAnim>,
    weapons: Option<Res<crate::weapons::WeaponTable>>,
    ai: Option<Res<crate::ai::AiTable>>,
    enemies: Query<(&crate::ai::Enemy, Option<&Children>)>,
    turrets: Query<&crate::ai::TurretAi>,
    models: Query<(), With<crate::ai::ClonedFrom>>,
) {
    for event in destroyed.read() {
        if let Ok((enemy, children)) = enemies.get(event.entity) {
            let model = children
                .into_iter()
                .flat_map(|children| children.iter())
                .find(|child| models.contains(*child));
            if let Some(model) = model {
                calls.write(CallAnim::on(enemy.kind.as_str(), model));
            }
        }

        if let Ok(turret) = turrets.get(event.entity)
            && let Some(def) = ai.as_ref().and_then(|ai| ai.turrets.get(turret.def))
            && !def.destroy_anim.is_empty()
        {
            calls.write(CallAnim::on(def.destroy_anim.clone(), event.entity));
        }

        let kill = event
            .weapon
            .and_then(|weapon| weapons.as_ref()?.by_index(weapon))
            .and_then(|weapon| weapon.impact.get(&crate::weapons::Surface::Default))
            .and_then(|impact| impact.kill_animation.clone());
        if let Some(kill) = kill {
            calls.write(CallAnim::at(kill, event.entity, Vec3::ZERO));
        }
    }
}

/// Play what `weapons.zrd`'s `IMPACT` block names where each shot lands.
///
/// Per surface (`default`, `water`, `quicksand`; a weapon naming no block
/// for the surface uses its `default`):
///
/// - **`ANIMATION`** on a miss — the ground, a wall, a prop: the mortar's
///   and missiles' `impact_miss` (orange debris, a flare, the `exp_bunk`
///   sprite), the pulse guns' `ric_green.flt`, `water_miss` in the sea.
///   `ANIMATION_ALWAYS` (the nukes' `expnuke.flt`) plays it on a hit too.
/// - **`DAMAGE_ANIM_ON_HEALTH`**, else **`DAMAGE_ANIMATION`**, on a target
///   the hit left alive, picked by the health it has left: sparks for a
///   scratch, `greenparts` for a wreck-to-be.
/// - Nothing more on a kill: [`call_destruction_animations`] plays the
///   weapon's `KILL_ANIMATION` and the victim's own death.
/// - **`EFFECT`**, an `effects.zrd` sprite (`exp_bunk` for the mines,
///   `smoke1` for napalm), on every impact, as an `Effect` output for the
///   presentation to draw — the same shape as the anim `Effect` event.
///
/// Until this ran, no impact in the game played its data: every hit drew
/// the same procedural fireball.
#[allow(clippy::too_many_arguments)]
pub fn call_impact_animations(
    mut hits: MessageReader<crate::weapons::ProjectileHit>,
    mut calls: MessageWriter<CallAnim>,
    mut outputs: MessageWriter<AnimOutput>,
    weapons: Option<Res<crate::weapons::WeaponTable>>,
    program: Option<Res<AnimProgram>>,
    healths: Query<&crate::damage::Health>,
    parents: Query<&ChildOf>,
    level_roots: Query<(Entity, &GlobalTransform), With<LevelRoot>>,
    mut names: Local<HashMap<String, Option<String>>>,
) {
    let (Some(weapons), Some(program), Some((level_root, level_at))) =
        (weapons, program, level_roots.iter().next())
    else {
        hits.clear();
        return;
    };
    if program.is_changed() {
        names.clear();
    }
    let to_level = level_at.affine().inverse();

    for hit in hits.read() {
        let Some(def) = weapons.by_index(hit.weapon) else {
            continue;
        };
        let Some(impact) = def
            .impact
            .get(&hit.surface)
            .or_else(|| def.impact.get(&crate::weapons::Surface::Default))
        else {
            continue;
        };

        let victim = hit.hit.and_then(|entity| {
            crate::weapons::health_owner(entity, |e| healths.contains(e), &parents)
        });
        let left = victim
            .and_then(|victim| healths.get(victim).ok())
            .map(|health| health.fraction());

        let mut play = Vec::with_capacity(2);
        match left {
            // Killed: the destruction animations have it.
            Some(fraction) if fraction <= 0.0 => {}
            Some(fraction) => play.extend(impact.damage_anim_for(fraction)),
            None => play.extend(impact.animation.as_deref()),
        }
        if def.impact_always && left.is_some() {
            play.extend(impact.animation.as_deref());
        }

        let offset = to_level.transform_point3(hit.at);
        for template in play {
            let name = names
                .entry(template.to_string())
                .or_insert_with(|| fire_animation_name(&program.0, template));
            if let Some(name) = name.clone() {
                calls.write(CallAnim::at(name, level_root, offset));
            }
        }

        if let Some(effect) = &impact.effect {
            outputs.write(AnimOutput {
                kind: "Effect",
                value: serde_json::json!({
                    "name": effect,
                    "at_node": { "pos": { "x": offset.x, "y": offset.y, "z": offset.z } },
                }),
                def: def.id.clone(),
                at: Some(level_root),
                node: Some(level_root),
            });
        }
    }
}

/// Play each shot's `FIRE/ANIMATION` where it left the barrel.
///
/// `weapons.zrd` names a template, not a definition: `rfpg_mzl.flt`,
/// `muzzleburst.flt`, `bftmbrst.flt`, `miss_launch.flt`. Each level carries a
/// definition rooted at it (`rfpg_mzl`: the flash on for a frame at twice its
/// size, and a blue light for a tenth of a second), and a placed call plays a
/// copy of it at the firepoint.
///
/// A shot with no firepoint node — a turret's, which fires from a point —
/// plays at the shooter, offset to where the shot left.
///
/// // approximation: `FIRE/RANDOM_ROTATE` (the two pulse guns) is not
/// applied; the copy takes the firepoint's orientation.
pub fn call_fire_animations(
    mut fired: MessageReader<crate::weapons::WeaponFired>,
    mut calls: MessageWriter<CallAnim>,
    weapons: Option<Res<crate::weapons::WeaponTable>>,
    program: Option<Res<AnimProgram>>,
    transforms: Query<&GlobalTransform>,
    mut names: Local<HashMap<String, Option<String>>>,
) {
    let (Some(weapons), Some(program)) = (weapons, program) else {
        fired.clear();
        return;
    };
    if program.is_changed() {
        names.clear();
    }

    for shot in fired.read() {
        let Some(template) = weapons
            .by_index(shot.weapon)
            .and_then(|def| def.fire.animation.as_deref())
        else {
            continue;
        };
        let name = names
            .entry(template.to_string())
            .or_insert_with(|| fire_animation_name(&program.0, template));
        let Some(name) = name.clone() else {
            continue;
        };

        let call = match shot.firepoint {
            Some(node) => CallAnim::at(name, node, Vec3::ZERO),
            None => {
                let Ok(shooter) = transforms.get(shot.shooter) else {
                    continue;
                };
                let offset = shooter.affine().inverse().transform_point3(shot.origin);
                CallAnim::at(name, shot.shooter, offset)
            }
        };
        calls.write(call);
    }
}

/// The definition a `FIRE/ANIMATION` template plays: the one rooted at it.
pub fn fire_animation_name(program: &AnimSet, template: &str) -> Option<String> {
    program
        .defs
        .iter()
        .find(|def| def.anim_root_name == template || def.name == template)
        .map(|def| def.anim_name.clone())
}

/// Feed `NodeDamage` into the definitions whose nodes were hit.
///
/// A hit-activated definition accumulates damage until the total reaches
/// `health`; `health == 0` means the first hit does it, which is how impact
/// sparks work (docs/data/anims.md).
///
/// **A hit belongs to the definition whose `anim_root_name` node it lands
/// under** — that node or anywhere in its subtree, since the collider is
/// often a child: m1's `drain_01` is rooted at `drhealth_01` and the surface
/// that gets shot is its child `o205`. The lists a definition declares
/// (`objects`/`nodes`) are what its script *touches*, not what takes its
/// hits, and reading them as the latter broke props three ways:
///
/// - **Either/or props fired both ways at once.** m1's `fuel_station` has
///   two definitions, rooted at the roof posts `postright` and `postwrong`;
///   both list the `healthy` group the two posts share, so one shot on
///   either post fired both collapses and the second hid the first's wreck.
///   m1's `frcgen` (`genleft`/`genright`) and m6's elevator `beltrig`
///   (`btrigupon`/`btrigdownon`) are the same shape.
/// - **A listed name the prop lacks matched the whole level.** m1's
///   `gate_part1` lists a `dbase` its gate does not have, so a shot on any
///   wreck's `dbase` broke that gate; `liteh_hit` (the lighthouse) lists
///   `part1` and took hits from every prop's debris.
/// - **A neighbour a script animates took its hits.** m1's `shak` lists the
///   `vwbus` it launches, so shooting the bus broke the shack, whose script
///   then flew the bus off and hid it.
///
/// A definition whose `anim_root_name` is not under its root object takes
/// no hits: all 92 in the game are a multi-piece prop's definition for a
/// piece its model lacks (m6's `hit_wall_102` names `wall1` on a wall of
/// `wall2` and `wall3`), and each such prop has a working definition per
/// piece it does have.
pub fn accumulate_hit_damage(
    mut damage: MessageReader<NodeDamage>,
    program: Option<Res<AnimProgram>>,
    scopes: Res<AnimScopes>,
    parents: Query<&ChildOf>,
    mut instances: Query<&mut AnimInstance>,
) {
    let Some(program) = program else { return };

    let hits: Vec<&NodeDamage> = damage.read().collect();
    if hits.is_empty() {
        return;
    }

    // Each hit node and its ancestors, walked once per hit rather than once
    // per definition.
    let chains: Vec<Vec<Entity>> = hits
        .iter()
        .map(|hit| {
            let mut chain = vec![hit.node];
            let mut cursor = hit.node;
            while let Ok(child_of) = parents.get(cursor) {
                cursor = child_of.parent();
                chain.push(cursor);
                if chain.len() > 64 {
                    break;
                }
            }
            chain
        })
        .collect();

    for mut instance in &mut instances {
        let Some(def) = program.0.defs.get(instance.def) else {
            continue;
        };
        if instance.invalidated || !def.activation.takes_weapon_hits() {
            continue;
        }
        let Some(hit_root) = hit_root(def, scopes.0.get(instance.def), instance.root) else {
            continue;
        };
        for (hit, chain) in hits.iter().zip(&chains) {
            if chain.contains(&hit_root) {
                instance.damage += hit.amount;
            }
        }
    }
}

/// The node whose subtree takes a hit definition's hits: its
/// `anim_root_name`, resolved in its scope, or its root object when that is
/// what the name means. `None` when the name is not under the root object.
fn hit_root(
    def: &AnimDefDesc,
    scope: Option<&HashMap<String, Entity>>,
    root: Option<Entity>,
) -> Option<Entity> {
    if def.anim_root_name.is_empty() || def.anim_root_name == def.name {
        return root;
    }
    scope
        .and_then(|scope| scope.get(&def.anim_root_name))
        .copied()
}

/// Arm what is due: hits that reached `health`, reset timers, range and zone.
///
/// `ByRange` and `ByZone` are *polled*, not edge-triggered (docs/data/anims.md),
/// and they layer on top of activation rather than replacing it: an
/// `OnStartup` definition with `ByRange` is armed from the start and runs
/// when the player comes close.
// Eight parameters because arming is where every condition on "may this
// definition start" meets: the program, the clock, the player's place, the
// instances, and the node lookups the prerequisites and the range and zone
// conditions read. Splitting it would spread one decision over two ticks.
#[allow(clippy::too_many_arguments)]
pub fn arm_animations(
    program: Option<Res<AnimProgram>>,
    clock: Res<AnimClock>,
    player: Res<PlayerLocation>,
    mut instances: Query<(Entity, &mut AnimInstance)>,
    transforms: Query<&GlobalTransform>,
    zones: Query<&NodeZone>,
    actives: Query<&NodeActive>,
    nodes: Option<Res<NodeIndex>>,
) {
    let (Some(program), Some(nodes)) = (program, nodes) else {
        return;
    };

    // Which definitions have started, for the `Animation` prerequisites.
    let started = started_names(&program.0, instances.iter().map(|(_, instance)| instance));
    let node_active = |name: &str| node_active(&nodes, name, |entity| actives.get(entity).ok());

    for (_, mut instance) in &mut instances {
        let Some(def) = program.0.defs.get(instance.def) else {
            continue;
        };
        if instance.invalidated || !def.active {
            continue;
        }

        instance.prereqs_met = prereqs_met(def, &started, node_active);
        instance.root_position = instance
            .run_root()
            .and_then(|entity| transforms.get(entity).ok())
            .map(|transform| transform.translation());

        // A reset timer that has come due re-arms the definition.
        if let Some(due) = instance.reset_at
            && clock.0 >= due
        {
            instance.reset_at = None;
            instance.armed = true;
            instance.damage = 0.0;
        }

        if instance.is_running() || !instance.armed || !instance.prereqs_met {
            continue;
        }

        // Accumulated damage arms a hit definition. `health == 0` fires on
        // the first hit, which `> 0.0` catches because `damage` is only ever
        // positive.
        let hit = def.activation.takes_weapon_hits()
            && instance.damage > 0.0
            && instance.damage >= def.health;

        let in_execution = match def.execution {
            Execution::None => false,
            Execution::ByRange { min_sq, max_sq } => {
                // Squared metres in the file, squared metres here: no root is
                // taken anywhere on this path.
                match (player.position, instance.root_position) {
                    (Some(player), Some(root)) => {
                        let distance_sq = player.distance_squared(root);
                        distance_sq >= min_sq && distance_sq <= max_sq
                    }
                    _ => false,
                }
            }
            Execution::ByZone => {
                let root_zone = instance
                    .root
                    .and_then(|entity| zones.get(entity).ok())
                    .map(|zone| zone.0);
                match (player.zone, root_zone) {
                    (Some(player), Some(root)) => player == root,
                    _ => false,
                }
            }
        };

        // `OnStartup` with no execution condition runs the moment it is
        // armed; anything else waits for its condition or for a call.
        let start = match def.activation {
            AnimActivation::OnStartup => def.execution == Execution::None || in_execution,
            AnimActivation::OnCall => in_execution,
            _ => hit || in_execution,
        };

        if start {
            start_instance(&mut instance, def, clock.0);
        }
    }
}

/// The `anim_name`s of every definition that has started at least once.
fn started_names<'p, 'i>(
    program: &'p AnimSet,
    instances: impl Iterator<Item = &'i AnimInstance>,
) -> HashSet<&'p str> {
    instances
        .filter(|instance| instance.started)
        .filter_map(|instance| program.defs.get(instance.def))
        .map(|def| def.anim_name.as_str())
        .collect()
}

/// A named node's active state, for the `Object` and `Parent` prerequisites.
///
/// Generic over the lookup because the arming system reads `&NodeActive` and
/// the stepper holds `&mut NodeActive`.
fn node_active<'w>(
    nodes: &NodeIndex,
    name: &str,
    get: impl Fn(Entity) -> Option<&'w NodeActive>,
) -> Option<bool> {
    nodes
        .by_name(name)
        .first()
        .and_then(|entity| get(*entity))
        .map(|state| state.0)
}

/// Whether a definition's prerequisites hold.
///
/// `required` ones must all hold; of the rest, `prereq_min` must
/// (docs/data/anims.md).
fn prereqs_met(
    def: &AnimDefDesc,
    started: &HashSet<&str>,
    node_active: impl Fn(&str) -> Option<bool>,
) -> bool {
    if def.prereqs.is_empty() {
        return true;
    }

    let holds = |prereq: &PrereqDesc| -> bool {
        match prereq {
            PrereqDesc::Animation { name, .. } => started.contains(name.as_str()),
            // `Object` and `Parent` both test a node's active state; the
            // difference is which node, and the compiled form names it either
            // way.
            PrereqDesc::Object { name, active, .. } | PrereqDesc::Parent { name, active, .. } => {
                node_active(name) == Some(*active)
            }
        }
    };

    let mut optional_met = 0u8;
    for prereq in &def.prereqs {
        let met = holds(prereq);
        if prereq.required() {
            if !met {
                return false;
            }
        } else if met {
            optional_met = optional_met.saturating_add(1);
        }
    }

    optional_met >= def.prereq_min
}

/// Start a definition: its `Initial` sequences run, the `OnCall` ones wait.
fn start_instance(instance: &mut AnimInstance, def: &AnimDefDesc, _now: f32) {
    instance.armed = false;
    instance.started = true;
    instance.operand_scope = None;
    instance.played_on = None;
    instance.damage = 0.0;
    instance.running = def
        .sequences
        .iter()
        .enumerate()
        .filter(|(_, sequence)| !sequence.on_call)
        .map(|(index, _)| SeqCursor::new(index, 0.0))
        .collect();
}

/// Mark the running intro to be finished on this tick's step.
///
/// Only an entry animation ([`is_entry_animation`]) can be skipped: an intro
/// has a hand-off state to land in, and an exit only ends the mission. Any
/// running entry animation is finished, which covers m1, where the mission
/// calls `start_single_player` and `m1_start_animation` is the one running.
pub fn skip_entry_animations(
    mut skips: MessageReader<SkipCinematic>,
    program: Option<Res<AnimProgram>>,
    mut instances: Query<&mut AnimInstance>,
) {
    if skips.read().count() == 0 {
        return;
    }
    let Some(program) = program else { return };

    for mut instance in &mut instances {
        let Some(def) = program.0.defs.get(instance.def) else {
            continue;
        };
        if instance.is_running() && instance.copy.is_none() && is_entry_animation(def) {
            info!("skipping {}", def.anim_name);
            instance.skip = true;
        }
    }
}

/// Step every running sequence by one fixed tick.
// The VM's whole state meets here: the program, the clock, the registry, the
// node lookups and the four message writers its handlers feed. Splitting it
// would mean handing the outbox between systems a tick apart, which is
// exactly the ordering the single pass makes impossible to get wrong.
#[allow(clippy::too_many_arguments)]
pub fn step_animations(
    time: Res<Time<Fixed>>,
    mut clock: ResMut<AnimClock>,
    program: Option<Res<AnimProgram>>,
    handlers: Res<EventHandlers>,
    mut copier: AnimCopier,
    player: Res<PlayerLocation>,
    mut rng: ResMut<SimRng>,
    mut warnings: ResMut<AnimWarnings>,
    // Grouped to stay inside Bevy's sixteen-parameter limit.
    (motions, mut pending, mut control, mut hand_over): (
        Res<AnimMotions>,
        ResMut<PendingAnimOps>,
        ResMut<PlayerControl>,
        ResMut<PendingHandOver>,
    ),
    tree: (
        Query<&Children>,
        Query<&NodeName>,
        Query<(), With<PlayerModelRoot>>,
    ),
    nodes: Option<Res<NodeIndex>>,
    mut instances: Query<(Entity, &mut AnimInstance)>,
    mut actives: ScriptedActives,
    mut active_changed: MessageWriter<NodeActiveChanged>,
    mut callbacks: MessageWriter<AnimCallback>,
    mut outputs: MessageWriter<AnimOutput>,
) {
    let (Some(program), Some(nodes)) = (program, nodes) else {
        return;
    };
    let dt = time.delta_secs();
    clock.0 += dt;

    let mut outbox = AnimOutbox::default();
    // Copies that may have finished. Decided after the outbox: a copy whose
    // first sequence only `CallSequence`s its others (`kill_laser`) has run
    // out of cursors in the loop and gets them back from the outbox.
    let mut retired = Vec::new();

    for (entity, mut instance) in &mut instances {
        if !instance.is_running() && !instance.pending_reset && !instance.ran_out {
            // A copy is done once nothing runs, however it got there: by
            // completing, or by a `StopAnimation` last tick.
            if let Some(copy) = instance.copy {
                retired.push((entity, copy));
            }
            continue;
        }
        let Some(def) = program.0.defs.get(instance.def) else {
            continue;
        };
        let empty = HashMap::new();
        let scope = copier.scopes.0.get(instance.def).unwrap_or(&empty);
        let env = StepEnv {
            handlers: &handlers,
            scope,
            nodes: &nodes,
            player: &player,
            si_scripts: &program.0.si_scripts,
            motions: &motions,
            program: &program.0,
        };

        // The rest of the `reset_state` — initial transforms, scales,
        // opacity, texture cycles, the camera — once, on the first tick.
        // `apply_initial_node_states` has already done the active states,
        // without announcing them, and repeating them here would.
        // A copy's active states are applied too: a clone already has its
        // prototype's, but a subtree it plays on (a dead enemy's model) has
        // never been reset.
        //
        // Not an entry animation's: that one is the hand-off state, applied
        // when the intro finishes or is skipped. Applied on the first tick,
        // m1's disarmed its own intro before it was called and m2–m6 handed
        // over control before the VTOL had set off (`tests/real_cinematics.rs`).
        if instance.pending_reset {
            instance.pending_reset = false;
            if !is_entry_animation(def) {
                let active = instance.copy.is_some_and(|copy| !copy.owned);
                dispatch_reset(entity, &instance, def, &env, active, &mut rng, &mut outbox);
            }
            if !instance.is_running() {
                continue;
            }
        }

        if instance.skip {
            instance.skip = false;
            finish_animation(entity, &mut instance, def, &env, &mut rng, &mut outbox);
            instance.completed = true;
            continue;
        }

        step_instance(
            entity,
            &mut instance,
            def,
            dt,
            &env,
            &mut rng,
            &mut warnings,
            &mut outbox,
        );

        // A definition that has run out of sequences has completed: its reset
        // rule decides whether it can run again. A copy does not run again;
        // it goes.
        if !instance.is_running() {
            instance.ran_out = false;
            instance.completed = true;
            if let Some(copy) = instance.copy {
                retired.push((entity, copy));
                continue;
            }
            // An intro that played out lands where a skip would.
            if is_entry_animation(def) {
                finish_animation(entity, &mut instance, def, &env, &mut rng, &mut outbox);
            }
            match def.reset_time {
                ResetTime::Immediate => instance.armed = true,
                ResetTime::After(delay) => instance.reset_at = Some(clock.0 + delay),
                // A hit definition with no timed reset takes hits again:
                // the ones that must break once invalidate themselves or
                // hide what takes their hits (`ResetTime::Never`). Hits
                // landed while it ran do not count toward the next run.
                ResetTime::Never if def.activation.takes_weapon_hits() => {
                    instance.armed = true;
                    instance.damage = 0.0;
                }
                // Anything else with `Never` stays done; `Unset` is mostly
                // the looping definitions, which never reach here.
                ResetTime::Never | ResetTime::Unset => {}
            }
            // `auto_reset_node_states` puts the definition's nodes back the
            // way its `reset_state` had them, so it can play again: the
            // active states, and the positions, scales and opacities its events moved away from.
            if def.auto_reset_node_states {
                dispatch_reset(entity, &instance, def, &env, true, &mut rng, &mut outbox);
            }
        }
    }

    // Before the tick's node changes: entering a cinematic has to hold for
    // the `bft_00` hide an exit fires one event before its `Callback 2`. A
    // hand-over waits until the ops are applied (`settle_control`).
    apply_callbacks(&mut control, &mut hand_over, &outbox.callback);

    let copies = apply_outbox(
        outbox,
        &program.0,
        &nodes,
        (&tree.0, &tree.1),
        (&tree.2, control.is_cinematic()),
        &mut instances,
        &mut actives,
        &mut active_changed,
        &mut callbacks,
        &mut outputs,
        &mut warnings,
        &mut pending,
    );
    for (entity, copy) in retired {
        if instances
            .get(entity)
            .is_ok_and(|(_, instance)| !instance.is_running())
        {
            copier.retire(entity, copy);
        }
    }
    for request in copies {
        copier.spawn(request, &program.0, (&tree.0, &tree.1), |entity| {
            actives.get(entity).ok().map(|active| active.0)
        });
    }
}

/// What stepping one definition reads besides its own instance.
pub struct StepEnv<'a> {
    handlers: &'a EventHandlers,
    scope: &'a HashMap<String, Entity>,
    nodes: &'a NodeIndex,
    player: &'a PlayerLocation,
    si_scripts: &'a [SiScriptDesc],
    motions: &'a AnimMotions,
    program: &'a AnimSet,
}

impl StepEnv<'_> {
    /// A handler context for one event of one definition.
    #[allow(clippy::too_many_arguments)]
    fn ctx<'b>(
        &'b self,
        def: &'b AnimDefDesc,
        entity: Entity,
        instance: &'b AnimInstance,
        position: (usize, usize, f32, f32, bool),
        rng: &'b mut SimRng,
        outbox: &'b mut AnimOutbox,
    ) -> AnimCtx<'b> {
        let (seq, event, elapsed, t_anim, first) = position;
        AnimCtx {
            def,
            instance: entity,
            elapsed,
            first,
            seq,
            event,
            input: instance.input,
            input_ends: instance.input_ends,
            // An operand run plays on the operand, so that is its root.
            root: instance.run_root(),
            t_anim,
            nodes: self.nodes,
            scoped: self.scope,
            operand: instance.operand_scope.as_ref(),
            player: self.player.position,
            rng,
            si_scripts: self.si_scripts,
            motions: self.motions,
            out: outbox,
        }
    }
}

/// Run a definition's `reset_state` through the handlers.
///
/// The events are instantaneous state sets — 14 of the 37 kinds occur there,
/// none of them timed — so a `Running` answer is not waited for. With
/// `active` false the `ObjectActiveState`s are left out: at load they have
/// already been applied, silently, by [`apply_initial_node_states`].
fn dispatch_reset(
    entity: Entity,
    instance: &AnimInstance,
    def: &AnimDefDesc,
    env: &StepEnv,
    active: bool,
    rng: &mut SimRng,
    outbox: &mut AnimOutbox,
) {
    for event in &def.reset_state {
        let skip = match &event.data {
            EventDataDesc::ObjectActiveState { .. } => !active,
            // Control flow has no meaning outside a sequence.
            EventDataDesc::If(_)
            | EventDataDesc::Elseif(_)
            | EventDataDesc::Else
            | EventDataDesc::Endif
            | EventDataDesc::Loop(_) => true,
            _ => false,
        };
        if skip {
            continue;
        }
        let Some(handler) = env.handlers.get(event.data.kind()) else {
            continue;
        };
        let mut ctx = env.ctx(
            def,
            entity,
            instance,
            (usize::MAX, usize::MAX, 0.0, 0.0, true),
            rng,
            outbox,
        );
        handler.run(&mut ctx, event);
    }
}

/// Apply a definition's `reset_state` in full — active states, transforms,
/// callbacks, calls — and stop its sequences. What the engine does when an
/// entry animation finishes or is skipped.
///
/// The six `mN_start_animation`s keep their *end* state in `reset_state`
/// (asserted in `tests/real_cinematics.rs`): the VTOL landed, `bft_00` in the bay, the cargo
/// door open, `Callback 15/10/1`, and the calls that start the hover and the
/// exit trigger. Finishing and skipping land in the same state by
/// construction.
///
/// A timed event a skip cuts off mid-run — the camera's flight, m1's
/// half-done fade in from black — is run once more at its end first, so it
/// is left where playing out would have left it rather than frozen part-way.
/// Events that had not started are not run: what they would have set is
/// what the reset sets.
pub fn finish_animation(
    entity: Entity,
    instance: &mut AnimInstance,
    def: &AnimDefDesc,
    env: &StepEnv,
    rng: &mut SimRng,
    outbox: &mut AnimOutbox,
) {
    /// Long enough that every timed event in the game is over.
    const THE_END: f32 = 1.0e6;

    for cursor in std::mem::take(&mut instance.running) {
        if !cursor.started || !cursor.executing() {
            continue;
        }
        let Some(event) = def
            .sequences
            .get(cursor.seq)
            .and_then(|sequence| sequence.events.get(cursor.event))
        else {
            continue;
        };
        let Some(handler) = env.handlers.get(event.data.kind()) else {
            continue;
        };
        let mut ctx = env.ctx(
            def,
            entity,
            instance,
            (cursor.seq, cursor.event, THE_END, cursor.t_anim, false),
            rng,
            outbox,
        );
        handler.run(&mut ctx, event);
    }

    info!("{} finished: applying its hand-off state", def.anim_name);
    dispatch_reset(entity, instance, def, env, true, rng, outbox);
    first_frames_of_calls(entity, instance, def, env, rng, outbox);
}

/// Put the nodes a reset's calls fly at the first frame of their scripts.
///
/// m6's reset parks `vtol1` 720 m from the bay and then calls
/// `m6_airdrop_loop`, whose first frame is where the fly-in ended, to within
/// a centimetre: the parked pose is never meant to be seen. The called
/// animation only steps next tick, and `m6_do_exit_trigger` — called in the
/// same reset — measures `PlayerRange` from the VTOL on that tick, sees the
/// player 720 m off and sends the VTOL away with the player in the bay. So
/// the SI scripts a called animation starts at its very first event are
/// posed at t = 0 here, after the reset's own placements.
fn first_frames_of_calls(
    entity: Entity,
    instance: &AnimInstance,
    def: &AnimDefDesc,
    env: &StepEnv,
    rng: &mut SimRng,
    outbox: &mut AnimOutbox,
) {
    let Some(handler) = env.handlers.get("ObjectMotionSiScript") else {
        return;
    };
    for event in &def.reset_state {
        let EventDataDesc::CallAnimation { name, .. } = &event.data else {
            continue;
        };
        for called in env
            .program
            .lookup(name)
            .iter()
            .filter_map(|&i| env.program.defs.get(i))
        {
            for sequence in called.sequences.iter().filter(|sequence| !sequence.on_call) {
                let Some(first) = sequence.events.first() else {
                    continue;
                };
                if first.start != EventStart::WithPrevious
                    || first.data.kind() != "ObjectMotionSiScript"
                {
                    continue;
                }
                let mut ctx = env.ctx(
                    def,
                    entity,
                    instance,
                    (usize::MAX, usize::MAX, 0.0, 0.0, true),
                    rng,
                    outbox,
                );
                handler.run(&mut ctx, first);
            }
        }
    }
}

/// Step one instance's sequences.
#[allow(clippy::too_many_arguments)]
fn step_instance(
    entity: Entity,
    instance: &mut AnimInstance,
    def: &AnimDefDesc,
    dt: f32,
    env: &StepEnv,
    rng: &mut SimRng,
    warnings: &mut AnimWarnings,
    outbox: &mut AnimOutbox,
) {
    let (handlers, scope, nodes, player) = (env.handlers, env.scope, env.nodes, env.player);
    let mut finished = Vec::new();

    for cursor_index in 0..instance.running.len() {
        let mut cursor = instance.running[cursor_index].clone();
        let Some(sequence) = def.sequences.get(cursor.seq) else {
            finished.push(cursor_index);
            continue;
        };

        cursor.t_anim += dt;
        cursor.t_seq += dt;
        cursor.t_event += dt;
        if cursor.started {
            cursor.t_running += dt;
        }

        // Dispatch every event whose start time has passed. The loop is a
        // `while` and not a `for` because a `WithPrevious` event fires in the
        // same tick as the one before it — 41 171 of the game's 53 167 do —
        // and because `Loop` moves the cursor backwards.
        let mut guard = 0usize;
        while cursor.event < sequence.events.len() {
            // A sequence of `WithPrevious` events is finite, but a `Loop`
            // with a zero-length body is not. The bound is per tick, so a
            // legitimate long burst still completes over two.
            guard += 1;
            if guard > MAX_EVENTS_PER_TICK {
                warn_once(
                    &mut warnings.unsupported,
                    format!("loop:{}", def.anim_name),
                    || {
                        warn!(
                            "{} sequence {} dispatched {MAX_EVENTS_PER_TICK} events in one \
                             tick; pausing it until the next",
                            def.anim_name, sequence.name
                        )
                    },
                );
                break;
            }

            let event = &sequence.events[cursor.event];
            // A running event was due when it started; it is re-run until it
            // says it is done.
            if !cursor.started && !event_is_due(event, &cursor) {
                break;
            }
            let first = !cursor.started;
            if first {
                // This event is now "the previous event" for the next one:
                // `OffsetFrom::Event` counts from its *start*, and a timed one
                // keeps that clock running under it. m2's gates show it —
                // a 3 s `ObjectMotionFromTo` followed by `{Event, 3.0}`
                // swaps the gate models exactly as the motion ends.
                cursor.t_event = 0.0;
                cursor.t_running = 0.0;
            }

            // Control flow is the VM's, not a handler's: an `If` whose branch
            // is not taken must still be tracked so its `Endif` closes it.
            match &event.data {
                EventDataDesc::If(condition) => {
                    let taking = cursor.executing()
                        && evaluate(condition, def, instance, nodes, player, rng);
                    cursor.cond_stack.push(CondFrame {
                        taking,
                        satisfied: taking,
                    });
                    advance(&mut cursor);
                    continue;
                }
                EventDataDesc::Elseif(condition) => {
                    if let Some(frame) = cursor.cond_stack.last_mut() {
                        let outer = true;
                        let taking = outer
                            && !frame.satisfied
                            && evaluate(condition, def, instance, nodes, player, rng);
                        frame.taking = taking;
                        frame.satisfied |= taking;
                    }
                    advance(&mut cursor);
                    continue;
                }
                EventDataDesc::Else => {
                    if let Some(frame) = cursor.cond_stack.last_mut() {
                        frame.taking = !frame.satisfied;
                        frame.satisfied = true;
                    }
                    advance(&mut cursor);
                    continue;
                }
                EventDataDesc::Endif => {
                    cursor.cond_stack.pop();
                    advance(&mut cursor);
                    continue;
                }
                EventDataDesc::Loop(count) => {
                    if !cursor.executing() {
                        advance(&mut cursor);
                        continue;
                    }
                    // −1 loops for ever (911 of the game's 1254 loops); a
                    // positive count is decremented and the sequence restarts
                    // until it runs out.
                    let left = cursor.loops_left.unwrap_or(*count);
                    if left < 0 {
                        cursor.event = 0;
                        cursor.t_seq = 0.0;
                        cursor.t_event = 0.0;
                        cursor.loops_left = Some(left);
                        // A `Loop` that restarts the sequence within this
                        // tick would spin; the rest of the body waits for the
                        // next one.
                        break;
                    }
                    if left > 0 {
                        cursor.event = 0;
                        cursor.t_seq = 0.0;
                        cursor.t_event = 0.0;
                        cursor.loops_left = Some(left - 1);
                        break;
                    }
                    cursor.loops_left = None;
                    advance(&mut cursor);
                    continue;
                }
                _ => {}
            }

            if !cursor.executing() {
                advance(&mut cursor);
                continue;
            }

            let kind = event.data.kind();
            let result = match handlers.get(kind) {
                Some(handler) => {
                    let mut ctx = env.ctx(
                        def,
                        entity,
                        instance,
                        (
                            cursor.seq,
                            cursor.event,
                            cursor.t_running,
                            cursor.t_anim,
                            first,
                        ),
                        rng,
                        outbox,
                    );
                    handler.run(&mut ctx, event)
                }
                None => HandlerResult::Unsupported,
            };

            match result {
                HandlerResult::Done => advance(&mut cursor),
                // The sequence waits here and the handler is re-run next
                // tick with a larger `elapsed`, which is how a timed motion
                // interpolates. Timed events *block* their sequence: m1's
                // debris flies with `run_time: null` and is switched off by
                // the very next, untimed event — which only makes sense once
                // it has landed — and `expnuke`'s three `FbfxColorFromTo`s
                // chain a fade in, a hold and a fade out with no offsets at
                // all. Parallel motion is authored as parallel sequences.
                HandlerResult::Running => {
                    cursor.started = true;
                    break;
                }
                HandlerResult::Unsupported => {
                    // Presentation events go out as messages even when
                    // nothing handles them, so sounds and fades can be added
                    // without touching this.
                    if let EventDataDesc::Presentation { kind, value } = &event.data {
                        // Resolved the way a handler's names are: the copy
                        // or operand first, then the scope, then the level.
                        let node = presentation_node_name(value).and_then(|name| {
                            match instance.operand_scope.as_ref().and_then(|s| s.get(name)) {
                                Some(found) => *found,
                                None => scope
                                    .get(name)
                                    .copied()
                                    .or_else(|| nodes.by_name(name).first().copied()),
                            }
                        });
                        outbox.presentation.push(AnimOutput {
                            kind,
                            value: value.clone(),
                            def: def.anim_name.clone(),
                            at: instance.run_root(),
                            node,
                        });
                    }
                    warn_once(&mut warnings.unsupported, kind.to_string(), || {
                        debug!("no handler for anim event {kind}; skipping every one of them")
                    });
                    advance(&mut cursor);
                }
            }
        }

        if cursor.event >= sequence.events.len() {
            finished.push(cursor_index);
        }
        instance.running[cursor_index] = cursor;
    }

    // Back to front, so an earlier removal does not shift a later index.
    for index in finished.into_iter().rev() {
        instance.running.remove(index);
    }
}

/// How many events one sequence may dispatch in a tick.
///
/// A guard against a zero-length `Loop`, not a budget: the longest run of
/// `WithPrevious` events in the shipped data is far below it.
const MAX_EVENTS_PER_TICK: usize = 512;

/// Move to the next event.
///
/// The per-event clock is *not* restarted here: it counts from an event's
/// start, which is where the stepper resets it.
fn advance(cursor: &mut SeqCursor) {
    cursor.event += 1;
    cursor.started = false;
    cursor.t_running = 0.0;
}

/// Whether an event's start time has arrived.
fn event_is_due(event: &EventDesc, cursor: &SeqCursor) -> bool {
    match event.start {
        EventStart::WithPrevious => true,
        EventStart::Offset { from, time } => {
            let elapsed = match from {
                OffsetFrom::Animation => cursor.t_anim,
                OffsetFrom::Event => cursor.t_event,
                OffsetFrom::Sequence => cursor.t_seq,
            };
            elapsed >= time
        }
    }
}

/// Evaluate one `If`/`Elseif` condition.
fn evaluate(
    condition: &Condition,
    def: &AnimDefDesc,
    instance: &AnimInstance,
    _nodes: &NodeIndex,
    player: &PlayerLocation,
    rng: &mut SimRng,
) -> bool {
    use rand::RngExt;

    match condition {
        // LOD is presentation: the sim has no camera and no detail level, so
        // the branch authored for the highest detail is the one taken. The
        // alternative — skipping it — would leave the 1204 `AnimationLod`
        // blocks dead, and they hold the real behaviour.
        Condition::AnimationLod(_) => true,
        Condition::PlayerRange(range_sq) => {
            // Squared, like `ByRange`. The definition's root is where it is
            // measured from, and without one the condition is false rather
            // than accidentally true everywhere.
            match (player.position, instance.root_position) {
                (Some(player), Some(root)) => player.distance_squared(root) <= *range_sq,
                _ => false,
            }
        }
        Condition::RandomWeight(weight) => rng.0.random::<f32>() < *weight,
        // Undercover needs a roof test the physics layer does not expose yet;
        // 56 events in the game use it, all on weather effects. False means
        // "not sheltered", which is the state a level starts in.
        Condition::NodeUndercover(_) => {
            let _ = def;
            false
        }
    }
}

/// Log a message the first time a key is seen.
fn warn_once(seen: &mut HashSet<String>, key: String, log: impl FnOnce()) {
    if seen.insert(key) {
        log();
    }
}

/// Apply everything the tick's handlers asked for.
///
/// Returns the copies the calls asked for, which need the copier the caller
/// holds.
#[allow(clippy::too_many_arguments)]
fn apply_outbox(
    outbox: AnimOutbox,
    program: &AnimSet,
    nodes: &NodeIndex,
    tree: (&Query<&Children>, &Query<&NodeName>),
    (player_models, cinematic): (&Query<(), With<PlayerModelRoot>>, bool),
    instances: &mut Query<(Entity, &mut AnimInstance)>,
    actives: &mut ScriptedActives,
    active_changed: &mut MessageWriter<NodeActiveChanged>,
    callbacks: &mut MessageWriter<AnimCallback>,
    outputs: &mut MessageWriter<AnimOutput>,
    warnings: &mut AnimWarnings,
    pending: &mut PendingAnimOps,
) -> Vec<CopyRequest> {
    pending.0.extend(outbox.ops);

    for change in outbox.active {
        // The player's model is the scripts' to show and hide only during a
        // cinematic (see [`PlayerModelRoot`]).
        if !cinematic && player_models.contains(change.node) {
            debug!(
                "{} → {} left to the engine: the player is driving",
                change.name, change.active
            );
            continue;
        }
        let Ok(mut active) = actives.get_mut(change.node) else {
            continue;
        };
        // Only a *change* is announced: an objective completes when its node
        // changes from the state it had at mission start, and a definition
        // that re-asserts the current state must not complete one.
        if active.0 == change.active {
            continue;
        }
        active.0 = change.active;
        active_changed.write(NodeActiveChanged {
            node: change.node,
            name: change.name,
            root: change.root,
            active: change.active,
        });
    }

    // Applied here and not echoed onto the `CallAnim` bus:
    // `start_called_animations` would read the echo next tick and run every
    // call a second time, which let `rocket_count`'s second pass through
    // `two_rocket` see `one_rocket` started by the first.
    let mut copies = Vec::new();
    for call in outbox.call {
        let node_active = |name: &str| node_active(nodes, name, |entity| actives.get(entity).ok());
        start_by_name(
            &call,
            program,
            instances,
            node_active,
            tree,
            warnings,
            &mut pending.0,
            &mut copies,
        );
    }

    for name in outbox.stop {
        for_each_named(&name, program, instances, |instance| {
            instance.running.clear();
            // Stopped, not finished: no reset rule applies.
            instance.ran_out = false;
        });
    }

    for name in outbox.invalidate {
        // Disarm, but let a run in progress finish. Definitions invalidate
        // *themselves* as they start — m1's `dh_doors_open` does it in its
        // first sequence while two others slide the doors, and `misc01`–`NN`
        // likewise — so clearing the running sequences here stopped every
        // one-shot prop the moment it began.
        for_each_named(&name, program, instances, |instance| {
            instance.invalidated = true;
            instance.armed = false;
            instance.reset_at = None;
        });
    }

    for name in outbox.reset {
        for_each_named(&name, program, instances, |instance| {
            instance.invalidated = false;
            instance.armed = true;
            instance.damage = 0.0;
            instance.reset_at = None;
        });
    }

    for (entity, name) in outbox.call_sequence {
        let Ok((_, mut instance)) = instances.get_mut(entity) else {
            continue;
        };
        let Some(def) = program.defs.get(instance.def) else {
            continue;
        };
        call_sequence(&mut instance, def, &name);
    }

    for (entity, name) in outbox.stop_sequence {
        let Ok((_, mut instance)) = instances.get_mut(entity) else {
            continue;
        };
        let Some(def) = program.defs.get(instance.def) else {
            continue;
        };
        let Some(index) = def.sequences.iter().position(|seq| seq.name == name) else {
            continue;
        };
        let was_running = instance.is_running();
        instance.running.retain(|cursor| cursor.seq != index);
        instance.ran_out |= was_running && !instance.is_running();
    }

    for (def, value) in outbox.callback {
        callbacks.write(AnimCallback { def, value });
    }

    for output in outbox.presentation {
        outputs.write(output);
    }

    copies
}

/// Start every definition carrying an `anim_name`, or arm it to wait.
///
/// A call is activation, not a bypass of what gates it: a definition whose
/// prerequisites fail ignores the call, and one with an `execution` condition
/// is only armed, for [`arm_animations`] to start when the player comes in
/// range or zone (DESIGN §anim VM). Starting both unconditionally let m1's
/// first rocket kill call `m1pickup` early, which called `m1_exit_trigger`
/// with the player nowhere near its 5.5 m, whose exit animation ends the
/// mission with callback 99.
///
/// Prerequisites are read fresh, not from the tick's cached `prereqs_met`:
/// `rocket_count` calls `three_rocket`, `two_rocket`, `one_rocket` in that
/// order so that each kill passes exactly one of them, which only works if
/// each call sees the ones before it.
///
/// A call made *at* a node moves the definition's root object there first.
/// A prototype's definition is the exception: a call that says where to play
/// it — at a node, between a connector's ends, on an operand or
/// [`CallAnim::on`] — plays a copy instead (see [`AnimCopy`]), pushed onto
/// `copies` for [`AnimCopier::spawn`], so one `lg_impact.flt` plays wherever
/// each shell lands and two piers broken together each get their own
/// `generic_6`.
///
/// [`CallAnim::on`] only starts the definitions rooted at that node. When
/// none is — `destroy_turret` and `destroy_bunker` are often one script on
/// one node, played on the others — the node is the run's operand instead.
// One parameter per thing a call can change: the instances, the ops the
// placement queues and the copies it asks for, plus what the prerequisites
// and operand scopes read.
#[allow(clippy::too_many_arguments)]
fn start_by_name(
    call: &CallAnim,
    program: &AnimSet,
    instances: &mut Query<(Entity, &mut AnimInstance)>,
    node_active: impl Fn(&str) -> Option<bool>,
    tree: (&Query<&Children>, &Query<&NodeName>),
    warnings: &mut AnimWarnings,
    ops: &mut Vec<AnimOp>,
    copies: &mut Vec<CopyRequest>,
) {
    let name = call.name.as_str();
    let targets = program.lookup(name);
    if targets.is_empty() {
        warn_once(&mut warnings.missing_anim, name.to_string(), || {
            warn!("no animation named {name} in this level")
        });
        return;
    }

    let gated = targets
        .iter()
        .filter_map(|&index| program.defs.get(index))
        .any(|def| !def.prereqs.is_empty());
    let started = if gated {
        started_names(program, instances.iter().map(|(_, instance)| instance))
    } else {
        HashSet::new()
    };

    // `on` names the definitions rooted there, where there are any.
    let rooted_on = call.on.filter(|&on| {
        instances.iter().any(|(_, instance)| {
            targets.contains(&instance.def) && instance.copy.is_none() && instance.root == Some(on)
        })
    });
    let operand = match (call.at_node.as_ref(), call.operand) {
        (Some(_), Some(operand)) => Some(operand),
        _ => call.on.filter(|_| rooted_on.is_none()),
    };

    for (_, mut instance) in instances.iter_mut() {
        if !targets.contains(&instance.def) || instance.invalidated || instance.copy.is_some() {
            continue;
        }
        if rooted_on.is_some() && instance.root != rooted_on {
            continue;
        }
        let Some(def) = program.defs.get(instance.def) else {
            continue;
        };
        if !prereqs_met(def, &started, &node_active) {
            continue;
        }
        if instance.prototype
            && def.execution == Execution::None
            && (call.places() || operand.is_some())
        {
            // Started, for the `Animation` prerequisites that name it.
            instance.started = true;
            copies.push(CopyRequest {
                def: instance.def,
                prototype: instance.root,
                on: call.on.or(operand),
                call: call.clone(),
            });
            continue;
        }
        instance.input = call.input.or(operand);
        instance.input_ends = call.input_ends;
        // Only a prototype is placed where it is called. A definition rooted
        // in the level stays where the level put it: m1's fuses call their
        // `powerbay_sparksN` at the `sparks` node under them, and moving the
        // root there shifted the whole fuse ~3.4 km, leaving a hole in the
        // arena wall. Those six, `hivolt_fx` and m10's `snake_loop` are the
        // only such calls in the game, against 3516 on prototypes.
        if instance.prototype
            && let (Some((at, offset)), Some(root)) = (call.place_at, instance.root)
        {
            ops.push(AnimOp::PlaceAt {
                node: root,
                at,
                offset,
                rotate: true,
            });
        }
        if def.execution != Execution::None {
            instance.armed = true;
            continue;
        }
        // Otherwise it runs now, whatever its activation says: that is what
        // `OnCall` means, and a hit definition called by name (an enemy's
        // death animation, say) must play too.
        start_instance(&mut instance, def, 0.0);
        if let Some(operand) = operand {
            instance.operand_scope = Some(operand_scope(def, operand, tree.0, tree.1));
            instance.played_on = Some(operand);
        }
    }
}

/// What spawning and retiring a copy needs besides the VM's own queries.
///
/// A `SystemParam` because both [`start_called_animations`] and
/// [`step_animations`] spawn copies, and the stepper is already at Bevy's
/// sixteen-parameter limit.
#[derive(SystemParam)]
pub struct AnimCopier<'w, 's> {
    commands: Commands<'w, 's>,
    scopes: Res<'w, AnimScopes>,
    lights: ResMut<'w, AnimLights>,
    transforms: Query<'w, 's, &'static Transform>,
    globals: Query<'w, 's, &'static GlobalTransform>,
    models: Query<'w, 's, &'static ModelRef>,
    lods: Query<'w, 's, &'static LodSwitch>,
    level_roots: Query<'w, 's, Entity, With<LevelRoot>>,
    parents: Query<'w, 's, &'static ChildOf>,
}

impl AnimCopier<'_, '_> {
    /// Spawn the copy a call asked for, and its instance.
    ///
    /// On an existing subtree ([`CopyRequest::on`]) the names resolve there;
    /// otherwise the prototype's subtree is cloned under the level root —
    /// names, transforms, models, LOD bands and the active states its reset
    /// left it in, but not colliders: debris that the player drives into
    /// would be a gameplay change the scripts never asked for. Placed at the
    /// call's node, else where the prototype is parked.
    ///
    /// The clone's world transforms are written here and not left to
    /// propagation, which only runs once a frame: a copy stepped by a second
    /// fixed tick in the same frame would otherwise launch its debris from
    /// the origin.
    fn spawn(
        &mut self,
        request: CopyRequest,
        program: &AnimSet,
        tree: (&Query<&Children>, &Query<&NodeName>),
        active: impl Fn(Entity) -> Option<bool>,
    ) {
        let Some(def) = program.defs.get(request.def) else {
            return;
        };
        let (children, names) = tree;
        let level_root = self.level_roots.iter().next();

        let (root, owned, scope) = match request.on {
            Some(on) => {
                let mut scope = subtree_names(on, children, names);
                if let Some(entity) = scope_root(Some(on), &def.anim_root_name, children, names) {
                    scope.extend(subtree_names(entity, children, names));
                }
                (on, false, scope)
            }
            None => {
                let Some(prototype) = request.prototype else {
                    return;
                };
                let clones = self.clone_subtree(prototype, &request.call, level_root, tree, active);
                let Some(&root) = clones.get(&prototype) else {
                    return;
                };
                // The prototype's scope, moved onto the clone: it was built
                // the same way (root subtree, `anim_root_name` winning).
                let scope = self
                    .scopes
                    .0
                    .get(request.def)
                    .map(|scope| {
                        scope
                            .iter()
                            .filter_map(|(name, entity)| {
                                clones.get(entity).map(|copy| (name.clone(), *copy))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                (root, true, scope)
            }
        };

        // Names resolve on the copy and nowhere else: a name the script
        // declares that this subtree lacks means nothing, not every node of
        // that name in the level.
        let mut resolved: HashMap<String, Option<Entity>> = scope
            .into_iter()
            .map(|(name, entity)| (name, Some(entity)))
            .collect();
        let prototype_names = self
            .scopes
            .0
            .get(request.def)
            .into_iter()
            .flat_map(|s| s.keys());
        for name in def.objects.iter().chain(&def.nodes).chain(prototype_names) {
            resolved.entry(name.clone()).or_insert(None);
        }

        let mut instance = AnimInstance::new(request.def, def.activation, Some(root));
        start_instance(&mut instance, def, 0.0);
        instance.input = request.call.input.or(request.on);
        instance.input_ends = request
            .call
            .input_ends
            .map(|(from, to)| (self.freeze(from), self.freeze(to)));
        instance.operand_scope = Some(resolved);
        instance.copy = Some(AnimCopy { root, owned });

        let mut spawned = self
            .commands
            .spawn((instance, Name::new(format!("anim:{} copy", def.anim_name))));
        if let Some(level_root) = level_root {
            spawned.insert(ChildOf(level_root));
        }
    }

    /// Clone a prototype's subtree under the level root, placed for `call`.
    ///
    /// Returns prototype node → clone, the root included.
    fn clone_subtree(
        &mut self,
        prototype: Entity,
        call: &CallAnim,
        level_root: Option<Entity>,
        tree: (&Query<&Children>, &Query<&NodeName>),
        active: impl Fn(Entity) -> Option<bool>,
    ) -> HashMap<Entity, Entity> {
        let (children, names) = tree;
        let parent_world = level_root
            .and_then(|root| self.globals.get(root).ok())
            .copied()
            .unwrap_or_default();

        // Where the root goes, in world space: `PlaceAt`'s rule — the node's
        // position plus the offset in its frame, turned the way it faces.
        let parked = self
            .globals
            .get(prototype)
            .map(GlobalTransform::compute_transform)
            .unwrap_or_default();
        let world = match call.place_at {
            Some((at, offset)) => match self.globals.get(at) {
                Ok(at) => {
                    let at = at.compute_transform();
                    Transform {
                        translation: at.transform_point(offset),
                        rotation: at.rotation,
                        scale: parked.scale,
                    }
                }
                Err(_) => parked,
            },
            // A connector copy — a trail's fading segment — starts at its
            // first end rather than wherever the prototype is parked, which
            // is the world origin for the trails: its connector only moves
            // it there on its first step.
            None => match call.input_ends.map(|(from, _)| self.freeze(from)) {
                Some(ConnectorEnd {
                    node: None, offset, ..
                }) => Transform {
                    translation: offset,
                    ..parked
                },
                Some(ConnectorEnd {
                    node: Some(node),
                    offset,
                    ..
                }) => match self.globals.get(node) {
                    Ok(at) => Transform {
                        translation: at.transform_point(offset),
                        ..parked
                    },
                    Err(_) => parked,
                },
                None => parked,
            },
        };
        let root_world = GlobalTransform::from(world);
        let root_local = root_world.reparented_to(&parent_world);

        let mut clones = HashMap::new();
        // Breadth-first, so a parent's clone exists before its children's.
        let mut queue =
            std::collections::VecDeque::from([(prototype, level_root, root_local, root_world)]);
        while let Some((node, parent, local, global)) = queue.pop_front() {
            let mut entity =
                self.commands
                    .spawn((local, global, NodeActive(active(node).unwrap_or(true))));
            if let Ok(name) = names.get(node) {
                entity.insert((name.clone(), Name::new(name.0.clone())));
            }
            if let Some(parent) = parent {
                entity.insert(ChildOf(parent));
            }
            if let Ok(model) = self.models.get(node) {
                entity.insert(*model);
            }
            if let Ok(lod) = self.lods.get(node) {
                entity.insert(*lod);
            }
            let entity = entity.id();
            clones.insert(node, entity);

            for child in children.get(node).into_iter().flatten() {
                let local = self.transforms.get(*child).copied().unwrap_or_default();
                queue.push_back((*child, Some(entity), local, global.mul_transform(local)));
            }
        }
        clones
    }

    /// A [`ConnectorEnd::frozen`] end as the fixed world point it names now.
    ///
    /// From the local transforms up the parent chain, not `GlobalTransform`:
    /// propagation runs once a frame, and a trail hands on a segment every
    /// fixed tick — two or three a frame at a high rate — each laid where the
    /// last one was re-anchored this tick.
    fn freeze(&self, end: ConnectorEnd) -> ConnectorEnd {
        let Some(node) = end.node.filter(|_| end.frozen) else {
            return end;
        };
        let mut world = self.transforms.get(node).copied().unwrap_or_default();
        let mut cursor = node;
        for _ in 0..64 {
            let Ok(parent) = self.parents.get(cursor).map(ChildOf::parent) else {
                break;
            };
            let local = self.transforms.get(parent).copied().unwrap_or_default();
            world = local.mul_transform(world);
            cursor = parent;
        }
        ConnectorEnd {
            node: None,
            offset: world.transform_point(end.offset),
            frozen: false,
        }
    }

    /// Despawn a finished copy's instance and, if the VM made it, its
    /// subtree; forget the lights it owned.
    fn retire(&mut self, instance: Entity, copy: AnimCopy) {
        self.lights.0.retain(|(owner, _), _| *owner != instance);
        self.commands.entity(instance).despawn();
        if copy.owned {
            self.commands.entity(copy.root).despawn();
        }
    }
}

/// A copy [`start_by_name`] asked for, spawned once the VM's borrows end.
#[derive(Debug, Clone)]
pub struct CopyRequest {
    /// The definition to play.
    pub def: usize,
    /// Its prototype's root, which is what gets cloned.
    pub prototype: Option<Entity>,
    /// An existing subtree to play on instead of a clone.
    pub on: Option<Entity>,
    /// The call, for its placement and inputs.
    pub call: CallAnim,
}

/// Start one of a definition's own sequences by name.
///
/// Restart rather than stack: calling a running sequence again is how the
/// looping trail animations re-seed themselves. Returns false when the
/// definition has no such sequence — `run_sequence`'s authoring slip.
pub fn call_sequence(instance: &mut AnimInstance, def: &AnimDefDesc, name: &str) -> bool {
    let Some(index) = def.sequences.iter().position(|seq| seq.name == name) else {
        return false;
    };
    instance.running.retain(|cursor| cursor.seq != index);
    let t_anim = instance
        .running
        .first()
        .map(|cursor| cursor.t_anim)
        .unwrap_or(0.0);
    instance.running.push(SeqCursor::new(index, t_anim));
    true
}

/// Apply a change to every instance of a named definition.
fn for_each_named(
    name: &str,
    program: &AnimSet,
    instances: &mut Query<(Entity, &mut AnimInstance)>,
    mut apply: impl FnMut(&mut AnimInstance),
) {
    let targets = program.lookup(name);
    if targets.is_empty() {
        return;
    }
    for (_, mut instance) in instances.iter_mut() {
        if targets.contains(&instance.def) {
            apply(&mut instance);
        }
    }
}
