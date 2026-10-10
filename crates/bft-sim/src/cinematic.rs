//! Mission cinematics: who drives the player, and the engine side of the
//! scripts' camera protocol.
//!
//! Every campaign mission opens with an in-engine cinematic and closes with
//! one. The scripts run them — the VTOL's flight, `camera1`'s path, the fades
//! — and talk to the engine through `Callback` values (docs/data/anims.md,
//! asserted in `tests/real_cinematics.rs`):
//!
//! | Values | Meaning |
//! |---|---|
//! | `2`, `14` | a cinematic starts: scripted camera, input locked |
//! | `11` | the player is carried: parked, not simulated, hidden by the script |
//! | `15`, `10`, `1` | control goes back to the player |
//! | `17` | a script moved the player (elevator, teleport) |
//! | `20` | intro only; meaning unknown, logged |
//! | `99` | the mission is won ([`crate::mission`]) |
//!
//! [`PlayerControl`] is the state those callbacks switch. The anim VM reads
//! it to decide whether the scripts may place and hide `bft_00`
//! ([`PlayerModelRoot`](crate::anim::PlayerModelRoot)), the input system zeroes the player's input while a
//! cinematic runs, and `bft-game` takes the view from `camera1`.
//!
//! # Entry animations
//!
//! The six `mN_start_animation`s keep their *end* state in `reset_state`: the
//! VTOL landed, `bft_00` in the bay, the cargo door open, `Callback 15/10/1`
//! and the calls that start the hover. The VM applies it when the intro
//! finishes or is skipped ([`crate::anim::finish_animation`]), never at load;
//! [`is_entry_animation`] finds them by content.
//!
//! # Carrying the player on a moving support
//!
//! Level geometry is static and the scripts teleport it tick by tick, so a
//! hull standing on m1's trailer would stay put while the trailer drove out
//! from under it. [`carry_on_support`] moves the hull with whatever node it
//! stands on, and hands it the support's velocity when it drives off.

use avian3d::prelude::{AngularVelocity, ColliderDisabled, LinearVelocity, Position, Rotation};
use bevy::prelude::*;
use bft_assets::anim::{AnimDefDesc, AnimSet, EventDataDesc};
use bft_assets::axes::AxisConvention;

use crate::anim::AnimCallback;
use crate::schedule::SimSet;
use crate::vehicle::input::PlayerInput;
use crate::vehicle::{GroundState, PlayerVehicle};

pub use crate::anim::{CONTROL_CALLBACKS, MISSION_END_CALLBACK};

/// The node the scripts mean by "the player".
pub const PLAYER_NODE: &str = "bft_00";

/// `Callback` values that start a cinematic: the first events of every intro
/// and every exit.
pub const ENTER_CINEMATIC_CALLBACKS: [i32; 2] = [2, 14];

/// The `Callback` fired exactly where `bft_00` is hidden: the player is
/// carried, not driven.
pub const CARRIED_CALLBACK: i32 = 11;

/// The `Callback` that follows a script moving `bft_00` by a relative
/// translate: the m3/m6 elevators and the m6/m13 teleports.
pub const PLAYER_MOVED_CALLBACK: i32 = 17;

/// The `Callback` only intros fire. What it means is not in the data
/// ("skippable" and "start the music" both fit); it is logged.
pub const INTRO_CALLBACK: i32 = 20;

/// Who drives the player right now.
#[derive(Resource, Debug, Clone, PartialEq, Eq, Default)]
pub enum PlayerControl {
    /// The player drives; the scripts may not place or hide `bft_00`.
    ///
    /// Only a relative translate reaches the hull, which is how the elevators
    /// and teleports carry the player.
    #[default]
    Driving,
    /// A cinematic runs: input is ignored, the view is `camera1`'s, and the
    /// scripts own `bft_00`.
    Cinematic {
        /// The anim that started it, for skip and for logs.
        anim: String,
        /// Whether `Callback 11` parked the player (hull frozen, model hidden
        /// by the script).
        carried: bool,
    },
}

impl PlayerControl {
    /// Whether a cinematic is running.
    pub fn is_cinematic(&self) -> bool {
        matches!(self, PlayerControl::Cinematic { .. })
    }

