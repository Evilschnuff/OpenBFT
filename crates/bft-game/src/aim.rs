//! The free cursor: where it points in the world, and how it turns the view.
//!
//! # The model
//!
//! The cursor is *free* — it moves over the window rather than being locked
//! to the centre — and two things follow from where it is:
//!
//!  1. **It names a world point.** A ray from the camera through the cursor
//!     is cast into the level, and what it hits becomes
//!     [`PlayerInput::aim_point`]. The turret slews to that point rather
//!     than integrating mouse movement, so the barrel tracks the cursor
//!     instead of the cursor tracking the barrel.
//!  2. **At the window's edge it scrolls the view.** Pushing the cursor into
//!     the margin turns the camera's own yaw, at a rate that grows with how
//!     far into the margin it is.
//!
//! The camera owns its world-space yaw and the hull turning underneath does
//! not drag it; see [`chase_camera`](crate::chase_camera) for the spring
//! that gives that its inertia.
//!
//! # Why the ray needs a fallback
//!
//! A cursor on the skyline hits nothing. Firing at "nothing" still has to
//! mean something, so the miss case takes the point at
//! [`AIM_FALLBACK_RANGE`] along the ray — far enough that aiming at the
//! horizon points the turret flat, which is what it looks like it should do.

use avian3d::prelude::*;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bft_sim::vehicle::{Controlled, PlayerInput};

use crate::chase_camera::ChaseCamera;
use crate::guided_view::GuidedMissile;

/// How far along the cursor ray the aim point sits when the ray hits
/// nothing.
///
/// // approximation: no reader field corresponds. The longest weapon range
/// in the data is the nuke's 1200 m, so a miss lands past anything that
/// could be shot at and the turret reads as "pointing at the horizon".
pub const AIM_FALLBACK_RANGE: f32 = 1500.0;

/// How wide the edge-scroll margin is, as a fraction of the window.
///
/// // approximation: no reader field corresponds. Narrow enough that the
/// middle of the screen is free for aiming, wide enough to be easy to hit.
pub const EDGE_MARGIN: f32 = 0.08;

/// How fast the view turns with the cursor at the very edge, rad/s.
///
/// // approximation: no reader field corresponds, and this deliberately does
/// *not* match `max_cam_yaw_rate` (0.87). That is the rate the camera's
/// automatic follow is limited to; a deliberate scroll has to beat the thing
/// it is fighting, which is the hull. Tracks turn at `turns[1]` = 3.5 rad/s
/// at speed, so a scroll slower than that cannot hold a heading through a
/// full-speed turn — which is exactly what 0.87 felt like. This is a little
/// over the hull's own rate so the player always wins.
pub const EDGE_SCROLL_RATE: f32 = 4.0;

/// The systems that turn the cursor into an aim point and a view rotation.
pub struct AimPlugin;

impl Plugin for AimPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Update,
            // Not under a menu: a cursor at the window's edge while the pause
            // menu is up must not spin the view behind it.
            (aim_point, edge_scroll).run_if(crate::ui::gameplay_input_enabled),
        );
    }
}

