//! Where the controllers are held: each hand's grip pose, located in the
//! seat's space every frame, for what rides on the hands ([`hud`]).
//!
//! [`hud`]: crate::hud

use bevy::prelude::*;
use bevy_mod_openxr::action_binding::{OxrSendActionBindings, OxrSuggestActionBinding};
use bevy_mod_openxr::action_set_attaching::OxrAttachActionSet;
use bevy_mod_openxr::action_set_syncing::{OxrActionSetSyncSet, OxrSyncActionSet};
use bevy_mod_openxr::openxr_session_running;
use bevy_mod_openxr::resources::OxrInstance;
use bevy_mod_openxr::session::OxrSession;
use bevy_mod_openxr::spaces::OxrSpaceLocationFlags;
use bevy_mod_xr::session::{XrSessionCreated, session_available};

use crate::profiles::{FRAME, FrameControllers, SIMPLE, TOUCH};

/// A hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Side {
    Left,
    Right,
}

impl Side {
    /// The grip pose's path, the same in both profiles.
    fn grip_path(self) -> &'static str {
        match self {
            Side::Left => "/user/hand/left/input/grip/pose",
            Side::Right => "/user/hand/right/input/grip/pose",
        }
    }
}

/// The grip actions and the set they are in.
#[derive(Resource)]
struct GripActions {
    set: openxr::ActionSet,
    grips: [(Side, openxr::Action<openxr::Posef>); 2],
}

/// A hand's grip space: at the palm, −z along the controller's barrel.
#[derive(Component, Debug, Clone, Copy)]
pub(crate) struct Grip(pub(crate) Side);

/// Tracks both hands' grips.
pub(crate) struct HandsPlugin;

impl Plugin for HandsPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, create_actions.run_if(session_available))
            .add_systems(OxrSendActionBindings, suggest_bindings)
            .add_systems(XrSessionCreated, attach_and_spawn)
            .add_systems(
                PreUpdate,
                sync_actions
                    .before(OxrActionSetSyncSet)
                    .run_if(openxr_session_running),
            );
    }
}

/// Make the action set and the two grip actions.
fn create_actions(instance: Res<OxrInstance>, mut commands: Commands) {
    let created = instance
        .create_action_set("hands", "Hands", 0)
        .and_then(|set| {
            Ok(GripActions {
                grips: [
                    (
                        Side::Left,
                        set.create_action("left_grip", "Left hand", &[])?,
                    ),
                    (
                        Side::Right,
                        set.create_action("right_grip", "Right hand", &[])?,
                    ),
                ],
                set,
            })
        });
    match created {
        Ok(actions) => commands.insert_resource(actions),
        Err(error) => warn!("hands: could not create the grip actions: {error}"),
    }
}

/// Bind each grip action to its hand's grip pose.
///
/// The grip pose has the same path in every profile ([`crate::profiles`]).
fn suggest_bindings(
    actions: Option<Res<GripActions>>,
    frame: Res<FrameControllers>,
    mut bindings: MessageWriter<OxrSuggestActionBinding>,
) {
    let Some(actions) = actions else {
        return;
    };
    let profiles = [TOUCH, SIMPLE].into_iter().chain(frame.0.then_some(FRAME));
    for (side, action) in &actions.grips {
        for profile in profiles.clone() {
            bindings.write(OxrSuggestActionBinding {
                action: action.as_raw(),
                interaction_profile: profile.into(),
                bindings: vec![side.grip_path().into()],
            });
        }
    }
}

/// Attach the set to the new session and make each grip's space.
fn attach_and_spawn(
    actions: Option<Res<GripActions>>,
    session: Res<OxrSession>,
    mut attach: MessageWriter<OxrAttachActionSet>,
    mut commands: Commands,
) {
    let Some(actions) = actions else {
        return;
    };
    attach.write(OxrAttachActionSet(actions.set.clone()));
    for (side, action) in &actions.grips {
        match session.create_action_space(action, openxr::Path::NULL, Isometry3d::IDENTITY) {
            // `XrSpace` makes it a child of the tracking root, located in the
            // seat's space every frame.
            Ok(space) => {
                commands.spawn((Name::new(format!("{side:?} grip")), Grip(*side), space));
            }
            Err(error) => warn!("hands: could not create the {side:?} grip space: {error}"),
        }
    }
}

/// Ask the runtime for this frame's grips.
fn sync_actions(actions: Option<Res<GripActions>>, mut sync: MessageWriter<OxrSyncActionSet>) {
    if let Some(actions) = actions {
        sync.write(OxrSyncActionSet(actions.set.clone()));
    }
}

/// A hand's grip in the seat's space, while it is located.
pub(crate) fn located_grip(
    grips: &Query<(&Grip, &Transform, &OxrSpaceLocationFlags), impl bevy::ecs::query::QueryFilter>,
    side: Side,
) -> Option<Transform> {
    grips
        .iter()
        .find(|(grip, _, flags)| grip.0 == side && flags.pos_valid())
        .map(|(_, transform, _)| *transform)
}
