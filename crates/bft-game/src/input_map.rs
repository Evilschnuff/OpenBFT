//! Keyboard and mouse into the local vehicle's
//! [`PlayerInput`](bft_sim::vehicle::PlayerInput).
//!
//! Runs in `PreUpdate`, every frame, and **accumulates**: mouse movement is
//! summed across however many frames fall inside one 60 Hz tick, and the
//! edge-triggered flags latch until the tick reads them.
//! [`consume_input`](bft_sim::vehicle::input::consume_input) does the
//! clearing on the tick side (DESIGN §4.2).
//!
//! What each button does is [`InputBindings`](crate::ui::InputBindings),
//! the rebindable table, read here through
//! [`ActionInput`](crate::ui::ActionInput). Input is read only while
//! [`gameplay_input_enabled`](crate::ui::gameplay_input_enabled): a click on a
//! pause-menu button must not also fire the gun.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bft_sim::vehicle::{Controlled, PlayerInput, VehicleTable};

use crate::chase_camera::ChaseCamera;
use crate::guided_view::{ExternalGuide, GUIDED_DEAD_ZONE, GuidedMissile, guide_from_cursor};
use crate::ui::{Action, ActionInput};

/// The input map: reads devices, writes the local vehicle's `PlayerInput`.
pub struct InputMapPlugin;

impl Plugin for InputMapPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PreUpdate,
            read_input.run_if(crate::ui::gameplay_input_enabled),
        );
    }
}

/// Fill the locally controlled vehicle's input from the devices.
///
/// While a tether-guided missile is being flown
/// ([`GuidedMissile`](crate::guided_view::GuidedMissile)) the controls are
/// on it: the cursor's offset from the centre steers (or the
/// [`ExternalGuide`], when a plugin supplies one), the launch key lets
/// go, and the tank neither drives nor fires. The trigger stays dead after
/// that until it is released, so the press that let go does not launch the
/// next missile.
// Nine parameters, all ECS injections.
#[allow(clippy::too_many_arguments)]
pub fn read_input(
    actions: ActionInput,
    motion: Res<AccumulatedMouseMotion>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    table: Option<Res<VehicleTable>>,
    guided: Res<GuidedMissile>,
    external_guide: Option<Res<ExternalGuide>>,
    cameras: Query<&ChaseCamera>,
    mut hold_fire: Local<bool>,
    mut inputs: Query<(&mut PlayerInput, &Controlled)>,
) {
    // A window that is not focused must not steer: alt-tabbing away with W
    // held would otherwise drive off a cliff. A plugin pressing actions
    // itself (a headset's controllers) answers for its own devices, so the
    // window's focus then only gates the mouse.
    let focused = window.as_ref().is_none_or(|window| window.focused);
    let listening = focused || actions.has_external();

    // The trigger that let a missile go, or launched one, must come up
    // before it fires again.
    if guided.is_guiding() {
        *hold_fire = true;
    } else if !actions.pressed(Action::Fire) {
        *hold_fire = false;
    }
    let guide = guided
        .is_guiding()
        .then(|| {
            if let Some(external) = &external_guide {
                return Some(external.0.clamp(Vec2::NEG_ONE, Vec2::ONE));
            }
            let window = window.as_ref()?;
            let cursor = window.cursor_position()?;
            let dead_zone = cameras
                .iter()
                .find_map(|camera| camera.overrides.guided.dead_zone)
                .unwrap_or(GUIDED_DEAD_ZONE);
            Some(guide_from_cursor(cursor, window.size(), dead_zone))
        })
        .flatten()
        .unwrap_or(Vec2::ZERO);

    let (sensitivity, invert) = actions
        .bindings()
        .map(|bindings| (bindings.mouse_sensitivity, bindings.invert_pitch))
        .unwrap_or((1.0, false));

    for (mut input, controlled) in &mut inputs {
        if *controlled != Controlled::Local {
            continue;
        }

        if !listening {
            // Release the held states, keep whatever is already latched: a
            // press that happened while focused deserves to be acted on.
            input.throttle = 0.0;
            input.steer = 0.0;
            input.pitch = 0.0;
            input.fire = false;
            input.fire_alt = false;
            input.guide = Vec2::ZERO;
            continue;
        }

        input.guide = guide;
        if guided.is_guiding() {
            // Flying the missile: the tank is left where it is.
            input.throttle = 0.0;
            input.steer = 0.0;
            input.pitch = 0.0;
            input.fire = false;
            input.fire_alt = false;
            if actions.just_pressed(Action::Fire) {
                input.release_guidance = true;
            }
            continue;
        }

        input.throttle = axis(&actions, Action::Forward, Action::Back);
        input.steer = axis(&actions, Action::Right, Action::Left);
        input.pitch = axis(&actions, Action::Climb, Action::Dive);
        input.fire = actions.pressed(Action::Fire) && !*hold_fire;
        input.fire_alt = actions.pressed(Action::AltFire);

        // Accumulate rather than assign: several frames may share one tick.
        if focused {
            input.turret_delta += turret_delta(&motion, sensitivity, invert, table.as_deref());
        }

        if actions.just_pressed(Action::NextForm) {
            input.next_form = true;
        }
        if actions.just_pressed(Action::ToggleStrong) {
            input.toggle_strong = true;
        }
        if actions.just_pressed(Action::Detonate) {
            input.detonate = true;
        }
        for slot in 1..=9 {
            if actions.just_pressed(Action::Slot(slot)) {
                input.select_slot = Some(slot);
            }
        }
    }
}

