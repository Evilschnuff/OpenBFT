//! The tabletop view: the level as a model on a table in front of the
//! player, who sits over it like a giant and pulls it about with the grips.
//!
//! # How
//!
//! Nothing shrinks. The simulation stays in metres, and the tracking space
//! grows instead: the tracking root carries a scale ([`Table`]), so a metre
//! of the room is [`DEFAULT_SCALE`] metres of the level, and the head, the
//! hands and everything riding on them (the beam, the HUD tiles, the UI
//! panel) are as large again in the world. The eyes go apart by the same
//! factor, which is what makes the level read as a model. The hand's beam is
//! still a ray in the world, so the turret aims where it lands on the model.
//!
//! The table is where the player leaves it: the tank drives across it, and
//! squeezing a grip twice quickly ([`DOUBLE_SQUEEZE`]) brings the tank back
//! to its centre and turns the table so the player sits behind it, looking
//! the way it faces, as the chase camera does; the scale stays. Holding one grip drags the level with that hand; holding
//! both drags it with their midpoint, turns it about the vertical as the
//! hands turn about each other and scales it as they part or close. Each
//! grip holds the level by the world point it had under it when it closed,
//! so the level stays under the hands.
//!
//! The sky dome is hidden while the table is up, since the level is now
//! seen from outside it, and the eyes see no fog. Behind the level they
//! clear to transparent black: where the runtime blends the room in behind
//! the picture, the table stands in the room, and otherwise the backdrop is
//! black. The room comes in either through the `ALPHA_BLEND` mode, asked
//! for when the runtime offers it, or by the runtime's own choice: SteamVR
//! offers only `OPAQUE` to an app streamed to a Steam Frame from a PC, but
//! its undocumented `camera/forceSceneAlphaBlending` setting blends the
//! camera in wherever the picture is transparent anyway.
//!
//! Recoil had no VR, so every number here is a presentation choice.

use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use bevy_mod_openxr::action_binding::{OxrSendActionBindings, OxrSuggestActionBinding};
use bevy_mod_openxr::action_set_attaching::OxrAttachActionSet;
use bevy_mod_openxr::action_set_syncing::{OxrActionSetSyncSet, OxrSyncActionSet};
use bevy_mod_openxr::environment_blend_mode::OxrEnvironmentBlendModes;
use bevy_mod_openxr::openxr_session_running;
use bevy_mod_openxr::resources::OxrInstance;
use bevy_mod_openxr::session::OxrSession;
use bevy_mod_openxr::spaces::OxrSpaceLocationFlags;
use bevy_mod_xr::camera::XrCamera;
use bevy_mod_xr::session::{XrSessionCreated, XrTrackingRoot, session_available};
use bft_game::level_view::SkyDome;
use bft_sim::vehicle::{Controlled, Vehicle};
use openxr::EnvironmentBlendMode;

use crate::hands::{Grip, Side, located_grip};
use crate::{MainCameras, View, main_camera_pose, seat_pose};

use crate::profiles::{FRAME, FrameControllers, TOUCH};

/// Where the table's centre is, in the room: metres below and ahead of
/// where the head was at the last recentre (the seated space's origin).
///
/// A presentation choice: about where a table top is for someone sitting.
const TABLE_CENTRE: Vec3 = Vec3::new(0.0, -0.45, -0.55);

/// Metres of the level to a metre of the room when the table first comes
/// up: a 1:100 model, so the tank is a few centimetres long and the table's
/// half metre is the fifty metres around it.
const DEFAULT_SCALE: f32 = 100.0;

/// The nearest the model can be pulled in: 1:10.
const MIN_SCALE: f32 = 10.0;

/// The furthest it can be pushed out: 1:2000, so the largest level fits
/// on the table whole.
const MAX_SCALE: f32 = 2000.0;

/// How quickly a grip has to close again after closing to count as a
/// double squeeze, seconds.
const DOUBLE_SQUEEZE: f32 = 0.4;

/// The fog the eyes get while the table is up: none, by starting it
/// further out than anything can be.
const NO_FOG: FogFalloff = FogFalloff::Linear {
    start: f32::MAX,
    end: f32::MAX,
};

/// The table: where the tracking root is while it is up, scale and all.
/// `None` until it is next put in front of the player, centred on the tank.
#[derive(Resource, Debug, Default)]
pub(crate) struct Table {
    root: Option<Transform>,
}

