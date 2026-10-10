//! `ObjectMotion`: velocity, gravity, spin and growth, until it lands or its
//! time runs out.
//!
//! The commonest timed event after the state flips (5219), and the one that
//! throws debris: `generic_6`'s six parts fly on randomised launch angles
//! with `gravity: -9.8`, `run_time: null`, and the very next event switches
//! the part off — so the motion ends when the part hits the ground, and the
//! sequence waits for it. The handler starts a motion here and reports
//! `Running` until [`advance_motions`] has retired it.
//!
//! Motions live in a resource keyed by node rather than as a component,
//! because the handler has to see a motion it started earlier in the same
//! tick, which a component inserted through `Commands` would not let it.

use std::collections::HashMap;

use avian3d::prelude::{SpatialQuery, SpatialQueryFilter};
use bevy::prelude::*;

use super::apply::{is_finite, safe_scale};
use super::{AnimInstance, AnimOutput, AnimProgram};
use crate::level::GameLayer;

/// Which event started a motion, so a stopped sequence stops it too.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MotionOwner {
    pub instance: Entity,
    pub seq: usize,
    pub event: usize,
}

/// How long a falling motion with no `run_time` may take to land.
///
/// Not tuning: a guard for a part launched over a hole in the level, which
/// would otherwise fall for ever and hold its sequence with it. The shipped
/// debris lands within two or three seconds.
const FALL_LIMIT_SECS: f32 = 20.0;

/// One node's `ObjectMotion` in flight.
#[derive(Debug, Clone, PartialEq)]
pub struct AnimMotion {
    pub owner: MotionOwner,
    /// Starting velocity, m/s, in the node's own frame at launch.
    pub velocity: Vec3,
    /// Its change per second, same frame.
    pub acceleration: Vec3,
    /// World-space vertical acceleration; the VM's −9.8, or 0.
    pub gravity: f32,
    /// Whether it stops on the ground (`gravity.no_altitude == false`).
    pub lands: bool,
    /// Rotation rates about the node's own x, y, z, rad/s, and their change.
    pub spin: Vec3,
    pub spin_rate: Vec3,
    /// `forward_rotation`: a tumble about the node's own x, rad/s.
    pub tumble: f32,
    pub tumble_rate: f32,
    /// Scale change per second, and its change.
    pub growth: Vec3,
    pub growth_rate: Vec3,
    /// Seconds it runs; `None` runs until it lands, or for ever when it
    /// cannot (the 46 rotors).
    pub run_time: Option<f32>,
    /// The sequence the owner calls on landing (`bounce_sequence.default`).
    pub bounce_sequence: Option<String>,
    /// The sound played on landing (`bounce_sound.default.name`).
    pub bounce_sound: Option<String>,
    /// Seconds since launch.
    pub age: f32,
    /// World position and velocity once launched; `None` before the first
    /// step, which reads the node's world transform.
    pub world: Option<(Vec3, Vec3)>,
}

impl AnimMotion {
    /// A motion that does nothing, for a handler to fill in.
    pub fn new(owner: MotionOwner) -> AnimMotion {
        AnimMotion {
            owner,
            velocity: Vec3::ZERO,
            acceleration: Vec3::ZERO,
            gravity: 0.0,
            lands: false,
            spin: Vec3::ZERO,
            spin_rate: Vec3::ZERO,
            tumble: 0.0,
            tumble_rate: 0.0,
            growth: Vec3::ZERO,
            growth_rate: Vec3::ZERO,
            run_time: None,
            bounce_sequence: None,
            bounce_sound: None,
            age: 0.0,
            world: None,
        }
    }
}

/// Every motion in flight, by node.
#[derive(Resource, Debug, Clone, Default)]
pub struct AnimMotions(pub HashMap<Entity, AnimMotion>);

impl AnimMotions {
    /// Whether a motion owned by this event is still running on a node.
    pub fn is_running(&self, node: Entity, owner: MotionOwner) -> bool {
        self.0
            .get(&node)
            .is_some_and(|motion| motion.owner == owner)
    }
}

/// How a motion ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MotionEnd {
    /// Its owner stopped, or it could not go on: nothing more happens.
    Stopped,
    /// It hit the ground or a liquid surface.
    Landed,
    /// Its `run_time`, or the fall limit, ran out in the air.
    TimedOut,
}