/// How far the turret should turn for this frame's mouse movement.
///
/// The scale is `player.zrd`'s `mouse_push[0]` — the fine sensitivity — times
/// the user's own multiplier. The coarse value (`mouse_push[1]`, five times
/// larger) is presumably a second sensitivity mode; the settings would be
/// where to pick it.
fn turret_delta(
    motion: &AccumulatedMouseMotion,
    sensitivity: f32,
    invert_pitch: bool,
    table: Option<&VehicleTable>,
) -> Vec2 {
    let push = table
        .map(|table| table.player.camera.mouse_push[0])
        .unwrap_or(0.002)
        * sensitivity;

    // Mouse +x is right, and a right turn is a *negative* yaw about up in a
    // right-handed y-up frame. Mouse +y is down the screen, so aiming up is
    // its negation unless the user inverted it.
    let pitch_sign = if invert_pitch { 1.0 } else { -1.0 };

    Vec2::new(-motion.delta.x * push, pitch_sign * motion.delta.y * push)
}

/// A −1..1 axis from two actions.
fn axis(actions: &ActionInput, positive: Action, negative: Action) -> f32 {
    f32::from(actions.pressed(positive)) - f32::from(actions.pressed(negative))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::InputBindings;

    /// A world with the given keys held and the default bindings, and the
    /// axis read through the real `ActionInput`.
    fn axis_with(keys: &[KeyCode], positive: Action, negative: Action) -> f32 {
        let mut world = World::new();
        let mut input = ButtonInput::<KeyCode>::default();
        for key in keys {
            input.press(*key);
        }
        world.insert_resource(input);
        world.insert_resource(InputBindings::default());
        world
            .run_system_cached_with(
                |In((positive, negative)): In<(Action, Action)>, actions: ActionInput| {
                    axis(&actions, positive, negative)
                },
                (positive, negative),
            )
            .unwrap()
    }

    #[test]
    fn opposite_keys_cancel_and_one_key_is_full_deflection() {
        let (forward, back) = (Action::Forward, Action::Back);

        assert_eq!(axis_with(&[KeyCode::KeyW], forward, back), 1.0);
        assert_eq!(axis_with(&[KeyCode::KeyS], forward, back), -1.0);
        assert_eq!(
            axis_with(&[KeyCode::KeyW, KeyCode::KeyS], forward, back),
            0.0
        );
        assert_eq!(axis_with(&[], forward, back), 0.0);
    }

    /// Moving the mouse right must turn the turret right, which is a
    /// negative yaw in Bevy's frame — the sign that is wrong half the time
    /// when written by eye.
    #[test]
    fn mouse_right_turns_the_turret_right() {
        let motion = AccumulatedMouseMotion {
            delta: Vec2::new(10.0, 0.0),
        };

        let delta = turret_delta(&motion, 1.0, false, None);

        assert!(delta.x < 0.0, "a right turn is a negative yaw: {delta:?}");
    }

    /// Pulling the mouse back (screen −y) aims up by default.
    #[test]
    fn pulling_back_aims_up_unless_inverted() {
        let motion = AccumulatedMouseMotion {
            delta: Vec2::new(0.0, -10.0),
        };

        let normal = turret_delta(&motion, 1.0, false, None);
        assert!(normal.y > 0.0, "should aim up: {normal:?}");

        let inverted = turret_delta(&motion, 1.0, true, None);
        assert!(inverted.y < 0.0, "inverted should aim down: {inverted:?}");
    }

    #[test]
    fn sensitivity_scales_the_delta() {
        let motion = AccumulatedMouseMotion {
            delta: Vec2::new(10.0, 0.0),
        };

        let normal = turret_delta(&motion, 1.0, false, None);
        let doubled = turret_delta(&motion, 2.0, false, None);

        assert!((doubled.x - normal.x * 2.0).abs() < 1e-9);
    }
}