    /// Whether the player is parked in a vehicle the scripts fly.
    pub fn is_carried(&self) -> bool {
        matches!(self, PlayerControl::Cinematic { carried: true, .. })
    }

    /// The anim that started the running cinematic.
    pub fn anim(&self) -> Option<&str> {
        match self {
            PlayerControl::Cinematic { anim, .. } => Some(anim),
            PlayerControl::Driving => None,
        }
    }
}

/// Ask for the running entry cinematic to end now, as its last frame would.
///
/// Only an intro — an [entry animation](is_entry_animation) — can be
/// skipped; the exits end the mission and have nothing to skip to.
#[derive(Message, Debug, Clone)]
pub struct SkipCinematic;

/// A script moved the player (`Callback 17`): a view following the hull
/// should snap rather than swing across the map.
#[derive(Message, Debug, Clone, PartialEq, Eq)]
pub struct PlayerTeleported {
    /// The definition that moved it.
    pub by: String,
}

/// A hand-over (`Callback 15/10/1`) fired this tick and waits for the tick's
/// ops to be applied first.
///
/// A start animation's `reset_state` places `bft_00` in the bay and *then*
/// hands over, in one tick. Returning to [`PlayerControl::Driving`] at the
/// callback would drop the placement it follows, so [`settle_control`] makes
/// the switch once [`crate::anim::apply::apply_anim_ops`] has run.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingHandOver(pub Option<String>);

/// What one `Callback` value does to the player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallbackMeaning {
    EnterCinematic,
    Carried,
    HandOver,
    PlayerMoved,
    Intro,
    MissionEnd,
    /// Damage and prop callbacks, none of this module's business.
    Other,
}

/// The meaning of a `Callback` value (see the table in the module docs).
pub fn callback_meaning(value: i32) -> CallbackMeaning {
    match value {
        value if ENTER_CINEMATIC_CALLBACKS.contains(&value) => CallbackMeaning::EnterCinematic,
        CARRIED_CALLBACK => CallbackMeaning::Carried,
        value if CONTROL_CALLBACKS.contains(&value) => CallbackMeaning::HandOver,
        PLAYER_MOVED_CALLBACK => CallbackMeaning::PlayerMoved,
        INTRO_CALLBACK => CallbackMeaning::Intro,
        MISSION_END_CALLBACK => CallbackMeaning::MissionEnd,
        _ => CallbackMeaning::Other,
    }
}

/// Apply the part of a tick's callbacks that must hold *before* its ops.
///
/// Entering a cinematic and being carried take effect at once: m1–m5's exits
/// hide `bft_00` one event *before* `Callback 2`, in the same tick, and that
/// hide has to reach the model. A hand-over only records itself in
/// `pending`, for [`settle_control`] to apply after the ops.
pub fn apply_callbacks<'a>(
    control: &mut PlayerControl,
    pending: &mut PendingHandOver,
    callbacks: impl IntoIterator<Item = &'a (String, i32)>,
) {
    for (def, value) in callbacks {
        match callback_meaning(*value) {
            CallbackMeaning::EnterCinematic => {
                if !control.is_cinematic() {
                    info!("cinematic: {def} takes the view ({value})");
                    *control = PlayerControl::Cinematic {
                        anim: def.clone(),
                        carried: false,
                    };
                }
                // A hand-over earlier in the same tick is superseded.
                pending.0 = None;
            }
            CallbackMeaning::Carried => match control {
                PlayerControl::Cinematic { carried, .. } => *carried = true,
                PlayerControl::Driving => {
                    *control = PlayerControl::Cinematic {
                        anim: def.clone(),
                        carried: true,
                    };
                }
            },
            CallbackMeaning::HandOver => pending.0 = Some(def.clone()),
            CallbackMeaning::Intro => debug!("{def} fired callback 20 (intro; meaning unknown)"),
            CallbackMeaning::PlayerMoved | CallbackMeaning::MissionEnd | CallbackMeaning::Other => {
            }
        }
    }
}

