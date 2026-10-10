//! What the anim handlers ask for, carried out.
//!
//! A handler runs inside the VM's stepping loop, which already holds the
//! queries a transform change would need, so it cannot move a node itself.
//! It pushes an [`AnimOp`] instead, and [`apply_anim_ops`] — the next system
//! in [`SimSet::Anim`](crate::schedule::SimSet::Anim) — applies the lot.
//!
//! State that outlives a tick and that several handlers touch in the same
//! tick — running motions, the definitions' own lights, the camera zoom, the
//! screen tint — lives in resources rather than components. A `LightState`
//! followed in the same tick by a `LightAnimation` on the light it just
//! created is common, and a component inserted through `Commands` would not
//! exist yet when the second op arrives.

use std::collections::{HashMap, HashSet};

use avian3d::prelude::{
    AngularVelocity, Collider, ColliderDisabled, LinearVelocity, Position, Rotation,
};
use bevy::prelude::*;

use super::motion::{AnimMotion, AnimMotions};
use super::{AnimProgram, AnimWarnings, PlayerModelRoot};
use crate::cinematic::PlayerControl;
use crate::level::{LevelLight, LevelRoot, NodeActive, WorldEnv};

/// A node's opacity, 0 (invisible) to 1, as `ObjectOpacity*` set it.
///
/// Presentation reads it and fades the node's materials; the sim never does.
/// Absent means "as the model says", which is opaque for all but the
/// already-translucent materials.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct AnimOpacity(pub f32);

/// A node's transform before any anim event moved it.
///
/// Captured the first time an op touches the node. The rate-driven scale of
/// `ObjectMotionFromTo`'s `scale_delta` grows from it, and a connector keeps
/// its authored thickness while it stretches.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct AnimRest(pub Transform);

/// This node's root was moved by a call made *at* a node.
///
/// The effect templates (`lg_impact.flt`, `generic6`, …) are prototypes that
/// `bft-game` hides; one that has been placed somewhere is meant to be seen,
/// so presentation shows it again.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnimPlaced;

/// One definition-owned light, as `LightState`/`LightAnimation` left it.
///
/// **These are not gamez nodes.** All 1204 `LightState` events name an entry
/// of their definition's own `lights` list (`turret_light`,
/// `bftmbrst_light`), so they are keyed by the definition instance and that
/// name — not resolved through `NodeIndex`, and not the level's sun
/// (docs/data/nodes.md). [`LevelLight`] is the colour-and-range shape they share with
/// the model-carried lights.
#[derive(Debug, Clone)]
pub struct AnimLight {
    pub active: bool,
    pub light: LevelLight,
    /// The node it rides on and the offset in that node's frame.
    pub at: Option<(Entity, Vec3)>,
    /// A `LightAnimation` in progress.
    pub animation: Option<LightAnimState>,
}

/// A `LightAnimation` in progress: rates, and how long they still run.
///
/// The event's `range` and `color` are **changes over its `run_time`, not
/// targets**: m1's `vtol_destruction1` animates `turret_light` by
/// `max: +10` and then `max: −30` from a light of range 20.5, which as
/// targets would give a negative range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightAnimState {
    /// (near, far) change per second.
    pub range_rate: Vec2,
    /// Linear RGB change per second.
    pub color_rate: Vec3,
    pub remaining: f32,
}

/// Every definition's lights, by (instance, light name).
#[derive(Resource, Debug, Clone, Default)]
pub struct AnimLights(pub HashMap<(Entity, String), AnimLight>);

/// The camera changes `CameraState`/`CameraFromTo` ask for.
///
/// 22 events in the game, all on `camera1`: a zoom (`bft_00`'s death anim
/// eases to 0.85) and far/near clip changes around the exit animations.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct AnimCamera {
    /// Horizontal and vertical zoom. 1 is the normal view; below 1 is wider.
    pub zoom: Vec2,
    pub clip_near: Option<f32>,
    pub clip_far: Option<f32>,
}

