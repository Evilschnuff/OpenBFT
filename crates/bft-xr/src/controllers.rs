//! Controllers: the game's actions on the headset's controllers.
//!
//! Each game [`Action`] worth having in the hand is an OpenXR action of its
//! own, named after it, so the runtime's binding screen (SteamVR's
//! "Controller bindings") lists them by name and the player can put any of
//! them on any button. The suggested layout is for the Oculus Touch profile,
//! which SteamVR remaps onto controllers it has no OpenXR profile for, the
//! Steam Frame's among them, plus the simple controller for a runtime with
//! nothing better:
//!
//! | Touch | Steam Frame (SteamVR's remap) | Action |
//! |---|---|---|
//! | left stick | left stick | drive toward where it points |
//! | left trigger | left trigger or bumper | alternate fire |
//! | left X | left d-pad down | automap |
//! | left Y | left d-pad up, left or right | objectives |
//! | left menu | View | pause |
//! | right trigger | right trigger or bumper | fire |
//! | right grip, seated | right grip | detonate mines |
//! | left stick click | left stick click | detonate mines |
//! | either grip, at the table | the same | hold and move the table |
//! | right A | A | change form |
//! | right B | B, X or Y | weak / strong |
//! | right stick up / down | right stick up / down | climb / dive |
//! | right stick flicked left / right | the same | previous / next weapon |
//! | right stick, flying a guided missile | the same | steer the missile |
//! | right stick click | right stick click | aim with the hand / the head |
//! | right stick held in | right stick held in | table / seat ([`View`]) |
//!
//! The weapon slots, quick save and quick load are actions too, unbound,
//! for a player who wants them on buttons of their own.
//!
//! The actions land in [`ExternalActions`], which `bft-game` reads on top of
//! its own key table, so the game sees a pressed action and not a device.
//!
//! The left stick does not press the drive keys. It names a direction as
//! the player sees it: pushed away is where they look, pushed right is to
//! the right of that, whichever way the tank faces. The tank turns toward
//! that direction and drives along it once it roughly faces it, or backs
//! along it when it lies nearly behind ([`drive_toward`]), as an
//! [`ExternalDrive`]. Where they look is the
//! head's heading ([`head_heading`]), which holds when they look steeply
//! down at the table.
//! While a missile is guided the right stick's two axes steer it instead,
//! through [`ExternalGuide`], and neither climbs, dives nor changes weapon.
//!
//! At the table the grips hold the table ([`table`](crate::table)), so the
//! right grip's detonate is not read there and the left stick's click is
//! the one that detonates.
//!
//! Recoil had no controllers in the hand, so the layout, the stick
//! thresholds, the weapon flick and the long press are presentation
//! choices.

use bevy::prelude::*;
use bevy_mod_openxr::action_binding::{OxrSendActionBindings, OxrSuggestActionBinding};
use bevy_mod_openxr::action_set_attaching::OxrAttachActionSet;
use bevy_mod_openxr::action_set_syncing::{OxrActionSetSyncSet, OxrSyncActionSet};
use bevy_mod_openxr::openxr_session_running;
use bevy_mod_openxr::resources::OxrInstance;
use bevy_mod_openxr::session::OxrSession;
use bevy_mod_xr::camera::XrCamera;
use bevy_mod_xr::session::{XrSessionCreated, session_available};
use bft_game::guided_view::{ExternalGuide, GuidedMissile};
use bft_game::input_map::{ExternalDrive, read_input};
use bft_game::ui::{Action, ExternalActions};
use bft_sim::vehicle::{Controlled, Vehicle};
use bft_sim::weapons::WeaponLoadout;

use crate::{AimSource, View};

/// The Oculus Touch profile, which SteamVR remaps onto the Steam Frame's
/// controllers.
const TOUCH: &str = "/interaction_profiles/oculus/touch_controller";

/// The Khronos simple controller: a select and a menu button per hand.
const SIMPLE: &str = "/interaction_profiles/khr/simple_controller";

/// How far a stick goes over before it counts as a held direction.
///
/// A presentation choice: halfway, so a diagonal (0.71 on each axis) drives
/// and turns at once the way two held keys do.
const STICK_PRESS: f32 = 0.5;

/// How far back a held direction has to come before it lets go: a little
/// short of [`STICK_PRESS`], so a stick resting on the threshold does not
/// chatter.
const STICK_RELEASE: f32 = 0.4;

