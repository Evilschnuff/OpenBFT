//! How every screen looks and how its buttons are clicked.
//!
//! Functional art: flat panels, the cursor's three
//! colours as the palette, and the game's own Eurostile read from the install.
//! The original menu art is a later phase.
//!
//! # Clicks
//!
//! A button carries a [`UiButton`] naming what it does. Clicks arrive as
//! Bevy's `Pointer<Click>` messages — on release, which is what a menu wants
//! and what lets "press a key" rebinding tell the click that started it from
//! the press that answers it — and [`dispatch_clicks`] turns each into a
//! [`UiClick`] carrying the action. Every screen's handler reads `UiClick`, so
//! a test drives a screen by writing one rather than simulating a pointer.

use bevy::picking::events::{Click, Pointer};
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bft_assets::DataPaths;
use bft_assets::catalog::LevelId;
use bft_assets::messages::Messages;

use super::MenuScreen;
use super::bindings::Action;
use super::settings_screen::{SettingControl, SettingsTab};

/// The page behind every menu.
pub const BACKGROUND: Color = Color::srgb(0.035, 0.04, 0.05);
/// A panel on top of it.
pub const PANEL: Color = Color::srgba(0.07, 0.08, 0.1, 0.92);
/// Body text.
pub const TEXT: Color = Color::srgb(0.88, 0.9, 0.93);
/// Secondary text: hints, values that cannot be changed.
pub const TEXT_DIM: Color = Color::srgb(0.55, 0.58, 0.63);
/// The cursor's orange: titles, the selected item, warnings.
pub const ACCENT: Color = Color::srgb(1.0, 0.39, 0.1);
/// The cursor's cyan: highlights that are not warnings.
pub const CYAN: Color = Color::srgb(0.1, 0.96, 0.86);
/// The hover cursor's green: "good", "on target", "complete".
pub const GREEN: Color = Color::srgb(0.42, 1.0, 0.1);
/// Something went wrong.
pub const ERROR: Color = Color::srgb(1.0, 0.45, 0.4);

const BUTTON_IDLE: Color = Color::srgb(0.12, 0.13, 0.16);
const BUTTON_HOVER: Color = Color::srgb(0.2, 0.22, 0.27);
const BUTTON_PRESSED: Color = Color::srgb(0.3, 0.33, 0.4);
const BUTTON_SELECTED: Color = Color::srgb(0.32, 0.15, 0.06);
const BUTTON_DISABLED: Color = Color::srgb(0.08, 0.08, 0.09);

/// Font sizes, in logical pixels.
pub const TITLE_SIZE: f32 = 40.0;
pub const HEADING_SIZE: f32 = 24.0;
pub const BODY_SIZE: f32 = 17.0;
pub const SMALL_SIZE: f32 = 14.0;

/// The fonts, as read from the install.
///
/// `regular` is also installed as Bevy's *default* font, so every `Text` in
/// the crate — boot, loading, the viewer's panel — draws in Eurostile without
/// naming it. `bold` has to be asked for. Both are `Handle::default()`, which
/// is Bevy's built-in font, when the install has no `fonts/` directory.
#[derive(Resource, Debug, Clone, Default)]
pub struct UiFonts {
    pub bold: Handle<Font>,
    /// Whether Eurostile was found. The built-in fallback has no em dash or
    /// ellipsis, so text that uses them checks this.
    pub eurostile: bool,
}

/// Read `fonts/eurosti.ttf` and `eurostib.ttf` from the install
/// (docs/data/install.md) and make the first the default font.
///
/// Called from `UiPlugin::build`, which runs after `DefaultPlugins` has put
/// the built-in font at the default handle and **before** Bevy's text system
/// first registers fonts with its layout engine. Replacing the default later
/// would not take: the font collection registers each asset id once and does
/// not notice the asset under an id changing.
pub fn install_fonts(app: &mut App) {
    let Some(root) = app
        .world()
        .get_resource::<DataPaths>()
        .map(|paths| paths.root.path.clone())
    else {
        app.init_resource::<UiFonts>();
        return;
    };

    let regular = read_font(&root, "eurosti.ttf");
    let bold = read_font(&root, "eurostib.ttf");

    let Some(mut fonts) = app.world_mut().get_resource_mut::<Assets<Font>>() else {
        // No text plugin (a headless test app): nothing to draw with anyway.
        app.init_resource::<UiFonts>();
        return;
    };

    let eurostile = regular.is_some();
    if let Some(bytes) = regular
        && fonts
            .insert(AssetId::default(), Font::from_bytes(bytes))
            .is_err()
    {
        warn!("cannot replace the default font; menus use Bevy's built-in one");
    }
    let bold = bold
        .map(|bytes| fonts.add(Font::from_bytes(bytes)))
        .unwrap_or_default();

    if eurostile {
        info!("menus use the install's Eurostile");
    } else {
        warn!(
            "no fonts/eurosti.ttf in {}; menus use Bevy's built-in font",
            root.display()
        );
    }

    app.insert_resource(UiFonts { bold, eurostile });
}

