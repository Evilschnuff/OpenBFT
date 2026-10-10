//! The settings panel: video, audio, controls and rebinding (FR-U3, FR-V7).
//!
//! One panel, spawned in two places — the front end's `Settings` screen and
//! the pause menu — so it knows nothing about which: it edits [`Settings`]
//! and emits [`UiAction::Back`](super::theme::UiAction::Back) when done, and
//! each host decides what "back" means. Every edit lands in [`Settings`] and
//! [`sync_settings`](super::settings::sync_settings) takes it from there.
//!
//! Three tabs rather than one scrolling page: the controls alone are 24 rows,
//! and a tab of two short columns fits a 720-line window without a scroll
//! area.

use bevy::prelude::*;
use bft_assets::DataPaths;

use super::bindings::{Action, ActionInput, Binding, InputBindings};
use super::settings::{Settings, next_resolution};
use super::theme::{
    self, BODY_SIZE, ClickGuard, SMALL_SIZE, Selected, TEXT, TEXT_DIM, UiAction, UiClick, button,
    label_text,
};

/// The panel's three pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsTab {
    #[default]
    Video,
    Audio,
    Controls,
}

impl SettingsTab {
    const ALL: [SettingsTab; 3] = [
        SettingsTab::Video,
        SettingsTab::Audio,
        SettingsTab::Controls,
    ];

    fn label(self) -> &'static str {
        match self {
            SettingsTab::Video => "Video",
            SettingsTab::Audio => "Audio",
            SettingsTab::Controls => "Controls",
        }
    }
}

/// One setting the panel can change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingKey {
    WindowMode,
    Resolution,
    VSync,
    TextureFilter,
    HudOriginal,
    HudStrips,
    HudScale,
    HudClock,
    /// The original's dialogs or the simple menus.
    MenusOriginal,
    Master,
    Effects,
    Music,
    Mute,
    /// Music on or off: the original's `MUSIC_ENABLE` box. Off sets the
    /// music volume to nothing, on back to the default.
    MusicOn,
    Sensitivity,
    InvertPitch,
}

/// What a settings button does to its setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingControl {
    /// Flip a switch or move to the next choice.
    Cycle(SettingKey),
    /// Nudge a number down (−1) or up (+1).
    Step(SettingKey, i8),
    /// Set a volume to a percentage: the original's sliders.
    Set(SettingKey, u8),
}

/// Volume steps: tenths, which is as fine as a click-per-step control
/// wants to be.
const VOLUME_STEP: f32 = 0.1;
/// Mouse sensitivity steps and range.
const SENSITIVITY_STEP: f32 = 0.1;
const SENSITIVITY_RANGE: (f32, f32) = (0.1, 5.0);
/// HUD size steps.
const HUD_SCALE_STEP: f32 = 0.1;

