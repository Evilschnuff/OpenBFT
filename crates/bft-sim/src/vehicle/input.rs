//! What a vehicle is being told to do this tick, and who is telling it.
//!
//! [`PlayerInput`] is the whole interface between a controller — a keyboard,
//! the network, an AI brain — and the vehicle. It is a component, not a
//! resource, because every vehicle has one: the local player's is filled by
//! `bft_game::input_map`, a remote player's by the network (once there is
//! one), and an enemy's by the AI.
//!
//! **It is accumulated in `PreUpdate` and consumed in `SimSet::Input`.** The
//! frame rate and the tick rate differ, so mouse movement is summed across
//! however many frames fall in a tick and the edge-triggered flags (`fire`,
//! `next_form`) latch until the tick reads them. [`consume_input`] does the
//! reset; nothing else may clear the component, or an input would be eaten
//! before the vehicle saw it.

use bevy::prelude::*;

use crate::schedule::SimSet;

/// What a vehicle is being asked to do this tick.
///
/// `Serialize`/`Deserialize` so it can be replicated: a client sends
/// this and the host steps the same simulation.
#[derive(
    Component, Clone, Copy, Default, PartialEq, Debug, serde::Serialize, serde::Deserialize,
)]
pub struct PlayerInput {
    /// Forward and back, −1..1.
    pub throttle: f32,
    /// Left and right, −1..1.
    pub steer: f32,
    /// Turret yaw and pitch change this tick, in radians. Summed over the
    /// frames since the last tick.
    ///
    /// Only the fallback now: when [`aim_point`](Self::aim_point) is set the
    /// turret slews to *it* and this is ignored. Kept because an AI brain and
    /// a gamepad both want to push the turret by a rate rather than name a
    /// world point.
    pub turret_delta: Vec2,
    /// Where the player is pointing, in world space.
    ///
    /// The cursor's ray cast into the level (`bft_game::aim`'s `aim_point`
    /// system). `None` when nothing was under the cursor, or before the
    /// first frame has run, and the turret then falls back to
    /// [`turret_delta`](Self::turret_delta).
    pub aim_point: Option<Vec3>,
    /// Sub dive and climb, −1..1.
    pub pitch: f32,
    /// Primary fire, held.
    pub fire: bool,
    /// Alternate fire, held: the pulse gun, whatever is selected.
    pub fire_alt: bool,
    /// Select a weapon slot, 1–9. Latched until a tick reads it.
    pub select_slot: Option<u8>,
    /// Toggle the strong variant of the selected weapon. Latched.
    pub toggle_strong: bool,
    /// Cycle to the next unlocked form. Latched.
    pub next_form: bool,
    /// Set off every remote mine this vehicle has laid. Latched.
    pub detonate: bool,
    /// How hard a tether-guided missile is being steered, −1..1 on each
    /// axis: `x` right, `y` up. Zero when nothing is being guided.
    ///
    /// A state, like `throttle`: the frame layer writes where the cursor
    /// sits relative to the window's centre, and the missile turns at that
    /// fraction of its `TURN_RATE` and `PITCH_RATE` (`Mover::Tethered`).
    pub guide: Vec2,
    /// Let go of the missile being guided, which flies on straight. Latched.
    pub release_guidance: bool,
}

impl PlayerInput {
    /// Clear the edge-triggered fields, keeping the held ones.
    ///
    /// `throttle`, `steer`, `pitch`, `fire`, `fire_alt` and `guide` describe a
    /// *state* the frame layer rewrites every frame; `turret_delta` and the
    /// five latched flags describe *events* since the last tick, and clearing them
    /// is what stops one keypress switching forms on every tick it is held.
    pub fn consume(&mut self) {
        self.turret_delta = Vec2::ZERO;
        self.select_slot = None;
        self.toggle_strong = false;
        self.next_form = false;
        self.detonate = false;
        self.release_guidance = false;
    }

    /// Whether anything is being asked for, for a cheap "is the player
    /// steering" check.
    pub fn is_idle(&self) -> bool {
        *self == PlayerInput::default()
    }
}

/// Who controls a vehicle.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Controlled {
    /// This client's player.
    Local,
    /// A remote player, by connection id.
    Net(u64),
    /// An AI brain.
    Ai,
}

/// Clear the edge-triggered inputs once the tick has read them.
///
/// Registered at the **end** of [`SimSet::Vehicle`] rather than in
/// [`SimSet::Input`]: weapons (`SimSet::Weapons`) read `fire` and
/// `select_slot` after the vehicle does, so a reset in `Input` would clear
/// them before the shot was taken.
pub fn consume_input(mut inputs: Query<&mut PlayerInput>) {
    for mut input in &mut inputs {
        // `bypass_change_detection`: every vehicle's input is cleared every
        // tick, so change detection on this component would report a change
        // on every entity on every tick and tell a reader nothing.
        input.bypass_change_detection().consume();
    }
}

/// The input plugin: the reset, and nothing else.
///
/// Reading a keyboard is `bft-game`'s job (`input_map`); this crate has no
/// window and a dedicated server has no keyboard at all.
pub struct InputPlugin;

impl Plugin for InputPlugin {
    fn build(&self, app: &mut App) {
        // After `SimSet::Weapons`, which is the last set that reads an input.
        app.add_systems(FixedUpdate, consume_input.after(SimSet::Weapons));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consuming_clears_the_events_and_keeps_the_states() {
        let mut input = PlayerInput {
            throttle: 1.0,
            steer: -0.5,
            turret_delta: Vec2::new(0.1, 0.2),
            aim_point: Some(Vec3::new(1.0, 2.0, 3.0)),
            pitch: 1.0,
            fire: true,
            fire_alt: true,
            select_slot: Some(3),
            toggle_strong: true,
            next_form: true,
            detonate: true,
            guide: Vec2::new(0.5, -0.5),
            release_guidance: true,
        };

        input.consume();

        // Held states survive: the frame layer owns them.
        assert_eq!(input.throttle, 1.0);
        assert_eq!(input.guide, Vec2::new(0.5, -0.5));
        assert!(!input.release_guidance);
        assert_eq!(input.steer, -0.5);
        assert_eq!(input.pitch, 1.0);
        assert!(input.fire);
        assert!(input.fire_alt);
        // The cursor's world point is a state too: it is where the player is
        // still pointing, not a thing that happened once.
        assert_eq!(input.aim_point, Some(Vec3::new(1.0, 2.0, 3.0)));
        // Events do not: one press is one action.
        assert_eq!(input.turret_delta, Vec2::ZERO);
        assert_eq!(input.select_slot, None);
        assert!(!input.toggle_strong);
        assert!(!input.next_form);
        assert!(!input.detonate);
    }

    /// A held `F` must not switch form on all 60 ticks a second.
    #[test]
    fn a_latched_flag_fires_once_per_press() {
        let mut input = PlayerInput {
            next_form: true,
            ..default()
        };

        assert!(input.next_form);
        input.consume();
        assert!(!input.next_form, "the second tick must not see it");
    }

    #[test]
    fn a_default_input_is_idle() {
        assert!(PlayerInput::default().is_idle());
        assert!(
            !PlayerInput {
                throttle: 0.1,
                ..default()
            }
            .is_idle()
        );
    }
}
