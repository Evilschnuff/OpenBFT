//! What each key, mouse button and pad button does (FR-V7).
//!
//! [`InputBindings`] maps every [`Action`] to the buttons that trigger it.
//! It is persisted in `config.ron` through [`Settings`](super::Settings),
//! rebound on the settings screen, and read by
//! [`input_map`](crate::input_map) and the HUD through [`ActionInput`].
//!
//! A plain table rather than `leafwing-input-manager`: every action is a button, the only analogue input
//! is the mouse — which aims through the cursor, not through a binding — and
//! a `HashMap` of buttons serialises to a config file a player can read and
//! edit. A dependency would buy axis handling and chords this game has no use
//! for.

use std::collections::HashMap;

use bevy::ecs::system::SystemParam;
use bevy::input::gamepad::{Gamepad, GamepadButton};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Something the player can do with a button.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Action {
    Forward,
    Back,
    Left,
    Right,
    /// Fire the selected weapon.
    Fire,
    /// The second trigger: the pulse gun, whatever is selected.
    AltFire,
    /// Select weapon slot 1–9.
    Slot(u8),
    /// Flip between the weak and strong variant of the selected slot.
    ToggleStrong,
    /// Set off every remote mine laid. The original's Spacebar, which Climb
    /// has here.
    Detonate,
    /// Cycle the vehicle form (track, amphib, hover, sub).
    NextForm,
    /// Sub form: go down.
    Dive,
    /// Sub form: go up.
    Climb,
    /// Open or close the pause menu; also backs out of menus.
    Pause,
    /// Enlarge or shrink the automap.
    ToggleMap,
    /// Show or hide the objective list.
    ToggleObjectives,
    /// Write a PNG of the window.
    Screenshot,
    /// The debug overlay (the viewer's info panel, the vehicle gizmos in game).
    DebugPanel,
    /// Dev tools only: pause the game and fly the camera; again (or `Pause`)
    /// to go back.
    FreeCamera,
    /// Save the mission over the quick save: the original's F8.
    QuickSave,
    /// Load the quick save: the original's F9.
    QuickLoad,
}

impl Action {
    /// Every action, in the order the settings screen lists them.
    pub fn all() -> Vec<Action> {
        let mut actions = vec![
            Action::Forward,
            Action::Back,
            Action::Left,
            Action::Right,
            Action::Fire,
            Action::AltFire,
            Action::ToggleStrong,
            Action::Detonate,
            Action::NextForm,
            Action::Climb,
            Action::Dive,
        ];
        actions.extend((1..=9).map(Action::Slot));
        actions.extend([
            Action::Pause,
            Action::ToggleMap,
            Action::ToggleObjectives,
            Action::QuickSave,
            Action::QuickLoad,
            Action::Screenshot,
            Action::DebugPanel,
            Action::FreeCamera,
        ]);
        actions
    }

    /// What the settings screen calls it.
    pub fn label(&self) -> String {
        match self {
            Action::Forward => "Forward".into(),
            Action::Back => "Reverse".into(),
            Action::Left => "Turn left".into(),
            Action::Right => "Turn right".into(),
            Action::Fire => "Fire".into(),
            Action::AltFire => "Alternate fire".into(),
            Action::Slot(slot) => format!("Weapon {slot}"),
            Action::ToggleStrong => "Weak / strong".into(),
            Action::Detonate => "Detonate mines".into(),
            Action::NextForm => "Change form".into(),
            Action::Dive => "Dive".into(),
            Action::Climb => "Climb".into(),
            Action::Pause => "Pause".into(),
            Action::ToggleMap => "Automap".into(),
            Action::ToggleObjectives => "Objectives".into(),
            Action::Screenshot => "Screenshot".into(),
            Action::DebugPanel => "Debug overlay".into(),
            Action::FreeCamera => "Free camera (dev)".into(),
            Action::QuickSave => "Quick save".into(),
            Action::QuickLoad => "Quick load".into(),
        }
    }
}

/// One physical button.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
pub enum Binding {
    Key(KeyCode),
    Mouse(MouseButton),
    Gamepad(GamepadButton),
}

impl Binding {
    /// A short name for the settings screen: `W`, `Mouse left`, `Pad south`.
    pub fn label(&self) -> String {
        match self {
            Binding::Key(key) => key_label(*key),
            Binding::Mouse(button) => match button {
                MouseButton::Left => "Mouse left".into(),
                MouseButton::Right => "Mouse right".into(),
                MouseButton::Middle => "Mouse middle".into(),
                MouseButton::Back => "Mouse back".into(),
                MouseButton::Forward => "Mouse forward".into(),
                MouseButton::Other(n) => format!("Mouse {n}"),
            },
            Binding::Gamepad(button) => format!("Pad {button:?}"),
        }
    }
}