/// How far the right stick goes sideways to flick to the next weapon.
const FLICK: f32 = 0.7;

/// How near the centre the right stick comes back before the next flick.
const FLICK_RESET: f32 = 0.3;

/// How far the right stick goes over before it steers a guided missile at
/// all; past it the steering ramps from nothing, so a stick at rest flies
/// straight.
const GUIDE_DEAD_ZONE: f32 = 0.1;

/// How far the left stick goes over before it drives at all; past it the
/// speed ramps from nothing.
const DRIVE_DEAD_ZONE: f32 = 0.2;

/// How far off the stick's direction the hull has to be for full steering,
/// radians; nearer, the steering eases off so the hull settles on it
/// instead of swinging past.
const STEER_FULL: f32 = 0.5;

/// How far off the stick's direction the hull can be and still drive,
/// radians: beyond it the hull turns on the spot first, and inside it the
/// throttle opens as it comes round.
const DRIVE_CONE: f32 = std::f32::consts::FRAC_PI_3;

/// How far off straight behind the stick's direction can be for the hull
/// to back toward it, radians: 60° either side, so backing covers 120°
/// behind and driving forward the rest. Forward is how the game is driven;
/// backing is for a direction nearly behind.
const REVERSE_CONE: f32 = std::f32::consts::FRAC_PI_3;

/// How far the stick's direction has to move past the edge of the
/// [`REVERSE_CONE`] before the hull switches ends, radians, so a stick held
/// on the edge does not flip it to and fro.
const REVERSE_HYSTERESIS: f32 = 0.15;

/// How long the right stick's click is held in to switch between the table
/// and the seat instead of switching the aim, seconds.
const LONG_PRESS: f32 = 0.6;

/// The button actions and where the Touch and simple profiles put them.
const BUTTONS: &[(Action, &[&str], &[&str])] = &[
    (
        Action::Fire,
        &["/user/hand/right/input/trigger/value"],
        &["/user/hand/right/input/select/click"],
    ),
    (
        Action::AltFire,
        &["/user/hand/left/input/trigger/value"],
        &["/user/hand/left/input/select/click"],
    ),
    (
        Action::Detonate,
        &[
            "/user/hand/right/input/squeeze/value",
            "/user/hand/left/input/thumbstick/click",
        ],
        &[],
    ),
    (Action::NextForm, &["/user/hand/right/input/a/click"], &[]),
    (
        Action::ToggleStrong,
        &["/user/hand/right/input/b/click"],
        &[],
    ),
    (Action::ToggleMap, &["/user/hand/left/input/x/click"], &[]),
    (
        Action::ToggleObjectives,
        &["/user/hand/left/input/y/click"],
        &[],
    ),
    (
        Action::Pause,
        &["/user/hand/left/input/menu/click"],
        &[
            "/user/hand/left/input/menu/click",
            "/user/hand/right/input/menu/click",
        ],
    ),
    (Action::Slot(1), &[], &[]),
    (Action::Slot(2), &[], &[]),
    (Action::Slot(3), &[], &[]),
    (Action::Slot(4), &[], &[]),
    (Action::Slot(5), &[], &[]),
    (Action::Slot(6), &[], &[]),
    (Action::Slot(7), &[], &[]),
    (Action::Slot(8), &[], &[]),
    (Action::Slot(9), &[], &[]),
    (Action::QuickSave, &[], &[]),
    (Action::QuickLoad, &[], &[]),
];

/// The controller actions and the set they are in.
#[derive(Resource)]
struct ControllerActions {
    set: openxr::ActionSet,
    /// One per entry of [`BUTTONS`], in its order, each read per hand.
    buttons: Vec<(Action, openxr::Action<bool>)>,
    /// `/user/hand/left`, to read a button from that hand alone.
    left_hand: openxr::Path,
    /// The left stick: where to drive, as the player sees it.
    drive: openxr::Action<openxr::Vector2f>,
    /// The right stick's up and down: climb and dive.
    climb_dive: openxr::Action<f32>,
    /// The right stick's left and right: previous and next weapon.
    change_weapon: openxr::Action<f32>,
    /// Aim the turret with the hand or with the head ([`AimSource`]),
    /// clicked; switch between the table and the seat ([`View`]), held.
    switch_aim: openxr::Action<bool>,
}

/// Binds the controllers and presses the game's actions from them.
pub(crate) struct ControllersPlugin;

