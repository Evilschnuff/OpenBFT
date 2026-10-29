//! VR over OpenXR: the game seen through a headset, layered over the flat
//! presentation without changing it.
//!
//! # Optional by construction
//!
//! Only `bft-bin` depends on this crate, behind its `vr` feature, and uses
//! it only for `play --vr` and `view --vr`; nothing in `bft-sim`,
//! `bft-assets` or `bft-game` names it (DESIGN §10). What it takes from
//! `bft-game` are four hooks that know nothing of VR:
//! [`AuxiliaryCamera`] keeps the eye cameras out of "the" main camera,
//! [`ExternalAim`] lets the gaze aim instead of the cursor,
//! [`ExternalActions`](bft_game::ui::ExternalActions) lets the controllers
//! press actions, and [`UiCamera`] is the UI camera to retarget. With no OpenXR runtime
//! answering, [`with_xr`] falls back to the flat renderer and [`VrPlugin`]
//! adds nothing, so a `--vr` run is then the flat game.
//!
//! # The seated view
//!
//! The headset's tracking space rides on the main camera (the chase camera
//! in play, the free-fly camera in the viewer), taking its position and its
//! yaw but not its pitch: the head does the pitching, and a world that tilts
//! under a still head is the quickest way to make a player ill. The flat
//! camera keeps drawing to the window as a spectator view, and everything
//! that follows "the" camera (sky dome, LOD, billboards, the listener) keeps
//! following it; the head is within a metre of it.
//!
//! The turret aims along a beam from the right controller ([`pointer`]),
//! or, switched with a click of the right stick ([`AimSource`]), where the
//! eyes look when the headset tracks them ([`eyes`]) and where the head
//! points otherwise. Either way the ray goes to `bft-game` as the
//! [`ExternalAim`], with a reticle where it lands; the head is also the
//! fallback while the controller is not tracked. The controllers press the game's actions ([`controllers`]):
//! drive on the left stick, fire on the right trigger. The UI (HUD, menus,
//! dialogs) draws into an image that only the eyes see: in play the HUD's
//! clusters each on their own tile, the bottom row's riding on the
//! controllers ([`hud`]), and with a
//! menu up the whole image on one panel in front of it, clicked with a beam
//! from the right controller ([`pointer`]).
//!
//! Recoil had no VR, so none of this rests on the original: every placement
//! here is a presentation choice, and the constants below say which.

#![cfg(not(target_os = "macos"))]

mod controllers;
mod eyes;
mod hands;
mod hud;
mod pointer;

use bevy::app::PluginGroupBuilder;
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::RenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::prelude::*;
use bevy::render::pipelined_rendering::PipelinedRenderingPlugin;
use bevy::render::render_resource::{
    AsBindGroup, CompareFunction, Extent3d, RenderPipelineDescriptor, SpecializedMeshPipelineError,
    TextureDimension, TextureFormat,
};
use bevy::shader::ShaderRef;
use bevy::transform::TransformSystems;
use bevy::window::{PresentMode, PrimaryWindow};
use bevy_mod_openxr::add_xr_plugins;
use bevy_mod_openxr::exts::{OxrEnabledExtensions, OxrExtensions};
use bevy_mod_openxr::init::OxrInitPlugin;
use bevy_mod_openxr::reference_space::OxrReferenceSpacePlugin;
use bevy_mod_openxr::spaces::OxrSpaceLocationFlags;
use bevy_mod_xr::camera::XrCamera;
use bevy_mod_xr::session::{XrState, XrTrackingRoot};
use bft_game::UiCamera;
use bft_game::aim::{ExternalAim, aim_point};
use bft_game::ui::hud::sensor::{AuxiliaryCamera, MainCamera3d, world_and_player};
use bft_sim::vehicle::{Controlled, PlayerInput};

/// The render layer only the eye cameras see: the UI panel and the
/// reticle, which the spectator window and the sensor must not draw.
pub const PANEL_LAYER: usize = 2;

/// How far in front of the seat the UI panel hangs, metres.
///
/// A presentation choice: near enough to read the HUD's small print at a
/// headset's resolution, far enough that the eyes need not cross for it.
const PANEL_DISTANCE: f32 = 2.0;

/// How wide the UI panel is, metres; its height follows the window's shape.
///
/// A presentation choice: about 58° across at [`PANEL_DISTANCE`], so the
/// HUD's corners are a glance away and not a turn of the head.
const PANEL_WIDTH: f32 = 2.2;

/// How large the reticle looks: its width over its distance from the head.
///
/// A presentation choice: about 1.7°, a little larger than a cursor on a
/// monitor at arm's length.
const RETICLE_SIZE: f32 = 0.03;