/// Apply one control to the settings.
pub fn apply_control(settings: &mut Settings, control: SettingControl) {
    let step = |value: &mut f32, direction: i8, size: f32, (low, high): (f32, f32)| {
        // Rounded to the step so repeated clicks land on 0.3, not
        // 0.30000001, and the label never shows a stray digit.
        let next = *value + f32::from(direction) * size;
        *value = ((next / size).round() * size).clamp(low, high);
    };

    match control {
        SettingControl::Cycle(key) => match key {
            SettingKey::WindowMode => {
                settings.video.window_mode = settings.video.window_mode.next();
            }
            SettingKey::Resolution => {
                settings.video.resolution = next_resolution(settings.video.resolution);
            }
            SettingKey::VSync => settings.video.vsync = !settings.video.vsync,
            SettingKey::TextureFilter => {
                settings.texture_filter_linear = !settings.texture_filter_linear;
            }
            SettingKey::HudOriginal => settings.hud.original = !settings.hud.original,
            SettingKey::HudStrips => settings.hud.strips = !settings.hud.strips,
            SettingKey::HudClock => settings.hud.clock = !settings.hud.clock,
            SettingKey::MenusOriginal => settings.menus.original = !settings.menus.original,
            SettingKey::Mute => settings.audio.muted = !settings.audio.muted,
            SettingKey::MusicOn => {
                settings.audio.music = if settings.audio.music > 0.0 {
                    0.0
                } else {
                    bft_assets::config::AudioSettings::default().music
                };
            }
            SettingKey::InvertPitch => {
                settings.input.invert_pitch = !settings.input.invert_pitch;
            }
            // Numbers are stepped, not cycled; a cycle on one is a no-op.
            SettingKey::Master
            | SettingKey::Effects
            | SettingKey::Music
            | SettingKey::HudScale
            | SettingKey::Sensitivity => {}
        },
        SettingControl::Step(key, direction) => match key {
            SettingKey::Master => step(
                &mut settings.audio.master,
                direction,
                VOLUME_STEP,
                (0.0, 1.0),
            ),
            SettingKey::Effects => {
                step(
                    &mut settings.audio.effects,
                    direction,
                    VOLUME_STEP,
                    (0.0, 1.0),
                );
            }
            SettingKey::Music => step(
                &mut settings.audio.music,
                direction,
                VOLUME_STEP,
                (0.0, 1.0),
            ),
            SettingKey::Sensitivity => step(
                &mut settings.input.mouse_sensitivity,
                direction,
                SENSITIVITY_STEP,
                SENSITIVITY_RANGE,
            ),
            SettingKey::HudScale => step(
                &mut settings.hud.scale,
                direction,
                HUD_SCALE_STEP,
                super::settings::HUD_SCALE_RANGE,
            ),
            // Switches are cycled.
            _ => apply_control(settings, SettingControl::Cycle(key)),
        },
        SettingControl::Set(key, percent) => {
            let level = (f32::from(percent.min(100)) / 100.0 / VOLUME_STEP).round() * VOLUME_STEP;
            match key {
                SettingKey::Master => settings.audio.master = level,
                SettingKey::Effects => settings.audio.effects = level,
                SettingKey::Music => settings.audio.music = level,
                // Only the volumes are sliders.
                _ => {}
            }
        }
    }
}

/// What the panel shows for a setting's current value.
pub fn value_label(settings: &Settings, key: SettingKey) -> String {
    let on_off = |on: bool| if on { "On" } else { "Off" }.to_string();
    let percent = |value: f32| format!("{:.0}%", value * 100.0);

    match key {
        SettingKey::WindowMode => settings.video.window_mode.label().to_string(),
        SettingKey::Resolution => {
            let (width, height) = settings.video.resolution;
            format!("{width} x {height}")
        }
        SettingKey::VSync => on_off(settings.video.vsync),
        SettingKey::TextureFilter => if settings.texture_filter_linear {
            "Smooth"
        } else {
            "Sharp (nearest)"
        }
        .to_string(),
        SettingKey::HudOriginal => if settings.hud.original {
            "Original"
        } else {
            "Simple"
        }
        .to_string(),
        SettingKey::HudStrips => on_off(settings.hud.strips),
        SettingKey::HudScale => percent(settings.hud.scale),
        SettingKey::HudClock => on_off(settings.hud.clock),
        SettingKey::MenusOriginal => if settings.menus.original {
            "Original"
        } else {
            "Simple"
        }
        .to_string(),
        SettingKey::MusicOn => on_off(settings.audio.music > 0.0),
        SettingKey::Master => percent(settings.audio.master),
        SettingKey::Effects => percent(settings.audio.effects),
        SettingKey::Music => percent(settings.audio.music),
        SettingKey::Mute => on_off(settings.audio.muted),
        SettingKey::Sensitivity => format!("{:.1}", settings.input.mouse_sensitivity),
        SettingKey::InvertPitch => on_off(settings.input.invert_pitch),
    }
}

/// Which tab is open.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CurrentSettingsTab(pub SettingsTab);

/// Which action, if any, is waiting for its new button.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RebindState {
    pub action: Option<Action>,
    /// Set the frame after the request, so the input that made the request
    /// cannot also answer it.
    armed: bool,
}

impl RebindState {
    /// Whether a key press should go to the rebinding rather than anywhere
    /// else.
    pub fn is_capturing(&self) -> bool {
        self.action.is_some()
    }
}

/// The panel's root.
#[derive(Component, Debug, Clone, Copy)]
pub struct SettingsPanel;

/// The part of the panel the tabs swap.
#[derive(Component, Debug, Clone, Copy)]
struct SettingsBody;

/// A text showing a setting's value.
#[derive(Component, Debug, Clone, Copy)]
struct SettingValue(SettingKey);

/// A text showing an action's bindings.
#[derive(Component, Debug, Clone, Copy)]
struct BindingValue(Action);

