//! The anim event handlers: every presentation kind Recoil's anims use.
//!
//! The VM hands these events over with mech3ax's JSON intact
//! ([`EventDataDesc::Presentation`]); each handler reads the fields it needs,
//! resolves names through [`AnimCtx`] and pushes [`AnimOp`]s. None of them
//! touches the world directly.
//!
//! # Timed events block their sequence
//!
//! A handler for a timed event (`ObjectMotionFromTo`, `ObjectOpacityFromTo`,
//! `LightAnimation`, `FbfxColorFromTo`, …) answers `Running` until its
//! `run_time` is up, and the sequence waits. The data leaves no other
//! reading: `expnuke`'s three untimed `FbfxColorFromTo`s are a fade in, a
//! hold and a fade out, and m1's debris is switched off by the event straight
//! after its fall. Parallel motion is authored as parallel sequences.
//!
//! # Frames and spaces
//!
//! Transforms are in the node's parent frame, like the level's own, and
//! `ObjectMotionFromTo`'s `from`/`to` are **absolute** local values: m1's
//! `dh_door_right` sits at `(20, 2, −185.721)` in `gamez` and its first
//! motion starts from exactly that, while m2's gate `locked` has an identity
//! transform and drops from `(0, 0, 0)` to `(0, −6, 0)`. Euler angles go
//! through [`AxisConvention::GAMEZ_DEFAULT`], the same conversion the level
//! import uses.

use bevy::prelude::*;
use bft_assets::anim::{EventDataDesc, EventDesc, SiScriptDesc};
use bft_assets::axes::AxisConvention;
use serde_json::Value;

use super::apply::{AnimOp, Combine, Connector, ConnectorEnd};
use super::motion::{AnimMotion, MotionOwner};
use super::{
    AnimCtx, AnimOutput, CallAnim, EventHandler, EventHandlers, HandlerResult, INPUT_NODE,
};

/// Register every presentation handler.
pub fn register(registry: &mut EventHandlers) {
    for kind in ["Sound", "SoundNode", "Effect"] {
        registry.insert(Forward {
            kind,
            node_field: None,
        });
    }
    registry.insert(Forward {
        kind: "ObjectCycleTexture",
        node_field: Some("name"),
    });
    registry.insert(AnimVerbose);
    registry.insert(ObjectTranslateState);
    registry.insert(ObjectRotateState);
    registry.insert(ObjectScaleState);
    registry.insert(ObjectOpacityState);
    registry.insert(ObjectOpacityFromTo);
    registry.insert(ObjectMotionFromTo);
    registry.insert(ObjectMotion);
    registry.insert(ObjectMotionSiScript);
    registry.insert(ObjectAddChild);
    registry.insert(ObjectDeleteChild);
    registry.insert(ObjectConnector);
    registry.insert(CallObjectConnector);
    registry.insert(LightState);
    registry.insert(LightAnimation);
    registry.insert(FogState);
    registry.insert(CameraState);
    registry.insert(CameraFromTo);
    registry.insert(FbfxColorFromTo);
    registry.insert(FbfxCsinwaveFromTo);
}

// ---------------------------------------------------------------------------
// Reading mech3ax's JSON
// ---------------------------------------------------------------------------

/// The event's JSON, for a presentation event.
fn payload(event: &EventDesc) -> Option<&Value> {
    match &event.data {
        EventDataDesc::Presentation { value, .. } => Some(value),
        _ => None,
    }
}

fn number(value: &Value, key: &str) -> Option<f32> {
    value.get(key)?.as_f64().map(|f| f as f32)
}

fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}

/// `{x, y, z}` as a vector.
fn vec3(value: &Value) -> Option<Vec3> {
    Some(Vec3::new(
        number(value, "x")?,
        number(value, "y")?,
        number(value, "z")?,
    ))
}

/// `{r, g, b}` as a linear colour.
fn rgb(value: &Value) -> Option<Vec3> {
    Some(Vec3::new(
        number(value, "r")?,
        number(value, "g")?,
        number(value, "b")?,
    ))
}

/// `{min, max}` as a pair.
fn range(value: &Value) -> Option<(f32, f32)> {
    Some((number(value, "min")?, number(value, "max")?))
}