impl Default for AnimCamera {
    fn default() -> AnimCamera {
        AnimCamera {
            zoom: Vec2::ONE,
            clip_near: None,
            clip_far: None,
        }
    }
}

/// The full-screen effects `FbfxColorFromTo` and `FbfxCsinwaveFromTo` drive.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Default)]
pub struct AnimScreenFx {
    /// A colour laid over the whole screen, alpha included. `None` when no
    /// `FbfxColorFromTo` has run, or the last one faded to nothing.
    pub tint: Option<LinearRgba>,
}

/// One end of a connector.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ConnectorEnd {
    /// The node it hangs from; `None` puts [`Self::offset`] in world space.
    pub node: Option<Entity>,
    /// An offset in that node's frame.
    pub offset: Vec3,
    /// Where the node is *now*, not wherever it goes: a
    /// `CallObjectConnector` end with `"pos": true`. The copy the call
    /// starts turns it into a fixed world point, so the fading
    /// segment a trail hands on stays where it was laid.
    pub frozen: bool,
}

/// A mesh stretched between two points: beams, trails, wakes.
///
/// Implemented as the node's model scaled along its own z between the two
/// ends — the model is assumed to be one metre long on z, which is what the
/// `ObjectConnector` trail models (`exhaust_trail`, `wake_trail`) look like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Connector {
    pub node: Entity,
    pub from: ConnectorEnd,
    pub to: ConnectorEnd,
    /// Where along the segment the stretched mesh starts and ends, 0–1.
    pub from_t: f32,
    pub to_t: f32,
    pub max_length: Option<f32>,
}

/// The smallest scale an anim may give a node, per axis.
///
/// Effects shrink to nothing — m1's `xplflare.flt` grows at 80/s and then
/// shrinks at −20/s until it is gone — and an exactly zero scale makes the
/// node's world matrix singular: decomposing it yields a NaN rotation, which
/// spreads into every child's transform and into avian's `Position`, where
/// it panics. A ten-thousandth is as invisible as zero and stays invertible.
pub const MIN_SCALE: f32 = 1e-4;

/// A scale an anim may write: never below [`MIN_SCALE`] on any axis.
///
/// Clamped, not mirrored: a shrink that overshoots zero stays at the floor
/// rather than bouncing back up. The handlers take the magnitude of the
/// authored values before they get here.
pub fn safe_scale(scale: Vec3) -> Vec3 {
    if !scale.is_finite() {
        return Vec3::ONE;
    }
    scale.max(Vec3::splat(MIN_SCALE))
}

/// Whether every part of a transform is a real number.
pub fn is_finite(transform: &Transform) -> bool {
    transform.translation.is_finite()
        && transform.rotation.is_finite()
        && transform.scale.is_finite()
}

/// How a transform op combines with what the node already has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Combine {
    /// Replace the node's local value.
    Set,
    /// Offset from the node's [`AnimRest`] pose.
    FromRest,
    /// Add to (or compose with) the current value.
    Add,
}

