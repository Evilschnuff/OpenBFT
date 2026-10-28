//! The sensor window's camera: "a close-up view of the area around the
//! Targeting Reticle" (manual p. 19).
//!
//! A magnified part of the main view: a second [`Camera3d`] at the chase
//! camera's eye, looking along the ray through the cursor — the reticle —
//! with the chase camera's field of view narrowed by `SENSOR`'s zoom. That
//! is exactly the main view's square around the reticle, blown up, but
//! rendered at the window's own resolution instead of scaled up from a
//! quarter as many pixels. From the tank's muzzle instead, a mortar pointed
//! up at the sky would show nothing.
//!
//! # Cost
//!
//! Cropping the main image would be free but blurry: a 4× zoom shows a
//! sixteenth of the screen's pixels, so the window would be 4×4 blocks. A
//! second pass is cheap here instead. Its target is the sensor view's size
//! on screen in physical pixels (a few hundred square), its frustum is a
//! sixteenth of the main one's so culling leaves little to draw, and it is
//! off whenever the HUD is hidden. It renders into its own target before
//! the UI camera, so the UI never draws into it.
//!
//! It draws everything but the player's tank, as the original's did: the
//! tank's meshes are on [`PLAYER_LAYER`], which every other camera and every
//! light also has, and the sensor camera does not.
//!
//! Every system that looks for "the" 3D camera — sky dome, LOD, billboards,
//! the audio listener — filters this one out with [`MainCamera3d`].

use bevy::camera::RenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::light::{DirectionalLight, PointLight, SpotLight};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::window::PrimaryWindow;
use bft_sim::vehicle::PlayerVehicle;

use super::original::CurrentHudFrame;
use super::{HudRoot, HudStyle};
use crate::aim::{ExternalAim, aim_ray};
use crate::chase_camera::ChaseCamera;

/// The sensor window's camera.
#[derive(Component, Debug, Clone, Copy)]
pub struct SensorCamera;

/// A [`Camera3d`] that is not the view the player looks through, added by a
/// plugin outside this crate: a VR headset's eye cameras, which ride along
/// with the main view rather than replace it.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct AuxiliaryCamera;

/// A query filter for "the" 3D camera the player looks through: any
/// [`Camera3d`] but the sensor's and the [`AuxiliaryCamera`]s.
pub type MainCamera3d = (
    With<Camera3d>,
    Without<SensorCamera>,
    Without<AuxiliaryCamera>,
);

/// The image the sensor camera draws into, shared with the HUD node that
/// shows it.
#[derive(Resource, Debug, Clone)]
pub struct SensorImage(pub Handle<Image>);

/// A render target of `size` physical pixels.
pub fn target_image(size: UVec2) -> Image {
    Image::new_target_texture(
        size.x.max(1),
        size.y.max(1),
        TextureFormat::Bgra8UnormSrgb,
        None,
    )
}

/// The sensor's field of view for the main view's vertical `fov`.
///
/// `SENSOR`'s zoom is a magnification of *screen pixels*: the window, `view`
/// canvas pixels tall, shows a patch `view / zoom` tall of the main view,
/// which is `canvas` pixels tall. So the tangent of the half-angle shrinks
/// by `zoom × canvas / view` — about 19 for the shipped numbers, not 4. A
/// zoom by angle alone showed a quarter of the screen height in a window a
/// fifth of it tall, which is no magnification at all.
pub fn zoomed_fov(fov: f32, zoom: f32, view: f32, canvas: f32) -> f32 {
    let magnification = (zoom * canvas / view.max(1.0)).max(1.0);
    2.0 * ((fov / 2.0).tan() / magnification).atan()
}

/// The sensor's pose: at the main camera's eye, looking along `ray` (the
/// ray through the cursor), keeping the main camera's up.
pub fn sensor_pose(eye: &GlobalTransform, ray: Dir3) -> Transform {
    Transform::from_translation(eye.translation()).looking_to(ray, eye.up())
}