/// `{from, to}` of vectors.
fn from_to(value: &Value) -> Option<(Vec3, Vec3)> {
    Some((vec3(value.get("from")?)?, vec3(value.get("to")?)?))
}

/// `{from, to}` of scalars.
fn from_to_scalar(value: &Value) -> Option<(f32, f32)> {
    Some((number(value, "from")?, number(value, "to")?))
}

/// Euler radians in GameZ's frame, as a rotation in Bevy's.
fn euler(angles: Vec3) -> Quat {
    AxisConvention::GAMEZ_DEFAULT.rotation(angles)
}

/// How far through a timed event this tick is, 0–1, and whether it is over.
///
/// A zero or negative `run_time` is instantaneous: the connectors' `0.0`
/// and the elevator's 0.001 s "motion", which is a teleport.
fn progress(ctx: &AnimCtx, run_time: f32) -> (f32, HandlerResult) {
    if run_time <= 0.0 || ctx.elapsed >= run_time {
        return (1.0, HandlerResult::Done);
    }
    (ctx.elapsed / run_time, HandlerResult::Running)
}

/// A uniform sample from `[min, max]`, drawn from the sim's seeded stream.
fn sample(ctx: &mut AnimCtx, (min, max): (f32, f32)) -> f32 {
    use rand::RngExt;
    min + (max - min) * ctx.rng.0.random::<f32>()
}

/// Write a presentation event to the sound layer unchanged, as the VM does for a
/// kind with no handler.
fn forward(ctx: &mut AnimCtx, kind: &'static str, value: &Value, node: Option<Entity>) {
    ctx.out.presentation.push(AnimOutput {
        kind,
        value: value.clone(),
        def: ctx.def.anim_name.clone(),
        at: ctx.root(),
        node,
    });
}

// ---------------------------------------------------------------------------
// Pass-through: sound, effects, texture cycles
// ---------------------------------------------------------------------------

/// `Sound`, `SoundNode`, `Effect` (`bft-game`'s audio and effects play them) and
/// `ObjectCycleTexture` (`bft-game` rewinds the cycle): passed on as
/// [`AnimOutput`] with their node resolved in scope.
pub struct Forward {
    kind: &'static str,
    /// The field naming the event's node, when it is not `at_node` /
    /// `translate.AtNode`.
    node_field: Option<&'static str>,
}

impl EventHandler for Forward {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let name = match self.node_field {
            Some(field) => string(value, field),
            None => super::presentation_node_name(value),
        };
        let node = name.and_then(|name| ctx.resolve(name));
        forward(ctx, self.kind, value, node);
        HandlerResult::Done
    }
}

/// `AnimVerbose`: one event in the game (m5's `start_clones`), a debug
/// switch for the original's own logging. Logged, nothing else.
pub struct AnimVerbose;

impl EventHandler for AnimVerbose {
    fn kind(&self) -> &'static str {
        "AnimVerbose"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let on = payload(event)
            .and_then(|value| value.get("on"))
            .and_then(Value::as_bool)
            .unwrap_or(false);
        debug!("anim {} asks for verbose logging: {on}", ctx.def.anim_name);
        HandlerResult::Done
    }
}

// ---------------------------------------------------------------------------
// Object state
// ---------------------------------------------------------------------------

/// Place a node: absolute, relative, or at another node (`INPUT_NODE`).
pub struct ObjectTranslateState;

impl EventHandler for ObjectTranslateState {
    fn kind(&self) -> &'static str {
        "ObjectTranslateState"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let Some(state) = value.get("state").and_then(vec3) else {
            return HandlerResult::Done;
        };
        let relative = value
            .get("relative")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let at = string(value, "at_node").and_then(|name| ctx.resolve(name));

        for node in string(value, "node")
            .map(|name| ctx.resolve_all(name))
            .unwrap_or_default()
        {
            let op = match at {
                // 78 of the 430, all `INPUT_NODE`: put an effect where it was
                // called, offset in that node's frame.
                Some(at) => AnimOp::PlaceAt {
                    node,
                    at,
                    offset: state,
                    rotate: false,
                },
                None => AnimOp::Translate {
                    node,
                    to: state,
                    combine: if relative { Combine::Add } else { Combine::Set },
                },
            };
            ctx.out.ops.push(op);
        }
        HandlerResult::Done
    }
}