/// One change a handler asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum AnimOp {
    Translate {
        node: Entity,
        to: Vec3,
        combine: Combine,
    },
    Rotate {
        node: Entity,
        to: Quat,
        combine: Combine,
    },
    Scale {
        node: Entity,
        to: Vec3,
        combine: Combine,
    },
    /// Put `node` where `at` is, plus `offset` in `at`'s frame, and turn it
    /// the same way when `rotate`.
    PlaceAt {
        node: Entity,
        at: Entity,
        offset: Vec3,
        rotate: bool,
    },
    /// Turn `node` to `at`'s world orientation, then by `rotation`
    /// (`ObjectRotateState` with `AtNodeXYZ`).
    RotateLike {
        node: Entity,
        at: Entity,
        rotation: Quat,
    },
    Opacity {
        node: Entity,
        opacity: Option<f32>,
    },
    /// Start an `ObjectMotion`.
    Motion {
        node: Entity,
        motion: Box<AnimMotion>,
    },
    /// `LightState`: create or change a definition's light. `None` fields
    /// keep what the light had.
    Light {
        owner: Entity,
        name: String,
        active: bool,
        color: Option<Color>,
        range: Option<(f32, f32)>,
        at: Option<(Entity, Vec3)>,
    },
    /// `LightAnimation`: change a light's range and colour over `run_time`.
    LightAnimation {
        owner: Entity,
        name: String,
        range: Vec2,
        color: Vec3,
        run_time: f32,
    },
    /// `FogState`: the level's fog, which `bft-game` puts on the camera.
    Fog {
        color: Color,
        range: Option<(f32, f32)>,
        altitude: Option<(f32, f32)>,
    },
    Camera {
        zoom: Option<Vec2>,
        clip_near: Option<f32>,
        clip_far: Option<f32>,
    },
    ScreenTint(Option<LinearRgba>),
    /// Hang `child` under `parent`, keeping where it is in the world. `None`
    /// detaches it to the level root.
    Reparent {
        child: Entity,
        parent: Option<Entity>,
    },
    Connector(Connector),
}

/// This tick's ops, between the stepper that makes them and
/// [`apply_anim_ops`].
#[derive(Resource, Debug, Default)]
pub struct PendingAnimOps(pub Vec<AnimOp>);

/// Forget the previous level's motions, lights, camera and screen state when
/// a new program arrives, or when the level goes.
pub fn forget_previous_level(
    program: Option<Res<AnimProgram>>,
    mut had_program: Local<bool>,
    mut motions: ResMut<AnimMotions>,
    mut lights: ResMut<AnimLights>,
    mut camera: ResMut<AnimCamera>,
    mut screen: ResMut<AnimScreenFx>,
    mut pending: ResMut<PendingAnimOps>,
) {
    let fresh = program.as_ref().is_some_and(|program| program.is_added());
    let gone = program.is_none() && *had_program;
    *had_program = program.is_some();

    if fresh || gone {
        motions.0.clear();
        lights.0.clear();
        *camera = AnimCamera::default();
        *screen = AnimScreenFx::default();
        pending.0.clear();
    }
}

