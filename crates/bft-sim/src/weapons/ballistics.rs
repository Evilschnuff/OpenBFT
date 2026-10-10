//! Solving for the launch angle an arcing weapon needs to hit a point.
//!
//! [`fire_weapons`](super::firing::fire_weapons) launches along the turret's
//! aim direction unmodified, which is right for a `Straight` round and wrong
//! for a `Ballistic` one: [`move_projectiles`](super::firing::move_projectiles)
//! subtracts `GRAVITY` every tick afterwards, so a mortar aimed straight at a
//! target always lands short. This module computes the pitch that makes the
//! arc pass *through* the target instead.
//!
//! # The solution
//!
//! For a launch speed `v`, gravity `g`, horizontal distance `x` and height
//! difference `y`, the launch angles that hit `(x, y)` are the roots of
//!
//! ```text
//!     θ = atan( (v² ± √(v⁴ − g(g·x² + 2·y·v²))) / (g·x) )
//! ```
//!
//! There are two: a low, flat shot and a high, lobbed one. We take the low
//! root — it arrives sooner and is what a direct-fire mortar wants. The
//! discriminant going negative means the target is out of range at this
//! speed, which is a real answer the HUD wants rather than an error.
//!
//! # Why the tests integrate rather than compare angles
//!
//! The closed form assumes continuous motion; the projectile actually moves
//! in 60 Hz steps of semi-implicit Euler (`velocity.y -= g·dt` *then*
//! `pos += velocity·dt`). Those disagree by a small amount that grows with
//! flight time, so the tests fly the solved velocity through the same loop
//! the simulation uses and check where it actually lands. An angle that is
//! textbook-correct but misses in the sim is not correct here.

use bevy::prelude::*;

/// The launch pitch, in radians above horizontal, that lands a projectile of
/// speed `speed` at `delta` under acceleration `gravity`.
///
/// `delta` is the target's offset from the muzzle in world space, `y` up.
/// Returns `None` when the target is out of range, and for the degenerate
/// inputs that have no meaningful angle: a non-positive `speed`, or a target
/// directly overhead (`x ≈ 0`) where no pitch is more correct than any other.
///
/// With `gravity` at or below zero the shot does not arc, so the answer is
/// simply the pitch that points straight at the target.
pub fn launch_pitch(delta: Vec3, speed: f32, gravity: f32) -> Option<f32> {
    if speed <= 0.0 {
        return None;
    }

    let x = delta.xz().length();
    let y = delta.y;

    // A weapon that does not fall points straight at the target: the arc is
    // a line, and `atan2` handles the overhead case the arcing branch cannot.
    if gravity <= 0.0 {
        return Some(y.atan2(x));
    }
    // Directly overhead: every pitch has the same zero horizontal distance,
    // so there is nothing to solve. The caller keeps its current aim.
    if x <= f32::EPSILON {
        return None;
    }

    let v2 = speed * speed;
    let discriminant = v2 * v2 - gravity * (gravity * x * x + 2.0 * y * v2);
    if discriminant < 0.0 {
        return None;
    }

    // The low root: `−√` gives the flatter of the two angles.
    Some(((v2 - discriminant.sqrt()) / (gravity * x)).atan())
}