/// `KeyW` → `W`, `Digit1` → `1`, `ArrowUp` → `Up`; anything else as Bevy
/// names it.
fn key_label(key: KeyCode) -> String {
    let name = format!("{key:?}");
    for prefix in ["Key", "Digit", "Arrow"] {
        if let Some(rest) = name.strip_prefix(prefix)
            && !rest.is_empty()
        {
            return rest.to_string();
        }
    }
    name
}

/// Which buttons trigger which action, plus the two mouse settings.
///
/// An action mapped to an empty list is deliberately unbound; an action
/// missing from the map (a config written before the action existed) gets
/// its default when the settings are loaded.
#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct InputBindings {
    pub map: HashMap<Action, Vec<Binding>>,
    /// Mouse sensitivity, as a multiplier on `player.zrd`'s `mouse_push`.
    /// Part of the bindings so it is saved with them.
    pub mouse_sensitivity: f32,
    /// Whether pushing the mouse forward aims down.
    pub invert_pitch: bool,
}

impl Default for InputBindings {
    /// The default table: WASD to drive, the mouse to aim and fire, 1–9 for
    /// slots, Shift for the strong variant, Space and Ctrl to climb and dive,
    /// F to change form — plus G to set off remote mines, Escape to pause, M
    /// for the map, O for the objectives, F1 and F12 as the viewer already
    /// used them, and a pad layout for the buttons that have an obvious home.
    fn default() -> InputBindings {
        use Binding::{Gamepad as Pad, Key, Mouse};

        let mut map: HashMap<Action, Vec<Binding>> = HashMap::from([
            (
                Action::Forward,
                vec![Key(KeyCode::KeyW), Pad(GamepadButton::DPadUp)],
            ),
            (
                Action::Back,
                vec![Key(KeyCode::KeyS), Pad(GamepadButton::DPadDown)],
            ),
            (
                Action::Left,
                vec![Key(KeyCode::KeyA), Pad(GamepadButton::DPadLeft)],
            ),
            (
                Action::Right,
                vec![Key(KeyCode::KeyD), Pad(GamepadButton::DPadRight)],
            ),
            (
                Action::Fire,
                vec![Mouse(MouseButton::Left), Pad(GamepadButton::RightTrigger2)],
            ),
            (
                Action::AltFire,
                vec![Mouse(MouseButton::Right), Pad(GamepadButton::LeftTrigger2)],
            ),
            (
                Action::ToggleStrong,
                vec![Key(KeyCode::ShiftLeft), Pad(GamepadButton::West)],
            ),
            (
                Action::NextForm,
                vec![Key(KeyCode::KeyF), Pad(GamepadButton::North)],
            ),
            (
                Action::Detonate,
                vec![Key(KeyCode::KeyG), Pad(GamepadButton::RightTrigger)],
            ),
            (
                Action::Climb,
                vec![Key(KeyCode::Space), Pad(GamepadButton::South)],
            ),
            (
                Action::Dive,
                vec![Key(KeyCode::ControlLeft), Pad(GamepadButton::East)],
            ),
            (
                Action::Pause,
                vec![Key(KeyCode::Escape), Pad(GamepadButton::Start)],
            ),
            (
                Action::ToggleMap,
                vec![Key(KeyCode::KeyM), Pad(GamepadButton::Select)],
            ),
            // The manual's "Open the Current Mission Objective Message O",
            // and the HUD's own "Incoming transmission - Press O key".
            (Action::ToggleObjectives, vec![Key(KeyCode::KeyO)]),
            (Action::Screenshot, vec![Key(KeyCode::F12)]),
            (Action::DebugPanel, vec![Key(KeyCode::F1)]),
            (Action::FreeCamera, vec![Key(KeyCode::F2)]),
            // The manual's "Save current progress F8, Load saved game F9".
            (Action::QuickSave, vec![Key(KeyCode::F8)]),
            (Action::QuickLoad, vec![Key(KeyCode::F9)]),
        ]);

        const SLOT_KEYS: [KeyCode; 9] = [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
            KeyCode::Digit9,
        ];
        for (index, key) in SLOT_KEYS.into_iter().enumerate() {
            map.insert(Action::Slot(index as u8 + 1), vec![Key(key)]);
        }

        InputBindings {
            map,
            mouse_sensitivity: 1.0,
            invert_pitch: false,
        }
    }
}

impl InputBindings {
    /// The buttons bound to an action; empty when it is unbound.
    pub fn get(&self, action: Action) -> &[Binding] {
        self.map.get(&action).map(Vec::as_slice).unwrap_or_default()
    }

