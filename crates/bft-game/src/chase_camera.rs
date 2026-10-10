//! The chase camera: follows the hull, holds the yaw the mouse set.
//!
//! # What the original does, as the data describes it
//!
//! `vehicle.zrd` gives every vehicle three `camback` offsets in hull space —
//! the player's are `(0, 4, 12.75)`, `(0, 3.5, 2.25)`, `(0, 3.5, 2.25)`, so
//! one chase position and two near ones — and `aimy` says how high the turret
//! sits. `player.zrd` gives the spring (`camera_elastic` 0.04), a dead zone
//! (`camera_zone` 0.7), a yaw rate limit (`max_cam_yaw_rate` 0.87 rad/s) and
//! a tether angle (`max_cam_tether_angle` 20°).
//!
//! The behaviour those describe, and what this implements: **the camera owns
//! its world-space yaw, and the hull tethers it.** The mouse sets that yaw —
//! by scrolling at the window's edge, see [`aim`](crate::aim) — and a hull
//! that turns underneath does *not* drag the view with it. What the hull does
//! is pull: once it has turned more than `zone` of the tether angle away from
//! where the camera is looking, the camera eases round after it, never faster
//! than the rate limit. That lag is the inertia you see when the body turns
//! and the view settles back a moment later.
//!
//! The turret is downstream of all this: it slews to
//! [`PlayerInput::aim_point`](bft_sim::vehicle::PlayerInput::aim_point), the
//! world point under the cursor, and neither owns nor follows the camera's
//! yaw.
//!
//! # Walls
//!
//! The arm from the turret to the camera is swept against the level each
//! frame, and a camera that would end up behind a wall or under the ground
//! is drawn in in front of it instead. Without that it looked through the
//! level from inside it, and the cursor's ray, starting behind the wall,
//! named a point there: the turret swung round to aim at the tank's own
//! back. The arm shortens at once and lengthens again at a rate, so a wall
//! the camera slides past does not make the view pop.
//!
//! # Why it runs in `Update` and not on the tick
//!
//! The camera is presentation: it interpolates toward the simulated hull
//! every *frame*, so it is smooth at any frame rate while the hull moves at
//! 60 Hz (DESIGN §4.2). It reads sim components and writes only its own
//! transform, so nothing downstream of it is frame-rate dependent.

use avian3d::prelude::*;
use bevy::prelude::*;
use bft_sim::level::GameLayer;
use bft_sim::vehicle::{Vehicle, VehicleForm, VehicleTable};

use crate::camera::FlyCamera;
use crate::camera_tuning::CameraOverrides;

/// Vertical field of view, degrees.
///
/// // approximation: no reader field corresponds. Bevy's own default is 45°;
/// the levels fog out at 320–650 units and a wider view reads better in
/// them.
pub const FOV_DEG: f32 = 60.0;

/// How far behind the hull the camera sits in Sub form.
///
/// // approximation: the `camback` offsets are one set per vehicle, not per
/// form, and 12.75 m behind reads as inside the sea floor in a 24 m deep
/// pool. Closer, so the sub stays visible.
pub const SUB_DISTANCE: f32 = 8.0;

/// A camera that follows a vehicle.
#[derive(Component, Debug, Clone)]
pub struct ChaseCamera {
    /// The hull it follows.
    pub target: Entity,
    /// Which of the vehicle's three `camback` offsets to use.
    pub mode: u8,
    /// The camera's own yaw, in world space. The mouse sets it and the hull
    /// pulls it round through the dead zone and the rate limit.
    pub yaw: f32,
    /// The yaw the player has scrolled to, relative to hull forward.
    ///
    /// This is what the tether pulls [`yaw`](Self::yaw) toward — *not* hull
    /// forward itself. Scrolling moves the anchor, so a held edge keeps
    /// turning the view instead of stalling where the tether's pull matches
    /// the scroll rate.
    pub yaw_offset: f32,
    /// How far above or below the hull the view is angled, in radians.
    ///
    /// Scrolled at the top and bottom edges and held until the player
    /// changes it: nothing pulls pitch back, because there is no hull
    /// heading for it to be wrong about.
    pub pitch: f32,
    /// Whether the camera has been put behind its target yet.
    ///
    /// The *yaw* is what this guards: a camera that lagged its yaw in from
    /// zero would swing round the vehicle on spawn, and m1's start faces
    /// 166.5 degrees. The first frame takes the hull's heading outright and
    /// every frame after it lags.
    placed: bool,
    /// How far from the turret the camera stands now, in metres: the full
    /// orbit, or less where a wall is in the way. See the module docs.
    arm: f32,
    /// The resolved overrides, so the systems need not look the asset up
    /// every frame.
    pub overrides: CameraOverrides,
}