impl Plugin for ControllersPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExternalActions>()
            .init_resource::<ExternalGuide>()
            .init_resource::<ExternalDrive>()
            .add_systems(Startup, create_actions.run_if(session_available))
            .add_systems(OxrSendActionBindings, suggest_bindings)
            .add_systems(XrSessionCreated, attach_actions)
            .add_systems(
                PreUpdate,
                (
                    sync_actions
                        .before(OxrActionSetSyncSet)
                        .run_if(openxr_session_running),
                    press_actions
                        .after(OxrActionSetSyncSet)
                        .before(read_input)
                        .run_if(openxr_session_running),
                    release_actions
                        .before(read_input)
                        .run_if(not(openxr_session_running)),
                ),
            );
    }
}

/// The OpenXR name of an action: `fire`, `toggle_strong`, `weapon_3`.
fn action_name(action: Action) -> String {
    if let Action::Slot(slot) = action {
        return format!("weapon_{slot}");
    }
    let mut name = String::new();
    for (index, letter) in format!("{action:?}").chars().enumerate() {
        if letter.is_ascii_uppercase() && index > 0 {
            name.push('_');
        }
        name.push(letter.to_ascii_lowercase());
    }
    name
}

/// Make the action set and its actions.
fn create_actions(instance: Res<OxrInstance>, mut commands: Commands) {
    let created = instance
        .create_action_set("gameplay", "Gameplay", 0)
        .and_then(|set| {
            let left_hand = instance.string_to_path("/user/hand/left")?;
            let hands = [left_hand, instance.string_to_path("/user/hand/right")?];
            let buttons = BUTTONS
                .iter()
                .map(|&(action, _, _)| {
                    set.create_action(&action_name(action), &action.label(), &hands)
                        .map(|created| (action, created))
                })
                .collect::<Result<_, _>>()?;
            Ok(ControllerActions {
                left_hand,
                drive: set.create_action("drive", "Drive", &[])?,
                climb_dive: set.create_action("climb_dive", "Climb / dive", &[])?,
                change_weapon: set.create_action("change_weapon", "Previous / next weapon", &[])?,
                switch_aim: set.create_action("switch_aim", "Aim with hand / head", &[])?,
                buttons,
                set,
            })
        });
    match created {
        Ok(actions) => commands.insert_resource(actions),
        Err(error) => warn!("controllers: could not create the actions: {error}"),
    }
}

/// Suggest the layout in the module docs.
fn suggest_bindings(
    actions: Option<Res<ControllerActions>>,
    mut bindings: MessageWriter<OxrSuggestActionBinding>,
) {
    let Some(actions) = actions else {
        return;
    };
    let mut suggest =
        |action: openxr::sys::Action, profile: &'static str, paths: &[&'static str]| {
            if !paths.is_empty() {
                bindings.write(OxrSuggestActionBinding {
                    action,
                    interaction_profile: profile.into(),
                    bindings: paths.iter().map(|&path| path.into()).collect(),
                });
            }
        };

    for ((_, action), &(_, touch, simple)) in actions.buttons.iter().zip(BUTTONS) {
        suggest(action.as_raw(), TOUCH, touch);
        suggest(action.as_raw(), SIMPLE, simple);
    }
    suggest(
        actions.drive.as_raw(),
        TOUCH,
        &["/user/hand/left/input/thumbstick"],
    );
    suggest(
        actions.climb_dive.as_raw(),
        TOUCH,
        &["/user/hand/right/input/thumbstick/y"],
    );
    suggest(
        actions.switch_aim.as_raw(),
        TOUCH,
        &["/user/hand/right/input/thumbstick/click"],
    );
    suggest(
        actions.change_weapon.as_raw(),
        TOUCH,
        &["/user/hand/right/input/thumbstick/x"],
    );
}

/// Attach the set to the new session.
fn attach_actions(
    actions: Option<Res<ControllerActions>>,
    mut attach: MessageWriter<OxrAttachActionSet>,
) {
    if let Some(actions) = actions {
        attach.write(OxrAttachActionSet(actions.set.clone()));
    }
}

/// Ask the runtime for this frame's buttons.
fn sync_actions(
    actions: Option<Res<ControllerActions>>,
    mut sync: MessageWriter<OxrSyncActionSet>,
) {
    if let Some(actions) = actions {
        sync.write(OxrSyncActionSet(actions.set.clone()));
    }
}