/// Carry out this tick's ops.
// One parameter per kind of thing an op can change: transforms, the
// hierarchy, the four resources and the level's fog.
#[allow(clippy::too_many_arguments)]
pub fn apply_anim_ops(
    mut commands: Commands,
    mut pending: ResMut<PendingAnimOps>,
    mut transforms: Query<(&mut Transform, Option<&AnimRest>)>,
    globals: Query<&GlobalTransform>,
    parents: Query<&ChildOf>,
    level_roots: Query<Entity, With<LevelRoot>>,
    mut motions: ResMut<AnimMotions>,
    mut lights: ResMut<AnimLights>,
    mut camera: ResMut<AnimCamera>,
    mut screen: ResMut<AnimScreenFx>,
    world_env: Option<ResMut<WorldEnv>>,
    // Whose the player's model is, and the hull's physics state a scripted
    // placement has to overwrite.
    (player_models, control, mut bodies): (
        Query<&PlayerModelRoot>,
        Res<PlayerControl>,
        Query<HullBody, Without<PlayerModelRoot>>,
    ),
    mut warnings: ResMut<AnimWarnings>,
) {
    if pending.0.is_empty() {
        return;
    }
    let mut world_env = world_env;
    // Rest poses captured this pass: the component goes in through
    // `Commands` and would not be visible to the next op on the same node.
    let mut rests: HashMap<Entity, Transform> = HashMap::new();

    let mut rest_of = |node: Entity,
                       current: &Transform,
                       stored: Option<&AnimRest>,
                       commands: &mut Commands|
     -> Transform {
        if let Some(rest) = stored {
            return rest.0;
        }
        *rests.entry(node).or_insert_with(|| {
            commands.entity(node).insert(AnimRest(*current));
            *current
        })
    };

    // Every node's transform before this pass touched it, so a result that
    // is not a real number can be undone rather than handed to physics.
    let mut before: HashMap<Entity, Transform> = HashMap::new();
    let mut remember = |node: Entity, transforms: &Query<(&mut Transform, Option<&AnimRest>)>| {
        if let Ok((transform, _)) = transforms.get(node) {
            before.entry(node).or_insert(*transform);
        }
    };

    for op in pending.0.drain(..) {
        // The player's model rides the hull (`PlayerModelRoot`), so what the
        // scripts do to `bft_00` is done to the hull, or not at all: a
        // relative translate always moves it (elevators, teleports); an
        // absolute placement only during a cinematic, when the scripts own
        // the player — the hand-off puts it in the VTOL's bay.
        let player_model = match &op {
            AnimOp::Translate { node, .. }
            | AnimOp::Rotate { node, .. }
            | AnimOp::Scale { node, .. }
            | AnimOp::PlaceAt { node, .. }
            | AnimOp::RotateLike { node, .. } => player_models.get(*node).ok(),
            _ => None,
        };
        if let Some(model) = player_model {
            let placed = match (&op, control.is_cinematic()) {
                (
                    AnimOp::Translate {
                        to,
                        combine: Combine::Add,
                        ..
                    },
                    _,
                ) => transforms.get_mut(model.hull).ok().map(|(mut hull, _)| {
                    hull.translation += *to;
                    *hull
                }),
                (
                    AnimOp::Translate {
                        to,
                        combine: Combine::Set,
                        ..
                    },
                    true,
                ) => transforms.get_mut(model.hull).ok().map(|(mut hull, _)| {
                    hull.translation = *to;
                    *hull
                }),
                (
                    AnimOp::Rotate {
                        to,
                        combine: Combine::Set,
                        ..
                    },
                    true,
                ) => transforms.get_mut(model.hull).ok().map(|(mut hull, _)| {
                    hull.rotation = *to;
                    *hull
                }),
                _ => None,
            };
            match placed {
                Some(hull) => place_body(&mut bodies, model.hull, &hull, &op),
                None => debug!("anim op on the player's model left to the engine: {op:?}"),
            }
            continue;
        }

        match &op {
            AnimOp::Translate { node, .. }
            | AnimOp::Rotate { node, .. }
            | AnimOp::Scale { node, .. }
            | AnimOp::PlaceAt { node, .. }
            | AnimOp::RotateLike { node, .. } => remember(*node, &transforms),
            AnimOp::Reparent { child, .. } => remember(*child, &transforms),
            AnimOp::Connector(connector) => remember(connector.node, &transforms),
            _ => {}
        }
        match op {
            AnimOp::Translate { node, to, combine } => {
                let Ok((mut transform, stored)) = transforms.get_mut(node) else {
                    continue;
                };
                let rest = rest_of(node, &transform, stored, &mut commands);
                transform.translation = match combine {
                    Combine::Set => to,
                    Combine::FromRest => rest.translation + to,
                    Combine::Add => transform.translation + to,
                };
            }
            AnimOp::Rotate { node, to, combine } => {
                let Ok((mut transform, stored)) = transforms.get_mut(node) else {
                    continue;
                };
                let rest = rest_of(node, &transform, stored, &mut commands);
                transform.rotation = match combine {
                    Combine::Set => to,
                    Combine::FromRest => rest.rotation * to,
                    Combine::Add => transform.rotation * to,
                };
            }
            AnimOp::Scale { node, to, combine } => {
                let Ok((mut transform, stored)) = transforms.get_mut(node) else {
                    continue;
                };
                let rest = rest_of(node, &transform, stored, &mut commands);
                transform.scale = safe_scale(match combine {
                    Combine::Set => to,
                    Combine::FromRest => rest.scale * to,
                    Combine::Add => transform.scale + to,
                });
            }
            AnimOp::PlaceAt {
                node,
                at,
                offset,
                rotate,
            } => {
                let Ok(at_world) = globals.get(at).map(GlobalTransform::compute_transform) else {
                    continue;
                };
                let target = Transform {
                    translation: at_world.transform_point(offset),
                    rotation: at_world.rotation,
                    scale: Vec3::ONE,
                };
                let local = to_local(node, target, &parents, &globals);
                let Ok((mut transform, stored)) = transforms.get_mut(node) else {
                    continue;
                };
                rest_of(node, &transform, stored, &mut commands);
                transform.translation = local.translation;
                if rotate {
                    transform.rotation = local.rotation;
                }
                commands.entity(node).insert(AnimPlaced);
            }
            AnimOp::RotateLike { node, at, rotation } => {
                let Ok(at_world) = globals.get(at).map(GlobalTransform::compute_transform) else {
                    continue;
                };
                let target = Transform::from_rotation(at_world.rotation * rotation);
                let local = to_local(node, target, &parents, &globals);
                let Ok((mut transform, stored)) = transforms.get_mut(node) else {
                    continue;
                };
                rest_of(node, &transform, stored, &mut commands);
                transform.rotation = local.rotation;
            }
            AnimOp::Opacity { node, opacity } => match opacity {
                Some(opacity) => {
                    commands
                        .entity(node)
                        .try_insert(AnimOpacity(opacity.clamp(0.0, 1.0)));
                }
                None => {
                    commands.entity(node).try_remove::<AnimOpacity>();
                }
            },
            AnimOp::Motion { node, motion } => {
                if let Ok((transform, stored)) = transforms.get(node) {
                    rest_of(node, transform, stored, &mut commands);
                }
                motions.0.insert(node, *motion);
            }
            AnimOp::Light {
                owner,
                name,
                active,
                color,
                range,
                at,
            } => {
                let light = lights.0.entry((owner, name)).or_insert_with(|| AnimLight {
                    active,
                    light: LevelLight {
                        color: Color::WHITE,
                        near: 0.0,
                        far: 10.0,
                    },
                    at: None,
                    animation: None,
                });
                light.active = active;
                if let Some(color) = color {
                    light.light.color = color;
                }
                if let Some((near, far)) = range {
                    light.light.near = near;
                    light.light.far = far;
                }
                if at.is_some() {
                    light.at = at;
                }
            }
            AnimOp::LightAnimation {
                owner,
                name,
                range,
                color,
                run_time,
            } => {
                let Some(light) = lights.0.get_mut(&(owner, name.clone())) else {
                    super::warn_once(&mut warnings.unsupported, format!("light:{name}"), || {
                        debug!("LightAnimation on {name}, which no LightState created")
                    });
                    continue;
                };
                // A zero `run_time` is an instant change.
                if run_time <= 0.0 {
                    light.light.near = (light.light.near + range.x).max(0.0);
                    light.light.far = (light.light.far + range.y).max(0.0);
                    light.light.color = add_linear(light.light.color, color);
                    light.animation = None;
                } else {
                    light.animation = Some(LightAnimState {
                        range_rate: range / run_time,
                        color_rate: color / run_time,
                        remaining: run_time,
                    });
                }
            }
            AnimOp::Fog {
                color,
                range,
                altitude,
            } => {
                let Some(env) = world_env.as_mut() else {
                    continue;
                };
                env.fog_color = color;
                if let Some(range) = range {
                    env.fog_range = range;
                }
                if let Some(altitude) = altitude {
                    env.fog_altitude = altitude;
                }
            }
            AnimOp::Camera {
                zoom,
                clip_near,
                clip_far,
            } => {
                if let Some(zoom) = zoom {
                    camera.zoom = zoom;
                }
                if clip_near.is_some() {
                    camera.clip_near = clip_near;
                }
                if clip_far.is_some() {
                    camera.clip_far = clip_far;
                }
            }
            AnimOp::ScreenTint(tint) => {
                screen.tint = tint.filter(|tint| tint.alpha > 0.0);
            }
            AnimOp::Reparent { child, parent } => {
                let parent = parent.or_else(|| level_roots.iter().next());
                let Some(parent) = parent else {
                    continue;
                };
                if parent == child {
                    continue;
                }
                let (Ok(child_world), Ok(parent_world)) = (globals.get(child), globals.get(parent))
                else {
                    continue;
                };
                // Keep the child where it is: its new local transform is its
                // world transform seen from the new parent.
                let local = parent_world.affine().inverse() * child_world.affine();
                let (scale, rotation, translation) = local.to_scale_rotation_translation();
                if let Ok((mut transform, _)) = transforms.get_mut(child) {
                    *transform = Transform {
                        translation,
                        rotation,
                        scale,
                    };
                }
                commands.entity(child).insert(ChildOf(parent));
            }
            AnimOp::Connector(connector) => {
                let end = |end: ConnectorEnd| -> Option<Vec3> {
                    match end.node {
                        Some(node) => globals
                            .get(node)
                            .ok()
                            .map(|world| world.transform_point(end.offset)),
                        None => Some(end.offset),
                    }
                };
                let (Some(a), Some(b)) = (end(connector.from), end(connector.to)) else {
                    continue;
                };
                let start = a.lerp(b, connector.from_t);
                let mut span = a.lerp(b, connector.to_t) - start;
                if let Some(max) = connector.max_length
                    && span.length() > max
                {
                    span = span.normalize_or_zero() * max;
                }
                let length = span.length();
                let rotation = if length > 1e-4 {
                    Quat::from_rotation_arc(Vec3::Z, span / length)
                } else {
                    Quat::IDENTITY
                };
                let target = Transform {
                    translation: start,
                    rotation,
                    scale: Vec3::ONE,
                };
                let local = to_local(connector.node, target, &parents, &globals);
                let Ok((mut transform, stored)) = transforms.get_mut(connector.node) else {
                    continue;
                };
                let rest = rest_of(connector.node, &transform, stored, &mut commands);
                transform.translation = local.translation;
                transform.rotation = local.rotation;
                // Stretch along z only; the other two keep whatever scale the
                // definition set (`ObjectScaleState` `(3, 1, 1)` on a wake).
                transform.scale.z = length.max(1e-3) * rest.scale.z.max(1e-3);
            }
        }
    }

    for (node, original) in before {
        let Ok((mut transform, _)) = transforms.get_mut(node) else {
            continue;
        };
        if !is_finite(&transform) {
            super::warn_once(
                &mut warnings.unsupported,
                format!("nonfinite:{node}"),
                || warn!("an anim op left node {node} with a non-finite transform; undone"),
            );
            *transform = original;
        }
    }
}