impl Table {
    /// Where the tracking root is this frame, once the table is placed.
    pub(crate) fn root(&self) -> Option<Transform> {
        self.root
    }
}

/// The grip actions and the set they are in.
#[derive(Resource)]
struct TableActions {
    set: openxr::ActionSet,
    /// A grip closed, read per hand.
    grab: openxr::Action<bool>,
    /// `/user/hand/left` and `/user/hand/right`, to read [`grab`](Self::grab)
    /// per hand.
    hands: [(Side, openxr::Path); 2],
}

/// What the hands hold of the level: per closed grip, the world point it
/// closed on.
#[derive(Debug, Default)]
struct Hold {
    left: Option<Vec3>,
    right: Option<Vec3>,
}

/// Puts the level on a table and lets the grips move it.
pub(crate) struct TablePlugin;

impl Plugin for TablePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Table>()
            .add_systems(Startup, create_actions.run_if(session_available))
            .add_systems(OxrSendActionBindings, suggest_bindings)
            .add_systems(XrSessionCreated, (attach_actions, log_blend_modes))
            .add_systems(
                PreUpdate,
                sync_actions
                    .before(OxrActionSetSyncSet)
                    .run_if(openxr_session_running),
            )
            .add_systems(
                Update,
                // Before the hand's ray is taken into the world, so it goes
                // by where the table is this frame.
                move_table
                    .before(crate::pointer::HandRaySet)
                    .run_if(openxr_session_running),
            )
            .add_systems(
                PostUpdate,
                (
                    place_table.before(TransformSystems::Propagate),
                    // After the level's own environment and sky systems, so
                    // a level loading under the table does not put its fog
                    // and sky back for a frame.
                    look_of_view
                        .after(bft_game::level_view::follow_sky_dome)
                        .before(bevy::camera::visibility::VisibilitySystems::VisibilityPropagate),
                ),
            );
    }
}

/// Make the action set and the grab action, read per hand.
fn create_actions(instance: Res<OxrInstance>, mut commands: Commands) {
    let created = (|| {
        let hands = [
            (Side::Left, instance.string_to_path("/user/hand/left")?),
            (Side::Right, instance.string_to_path("/user/hand/right")?),
        ];
        let set = instance.create_action_set("table", "Table", 0)?;
        let grab =
            set.create_action("grab_table", "Grab the table", &hands.map(|(_, path)| path))?;
        Ok::<_, openxr::sys::Result>(TableActions { set, grab, hands })
    })();
    match created {
        Ok(actions) => commands.insert_resource(actions),
        Err(error) => warn!("table: could not create the actions: {error}"),
    }
}

/// Both grips grab.
fn suggest_bindings(
    actions: Option<Res<TableActions>>,
    frame: Res<FrameControllers>,
    mut bindings: MessageWriter<OxrSuggestActionBinding>,
) {
    let Some(actions) = actions else {
        return;
    };
    // The same squeeze in both profiles.
    for profile in std::iter::once(TOUCH).chain(frame.0.then_some(FRAME)) {
        bindings.write(OxrSuggestActionBinding {
            action: actions.grab.as_raw(),
            interaction_profile: profile.into(),
            bindings: vec![
                "/user/hand/left/input/squeeze/value".into(),
                "/user/hand/right/input/squeeze/value".into(),
            ],
        });
    }
}

/// Attach the set to the new session.
fn attach_actions(
    actions: Option<Res<TableActions>>,
    mut attach: MessageWriter<OxrAttachActionSet>,
) {
    if let Some(actions) = actions {
        attach.write(OxrAttachActionSet(actions.set.clone()));
    }
}

/// Say whether the room can show behind the table, for the log.
fn log_blend_modes(modes: Option<Res<OxrEnvironmentBlendModes>>) {
    let Some(modes) = modes else {
        return;
    };
    let passthrough = modes
        .available_blend_modes()
        .contains(&EnvironmentBlendMode::ALPHA_BLEND);
    info!(
        "table: the runtime blends with {:?}; {}",
        modes.available_blend_modes(),
        if passthrough {
            "the room shows behind the table"
        } else {
            "it offers no passthrough (SteamVR's camera/forceSceneAlphaBlending may blend it in regardless)"
        }
    );
}

/// Ask the runtime for this frame's grips.
fn sync_actions(actions: Option<Res<TableActions>>, mut sync: MessageWriter<OxrSyncActionSet>) {
    if let Some(actions) = actions {
        sync.write(OxrSyncActionSet(actions.set.clone()));
    }
}