/// Turn a node to Euler angles, in its parent's frame or another node's.
pub struct ObjectRotateState;

impl EventHandler for ObjectRotateState {
    fn kind(&self) -> &'static str {
        "ObjectRotateState"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let (Some(name), Some(state)) = (string(value, "name"), value.get("state").and_then(vec3))
        else {
            return HandlerResult::Done;
        };
        let rotation = euler(state);
        // `"Absolute"` (181) or `{"AtNodeXYZ": node}` (13, all `INPUT_NODE`).
        let at = value
            .get("basis")
            .and_then(|basis| basis.get("AtNodeXYZ"))
            .and_then(Value::as_str)
            .and_then(|name| ctx.resolve(name));

        for node in ctx.resolve_all(name) {
            ctx.out.ops.push(match at {
                Some(at) => AnimOp::RotateLike { node, at, rotation },
                None => AnimOp::Rotate {
                    node,
                    to: rotation,
                    combine: Combine::Set,
                },
            });
        }
        HandlerResult::Done
    }
}

/// Set a node's scale.
pub struct ObjectScaleState;

impl EventHandler for ObjectScaleState {
    fn kind(&self) -> &'static str {
        "ObjectScaleState"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let (Some(name), Some(state)) = (string(value, "name"), value.get("state").and_then(vec3))
        else {
            return HandlerResult::Done;
        };
        for node in ctx.resolve_all(name) {
            ctx.out.ops.push(AnimOp::Scale {
                node,
                to: AxisConvention::GAMEZ_DEFAULT.point(state).abs(),
                combine: Combine::Set,
            });
        }
        HandlerResult::Done
    }
}

/// Set a node's opacity, and its active state.
///
/// The `state` flag is the node's *active* state, not an "opacity on"
/// switch: `ObjectOpacityFromTo` fades effects from `{1.0, true}` to
/// `{0.0, false}` — visible to gone — and `smoke_destroy.flt` is set
/// `{true, 0.0}` just before it fades in.
pub struct ObjectOpacityState;

impl EventHandler for ObjectOpacityState {
    fn kind(&self) -> &'static str {
        "ObjectOpacityState"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let Some(name) = string(value, "name") else {
            return HandlerResult::Done;
        };
        if let Some(opacity) = number(value, "opacity") {
            for node in ctx.resolve_all(name) {
                ctx.out.ops.push(AnimOp::Opacity {
                    node,
                    opacity: Some(opacity),
                });
            }
        }
        if let Some(state) = value.get("state").and_then(Value::as_bool) {
            ctx.set_active(name, state);
        }
        HandlerResult::Done
    }
}

/// Fade a node's opacity over `run_time`.
///
/// Each end may carry a `state`, the node's active state at that end: shown
/// throughout if either end is, and the end's state once the fade is done.
pub struct ObjectOpacityFromTo;

impl EventHandler for ObjectOpacityFromTo {
    fn kind(&self) -> &'static str {
        "ObjectOpacityFromTo"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let Some(name) = string(value, "name") else {
            return HandlerResult::Done;
        };
        let end = |key: &str| -> (f32, Option<bool>) {
            let end = value.get(key);
            (
                end.and_then(|end| number(end, "opacity")).unwrap_or(1.0),
                end.and_then(|end| end.get("state"))
                    .and_then(Value::as_bool),
            )
        };
        let (from, from_state) = end("opacity_from");
        let (to, to_state) = end("opacity_to");
        let (t, result) = progress(ctx, number(value, "run_time").unwrap_or(0.0));

        // A fade shows the node while it runs if either end is visible:
        // `smoke1` fades in `{0.0, false}` → `{1.0, true}`, and reading the
        // start's `false` literally hid the puff for the whole fade-in, so it
        // popped in at full strength. The end's state lands when it is done.
        if ctx.first
            && let Some(state) = from_state
        {
            ctx.set_active(name, state || to_state == Some(true));
        }
        let opacity = from + (to - from) * t;
        for node in ctx.resolve_all(name) {
            ctx.out.ops.push(AnimOp::Opacity {
                node,
                opacity: Some(opacity),
            });
        }
        if result == HandlerResult::Done
            && let Some(state) = to_state
        {
            ctx.set_active(name, state);
        }
        result
    }
}