/// The physics state of the player's hull that a scripted placement writes.
type HullBody = (
    Option<&'static mut Position>,
    Option<&'static mut Rotation>,
    Option<&'static mut LinearVelocity>,
    Option<&'static mut AngularVelocity>,
);

/// Carry a scripted move of the hull into avian.
///
/// Written straight to `Position`/`Rotation` as well as the transform: a
/// carried hull is a disabled body, and avian does not sync a disabled
/// body's transform, so when the hand-off re-enables it the old position
/// would win. An absolute placement also stops the hull — it is being put
/// somewhere, not thrown there — while a relative one (an elevator) keeps
/// whatever speed it had.
fn place_body(
    bodies: &mut Query<HullBody, Without<PlayerModelRoot>>,
    hull: Entity,
    transform: &Transform,
    op: &AnimOp,
) {
    let Ok((position, rotation, linear, angular)) = bodies.get_mut(hull) else {
        return;
    };
    if let Some(mut position) = position {
        position.0 = transform.translation;
    }
    if let Some(mut rotation) = rotation {
        rotation.0 = transform.rotation;
    }
    let absolute = !matches!(
        op,
        AnimOp::Translate {
            combine: Combine::Add,
            ..
        }
    );
    if absolute {
        if let Some(mut linear) = linear {
            linear.0 = Vec3::ZERO;
        }
        if let Some(mut angular) = angular {
            angular.0 = Vec3::ZERO;
        }
    }
}