/// `defaults` with OpenXR rendering in place of the flat renderer.
///
/// When no OpenXR runtime answers this falls back to the flat renderer by
/// itself, and [`VrPlugin`] then stays out of the way.
pub fn with_xr<G: PluginGroup>(defaults: G) -> PluginGroupBuilder {
    let mut defaults = defaults.build();
    // Pipelined rendering draws one frame behind the simulation. In a
    // headset that is a frame between turning the head and the view
    // following, which is worth more than the throughput.
    if defaults.contains::<PipelinedRenderingPlugin>() {
        defaults = defaults.disable::<PipelinedRenderingPlugin>();
    }
    // Asked for, not required: the runtime enables what it has, and the
    // rest is a warning in the log.
    // Not hand tracking, which the library asks for by default: nothing
    // here uses it, and SteamVR logs an error for every frame it is asked
    // for hands it cannot see.
    let mut exts = OxrExtensions::default();
    exts.ext_eye_gaze_interaction = true;
    add_xr_plugins(defaults)
        .set(OxrInitPlugin { exts, ..default() })
        .set(OxrReferenceSpacePlugin {
            // Seated: the origin is where the head was at the last recentre,
            // not the floor of a room-scale play area.
            default_primary_ref_space: openxr::ReferenceSpaceType::LOCAL,
        })
}

/// The seated VR view: rig, gaze aim, reticle and UI panel.
///
/// Add it after `GamePlugin`, in an app built from [`with_xr`].
pub struct VrPlugin;

impl Plugin for VrPlugin {
    fn build(&self, app: &mut App) {
        // `with_xr` has already tried the runtime by now; its plugins insert
        // `Unavailable` while they are built when none answered.
        let available = app
            .world()
            .get_resource::<XrState>()
            .is_some_and(|state| *state != XrState::Unavailable);
        if !available {
            warn!("no OpenXR runtime answered; running without VR");
            return;
        }

        let eye_gaze = app
            .world()
            .get_resource::<OxrEnabledExtensions>()
            .is_some_and(|exts| exts.ext_eye_gaze_interaction);
        if eye_gaze {
            info!("eye tracking: the runtime offers eye gaze");
            app.add_plugins(eyes::EyeGazePlugin);
        } else {
            info!("eye tracking: the runtime offers no eye gaze; aiming with the head");
        }

        embedded_asset!(app, "panel.wgsl");
        app.add_plugins(MaterialPlugin::<PanelMaterial>::default())
            // On the eye cameras from the moment they exist, so no system
            // that looks for "the" camera ever finds one of them instead.
            .register_required_components::<XrCamera, AuxiliaryCamera>()
            .register_required_components_with::<XrCamera, RenderLayers>(|| {
                world_and_player().with(PANEL_LAYER)
            })
            .init_resource::<ExternalAim>()
            .init_resource::<AimSource>()
            .add_plugins((
                controllers::ControllersPlugin,
                pointer::PointerPlugin,
                hands::HandsPlugin,
            ))
            .add_systems(Startup, (spawn_panel, spawn_reticle, uncap_window))
            .add_plugins(hud::HudTilesPlugin)
            .add_systems(
                Update,
                (
                    choose_aim.after(pointer::HandRaySet).before(aim_point),
                    place_reticle.after(aim_point),
                    show_ui_on_panel,
                    fit_panel_to_window,
                ),
            )
            .add_systems(
                PostUpdate,
                follow_main_camera.before(TransformSystems::Propagate),
            );
    }
}

/// An unlit textured quad drawn over everything: the UI panel, the HUD
/// tiles and the reticle.
///
/// The depth test is off. The panel hangs two metres out, and the near
/// chase offsets put the player's own tank closer than that; a panel that
/// was depth tested would vanish into the hull.
#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub(crate) struct PanelMaterial {
    #[texture(0)]
    #[sampler(1)]
    texture: Handle<Image>,
    /// The part of the texture the quad shows, in UV: the corner, then the
    /// size.
    #[uniform(2)]
    pub(crate) uv_rect: Vec4,
}

impl PanelMaterial {
    /// Showing all of `texture`.
    pub(crate) fn whole(texture: Handle<Image>) -> PanelMaterial {
        PanelMaterial {
            texture,
            uv_rect: Vec4::new(0.0, 0.0, 1.0, 1.0),
        }
    }
}

impl Material for PanelMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://bft_xr/panel.wgsl".into()
    }

    // The UI blends into a transparent image with ordinary alpha blending,
    // which leaves its colours premultiplied.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Premultiplied
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _pipeline: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _layout: &MeshVertexBufferLayoutRef,
        _key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if let Some(depth) = descriptor.depth_stencil.as_mut() {
            depth.depth_compare = Some(CompareFunction::Always);
            depth.depth_write_enabled = Some(false);
        }
        Ok(())
    }
}