// ---------------------------------------------------------------------------
// Motion
// ---------------------------------------------------------------------------

/// Interpolate a node's translation, rotation and scale over `run_time`.
///
/// Doors, gates, elevators, and most of the effect templates' growth. The
/// 373 `morph` channels are not interpolated: the engine keeps no morph
/// targets, so the timing runs and the shape does not change.
pub struct ObjectMotionFromTo;

impl EventHandler for ObjectMotionFromTo {
    fn kind(&self) -> &'static str {
        "ObjectMotionFromTo"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let Some(name) = string(value, "name") else {
            return HandlerResult::Done;
        };
        let (t, result) = progress(ctx, number(value, "run_time").unwrap_or(0.0));
        let translate = value.get("translate").and_then(from_to);
        let rotate = value.get("rotate").and_then(from_to);
        let scale = value.get("scale").and_then(from_to);
        // 43 events grow at a rate instead of between two sizes.
        let scale_delta = value.get("scale_delta").and_then(vec3);
        let elapsed = ctx
            .elapsed
            .min(number(value, "run_time").unwrap_or(0.0).max(0.0));

        for node in ctx.resolve_all(name) {
            if let Some((from, to)) = translate {
                ctx.out.ops.push(AnimOp::Translate {
                    node,
                    to: AxisConvention::GAMEZ_DEFAULT.point(from.lerp(to, t)),
                    combine: Combine::Set,
                });
            }
            if let Some((from, to)) = rotate {
                ctx.out.ops.push(AnimOp::Rotate {
                    node,
                    to: euler(from.lerp(to, t)),
                    combine: Combine::Set,
                });
            }
            if let Some((from, to)) = scale {
                ctx.out.ops.push(AnimOp::Scale {
                    node,
                    to: AxisConvention::GAMEZ_DEFAULT.point(from.lerp(to, t)).abs(),
                    combine: Combine::Set,
                });
            } else if let Some(delta) = scale_delta {
                ctx.out.ops.push(AnimOp::Scale {
                    node,
                    to: Vec3::ONE + delta * elapsed,
                    combine: Combine::FromRest,
                });
            }
        }
        result
    }
}

/// Throw, spin, grow or drop a node until it lands or its time is up.
pub struct ObjectMotion;

impl EventHandler for ObjectMotion {
    fn kind(&self) -> &'static str {
        "ObjectMotion"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let Some(node) = string(value, "node").and_then(|name| ctx.resolve(name)) else {
            return HandlerResult::Done;
        };
        let owner = MotionOwner {
            instance: ctx.instance,
            seq: ctx.seq,
            event: ctx.event,
        };

        if !ctx.first {
            // Running until `advance_motions` retires it: landed, timed out,
            // or its owner stopped.
            return if ctx.motions.is_running(node, owner) {
                HandlerResult::Running
            } else {
                HandlerResult::Done
            };
        }

        let motion = build_motion(ctx, value, owner);
        ctx.out.ops.push(AnimOp::Motion {
            node,
            motion: Box::new(motion),
        });
        // Even a spin with no end (the 46 rotors and radars) holds its
        // sequence: each is the only event of its own sequence, and
        // `StopAnimation` is what ends it.
        HandlerResult::Running
    }
}

