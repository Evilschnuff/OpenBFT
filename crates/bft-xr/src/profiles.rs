//! The interaction profiles the controllers' bindings are suggested for.
//!
//! The runtime picks the best match for the controllers in hand, and a
//! player can rebind any action from its binding screen:
//!
//! - [`FRAME`], the Steam Frame's own controllers, through
//!   `XR_VALVE_frame_controller_interaction`: the full d-pad, separate
//!   bumpers, the right hand's X, Y and menu. Suggested only when the runtime
//!   enabled the extension ([`FrameControllers`]), since a runtime without it
//!   rejects the profile's paths.
//! - [`TOUCH`], the Oculus Touch controllers. SteamVR falls back to it for
//!   the Frame without the extension, remapping left X to the d-pad's down,
//!   left Y to its other three, right B to B, X and Y, and each trigger to
//!   its bumper as well.
//! - [`SIMPLE`], the Khronos simple controller, for a runtime with nothing
//!   better: a select and a menu button per hand.
//!
//! The Frame's paths are from Valve's Steam Frame input documentation
//! (Steamworks, "Steam Frame: Input"); its profile path is the one SteamVR
//! has used since 2.15.1.

use bevy::prelude::*;

/// The Steam Frame controller profile.
pub(crate) const FRAME: &str = "/interaction_profiles/valve/frame_controller_valve";

/// The extension that offers [`FRAME`], nul-terminated as the `openxr`
/// crate keeps names it has no field for.
pub(crate) const FRAME_EXTENSION: &[u8] = b"XR_VALVE_frame_controller_interaction\0";

/// The Oculus Touch profile.
pub(crate) const TOUCH: &str = "/interaction_profiles/oculus/touch_controller";

/// The Khronos simple controller profile.
pub(crate) const SIMPLE: &str = "/interaction_profiles/khr/simple_controller";

/// Whether the runtime enabled [`FRAME_EXTENSION`], so bindings for
/// [`FRAME`] may be suggested.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct FrameControllers(pub(crate) bool);