/// A tab button, so the open one can be marked.
#[derive(Component, Debug, Clone, Copy)]
struct TabButton(SettingsTab);

/// The systems that run the panel wherever it is spawned.
pub fn plugin(app: &mut App) {
    app.init_resource::<CurrentSettingsTab>()
        .init_resource::<RebindState>()
        .add_systems(
            Update,
            (
                capture_rebind,
                handle_settings_clicks,
                rebuild_body,
                mark_open_tab,
                refresh_values,
            )
                .chain()
                .after(theme::dispatch_clicks),
        );
}

/// Spawn the panel under `parent`. The caller owns its lifetime.
pub fn spawn_settings_panel(commands: &mut Commands, parent: Entity, paths: Option<&DataPaths>) {
    let tabs: Vec<_> = SettingsTab::ALL
        .into_iter()
        .map(|tab| {
            commands
                .spawn((
                    button(tab.label(), UiAction::SettingsTab(tab), 150.0),
                    TabButton(tab),
                ))
                .id()
        })
        .collect();
    let tab_row = commands
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: px(8),
            ..default()
        })
        .add_children(&tabs)
        .id();

    let body = commands
        .spawn((
            SettingsBody,
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                min_height: px(380),
                ..default()
            },
        ))
        .id();

    let data_line = paths
        .map(|paths| {
            format!(
                "Game data: {}    Cache: {}",
                paths.root.path.display(),
                paths.cache.path.display()
            )
        })
        .unwrap_or_default();

    let footer = commands
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: px(16),
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|footer| {
            footer.spawn(button("Back", UiAction::Back, 150.0));
            footer.spawn(label_text(data_line, SMALL_SIZE, TEXT_DIM));
        })
        .id();

    let panel = commands
        .spawn((SettingsPanel, theme::panel()))
        .insert(Node {
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(px(20)),
            row_gap: px(14),
            border: UiRect::left(px(3)),
            width: px(900),
            max_width: percent(100),
            ..default()
        })
        .add_children(&[tab_row, body, footer])
        .id();

    commands.entity(parent).add_child(panel);
}

/// Fill the body for the open tab, whenever the tab changes or a new panel
/// appears.
fn rebuild_body(
    mut commands: Commands,
    tab: Res<CurrentSettingsTab>,
    bodies: Query<(Entity, Ref<SettingsBody>)>,
) {
    for (body, marker) in &bodies {
        if !tab.is_changed() && !marker.is_added() {
            continue;
        }

        commands.entity(body).despawn_related::<Children>();
        commands.entity(body).with_children(|body| match tab.0 {
            SettingsTab::Video => {
                cycle_row(body, "Window", SettingKey::WindowMode);
                cycle_row(body, "Resolution (windowed)", SettingKey::Resolution);
                cycle_row(body, "Vertical sync", SettingKey::VSync);
                cycle_row(body, "Texture filtering", SettingKey::TextureFilter);
                cycle_row(body, "HUD", SettingKey::HudOriginal);
                cycle_row(body, "HUD strips", SettingKey::HudStrips);
                step_row(body, "HUD size", SettingKey::HudScale);
                cycle_row(body, "Game clock", SettingKey::HudClock);
                cycle_row(body, "Menus", SettingKey::MenusOriginal);
            }
            SettingsTab::Audio => {
                step_row(body, "Master volume", SettingKey::Master);
                step_row(body, "Effects volume", SettingKey::Effects);
                step_row(body, "Music volume", SettingKey::Music);
                cycle_row(body, "Mute", SettingKey::Mute);
            }
            SettingsTab::Controls => controls(body),
        });
    }
}

/// A row with a label and one button that shows and cycles the value.
fn cycle_row(parent: &mut ChildSpawnerCommands, label: &str, key: SettingKey) {
    parent.spawn(row()).with_children(|row| {
        row.spawn((label_text(label, BODY_SIZE, TEXT), label_width()));
        row.spawn(button(
            "",
            UiAction::Setting(SettingControl::Cycle(key)),
            220.0,
        ))
        .with_children(|button| {
            // The button's own label is replaced by a live one.
            button.spawn((label_text("", BODY_SIZE, TEXT), SettingValue(key)));
        });
    });
}