/// An [`AnimMotion`] from an `ObjectMotion` event's fields.
///
/// `translation` is a velocity (`initial`, m/s) and its change (`delta`,
/// m/s²) in the node's frame, with `rnd_xz` as a per-axis random spread —
/// pickups pop up at 16 m/s for an eighth of a second, 2 m. The alternative
/// `translation_range` launches at a random heading (`xz`, degrees), climb
/// (`y`, degrees) and speed (`initial`).
fn build_motion(ctx: &mut AnimCtx, value: &Value, owner: MotionOwner) -> AnimMotion {
    let mut motion = AnimMotion::new(owner);

    if let Some(gravity) = value.get("gravity").filter(|g| !g.is_null()) {
        motion.gravity = number(gravity, "value").unwrap_or(0.0);
        motion.lands = !gravity
            .get("no_altitude")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    }

    if let Some(translation) = value.get("translation").filter(|t| !t.is_null()) {
        let mut velocity = translation
            .get("initial")
            .and_then(vec3)
            .unwrap_or_default();
        if let Some(spread) = translation.get("rnd_xz").and_then(vec3) {
            velocity += Vec3::new(
                sample(ctx, (-spread.x.abs(), spread.x.abs())),
                sample(ctx, (-spread.y.abs(), spread.y.abs())),
                sample(ctx, (-spread.z.abs(), spread.z.abs())),
            );
        }
        motion.velocity = velocity;
        motion.acceleration = translation.get("delta").and_then(vec3).unwrap_or_default();
    } else if let Some(launch) = value.get("translation_range").filter(|t| !t.is_null()) {
        let heading =
            sample(ctx, launch.get("xz").and_then(range).unwrap_or_default()).to_radians();
        let climb = sample(ctx, launch.get("y").and_then(range).unwrap_or_default()).to_radians();
        let speed = sample(
            ctx,
            launch.get("initial").and_then(range).unwrap_or_default(),
        );
        let change = sample(ctx, launch.get("delta").and_then(range).unwrap_or_default());
        let direction = Vec3::new(
            climb.cos() * heading.sin(),
            climb.sin(),
            climb.cos() * heading.cos(),
        );
        motion.velocity = direction * speed;
        motion.acceleration = direction * change;
    }

    if let Some(Some(time)) = value.get("forward_rotation").map(|r| r.get("Time")) {
        motion.tumble = number(time, "initial").unwrap_or(0.0);
        motion.tumble_rate = number(time, "delta").unwrap_or(0.0);
    }
    if let Some(spin) = value.get("xyz_rotation").filter(|r| !r.is_null()) {
        motion.spin = spin.get("initial").and_then(vec3).unwrap_or_default();
        motion.spin_rate = spin.get("delta").and_then(vec3).unwrap_or_default();
    }
    if let Some(scale) = value.get("scale").filter(|s| !s.is_null()) {
        motion.growth = scale.get("initial").and_then(vec3).unwrap_or_default();
        motion.growth_rate = scale.get("delta").and_then(vec3).unwrap_or_default();
    }

    motion.run_time = number(value, "run_time");
    motion.bounce_sequence = value
        .get("bounce_sequence")
        .and_then(|b| string(b, "default"))
        .map(str::to_string);
    motion.bounce_sound = value
        .get("bounce_sound")
        .and_then(|b| b.get("default"))
        .and_then(|d| string(d, "name"))
        .map(str::to_string);
    motion
}

/// Play a `.zan` keyframe script on a node: the VTOLs' flights, the bus.
///
/// Linear between frames, as the data says: every script has `spline_interp: false` and frames 0.25 s
/// apart, and translation is `base + delta·t` within a frame exactly.
pub struct ObjectMotionSiScript;

impl EventHandler for ObjectMotionSiScript {
    fn kind(&self) -> &'static str {
        "ObjectMotionSiScript"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let Some(script) = ctx.si_scripts.get(index) else {
            return HandlerResult::Done;
        };
        let duration = script.duration();
        let t = ctx.elapsed.min(duration);
        let (translation, rotation, scale) = si_pose(script, t);

        for node in string(value, "name")
            .map(|name| ctx.resolve_all(name))
            .unwrap_or_default()
        {
            if let Some(to) = translation {
                ctx.out.ops.push(AnimOp::Translate {
                    node,
                    to: AxisConvention::GAMEZ_DEFAULT.point(to),
                    combine: Combine::Set,
                });
            }
            if let Some(to) = rotation {
                ctx.out.ops.push(AnimOp::Rotate {
                    node,
                    to,
                    combine: Combine::Set,
                });
            }
            if let Some(to) = scale {
                ctx.out.ops.push(AnimOp::Scale {
                    node,
                    to: to.abs(),
                    combine: Combine::Set,
                });
            }
        }

        if ctx.elapsed >= duration {
            HandlerResult::Done
        } else {
            HandlerResult::Running
        }
    }
}