/// Whether a definition is an *entry animation*: one whose `reset_state`
/// carries control flow.
///
/// Exactly the six `mN_start_animation`s in the shipped game
/// (`tests/real_cinematics.rs`). Their `reset_state` is the moment of hand-off, so it is
/// applied when they finish or are skipped, not at load.
pub fn is_entry_animation(def: &AnimDefDesc) -> bool {
    def.reset_state.iter().any(|event| {
        matches!(
            event.data,
            EventDataDesc::Callback(_)
                | EventDataDesc::CallAnimation { .. }
                | EventDataDesc::StopAnimation(_)
                | EventDataDesc::CallSequence(_)
                | EventDataDesc::StopSequence(_)
                | EventDataDesc::InvalidateAnimation(_)
                | EventDataDesc::ResetAnimation(_)
        )
    })
}

/// Whether the running cinematic is an intro that [`SkipCinematic`] can end:
/// the definition that started it is an entry animation.
pub fn is_skippable(control: &PlayerControl, set: &AnimSet) -> bool {
    control.anim().is_some_and(|anim| {
        set.lookup(anim)
            .iter()
            .filter_map(|&index| set.defs.get(index))
            .any(is_entry_animation)
    })
}

/// Where the level's entry animation leaves the player at hand-off: its
/// `reset_state`'s absolute placement of `bft_00`.
///
/// In the original the player *is* `bft_00`, so this is where it starts —
/// in the landed VTOL's bay, tilted to its floor. `None` for a level whose
/// entry animation does not place `bft_00` (m1, where the BFT stands on the
/// trailer at `aiv.zrd`'s start) or that has none.
pub fn hand_off_pose(set: &AnimSet) -> Option<Transform> {
    let def = set.defs.iter().find(|def| is_entry_animation(def))?;

    let mut translation = None;
    let mut rotation = Quat::IDENTITY;
    for event in &def.reset_state {
        let EventDataDesc::Presentation { kind, value } = &event.data else {
            continue;
        };
        match *kind {
            "ObjectTranslateState"
                if value.get("node").and_then(|n| n.as_str()) == Some(PLAYER_NODE)
                    && value.get("relative").and_then(|r| r.as_bool()) != Some(true) =>
            {
                translation = value.get("state").and_then(vec3);
            }
            "ObjectRotateState"
                if value.get("name").and_then(|n| n.as_str()) == Some(PLAYER_NODE)
                    && value.get("basis").and_then(|b| b.as_str()) == Some("Absolute") =>
            {
                if let Some(euler) = value.get("state").and_then(vec3) {
                    rotation = AxisConvention::GAMEZ_DEFAULT.rotation(euler);
                }
            }
            _ => {}
        }
    }

    Some(Transform {
        translation: AxisConvention::GAMEZ_DEFAULT.point(translation?),
        rotation,
        scale: Vec3::ONE,
    })
}

fn vec3(value: &serde_json::Value) -> Option<Vec3> {
    let n = |key: &str| value.get(key).and_then(|v| v.as_f64()).map(|v| v as f32);
    Some(Vec3::new(n("x")?, n("y")?, n("z")?))
}

/// The cinematic plugin: the control state and what follows from it.
pub struct CinematicPlugin;

impl Plugin for CinematicPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerControl>()
            .init_resource::<PendingHandOver>()
            .init_resource::<SupportCarry>()
            .add_message::<SkipCinematic>()
            .add_message::<PlayerTeleported>()
            // After the input layer wrote this tick's input and before the
            // vehicle reads it.
            .add_systems(FixedUpdate, lock_player_input.in_set(SimSet::Input))
            .add_systems(FixedUpdate, hold_parked_player.in_set(SimSet::Damage))
            .add_systems(
                FixedUpdate,
                (
                    settle_control,
                    park_carried_player,
                    carry_on_support,
                    announce_player_moves,
                )
                    .chain()
                    .after(crate::anim::apply::apply_anim_ops)
                    .after(crate::anim::motion::advance_motions)
                    .in_set(SimSet::Anim),
            )
            .add_systems(OnExit(crate::states::AppState::InGame), reset_control);
    }
}