/// The render layer the player's own meshes are on, so the sensor can leave
/// them out.
pub const PLAYER_LAYER: usize = 1;

/// The layers everything but the sensor camera sees: the world and the
/// player.
pub fn world_and_player() -> RenderLayers {
    RenderLayers::from_layers(&[0, PLAYER_LAYER])
}

/// Put the player's meshes on [`PLAYER_LAYER`], including parts that
/// appear later (the model is re-parented onto the hull after it spawns).
pub fn layer_player_meshes(
    mut commands: Commands,
    players: Query<Entity, With<PlayerVehicle>>,
    children: Query<&Children>,
    meshes: Query<Option<&RenderLayers>, With<Mesh3d>>,
) {
    let player = RenderLayers::layer(PLAYER_LAYER);
    for root in &players {
        for entity in std::iter::once(root).chain(children.iter_descendants(root)) {
            if let Ok(layers) = meshes.get(entity)
                && layers != Some(&player)
            {
                commands.entity(entity).insert(player.clone());
            }
        }
    }
}

/// Let every camera but the sensor, and every light, see [`PLAYER_LAYER`]
/// as well as the world, so the tank is drawn, lit and casts its shadow as
/// before.
#[allow(clippy::type_complexity)]
pub fn show_player_layer(
    mut commands: Commands,
    viewers: Query<
        (Entity, Option<&RenderLayers>),
        (
            Or<(
                MainCamera3d,
                With<DirectionalLight>,
                With<PointLight>,
                With<SpotLight>,
            )>,
            Without<SensorCamera>,
        ),
    >,
) {
    let wanted = world_and_player();
    for (entity, layers) in &viewers {
        if layers != Some(&wanted) {
            commands.entity(entity).insert(wanted.clone());
        }
    }
}

/// Make the image the HUD shows, once.
pub fn create_sensor_image(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    commands.insert_resource(SensorImage(images.add(target_image(UVec2::ONE))));
}