/// One font file from the install's `fonts/`, matched without regard to
/// case: the installer's own casing is not something to depend on.
fn read_font(root: &std::path::Path, name: &str) -> Option<Vec<u8>> {
    let directory = std::fs::read_dir(root.join("fonts")).ok()?;
    let path = directory.flatten().map(|entry| entry.path()).find(|path| {
        path.file_name()
            .and_then(|file| file.to_str())
            .is_some_and(|file| file.eq_ignore_ascii_case(name))
    })?;
    std::fs::read(path).ok()
}

/// A `MSG_*` key resolved through the string table, `%n` newlines and all,
/// or the key itself when there is no table or no such string.
pub fn message_text(messages: Option<&Messages>, key: &str) -> String {
    messages
        .and_then(|table| table.format(key, &[]))
        .unwrap_or_else(|| key.to_string())
}

/// What a button does.
///
/// One enum for every screen so one dispatcher serves them all; the variants
/// are grouped by the screen that owns them.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct UiButton(pub UiAction);

/// Everything a button can ask for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UiAction {
    // Main menu.
    Open(MenuScreen),
    Quit,
    // Campaign.
    SelectMission(LevelId),
    StartMission,
    CycleDifficulty,
    // Multiplayer.
    SelectMap(LevelId),
    Host,
    Join,
    Practice,
    // Either list: open the selected level in the viewer (FR-T1).
    ViewSelected,
    // Settings, from the front end or the pause menu.
    SettingsTab(SettingsTab),
    Setting(SettingControl),
    Rebind(Action),
    ResetBindings,
    // Pause.
    Resume,
    PauseSettings,
    QuitToMenu,
    // Mission end.
    Retry,
    NextMission,
    // Save and load: the lists index `SaveListing`.
    OpenSaves,
    OpenLoads,
    SaveNew,
    SaveOver(u16),
    ConfirmOverwrite,
    LoadSave(u16),
    // The original menus.
    /// A page of the original menus, in the front end or the pause menu.
    Page(super::dialog::screens::Page),
    /// The new game panel's Start: the campaign from its first mission.
    NewCampaign,
    SetDifficulty(bft_sim::vehicle::Difficulty),
    /// The save and load dialogs: move the selection up (−) or down (+),
    /// pick a row, act on the selection, delete it.
    BrowseSaves(i8),
    PickSave(u16),
    CommitSave,
    DeleteSave,
    /// A message box's two buttons.
    BoxOk,
    BoxCancel,
    /// The commands dialog: the next set of commands, and scrolling.
    NextCommandSet,
    ScrollCommands(i8),
    PickCommand(u16),
    /// The cheat dialog's Go.
    SubmitCheat,
    /// The briefing, once the level has loaded: play.
    Proceed,
    // Back out of whatever this is.
    Back,
}

/// A button was clicked. Read by each screen's handler.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct UiClick(pub UiAction);

/// A button that is the current choice in a list: the selected mission, the
/// open settings tab.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct Selected;

/// A button drawn from the game's art, which
/// [`button_colours`] leaves alone.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ArtButton;

/// A button that cannot be pressed: a locked mission.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct Disabled;

/// Swallow clicks until every mouse button is up.
///
/// Set by the rebinding screen when the answer to "press a key" was a mouse
/// button: its release would otherwise arrive as a click on whatever is under
/// the cursor, which is usually the row that was just rebound.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct ClickGuard(pub bool);

/// Turn picking's clicks on buttons into [`UiClick`]s.
pub fn dispatch_clicks(
    mut clicks: MessageReader<Pointer<Click>>,
    buttons: Query<(&UiButton, Has<Disabled>)>,
    parents: Query<&ChildOf>,
    mouse: Option<Res<ButtonInput<MouseButton>>>,
    mut guard: ResMut<ClickGuard>,
    mut out: MessageWriter<UiClick>,
) {
    if guard.0 {
        clicks.clear();
        if mouse.is_none_or(|mouse| mouse.get_pressed().next().is_none()) {
            guard.0 = false;
        }
        return;
    }

    for click in clicks.read() {
        if click.event.button != PointerButton::Primary {
            continue;
        }
        // The label is a child of the button; a click that lands on it
        // belongs to the button.
        let target = std::iter::once(click.entity)
            .chain(parents.get(click.entity).map(ChildOf::parent))
            .find(|entity| buttons.contains(*entity));
        let Some((button, disabled)) = target.and_then(|entity| buttons.get(entity).ok()) else {
            continue;
        };
        if !disabled {
            out.write(UiClick(button.0));
        }
    }
}