/// Apply a hand-over recorded this tick, now that the tick's ops are in.
pub fn settle_control(mut control: ResMut<PlayerControl>, mut pending: ResMut<PendingHandOver>) {
    let Some(def) = pending.0.take() else {
        return;
    };
    if control.is_cinematic() {
        info!("cinematic: {def} hands the player control");
        *control = PlayerControl::Driving;
    }
}

/// Zero the player's input while a cinematic runs.
///
/// The frame layer keeps writing it — `bft-game` also stops reading the keys,
/// but the sim must not trust that: a replay, a network client or a test
/// drives this component directly.
pub fn lock_player_input(
    control: Res<PlayerControl>,
    mut inputs: Query<&mut PlayerInput, With<PlayerVehicle>>,
) {
    if !control.is_cinematic() {
        return;
    }
    for mut input in &mut inputs {
        if !input.is_idle() {
            *input = PlayerInput::default();
        }
    }
}

/// A carried player's hull, held where the scripts last put it.
///
/// Not `RigidBodyDisabled`: toggling that on a body with touching contacts
/// trips avian 0.7's island bookkeeping (`contact.island.is_none()`), and a
/// hull resting in a bay always has some. The body stays enabled; its
/// collider is off, so the VTOL arriving round it and shots at the hidden
/// model pass through, and [`hold_parked_player`] puts it back after every
/// physics step.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Parked(pub Transform);

/// Park the hull while the player is carried, and release it on hand-off.
///
/// A carried player is not simulated: the scripts fly the vehicle it sits in
/// and hide its model. Its hull would otherwise fall out of the sky under the
/// VTOL for the whole fly-in. The parked pose is taken every tick, after the
/// scripts' ops, so a placement made while carried is where it is held.
pub fn park_carried_player(
    mut commands: Commands,
    control: Res<PlayerControl>,
    mut hulls: Query<(Entity, &Transform, Option<&mut Parked>), With<PlayerVehicle>>,
    mut velocities: Query<(&mut LinearVelocity, &mut AngularVelocity)>,
) {
    let carried = control.is_carried();
    for (hull, transform, parked) in &mut hulls {
        match (carried, parked) {
            (true, Some(mut parked)) => parked.0 = *transform,
            (true, None) => {
                commands
                    .entity(hull)
                    .insert((Parked(*transform), ColliderDisabled));
            }
            (false, Some(_)) => {
                commands.entity(hull).remove::<(Parked, ColliderDisabled)>();
                // Handed over at rest, wherever the reset put it.
                if let Ok((mut linear, mut angular)) = velocities.get_mut(hull) {
                    linear.0 = Vec3::ZERO;
                    angular.0 = Vec3::ZERO;
                }
            }
            (false, None) => {}
        }
    }
}

/// A hull's pose and motion, as avian and the transform both hold them.
type HullPhysics = (
    &'static mut Transform,
    Option<&'static mut Position>,
    Option<&'static mut Rotation>,
    Option<&'static mut LinearVelocity>,
    Option<&'static mut AngularVelocity>,
);

/// Put a parked hull back where it was parked, stopped.
///
/// After the physics step and before anything reads the hull's place, so
/// the suspension's forces and gravity from this tick's step are undone.
pub fn hold_parked_player(mut hulls: Query<(&Parked, HullPhysics), With<PlayerVehicle>>) {
    for (parked, (mut transform, position, rotation, linear, angular)) in &mut hulls {
        *transform = parked.0;
        if let Some(mut position) = position {
            position.0 = parked.0.translation;
        }
        if let Some(mut rotation) = rotation {
            rotation.0 = parked.0.rotation;
        }
        if let Some(mut linear) = linear {
            linear.0 = Vec3::ZERO;
        }
        if let Some(mut angular) = angular {
            angular.0 = Vec3::ZERO;
        }
    }
}

/// Tell presentation that a script moved the player (`Callback 17`).
pub fn announce_player_moves(
    mut callbacks: MessageReader<AnimCallback>,
    mut moved: MessageWriter<PlayerTeleported>,
) {
    for callback in callbacks.read() {
        if callback_meaning(callback.value) == CallbackMeaning::PlayerMoved {
            moved.write(PlayerTeleported {
                by: callback.def.clone(),
            });
        }
    }
}