/// Cast the cursor's ray into the level and record what it points at.
///
/// Runs every frame rather than on the tick: the cursor moves with the
/// frame, and a turret that only learned about it at 60 Hz would lag a fast
/// mouse on a fast display.
pub fn aim_point(
    guided: Res<GuidedMissile>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    cameras: Query<(&Camera, &GlobalTransform), With<ChaseCamera>>,
    spatial: SpatialQuery,
    mut inputs: Query<(Entity, &mut PlayerInput, &Controlled)>,
) {
    // The view is on a guided missile, and the cursor is steering it: a ray
    // from there would swing the turret after whatever the missile sees.
    if guided.is_guiding() {
        return;
    }
    let Some(window) = window else {
        return;
    };
    let Ok((camera, camera_transform)) = cameras.single() else {
        return;
    };
    // No cursor over the window — alt-tabbed, or the pointer left — leaves
    // the last aim point standing rather than snapping the turret to a
    // default: a turret that swung to centre every time the mouse left the
    // window would be unusable.
    let Some(cursor) = window.cursor_position() else {
        return;
    };
    let Ok(ray) = camera.viewport_to_world(camera_transform, cursor) else {
        return;
    };

    for (entity, mut input, controlled) in &mut inputs {
        if *controlled != Controlled::Local {
            continue;
        }

        // The shooter's own hull is excluded: a camera behind the vehicle
        // looks through it, and a cursor over the hull must name what is
        // beyond it rather than the vehicle itself.
        let filter = SpatialQueryFilter::default().with_excluded_entities([entity]);

        let point = spatial
            .cast_ray(ray.origin, ray.direction, AIM_FALLBACK_RANGE, true, &filter)
            .map(|hit| ray.origin + ray.direction * hit.distance)
            .unwrap_or_else(|| ray.origin + ray.direction * AIM_FALLBACK_RANGE);

        input.aim_point = Some(point);
    }
}

/// Turn the camera when the cursor is in the window's edge margin.
///
/// Scrolling moves the camera's *anchor* — the offset from hull forward that
/// the tether pulls toward — and not just its current yaw. Writing
/// `camera.yaw` alone would stall: `follow_target`'s tether pulls back toward
/// the hull at `max_cam_yaw_rate`, which is the same rate this pushes at, so
/// the view would creep to the edge of the dead zone and stop. Moving the
/// anchor makes a held edge keep turning.
pub fn edge_scroll(
    time: Res<Time>,
    guided: Res<GuidedMissile>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    mut cameras: Query<&mut ChaseCamera>,
) {
    // A cursor at the edge is steering a guided missile hard, not turning
    // the chase view behind it.
    if guided.is_guiding() {
        return;
    }
    let Some(window) = window else {
        return;
    };
    if !window.focused {
        return;
    }
    let Some(cursor) = window.cursor_position() else {
        return;
    };

    let (width, height) = (window.width(), window.height());
    if width <= 0.0 || height <= 0.0 {
        return;
    }

    let horizontal = edge_push(cursor.x, width, EDGE_MARGIN);
    let vertical = edge_push(cursor.y, height, EDGE_MARGIN);
    if horizontal.is_none() && vertical.is_none() {
        return;
    }

    let dt = time.delta_secs();

    for mut camera in &mut cameras {
        let rate = camera
            .overrides
            .edge_scroll_rate
            .unwrap_or(EDGE_SCROLL_RATE);
        let limit = camera
            .overrides
            .pitch_limit
            .unwrap_or(crate::chase_camera::PITCH_LIMIT);

        if let Some(push) = horizontal {
            // Mouse +x is right, and turning right is a negative yaw about up
            // in a right-handed y-up frame — the same convention
            // `turret_delta` uses.
            let step = push * rate * dt;

            // Wrapped, so scrolling one way for a minute does not leave the
            // offset several turns from zero and the tether unwinding them.
            camera.yaw_offset = wrap_angle(camera.yaw_offset - step);
            // Straight onto the yaw as well, so the view answers the scroll
            // this frame instead of waiting for the tether to notice the
            // anchor moved.
            camera.yaw -= step;
        }

        if let Some(push) = vertical {
            // Window +y is *down* the screen, so pushing the cursor to the
            // top edge raises the view.
            camera.pitch = (camera.pitch + push * rate * dt).clamp(-limit, limit);
        }
    }
}

/// Keep an angle in −π…π.
///
/// The same wrap `bft_sim::vehicle` applies to turret yaw, kept local rather
/// than made public there for one caller.
fn wrap_angle(angle: f32) -> f32 {
    let wrapped = angle.rem_euclid(std::f32::consts::TAU);

    if wrapped > std::f32::consts::PI {
        wrapped - std::f32::consts::TAU
    } else {
        wrapped
    }
}