/// Press and release the game's actions from this frame's controllers.
///
/// An action the runtime cannot read, or has bound to nothing, reads as let
/// go, as does everything while the headset is off and the session loses
/// focus.
// All ECS injections.
#[allow(clippy::too_many_arguments)]
fn press_actions(
    time: Res<Time>,
    actions: Option<Res<ControllerActions>>,
    session: Res<OxrSession>,
    loadouts: Query<(&WeaponLoadout, &Controlled)>,
    guided: Option<Res<GuidedMissile>>,
    hulls: Query<(&Transform, &Controlled), With<Vehicle>>,
    eyes: Query<&GlobalTransform, With<XrCamera>>,
    mut external: ResMut<ExternalActions>,
    mut guide: ResMut<ExternalGuide>,
    mut drive_out: ResMut<ExternalDrive>,
    mut source: ResMut<AimSource>,
    mut view: ResMut<View>,
    mut flicked: Local<bool>,
    mut stick_click: Local<StickClick>,
    mut reversing: Local<bool>,
) {
    let Some(actions) = actions else {
        return;
    };
    let clicked = actions
        .switch_aim
        .state(&session, openxr::Path::NULL)
        .is_ok_and(|state| state.is_active && state.current_state);
    match stick_click.update(clicked, time.elapsed_secs()) {
        Some(Click::Short) => {
            *source = match *source {
                AimSource::Hand => AimSource::Head,
                AimSource::Head => AimSource::Hand,
            };
            info!(
                "aiming with the {}",
                match *source {
                    AimSource::Hand => "hand",
                    AimSource::Head => "head",
                }
            );
        }
        Some(Click::Long) => {
            *view = match *view {
                View::Table => View::Seat,
                View::Seat => View::Table,
            };
            info!("view: {:?}", *view);
        }
        None => {}
    }

    let buttons = &mut external.0;
    buttons.clear();

    for (action, button) in &actions.buttons {
        // At the table the right grip holds the table, so only the left
        // hand's binding detonates.
        let hand = if *action == Action::Detonate && *view == View::Table {
            actions.left_hand
        } else {
            openxr::Path::NULL
        };
        let held = button
            .state(&session, hand)
            .is_ok_and(|state| state.is_active && state.current_state);
        hold(buttons, *action, held);
    }

    let drive = actions
        .drive
        .state(&session, openxr::Path::NULL)
        .ok()
        .filter(|state| state.is_active)
        .map_or(Vec2::ZERO, |state| {
            Vec2::new(state.current_state.x, state.current_state.y)
        });
    let climb_dive = axis(&actions.climb_dive, &session);
    let change = axis(&actions.change_weapon, &session);

    // Flying a missile: the right stick is its stick.
    let guiding = guided.is_some_and(|guided| guided.is_guiding());
    guide.0 = if guiding {
        steer(Vec2::new(change, climb_dive))
    } else {
        Vec2::ZERO
    };
    let climb_dive = if guiding { 0.0 } else { climb_dive };

    // Where the player looks: last frame's head, which a stick cannot tell.
    let heading = crate::head(&eyes).map_or(Quat::IDENTITY, head_heading);
    let hull = hulls
        .iter()
        .find(|(_, controlled)| **controlled == Controlled::Local)
        .map(|(hull, _)| hull.rotation);
    let (throttle, steer) = hull.map_or((0.0, 0.0), |hull| {
        drive_toward(drive, heading, hull, &mut reversing)
    });
    *drive_out = ExternalDrive { throttle, steer };

    for (action, value) in [(Action::Climb, climb_dive), (Action::Dive, -climb_dive)] {
        let held = deflected(value, buttons.pressed(action));
        hold(buttons, action, held);
    }

    if *flicked || guiding {
        // Off a missile with the stick still over, wait for it to come back.
        *flicked = change.abs() > FLICK_RESET;
    } else if change.abs() >= FLICK {
        *flicked = true;
        let loadout = loadouts
            .iter()
            .find(|(_, controlled)| **controlled == Controlled::Local)
            .map(|(loadout, _)| loadout);
        if let Some(slot) = loadout.and_then(|loadout| next_weapon(loadout, change > 0.0)) {
            // Let go again next frame, when the slot's own action reads up.
            buttons.press(Action::Slot(slot));
        }
    }
}