/// Step every motion by one fixed tick.
// Motion meets the hierarchy (to move a node in world space), the level (to
// land), the VM (to check its owner still wants it, and to call the bounce
// sequence) and the sound layer (the bounce sound).
#[allow(clippy::too_many_arguments)]
pub fn advance_motions(
    time: Res<Time<Fixed>>,
    mut motions: ResMut<AnimMotions>,
    program: Option<Res<AnimProgram>>,
    mut instances: Query<&mut AnimInstance>,
    mut transforms: Query<&mut Transform>,
    globals: Query<&GlobalTransform>,
    parents: Query<&ChildOf>,
    spatial: SpatialQuery,
    liquids: Query<&crate::water::WaterVolume>,
    mut outputs: MessageWriter<AnimOutput>,
) {
    if motions.0.is_empty() {
        return;
    }
    let dt = time.delta_secs();
    let mut finished: Vec<(Entity, MotionEnd)> = Vec::new();

    for (&node, motion) in motions.0.iter_mut() {
        // A motion belongs to the event that started it. If that sequence was
        // stopped — `StopSequence`, `StopAnimation`, `InvalidateAnimation` —
        // the motion stops with it, which is how the rotors stop turning.
        let owner = motion.owner;
        let wanted = owner.seq == usize::MAX
            || instances.get(owner.instance).is_ok_and(|instance| {
                instance
                    .running
                    .iter()
                    .any(|cursor| cursor.seq == owner.seq && cursor.event == owner.event)
            });
        if !wanted {
            finished.push((node, MotionEnd::Stopped));
            continue;
        }

        let Ok(world_now) = globals.get(node).map(GlobalTransform::compute_transform) else {
            finished.push((node, MotionEnd::Stopped));
            continue;
        };
        // A degenerate world matrix decomposes to a NaN rotation; fall back to
        // the node's own frame rather than let it poison the velocity.
        let facing = if world_now.rotation.is_finite() {
            world_now.rotation
        } else {
            Quat::IDENTITY
        };
        let (position, velocity) = *motion.world.get_or_insert_with(|| {
            // Launch: the node's own frame is what the velocity is in.
            (world_now.translation, facing * motion.velocity)
        });
        let acceleration = facing * motion.acceleration + Vec3::Y * motion.gravity;
        // Spin- and growth-only motions (the rotors, `xplflare.flt`) leave the
        // position alone: rewriting it through the parent's inverse would
        // only add error, and nothing there to integrate.
        let translates = motion.gravity != 0.0
            || motion.velocity != Vec3::ZERO
            || motion.acceleration != Vec3::ZERO;

        let velocity = velocity + acceleration * dt;
        let mut next = position + velocity * dt;
        let mut landed = false;

        if motion.lands && velocity.y < 0.0 {
            let step = next - position;
            if let Ok(direction) = Dir3::new(step) {
                let filter =
                    SpatialQueryFilter::from_mask(GameLayer::Level).with_excluded_entities([node]);
                if let Some(hit) =
                    spatial.cast_ray(position, direction, step.length(), true, &filter)
                {
                    next = position + direction * hit.distance;
                    landed = true;
                }
            }
            // A liquid's surface is ground too: the original's altitude
            // surface includes it. Water is not on the level layer, so debris
            // thrown over m1's pier sank, never landed, and its smoke trail
            // — stopped only by the landing's `bounce_sequence` — ran on.
            if !landed
                && let Some(liquid) = crate::water::water_at(&liquids, next)
                && position.y >= liquid.surface_y
            {
                next.y = liquid.surface_y;
                landed = true;
            }
        }
        motion.world = Some((next, velocity));

        let local = world_to_local(node, next, &parents, &globals);
        if let Ok(mut transform) = transforms.get_mut(node) {
            let original = *transform;
            if translates {
                transform.translation = local;
            }
            let spin = Quat::from_euler(
                EulerRot::XYZ,
                motion.spin.x * dt,
                motion.spin.y * dt,
                motion.spin.z * dt,
            ) * Quat::from_rotation_x(motion.tumble * dt);
            transform.rotation = (transform.rotation * spin).normalize();
            transform.scale = safe_scale(transform.scale + motion.growth * dt);
            if !is_finite(&transform) {
                // Undo, and end the motion: it cannot go anywhere sensible.
                *transform = original;
                finished.push((node, MotionEnd::Stopped));
                continue;
            }
        }
        motion.spin += motion.spin_rate * dt;
        motion.tumble += motion.tumble_rate * dt;
        motion.growth += motion.growth_rate * dt;
        motion.age += dt;

        let timed_out = match motion.run_time {
            Some(run_time) => motion.age >= run_time,
            None => motion.lands && motion.age >= FALL_LIMIT_SECS,
        };
        if landed {
            finished.push((node, MotionEnd::Landed));
        } else if timed_out {
            finished.push((node, MotionEnd::TimedOut));
        }
    }

    for (node, end) in finished {
        let Some(motion) = motions.0.remove(&node) else {
            continue;
        };
        if end == MotionEnd::Stopped {
            continue;
        }
        let owner = motion.owner;
        let Ok(mut instance) = instances.get_mut(owner.instance) else {
            continue;
        };
        let Some(def) = program
            .as_ref()
            .and_then(|program| program.0.defs.get(instance.def))
        else {
            continue;
        };
        // A flight that ran out of time without touching anything still
        // ends: the bounce sequence runs, silently. It is what stops a
        // debris part's smoke trail, and nothing else would.
        if let Some(sound) = motion
            .bounce_sound
            .as_ref()
            .filter(|_| end == MotionEnd::Landed)
        {
            outputs.write(AnimOutput {
                kind: "Sound",
                value: serde_json::json!({ "name": sound, "at_node": null }),
                def: def.anim_name.clone(),
                at: instance.run_root(),
                node: Some(node),
            });
        }
        if let Some(sequence) = &motion.bounce_sequence {
            super::call_sequence(&mut instance, def, sequence);
        }
    }
}

/// A world position in `node`'s parent's frame.
fn world_to_local(
    node: Entity,
    world: Vec3,
    parents: &Query<&ChildOf>,
    globals: &Query<&GlobalTransform>,
) -> Vec3 {
    parents
        .get(node)
        .ok()
        .and_then(|child_of| globals.get(child_of.parent()).ok())
        .map(|parent| parent.affine().inverse().transform_point3(world))
        .unwrap_or(world)
}