/// Whether a hand's grip is closed.
fn grabbing(actions: &TableActions, session: &OxrSession, side: Side) -> bool {
    actions
        .hands
        .iter()
        .find(|(hand, _)| *hand == side)
        .is_some_and(|&(_, path)| {
            actions
                .grab
                .state(session, path)
                .is_ok_and(|state| state.is_active && state.current_state)
        })
}

/// Grip state between frames.
#[derive(Debug, Default)]
struct Grips {
    /// What the hands hold of the level.
    hold: Hold,
    /// Whether each grip (left, right) was closed last frame.
    closed: [bool; 2],
    /// When each grip last closed, for the double squeeze.
    closed_at: [Option<f32>; 2],
    /// The tank the table was last centred on.
    followed: Option<Entity>,
}

/// Recentre, drag, turn and scale the table from the grips.
#[allow(clippy::too_many_arguments)]
fn move_table(
    time: Res<Time>,
    view: Res<View>,
    actions: Option<Res<TableActions>>,
    session: Res<OxrSession>,
    grips: Query<(&Grip, &Transform, &OxrSpaceLocationFlags)>,
    hulls: Query<(Entity, &Transform, &Controlled), With<Vehicle>>,
    cameras: MainCameras,
    mut table: ResMut<Table>,
    mut state: Local<Grips>,
) {
    if *view != View::Table {
        table.root = None;
        *state = Grips::default();
        return;
    }
    let now = time.elapsed_secs();
    let hull = hulls
        .iter()
        .find(|(_, _, controlled)| **controlled == Controlled::Local);

    // A new tank (a mission started, or the player respawned into another
    // hull) is put back in the middle.
    let mut recentre = false;
    let hull_entity = hull.map(|(entity, ..)| entity);
    if hull_entity.is_some() && hull_entity != state.followed {
        state.followed = hull_entity;
        recentre = true;
    }

    // The grips, and a double squeeze on either.
    let mut held = [false; 2];
    if let Some(actions) = &actions {
        for (index, side) in [Side::Left, Side::Right].into_iter().enumerate() {
            held[index] = grabbing(actions, &session, side);
            if held[index] && !state.closed[index] {
                if state.closed_at[index].is_some_and(|then| now - then <= DOUBLE_SQUEEZE) {
                    recentre = true;
                    state.closed_at[index] = None;
                } else {
                    state.closed_at[index] = Some(now);
                }
            }
        }
    }
    state.closed = held;

    let root = match table.root.filter(|_| !recentre) {
        Some(root) => root,
        None => {
            let camera = main_camera_pose(&cameras);
            let centre = hull
                .map(|(_, transform, _)| transform.translation)
                .or(camera.map(|camera| camera.translation));
            let Some(centre) = centre else {
                return;
            };
            // Behind the tank, looking the way it faces, as the chase
            // camera rides; at the scale the player left it.
            let facing = hull
                .map(|(_, transform, _)| behind(transform.rotation))
                .or(camera.map(|camera| seat_pose(camera, Transform::default()).rotation))
                .unwrap_or_default();
            let scale = table.root.map_or(DEFAULT_SCALE, |root| root.scale.x);
            // Any hold is let go: it held the table where it was.
            state.hold = Hold::default();
            centred(centre, facing, scale)
        }
    };

    let left = located_grip(&grips, Side::Left)
        .map(|grip| grip.translation)
        .filter(|_| held[0]);
    let right = located_grip(&grips, Side::Right)
        .map(|grip| grip.translation)
        .filter(|_| held[1]);

    // A grip that closed takes hold of the world point under it; one that
    // opened lets go. Whenever the hands holding change, both take hold
    // afresh where they are, so going from one hand to two does not jump.
    let hold = &mut state.hold;
    if left.is_some() != hold.left.is_some() || right.is_some() != hold.right.is_some() {
        hold.left = left.map(|hand| root.transform_point(hand));
        hold.right = right.map(|hand| root.transform_point(hand));
    }

    table.root = Some(match (left.zip(hold.left), right.zip(hold.right)) {
        (Some(left), Some(right)) => held_by_two(root, left, right),
        (Some((hand, held)), None) | (None, Some((hand, held))) => held_by_one(root, hand, held),
        (None, None) => root,
    });
}