/// What a click of the right stick turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Click {
    /// Let go before [`LONG_PRESS`].
    Short,
    /// Held in to [`LONG_PRESS`]; reported then, not on letting go.
    Long,
}

/// The right stick's click over time: when it went in, and whether that
/// press has already counted as long.
#[derive(Debug, Default)]
struct StickClick {
    since: Option<f32>,
    long: bool,
}

impl StickClick {
    /// The click held or not at `now`, seconds: a short click as it is let
    /// go, a long one as it reaches [`LONG_PRESS`].
    fn update(&mut self, held: bool, now: f32) -> Option<Click> {
        match (held, self.since) {
            (true, None) => {
                *self = StickClick {
                    since: Some(now),
                    long: false,
                };
                None
            }
            (true, Some(since)) if !self.long && now - since >= LONG_PRESS => {
                self.long = true;
                Some(Click::Long)
            }
            (false, Some(_)) => {
                let long = self.long;
                *self = StickClick::default();
                (!long).then_some(Click::Short)
            }
            _ => None,
        }
    }
}

/// Let go of everything while there is no session to read: the headset
/// went to sleep or the runtime stopped with a button down.
fn release_actions(
    mut external: ResMut<ExternalActions>,
    mut guide: ResMut<ExternalGuide>,
    mut drive: ResMut<ExternalDrive>,
) {
    external.0.reset_all();
    guide.0 = Vec2::ZERO;
    *drive = ExternalDrive::default();
}

/// The way the head faces across the ground, as a turn about world up.
///
/// The head's forward and its up, both flattened onto the ground and added:
/// looking ahead the forward says it, looking straight down at the table
/// the top of the head points the way the player faces, and in between
/// both lean the same way.
fn head_heading(head: Transform) -> Quat {
    let facing = (head.forward().as_vec3() + head.up().as_vec3()).with_y(0.0);
    Dir3::new(facing).map_or(Quat::IDENTITY, |facing| {
        Transform::default().looking_to(facing, Vec3::Y).rotation
    })
}

/// Throttle and steering for the left stick at `stick`, seen facing
/// `heading` in the world, driving a hull turned `hull`: turn the hull's
/// front toward the direction the stick points, or its rear when that
/// direction is within [`REVERSE_CONE`] of straight behind, and drive that
/// end first along it, as fast as the stick is pushed, once it is within
/// [`DRIVE_CONE`] of it. `reversing` is which end it was
/// last time, and changes only [`REVERSE_HYSTERESIS`] past that edge.
fn drive_toward(stick: Vec2, heading: Quat, hull: Quat, reversing: &mut bool) -> (f32, f32) {
    let push = ((stick.length() - DRIVE_DEAD_ZONE) / (1.0 - DRIVE_DEAD_ZONE)).clamp(0.0, 1.0);
    if push <= 0.0 {
        return (0.0, 0.0);
    }
    // Pushed away is the heading's forward, −z.
    let wanted = (heading * Vec3::new(stick.x, 0.0, -stick.y)).xz();
    let facing = (hull * Vec3::NEG_Z).xz();
    if wanted.length_squared() < f32::EPSILON || facing.length_squared() < f32::EPSILON {
        return (0.0, 0.0);
    }
    // How far the wanted direction is round from the facing, positive to
    // the left. Seen from above, the x–z plane is mirrored against `Vec2`'s
    // own (z points toward the viewer), hence the sign.
    let off = -facing.angle_to(wanted);
    let edge = std::f32::consts::PI - REVERSE_CONE;
    *reversing = if *reversing {
        off.abs() > edge - REVERSE_HYSTERESIS
    } else {
        off.abs() > edge + REVERSE_HYSTERESIS
    };
    // Backing, it is the rear that turns toward the stick; the hull turns
    // the same way for the same steering whichever way it drives.
    let (off, direction) = if *reversing {
        (off - std::f32::consts::PI.copysign(off), -1.0)
    } else {
        (off, 1.0)
    };
    // Positive steering turns right.
    let steer = (-off / STEER_FULL).clamp(-1.0, 1.0);
    let aligned = (off.cos() - DRIVE_CONE.cos()) / (1.0 - DRIVE_CONE.cos());
    (direction * push * aligned.clamp(0.0, 1.0), steer)
}

