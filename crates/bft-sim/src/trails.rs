//! The player's dust and wake trails.
//!
//! `exhaust.zrd` and `boatwake.zrd` define two loops on `bft_00`:
//! `bft_exhaust_trail` and `boat_wake_trail`. Each calls a trail per track
//! (`CallObjectConnector` from `right_morphs` and `left_morphs`), and each
//! trail lays a ribbon behind its track — dust (`exhaust.tif`, 2 m behind,
//! segments of at most 0.5 m) or wake (`turb*.tif`, up to 3 m) — handing
//! every finished segment to a fading copy that is gone about a second
//! later. The data fades and removes them itself; nothing in any script
//! starts or stops either loop, so the engine did, and this module is that
//! part of the engine.
//!
//! // approximation: when each plays is ours. Dust while a track-standing
//! form (track, or amphib on land) drives on the ground; wake while the
//! amphib drives in water; nothing while stopped, airborne, hovering, diving
//! or carried by a cinematic.

use avian3d::prelude::LinearVelocity;
use bevy::prelude::*;

use crate::anim::{CallAnim, StopAnim};
use crate::cinematic::PlayerControl;
use crate::schedule::SimSet;
use crate::vehicle::{GroundState, PlayerVehicle, Vehicle, VehicleForm};

/// The loop on `bft_00` that starts the two dust trails.
pub const DUST_LOOP: &str = "bft_exhaust_trail";
/// The per-track dust trail it calls.
pub const DUST_TRAIL: &str = "bft_exhaust";
/// The loop on `bft_00` that starts the two wakes.
pub const WAKE_LOOP: &str = "boat_wake_trail";
/// The per-track wake it calls.
pub const WAKE_TRAIL: &str = "boat_wake";

/// Above this speed a trail starts, m/s. // approximation
pub const TRAIL_START_SPEED: f32 = 1.5;
/// Below this speed it stops, m/s: lower than the start, so a vehicle
/// creeping at the threshold does not flicker its trail on and off.
/// // approximation
pub const TRAIL_STOP_SPEED: f32 = 0.5;

/// Which trail a vehicle lays.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trail {
    Dust,
    Wake,
}

impl Trail {
    /// The loop to call, and the two names a stop has to reach: the loop
    /// and the per-track trails it started, which loop by themselves.
    fn names(self) -> (&'static str, &'static str) {
        match self {
            Trail::Dust => (DUST_LOOP, DUST_TRAIL),
            Trail::Wake => (WAKE_LOOP, WAKE_TRAIL),
        }
    }
}

/// The trail a vehicle is laying now, if any.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TrailState(pub Option<Trail>);

/// Which trail a vehicle should lay, given the one it lays now.
pub fn wanted_trail(
    current: Option<Trail>,
    form: VehicleForm,
    ground: &GroundState,
    speed: f32,
) -> Option<Trail> {
    let threshold = if current.is_some() {
        TRAIL_STOP_SPEED
    } else {
        TRAIL_START_SPEED
    };
    if speed < threshold {
        return None;
    }
    match form {
        VehicleForm::Amphib if ground.is_in_water() => Some(Trail::Wake),
        VehicleForm::Track | VehicleForm::Amphib if ground.grounded && !ground.is_in_water() => {
            Some(Trail::Dust)
        }
        _ => None,
    }
}

/// Start and stop the player's trails as it drives.
#[allow(clippy::type_complexity)]
pub fn drive_trails(
    mut commands: Commands,
    mut calls: MessageWriter<CallAnim>,
    mut stops: MessageWriter<StopAnim>,
    control: Option<Res<PlayerControl>>,
    mut players: Query<
        (
            Entity,
            &Vehicle,
            &GroundState,
            &LinearVelocity,
            Option<&mut TrailState>,
        ),
        With<PlayerVehicle>,
    >,
) {
    let cinematic = control.is_some_and(|control| control.is_cinematic());
    for (entity, vehicle, ground, velocity, state) in &mut players {
        let current = state.as_deref().and_then(|state| state.0);
        let wanted = if cinematic {
            None
        } else {
            wanted_trail(current, vehicle.form, ground, velocity.0.length())
        };
        if wanted == current {
            continue;
        }
        if let Some(old) = current {
            // The loop and the two trails; the segments already handed on
            // keep fading by themselves.
            let (outer, inner) = old.names();
            for name in [outer, inner] {
                stops.write(StopAnim {
                    name: name.to_string(),
                });
            }
        }
        if let Some(new) = wanted {
            calls.write(CallAnim::new(new.names().0));
        }
        match state {
            Some(mut state) => state.0 = wanted,
            None => {
                commands.entity(entity).insert(TrailState(wanted));
            }
        }
    }
}

/// The trails plugin.
pub struct TrailsPlugin;

impl Plugin for TrailsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, drive_trails.in_set(SimSet::Damage));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn on_land() -> GroundState {
        GroundState {
            grounded: true,
            ..GroundState::airborne()
        }
    }

    #[test]
    fn a_tank_driving_on_land_raises_dust_and_a_stopped_one_does_not() {
        assert_eq!(
            wanted_trail(None, VehicleForm::Track, &on_land(), 5.0),
            Some(Trail::Dust)
        );
        assert_eq!(
            wanted_trail(None, VehicleForm::Track, &on_land(), 0.2),
            None
        );
        assert_eq!(
            wanted_trail(None, VehicleForm::Track, &GroundState::airborne(), 5.0),
            None
        );
        assert_eq!(
            wanted_trail(None, VehicleForm::Hover, &on_land(), 5.0),
            None
        );
    }

    /// Between the two thresholds a trail keeps whatever it was doing.
    #[test]
    fn the_trail_does_not_flicker_at_the_threshold() {
        let between = (TRAIL_START_SPEED + TRAIL_STOP_SPEED) / 2.0;
        assert_eq!(
            wanted_trail(Some(Trail::Dust), VehicleForm::Track, &on_land(), between),
            Some(Trail::Dust)
        );
        assert_eq!(
            wanted_trail(None, VehicleForm::Track, &on_land(), between),
            None
        );
    }
}