/// A row with a label, a value and − / + buttons.
fn step_row(parent: &mut ChildSpawnerCommands, label: &str, key: SettingKey) {
    parent.spawn(row()).with_children(|row| {
        row.spawn((label_text(label, BODY_SIZE, TEXT), label_width()));
        row.spawn(button(
            "-",
            UiAction::Setting(SettingControl::Step(key, -1)),
            44.0,
        ));
        row.spawn((
            label_text("", BODY_SIZE, TEXT),
            SettingValue(key),
            Node {
                width: px(80),
                justify_content: JustifyContent::Center,
                ..default()
            },
            TextLayout::justify(Justify::Center),
        ));
        row.spawn(button(
            "+",
            UiAction::Setting(SettingControl::Step(key, 1)),
            44.0,
        ));
    });
}

/// The controls tab: the two mouse settings, then every action in two
/// columns.
fn controls(parent: &mut ChildSpawnerCommands) {
    step_row(parent, "Mouse sensitivity", SettingKey::Sensitivity);
    cycle_row(parent, "Invert mouse pitch", SettingKey::InvertPitch);

    let actions = Action::all();
    let half = actions.len().div_ceil(2);

    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: px(24),
            margin: UiRect::top(px(8)),
            ..default()
        })
        .with_children(|columns| {
            for column in actions.chunks(half) {
                columns
                    .spawn(Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: px(3),
                        ..default()
                    })
                    .with_children(|list| {
                        for action in column {
                            binding_row(list, *action);
                        }
                    });
            }
        });

    parent.spawn(label_text(
        "Click a binding, then press the new key, mouse or pad button. Escape cancels.",
        SMALL_SIZE,
        TEXT_DIM,
    ));
    parent.spawn(button("Reset to defaults", UiAction::ResetBindings, 220.0));
}

/// One action and its bindings, the latter a button that starts a rebind.
fn binding_row(parent: &mut ChildSpawnerCommands, action: Action) {
    parent.spawn(row()).with_children(|row| {
        row.spawn((
            label_text(action.label(), SMALL_SIZE, TEXT),
            Node {
                width: px(140),
                ..default()
            },
        ));
        row.spawn(button("", UiAction::Rebind(action), 250.0))
            // Over the helper's own `Node`: shorter and left-aligned, so a
            // column of these reads as a table.
            .insert(Node {
                width: px(250),
                min_height: px(26),
                padding: UiRect::axes(px(8), px(2)),
                justify_content: JustifyContent::FlexStart,
                align_items: AlignItems::Center,
                border: UiRect::all(px(1)),
                ..default()
            })
            .with_children(|button| {
                button.spawn((label_text("", SMALL_SIZE, TEXT), BindingValue(action)));
            });
    });
}

fn row() -> Node {
    Node {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        column_gap: px(8),
        ..default()
    }
}

fn label_width() -> Node {
    Node {
        width: px(260),
        ..default()
    }
}

/// Act on the panel's buttons.
fn handle_settings_clicks(
    mut clicks: MessageReader<UiClick>,
    mut settings: ResMut<Settings>,
    mut tab: ResMut<CurrentSettingsTab>,
    mut rebind: ResMut<RebindState>,
) {
    for click in clicks.read() {
        match click.0 {
            UiAction::SettingsTab(next) => {
                tab.set_if_neq(CurrentSettingsTab(next));
            }
            UiAction::Setting(control) => apply_control(&mut settings, control),
            UiAction::Rebind(action) => {
                *rebind = RebindState {
                    action: Some(action),
                    armed: false,
                };
            }
            UiAction::ResetBindings => {
                settings.input = InputBindings::default();
                rebind.action = None;
            }
            _ => {}
        }
    }
}

/// Wait for the button that answers "press a key", and bind it.
///
/// Escape cancels, which means Escape itself can only be bound by resetting
/// to the defaults — the price of a cancel key that works everywhere.
fn capture_rebind(
    input: ActionInput,
    mut state: ResMut<RebindState>,
    mut settings: ResMut<Settings>,
    mut guard: ResMut<ClickGuard>,
) {
    let Some(action) = state.action else {
        return;
    };
    if !state.armed {
        state.armed = true;
        return;
    }

    let Some(binding) = input.devices().first_just_pressed() else {
        return;
    };

    if binding != Binding::Key(KeyCode::Escape) {
        settings.input.rebind(action, binding);
        info!("{action:?} bound to {}", binding.label());
    }
    if matches!(binding, Binding::Mouse(_)) {
        // The press answered the question; its release must not click the
        // button under the cursor.
        guard.0 = true;
    }
    *state = RebindState::default();
}

