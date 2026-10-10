//! A free-fly camera for inspecting levels: WASD to move, Q/E for down/up,
//! hold right mouse to look, shift to move faster.

use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;

const MOVE_SPEED: f32 = 64.0;
const SPRINT_MULTIPLIER: f32 = 4.0;
const LOOK_SENSITIVITY: f32 = 0.002;
/// Stop just short of vertical, where yaw/pitch degenerates.
const PITCH_LIMIT: f32 = std::f32::consts::FRAC_PI_2 - 0.01;

/// The free-fly camera's input systems.
///
/// The camera entity itself is **not** spawned here. The viewer spawns it on
/// entering [`AppState::Viewer`](bft_sim::states::AppState::Viewer) and
/// despawns it on leaving, because a camera sitting at the origin of a level
/// that spans x 1287–2769 shows the user nothing (docs/data/coordinates.md).
pub struct FlyCameraPlugin;

impl Plugin for FlyCameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (look, move_camera));
    }
}

/// A camera the player flies with WASD/QE and the right mouse button.
///
/// Yaw and pitch are kept here rather than read back from the `Transform`
/// each frame, so writing the transform from outside would be silently undone
/// on the next mouse movement. [`FlyCamera::looking_at`] and
/// [`FlyCamera::place`] are how to move it.
#[derive(Component, Debug, Clone, Copy)]
pub struct FlyCamera {
    yaw: f32,
    pitch: f32,
}

impl FlyCamera {
    /// A camera at `eye` pointed at `target`.
    ///
    /// Returns the component and the transform together, because the two have
    /// to agree: the angles are the authority and the transform is derived
    /// from them.
    pub fn looking_at(eye: Vec3, target: Vec3) -> (FlyCamera, Transform) {
        let transform = Transform::from_translation(eye).looking_at(target, Vec3::Y);
        let (yaw, pitch, _) = transform.rotation.to_euler(EulerRot::YXZ);

        (FlyCamera { yaw, pitch }, transform)
    }

    /// Move an existing camera, keeping its look state consistent.
    pub fn place(&mut self, transform: &mut Transform, eye: Vec3, target: Vec3) {
        let (fly, placed) = FlyCamera::looking_at(eye, target);
        *self = fly;
        *transform = placed;
    }

    /// Where the camera is looking, in yaw/pitch radians.
    pub fn angles(&self) -> (f32, f32) {
        (self.yaw, self.pitch)
    }
}

fn look(
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    camera: Option<Single<(&mut Transform, &mut FlyCamera)>>,
) {
    // `Option` on both: the camera exists only in the viewer now, and
    // `Single` on an empty query would skip the system with a warning every
    // frame while a menu is up.
    let (Some(window), Some(camera)) = (window, camera) else {
        return;
    };
    if !mouse_buttons.pressed(MouseButton::Right) || !window.focused {
        return;
    }

    let (mut transform, mut fly) = camera.into_inner();
    let delta = mouse_motion.delta;

    fly.yaw -= delta.x * LOOK_SENSITIVITY;
    fly.pitch = (fly.pitch - delta.y * LOOK_SENSITIVITY).clamp(-PITCH_LIMIT, PITCH_LIMIT);

    transform.rotation = Quat::from_euler(EulerRot::YXZ, fly.yaw, fly.pitch, 0.0);
}

fn move_camera(
    keys: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    transform: Option<Single<&mut Transform, With<FlyCamera>>>,
) {
    let Some(mut transform) = transform else {
        return;
    };
    let mut direction = Vec3::ZERO;

    if keys.pressed(KeyCode::KeyW) {
        direction += *transform.forward();
    }
    if keys.pressed(KeyCode::KeyS) {
        direction += *transform.back();
    }
    if keys.pressed(KeyCode::KeyA) {
        direction += *transform.left();
    }
    if keys.pressed(KeyCode::KeyD) {
        direction += *transform.right();
    }
    if keys.pressed(KeyCode::KeyE) {
        direction += Vec3::Y;
    }
    if keys.pressed(KeyCode::KeyQ) {
        direction -= Vec3::Y;
    }

    let Some(direction) = direction.try_normalize() else {
        return;
    };

    let speed = if keys.pressed(KeyCode::ShiftLeft) {
        MOVE_SPEED * SPRINT_MULTIPLIER
    } else {
        MOVE_SPEED
    };

    transform.translation += direction * speed * time.delta_secs();
}