/// How hard the cursor is pushing against an edge: −1 at the left edge, +1 at
/// the right, and `None` anywhere in the middle.
///
/// Ramps rather than switching on, so easing the cursor into the margin turns
/// the view slowly and burying it turns at the full rate.
fn edge_push(x: f32, width: f32, margin: f32) -> Option<f32> {
    let margin = width * margin;
    if margin <= 0.0 {
        return None;
    }

    if x < margin {
        Some(-(1.0 - x.max(0.0) / margin))
    } else if x > width - margin {
        Some((x.min(width) - (width - margin)) / margin)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The middle of the window does not scroll: most of the screen has to
    /// be free for aiming.
    #[test]
    fn the_middle_of_the_window_does_not_scroll() {
        assert_eq!(edge_push(500.0, 1000.0, 0.08), None);
        assert_eq!(edge_push(100.0, 1000.0, 0.08), None);
        assert_eq!(edge_push(900.0, 1000.0, 0.08), None);
    }

    /// Both edges scroll, and in opposite directions.
    #[test]
    fn the_edges_scroll_in_opposite_directions() {
        let left = edge_push(0.0, 1000.0, 0.08).expect("the left edge scrolls");
        let right = edge_push(1000.0, 1000.0, 0.08).expect("the right edge scrolls");

        assert!((left + 1.0).abs() < 1e-5, "full left is −1, got {left}");
        assert!((right - 1.0).abs() < 1e-5, "full right is +1, got {right}");
    }

    /// The ramp: just inside the margin is a gentle push, the edge is a full
    /// one.
    #[test]
    fn the_push_ramps_with_depth_into_the_margin() {
        let margin = 1000.0 * 0.08;

        let edge = edge_push(999.0, 1000.0, 0.08).expect("in the margin");
        let shallow = edge_push(1000.0 - margin + 1.0, 1000.0, 0.08).expect("in the margin");

        assert!(
            edge > shallow,
            "deeper into the margin must push harder: {edge} vs {shallow}"
        );
        assert!(shallow > 0.0 && shallow < 0.2, "just inside is gentle");
    }

    /// A cursor dragged outside the window clamps rather than running away:
    /// some platforms report positions past the edge during a drag.
    #[test]
    fn a_cursor_past_the_edge_clamps_to_full_push() {
        let past = edge_push(1200.0, 1000.0, 0.08).expect("past the right edge");
        let before = edge_push(-200.0, 1000.0, 0.08).expect("past the left edge");

        assert!((past - 1.0).abs() < 1e-5, "clamped to +1, got {past}");
        assert!((before + 1.0).abs() < 1e-5, "clamped to −1, got {before}");
    }

    /// A degenerate window must not divide by zero.
    #[test]
    fn a_zero_margin_does_not_scroll() {
        assert_eq!(edge_push(0.0, 1000.0, 0.0), None);
    }

    /// The bug a held edge showed: the view turned a few degrees and stopped.
    ///
    /// `follow_target`'s tether pulls the camera back toward the hull at
    /// `max_cam_yaw_rate`, and a scroll that only wrote `camera.yaw` pushed at
    /// the same rate — so the two cancelled at the edge of the dead zone.
    /// Scrolling has to move the *anchor* the tether pulls toward, which this
    /// checks by holding the edge for a second of frames and requiring the
    /// offset to keep growing the whole way.
    #[test]
    fn holding_the_edge_keeps_turning_rather_than_stalling() {
        let dt = 1.0 / 60.0;
        // Half a second: long enough to show it does not stall, short enough
        // that at the scroll rate the offset stays inside one turn and
        // `wrap_angle` does not fold it round.
        let frames = 30;
        let mut offset: f32 = 0.0;
        let mut samples = Vec::new();

        for _ in 0..frames {
            let push = edge_push(1000.0, 1000.0, 0.08).expect("held at the right edge");
            offset = wrap_angle(offset - push * EDGE_SCROLL_RATE * dt);
            samples.push(offset);
        }

        let expected = EDGE_SCROLL_RATE * dt * frames as f32;
        assert!(
            expected < std::f32::consts::PI,
            "the test window has to stay inside one turn, not {expected} rad"
        );
        assert!(
            (samples[frames - 1] + expected).abs() < 1e-3,
            "{frames} frames at the edge should turn {expected} rad, got {}",
            -samples[frames - 1]
        );
        // And it never stopped partway: every frame moved it further.
        for pair in samples.windows(2) {
            assert!(
                pair[1] < pair[0],
                "the scroll stalled: {} then {}",
                pair[0],
                pair[1]
            );
        }
    }

    /// At full speed the hull must not out-turn the scroll, or holding the
    /// edge cannot hold a heading.
    ///
    /// Tracks turn at `turns[1]` = 3.5 rad/s moving. The camera's tether
    /// drags its yaw along with the hull, and the scroll has to beat that to
    /// give the player authority — so the scroll rate has to exceed the
    /// hull's own turn rate, not the camera's 0.87 follow limit.
    #[test]
    fn the_scroll_out_turns_a_hull_at_full_speed() {
        // `vehicle.zrd`'s `turns` for the player's track mode.
        const HULL_TURN_RATE: f32 = 3.5;

        // Read at run time: a `const` comparison clippy can fold is not an
        // assertion.
        assert!(
            std::hint::black_box(EDGE_SCROLL_RATE) > std::hint::black_box(HULL_TURN_RATE),
            "a scroll at {EDGE_SCROLL_RATE} rad/s cannot beat a hull turning at {HULL_TURN_RATE}"
        );

        // Over a second of a full-speed turn with the edge held against it,
        // the player still gains ground rather than losing it.
        let dt = 1.0 / 60.0;
        let mut offset: f32 = 0.0;
        for _ in 0..60 {
            let push = edge_push(1000.0, 1000.0, 0.08).expect("held at the right edge");
            // The hull turning one way, the scroll pushing the other.
            offset = wrap_angle(offset - push * EDGE_SCROLL_RATE * dt + HULL_TURN_RATE * dt);
        }

        assert!(
            offset < 0.0,
            "the scroll lost to the hull: ended at {offset} rad"
        );
    }

    /// The top edge raises the view and the bottom lowers it.
    ///
    /// Window +y runs *down* the screen, so the signs are easy to get
    /// backwards; this pins them.
    #[test]
    fn the_top_edge_raises_the_view_and_the_bottom_lowers_it() {
        let top = edge_push(0.0, 800.0, 0.08).expect("the top edge scrolls");
        let bottom = edge_push(800.0, 800.0, 0.08).expect("the bottom edge scrolls");

        // `edge_scroll` negates the push onto pitch, so a negative push is a
        // positive pitch: cursor at the top, view angled up.
        assert!(-top > 0.0, "the top edge must raise the view");
        assert!(-bottom < 0.0, "the bottom edge must lower it");
    }

    /// Pitch stops short of vertical however long the edge is held: at the
    /// poles the orbit degenerates and the horizon flips.
    #[test]
    fn the_vertical_scroll_stops_short_of_straight_up() {
        let dt = 1.0 / 60.0;
        let mut pitch: f32 = 0.0;

        // Ten seconds of holding the top edge, far more than the limit needs.
        for _ in 0..600 {
            let push = edge_push(0.0, 800.0, 0.08).expect("held at the top edge");
            pitch = (pitch - push * EDGE_SCROLL_RATE * dt).clamp(
                -crate::chase_camera::PITCH_LIMIT,
                crate::chase_camera::PITCH_LIMIT,
            );
        }

        assert_eq!(pitch, crate::chase_camera::PITCH_LIMIT);
        // And the limit itself is short of vertical, where the orbit would
        // degenerate. A `const` comparison clippy can fold is not an
        // assertion, so this reads the value at run time.
        assert!(
            std::hint::black_box(crate::chase_camera::PITCH_LIMIT) < std::f32::consts::FRAC_PI_2,
            "the limit has to stay short of vertical"
        );
    }
}