    /// Give every action the file did not mention its default.
    ///
    /// Run after loading, so a config written before an action existed still
    /// gets a key for it — while an action the player cleared on purpose
    /// (present, empty) stays cleared.
    pub fn fill_missing_defaults(&mut self) {
        let defaults = InputBindings::default();
        for action in Action::all() {
            self.map
                .entry(action)
                .or_insert_with(|| defaults.get(action).to_vec());
        }
    }

    /// Bind an action to one button, replacing what it had.
    ///
    /// The button is taken off every other action first, so one key never
    /// does two things — the settings screen shows the other action as
    /// unbound rather than silently leaving a conflict.
    pub fn rebind(&mut self, action: Action, binding: Binding) {
        for (other, bindings) in self.map.iter_mut() {
            if *other != action {
                bindings.retain(|existing| *existing != binding);
            }
        }
        // Pad buttons are kept: rebinding from the keyboard should not strip
        // a controller layout, and the reverse.
        let keep_pad = !matches!(binding, Binding::Gamepad(_));
        let list = self.map.entry(action).or_default();
        list.retain(|existing| keep_pad && matches!(existing, Binding::Gamepad(_)));
        list.insert(0, binding);
    }

    /// The labels of an action's bindings, joined: `W / Pad DPadUp`.
    pub fn describe(&self, action: Action) -> String {
        let bindings = self.get(action);
        if bindings.is_empty() {
            return "unbound".into();
        }
        bindings
            .iter()
            .map(Binding::label)
            .collect::<Vec<_>>()
            .join(" / ")
    }

    /// Whether any button bound to `action` is held.
    pub fn pressed(&self, action: Action, devices: &Devices<'_>) -> bool {
        self.get(action)
            .iter()
            .any(|binding| devices.pressed(*binding))
    }

    /// Whether any button bound to `action` went down this frame.
    pub fn just_pressed(&self, action: Action, devices: &Devices<'_>) -> bool {
        self.get(action)
            .iter()
            .any(|binding| devices.just_pressed(*binding))
    }
}

/// The three kinds of button state, borrowed for one query.
pub struct Devices<'a> {
    pub keys: Option<&'a ButtonInput<KeyCode>>,
    pub mouse: Option<&'a ButtonInput<MouseButton>>,
    pub gamepads: Vec<&'a Gamepad>,
}

impl Devices<'_> {
    fn pressed(&self, binding: Binding) -> bool {
        match binding {
            Binding::Key(key) => self.keys.is_some_and(|keys| keys.pressed(key)),
            Binding::Mouse(button) => self.mouse.is_some_and(|mouse| mouse.pressed(button)),
            Binding::Gamepad(button) => self.gamepads.iter().any(|pad| pad.pressed(button)),
        }
    }

    fn just_pressed(&self, binding: Binding) -> bool {
        match binding {
            Binding::Key(key) => self.keys.is_some_and(|keys| keys.just_pressed(key)),
            Binding::Mouse(button) => self.mouse.is_some_and(|mouse| mouse.just_pressed(button)),
            Binding::Gamepad(button) => self.gamepads.iter().any(|pad| pad.just_pressed(button)),
        }
    }

    /// The first button of any kind that went down this frame, for the
    /// rebinding screen's "press a key".
    pub fn first_just_pressed(&self) -> Option<Binding> {
        if let Some(key) = self.keys.and_then(|keys| keys.get_just_pressed().next()) {
            return Some(Binding::Key(*key));
        }
        if let Some(button) = self.mouse.and_then(|mouse| mouse.get_just_pressed().next()) {
            return Some(Binding::Mouse(*button));
        }
        self.gamepads
            .iter()
            .find_map(|pad| pad.get_just_pressed().next())
            .map(|button| Binding::Gamepad(*button))
    }
}

/// Actions as a system parameter: `input.pressed(Action::Fire)`.
///
/// Every device is optional, so a headless test app that never added the
/// input plugin reads "nothing pressed" rather than failing to build.
#[derive(SystemParam)]
pub struct ActionInput<'w, 's> {
    keys: Option<Res<'w, ButtonInput<KeyCode>>>,
    mouse: Option<Res<'w, ButtonInput<MouseButton>>>,
    gamepads: Query<'w, 's, &'static Gamepad>,
    bindings: Option<Res<'w, InputBindings>>,
}