/// The launch velocity for [`launch_pitch`], in world space.
///
/// The pitch is applied in the vertical plane containing `delta`, so the
/// shot keeps the target's bearing and only its elevation changes.
pub fn launch_velocity(delta: Vec3, speed: f32, gravity: f32) -> Option<Vec3> {
    let pitch = launch_pitch(delta, speed, gravity)?;

    // The horizontal bearing to the target. A target directly overhead has
    // none; `launch_pitch` has already rejected that for an arcing weapon,
    // and a non-arcing one straight up needs a fallback that is not the zero
    // vector.
    let flat = delta.xz();
    let bearing = if flat.length_squared() > f32::EPSILON {
        Vec3::new(flat.x, 0.0, flat.y).normalize()
    } else {
        return Some(Vec3::Y * speed * pitch.sin().signum());
    };

    Some((bearing * pitch.cos() + Vec3::Y * pitch.sin()) * speed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `wep_2.0`, the HEM mortar, as `weapons.zrd` states it. The real-data
    /// test in `tests/real_weapons.rs` checks these against the shipped file;
    /// here they only need to be representative.
    const MORTAR_SPEED: f32 = 60.0;
    const MORTAR_GRAVITY: f32 = 9.8;

    /// The simulation's own integration: `move_projectiles`' semi-implicit
    /// Euler at the fixed 60 Hz tick, flown until the projectile passes the
    /// target's horizontal distance.
    ///
    /// Returns the height at that moment, so a caller can compare it against
    /// the target's. Stepping to a *distance* rather than a time is what makes
    /// the comparison meaningful: the shot either arrives at the right height
    /// or it does not.
    fn fly_to(delta: Vec3, velocity: Vec3, gravity: f32) -> f32 {
        let dt = 1.0 / 60.0;
        let target_x = delta.xz().length();
        let mut position = Vec3::ZERO;
        let mut velocity = velocity;

        // Generous: a 60 m/s shot at the mortar's range arrives well inside
        // this, and an infinite loop in a test is worse than a wrong answer.
        for _ in 0..6000 {
            let previous = position;
            velocity.y -= gravity * dt;
            position += velocity * dt;

            let travelled = position.xz().length();
            if travelled >= target_x {
                // Interpolate across the step that crossed the line, so the
                // result does not depend on where the tick boundary fell.
                let before = previous.xz().length();
                let t = ((target_x - before) / (travelled - before)).clamp(0.0, 1.0);
                return previous.y + (position.y - previous.y) * t;
            }
        }

        f32::NAN
    }

    /// A flat shot at a reachable target lands on it, in the simulation's own
    /// integrator and not just in the closed form.
    #[test]
    fn a_flat_shot_lands_on_the_target() {
        let delta = Vec3::new(0.0, 0.0, -200.0);
        let velocity =
            launch_velocity(delta, MORTAR_SPEED, MORTAR_GRAVITY).expect("200 m is in range");

        let height = fly_to(delta, velocity, MORTAR_GRAVITY);

        assert!(
            (height - delta.y).abs() < 1.0,
            "landed at {height}, wanted {}",
            delta.y
        );
    }

    /// Uphill and downhill both solve, and both land where they were aimed.
    #[test]
    fn the_solution_holds_up_and_down_a_slope() {
        for y in [-40.0, -15.0, 15.0, 40.0] {
            let delta = Vec3::new(0.0, y, -150.0);
            let velocity =
                launch_velocity(delta, MORTAR_SPEED, MORTAR_GRAVITY).expect("150 m is in range");

            let height = fly_to(delta, velocity, MORTAR_GRAVITY);

            assert!(
                (height - y).abs() < 1.0,
                "aimed at y = {y}, landed at {height}"
            );
        }
    }

    /// The bearing survives: a shot at a target off to one side keeps its
    /// heading and only gains elevation.
    #[test]
    fn the_shot_keeps_the_targets_bearing() {
        let delta = Vec3::new(120.0, 10.0, -90.0);
        let velocity =
            launch_velocity(delta, MORTAR_SPEED, MORTAR_GRAVITY).expect("150 m is in range");

        let wanted = delta.xz().normalize();
        let got = velocity.xz().normalize();

        assert!(
            wanted.distance(got) < 1e-4,
            "bearing drifted: wanted {wanted}, got {got}"
        );
        assert!(velocity.y > 0.0, "a 10 m climb needs an upward component");
    }

    /// The low root, not the high one: a reachable target has two solutions
    /// and we want the flatter, faster shot.
    #[test]
    fn the_low_root_is_the_one_returned() {
        let delta = Vec3::new(0.0, 0.0, -150.0);
        let pitch = launch_pitch(delta, MORTAR_SPEED, MORTAR_GRAVITY).expect("150 m is in range");

        // The two roots of a flat shot are symmetric about 45°; the low one
        // is below it and the high one above.
        assert!(
            pitch > 0.0 && pitch < std::f32::consts::FRAC_PI_4,
            "{pitch} rad is not the low root"
        );
    }

    /// Out of range is `None`, not a wrong angle: the maximum range of a
    /// 45° shot is v²/g, so anything past that has no solution.
    #[test]
    fn a_target_past_the_maximum_range_has_no_solution() {
        let max = MORTAR_SPEED * MORTAR_SPEED / MORTAR_GRAVITY;

        let inside = Vec3::new(0.0, 0.0, -(max * 0.95));
        assert!(launch_pitch(inside, MORTAR_SPEED, MORTAR_GRAVITY).is_some());

        let outside = Vec3::new(0.0, 0.0, -(max * 1.05));
        assert!(launch_pitch(outside, MORTAR_SPEED, MORTAR_GRAVITY).is_none());
    }

    /// Height counts against range: a target at the flat maximum becomes
    /// unreachable once it is raised.
    #[test]
    fn climbing_shortens_the_reach() {
        let max = MORTAR_SPEED * MORTAR_SPEED / MORTAR_GRAVITY;
        let delta = Vec3::new(0.0, 0.0, -(max * 0.95));

        assert!(launch_pitch(delta, MORTAR_SPEED, MORTAR_GRAVITY).is_some());
        assert!(
            launch_pitch(delta.with_y(60.0), MORTAR_SPEED, MORTAR_GRAVITY).is_none(),
            "the same distance 60 m up must not still be reachable"
        );
    }

    /// A weapon with no gravity points straight at the target: the `Straight`
    /// and `Beam` movers share this path and must not gain an arc.
    #[test]
    fn a_weapon_that_does_not_fall_aims_straight() {
        let delta = Vec3::new(0.0, 50.0, -50.0);

        let pitch = launch_pitch(delta, 100.0, 0.0).expect("no gravity always solves");

        assert!(
            (pitch - std::f32::consts::FRAC_PI_4).abs() < 1e-5,
            "a 45° offset wants a 45° pitch, got {pitch}"
        );
    }

    /// The degenerate inputs answer `None` rather than a NaN angle.
    #[test]
    fn degenerate_inputs_do_not_produce_an_angle() {
        let delta = Vec3::new(0.0, 0.0, -100.0);

        assert!(
            launch_pitch(delta, 0.0, MORTAR_GRAVITY).is_none(),
            "no speed"
        );
        assert!(
            launch_pitch(delta, -10.0, MORTAR_GRAVITY).is_none(),
            "negative speed"
        );
        assert!(
            launch_pitch(Vec3::new(0.0, 100.0, 0.0), MORTAR_SPEED, MORTAR_GRAVITY).is_none(),
            "directly overhead has no horizontal distance to solve for"
        );
    }

    /// A target at the muzzle is degenerate for an arcing weapon but must not
    /// panic or return a NaN.
    #[test]
    fn a_target_at_the_muzzle_is_handled() {
        let pitch = launch_pitch(Vec3::ZERO, MORTAR_SPEED, MORTAR_GRAVITY);

        assert!(pitch.is_none_or(|pitch| pitch.is_finite()));
    }
}