impl ChaseCamera {
    /// A camera behind `target` in its default chase mode.
    pub fn following(target: Entity, overrides: CameraOverrides) -> ChaseCamera {
        ChaseCamera {
            target,
            mode: 0,
            yaw: 0.0,
            yaw_offset: 0.0,
            pitch: 0.0,
            placed: false,
            arm: 0.0,
            overrides,
        }
    }

    /// Put the camera straight behind the hull on its next frame instead of
    /// swinging round to it: after a cinematic hands the view back, and
    /// after a script teleported the player.
    pub fn snap(&mut self) {
        self.placed = false;
        self.yaw_offset = 0.0;
    }
}

/// Spawn a chase camera behind a vehicle.
///
/// `order` is 0, below the UI camera `GamePlugin` spawns at 1, so the HUD
/// draws over the world.
pub fn spawn_chase_camera(
    commands: &mut Commands,
    target: Entity,
    overrides: &CameraOverrides,
) -> Entity {
    commands
        .spawn((
            Name::new("chase camera"),
            ChaseCamera::following(target, overrides.clone()),
            Camera3d::default(),
            Camera {
                order: 0,
                ..default()
            },
            Projection::Perspective(PerspectiveProjection {
                fov: overrides.fov_deg.unwrap_or(FOV_DEG).to_radians(),
                ..default()
            }),
            Transform::default(),
        ))
        .id()
}

/// Where a chase camera wants to be, and what it wants to look at.
///
/// Pure, so the placement can be tested without an `App`: given the hull's
/// transform, the camera yaw and the offsets, it returns `(eye, target)`.
pub fn chase_viewpoint(
    hull: &Transform,
    yaw: f32,
    distance: f32,
    height: f32,
    look_height: f32,
) -> (Vec3, Vec3) {
    // The camera orbits the hull at its own yaw about world up, not about the
    // hull's up: a hull pitched over a rise must not roll the view with it.
    let rotation = Quat::from_rotation_y(yaw);
    let back = rotation * Vec3::Z;

    let focus = hull.translation + Vec3::Y * look_height;
    let eye = hull.translation + back * distance + Vec3::Y * height;

    (eye, focus)
}

/// [`chase_viewpoint`] with the view angled above or below the hull.
///
/// A positive `pitch` lifts the camera and tips it down at the vehicle; a
/// negative one drops it toward the ground. The orbit radius is preserved, so
/// scrolling up and down swings the camera over a sphere rather than sliding
/// it along a wall and ending up inside the terrain.
pub fn chase_viewpoint_pitched(
    hull: &Transform,
    yaw: f32,
    pitch: f32,
    distance: f32,
    height: f32,
    look_height: f32,
) -> (Vec3, Vec3) {
    // Clamped short of straight up and down: at the poles the orbit direction
    // degenerates and `look_at` has no stable up vector.
    let pitch = pitch.clamp(-PITCH_LIMIT, PITCH_LIMIT);

    let rotation = Quat::from_rotation_y(yaw);
    let back = rotation * Vec3::Z;

    // The offset the unpitched camera would use, rotated about the axis
    // running through the hull to its right. That keeps `distance` and
    // `height` meaning what they do at pitch zero.
    //
    // Negated: the camera sits *behind* the hull, so its offset is along
    // `+Z` at yaw zero, and a positive rotation about `+X` would swing that
    // toward `−Y` — scrolling up would dig the camera into the ground.
    let offset = back * distance + Vec3::Y * height;
    let right = rotation * Vec3::X;
    let offset = Quat::from_axis_angle(right, -pitch) * offset;

    let focus = hull.translation + Vec3::Y * look_height;
    let eye = hull.translation + offset;

    (eye, focus)
}