/// Leaving the game must not leave the next level mid-cinematic.
fn reset_control(mut control: ResMut<PlayerControl>, mut pending: ResMut<PendingHandOver>) {
    *control = PlayerControl::Driving;
    pending.0 = None;
}

/// What [`carry_on_support`] remembers between ticks: the node under the
/// hull and where it was, and how far it moved last tick.
#[derive(Resource, Debug, Clone, Default)]
pub struct SupportCarry {
    last: Option<(Entity, Transform)>,
    /// The support's last displacement, per second.
    velocity: Vec3,
}

/// The largest step a support may take in one tick and still carry the
/// player. m1's trailer moves 0.14 m a tick; the elevators and teleports jump
/// tens to thousands of metres and move `bft_00` themselves (`Callback 17`).
pub const MAX_CARRY_STEP: f32 = 2.0;

/// Move the player's hull with the node it stands on.
///
/// Level geometry is a static body that the scripts move by rewriting its
/// transform, which avian treats as a teleport with no velocity: m1's
/// trailer, driving 8.5 m/s with the BFT chained on it, would slide out from
/// under the hull. So the hull follows the support's displacement each tick,
/// and takes its velocity with it when it drives off.
///
/// Runs at the end of [`SimSet::Anim`], after the scripts moved the support
/// this tick. `GroundState::support` is this tick's, sensed before they did.
// Five parameters: the time step, the memory, the hull, the support's
// hierarchy, and the state that switches a parked hull off.
pub fn carry_on_support(
    time: Res<Time<Fixed>>,
    mut carry: ResMut<SupportCarry>,
    control: Res<PlayerControl>,
    mut hulls: Query<(&GroundState, HullPhysics), With<PlayerVehicle>>,
    nodes: Query<(&Transform, Option<&ChildOf>), Without<PlayerVehicle>>,
) {
    let Some((ground, (mut hull, position, rotation, velocity, _))) = hulls.iter_mut().next()
    else {
        carry.last = None;
        return;
    };
    let dt = time.delta_secs().max(1e-6);

    let support = if control.is_carried() {
        None
    } else {
        ground.support
    };
    let now = support.and_then(|entity| world_transform(entity, &nodes).map(|t| (entity, t)));

    match (carry.last, now) {
        (Some((before, was)), Some((entity, is))) if before == entity => {
            let step = is.translation - was.translation;
            if is != was && step.length() <= MAX_CARRY_STEP {
                // The support's motion as one rigid transform, applied to the
                // hull: a turning trailer turns the hull with it.
                let turn = is.rotation * was.rotation.inverse();
                let pivot = was.translation;
                hull.translation = is.translation + turn * (hull.translation - pivot);
                hull.rotation = (turn * hull.rotation).normalize();
                if let Some(mut position) = position {
                    position.0 = hull.translation;
                }
                if let Some(mut rotation) = rotation {
                    rotation.0 = hull.rotation;
                }
            }
            carry.velocity = if step.length() <= MAX_CARRY_STEP {
                step / dt
            } else {
                Vec3::ZERO
            };
        }
        (Some(_), _) => {
            // Off the support (or onto another): keep going at its speed, as
            // anything leaving a moving bed does.
            if let Some(mut velocity) = velocity {
                velocity.0 += carry.velocity;
            }
            carry.velocity = Vec3::ZERO;
        }
        (None, _) => carry.velocity = Vec3::ZERO,
    }
    carry.last = now;
}

