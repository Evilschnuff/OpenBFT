//! The constants our steering needs and the game does not state.
//!
//! **Every field here is an approximation and every one is overridable** in
//! `assets/tuning/ai.ron`. The rule is that a parameter
//! the reader data provides — activation and attack radii, dwell times,
//! pursuit standoff, return range, strategy, detection range, fire interval,
//! ammunition — is read from the data and never appears here. What is here is
//! only what our own implementation invented: a perception cadence, a lead
//! factor, an aim error, three whisker rays, a steering gain.
//!
//! The `// approximation` comment on each says which reader field it was
//! derived from, or that there is none.

use bevy::prelude::*;

/// The steering knobs, as a resource so one file serves every mission.
///
/// Built from `ai.ron` by
/// [`AiOverrides::steering`](super::AiOverrides::steering); the `Default` is
/// what a run with no tuning file gets.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct SteeringTuning {
    /// Ticks between perception passes.
    ///
    /// // approximation: no reader field corresponds. The sim runs at 60 Hz
    /// and a tank does not re-decide who to shoot 60 times a second; six is
    /// a tenth of a second, which is below a human's own reaction and keeps
    /// the cost of 50 enemies' line-of-sight rays to eight a tick.
    pub reaction_ticks: u32,
    /// Perception passes of broken line of sight before the target is
    /// dropped.
    ///
    /// // approximation: `not_pursuit_dwell` is the reader's *seconds* of
    /// lost contact and is used for that; this is the shorter "it ducked
    /// behind a rock" filter that stops the aim snapping off on one frame of
    /// occlusion. Three passes is half a second at the default cadence.
    pub los_lost_ticks: u32,
    /// How much of the computed lead to apply, 0..1.
    ///
    /// // approximation: no reader field corresponds. 1.0 is a perfect
    /// interception solution, which reads as inhuman; 0.85 lands most shots
    /// on a target moving in a straight line and misses one that turns.
    pub lead_factor: f32,
    /// The standard deviation of the aim error, in degrees.
    ///
    /// // approximation: no reader field corresponds. At 300 m — the
    /// commonest `DETECTION_RANGE` — two degrees is ten metres of scatter,
    /// about two vehicle lengths, so a distant enemy is a threat rather than
    /// a sniper.
    pub aim_error_deg: f32,
    /// How close to a waypoint counts as reaching it, in metres.
    ///
    /// // approximation: derived from `path_width`, which is 10 m in every
    /// shipped net and states the corridor's *half-width*. Arriving anywhere
    /// inside the corridor is arriving.
    pub arrive_radius: f32,
    /// How far the obstacle whiskers look ahead, in metres.
    ///
    /// // approximation: no reader field corresponds. An enemy's `rates[0]`
    /// is 12 m/s and a full-lock turn takes about a second at `turns[1]`, so
    /// a whisker has to see about that far to turn out of a wall in time.
    pub whisker_length: f32,
    /// The angle the two side whiskers sit at, in degrees.
    ///
    /// // approximation: wide enough to see a wall being approached
    /// obliquely, narrow enough not to fire at the ground on a slope.
    pub whisker_angle_deg: f32,
    /// How far above the hull origin the whiskers are cast, in metres.
    ///
    /// // approximation: the hull origin is at the track contact patch
    /// (the vehicle's `HULL_FLOOR_Y`), so a whisker from there rakes the
    /// ground. A metre up is inside the hull box and clears a rise the
    /// suspension would climb anyway.
    pub whisker_height: f32,
    /// Radians of heading error that produce full steering lock.
    ///
    /// // approximation: no reader field corresponds. A quarter turn of
    /// error asking for full lock gives a proportional response that settles
    /// rather than weaving.
    pub steer_gain: f32,
    /// Seconds of one leg of a `back_forth` strafe.
    ///
    /// // approximation: `attack_dwell` is the reader's time between attack
    /// *decisions* and is used for that. The strafe's own period has no
    /// field; three seconds crosses about 36 m at an enemy's 12 m/s, which
    /// is a visible sweep across a target at the 60–100 m standoff
    /// `pursuit` states.
    pub strafe_period: f32,
    /// How near the aim has to be before the trigger is pulled, in degrees.
    ///
    /// // approximation: no reader field corresponds. Wider than
    /// [`Self::aim_error_deg`], or a brain would never fire the shot its own
    /// error just spoiled.
    pub fire_cone_deg: f32,
    /// How fast an AI turret traverses, rad/s.
    ///
    /// // approximation: `ai.zrd` states a turret's fire interval and
    /// detection range but nothing about how fast it tracks. Slower than the
    /// player's [`TurretRate`](crate::vehicle::TurretRate) default, so
    /// driving past a turret at speed outruns its gun.
    pub turret_yaw_rate: f32,
    /// How fast an AI turret elevates, rad/s.
    ///
    /// // approximation: as [`Self::turret_yaw_rate`].
    pub turret_pitch_rate: f32,
    /// Heading error beyond which a vehicle reverses instead of driving on,
    /// in degrees.
    ///
    /// // approximation: no reader field corresponds. Past a right angle,
    /// backing up and turning is quicker than driving a circle — which is
    /// what a tracked vehicle with `turns[0]` > 0 does anyway.
    pub reverse_angle_deg: f32,
    /// How near a goal behind the hull has to be for the brain to back up to
    /// it, in metres. A goal further behind is turned to going forwards.
    ///
    /// // approximation: no reader field corresponds. Backing toward a goal
    /// keeps it behind, so with no limit a unit whose target started behind
    /// it reversed the whole way there — m2's minelayer did. A few hull
    /// lengths is a shunt; further than that it turns round at full lock
    /// and drives off forwards.
    pub reverse_max_distance: f32,
    /// Throttle floor while engaging, 0..1.
    ///
    /// // approximation: no reader field corresponds. Stops a brain that has
    /// arrived at its standoff distance sitting at exactly zero throttle and
    /// jittering across the threshold.
    pub min_engage_speed: f32,
    /// How much to scale every net's `activate_rad` by, 1.0 being the data's.
    ///
    /// // approximation: the radius is the data's, but what it is measured
    /// against is not stated — we measure straight-line distance to the
    /// player, with no line of sight. A factor here is the one knob for "they
    /// wake too late" without editing 470 nets through the escape hatch.
    pub activation_scale: f32,
    /// How near the player has to be before a sleeping unit starts walking
    /// its net, in metres. Zero keeps sleeping units parked.
    ///
    /// // approximation: no reader field corresponds. Half of m1's units have
    /// an `activate_rad` of 150 m or less, and a unit that stands still until
    /// then reads as a wreck. Beyond the 325 m weapon range, so anything that
    /// could open fire on the player is already moving when it does — but
    /// it is walking its patrol, not hunting: it only turns on the player
    /// once `activate_rad` or a hit wakes it.
    pub roam_range: f32,
}