/// The room's turn that puts the player behind a hull turned `hull`,
/// looking along it.
fn behind(hull: Quat) -> Quat {
    let ahead = (hull * Vec3::NEG_Z).with_y(0.0);
    Dir3::new(ahead).map_or(Quat::IDENTITY, |ahead| {
        Transform::default().looking_to(ahead, Vec3::Y).rotation
    })
}

/// The root that puts `centre` at the table's centre, the room turned to
/// `facing` and scaled by `scale`.
fn centred(centre: Vec3, facing: Quat, scale: f32) -> Transform {
    let scale = scale.clamp(MIN_SCALE, MAX_SCALE);
    Transform {
        translation: centre - facing * (TABLE_CENTRE * scale),
        rotation: facing,
        scale: Vec3::splat(scale),
    }
}

/// `root` moved so the hand at `hand` (in the room) is over `held` (in the
/// world).
fn held_by_one(root: Transform, hand: Vec3, held: Vec3) -> Transform {
    Transform {
        translation: root.translation + held - root.transform_point(hand),
        ..root
    }
}

/// The root that keeps the two hands over the two points they hold: their
/// midpoint over the points', turned so the line between the hands lies
/// along the line between the points, and scaled so the one is as long as
/// the other, both across the level and not up it.
fn held_by_two(root: Transform, left: (Vec3, Vec3), right: (Vec3, Vec3)) -> Transform {
    let ((left_hand, left_held), (right_hand, right_held)) = (left, right);
    let hands = (right_hand - left_hand).xz();
    let held = (right_held - left_held).xz();
    // Hands too close together to read a turn or a scale from: drag only.
    if hands.length() < 0.02 || held.length() < f32::EPSILON {
        return held_by_one(
            root,
            left_hand.midpoint(right_hand),
            left_held.midpoint(right_held),
        );
    }
    let scale = (held.length() / hands.length()).clamp(MIN_SCALE, MAX_SCALE);
    // A turn about +y by θ adds θ to a horizontal vector's angle measured
    // from +x toward −z, so the turn is the difference of the two lines'.
    let angle = |v: Vec2| (-v.y).atan2(v.x);
    let rotation = Quat::from_rotation_y(angle(held) - angle(hands));
    let turned = Transform {
        translation: Vec3::ZERO,
        rotation,
        scale: Vec3::splat(scale),
    };
    let hand = left_hand.midpoint(right_hand);
    let point = left_held.midpoint(right_held);
    Transform {
        translation: point - turned.transform_point(hand),
        ..turned
    }
}

/// Put the tracking root where the table is, while it is up.
pub(crate) fn place_table(
    view: Res<View>,
    table: Res<Table>,
    mut roots: Query<&mut Transform, With<XrTrackingRoot>>,
) {
    if *view != View::Table {
        return;
    }
    if let Some(table) = table.root {
        for mut root in &mut roots {
            *root = table;
        }
    }
}

/// On an eye camera whose fog and clear colour the table has taken over.
#[derive(Component, Debug)]
struct TableLook;