/// A world transform expressed in `node`'s parent's frame.
fn to_local(
    node: Entity,
    world: Transform,
    parents: &Query<&ChildOf>,
    globals: &Query<&GlobalTransform>,
) -> Transform {
    let Some(parent_world) = parents
        .get(node)
        .ok()
        .and_then(|child_of| globals.get(child_of.parent()).ok())
    else {
        return world;
    };
    let local = parent_world.affine().inverse() * world.compute_affine();
    let (scale, rotation, translation) = local.to_scale_rotation_translation();
    Transform {
        translation,
        rotation,
        scale,
    }
}

/// A colour plus a linear-RGB change, clamped to the displayable range.
fn add_linear(color: Color, change: Vec3) -> Color {
    let linear = color.to_linear();
    Color::linear_rgb(
        (linear.red + change.x).clamp(0.0, 1.0),
        (linear.green + change.y).clamp(0.0, 1.0),
        (linear.blue + change.z).clamp(0.0, 1.0),
    )
}

/// Step every `LightAnimation` in progress by one fixed tick.
pub fn advance_light_animations(time: Res<Time<Fixed>>, mut lights: ResMut<AnimLights>) {
    let dt = time.delta_secs();
    for light in lights.0.values_mut() {
        let Some(mut animation) = light.animation else {
            continue;
        };
        let step = dt.min(animation.remaining);
        light.light.near = (light.light.near + animation.range_rate.x * step).max(0.0);
        light.light.far = (light.light.far + animation.range_rate.y * step).max(0.0);
        light.light.color = add_linear(light.light.color, animation.color_rate * step);
        animation.remaining -= step;
        light.animation = (animation.remaining > 0.0).then_some(animation);
    }
}

