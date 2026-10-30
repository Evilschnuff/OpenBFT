//! The menu pointer: a beam from the right controller onto the UI panel,
//! clicking with the trigger.
//!
//! The UI draws into the panel's image, and Bevy's picking takes pointers on
//! an image as readily as on a window. So the beam is a picking pointer of
//! its own: where it meets the panel becomes a position in the image, and
//! the trigger its primary button. The menus, dialogs and sliders react to
//! it as they do to the mouse.
//!
//! The pointer works only while the whole UI is on the panel (menus,
//! dialogs, result screens); in play the HUD is on its tiles and the
//! trigger fires. While the turret aims with the hand ([`AimSource`]) the
//! beam then runs to the aim point instead, where the reticle sits. The beam
//! and its dot draw over everything, like the panel.
//!
//! The hand's aim is smoothed ([`HandRay`]) in the seat's space, so a hand
//! held still holds the beam still while the seat moves with the tank. The
//! beam and dot are children of the seat too, placed once the seat has
//! caught up with the camera this frame: placed in the world from the
//! seat's last position instead, the beam's near end would swing by a
//! frame's travel of the tank.
//!
//! Recoil had no VR, so the beam's look, its reach and the smoothing are
//! presentation choices.

use bevy::asset::RenderAssetUsages;
use bevy::asset::uuid::Uuid;
use bevy::camera::NormalizedRenderTarget;
use bevy::camera::visibility::RenderLayers;
use bevy::ecs::query::QueryFilter;
use bevy::picking::pointer::{Location, PointerAction, PointerButton, PointerId, PointerInput};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::transform::TransformSystems;
use bevy_mod_openxr::action_binding::{OxrSendActionBindings, OxrSuggestActionBinding};
use bevy_mod_openxr::action_set_attaching::OxrAttachActionSet;
use bevy_mod_openxr::action_set_syncing::{OxrActionSetSyncSet, OxrSyncActionSet};
use bevy_mod_openxr::openxr_session_running;
use bevy_mod_openxr::resources::OxrInstance;
use bevy_mod_openxr::session::OxrSession;
use bevy_mod_openxr::spaces::OxrSpaceLocationFlags;
use bevy_mod_xr::session::{XrSessionCreated, XrTrackingRoot, session_available};
use bft_game::guided_view::GuidedMissile;
use bft_sim::vehicle::{Controlled, PlayerInput};

use crate::assist::AimLock;
use crate::table::Table;
use crate::{
    AimSource, MainCameras, PANEL_LAYER, Panel, PanelMaterial, View, ring_image, root_pose_now,
};

/// The beam's picking pointer.
const POINTER: PointerId =
    PointerId::Custom(Uuid::from_u128(0x6f70_656e_6266_742d_7672_2d70_6f69_6e74));

/// Where the pointer goes when the beam is off the panel: outside the
/// image, so nothing stays hovered.
const OFF_PANEL: Vec2 = Vec2::splat(-1.0);

/// How thick the beam is, metres.
const BEAM_WIDTH: f32 = 0.003;

/// How far the beam reaches when it meets nothing, metres: about to the
/// panel, so it reads as the same pointer either way.
const BEAM_REACH: f32 = 2.0;

/// How wide the dot where the beam meets the panel is, metres.
const DOT_SIZE: f32 = 0.025;

/// How far the dot sits in front of the panel, metres, so the two do not
/// fight over the same depth.
const DOT_LIFT: f32 = 0.005;

/// How long the hand's aim takes to settle when held still, seconds: the
/// time constant that smooths away the tremor of a hand in the air.
///
/// A presentation choice: long enough to steady a beam reaching a hundred
/// metres, short enough not to feel like dragging it.
const STEADY_TIME: f32 = 0.05;

/// How fast the aim has to swing, radians a second, before the smoothing
/// lets go entirely, so a sweep of the hand is not trailed.
const SWEEP_SPEED: f32 = 3.0;

/// The Oculus Touch profile, which SteamVR remaps onto the Steam Frame's
/// controllers.
const TOUCH: &str = "/interaction_profiles/oculus/touch_controller";

