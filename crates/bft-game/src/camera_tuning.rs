//! `assets/tuning/camera.ron`: the chase camera's override layer.
//!
//! The primary source is the data: `vehicle.zrd`'s `camback` gives three
//! chase offsets in hull space per vehicle and `player.zrd` gives the spring
//! (`camera_elastic`), the follow dead zone (`camera_zone`), the yaw rate
//! limit and the tether angle (DESIGN §5.3). Everything here is optional and
//! overrides one of those, or supplies one of the handful of numbers the
//! original has no equivalent for.

use bevy::prelude::*;
use serde::Deserialize;

/// The whole `camera.ron` file.
#[derive(Asset, TypePath, Deserialize, Debug, Clone, Default)]
#[serde(default, deny_unknown_fields)]
pub struct CameraOverrides {
    /// Replaces `camback[mode].z`: how far behind the hull the camera sits.
    pub distance: Option<f32>,
    /// Replaces `camback[mode].y`: how far above it.
    pub height: Option<f32>,
    /// How far above the hull origin the camera looks. Defaults to the
    /// vehicle's `aimy`, which is where the turret is.
    pub look_height: Option<f32>,
    /// How fast the cursor at the window's edge turns the view, rad/s. No
    /// reader equivalent.
    ///
    /// Deliberately not `max_cam_yaw_rate`: that limits the camera's
    /// *automatic* follow, while this has to beat the hull's own turn rate
    /// (`turns[1]`, 3.5 rad/s on tracks) or the player cannot hold a heading
    /// through a full-speed turn.
    pub edge_scroll_rate: Option<f32>,
    /// How far the view may be scrolled above or below the hull, radians.
    /// No reader equivalent.
    pub pitch_limit: Option<f32>,
    /// Multiplies the dead zone `camera_zone × max_cam_tether_angle` gives.
    ///
    /// Those two are sized for a camera chasing the turret; tethered to the
    /// hull the 14° they produce is narrow enough that the view follows
    /// every small correction.
    pub dead_zone_scale: Option<f32>,
    /// Over how many radians past the dead zone the follow reaches its full
    /// rate. No reader equivalent: the original's follow switches on hard
    /// and ours eases, because there is no positional spring downstream to
    /// smooth the transition.
    pub ease_band: Option<f32>,
    /// Replaces `player.zrd`'s `camera_elastic`.
    ///
    /// **Currently unread.** It drove a spring on the camera's *position*,
    /// which let the distance to the hull stretch and compress as the
    /// vehicle accelerated. The camera is placed rigidly now and all of its
    /// give is in the yaw, which `max_cam_yaw_rate` and the tether govern.
    /// Kept because `camera_elastic` is real reader data and a positional
    /// spring may yet earn its place — for a collision pull-in, say.
    pub follow_stiffness: Option<f32>,
    /// Vertical field of view, degrees. No reader equivalent.
    pub fov_deg: Option<f32>,
    /// How far behind the hull the camera sits in Sub form, where the
    /// `camback` offsets read as too close underwater.
    pub sub_distance: Option<f32>,
    /// The radius of the sphere swept from the turret to the camera to keep
    /// it out of walls, metres. No reader equivalent.
    pub collision_radius: Option<f32>,
    /// How fast a camera a wall drew in moves back out, m/s. No reader
    /// equivalent.
    pub arm_recovery_rate: Option<f32>,
    /// The view from a tether-guided missile. No reader equivalent.
    pub guided: GuidedViewOverrides,
}

/// `camera.ron`'s `guided` block: the view from a tether-guided missile.
#[derive(Deserialize, Debug, Clone, Default)]
#[serde(default, deny_unknown_fields)]
pub struct GuidedViewOverrides {
    /// How far behind the tail end of the missile the camera rides, metres;
    /// zero is straight over the end, negative forward along the body.
    pub back: Option<f32>,
    /// How far above its top, metres. What puts the missile in the lower
    /// half of the frame.
    pub up: Option<f32>,
    /// How opaque the missile is drawn, 0–1.
    pub missile_alpha: Option<f32>,
    /// The filter over the view, as sRGB red, green, blue and opacity.
    pub filter: Option<[f32; 4]>,
    /// The fraction of the half-window around the centre that steers
    /// nothing, 0–1.
    pub dead_zone: Option<f32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_file_parses_to_all_defaults() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tuning/camera.ron"),
        )
        .expect("the shipped camera.ron is in the repo");

        let parsed: CameraOverrides = ron::de::from_str(&text).expect("it parses");

        assert!(parsed.distance.is_none());
        assert!(parsed.follow_stiffness.is_none());
    }

    /// `implicit_some` in the file's header: a bare value is read as set,
    /// so `back: 10` is not the parse error that drops the whole file.
    #[test]
    fn a_bare_value_is_read_as_set() {
        let text = "#![enable(implicit_some)]\n(fov_deg: 70, guided: (back: 1.5, up: Some(0.2)))";

        let parsed: CameraOverrides = ron::de::from_str(text).expect("it parses");

        assert_eq!(parsed.fov_deg, Some(70.0));
        assert_eq!(parsed.guided.back, Some(1.5));
        assert_eq!(parsed.guided.up, Some(0.2));
    }

    #[test]
    fn a_typo_is_a_parse_error_rather_than_a_silent_no_op() {
        let result: Result<CameraOverrides, _> = ron::de::from_str("(distanse: Some(10.0))");

        assert!(result.is_err());
    }
}