/// Spawn, aim, size and switch the sensor camera.
// Ten parameters: the two cameras, the HUD's state, the window, the aim and
// the image store.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn drive_sensor_camera(
    mut commands: Commands,
    style: Res<HudStyle>,
    image: Option<Res<SensorImage>>,
    frame: Option<Res<CurrentHudFrame>>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    external: Option<Res<ExternalAim>>,
    chase: Query<
        (&Camera, &GlobalTransform, &Projection),
        (With<ChaseCamera>, Without<SensorCamera>),
    >,
    huds: Query<&InheritedVisibility, With<HudRoot>>,
    mut sensors: Query<
        (Entity, &mut Camera, &mut Transform, &mut Projection),
        (With<SensorCamera>, Without<ChaseCamera>),
    >,
    mut images: ResMut<Assets<Image>>,
) {
    let (HudStyle::Original(hud), Some(image), Some(frame)) = (&*style, image, frame) else {
        for (entity, ..) in &sensors {
            commands.entity(entity).despawn();
        }
        return;
    };

    // The target follows the view's size on screen.
    let scale_factor = window.as_ref().map_or(1.0, |window| window.scale_factor());
    let wanted = (hud.layout.sensor.view.as_rect().size() * frame.0.scale * scale_factor)
        .round()
        .as_uvec2()
        .max(UVec2::ONE);
    if images
        .get(&image.0)
        .is_some_and(|current| current.size() != wanted)
        && let Some(mut current) = images.get_mut(&image.0)
    {
        current.resize(bevy::render::render_resource::Extent3d {
            width: wanted.x,
            height: wanted.y,
            depth_or_array_layers: 1,
        });
    }

    let visible = huds.iter().any(|visibility| visibility.get());
    let Some((main_camera, eye, main_projection)) = chase.iter().next() else {
        for (entity, ..) in &sensors {
            commands.entity(entity).despawn();
        }
        return;
    };

    if sensors.is_empty() {
        commands.spawn((
            Name::new("sensor camera"),
            SensorCamera,
            Camera3d::default(),
            Camera {
                // Before the chase camera and the UI; it has a target of its
                // own, so the order only sequences the work.
                order: -1,
                is_active: false,
                ..default()
            },
            RenderTarget::Image(image.0.clone().into()),
            Projection::Perspective(PerspectiveProjection::default()),
            Transform::default(),
            DespawnOnExit(bft_sim::states::AppState::InGame),
        ));
        return;
    }

    for (_, mut camera, mut transform, mut projection) in &mut sensors {
        if camera.is_active != visible {
            camera.is_active = visible;
        }
        // With the pointer out of the window the view holds where it was,
        // as the turret does.
        if let Some(ray) = aim_ray(
            external.as_deref(),
            window.as_deref().copied(),
            main_camera,
            eye,
        ) {
            *transform = sensor_pose(eye, ray.direction);
        }

        let fov = match main_projection {
            Projection::Perspective(perspective) => perspective.fov,
            _ => PerspectiveProjection::default().fov,
        };
        if let Projection::Perspective(perspective) = &mut *projection {
            perspective.fov = zoomed_fov(
                fov,
                hud.layout.sensor.zoom,
                hud.layout.sensor.view.as_rect().height(),
                hud.canvas().y,
            );
            perspective.aspect_ratio = 1.0;
            if let Projection::Perspective(main) = main_projection {
                perspective.near = main.near;
                perspective.far = main.far;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tank's meshes, however deep, go on the player's layer; the main
    /// camera and the lights see it; the sensor camera does not.
    #[test]
    fn the_sensor_leaves_the_tank_out() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let hull = world.spawn(PlayerVehicle).id();
        let turret = world.spawn(ChildOf(hull)).id();
        let barrel = world
            .spawn((Mesh3d(Handle::default()), ChildOf(turret)))
            .id();
        let tree = world.spawn(Mesh3d(Handle::default())).id();
        let main = world.spawn(Camera3d::default()).id();
        let sun = world.spawn(DirectionalLight::default()).id();
        let sensor = world.spawn((Camera3d::default(), SensorCamera)).id();

        world.run_system_once(layer_player_meshes).expect("runs");
        world.run_system_once(show_player_layer).expect("runs");

        let layers = |world: &World, entity| world.get::<RenderLayers>(entity).cloned();
        assert_eq!(
            layers(&world, barrel),
            Some(RenderLayers::layer(PLAYER_LAYER))
        );
        assert_eq!(layers(&world, tree), None, "the world stays on layer 0");
        assert_eq!(layers(&world, main), Some(world_and_player()));
        assert_eq!(layers(&world, sun), Some(world_and_player()));
        assert!(
            !layers(&world, sensor)
                .unwrap_or_default()
                .intersects(&RenderLayers::layer(PLAYER_LAYER))
        );
    }

    #[test]
    fn the_zoom_magnifies_screen_pixels() {
        let fov = 60f32.to_radians();
        // A window 120 lines tall on a 480-line screen at zoom 4 shows a
        // patch 30 lines tall: the tangent shrinks 16 times.
        let zoomed = zoomed_fov(fov, 4.0, 120.0, 480.0);
        assert!(((zoomed / 2.0).tan() * 16.0 - (fov / 2.0).tan()).abs() < 1e-6);
        assert_eq!(
            zoomed_fov(fov, 0.5, 480.0, 480.0),
            fov,
            "never wider than the main view"
        );
    }

    #[test]
    fn the_sensor_looks_from_the_eye_along_the_cursor_ray() {
        let eye = GlobalTransform::from(
            Transform::from_xyz(1.0, 5.0, 10.0).looking_at(Vec3::ZERO, Vec3::Y),
        );
        let ray = Dir3::new(Vec3::new(0.2, -0.3, -1.0)).expect("a direction");
        let pose = sensor_pose(&eye, ray);
        assert_eq!(pose.translation, eye.translation());
        assert!((pose.forward().as_vec3() - ray.as_vec3()).length() < 1e-5);
    }
}