/// A script's pose `t` seconds in: each channel from the last frame that
/// carries it.
pub fn si_pose(script: &SiScriptDesc, t: f32) -> (Option<Vec3>, Option<Quat>, Option<Vec3>) {
    let current = script
        .keys
        .iter()
        .rposition(|key| key.start <= t)
        .unwrap_or(0);
    let keys = &script.keys[..=current.min(script.keys.len().saturating_sub(1))];

    let linear = |channel: fn(&bft_assets::anim::SiFrame) -> Option<(Vec3, Vec3)>| {
        keys.iter().rev().find_map(|key| {
            let (base, delta) = channel(key)?;
            // Within its own span only: a channel a later frame dropped holds
            // where its last frame ended rather than extrapolating on.
            let local = (t.min(key.end) - key.start).max(0.0);
            Some(base + delta * local)
        })
    };
    let translation = linear(|key| key.translate);
    let scale = linear(|key| key.scale);

    let rotation = keys
        .iter()
        .enumerate()
        .rev()
        .find_map(|(index, key)| key.rotate.map(|rotation| (index, key.start, rotation)))
        .map(|(index, start, rotation)| {
            // Towards the next frame that carries a rotation.
            match script.keys[index + 1..]
                .iter()
                .find_map(|key| key.rotate.map(|next| (key.start, next)))
            {
                Some((next_start, next)) if next_start > start => {
                    let s = ((t - start) / (next_start - start)).clamp(0.0, 1.0);
                    rotation.slerp(next, s)
                }
                _ => rotation,
            }
        });

    (translation, rotation, scale)
}

// ---------------------------------------------------------------------------
// Hierarchy
// ---------------------------------------------------------------------------

/// Hang a node under another. 103 events, mostly a looping sound node under
/// its source (`snd_fire1` under `burning_model`) — sound nodes are the sound
/// layer's, not gamez nodes, so those are passed on rather than reparented.
pub struct ObjectAddChild;

impl EventHandler for ObjectAddChild {
    fn kind(&self) -> &'static str {
        "ObjectAddChild"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let parent = string(value, "parent").and_then(|name| ctx.resolve(name));
        let child = string(value, "child").and_then(|name| ctx.resolve(name));
        match (parent, child) {
            (Some(parent), Some(child)) => ctx.out.ops.push(AnimOp::Reparent {
                child,
                parent: Some(parent),
            }),
            _ => forward(ctx, "ObjectAddChild", value, parent),
        }
        HandlerResult::Done
    }
}

/// Detach a node, leaving it where it is (13 events, all `redsprks.flt`).
pub struct ObjectDeleteChild;

impl EventHandler for ObjectDeleteChild {
    fn kind(&self) -> &'static str {
        "ObjectDeleteChild"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        if let Some(child) = string(value, "child").and_then(|name| ctx.resolve(name)) {
            ctx.out.ops.push(AnimOp::Reparent {
                child,
                parent: None,
            });
        }
        HandlerResult::Done
    }
}

// ---------------------------------------------------------------------------
// Connectors
// ---------------------------------------------------------------------------

/// One end of an `ObjectConnector`: a node and an offset, or the input ends
/// a `CallObjectConnector` handed over (`"Input"`).
fn connector_end(
    ctx: &AnimCtx,
    value: &Value,
    node_key: &str,
    pos_key: &str,
    from: bool,
) -> ConnectorEnd {
    let pos = value.get(pos_key);
    // `"Input"` for the position, or `INPUT_NODE` for the node with no
    // position: the source form of the pickups' connectors is
    // `FROM_INPUT_NODE`/`TO_INPUT_NODE`, compiled to `INPUT_NODE` at both
    // ends — the two ends the caller handed over, not one node twice.
    let input_node = string(value, node_key) == Some(INPUT_NODE)
        && pos.is_none_or(Value::is_null)
        && ctx.input_ends.is_some();
    if input_node || pos.and_then(Value::as_str) == Some("Input") {
        return match ctx.input_ends {
            Some((a, b)) => {
                if from {
                    a
                } else {
                    b
                }
            }
            None => ConnectorEnd {
                node: ctx.input,
                offset: Vec3::ZERO,
                frozen: false,
            },
        };
    }
    ConnectorEnd {
        node: string(value, node_key).and_then(|name| ctx.resolve(name)),
        offset: pos
            .and_then(|pos| pos.get("Pos"))
            .and_then(vec3)
            .unwrap_or_default(),
        frozen: false,
    }
}