/// Switch a node subtree's colliders off while the node is inactive.
///
/// `active` is visibility **and** collision (docs/data/anims.md): a door whose
/// `locked` model the VM switches off must stop blocking, and a destroyed
/// hulk that has not appeared yet must not stop a tank. Driven by change
/// detection rather than by `NodeActiveChanged`, so the reset states applied
/// silently at load are honoured too.
pub fn sync_node_colliders(
    mut commands: Commands,
    changed: Query<Entity, Changed<NodeActive>>,
    actives: Query<&NodeActive>,
    parents: Query<&ChildOf>,
    children: Query<&Children>,
    colliders: Query<Has<ColliderDisabled>, With<Collider>>,
) {
    let changed: HashSet<Entity> = changed.iter().collect();
    if changed.is_empty() {
        return;
    }

    for &node in &changed {
        // A changed ancestor walks this subtree anyway.
        let mut effective = actives.get(node).is_ok_and(|active| active.0);
        let mut cursor = node;
        let mut covered = false;
        while let Ok(child_of) = parents.get(cursor) {
            cursor = child_of.parent();
            if changed.contains(&cursor) {
                covered = true;
                break;
            }
            if actives.get(cursor).is_ok_and(|active| !active.0) {
                effective = false;
            }
        }
        if covered {
            continue;
        }

        let mut stack = vec![(node, effective)];
        while let Some((entity, parent_on)) = stack.pop() {
            let on = parent_on && actives.get(entity).map_or(true, |active| active.0);
            if let Ok(disabled) = colliders.get(entity) {
                if on && disabled {
                    commands.entity(entity).remove::<ColliderDisabled>();
                } else if !on && !disabled {
                    commands.entity(entity).insert(ColliderDisabled);
                }
            }
            if let Ok(kids) = children.get(entity) {
                stack.extend(kids.iter().map(|kid| (kid, on)));
            }
        }
    }
}