/// How far the view may be scrolled above or below the hull, in radians.
///
/// // approximation: no reader field corresponds. Short of vertical, where
/// the orbit degenerates and the horizon flips.
pub const PITCH_LIMIT: f32 = 1.2;

/// The radius of the sphere swept from the turret to the camera, in metres.
///
/// // approximation: no reader field corresponds. Comfortably wider than the
/// near plane's corners (0.1 m out at 60°), so a camera pressed against a
/// wall stops short of showing through it rather than grazing it.
pub const COLLISION_RADIUS: f32 = 0.4;

/// How fast a camera drawn in by a wall lengthens its arm again, m/s.
///
/// // approximation: no reader field corresponds. Quick enough to be back
/// out within a second of clearing a corner, slow enough not to pop.
pub const ARM_RECOVERY_RATE: f32 = 10.0;

/// How far from `focus` towards `eye` a sphere of `radius` gets before the
/// level stops it: the full distance when nothing is in the way.
///
/// The level only: vehicles passing between the camera and the hull, water
/// surfaces and trigger volumes do not draw the camera in. A focus that
/// starts inside geometry — the turret against a low ceiling — is not a hit,
/// or the camera would collapse onto the turret.
pub fn clear_distance(spatial: &SpatialQuery, focus: Vec3, eye: Vec3, radius: f32) -> f32 {
    let offset = eye - focus;
    let length = offset.length();
    let Ok(direction) = Dir3::new(offset) else {
        return length;
    };

    let config = ShapeCastConfig {
        max_distance: length,
        ignore_origin_penetration: true,
        ..ShapeCastConfig::DEFAULT
    };
    spatial
        .cast_shape(
            &Collider::sphere(radius),
            focus,
            Quat::IDENTITY,
            direction,
            &config,
            &SpatialQueryFilter::from_mask(GameLayer::Level),
        )
        .map_or(length, |hit| hit.distance.min(length))
}

/// The arm's length this frame: at once down to what is clear, and back out
/// toward it at `rate`.
fn next_arm(current: f32, clear: f32, rate: f32, dt: f32) -> f32 {
    if clear <= current {
        clear
    } else {
        (current + rate * dt).min(clear)
    }
}

/// The chase camera's systems.
pub struct ChaseCameraPlugin;

impl Plugin for ChaseCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, follow_target);
    }
}

/// The chase cameras [`follow_target`] moves: not a vehicle, not flying free.
type Chasing = (Without<Vehicle>, Without<FlyCamera>);