/// The Khronos simple controller.
const SIMPLE: &str = "/interaction_profiles/khr/simple_controller";

/// The pointer's actions and the set they are in.
#[derive(Resource)]
struct PointerActions {
    set: openxr::ActionSet,
    /// The right controller's aim: where the beam starts, and its direction.
    aim: openxr::Action<openxr::Posef>,
    /// The click.
    select: openxr::Action<bool>,
}

/// The located aim space of the right controller.
#[derive(Component, Debug)]
struct PointerAim;

/// The beam.
#[derive(Component, Debug)]
struct Beam;

/// The dot where the beam meets the panel.
#[derive(Component, Debug)]
struct Dot;

/// The right controller's aim, smoothed: where the beam starts and where it
/// points. `None` while the controller is not tracked.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub(crate) struct HandRay {
    /// In the seat's space.
    seat: Option<Transform>,
    /// In the world, by where the seat goes this frame: for aiming.
    pub(crate) world: Option<Ray3d>,
}

/// Where [`HandRay`] is brought up to date, for the systems that read it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct HandRaySet;

/// Points at the UI panel and clicks on it.
pub(crate) struct PointerPlugin;

impl Plugin for PointerPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            Startup,
            (create_actions.run_if(session_available), spawn_beam),
        )
        .add_systems(OxrSendActionBindings, suggest_bindings)
        .add_systems(XrSessionCreated, attach_and_spawn)
        .add_systems(
            PreUpdate,
            sync_actions
                .before(OxrActionSetSyncSet)
                .run_if(openxr_session_running),
        )
        .init_resource::<HandRay>()
        .add_systems(
            Update,
            (
                // After the chase camera has moved, so the seat's pose
                // below is this frame's.
                smooth_hand
                    .in_set(HandRaySet)
                    .after(bft_game::chase_camera::follow_target),
                point.after(HandRaySet),
            )
                .run_if(openxr_session_running),
        )
        .add_systems(
            PostUpdate,
            place_beam
                .after(crate::follow_main_camera)
                .after(crate::table::place_table)
                .before(TransformSystems::Propagate)
                .run_if(openxr_session_running),
        );
    }
}

/// Make the action set and its actions.
fn create_actions(instance: Res<OxrInstance>, mut commands: Commands) {
    let created = instance
        .create_action_set("menus", "Menus", 0)
        .and_then(|set| {
            Ok(PointerActions {
                aim: set.create_action("pointer", "Menu pointer", &[])?,
                select: set.create_action("select", "Select in menus", &[])?,
                set,
            })
        });
    match created {
        Ok(actions) => commands.insert_resource(actions),
        Err(error) => warn!("pointer: could not create the actions: {error}"),
    }
}

/// The right controller aims and its trigger clicks.
fn suggest_bindings(
    actions: Option<Res<PointerActions>>,
    mut bindings: MessageWriter<OxrSuggestActionBinding>,
) {
    let Some(actions) = actions else {
        return;
    };
    for (action, profile, path) in [
        (
            actions.aim.as_raw(),
            TOUCH,
            "/user/hand/right/input/aim/pose",
        ),
        (
            actions.select.as_raw(),
            TOUCH,
            "/user/hand/right/input/trigger/value",
        ),
        (
            actions.aim.as_raw(),
            SIMPLE,
            "/user/hand/right/input/aim/pose",
        ),
        (
            actions.select.as_raw(),
            SIMPLE,
            "/user/hand/right/input/select/click",
        ),
    ] {
        bindings.write(OxrSuggestActionBinding {
            action,
            interaction_profile: profile.into(),
            bindings: vec![path.into()],
        });
    }
}