/// A node's world transform, composed up its hierarchy.
///
/// `GlobalTransform` is only propagated once a frame and the fixed tick can
/// run several times in one, so it would lag a script-moved node.
fn world_transform(
    entity: Entity,
    nodes: &Query<(&Transform, Option<&ChildOf>), Without<PlayerVehicle>>,
) -> Option<Transform> {
    let (local, parent) = nodes.get(entity).ok()?;
    let mut world = *local;
    let mut cursor = parent.map(ChildOf::parent);
    let mut depth = 0;
    while let Some(entity) = cursor {
        let Ok((local, parent)) = nodes.get(entity) else {
            break;
        };
        world = local.mul_transform(world);
        cursor = parent.map(ChildOf::parent);
        depth += 1;
        if depth > 64 {
            break;
        }
    }
    Some(world)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::anim::{EventDesc, EventStart};

    fn event(data: EventDataDesc) -> EventDesc {
        EventDesc {
            start: EventStart::WithPrevious,
            data,
        }
    }

    #[test]
    fn the_callback_groups_mean_what_the_findings_say() {
        assert_eq!(callback_meaning(2), CallbackMeaning::EnterCinematic);
        assert_eq!(callback_meaning(14), CallbackMeaning::EnterCinematic);
        assert_eq!(callback_meaning(11), CallbackMeaning::Carried);
        for value in [15, 10, 1] {
            assert_eq!(callback_meaning(value), CallbackMeaning::HandOver);
        }
        assert_eq!(callback_meaning(17), CallbackMeaning::PlayerMoved);
        assert_eq!(callback_meaning(20), CallbackMeaning::Intro);
        assert_eq!(callback_meaning(99), CallbackMeaning::MissionEnd);
        assert_eq!(callback_meaning(25), CallbackMeaning::Other);
        assert_eq!(callback_meaning(911), CallbackMeaning::Other);
    }

    /// An exit hides `bft_00`, then fires `2, 14, 11`; a reset places it and
    /// then fires `10, 1, 15`. Entering holds at once, handing over waits.
    #[test]
    fn entering_holds_at_once_and_handing_over_waits_for_the_ops() {
        let mut control = PlayerControl::Driving;
        let mut pending = PendingHandOver::default();
        let exit = |value| ("m2_exit_animation".to_string(), value);

        apply_callbacks(&mut control, &mut pending, &[exit(2), exit(14), exit(11)]);
        assert_eq!(
            control,
            PlayerControl::Cinematic {
                anim: "m2_exit_animation".into(),
                carried: true
            }
        );

        let reset = |value| ("m2_start_animation".to_string(), value);
        apply_callbacks(
            &mut control,
            &mut pending,
            &[reset(10), reset(1), reset(15)],
        );
        assert!(control.is_cinematic(), "not until the ops are applied");
        assert_eq!(pending.0.as_deref(), Some("m2_start_animation"));
    }

    #[test]
    fn a_reset_with_control_flow_is_an_entry_animation() {
        let mut def = AnimDefDesc {
            reset_state: vec![event(EventDataDesc::ObjectActiveState {
                node: "healthy".into(),
                state: true,
            })],
            ..Default::default()
        };
        assert!(!is_entry_animation(&def), "plain initial state");

        def.reset_state.push(event(EventDataDesc::Callback(15)));
        assert!(is_entry_animation(&def));
    }

    #[test]
    fn the_hand_off_pose_is_bft_00s_in_the_entry_reset() {
        let mut def = AnimDefDesc {
            anim_name: "m2_start_animation".into(),
            ..Default::default()
        };
        def.reset_state = vec![
            event(EventDataDesc::Presentation {
                kind: "ObjectTranslateState",
                value: serde_json::json!({ "node": "bft_00", "relative": false,
                    "state": { "x": 1697.0, "y": 36.6, "z": 656.5 }, "at_node": null }),
            }),
            event(EventDataDesc::Presentation {
                kind: "ObjectRotateState",
                value: serde_json::json!({ "name": "bft_00", "basis": "Absolute",
                    "state": { "x": 0.0, "y": -1.5707964, "z": 0.0 } }),
            }),
            event(EventDataDesc::Callback(15)),
        ];
        let set = AnimSet {
            defs: vec![def],
            ..Default::default()
        };

        let pose = hand_off_pose(&set).expect("placed");
        assert_eq!(pose.translation, Vec3::new(1697.0, 36.6, 656.5));
        let facing = pose.rotation * Vec3::NEG_Z;
        assert!((facing - Vec3::X).length() < 1e-5, "yaw −90°: {facing}");
    }

    #[test]
    fn no_entry_placement_means_no_hand_off_pose() {
        let def = AnimDefDesc {
            reset_state: vec![event(EventDataDesc::Callback(15))],
            ..Default::default()
        };
        let set = AnimSet {
            defs: vec![def],
            ..Default::default()
        };
        assert_eq!(hand_off_pose(&set), None, "m1's shape");
    }
}