/// A connector parameter: `null`, `{Scalar: t}` or `{Range: {min, max}}`
/// swept over the event's time.
fn connector_t(value: Option<&Value>, default: f32, progress: f32) -> f32 {
    let Some(value) = value.filter(|v| !v.is_null()) else {
        return default;
    };
    if let Some(scalar) = value.get("Scalar").and_then(Value::as_f64) {
        return scalar as f32;
    }
    match value.get("Range").and_then(range) {
        Some((min, max)) => min + (max - min) * progress,
        None => default,
    }
}

/// Stretch a node between two points: trails, wakes, the pickups' beams.
pub struct ObjectConnector;

impl EventHandler for ObjectConnector {
    fn kind(&self) -> &'static str {
        "ObjectConnector"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let Some(node) = string(value, "name").and_then(|name| ctx.resolve(name)) else {
            return HandlerResult::Done;
        };
        let (t, result) = progress(ctx, number(value, "run_time").unwrap_or(0.0));
        let connector = Connector {
            node,
            from: connector_end(ctx, value, "from_node", "from_pos", true),
            to: connector_end(ctx, value, "to_node", "to_pos", false),
            from_t: connector_t(value.get("from_t"), 0.0, t),
            to_t: connector_t(value.get("to_t"), 1.0, t),
            max_length: number(value, "max_length"),
        };
        ctx.out.ops.push(AnimOp::Connector(connector));
        result
    }
}

/// Start a connector animation between two points (the trail segments left
/// behind a missile, a boat, the player's exhaust).
///
/// The called definition sees the two ends as its `"Input"` positions and
/// the `from` node as its `INPUT_NODE`.
pub struct CallObjectConnector;

impl EventHandler for CallObjectConnector {
    fn kind(&self) -> &'static str {
        "CallObjectConnector"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let Some(name) = string(value, "name") else {
            return HandlerResult::Done;
        };
        let end = |ctx: &AnimCtx, node_key: &str, pos_key: &str| ConnectorEnd {
            node: value
                .get(node_key)
                .and_then(|node| string(node, "name"))
                .and_then(|name| ctx.resolve(name)),
            offset: value
                .get(pos_key)
                .and_then(|pos| pos.get("Pos"))
                .and_then(vec3)
                .unwrap_or_default(),
            // `FROM_NODE_POS`/`TO_INPUT_NODE_POS` in the source: the node's
            // position at the call, which the trails' fading segments need
            // to stay put while the vehicle drives on.
            frozen: value
                .get(node_key)
                .and_then(|node| node.get("pos"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        };
        let from = end(ctx, "from_node", "from_pos");
        let to = end(ctx, "to_node", "to_pos");

        let mut call = CallAnim::new(name);
        call.input = from.node;
        call.input_ends = Some((from, to));
        ctx.out.call.push(call);
        HandlerResult::Done
    }
}

// ---------------------------------------------------------------------------
// World: lights and fog
// ---------------------------------------------------------------------------

/// Switch, place and colour one of the definition's own lights.
pub struct LightState;

impl EventHandler for LightState {
    fn kind(&self) -> &'static str {
        "LightState"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let Some(name) = string(value, "name") else {
            return HandlerResult::Done;
        };
        let at = value
            .get("translate")
            .and_then(|translate| translate.get("AtNode"))
            .and_then(|at| {
                let node = ctx.resolve(string(at, "name")?)?;
                Some((node, at.get("pos").and_then(vec3).unwrap_or_default()))
            });
        ctx.out.ops.push(AnimOp::Light {
            owner: ctx.instance,
            name: name.to_string(),
            active: value
                .get("active_state")
                .and_then(Value::as_bool)
                .unwrap_or(true),
            color: value
                .get("color")
                .and_then(rgb)
                .map(|c| Color::linear_rgb(c.x, c.y, c.z)),
            range: value.get("range").and_then(range),
            at,
        });
        HandlerResult::Done
    }
}

/// Change one of the definition's lights' range and colour over time.
pub struct LightAnimation;

impl EventHandler for LightAnimation {
    fn kind(&self) -> &'static str {
        "LightAnimation"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let run_time = number(value, "run_time").unwrap_or(0.0);
        if ctx.first
            && let Some(name) = string(value, "name")
        {
            let (near, far) = value.get("range").and_then(range).unwrap_or_default();
            ctx.out.ops.push(AnimOp::LightAnimation {
                owner: ctx.instance,
                name: name.to_string(),
                range: Vec2::new(near, far),
                color: value.get("color").and_then(rgb).unwrap_or_default(),
                run_time,
            });
        }
        progress(ctx, run_time).1
    }
}