/// The right stick as missile steering: nothing inside
/// [`GUIDE_DEAD_ZONE`], ramping to full deflection at the rim.
fn steer(stick: Vec2) -> Vec2 {
    let ramp = |value: f32| {
        let excess = (value.abs() - GUIDE_DEAD_ZONE).max(0.0) / (1.0 - GUIDE_DEAD_ZONE);
        excess.min(1.0) * value.signum()
    };
    Vec2::new(ramp(stick.x), ramp(stick.y))
}

/// Press or release `action`.
fn hold(buttons: &mut ButtonInput<Action>, action: Action, held: bool) {
    if held {
        buttons.press(action);
    } else {
        buttons.release(action);
    }
}

/// A float action's value, or 0 when it is unbound or unreadable.
fn axis(action: &openxr::Action<f32>, session: &OxrSession) -> f32 {
    action
        .state(session, openxr::Path::NULL)
        .ok()
        .filter(|state| state.is_active)
        .map_or(0.0, |state| state.current_state)
}

/// Whether a stick direction is held, given whether it was: pressed past
/// [`STICK_PRESS`], let go short of [`STICK_RELEASE`].
fn deflected(value: f32, was: bool) -> bool {
    value >= if was { STICK_RELEASE } else { STICK_PRESS }
}

/// The next selectable slot after the selected one, upward or downward,
/// round the end; `None` when no other slot has a weapon.
fn next_weapon(loadout: &WeaponLoadout, upward: bool) -> Option<u8> {
    (1..9u8)
        .map(|step| {
            let offset = if upward { step } else { 9 - step };
            (loadout.selected - 1 + offset) % 9 + 1
        })
        .find(|&slot| loadout.is_selectable(slot))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_names_are_lowercase_words_joined_by_underscores() {
        assert_eq!(action_name(Action::Fire), "fire");
        assert_eq!(action_name(Action::ToggleStrong), "toggle_strong");
        assert_eq!(action_name(Action::AltFire), "alt_fire");
        assert_eq!(action_name(Action::Slot(3)), "weapon_3");
    }

    #[test]
    fn a_held_direction_lets_go_short_of_where_it_pressed() {
        assert!(!deflected(0.45, false));
        assert!(deflected(0.5, false));
        assert!(deflected(0.45, true));
        assert!(!deflected(0.35, true));
        assert!(!deflected(-0.9, false));
    }

    /// A loadout with weapons mounted in `slots`, `selected` selected.
    fn loadout(slots: &[u8], selected: u8) -> WeaponLoadout {
        let mut loadout = WeaponLoadout {
            selected,
            ..default()
        };
        for &slot in slots {
            loadout.slot_mut(slot).unwrap().weak = Some(bft_sim::weapons::AmmoState {
                weapon: 0,
                ammo: 1,
                interval: 1.0,
                mounted: true,
            });
        }
        loadout
    }

    #[test]
    fn the_weapon_flick_skips_empty_slots_and_wraps() {
        let carried = loadout(&[1, 4, 9], 4);
        assert_eq!(next_weapon(&carried, true), Some(9));
        assert_eq!(next_weapon(&carried, false), Some(1));
        assert_eq!(next_weapon(&loadout(&[1, 4, 9], 9), true), Some(1));
        assert_eq!(next_weapon(&loadout(&[1, 4, 9], 1), false), Some(9));
        assert_eq!(next_weapon(&loadout(&[1], 1), true), None);
    }

    #[test]
    fn the_stick_steers_from_its_dead_zone_to_its_rim() {
        assert_eq!(steer(Vec2::new(0.05, -0.08)), Vec2::ZERO);
        assert_eq!(steer(Vec2::new(1.0, -1.0)), Vec2::new(1.0, -1.0));
        let half = steer(Vec2::new(0.55, 0.0));
        assert!((half.x - 0.5).abs() < 1e-6, "{half:?}");
    }

    #[test]
    fn the_stick_turns_the_hull_toward_where_it_points_then_drives() {
        let room = Quat::from_rotation_y(0.4);
        // Pushed away, hull facing away: straight on, full speed.
        let (throttle, steer) = drive_toward(Vec2::Y, room, room, &mut false);
        assert!((throttle - 1.0).abs() < 1e-5 && steer.abs() < 1e-5);

        // Pushed right, hull facing away: turn right on the spot.
        let (throttle, steer) = drive_toward(Vec2::X, room, room, &mut false);
        assert_eq!(throttle, 0.0);
        assert_eq!(steer, 1.0);

        // Pushed left: turn left.
        let (_, steer) = drive_toward(Vec2::NEG_X, room, room, &mut false);
        assert_eq!(steer, -1.0);

        // Pushed right with the hull already facing the room's right: drive.
        let right = room * Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
        let (throttle, steer) = drive_toward(Vec2::X, room, right, &mut false);
        assert!(
            (throttle - 1.0).abs() < 1e-5 && steer.abs() < 1e-5,
            "{throttle} {steer}"
        );

        // Slightly off: steer gently back, drive a little slower.
        let near = room * Quat::from_rotation_y(0.1);
        let (throttle, steer) = drive_toward(Vec2::Y, room, near, &mut false);
        assert!(steer > 0.0 && steer < 1.0, "{steer}");
        assert!(throttle > 0.9 && throttle < 1.0, "{throttle}");

        // Resting in the dead zone: nothing.
        assert_eq!(
            drive_toward(Vec2::new(0.1, 0.1), room, room, &mut false),
            (0.0, 0.0)
        );
    }

    #[test]
    fn a_direction_behind_the_hull_is_backed_toward() {
        let room = Quat::from_rotation_y(0.4);
        let mut reversing = false;
        // Pulled straight back: back up, no turn.
        let (throttle, steer) = drive_toward(Vec2::NEG_Y, room, room, &mut reversing);
        assert!(reversing);
        assert!(
            (throttle + 1.0).abs() < 1e-5 && steer.abs() < 1e-5,
            "{throttle} {steer}"
        );

        // Back and a little right: the rear swings right, so the hull turns
        // left, as it would driving forward to the front's left.
        let (throttle, steer) = drive_toward(Vec2::new(0.3, -1.0), room, room, &mut reversing);
        assert!(throttle < 0.0, "{throttle}");
        assert!(steer < 0.0, "{steer}");

        // A stick `degrees` round to the right of where the hull faces.
        let round = |degrees: f32| {
            let angle = degrees.to_radians();
            Vec2::new(angle.sin(), angle.cos())
        };
        // Backing, it keeps backing a little past the cone's edge (120°)...
        drive_toward(round(115.0), room, room, &mut reversing);
        assert!(reversing);
        // ...and drives forward toward anything nearer the front.
        let (throttle, _) = drive_toward(round(100.0), room, room, &mut reversing);
        assert!(!reversing);
        assert!(throttle >= 0.0);
        // Going forward, it turns round rather than back a little past
        // the edge...
        drive_toward(round(125.0), room, room, &mut reversing);
        assert!(!reversing);
        // ...and backs once well inside the cone.
        drive_toward(round(135.0), room, room, &mut reversing);
        assert!(reversing);
    }

    #[test]
    fn the_heading_holds_looking_down_at_the_table() {
        let facing = |heading: Quat| heading * Vec3::NEG_Z;
        // Looking ahead, turned a quarter left.
        let ahead = Transform::default().looking_to(Vec3::NEG_X, Vec3::Y);
        assert!(facing(head_heading(ahead)).distance(Vec3::NEG_X) < 1e-5);
        // Looking straight down, the top of the head toward −x.
        let down = Transform::default().looking_to(Vec3::NEG_Y, Vec3::NEG_X);
        assert!(facing(head_heading(down)).distance(Vec3::NEG_X) < 1e-5);
        // Halfway down.
        let tilted = Transform::default().looking_to(Vec3::new(-1.0, -1.0, 0.0), Vec3::Y);
        assert!(facing(head_heading(tilted)).distance(Vec3::NEG_X) < 1e-5);
    }

    #[test]
    fn a_stick_click_is_short_on_letting_go_or_long_once_held() {
        let mut click = StickClick::default();
        assert_eq!(click.update(true, 0.0), None);
        assert_eq!(click.update(true, 0.3), None);
        assert_eq!(click.update(false, 0.4), Some(Click::Short));

        assert_eq!(click.update(true, 1.0), None);
        assert_eq!(click.update(true, 1.0 + LONG_PRESS), Some(Click::Long));
        assert_eq!(click.update(true, 2.5), None);
        assert_eq!(click.update(false, 3.0), None);
        assert_eq!(click.update(false, 3.1), None);
    }

    #[test]
    fn every_button_action_has_a_distinct_name() {
        let mut names: Vec<_> = BUTTONS
            .iter()
            .map(|&(action, _, _)| action_name(action))
            .collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), BUTTONS.len());
    }
}