/// Move the camera toward where it wants to be.
///
/// Not a camera the dev tools' free camera is flying (`FlyCamera`), which
/// picks the chase back up from wherever the flight ended.
pub fn follow_target(
    time: Res<Time>,
    spatial: SpatialQuery,
    table: Option<Res<VehicleTable>>,
    mut cameras: Query<(&mut ChaseCamera, &mut Transform), Chasing>,
    hulls: Query<(&Transform, &Vehicle)>,
) {
    let dt = time.delta_secs();

    for (mut camera, mut transform) in &mut cameras {
        let Ok((hull, vehicle)) = hulls.get(camera.target) else {
            continue;
        };

        let def = table.as_ref().and_then(|table| table.kind(&vehicle.kind));

        // `camback` is (x, up, back) in hull space; the mode index picks one
        // of the three.
        let camback = def
            .map(|def| def.camback[(camera.mode as usize).min(2)])
            .unwrap_or(Vec3::new(0.0, 4.0, 12.75));

        let distance = match vehicle.form {
            VehicleForm::Sub => camera.overrides.sub_distance.unwrap_or(SUB_DISTANCE),
            _ => camera.overrides.distance.unwrap_or(camback.z),
        };
        let height = camera.overrides.height.unwrap_or(camback.y);
        let look_height = camera
            .overrides
            .look_height
            // `aimy` is where the turret is, which is what the camera should
            // be looking through.
            .or_else(|| def.map(|def| def.aim_y[0]))
            .unwrap_or(camback.y);

        // What the camera is tethered to is the *hull*, offset by however far
        // the player has scrolled: the view holds the angle the mouse left it
        // at and the hull pulls it round only once it has turned far enough
        // away. Tethering to bare `hull_yaw` instead would make a held edge
        // scroll stall, because the tether pulls back at the same rate the
        // scroll pushes. The turret is downstream of the cursor and does not
        // enter into it.
        let (hull_yaw, ..) = hull.rotation.to_euler(EulerRot::YXZ);
        let wanted_yaw = hull_yaw + camera.yaw_offset;

        // The first frame starts *at* the aim rather than chasing it: a
        // camera that springs its yaw in from zero swings round the vehicle
        // on spawn, and m1's start faces 166.5 degrees.
        camera.yaw = if camera.placed {
            follow_yaw(
                camera.yaw,
                wanted_yaw,
                table.as_ref().map(|table| &table.player),
                &camera.overrides,
                dt,
            )
        } else {
            wanted_yaw
        };

        let (eye, focus) = chase_viewpoint_pitched(
            hull,
            camera.yaw,
            camera.pitch,
            distance,
            height,
            look_height,
        );

        // Drawn in in front of whatever wall is between the turret and
        // where the camera wants to be. Shortening is immediate — a frame
        // spent inside the wall is a frame of seeing through the level —
        // and lengthening eases, so sliding past a corner does not pop.
        let clear = clear_distance(
            &spatial,
            focus,
            eye,
            camera
                .overrides
                .collision_radius
                .unwrap_or(COLLISION_RADIUS),
        );
        camera.arm = if camera.placed {
            next_arm(
                camera.arm,
                clear,
                camera
                    .overrides
                    .arm_recovery_rate
                    .unwrap_or(ARM_RECOVERY_RATE),
                dt,
            )
        } else {
            clear
        };
        let eye = focus + (eye - focus).normalize_or_zero() * camera.arm;

        // Placed rigidly, not sprung toward. Away from walls the distance to
        // the hull is fixed: all the give in this camera is in the *angle*,
        // which `follow_yaw` above has already lagged through the dead zone
        // and the rate limit.
        //
        // Lerping the position instead would let the radius stretch whenever
        // the hull accelerated away and compress when it stopped, so the
        // camera would breathe in and out along its own view axis. `chase_viewpoint_pitched` returns a point at exactly
        // `distance` from the hull and the camera simply goes there.
        transform.translation = eye;
        transform.look_at(focus, Vec3::Y);
        camera.placed = true;
    }
}

/// The camera's new yaw: chase the aim, but only outside the dead zone and
/// never faster than the rate limit.
fn follow_yaw(
    current: f32,
    wanted: f32,
    player: Option<&bft_sim::vehicle::PlayerConstants>,
    overrides: &CameraOverrides,
    dt: f32,
) -> f32 {
    let (zone, max_rate, tether) = match player {
        Some(player) => (
            player.camera.zone,
            player.camera.max_yaw_rate,
            player.camera.max_tether_angle_deg.to_radians(),
        ),
        None => (0.7, 0.87, 20f32.to_radians()),
    };

    let error = shortest_angle(wanted - current);

    // Inside the dead zone the camera holds still.
    let dead = zone * tether * overrides.dead_zone_scale.unwrap_or(DEAD_ZONE_SCALE);
    if error.abs() <= dead {
        return current;
    }

    // Outside it, close the *excess* — the part beyond the dead zone — so the
    // camera settles at the edge of the zone rather than snapping to centre.
    let excess = error.abs() - dead;

    // Eased in over the first `EASE_BAND` radians past the zone rather than
    // jumping to the full rate the moment the hull leaves it. A hard switch
    // reads as a lurch: the camera is perfectly still, then instantly moving
    // at `max_cam_yaw_rate`. With the camera placed rigidly this is the
    // only smoothing, so it has to be here.
    let band = overrides.ease_band.unwrap_or(EASE_BAND);
    let eased = if band > 0.0 {
        max_rate * (excess / band).clamp(0.0, 1.0)
    } else {
        max_rate
    };

    let step = (eased * dt).min(excess);

    current + step * error.signum()
}