impl Default for SteeringTuning {
    fn default() -> SteeringTuning {
        SteeringTuning {
            reaction_ticks: 6,
            los_lost_ticks: 3,
            lead_factor: 0.85,
            aim_error_deg: 2.0,
            arrive_radius: 10.0,
            whisker_length: 18.0,
            whisker_angle_deg: 35.0,
            whisker_height: 1.0,
            steer_gain: 4.0,
            strafe_period: 3.0,
            fire_cone_deg: 8.0,
            turret_yaw_rate: 1.6,
            turret_pitch_rate: 1.0,
            reverse_angle_deg: 110.0,
            reverse_max_distance: 25.0,
            min_engage_speed: 0.15,
            activation_scale: 1.0,
            roam_range: 400.0,
        }
    }
}

impl SteeringTuning {
    /// [`Self::aim_error_deg`] in radians.
    pub fn aim_error(&self) -> f32 {
        self.aim_error_deg.to_radians()
    }

    /// [`Self::fire_cone_deg`] in radians.
    pub fn fire_cone(&self) -> f32 {
        self.fire_cone_deg.to_radians()
    }

    /// [`Self::whisker_angle_deg`] in radians.
    pub fn whisker_angle(&self) -> f32 {
        self.whisker_angle_deg.to_radians()
    }

    /// [`Self::reverse_angle_deg`] in radians.
    pub fn reverse_angle(&self) -> f32 {
        self.reverse_angle_deg.to_radians()
    }

    /// Whether a brain with this phase perceives on this tick.
    ///
    /// The phase is the entity's own, so 50 brains spread their perception
    /// across the cadence rather than all casting on the same tick — which
    /// is the difference between 50 rays on one tick in eight and six on
    /// every tick.
    pub fn perceives(&self, tick: u64, phase: u32) -> bool {
        let cadence = self.reaction_ticks.max(1) as u64;

        tick % cadence == (phase as u64) % cadence
    }
}

/// How far above a hull's origin a brain looks from — and looks *at* — in
/// metres.
///
/// // approximation: no reader field corresponds. The hull origin is at the
/// track contact patch (the vehicle's `HULL_FLOOR_Y`), so a line-of-sight ray
/// from there is cast through the ground on any slope. The turret's own
/// `aimy` is 3 m on the enemies that state one; this is the middle of the
/// hull, which is where an eye would be.
///
/// The ray's far end is lifted by the same amount. Ending at the target's
/// origin, on the ground, a ray grazing the terrain for 200 m would clip the
/// first rise in between: enemies would go blind at range over any ground
/// that was not flat.
pub const EYE_HEIGHT: f32 = 1.5;

#[cfg(test)]
mod tests {
    use super::*;

    /// 50 brains must not all cast their rays on the same tick.
    #[test]
    fn perception_phases_spread_across_the_cadence() {
        let tuning = SteeringTuning {
            reaction_ticks: 6,
            ..default()
        };

        let on_tick_zero = (0..6).filter(|phase| tuning.perceives(0, *phase)).count();
        assert_eq!(on_tick_zero, 1, "one phase in six perceives per tick");

        // And each phase perceives exactly once per cadence.
        for phase in 0..6 {
            let hits = (0..6).filter(|tick| tuning.perceives(*tick, phase)).count();
            assert_eq!(hits, 1, "phase {phase}");
        }
    }

    #[test]
    fn a_cadence_of_one_perceives_every_tick() {
        let tuning = SteeringTuning {
            reaction_ticks: 1,
            ..default()
        };

        assert!((0..10).all(|tick| tuning.perceives(tick, 0)));
    }

    /// A zero cadence reaches here only through a hand-built resource, and it
    /// must not divide by zero.
    #[test]
    fn a_zero_cadence_does_not_divide_by_zero() {
        let tuning = SteeringTuning {
            reaction_ticks: 0,
            ..default()
        };

        assert!(tuning.perceives(7, 0));
    }

    #[test]
    fn the_fire_cone_is_wider_than_the_aim_error() {
        let tuning = SteeringTuning::default();

        assert!(
            tuning.fire_cone() > tuning.aim_error(),
            "or a brain never fires the shot its own error spoiled"
        );
    }
}