/// The UI panel: its entity, the image the UI draws into and the material
/// that shows it.
#[derive(Resource, Debug)]
pub(crate) struct Panel {
    pub(crate) entity: Entity,
    pub(crate) image: Handle<Image>,
    material: Handle<PanelMaterial>,
}

/// The reticle at the end of the gaze.
#[derive(Component, Debug)]
struct Reticle;

/// Hang the UI panel in front of the seat.
pub(crate) fn spawn_panel(
    mut commands: Commands,
    root: Single<Entity, With<XrTrackingRoot>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<PanelMaterial>>,
) {
    // One pixel until `fit_panel_to_window` sees the window.
    let image = images.add(Image::new_target_texture(
        1,
        1,
        TextureFormat::Bgra8UnormSrgb,
        None,
    ));
    let material = materials.add(PanelMaterial::whole(image.clone()));
    let entity = commands
        .spawn((
            Name::new("VR UI panel"),
            Mesh3d(meshes.add(Rectangle::new(1.0, 1.0))),
            MeshMaterial3d(material.clone()),
            Transform::from_xyz(0.0, 0.0, -PANEL_DISTANCE),
            RenderLayers::layer(PANEL_LAYER),
            ChildOf(*root),
        ))
        .id();
    commands.insert_resource(Panel {
        entity,
        image,
        material,
    });
}

/// Make the reticle, hidden until there is an aim point to put it on.
fn spawn_reticle(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<PanelMaterial>>,
) {
    commands.spawn((
        Name::new("VR reticle"),
        Reticle,
        Mesh3d(meshes.add(Rectangle::new(1.0, 1.0))),
        MeshMaterial3d(materials.add(PanelMaterial::whole(images.add(ring_image())))),
        Transform::default(),
        Visibility::Hidden,
        RenderLayers::layer(PANEL_LAYER),
    ));
}

/// A white ring with a dark edge, premultiplied, so it reads against both
/// the fog and the ground.
pub(crate) fn ring_image() -> Image {
    const SIZE: u32 = 64;
    let centre = (SIZE as f32 - 1.0) / 2.0;
    let pixel = 1.0 / centre;
    let mut data = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let radius = Vec2::new(x as f32 - centre, y as f32 - centre).length() / centre;
            let off_ring = (radius - 0.75).abs();
            // A pixel of soft edge on each boundary.
            let coverage = |half_width: f32| ((half_width - off_ring) / pixel).clamp(0.0, 1.0);
            let white = coverage(0.08);
            let alpha = coverage(0.14);
            let to_byte = |value: f32| (value * 255.0).round() as u8;
            data.extend_from_slice(&[
                to_byte(white),
                to_byte(white),
                to_byte(white),
                to_byte(alpha),
            ]);
        }
    }
    Image::new(
        Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// Let the window present as fast as the headset runs.
///
/// A vsynced window holds every frame to the monitor's refresh, typically
/// 60 Hz, while the headset wants 90 or more. The OpenXR runtime paces the
/// frame instead.
fn uncap_window(mut windows: Query<&mut Window, With<PrimaryWindow>>) {
    for mut window in &mut windows {
        window.present_mode = PresentMode::AutoNoVsync;
    }
}

/// The head's pose: midway between the eyes, facing halfway between them.
///
/// Last frame's: the eyes are posed in `PostUpdate`. For aiming that is a
/// frame of lag nobody can see.
fn head(eyes: &Query<&GlobalTransform, With<XrCamera>>) -> Option<Transform> {
    let mut eyes = eyes.iter().map(GlobalTransform::compute_transform);
    let first = eyes.next()?;
    Some(match eyes.next() {
        Some(second) => Transform::from_translation(first.translation.midpoint(second.translation))
            .with_rotation(first.rotation.slerp(second.rotation, 0.5)),
        None => first,
    })
}

/// What the turret aims with.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum AimSource {
    /// A beam from the right controller.
    #[default]
    Hand,
    /// The gaze: the eyes when the runtime locates them, the head otherwise.
    Head,
}

/// Aim along the hand's beam or the gaze, as the [`AimSource`] says; along
/// the gaze while the hand is not tracked.
fn choose_aim(
    source: Res<AimSource>,
    hand: Res<pointer::HandRay>,
    eyes: Query<&GlobalTransform, With<XrCamera>>,
    gazes: Query<(&GlobalTransform, &OxrSpaceLocationFlags), With<eyes::EyeGaze>>,
    mut aim: ResMut<ExternalAim>,
) {
    let hand = hand.world.filter(|_| *source == AimSource::Hand);
    aim.0 = hand.or_else(|| {
        let gaze = eyes::located_gaze(&gazes).or_else(|| head(&eyes))?;
        Some(Ray3d::new(gaze.translation, gaze.forward()))
    });
}