/// Mark the open tab's button.
fn mark_open_tab(
    mut commands: Commands,
    tab: Res<CurrentSettingsTab>,
    buttons: Query<(Entity, &TabButton, Has<Selected>)>,
) {
    for (entity, button, selected) in &buttons {
        match (button.0 == tab.0, selected) {
            (true, false) => {
                commands.entity(entity).insert(Selected);
            }
            (false, true) => {
                commands.entity(entity).remove::<Selected>();
            }
            _ => {}
        }
    }
}

/// Keep the value labels in step with the settings.
#[allow(clippy::type_complexity)]
fn refresh_values(
    settings: Res<Settings>,
    rebind: Res<RebindState>,
    mut values: Query<(&mut Text, Ref<SettingValue>), Without<BindingValue>>,
    mut bindings: Query<(&mut Text, Ref<BindingValue>, &mut TextColor), Without<SettingValue>>,
) {
    let all = settings.is_changed() || rebind.is_changed();

    for (mut text, value) in &mut values {
        if all || value.is_added() {
            text.set_if_neq(Text::new(value_label(&settings, value.0)));
        }
    }
    for (mut text, value, mut colour) in &mut bindings {
        if !(all || value.is_added()) {
            continue;
        }
        let waiting = rebind.action == Some(value.0);
        text.set_if_neq(Text::new(if waiting {
            "Press a button...".to_string()
        } else {
            settings.input.describe(value.0)
        }));
        colour.set_if_neq(TextColor(if waiting { theme::ACCENT } else { TEXT }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volumes_step_in_tenths_and_stay_in_range() {
        let mut settings = Settings::default();
        settings.audio.master = 0.95;

        apply_control(&mut settings, SettingControl::Step(SettingKey::Master, 1));
        assert_eq!(settings.audio.master, 1.0);

        for _ in 0..15 {
            apply_control(&mut settings, SettingControl::Step(SettingKey::Master, -1));
        }
        assert_eq!(settings.audio.master, 0.0);

        apply_control(&mut settings, SettingControl::Step(SettingKey::Music, 1));
        assert_eq!(value_label(&settings, SettingKey::Music), "70%");
    }

    #[test]
    fn switches_flip() {
        let mut settings = Settings::default();

        apply_control(&mut settings, SettingControl::Cycle(SettingKey::VSync));
        apply_control(
            &mut settings,
            SettingControl::Cycle(SettingKey::TextureFilter),
        );
        apply_control(
            &mut settings,
            SettingControl::Cycle(SettingKey::InvertPitch),
        );

        assert!(!settings.video.vsync);
        assert!(!settings.texture_filter_linear);
        assert!(settings.input.invert_pitch);
        assert_eq!(value_label(&settings, SettingKey::VSync), "Off");
    }

    /// The rebind flow end to end: the click asks, the next frame arms, the
    /// next key press answers, and the settings change.
    #[test]
    fn a_rebind_takes_the_next_key_press() {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .add_message::<UiClick>()
            .init_resource::<ClickGuard>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .insert_resource(Settings::default())
            .add_systems(Update, theme::dispatch_clicks)
            .add_message::<bevy::picking::events::Pointer<bevy::picking::events::Click>>();
        plugin(&mut app);

        app.world_mut()
            .write_message(UiClick(UiAction::Rebind(Action::NextForm)));
        app.update();
        assert_eq!(
            app.world().resource::<RebindState>().action,
            Some(Action::NextForm)
        );

        // Arming frame: nothing pressed.
        app.update();

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::KeyG);
        app.update();

        let settings = app.world().resource::<Settings>();
        assert_eq!(
            settings.input.get(Action::NextForm)[0],
            Binding::Key(KeyCode::KeyG)
        );
        assert!(!app.world().resource::<RebindState>().is_capturing());
    }

    #[test]
    fn escape_cancels_a_rebind() {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .add_message::<UiClick>()
            .init_resource::<ClickGuard>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(Settings::default())
            .insert_resource(RebindState {
                action: Some(Action::Fire),
                armed: true,
            })
            .add_systems(Update, capture_rebind);

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();

        assert_eq!(
            app.world().resource::<Settings>().input,
            InputBindings::default()
        );
        assert!(!app.world().resource::<RebindState>().is_capturing());
    }
}