/// Colour each button for its state: idle, hovered, pressed, selected,
/// disabled.
///
/// Every frame rather than on `Changed<Interaction>`, because `Selected` and
/// `Disabled` come and go without the interaction changing. There are a few
/// dozen buttons at most.
#[allow(clippy::type_complexity)]
pub fn button_colours(
    mut buttons: Query<
        (
            &Interaction,
            &mut BackgroundColor,
            &mut BorderColor,
            Has<Selected>,
            Has<Disabled>,
        ),
        (With<UiButton>, Without<ArtButton>),
    >,
) {
    for (interaction, mut background, mut border, selected, disabled) in &mut buttons {
        let (fill, edge) = if disabled {
            (BUTTON_DISABLED, BUTTON_DISABLED)
        } else {
            match (interaction, selected) {
                (Interaction::Pressed, _) => (BUTTON_PRESSED, ACCENT),
                (Interaction::Hovered, true) => (BUTTON_SELECTED, ACCENT),
                (Interaction::Hovered, false) => (BUTTON_HOVER, CYAN),
                (Interaction::None, true) => (BUTTON_SELECTED, ACCENT),
                (Interaction::None, false) => (BUTTON_IDLE, BUTTON_IDLE),
            }
        };
        background.set_if_neq(BackgroundColor(fill));
        border.set_if_neq(BorderColor::all(edge));
    }
}

/// A button: a label, an action, and a width in logical pixels.
pub fn button(label: impl Into<String>, action: UiAction, width: f32) -> impl Bundle {
    (
        Button,
        UiButton(action),
        Node {
            width: px(width),
            min_height: px(34),
            padding: UiRect::axes(px(14), px(6)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            border: UiRect::all(px(1)),
            ..default()
        },
        BackgroundColor(BUTTON_IDLE),
        BorderColor::all(BUTTON_IDLE),
        children![label_text(label, BODY_SIZE, TEXT)],
    )
}

/// A button's label, or any line of text: it ignores the pointer so a click
/// on it reaches the button behind.
pub fn label_text(text: impl Into<String>, size: f32, colour: Color) -> impl Bundle {
    (
        Text::new(text),
        TextFont::from_font_size(size),
        TextColor(colour),
        Pickable::IGNORE,
    )
}

/// A title in the bold face.
pub fn title_text(text: impl Into<String>, size: f32, fonts: &UiFonts) -> impl Bundle {
    (
        Text::new(text),
        TextFont {
            font: fonts.bold.clone().into(),
            ..TextFont::from_font_size(size)
        },
        TextColor(ACCENT),
        Pickable::IGNORE,
    )
}

/// The full-window root every menu screen spawns under.
pub fn screen_root() -> impl Bundle {
    (
        Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            padding: UiRect::all(px(32)),
            row_gap: px(16),
            ..default()
        },
        BackgroundColor(BACKGROUND),
    )
}

/// A panel: a darker box with a thin accent edge on the left.
pub fn panel() -> impl Bundle {
    (
        Node {
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(px(16)),
            row_gap: px(8),
            border: UiRect::left(px(3)),
            ..default()
        },
        BackgroundColor(PANEL),
        BorderColor::all(ACCENT),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_message_shows_its_key_and_newlines_expand() {
        let mut table = Messages::default();
        table.entries.insert("MSG_TWO".into(), "one%ntwo".into());

        assert_eq!(message_text(Some(&table), "MSG_TWO"), "one\ntwo");
        assert_eq!(message_text(Some(&table), "MSG_NONE"), "MSG_NONE");
        assert_eq!(message_text(None, "MSG_TWO"), "MSG_TWO");
    }

    /// A click on a button's label counts as a click on the button, and a
    /// disabled button does nothing.
    #[test]
    fn clicks_reach_the_button_through_its_label() {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .add_message::<Pointer<Click>>()
            .add_message::<UiClick>()
            .init_resource::<ClickGuard>()
            .add_systems(Update, dispatch_clicks);

        let live = app
            .world_mut()
            .spawn(UiButton(UiAction::Quit))
            .with_child(Text::new("Quit"))
            .id();
        let label = app.world().entity(live).get::<Children>().unwrap()[0];
        let dead = app
            .world_mut()
            .spawn((UiButton(UiAction::StartMission), Disabled))
            .id();

        for entity in [label, dead] {
            app.world_mut().write_message(click(entity));
        }
        app.update();

        let clicks: Vec<UiClick> = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<UiClick>>()
            .drain()
            .collect();
        assert_eq!(clicks, vec![UiClick(UiAction::Quit)]);
    }

    fn click(entity: Entity) -> Pointer<Click> {
        use bevy::camera::NormalizedRenderTarget;
        use bevy::picking::pointer::{Location, PointerId};
        use bevy::window::WindowRef;

        Pointer::new(
            PointerId::Mouse,
            Location {
                target: NormalizedRenderTarget::Window(
                    WindowRef::Primary
                        .normalize(Some(Entity::PLACEHOLDER))
                        .unwrap(),
                ),
                position: Vec2::ZERO,
            },
            Click {
                button: PointerButton::Primary,
                hit: bevy::picking::backend::HitData::new(Entity::PLACEHOLDER, 0.0, None, None),
                duration: default(),
                count: 1,
            },
            entity,
        )
    }
}