/// Put the reticle on the local player's aim point, facing the head, at a
/// size that looks the same at any distance.
fn place_reticle(
    eyes: Query<&GlobalTransform, With<XrCamera>>,
    inputs: Query<(&PlayerInput, &Controlled)>,
    mut reticles: Query<(&mut Transform, &mut Visibility), With<Reticle>>,
) {
    let aim = inputs
        .iter()
        .find(|(_, controlled)| **controlled == Controlled::Local)
        .and_then(|(input, _)| input.aim_point);
    let placed = head(&eyes).zip(aim).and_then(|(head, point)| {
        let distance = head.translation.distance(point);
        (distance > f32::EPSILON).then(|| {
            // Looking away from the head turns the quad's face toward it.
            Transform::from_translation(point)
                .looking_to(point - head.translation, head.up())
                .with_scale(Vec3::splat(distance * RETICLE_SIZE))
        })
    });

    for (mut transform, mut visibility) in &mut reticles {
        match placed {
            Some(placed) => {
                *transform = placed;
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

/// Draw the UI into the panel's image instead of the window.
#[allow(clippy::type_complexity)]
fn show_ui_on_panel(
    mut commands: Commands,
    panel: Res<Panel>,
    mut cameras: Query<(Entity, &mut Camera), (With<UiCamera>, Without<IsDefaultUiCamera>)>,
) {
    for (entity, mut camera) in &mut cameras {
        // Over the window the UI draws onto the level; the image has nothing
        // under it, so it clears itself to transparent or last frame's HUD
        // would smear.
        camera.clear_color = ClearColorConfig::Custom(Color::NONE);
        // Default UI camera, because Bevy otherwise only picks a camera that
        // draws to the window.
        commands.entity(entity).insert((
            RenderTarget::Image(panel.image.clone().into()),
            IsDefaultUiCamera,
        ));
    }
}

/// Keep the panel's image the window's size, and the panel its shape.
///
/// The HUD and the dialogs lay themselves out in the window's logical
/// pixels. An image target's pixels are logical pixels too (its scale factor
/// is 1), so an image the window's logical size takes their layout as is.
fn fit_panel_to_window(
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    panel: Res<Panel>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<PanelMaterial>>,
    mut transforms: Query<&mut Transform>,
) {
    let Some(window) = window else {
        return;
    };
    let wanted = window.size().round().as_uvec2().max(UVec2::ONE);
    if images
        .get(&panel.image)
        .is_some_and(|current| current.size() != wanted)
        && let Some(mut image) = images.get_mut(&panel.image)
    {
        image.resize(Extent3d {
            width: wanted.x,
            height: wanted.y,
            depth_or_array_layers: 1,
        });
        // Touched so its bind group is rebuilt on the resized texture.
        materials.get_mut(&panel.material);
    }

    let scale = Vec3::new(
        PANEL_WIDTH,
        PANEL_WIDTH * wanted.y as f32 / wanted.x as f32,
        1.0,
    );
    if let Ok(mut transform) = transforms.get_mut(panel.entity)
        && transform.scale != scale
    {
        transform.scale = scale;
    }
}

/// The main camera's world pose as of now, for [`seat_pose`].
///
/// Its own transform when it has no parent, so a camera moved this frame
/// counts before propagation; a parented camera's own transform is not its
/// world one, so that case settles for last frame's.
pub(crate) type MainCameras<'w, 's> = Query<
    'w,
    's,
    (&'static Transform, &'static GlobalTransform, Has<ChildOf>),
    (MainCamera3d, Without<XrTrackingRoot>),
>;

/// The main camera's pose, if there is one (see [`MainCameras`]).
pub(crate) fn main_camera_pose(cameras: &MainCameras) -> Option<Transform> {
    let (local, global, parented) = cameras.iter().next()?;
    Some(if parented {
        global.compute_transform()
    } else {
        *local
    })
}

/// Where the seat goes for a camera at `camera`: its position, and its yaw
/// about world up. Looking straight up or down there is no yaw to take, so
/// `previous`'s stays.
pub(crate) fn seat_pose(camera: Transform, previous: Transform) -> Transform {
    let rotation = Dir3::new(camera.forward().as_vec3().with_y(0.0))
        .map(|forward| Transform::default().looking_to(forward, Vec3::Y).rotation)
        .unwrap_or(previous.rotation);
    Transform {
        translation: camera.translation,
        rotation,
        scale: previous.scale,
    }
}

/// Carry the tracking space with the main camera: its position, and its
/// yaw about world up.
///
/// Before propagation, so the seat moves with the camera this frame rather
/// than a frame behind it.
pub(crate) fn follow_main_camera(
    cameras: MainCameras,
    mut roots: Query<&mut Transform, With<XrTrackingRoot>>,
) {
    let Some(camera) = main_camera_pose(&cameras) else {
        return;
    };
    for mut root in &mut roots {
        *root = seat_pose(camera, *root);
    }
}
