//! The fixed-timestep schedule every simulation subsystem hangs off.

use bevy::prelude::*;

use crate::states::{AppState, GameMode, Paused, sim_running};

/// The simulation tick rate, in hertz.
pub const SIM_HZ: f64 = 60.0;

/// The ordered phases of one simulation tick, chained in `FixedUpdate`.
///
/// Subsystems add their systems to a set rather than ordering against each
/// other, so a subsystem can be written without knowing what its
/// neighbours do.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum SimSet {
    /// Collect `PlayerInput` per controlled entity, local or from the network.
    Input,
    /// Vehicle controller: turn input into forces and velocities.
    Vehicle,
    /// Firing, projectile spawn, ammo.
    Weapons,
    /// The avian3d step, ordered here.
    Physics,
    /// Hit resolution, health, destruction, pickups.
    Damage,
    /// Enemy decision and steering.
    Ai,
    /// The anim event VM: prerequisites, sequences, node transforms.
    Anim,
    /// Objective state, triggers, win and lose conditions.
    Mission,
    /// Host only: snapshot marking for replication.
    Replicate,
}

/// Sets the fixed timestep and the `SimSet` order. Private to `bft-sim`.
pub(crate) struct SchedulePlugin;

impl Plugin for SchedulePlugin {
    fn build(&self, app: &mut App) {
        // `init_state` panics without it, and `MinimalPlugins` — what a
        // headless server and every sim unit test use — does not carry it.
        // `DefaultPlugins` does, hence the check rather than a bare add.
        if !app.is_plugin_added::<bevy::state::app::StatesPlugin>() {
            app.add_plugins(bevy::state::app::StatesPlugin);
        }

        app.init_state::<AppState>()
            .add_sub_state::<GameMode>()
            .init_resource::<Paused>()
            .insert_resource(Time::<Fixed>::from_hz(SIM_HZ))
            .configure_sets(
                FixedUpdate,
                (
                    SimSet::Input,
                    SimSet::Vehicle,
                    SimSet::Weapons,
                    SimSet::Physics,
                    SimSet::Damage,
                    SimSet::Ai,
                    SimSet::Anim,
                    SimSet::Mission,
                    SimSet::Replicate,
                )
                    .chain()
                    // One condition on the whole chain rather than nine
                    // copies: a subsystem joins a set and inherits "only
                    // while playing, and not paused" without asking for it
                    // ([`sim_running`]).
                    .run_if(sim_running),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tick rate has to survive plugin setup: a subsystem that assumes
    /// 60 Hz in its tuning is wrong the moment this silently changes.
    #[test]
    fn fixed_timestep_is_sim_hz() {
        let mut app = App::new();
        app.add_plugins(SchedulePlugin);

        let timestep = app.world().resource::<Time<Fixed>>().timestep();

        // `Duration` has nanosecond resolution, so 1/60 s does not round-trip
        // exactly; a tick within a nanosecond of it is the right one.
        assert!((timestep.as_secs_f64() - 1.0 / SIM_HZ).abs() < 1e-9);
    }
}
