//! Where the viewer is, for what turns to face them: billboards, facades
//! (trees, pickups) and tracers.
//!
//! That is the main camera, unless a plugin that draws the level from
//! somewhere else says otherwise through [`ExternalViewer`]: a headset whose
//! eyes are not at the main camera, or a view of the level as a model from
//! above it. Billboards that kept facing the main camera would show such a
//! viewer their edges.

use bevy::prelude::*;

use crate::ui::hud::sensor::MainCamera3d;

/// The viewer's pose, kept up to date every frame by a plugin that draws
/// the level from somewhere other than the main camera.
///
/// While it holds a pose, billboards take its rotation and facades and
/// tracers turn toward its position. `None` (or no resource) is the main
/// camera.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq)]
pub struct ExternalViewer(pub Option<Transform>);

/// The viewer's pose this frame: the [`ExternalViewer`] when it holds one,
/// the first main camera's otherwise.
pub fn viewer_pose(
    external: Option<&ExternalViewer>,
    cameras: &Query<&GlobalTransform, impl bevy::ecs::query::QueryFilter>,
) -> Option<Transform> {
    external.and_then(|external| external.0).or_else(|| {
        cameras
            .iter()
            .next()
            .map(GlobalTransform::compute_transform)
    })
}

/// The main cameras, for [`viewer_pose`].
pub type ViewerCameras<'w, 's> = Query<'w, 's, &'static GlobalTransform, MainCamera3d>;
