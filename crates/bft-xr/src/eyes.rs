//! Eye tracking: aiming where the eyes look rather than where the head
//! points, on a headset and runtime that offer it.
//!
//! OpenXR exposes eye tracking through `XR_EXT_eye_gaze_interaction`: a
//! pose action bound to `/user/eyes_ext/input/gaze_ext/pose`, whose space
//! sits between the eyes and looks down −z along the gaze. [`with_xr`]
//! asks for the extension and the runtime enables it only if it has it; the
//! plugin here binds the action and keeps an [`EyeGaze`] space located every
//! frame. Whether gaze data actually arrives is up to the runtime (the user
//! may have to allow it), so the aim takes the eyes only while the space
//! has a valid rotation, and the head otherwise. A valid rotation is enough:
//! a runtime may report the gaze valid without also flagging it tracked.
//!
//! [`with_xr`]: crate::with_xr

use bevy::prelude::*;
use bevy_mod_openxr::action_binding::{OxrSendActionBindings, OxrSuggestActionBinding};
use bevy_mod_openxr::action_set_attaching::OxrAttachActionSet;
use bevy_mod_openxr::action_set_syncing::{OxrActionSetSyncSet, OxrSyncActionSet};
use bevy_mod_openxr::openxr_session_running;
use bevy_mod_openxr::resources::OxrInstance;
use bevy_mod_openxr::session::OxrSession;
use bevy_mod_openxr::spaces::OxrSpaceLocationFlags;
use bevy_mod_xr::session::{XrSessionCreated, session_available};

/// The interaction profile the gaze is bound under.
const GAZE_PROFILE: &str = "/interaction_profiles/ext/eye_gaze_interaction";

/// The gaze pose within that profile.
const GAZE_POSE: &str = "/user/eyes_ext/input/gaze_ext/pose";

/// The eye-gaze action and the set it is in.
#[derive(Resource)]
struct GazeAction {
    set: openxr::ActionSet,
    pose: openxr::Action<openxr::Posef>,
}

/// The located eye-gaze space: between the eyes, looking along the gaze.
#[derive(Component, Debug)]
pub(crate) struct EyeGaze;

/// Binds and locates the eye gaze. Added only when the runtime enabled
/// `XR_EXT_eye_gaze_interaction`.
pub(crate) struct EyeGazePlugin;

impl Plugin for EyeGazePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, create_action.run_if(session_available))
            .add_systems(OxrSendActionBindings, suggest_binding)
            .add_systems(XrSessionCreated, attach_and_spawn)
            .add_systems(
                PreUpdate,
                sync_action
                    .before(OxrActionSetSyncSet)
                    .run_if(openxr_session_running),
            )
            .add_systems(Update, report_gaze.run_if(openxr_session_running));
    }
}

/// Make the action set and the pose action.
fn create_action(instance: Res<OxrInstance>, mut commands: Commands) {
    let created = instance
        .create_action_set("eye_gaze", "Eye gaze", 0)
        .and_then(|set| {
            let pose = set.create_action("gaze", "Gaze", &[])?;
            Ok(GazeAction { set, pose })
        });
    match created {
        Ok(action) => commands.insert_resource(action),
        Err(error) => warn!("eye tracking: could not create the gaze action: {error}"),
    }
}

/// Bind the action to the gaze pose.
fn suggest_binding(
    action: Option<Res<GazeAction>>,
    mut bindings: MessageWriter<OxrSuggestActionBinding>,
) {
    let Some(action) = action else {
        return;
    };
    bindings.write(OxrSuggestActionBinding {
        action: action.pose.as_raw(),
        interaction_profile: GAZE_PROFILE.into(),
        bindings: vec![GAZE_POSE.into()],
    });
}

/// Attach the set to the new session and make the gaze's space.
fn attach_and_spawn(
    action: Option<Res<GazeAction>>,
    session: Res<OxrSession>,
    mut attach: MessageWriter<OxrAttachActionSet>,
    mut commands: Commands,
) {
    let Some(action) = action else {
        return;
    };
    attach.write(OxrAttachActionSet(action.set.clone()));
    match session.create_action_space(&action.pose, openxr::Path::NULL, Isometry3d::IDENTITY) {
        // `XrSpace` makes it a child of the tracking root, located in the
        // seat's space every frame.
        Ok(space) => {
            commands.spawn((Name::new("eye gaze"), EyeGaze, space));
        }
        Err(error) => warn!("eye tracking: could not create the gaze space: {error}"),
    }
}

/// Ask the runtime for this frame's gaze.
fn sync_action(action: Option<Res<GazeAction>>, mut sync: MessageWriter<OxrSyncActionSet>) {
    if let Some(action) = action {
        sync.write(OxrSyncActionSet(action.set.clone()));
    }
}

/// How far the gaze got this frame, from nothing at all to tracked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum GazeState {
    /// No gaze space: the action or its space could not be made.
    #[default]
    NoSpace,
    /// The runtime has no source bound to the action: it offers the
    /// extension but sends no gaze.
    Inactive,
    /// Bound, but the space has no valid rotation.
    NotLocated,
    /// A valid rotation the runtime does not call tracked.
    Valid,
    /// A tracked rotation.
    Tracked,
}

/// Say in the log each time the gaze's state changes, so a run shows which
/// aim it is on and, when it is the head, where the eyes' gaze stopped.
fn report_gaze(
    action: Option<Res<GazeAction>>,
    session: Res<OxrSession>,
    gazes: Query<&OxrSpaceLocationFlags, With<EyeGaze>>,
    mut was: Local<GazeState>,
) {
    let active = action.is_some_and(|action| {
        action
            .pose
            .is_active(&session, openxr::Path::NULL)
            .unwrap_or(false)
    });
    let state = match gazes.iter().next() {
        None => GazeState::NoSpace,
        Some(_) if !active => GazeState::Inactive,
        Some(flags) if flags.rot_valid() && flags.rot_tracked() => GazeState::Tracked,
        Some(flags) if flags.rot_valid() => GazeState::Valid,
        Some(_) => GazeState::NotLocated,
    };
    if state != *was {
        let aim = match state {
            GazeState::Valid | GazeState::Tracked => "aiming with the eyes",
            _ => "aiming with the head",
        };
        info!("eye tracking: gaze {state:?}, {aim}");
        *was = state;
    }
}

/// The gaze's pose, while its rotation is valid.
pub(crate) fn located_gaze(
    gazes: &Query<(&GlobalTransform, &OxrSpaceLocationFlags), With<EyeGaze>>,
) -> Option<Transform> {
    gazes
        .iter()
        .find(|(_, flags)| flags.rot_valid())
        .map(|(gaze, _)| gaze.compute_transform())
}