/// How much wider the camera's dead zone is than `camera_zone × tether`.
///
/// // approximation: `camera_zone` (0.7) and `max_cam_tether_angle` (20°)
/// give 14°, which was sized for a camera chasing the *turret*. Tethered to
/// the hull instead, and with the distance rigid, 14° has the view following
/// every small correction. This widens it so only a real turn moves the
/// camera.
pub const DEAD_ZONE_SCALE: f32 = 2.0;

/// Over how many radians past the dead zone the follow reaches full rate.
///
/// // approximation: no reader field corresponds. Wide enough that leaving
/// the zone is a roll rather than a lurch, narrow enough that a hard turn
/// still gets the full `max_cam_yaw_rate` promptly.
pub const EASE_BAND: f32 = 0.35;

/// The signed shortest way round from one angle to another.
fn shortest_angle(angle: f32) -> f32 {
    let wrapped = angle.rem_euclid(std::f32::consts::TAU);

    if wrapped > std::f32::consts::PI {
        wrapped - std::f32::consts::TAU
    } else {
        wrapped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_sim::vehicle::{CameraConstants, PlayerConstants};

    fn constants() -> PlayerConstants {
        PlayerConstants {
            camera: CameraConstants::default(),
            ..PlayerConstants::default()
        }
    }

    /// The camera sits behind and above the hull and looks at it.
    #[test]
    fn the_camera_stands_behind_the_yaw_it_is_given() {
        let hull = Transform::from_xyz(10.0, 0.0, 20.0);

        let (eye, focus) = chase_viewpoint(&hull, 0.0, 12.75, 4.0, 3.0);

        // Yaw 0 means looking along -z, so the camera is at +z from the hull.
        assert!((eye.z - 32.75).abs() < 1e-4, "{eye:?}");
        assert!((eye.x - 10.0).abs() < 1e-4, "{eye:?}");
        assert!((eye.y - 4.0).abs() < 1e-4, "{eye:?}");
        assert_eq!(focus, Vec3::new(10.0, 3.0, 20.0));
    }

    /// Scrolling up lifts the camera without changing how far away it is:
    /// the view swings over a sphere rather than sliding up a wall.
    #[test]
    fn pitching_the_view_keeps_the_orbit_radius() {
        let hull = Transform::from_xyz(5.0, 1.0, -3.0);

        let (flat, _) = chase_viewpoint_pitched(&hull, 0.0, 0.0, 12.75, 4.0, 3.0);
        let (raised, _) = chase_viewpoint_pitched(&hull, 0.0, 0.6, 12.75, 4.0, 3.0);

        let flat_radius = (flat - hull.translation).length();
        let raised_radius = (raised - hull.translation).length();

        assert!(
            (flat_radius - raised_radius).abs() < 1e-3,
            "radius changed from {flat_radius} to {raised_radius}"
        );
        assert!(
            raised.y > flat.y,
            "a positive pitch must lift the camera: {} vs {}",
            raised.y,
            flat.y
        );
    }

    /// Zero pitch is exactly the unpitched placement, so the new path cannot
    /// have quietly moved the default camera.
    #[test]
    fn zero_pitch_matches_the_flat_viewpoint() {
        let hull = Transform::from_xyz(10.0, 0.0, 20.0);

        let (flat, flat_focus) = chase_viewpoint(&hull, 0.7, 12.75, 4.0, 3.0);
        let (pitched, pitched_focus) = chase_viewpoint_pitched(&hull, 0.7, 0.0, 12.75, 4.0, 3.0);

        assert!(flat.distance(pitched) < 1e-4, "{flat:?} vs {pitched:?}");
        assert_eq!(flat_focus, pitched_focus);
    }

    /// A quarter turn puts the camera to the side, not behind in world space.
    #[test]
    fn the_camera_orbits_with_the_yaw() {
        let hull = Transform::from_xyz(0.0, 0.0, 0.0);

        let (eye, _) = chase_viewpoint(&hull, std::f32::consts::FRAC_PI_2, 10.0, 0.0, 0.0);

        assert!((eye.x - 10.0).abs() < 1e-4, "{eye:?}");
        assert!(eye.z.abs() < 1e-4, "{eye:?}");
    }

    /// The dead zone is what stops the view swinging on every small aim.
    #[test]
    fn a_small_aim_change_does_not_move_the_camera() {
        let player = constants();
        // 0.7 × 20° = 14°; 5° is well inside.
        let wanted = 5f32.to_radians();

        let yaw = follow_yaw(
            0.0,
            wanted,
            Some(&player),
            &CameraOverrides::default(),
            1.0 / 60.0,
        );

        assert_eq!(yaw, 0.0);
    }

    #[test]
    fn a_large_aim_change_pulls_the_camera_round_at_the_rate_limit() {
        let player = constants();
        let wanted = 90f32.to_radians();
        let dt = 1.0 / 60.0;

        let yaw = follow_yaw(0.0, wanted, Some(&player), &CameraOverrides::default(), dt);

        // One tick of `max_cam_yaw_rate`, and no more.
        assert!(
            (yaw - player.camera.max_yaw_rate * dt).abs() < 1e-6,
            "{yaw}"
        );
    }

    /// Chasing must settle at the edge of the dead zone, not overshoot past
    /// the aim and oscillate.
    #[test]
    fn chasing_settles_at_the_edge_of_the_dead_zone() {
        let player = constants();
        let wanted = 90f32.to_radians();
        let zone =
            player.camera.zone * player.camera.max_tether_angle_deg.to_radians() * DEAD_ZONE_SCALE;

        let mut yaw = 0.0;
        for _ in 0..1200 {
            yaw = follow_yaw(
                yaw,
                wanted,
                Some(&player),
                &CameraOverrides::default(),
                1.0 / 60.0,
            );
        }

        assert!(
            (wanted - yaw - zone).abs() < 1e-3,
            "settled at {yaw}, wanted {}",
            wanted - zone
        );
    }

    /// Going the short way round: chasing an aim just past −π must not spin
    /// the camera the whole way.
    ///
    /// The error here is `shortest_angle(-3 - 3)` ≈ 0.28 rad, which is inside
    /// the widened dead zone, so the angle alone no longer moves the camera.
    /// Pushed past the zone to keep the test about *direction*.
    #[test]
    fn the_camera_takes_the_short_way_round() {
        let player = constants();
        let dt = 1.0;
        // Just under −π against just over +π: the short way is forward.
        let yaw = follow_yaw(2.0, -2.5, Some(&player), &CameraOverrides::default(), dt);

        assert!(yaw > 2.0, "should have gone forward past pi, got {yaw}");
    }

    /// Leaving the dead zone must be a roll, not a lurch.
    ///
    /// A hard switch would go from perfectly still to the full
    /// `max_cam_yaw_rate` the instant the hull left the zone. With the camera
    /// placed rigidly the ease is the only smoothing.
    #[test]
    fn the_follow_eases_in_rather_than_switching_on() {
        let player = constants();
        let dt = 1.0 / 60.0;
        let zone =
            player.camera.zone * player.camera.max_tether_angle_deg.to_radians() * DEAD_ZONE_SCALE;

        // A hair outside the zone moves the camera, but nowhere near the
        // full rate.
        let just_outside = follow_yaw(
            0.0,
            zone + 0.01,
            Some(&player),
            &CameraOverrides::default(),
            dt,
        );
        assert!(just_outside > 0.0, "it has to move at all");
        assert!(
            just_outside < player.camera.max_yaw_rate * dt * 0.2,
            "leaving the zone lurched: {just_outside} of a possible {}",
            player.camera.max_yaw_rate * dt
        );

        // Well past it, the full rate is available.
        let far_outside = follow_yaw(
            0.0,
            zone + EASE_BAND + 1.0,
            Some(&player),
            &CameraOverrides::default(),
            dt,
        );
        assert!(
            (far_outside - player.camera.max_yaw_rate * dt).abs() < 1e-6,
            "a hard turn must still get the full rate, got {far_outside}"
        );
    }

    /// The dead zone is wider than the raw `camera_zone × tether`, which was
    /// sized for a camera chasing the turret rather than the hull.
    #[test]
    fn the_dead_zone_is_wider_than_the_raw_reader_value() {
        let player = constants();
        let raw = player.camera.zone * player.camera.max_tether_angle_deg.to_radians();
        let dt = 1.0 / 60.0;

        // An error just past the raw zone leaves the camera still.
        let yaw = follow_yaw(
            0.0,
            raw + 0.02,
            Some(&player),
            &CameraOverrides::default(),
            dt,
        );

        assert_eq!(
            yaw,
            0.0,
            "{}° should still be inside the zone",
            (raw + 0.02).to_degrees()
        );
    }

    /// A new camera starts with its yaw unplaced, so the first frame takes
    /// the hull's heading outright instead of lagging in from zero.
    ///
    /// In m1 the level sits 2500 units from the origin and the start faces
    /// 166.5°, so a camera that eased its yaw in would swing right round the
    /// vehicle on spawn.
    #[test]
    fn a_new_camera_has_not_placed_its_yaw_yet() {
        let fresh = ChaseCamera::following(Entity::PLACEHOLDER, CameraOverrides::default());

        assert!(!fresh.placed, "a new camera has not been placed");
        assert_eq!(fresh.yaw, 0.0);
        assert_eq!(fresh.yaw_offset, 0.0);
    }

    /// The distance to the hull is rigid: all the give is in the angle.
    ///
    /// This is what replaced the positional spring. Lerping the camera's
    /// *position* toward the wanted eye point let the radius stretch while
    /// the hull accelerated and compress when it stopped, so the view
    /// breathed in and out along its own axis. However the hull is moving,
    /// the camera stands exactly `distance` away.
    #[test]
    fn the_distance_to_the_hull_never_changes() {
        let distance = 12.75f32;
        let height = 4.0f32;
        let radius = (distance * distance + height * height).sqrt();

        // A hull scattered around the level, at assorted headings and with
        // the view scrolled to assorted angles.
        for (x, z, yaw, pitch) in [
            (0.0, 0.0, 0.0, 0.0),
            (2222.1, 1294.2, 2.9, 0.0),
            (-800.0, 130.0, -1.2, 0.6),
            (15.0, -60.0, 0.4, -1.1),
        ] {
            let hull = Transform::from_xyz(x, 3.9, z);
            let (eye, _) = chase_viewpoint_pitched(&hull, yaw, pitch, distance, height, 3.0);

            let actual = (eye - hull.translation).length();
            assert!(
                (actual - radius).abs() < 1e-3,
                "at ({x}, {z}) yaw {yaw} pitch {pitch} the camera stood {actual} m out, wanted {radius}"
            );
        }
    }

    /// A wall appearing between the turret and the camera draws it in on
    /// the same frame: a frame inside the wall is a frame of seeing through
    /// the level.
    #[test]
    fn the_arm_shortens_at_once_and_lengthens_at_a_rate() {
        let dt = 1.0 / 60.0;

        assert_eq!(next_arm(13.0, 4.0, ARM_RECOVERY_RATE, dt), 4.0);

        let eased = next_arm(4.0, 13.0, ARM_RECOVERY_RATE, dt);
        assert!(
            (eased - (4.0 + ARM_RECOVERY_RATE * dt)).abs() < 1e-5,
            "{eased}"
        );

        // And never past what is clear.
        assert_eq!(next_arm(12.99, 13.0, ARM_RECOVERY_RATE, dt), 13.0);
    }

    #[test]
    fn the_shortest_angle_is_signed_and_within_half_a_turn() {
        assert!((shortest_angle(0.1) - 0.1).abs() < 1e-6);
        assert!((shortest_angle(-0.1) + 0.1).abs() < 1e-6);
        assert!((shortest_angle(std::f32::consts::TAU + 0.1) - 0.1).abs() < 1e-5);
        assert!(shortest_angle(4.0) < 0.0, "past pi goes the other way");
    }
}