/// While the table is up: no sky, no fog for the eyes, and the room or a
/// plain backdrop behind the level. Put back when the seat comes back.
#[allow(clippy::type_complexity)]
fn look_of_view(
    view: Res<View>,
    mut commands: Commands,
    mut blend: Option<ResMut<OxrEnvironmentBlendModes>>,
    mut eyes: Query<
        (
            Entity,
            &mut Camera,
            Option<&mut DistanceFog>,
            Has<TableLook>,
        ),
        With<XrCamera>,
    >,
    domes: Query<Entity, With<SkyDome>>,
    children: Query<&Children>,
    mut meshes: Query<&mut Visibility, With<Mesh3d>>,
) {
    let table = *view == View::Table;

    if let Some(blend) = blend.as_mut() {
        let wanted = if table {
            EnvironmentBlendMode::ALPHA_BLEND
        } else {
            EnvironmentBlendMode::OPAQUE
        };
        if blend.blend_mode() != wanted {
            // Refused when the runtime has no such mode; then it stays.
            blend.set_blend_mode(wanted);
        }
    }

    for (entity, mut camera, fog, taken) in &mut eyes {
        if table {
            // The level's fog is put back on the eyes whenever its
            // environment changes; it is taken off again here, the same
            // frame.
            if let Some(mut fog) = fog
                && !matches!(fog.falloff, FogFalloff::Linear { start, .. } if start == f32::MAX)
            {
                fog.falloff = NO_FOG;
            }
            // Transparent whatever the blend mode: an opaque runtime shows
            // it black, and one that blends the room in regardless (see the
            // module docs) shows the room.
            camera.clear_color = ClearColorConfig::Custom(Color::NONE);
            if !taken {
                commands.entity(entity).insert(TableLook);
            }
        } else if taken {
            // Without fog, the level's environment puts its own fog and
            // clear colour back on its next pass.
            commands.entity(entity).remove::<(TableLook, DistanceFog)>();
        }
    }

    let wanted = if table {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    for dome in &domes {
        for entity in std::iter::once(dome).chain(children.iter_descendants(dome)) {
            if let Ok(mut visibility) = meshes.get_mut(entity) {
                visibility.set_if_neq(wanted);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1e-3 * (1.0 + a.length().max(b.length()))
    }

    #[test]
    fn the_centre_lands_on_the_table() {
        let facing = Quat::from_rotation_y(0.7);
        let root = centred(Vec3::new(2200.0, 30.0, 1700.0), facing, 100.0);
        assert!(close(
            root.transform_point(TABLE_CENTRE),
            Vec3::new(2200.0, 30.0, 1700.0)
        ));
        assert_eq!(root.scale, Vec3::splat(100.0));
    }

    #[test]
    fn recentring_puts_the_player_behind_the_tank() {
        let hull = Quat::from_rotation_y(1.2);
        let root = centred(Vec3::new(10.0, 0.0, 20.0), behind(hull), 100.0);
        // The room's forward, the way the player faces, is the hull's.
        let room_ahead = root.rotation * Vec3::NEG_Z;
        assert!(room_ahead.distance(hull * Vec3::NEG_Z) < 1e-5);
        // And the head, at the room's origin, is behind the tank.
        let head = root.transform_point(Vec3::ZERO);
        assert!((head - Vec3::new(10.0, 0.0, 20.0)).dot(hull * Vec3::NEG_Z) < 0.0);
    }

    #[test]
    fn one_hand_drags_the_level_with_it() {
        let root = centred(Vec3::new(10.0, 0.0, 20.0), Quat::from_rotation_y(1.0), 50.0);
        let held = root.transform_point(Vec3::new(0.1, -0.3, -0.4));
        let moved = held_by_one(root, Vec3::new(0.3, -0.2, -0.5), held);
        assert!(close(
            moved.transform_point(Vec3::new(0.3, -0.2, -0.5)),
            held
        ));
        assert_eq!(moved.rotation, root.rotation);
        assert_eq!(moved.scale, root.scale);
    }

    #[test]
    fn two_hands_keep_both_points_under_them() {
        let root = centred(
            Vec3::new(10.0, 0.0, 20.0),
            Quat::from_rotation_y(0.3),
            100.0,
        );
        let (left, right) = (Vec3::new(-0.2, -0.4, -0.5), Vec3::new(0.2, -0.4, -0.5));
        let (left_held, right_held) = (root.transform_point(left), root.transform_point(right));

        // Hands apart to twice the gap and turned a quarter: the level
        // scales to half and turns with them.
        let (left_now, right_now) = (Vec3::new(0.0, -0.4, -0.1), Vec3::new(0.0, -0.4, -0.9));
        let moved = held_by_two(root, (left_now, left_held), (right_now, right_held));
        assert!((moved.scale.x - 50.0).abs() < 1e-3, "{:?}", moved.scale);
        assert!(close(moved.transform_point(left_now), left_held));
        assert!(close(moved.transform_point(right_now), right_held));
    }

    #[test]
    fn two_hands_still_hold_still() {
        let root = centred(
            Vec3::new(10.0, 0.0, 20.0),
            Quat::from_rotation_y(-2.0),
            100.0,
        );
        let (left, right) = (Vec3::new(-0.2, -0.4, -0.5), Vec3::new(0.25, -0.35, -0.45));
        let moved = held_by_two(
            root,
            (left, root.transform_point(left)),
            (right, root.transform_point(right)),
        );
        assert!(close(moved.translation, root.translation));
        // `angle_between` bottoms out near 7e-4 in f32.
        assert!(moved.rotation.angle_between(root.rotation) < 1e-3);
        assert!((moved.scale.x - 100.0).abs() < 1e-2);
    }

    #[test]
    fn the_scale_stops_at_its_limits() {
        assert_eq!(centred(Vec3::ZERO, Quat::IDENTITY, 1.0).scale.x, MIN_SCALE);
        assert_eq!(centred(Vec3::ZERO, Quat::IDENTITY, 1e6).scale.x, MAX_SCALE);
    }
}