/// Attach the set to the new session, make the aim's space and the picking
/// pointer.
fn attach_and_spawn(
    actions: Option<Res<PointerActions>>,
    session: Res<OxrSession>,
    mut attach: MessageWriter<OxrAttachActionSet>,
    mut commands: Commands,
) {
    let Some(actions) = actions else {
        return;
    };
    attach.write(OxrAttachActionSet(actions.set.clone()));
    match session.create_action_space(&actions.aim, openxr::Path::NULL, Isometry3d::IDENTITY) {
        // `XrSpace` makes it a child of the tracking root, located in the
        // seat's space every frame.
        Ok(space) => {
            commands.spawn((Name::new("VR pointer aim"), PointerAim, space));
            commands.spawn((Name::new("VR pointer"), POINTER));
        }
        Err(error) => warn!("pointer: could not create the aim space: {error}"),
    }
}

/// Ask the runtime for this frame's aim and trigger.
fn sync_actions(actions: Option<Res<PointerActions>>, mut sync: MessageWriter<OxrSyncActionSet>) {
    if let Some(actions) = actions {
        sync.write(OxrSyncActionSet(actions.set.clone()));
    }
}

/// Make the beam and the dot, hidden until there is something to point at.
fn spawn_beam(
    mut commands: Commands,
    root: Single<Entity, With<XrTrackingRoot>>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<PanelMaterial>>,
) {
    // A pale blue, a little see-through, premultiplied.
    let beam = Image::new_fill(
        Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        &[105, 150, 170, 180],
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    commands.spawn((
        Name::new("VR pointer beam"),
        Beam,
        Mesh3d(meshes.add(Cuboid::new(1.0, 1.0, 1.0))),
        MeshMaterial3d(materials.add(PanelMaterial::whole(images.add(beam)))),
        Transform::default(),
        Visibility::Hidden,
        RenderLayers::layer(PANEL_LAYER),
        ChildOf(*root),
    ));
    commands.spawn((
        Name::new("VR pointer dot"),
        Dot,
        Mesh3d(meshes.add(Rectangle::new(1.0, 1.0))),
        MeshMaterial3d(materials.add(PanelMaterial::whole(images.add(ring_image())))),
        Transform::default(),
        Visibility::Hidden,
        RenderLayers::layer(PANEL_LAYER),
        ChildOf(*root),
    ));
}

/// Follow the right controller's aim, smoothed, into [`HandRay`].
///
/// The aim space is a child of the tracking root, so its own transform is
/// the hand in the seat's space; that is what is smoothed. Into the world
/// it goes by where the seat will be this frame, worked out from the camera
/// the same way [`follow_main_camera`](crate::follow_main_camera) will, or
/// by where the grips have moved the table: the seat's transform still
/// holds last frame's, and the difference, while the tank turns, swings a
/// ray's far end by metres.
#[allow(clippy::too_many_arguments)]
fn smooth_hand(
    time: Res<Time>,
    view: Res<View>,
    table: Res<Table>,
    aims: Query<(&Transform, &OxrSpaceLocationFlags), With<PointerAim>>,
    cameras: MainCameras,
    roots: Query<&Transform, (With<XrTrackingRoot>, Without<PointerAim>)>,
    mut smoothed: Local<Option<Transform>>,
    mut hand: ResMut<HandRay>,
) {
    let located = aims
        .iter()
        .find(|(_, flags)| flags.pos_valid() && flags.rot_valid())
        .map(|(aim, _)| *aim);
    let Some(target) = located else {
        *smoothed = None;
        *hand = HandRay::default();
        return;
    };
    let seat = match smoothed.as_ref() {
        Some(previous) => steady(*previous, target, time.delta_secs()),
        None => target,
    };
    *smoothed = Some(seat);

    let last = roots.iter().next().copied().unwrap_or_default();
    let root = root_pose_now(*view, &table, &cameras, last);
    let world = root.mul_transform(seat);
    *hand = HandRay {
        seat: Some(seat),
        world: Some(Ray3d::new(world.translation, world.forward())),
    };
}

/// One frame of smoothing from `previous` toward `target`, `dt` seconds on.
///
/// An exponential follow whose time constant shrinks as the gap opens: at
/// rest it is [`STEADY_TIME`], and by [`SWEEP_SPEED`] it is gone, so the
/// aim keeps up with a swing.
fn steady(previous: Transform, target: Transform, dt: f32) -> Transform {
    if dt <= 0.0 {
        return previous;
    }
    let speed = previous.rotation.angle_between(target.rotation) / dt;
    let time_constant = STEADY_TIME * (1.0 - (speed / SWEEP_SPEED).min(1.0));
    let follow = if time_constant > 0.0 {
        1.0 - (-dt / time_constant).exp()
    } else {
        1.0
    };
    Transform {
        translation: previous.translation.lerp(target.translation, follow),
        rotation: previous.rotation.slerp(target.rotation, follow),
        scale: target.scale,
    }
}

/// What the pointer did last frame.
#[derive(Debug, Default)]
struct PointerState {
    /// Where it was in the panel's image; `None` before it first moved.
    position: Option<Vec2>,
    /// Whether its button is down.
    pressed: bool,
}

/// Move the picking pointer to where the hand's beam meets the panel and
/// press or release it with the trigger.
///
/// In the seat's space, where the panel hangs, so the seat's movement does
/// not come into it.
#[allow(clippy::too_many_arguments)]
fn point(
    actions: Option<Res<PointerActions>>,
    session: Res<OxrSession>,
    panel: Option<Res<Panel>>,
    panels: Query<(&Transform, &InheritedVisibility)>,
    hand: Res<HandRay>,
    images: Res<Assets<Image>>,
    mut inputs: MessageWriter<PointerInput>,
    mut state: Local<PointerState>,
) {
    let (Some(actions), Some(panel)) = (actions, panel) else {
        return;
    };
    let location = |position: Vec2| Location {
        target: NormalizedRenderTarget::Image(panel.image.clone().into()),
        position,
    };
    let size = images.get(&panel.image).map(|image| image.size().as_vec2());

    let (Some(panel_pose), Some(hand), Some(size)) =
        (shown_panel(&panel, &panels), hand.seat, size)
    else {
        // Nothing to point at: let go and leave the panel.
        if state.pressed {
            inputs.write(PointerInput::new(
                POINTER,
                location(state.position.unwrap_or(OFF_PANEL)),
                PointerAction::Release(PointerButton::Primary),
            ));
            state.pressed = false;
        }
        if let Some(last) = state.position.filter(|last| *last != OFF_PANEL) {
            inputs.write(PointerInput::new(
                POINTER,
                location(OFF_PANEL),
                PointerAction::Move {
                    delta: OFF_PANEL - last,
                },
            ));
            state.position = Some(OFF_PANEL);
        }
        return;
    };

    let hit = hit_panel(&panel_pose, ray_of(hand));
    let position = hit.map_or(OFF_PANEL, |(local, _)| panel_pixel(local, size));
    if state.position != Some(position) {
        let delta = position - state.position.unwrap_or(position);
        inputs.write(PointerInput::new(
            POINTER,
            location(position),
            PointerAction::Move { delta },
        ));
        state.position = Some(position);
    }

    let held = actions
        .select
        .state(&session, openxr::Path::NULL)
        .is_ok_and(|state| state.is_active && state.current_state);
    // A press starts only on the panel; a release ends it anywhere.
    let action = match (held, state.pressed) {
        (true, false) if hit.is_some() => Some(PointerAction::Press(PointerButton::Primary)),
        (false, true) => Some(PointerAction::Release(PointerButton::Primary)),
        _ => None,
    };
    if let Some(action) = action {
        state.pressed = matches!(action, PointerAction::Press(_));
        inputs.write(PointerInput::new(POINTER, location(position), action));
    }
}

/// The panel's pose in the seat's space, while it is shown.
fn shown_panel(
    panel: &Panel,
    panels: &Query<(&Transform, &InheritedVisibility), impl QueryFilter>,
) -> Option<GlobalTransform> {
    panels
        .get(panel.entity)
        .ok()
        .filter(|(_, visibility)| visibility.get())
        .map(|(transform, _)| GlobalTransform::from(*transform))
}

/// The ray along a pose's forward.
fn ray_of(pose: Transform) -> Ray3d {
    Ray3d::new(pose.translation, pose.forward())
}

/// Draw the beam: onto the panel while it is shown, with the dot where it
/// lands; in play, while the hand aims, out to the aim point.
///
/// After the seat has followed the camera this frame and before transforms
/// propagate, so the beam, a child of the seat, moves with the eyes.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn place_beam(
    panel: Option<Res<Panel>>,
    panels: Query<(&Transform, &InheritedVisibility), (Without<Beam>, Without<Dot>)>,
    roots: Query<&Transform, (With<XrTrackingRoot>, Without<Beam>, Without<Dot>)>,
    hand: Res<HandRay>,
    source: Res<AimSource>,
    lock: Res<AimLock>,
    guided: Option<Res<GuidedMissile>>,
    players: Query<(&PlayerInput, &Controlled)>,
    mut beams: Query<(&mut Transform, &mut Visibility), (With<Beam>, Without<Dot>)>,
    mut dots: Query<(&mut Transform, &mut Visibility), (With<Dot>, Without<Beam>)>,
) {
    let shown = panel.and_then(|panel| shown_panel(&panel, &panels));
    let mut ray = hand.seat.map(ray_of);

    let (end, dot) = match (shown, ray) {
        (Some(panel_pose), Some(ray)) => {
            let hit = hit_panel(&panel_pose, ray);
            let end = hit.map_or(ray.get_point(BEAM_REACH), |(_, point)| point);
            // The panel's own facing, a hair in front of it.
            let rotation = panel_pose.compute_transform().rotation;
            let dot = hit.map(|(_, point)| {
                Transform::from_translation(point + rotation * Vec3::Z * DOT_LIFT)
                    .with_rotation(rotation)
                    .with_scale(Vec3::splat(DOT_SIZE))
            });
            (Some(end), dot)
        }
        // In play, aiming with the hand: the beam runs to the aim point. Not
        // while a missile is flown, whose view holds a stale one.
        (None, Some(_))
            if *source == AimSource::Hand && !guided.is_some_and(|guided| guided.is_guiding()) =>
        {
            let aim_point = players
                .iter()
                .find(|(_, controlled)| **controlled == Controlled::Local)
                .and_then(|(input, _)| input.aim_point);
            // Along the hand's own ray, as far as the aim point: rigid with
            // the hand, whatever the seat does between the frame's aim and
            // its drawing. The seat has no parent, so its transform is its
            // world pose.
            let seat = roots.iter().next().copied().unwrap_or_default();
            let to_seat = seat.compute_affine().inverse();
            // Locked on an enemy, the beam bends to it.
            if let (Some(locked), Some(hand)) = (lock.point, ray) {
                let end = to_seat.transform_point3(locked);
                if let Ok(toward) = Dir3::new(end - hand.origin) {
                    ray = Some(Ray3d::new(hand.origin, toward));
                }
                (Some(end), None)
            } else {
                let end = ray.zip(aim_point).map(|(ray, point)| {
                    let point = to_seat.transform_point3(point);
                    ray.get_point(ray.origin.distance(point))
                });
                (end, None)
            }
        }
        _ => (None, None),
    };

    for (mut transform, mut visibility) in &mut beams {
        match ray.zip(end) {
            Some((ray, end)) => {
                *transform = beam(ray, end);
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    for (mut transform, mut visibility) in &mut dots {
        match dot {
            Some(dot) => {
                *transform = dot;
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
}

/// The beam's box: from the ray's origin to `end`, thin across.
fn beam(ray: Ray3d, end: Vec3) -> Transform {
    let length = ray.origin.distance(end);
    // Any up will do for a beam square across; one off the beam's line.
    let up = if ray.direction.y.abs() < 0.99 {
        Vec3::Y
    } else {
        Vec3::X
    };
    Transform::from_translation(ray.origin.midpoint(end))
        .looking_to(ray.direction, up)
        .with_scale(Vec3::new(BEAM_WIDTH, BEAM_WIDTH, length))
}

/// Where `ray` meets the panel: the point on the unit quad (−0.5…0.5 each
/// way, +y up) and in the space the panel's pose is in. `None` when it misses or points away.
fn hit_panel(panel: &GlobalTransform, ray: Ray3d) -> Option<(Vec2, Vec3)> {
    let to_local = panel.affine().inverse();
    let origin = to_local.transform_point3(ray.origin);
    let direction = to_local.transform_vector3(*ray.direction);
    if direction.z.abs() < f32::EPSILON {
        return None;
    }
    // The same parameter along the ray in either space: the map is affine.
    let distance = -origin.z / direction.z;
    let local = (origin + direction * distance).truncate();
    (distance > 0.0 && local.x.abs() <= 0.5 && local.y.abs() <= 0.5)
        .then(|| (local, ray.get_point(distance)))
}

/// A point on the unit quad as a position in the panel's image, whose
/// origin is its top-left corner.
fn panel_pixel(local: Vec2, size: Vec2) -> Vec2 {
    Vec2::new(local.x + 0.5, 0.5 - local.y) * size
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panel 2 m wide and 1 m high, 2 m ahead, facing back at the origin.
    fn panel() -> GlobalTransform {
        GlobalTransform::from(
            Transform::from_xyz(0.0, 0.0, -2.0).with_scale(Vec3::new(2.0, 1.0, 1.0)),
        )
    }

    #[test]
    fn a_ray_straight_ahead_meets_the_panels_centre() {
        let (local, world) = hit_panel(&panel(), Ray3d::new(Vec3::ZERO, Dir3::NEG_Z)).unwrap();
        assert!(local.length() < 1e-6, "{local:?}");
        assert!(
            world.distance(Vec3::new(0.0, 0.0, -2.0)) < 1e-6,
            "{world:?}"
        );
    }

    #[test]
    fn a_ray_up_and_right_meets_the_upper_right() {
        let ray = Ray3d::new(Vec3::ZERO, Dir3::new(Vec3::new(0.5, 0.25, -2.0)).unwrap());
        let (local, world) = hit_panel(&panel(), ray).unwrap();
        // Half a metre right of 2 m wide is a quarter; a quarter metre up of
        // 1 m high is a quarter.
        assert!(local.distance(Vec2::new(0.25, 0.25)) < 1e-5, "{local:?}");
        assert!(
            world.distance(Vec3::new(0.5, 0.25, -2.0)) < 1e-5,
            "{world:?}"
        );
    }

    #[test]
    fn a_ray_past_the_edge_or_away_misses() {
        let wide = Ray3d::new(Vec3::ZERO, Dir3::new(Vec3::new(1.5, 0.0, -2.0)).unwrap());
        assert_eq!(hit_panel(&panel(), wide), None);
        assert_eq!(hit_panel(&panel(), Ray3d::new(Vec3::ZERO, Dir3::Z)), None);
        assert_eq!(hit_panel(&panel(), Ray3d::new(Vec3::ZERO, Dir3::X)), None);
    }

    #[test]
    fn a_still_hand_is_steadied_and_a_swing_is_not_trailed() {
        let from = Transform::default();
        let dt = 1.0 / 90.0;

        // A tremor of a tenth of a degree is mostly held back.
        let tremor = Transform::from_rotation(Quat::from_rotation_y(0.1_f32.to_radians()));
        let held = steady(from, tremor, dt);
        let moved = from.rotation.angle_between(held.rotation);
        assert!(moved < 0.5 * 0.1_f32.to_radians(), "{moved}");

        // A swing faster than the sweep speed is followed at once.
        let swing = Transform::from_rotation(Quat::from_rotation_y(SWEEP_SPEED * dt * 1.5));
        let followed = steady(from, swing, dt);
        assert!(followed.rotation.angle_between(swing.rotation) < 1e-5);

        // No time, no movement.
        assert_eq!(steady(from, swing, 0.0), from);
    }

    #[test]
    fn the_image_origin_is_the_top_left() {
        let size = Vec2::new(1280.0, 720.0);
        assert_eq!(panel_pixel(Vec2::new(-0.5, 0.5), size), Vec2::ZERO);
        assert_eq!(panel_pixel(Vec2::new(0.5, -0.5), size), size);
        assert_eq!(panel_pixel(Vec2::ZERO, size), size / 2.0);
    }
}