/// Change the level's fog: 19 events, around m2–m6's sky changes and exits.
pub struct FogState;

impl EventHandler for FogState {
    fn kind(&self) -> &'static str {
        "FogState"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let Some(color) = value.get("color").and_then(rgb) else {
            return HandlerResult::Done;
        };
        ctx.out.ops.push(AnimOp::Fog {
            color: Color::linear_rgb(color.x, color.y, color.z),
            range: value.get("range").and_then(range),
            altitude: value.get("altitude").and_then(range),
        });
        HandlerResult::Done
    }
}

// ---------------------------------------------------------------------------
// Presentation cues: camera and screen
// ---------------------------------------------------------------------------

/// Set the camera's zoom and clip planes.
pub struct CameraState;

impl EventHandler for CameraState {
    fn kind(&self) -> &'static str {
        "CameraState"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let zoom = match (number(value, "zoom_h"), number(value, "zoom_v")) {
            (Some(h), Some(v)) => Some(Vec2::new(h, v)),
            (Some(h), None) => Some(Vec2::splat(h)),
            (None, Some(v)) => Some(Vec2::splat(v)),
            (None, None) => None,
        };
        ctx.out.ops.push(AnimOp::Camera {
            zoom,
            clip_near: number(value, "clip_near"),
            clip_far: number(value, "clip_far"),
        });
        HandlerResult::Done
    }
}

/// Ease the camera's zoom over `run_time` (the six death animations).
pub struct CameraFromTo;

impl EventHandler for CameraFromTo {
    fn kind(&self) -> &'static str {
        "CameraFromTo"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let (t, result) = progress(ctx, number(value, "run_time").unwrap_or(0.0));
        let lerp = |key: &str| {
            value
                .get(key)
                .and_then(from_to_scalar)
                .map(|(from, to)| from + (to - from) * t)
        };
        let zoom = match (lerp("zoom_h"), lerp("zoom_v")) {
            (Some(h), Some(v)) => Some(Vec2::new(h, v)),
            (Some(h), None) => Some(Vec2::splat(h)),
            (None, Some(v)) => Some(Vec2::splat(v)),
            (None, None) => None,
        };
        ctx.out.ops.push(AnimOp::Camera {
            zoom,
            clip_near: None,
            clip_far: None,
        });
        result
    }
}

/// Tint the whole screen, fading between two colours (flashes, the nuke).
pub struct FbfxColorFromTo;

impl EventHandler for FbfxColorFromTo {
    fn kind(&self) -> &'static str {
        "FbfxColorFromTo"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let Some(value) = payload(event) else {
            return HandlerResult::Done;
        };
        let rgba = |key: &str| -> Option<Vec4> {
            let color = value.get(key)?;
            Some(rgb(color)?.extend(number(color, "a").unwrap_or(1.0)))
        };
        let (Some(from), Some(to)) = (rgba("from"), rgba("to")) else {
            return HandlerResult::Done;
        };
        let (t, result) = progress(ctx, number(value, "run_time").unwrap_or(0.0));
        let tint = from.lerp(to, t);
        ctx.out.ops.push(AnimOp::ScreenTint(Some(LinearRgba::new(
            tint.x,
            tint.y,
            tint.z,
            tint.w.clamp(0.0, 1.0),
        ))));
        result
    }
}

/// The screen ripple around a node (`csinwave`, 78 events, one template per
/// level). **Simplified to its timing**: the original warps the frame
/// buffer with a sine field, which has no colour to tint with, so the event
/// holds its sequence for its `run_time` and draws nothing.
pub struct FbfxCsinwaveFromTo;

impl EventHandler for FbfxCsinwaveFromTo {
    fn kind(&self) -> &'static str {
        "FbfxCsinwaveFromTo"
    }

    fn run(&self, ctx: &mut AnimCtx, event: &EventDesc) -> HandlerResult {
        let run_time = payload(event)
            .and_then(|value| number(value, "run_time"))
            .unwrap_or(0.0);
        progress(ctx, run_time).1
    }
}
