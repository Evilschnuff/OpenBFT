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
//! | left stick | left stick | drive and turn |
//! | left trigger | left trigger or bumper | alternate fire |
//! | left X | left d-pad down | automap |
//! | left Y | left d-pad up, left or right | objectives |
//! | left menu | View | pause |
//! | right trigger | right trigger or bumper | fire |
//! | right grip | right grip | detonate mines |
//! | right A | A | change form |
//! | right B | B, X or Y | weak / strong |
//! | right stick up / down | right stick up / down | climb / dive |
//! | right stick flicked left / right | the same | previous / next weapon |
//! | right stick, flying a guided missile | the same | steer the missile |
//! | right stick click | right stick click | aim with the hand / the head |
//!
//! The weapon slots, quick save and quick load are actions too, unbound,
//! for a player who wants them on buttons of their own.
//!
//! The actions land in [`ExternalActions`], which `bft-game` reads on top of
//! its own key table, so the game sees a pressed action and not a device.
//! While a missile is guided the right stick's two axes steer it instead,
//! through [`ExternalGuide`], and neither climbs, dives nor changes weapon.
//!
//! Recoil had no controllers in the hand, so the layout, the stick
//! thresholds and the weapon flick are presentation choices.

use bevy::prelude::*;
use bevy_mod_openxr::action_binding::{OxrSendActionBindings, OxrSuggestActionBinding};
use bevy_mod_openxr::action_set_attaching::OxrAttachActionSet;
use bevy_mod_openxr::action_set_syncing::{OxrActionSetSyncSet, OxrSyncActionSet};
use bevy_mod_openxr::openxr_session_running;
use bevy_mod_openxr::resources::OxrInstance;
use bevy_mod_openxr::session::OxrSession;
use bevy_mod_xr::session::{XrSessionCreated, session_available};
use bft_game::guided_view::{ExternalGuide, GuidedMissile};
use bft_game::input_map::read_input;
use bft_game::ui::{Action, ExternalActions};
use bft_sim::vehicle::Controlled;
use bft_sim::weapons::WeaponLoadout;

use crate::AimSource;

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
        &["/user/hand/right/input/squeeze/value"],
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
    /// One per entry of [`BUTTONS`], in its order.
    buttons: Vec<(Action, openxr::Action<bool>)>,
    /// The left stick: forward, back and the turns.
    drive: openxr::Action<openxr::Vector2f>,
    /// The right stick's up and down: climb and dive.
    climb_dive: openxr::Action<f32>,
    /// The right stick's left and right: previous and next weapon.
    change_weapon: openxr::Action<f32>,
    /// Aim the turret with the hand or with the head ([`AimSource`]).
    switch_aim: openxr::Action<bool>,
}

/// Binds the controllers and presses the game's actions from them.
pub(crate) struct ControllersPlugin;

impl Plugin for ControllersPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ExternalActions>()
            .init_resource::<ExternalGuide>()
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
            let buttons = BUTTONS
                .iter()
                .map(|&(action, _, _)| {
                    set.create_action(&action_name(action), &action.label(), &[])
                        .map(|created| (action, created))
                })
                .collect::<Result<_, _>>()?;
            Ok(ControllerActions {
                drive: set.create_action("drive", "Drive and turn", &[])?,
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
// Eight parameters, all ECS injections.
#[allow(clippy::too_many_arguments)]
fn press_actions(
    actions: Option<Res<ControllerActions>>,
    session: Res<OxrSession>,
    loadouts: Query<(&WeaponLoadout, &Controlled)>,
    guided: Option<Res<GuidedMissile>>,
    mut external: ResMut<ExternalActions>,
    mut guide: ResMut<ExternalGuide>,
    mut source: ResMut<AimSource>,
    mut flicked: Local<bool>,
) {
    let Some(actions) = actions else {
        return;
    };
    let switched = actions
        .switch_aim
        .state(&session, openxr::Path::NULL)
        .is_ok_and(|state| state.is_active && state.changed_since_last_sync && state.current_state);
    if switched {
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

    let buttons = &mut external.0;
    buttons.clear();

    for (action, button) in &actions.buttons {
        let held = button
            .state(&session, openxr::Path::NULL)
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

    for (action, value) in [
        (Action::Forward, drive.y),
        (Action::Back, -drive.y),
        (Action::Right, drive.x),
        (Action::Left, -drive.x),
        (Action::Climb, climb_dive),
        (Action::Dive, -climb_dive),
    ] {
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

/// Let go of everything while there is no session to read: the headset
/// went to sleep or the runtime stopped with a button down.
fn release_actions(mut external: ResMut<ExternalActions>, mut guide: ResMut<ExternalGuide>) {
    external.0.reset_all();
    guide.0 = Vec2::ZERO;
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