impl ActionInput<'_, '_> {
    /// The devices, borrowed.
    pub fn devices(&self) -> Devices<'_> {
        Devices {
            keys: self.keys.as_deref(),
            mouse: self.mouse.as_deref(),
            gamepads: self.gamepads.iter().collect(),
        }
    }

    /// Whether the action's button is held.
    pub fn pressed(&self, action: Action) -> bool {
        self.bindings
            .as_ref()
            .is_some_and(|bindings| bindings.pressed(action, &self.devices()))
    }

    /// Whether the action's button went down this frame.
    pub fn just_pressed(&self, action: Action) -> bool {
        self.bindings
            .as_ref()
            .is_some_and(|bindings| bindings.just_pressed(action, &self.devices()))
    }

    /// The bindings themselves, when loaded.
    pub fn bindings(&self) -> Option<&InputBindings> {
        self.bindings.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(pressed: &[KeyCode]) -> ButtonInput<KeyCode> {
        let mut input = ButtonInput::default();
        for key in pressed {
            input.press(*key);
        }
        input
    }

    /// The bindings survive the config file unchanged, including
    /// the tuple variant `Slot(n)` as a map key and all three kinds of
    /// button.
    #[test]
    fn input_bindings_round_trip_through_ron() {
        let mut bindings = InputBindings::default();
        bindings.rebind(Action::Fire, Binding::Key(KeyCode::KeyJ));
        bindings.map.insert(Action::Screenshot, Vec::new());
        bindings.mouse_sensitivity = 1.75;
        bindings.invert_pitch = true;

        let text = ron::ser::to_string_pretty(&bindings, ron::ser::PrettyConfig::default())
            .expect("serialises");
        let back: InputBindings = ron::from_str(&text).expect("parses");

        assert_eq!(back, bindings);
        assert!(text.contains("Slot(3)"), "{text}");
    }

    #[test]
    fn every_action_has_a_default_binding() {
        let defaults = InputBindings::default();
        for action in Action::all() {
            assert!(
                !defaults.get(action).is_empty(),
                "{action:?} has no default"
            );
        }
    }

    /// No key does two things out of the box.
    #[test]
    fn the_defaults_do_not_share_a_button() {
        let defaults = InputBindings::default();
        let mut seen: HashMap<Binding, Action> = HashMap::new();
        for action in Action::all() {
            for binding in defaults.get(action) {
                if let Some(other) = seen.insert(*binding, action) {
                    panic!("{binding:?} is bound to both {other:?} and {action:?}");
                }
            }
        }
    }

    #[test]
    fn a_config_from_before_an_action_existed_gets_its_default() {
        let mut bindings = InputBindings::default();
        bindings.map.remove(&Action::ToggleMap);
        bindings.map.insert(Action::Screenshot, Vec::new());

        bindings.fill_missing_defaults();

        assert_eq!(
            bindings.get(Action::ToggleMap),
            InputBindings::default().get(Action::ToggleMap)
        );
        assert!(
            bindings.get(Action::Screenshot).is_empty(),
            "a deliberately cleared action must stay cleared"
        );
    }

    #[test]
    fn rebinding_takes_the_key_off_whatever_had_it() {
        let mut bindings = InputBindings::default();

        bindings.rebind(Action::Fire, Binding::Key(KeyCode::KeyW));

        assert_eq!(bindings.get(Action::Fire)[0], Binding::Key(KeyCode::KeyW));
        assert!(
            !bindings
                .get(Action::Forward)
                .contains(&Binding::Key(KeyCode::KeyW)),
            "W must no longer drive forward"
        );
        // The mouse button it replaced is gone; the pad trigger stays.
        assert!(
            !bindings
                .get(Action::Fire)
                .contains(&Binding::Mouse(MouseButton::Left))
        );
        assert!(
            bindings
                .get(Action::Fire)
                .contains(&Binding::Gamepad(GamepadButton::RightTrigger2))
        );
    }

    #[test]
    fn an_action_reads_any_of_its_buttons() {
        let bindings = InputBindings::default();
        let held = keys(&[KeyCode::KeyW]);
        let devices = Devices {
            keys: Some(&held),
            mouse: None,
            gamepads: Vec::new(),
        };

        assert!(bindings.pressed(Action::Forward, &devices));
        assert!(bindings.just_pressed(Action::Forward, &devices));
        assert!(!bindings.pressed(Action::Back, &devices));
    }

    #[test]
    fn labels_are_short() {
        assert_eq!(Binding::Key(KeyCode::KeyW).label(), "W");
        assert_eq!(Binding::Key(KeyCode::Digit3).label(), "3");
        assert_eq!(Binding::Key(KeyCode::ArrowUp).label(), "Up");
        assert_eq!(Binding::Key(KeyCode::Escape).label(), "Escape");
        assert_eq!(Binding::Mouse(MouseButton::Left).label(), "Mouse left");
    }
}
