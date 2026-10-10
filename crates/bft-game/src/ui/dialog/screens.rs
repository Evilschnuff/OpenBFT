//! The original menus' pages, and what their controls do.
//!
//! Each [`Page`] is one dialog of `dialog.zrd`; this module maps its
//! controls, by name, onto the game's own actions and draws what the game
//! fills in itself (the save list, the commands table, the credits roll).
//! The front end shows a page per [`MenuScreen`]; the pause menu
//! ([`overlays`](crate::ui::overlays)) asks [`spawn_page`] for its views.
//!
//! | Dialog | Page | Notes |
//! |---|---|---|
//! | `MAINMENU0` | front end main menu | plus our own Missions and Multiplayer buttons |
//! | `MAINMENU1` | pause menu | |
//! | `NEWGAMEPANEL` | new game | name, intensity (easy/medium/hard), start at mission 1 |
//! | `LOAD_GAME_DIALOG`, `SAVE_GAME_DIALOG` | load, save | the in-mission saves; the name bar names a save |
//! | `OPTIONSPANEL` | options | volumes, music and sound, resolution, strips; the rest drawn and inert; our own button to the simple settings panel |
//! | `CONTROLS_DIALOG` | controls | the modes are drawn as this game plays and are inert |
//! | `COMMANDS_DIALOG` | commands | the bindings, in the original's command sets |
//! | `CREDITSPANEL` | credits | the original's roll, then ours |
//! | `CONFIRM_QUIT` | quit? | |
//! | `MESSAGEBOX` | overwrite? delete? | over the save dialogs |
//! | `CHEAT_CODE_DIALOG` | cheats (Ctrl+X in play) | `Medic` and `Hemmit` |
//!
//! Not mapped: `MAINMENU2` (the in-game menu without New, Save and Load —
//! multiplayer's, by its buttons), `MAINMENU3` (Load, New and Quit alone
//! over the new game art; unknown, likely what follows a lost mission),
//! `NETEXIT`/`MPEXIT` and `MP_NEW_GAME` (there is no multiplayer to host
//! yet), and `WOL_NEW_GAME` (Westwood Online is gone).

use std::path::PathBuf;

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::text::EditableText;
use bft_assets::DataPaths;
use bft_assets::dialog::{Control, ControlKind, Dialog, ListStyle, Scroller};
use bft_assets::messages::Messages;
use bft_sim::damage::Health;
use bft_sim::mission::{Campaign, Progress};
use bft_sim::states::{AppState, GameMode};
use bft_sim::vehicle::{Difficulty, PlayerVehicle};
use bft_sim::weapons::{WeaponLoadout, WeaponTable};

use super::frame::DIALOG_CANVAS;
use super::render::{Backdrop, DialogField, DialogPainter, DialogRoot, OnCanvas, Wiring};
use super::{MenuStyle, original_menus};
use crate::boot::UnlockAll;
use crate::globals::{self, Globals};
use crate::loading::LoadFailure;
use crate::savegame::{LoadRequest, SaveListing, SaveRequest};
use crate::ui::bindings::{Action, Binding, InputBindings};
use crate::ui::settings::Settings;
use crate::ui::settings_screen::{RebindState, SettingKey};
use crate::ui::theme::{
    BODY_SIZE, ERROR, UiAction, UiClick, UiFonts, button, label_text, message_text,
};
use crate::ui::{MenuScreen, PauseMenuOpen, RequestStartMission};

/// One page of the original menus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Page {
    /// `MAINMENU0` in the front end, `MAINMENU1` in play.
    Main,
    NewGame,
    Options,
    /// The simple settings panel, for what `OPTIONSPANEL` has no control
    /// for.
    MoreSettings,
    Controls,
    Commands,
    Credits,
    ConfirmQuit,
    Saves,
    Loads,
    Cheat,
}

/// Where a page is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    FrontEnd,
    InGame,
}

impl Page {
    /// The `dialog.zrd` dialog the page draws, or `None` for a page that is
    /// one of the simple menus.
    pub fn dialog(self, place: Place) -> Option<&'static str> {
        Some(match self {
            Page::Main => match place {
                Place::FrontEnd => "MAINMENU0",
                Place::InGame => "MAINMENU1",
            },
            Page::NewGame => "NEWGAMEPANEL",
            Page::Options => "OPTIONSPANEL",
            Page::MoreSettings => return None,
            Page::Controls => "CONTROLS_DIALOG",
            Page::Commands => "COMMANDS_DIALOG",
            Page::Credits => "CREDITSPANEL",
            Page::ConfirmQuit => "CONFIRM_QUIT",
            Page::Saves => "SAVE_GAME_DIALOG",
            Page::Loads => "LOAD_GAME_DIALOG",
            Page::Cheat => "CHEAT_CODE_DIALOG",
        })
    }

    /// The front-end screen that shows this page.
    pub fn screen(self) -> Option<MenuScreen> {
        Some(match self {
            Page::Main => MenuScreen::Main,
            Page::NewGame => MenuScreen::NewGame,
            Page::Options => MenuScreen::Settings,
            Page::MoreSettings => MenuScreen::MoreSettings,
            Page::Controls => MenuScreen::Controls,
            Page::Commands => MenuScreen::Commands,
            Page::Credits => MenuScreen::Credits,
            Page::ConfirmQuit => MenuScreen::ConfirmQuit,
            Page::Loads => MenuScreen::Load,
            Page::Saves | Page::Cheat => return None,
        })
    }

    /// The page a front-end screen shows, when it is an original one.
    pub fn of_screen(screen: MenuScreen) -> Option<Page> {
        Some(match screen {
            MenuScreen::Main => Page::Main,
            MenuScreen::NewGame => Page::NewGame,
            MenuScreen::Settings => Page::Options,
            MenuScreen::Controls => Page::Controls,
            MenuScreen::Commands => Page::Commands,
            MenuScreen::Credits => Page::Credits,
            MenuScreen::ConfirmQuit => Page::ConfirmQuit,
            MenuScreen::Load => Page::Loads,
            MenuScreen::Campaign | MenuScreen::Multiplayer | MenuScreen::MoreSettings => {
                return None;
            }
        })
    }
}

/// Whether the original page can be drawn: the menus are on and its dialog
/// is complete.
pub fn can_draw(style: &MenuStyle, settings: Option<&Settings>, page: Page, place: Place) -> bool {
    let (Some(menus), Some(name)) = (original_menus(Some(style), settings), page.dialog(place))
    else {
        return false;
    };
    menus.dialog(name).is_some()
}

/// Run condition for a front-end screen the simple menus draw: when its original
/// page cannot be. Not while the style is still pending.
pub fn fallback_page(
    page: Page,
) -> impl FnMut(Option<Res<MenuStyle>>, Option<Res<Settings>>) -> bool + Clone {
    move |style, settings| match style.as_deref() {
        None => true,
        Some(MenuStyle::Pending) => false,
        Some(style) => !can_draw(style, settings.as_deref(), page, Place::FrontEnd),
    }
}

/// The save and load dialogs' selection.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct SaveBrowser {
    /// The selected row. In the save dialog row 0 is a new save.
    pub selected: usize,
    /// Whether the dialog up is the save one.
    pub saving: bool,
}

/// The commands dialog's place: which set, how far scrolled, which row.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct CommandsView {
    pub set: usize,
    pub scroll: usize,
    pub selected: usize,
}

/// The question a message box is asking, if one is up.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
pub struct MessageBox(pub Option<Question>);

/// What a message box asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Question {
    /// "Overwrite save game?"
    Overwrite { path: PathBuf, label: String },
    /// "Delete this game?"
    Delete { path: PathBuf },
}

/// The front-end screen to open on arriving there, instead of the main
/// menu: the pause menu's New Game.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrontEndStart(pub MenuScreen);

/// A page the front end drew.
#[derive(Component, Debug, Clone, Copy)]
struct FrontEndPage;

/// The message box's root.
#[derive(Component, Debug, Clone, Copy)]
struct MessageBoxRoot;

/// The credits roll: its column scrolls up through the window.
#[derive(Component, Debug, Clone, Copy)]
struct CreditRoll {
    /// Seconds per pixel.
    rate: f32,
    /// The window's height and the column's, canvas pixels.
    window: f32,
    column: f32,
    since: f32,
}

/// Our own buttons, drawn next to an original page.
#[derive(Component, Debug, Clone, Copy)]
struct Extras;

/// The command sets, in the original's order (`MSG_CMDSET_*`), holding
/// this game's actions. Every action is in exactly one.
pub const COMMAND_SETS: [(&str, &[Action]); 5] = [
    (
        "MSG_CMDSET_BFT_CONTROL",
        &[
            Action::Forward,
            Action::Back,
            Action::Left,
            Action::Right,
            Action::NextForm,
            Action::Climb,
            Action::Dive,
        ],
    ),
    (
        "MSG_CMDSET_WEAPONS",
        &[
            Action::Fire,
            Action::AltFire,
            Action::ToggleStrong,
            Action::Detonate,
            Action::Slot(1),
            Action::Slot(2),
            Action::Slot(3),
            Action::Slot(4),
            Action::Slot(5),
            Action::Slot(6),
            Action::Slot(7),
            Action::Slot(8),
            Action::Slot(9),
        ],
    ),
    (
        "MSG_CMDSET_DISPLAY_CONTROLS",
        &[Action::Screenshot, Action::DebugPanel, Action::FreeCamera],
    ),
    (
        "MSG_CMDSET_CAMPAIGN_CONTROLS",
        &[Action::ToggleMap, Action::ToggleObjectives],
    ),
    (
        "MSG_CMDSET_OTHER_CONTROLS",
        &[Action::Pause, Action::QuickSave, Action::QuickLoad],
    ),
];

/// The original's name and description keys for an action, where it had
/// the same command.
pub fn command_keys(action: Action) -> Option<(String, String)> {
    let stem = match action {
        Action::Forward => "GO_FORWARD".to_string(),
        Action::Back => "GO_BACKWARD".to_string(),
        Action::Left => "TURN_LEFT".to_string(),
        Action::Right => "TURN_RIGHT".to_string(),
        Action::Climb => "GO_UP".to_string(),
        Action::Dive => "GO_DOWN".to_string(),
        Action::Slot(slot) => format!("WEAPON{slot}"),
        Action::ToggleObjectives => "OBJECTIVE".to_string(),
        Action::ToggleMap => "MAPDISPLAY".to_string(),
        Action::QuickSave => "SAVE_GAME".to_string(),
        Action::QuickLoad => "LOAD_GAME".to_string(),
        _ => return None,
    };
    Some((format!("MSG_CMD_{stem}"), format!("MSG_CMD_{stem}_VERBOSE")))
}

/// What the commands table calls an action: the original's word when it
/// had the command and the table has it, ours otherwise.
fn command_name(action: Action, messages: Option<&Messages>) -> String {
    command_keys(action)
        .and_then(|(name, _)| messages?.get(&name).map(str::to_string))
        .unwrap_or_else(|| action.label())
}

/// An action's binding in one of the table's four columns: first key,
/// second key, mouse, pad.
pub fn binding_in_column(
    bindings: &InputBindings,
    action: Action,
    column: usize,
) -> Option<Binding> {
    let all = bindings.get(action);
    let keys = all.iter().filter(|b| matches!(b, Binding::Key(_)));
    match column {
        0 => keys.clone().next().copied(),
        1 => keys.clone().nth(1).copied(),
        2 => all.iter().find(|b| matches!(b, Binding::Mouse(_))).copied(),
        _ => all
            .iter()
            .find(|b| matches!(b, Binding::Gamepad(_)))
            .copied(),
    }
}

/// A binding as its column shows it: the column's heading already says
/// mouse or joystick, so the label drops the device.
pub fn column_label(binding: Binding) -> String {
    let label = binding.label();
    label
        .strip_prefix("Pad ")
        .or_else(|| label.strip_prefix("Mouse "))
        .map(str::to_string)
        .unwrap_or(label)
}

/// Everything a page is drawn from.
#[derive(SystemParam)]
pub struct PageContext<'w> {
    pub style: Res<'w, MenuStyle>,
    pub settings: Res<'w, Settings>,
    pub fonts: Res<'w, UiFonts>,
    pub globals: Option<Res<'w, Globals>>,
    pub tables: Res<'w, Assets<Messages>>,
    pub listing: Res<'w, SaveListing>,
    pub browser: Res<'w, SaveBrowser>,
    pub commands_view: Res<'w, CommandsView>,
    pub rebind: Res<'w, RebindState>,
    pub difficulty: Option<Res<'w, Difficulty>>,
    pub failure: Option<Res<'w, LoadFailure>>,
    pub thumbnails: Res<'w, crate::savegame::SaveThumbnails>,
    pub splash: Option<Res<'w, super::Splash>>,
    pub backdrop: Option<Res<'w, super::GameBackdrop>>,
}

impl PageContext<'_> {
    fn messages(&self) -> Option<&Messages> {
        globals::messages(self.globals.as_deref(), &self.tables)
    }

    /// Whether anything a page on show was drawn from has changed, so it
    /// needs drawing again. Volumes and switches are not in it: the
    /// options page follows them live, and redrawing under a dragged
    /// slider would drop the drag.
    pub fn changed(&self, page: Page) -> bool {
        self.style.is_changed()
            || match page {
                Page::Saves | Page::Loads => {
                    self.listing.is_changed()
                        || self.browser.is_changed()
                        || self.thumbnails.is_changed()
                }
                Page::Commands => {
                    self.commands_view.is_changed()
                        || self.rebind.is_changed()
                        || self.settings.is_changed()
                }
                Page::NewGame => self.difficulty.as_ref().is_some_and(|d| d.is_changed()),
                _ => false,
            }
    }
}

/// Draw a page, or `None` when its dialog is not there to draw (the caller
/// shows the simple menus' instead). The root carries no lifetime; the caller
/// gives it one.
pub fn spawn_page(
    commands: &mut Commands,
    ctx: &PageContext,
    page: Page,
    place: Place,
    save_refused: bool,
) -> Option<Entity> {
    let menus = original_menus(Some(&ctx.style), Some(&ctx.settings))?;
    let dialog = menus.dialog(page.dialog(place)?)?;
    let messages = ctx.messages();
    let backdrop = match place {
        Place::FrontEnd => ctx
            .splash
            .as_ref()
            .map_or(Backdrop::Black, |splash| Backdrop::Splash(splash.0.clone())),
        Place::InGame => ctx
            .backdrop
            .as_ref()
            .map_or(Backdrop::Dimmed, |view| Backdrop::Blurred(view.0.clone())),
    };
    let (root, painter) =
        DialogPainter::open(commands, menus, dialog, messages, &ctx.fonts, backdrop, 10);

    match page {
        Page::Main => {
            painter.controls(commands, |control| {
                main_wiring(control, place, save_refused)
            });
            if place == Place::FrontEnd {
                spawn_main_extras(commands, root, ctx.failure.as_deref());
            }
        }
        Page::NewGame => {
            let difficulty = ctx.difficulty.as_deref().copied().unwrap_or_default();
            let name = ctx.settings.player_name.clone();
            painter.controls(commands, |control| {
                new_game_wiring(control, difficulty, &name)
            });
        }
        Page::Options => {
            painter.controls(commands, options_wiring);
            spawn_extra_button(commands, root, "More settings", Page::MoreSettings);
        }
        Page::Controls => {
            painter.controls(commands, controls_wiring);
        }
        Page::Commands => spawn_commands(commands, &painter, ctx),
        Page::Credits => spawn_credits(commands, &painter, dialog),
        Page::ConfirmQuit => {
            painter.controls(commands, |control| {
                let quit = match place {
                    Place::FrontEnd => UiAction::Quit,
                    Place::InGame => UiAction::QuitToMenu,
                };
                match control.name.as_str() {
                    "OK_TO_QUIT" => Wiring::action(quit),
                    "CANCEL_QUIT" => Wiring::action(UiAction::Back),
                    _ => Wiring::inert(),
                }
            });
        }
        Page::Saves | Page::Loads => spawn_saves(commands, &painter, ctx, page == Page::Saves),
        Page::Cheat => {
            painter.controls(commands, |control| match control.name.as_str() {
                "CHEATCODE" => Wiring {
                    field: Some((DialogField::CheatCode, 24)),
                    ..default()
                },
                "GO" => Wiring::action(UiAction::SubmitCheat),
                _ => Wiring::inert(),
            });
        }
        Page::MoreSettings => {}
    }
    Some(root)
}

fn main_wiring(control: &Control, place: Place, save_refused: bool) -> Wiring {
    let open = |page: Page| Wiring::action(UiAction::Page(page));
    match (place, control.name.as_str()) {
        (_, "NEWGAME") => open(Page::NewGame),
        (_, "LOADGAME") => open(Page::Loads),
        (_, "OPTIONS") => open(Page::Options),
        (_, "CONTROLS") => open(Page::Controls),
        (_, "CREDITS") => open(Page::Credits),
        (_, "QUIT") => open(Page::ConfirmQuit),
        (Place::InGame, "BACK") => Wiring::action(UiAction::Resume),
        (Place::InGame, "SAVEGAME") => open(Page::Saves).disabled_if(save_refused),
        _ => Wiring::inert(),
    }
}

/// `INTENSITY`'s three buttons are labelled easy, medium and hard on the
/// art itself.
const INTENSITIES: [Difficulty; 3] = [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard];

fn new_game_wiring(control: &Control, difficulty: Difficulty, name: &str) -> Wiring {
    match control.name.as_str() {
        "BACK" => Wiring::action(UiAction::Back),
        "START" => Wiring::action(UiAction::NewCampaign),
        "NAME" => Wiring {
            field: Some((DialogField::PlayerName, 24)),
            text: Some(name.to_string()),
            ..default()
        },
        "INTENSITY" => Wiring {
            items: INTENSITIES
                .iter()
                .map(|level| Wiring {
                    action: Some(UiAction::SetDifficulty(*level)),
                    selected: *level == difficulty,
                    ..default()
                })
                .collect(),
            ..default()
        },
        _ => Wiring::inert(),
    }
}

/// The options this game has, on the controls the original gave them; the
/// rest (lighting, perspective, detail, texture memory, effects, sound
/// quality) are drawn as the game is and do nothing.
fn options_wiring(control: &Control) -> Wiring {
    let slider = |key: SettingKey| Wiring {
        setting: Some(key),
        ..default()
    };
    match control.name.as_str() {
        "BACK" => Wiring::action(UiAction::Back),
        "MUSIC_ENABLE" => Wiring::setting(SettingKey::MusicOn),
        "SOUND_ACTIVE" => Wiring::setting(SettingKey::Mute),
        "FULLHUD" => Wiring::setting(SettingKey::HudStrips),
        "RESOLUTION_CYCLE" => Wiring::setting(SettingKey::Resolution),
        "MUSIC_VOLUME" => slider(SettingKey::Music),
        "SOUND_VOLUME" => slider(SettingKey::Effects),
        // Always on here: lighting, perspective-correct textures.
        "LIGHTING" | "PERSPECTIVE" => Wiring {
            checked: true,
            ..default()
        },
        // The first state is the best: HIGH, MAX.
        _ => Wiring::inert(),
    }
}

/// The controls page: its modes are what this game does, and fixed.
fn controls_wiring(control: &Control) -> Wiring {
    let chosen = |count: usize, which: Option<usize>| Wiring {
        items: (0..count)
            .map(|index| Wiring {
                selected: Some(index) == which,
                ..default()
            })
            .collect(),
        ..default()
    };
    let items = match &control.kind {
        ControlKind::Radio(items) => items.len(),
        _ => 0,
    };
    match control.name.as_str() {
        "RESUME" => Wiring::action(UiAction::Back),
        "COMMANDS_BTN" => Wiring::action(UiAction::Page(Page::Commands)),
        // The mouse aims; the camera is behind the tank.
        "MOUSE_OR_JOYSTICK" => chosen(items, Some(0)),
        "CAMERA_MODE" => chosen(items, Some(1)),
        _ if items > 0 => chosen(items, None),
        _ => Wiring::inert(),
    }
}

/// Our own buttons on the original main menu, bottom right: the simple
/// menus' mission list and multiplayer screen, which the original's front end had
/// no door to.
fn spawn_main_extras(commands: &mut Commands, root: Entity, failure: Option<&LoadFailure>) {
    let extras = commands
        .spawn((
            Extras,
            Node {
                position_type: PositionType::Absolute,
                right: px(24),
                bottom: px(24),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexEnd,
                row_gap: px(8),
                ..default()
            },
            ChildOf(root),
        ))
        .id();
    commands.spawn((
        button("Missions", UiAction::Open(MenuScreen::Campaign), 200.0),
        ChildOf(extras),
    ));
    commands.spawn((
        button(
            "Multiplayer",
            UiAction::Open(MenuScreen::Multiplayer),
            200.0,
        ),
        ChildOf(extras),
    ));
    if let Some(failure) = failure.and_then(|failure| failure.0.clone()) {
        commands.spawn((
            label_text(failure, BODY_SIZE, ERROR),
            Node {
                max_width: px(480),
                ..default()
            },
            ChildOf(extras),
        ));
    }
}

/// One of our own buttons, bottom right of an original page.
fn spawn_extra_button(commands: &mut Commands, root: Entity, label: &str, page: Page) {
    commands
        .spawn((
            Extras,
            button(label, UiAction::Page(page), 200.0),
            ChildOf(root),
        ))
        .insert(Node {
            position_type: PositionType::Absolute,
            right: px(24),
            bottom: px(24),
            width: px(200),
            min_height: px(34),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            border: UiRect::all(px(1)),
            ..default()
        });
}

// ---------------------------------------------------------------------------
// Saves
// ---------------------------------------------------------------------------

/// The rows of the save or load dialog: in the save dialog a new save
/// first, then every save, newest first.
fn save_rows(listing: &SaveListing, saving: bool) -> usize {
    listing.entries.len() + usize::from(saving)
}

/// The listing entry a row is, `None` for the save dialog's new save.
fn save_entry(row: usize, saving: bool) -> Option<usize> {
    if saving {
        row.checked_sub(1)
    } else {
        Some(row)
    }
}

/// The save and load dialogs: the selection in the name bar, the saves
/// before it in the rows above and after it in the rows below — the three
/// and six `LIST_n` rows either side of the bar.
fn spawn_saves(commands: &mut Commands, painter: &DialogPainter, ctx: &PageContext, saving: bool) {
    let listing = &ctx.listing;
    let messages = ctx.messages();
    let rows = save_rows(listing, saving);
    let selected = ctx.browser.selected.min(rows.saturating_sub(1));
    let selected_entry = save_entry(selected, saving).and_then(|i| listing.entries.get(i));

    let bar_text = match selected_entry {
        Some(entry) if saving => entry
            .header
            .as_ref()
            .map(|header| header.label.clone())
            .unwrap_or_default(),
        Some(entry) => bar_label(entry, messages),
        None => String::new(),
    };
    let loadable = selected_entry.is_some_and(|entry| entry.is_loadable());

    painter.controls(commands, |control| match control.name.as_str() {
        "BACK" => Wiring::action(UiAction::Back),
        "SAVE" | "LOAD" => Wiring::action(UiAction::CommitSave).disabled_if(!saving && !loadable),
        "GAMENAME" if saving => Wiring {
            field: Some((DialogField::SaveName, 40)),
            text: Some(bar_text.clone()),
            ..default()
        },
        "GAMENAME" => Wiring {
            text: Some(bar_text.clone()),
            ..default()
        },
        "PREV_GAME_BTN" => Wiring::action(UiAction::BrowseSaves(-1)),
        "NEXT_GAME_BTN" => Wiring::action(UiAction::BrowseSaves(1)),
        "DELETE_BTN" => Wiring::action(UiAction::DeleteSave).disabled_if(selected_entry.is_none()),
        _ => Wiring::inert(),
    });

    // The rows: `LIST_n` in order, the bar sitting between the rows whose
    // positions straddle it.
    let mut list: Vec<_> = painter
        .dialog
        .primitives
        .iter()
        .filter(|p| p.name.starts_with("LIST_"))
        .collect();
    list.sort_by_key(|p| {
        p.name
            .trim_start_matches("LIST_")
            .parse::<u32>()
            .unwrap_or(0)
    });
    let bar_y = painter
        .dialog
        .control("GAMENAME")
        .map_or(0, |control| control.pos.y);
    let above = list.iter().filter(|p| p.pos.y < bar_y).count();

    for (index, primitive) in list.iter().enumerate() {
        let offset = index as isize - above as isize + if index >= above { 1 } else { 0 };
        let row = selected as isize + offset;
        if row < 0 || row as usize >= rows {
            continue;
        }
        let row = row as usize;
        let text = match save_entry(row, saving) {
            Some(entry) => listing
                .entries
                .get(entry)
                .map(|entry| short_label(entry, messages))
                .unwrap_or_default(),
            None => "New save".to_string(),
        };
        // The rows are narrow: each is cut off at its frame's edge.
        let pos = primitive.pos.as_vec2();
        let size = Vec2::new(
            room_right_of(painter, pos),
            painter
                .font(primitive.font.unwrap_or(0))
                .map_or(16.0, |font| font.size as f32 * 1.3),
        );
        let cell = commands
            .spawn((
                OnCanvas::sized(pos, size),
                Node {
                    position_type: PositionType::Absolute,
                    overflow: Overflow::clip(),
                    ..default()
                },
                Button,
                crate::ui::theme::UiButton(UiAction::PickSave(row as u16)),
                crate::ui::theme::ArtButton,
                ChildOf(painter.canvas),
            ))
            .id();
        let entity = painter.text(commands, Vec2::ZERO, primitive.font.unwrap_or(0), text);
        commands.entity(entity).insert(ChildOf(cell));
    }

    // The selected save's picture, beside the list below the bar.
    if let Some(picture) = selected_entry.and_then(|entry| ctx.thumbnails.get(&entry.path)) {
        let below = list
            .iter()
            .find(|p| p.pos.y > bar_y)
            .map(|p| p.pos.as_vec2());
        if let Some(frame) = below.and_then(|pos| frame_around(painter, pos)) {
            // Ours: a 4:3 picture a little right of the frame, as tall as
            // it, kept on the canvas.
            let at = Vec2::new(frame.max.x + 12.0, frame.min.y + 4.0);
            let height = (frame.height() - 8.0)
                .min((DIALOG_CANVAS.x - 16.0 - at.x) * 0.75)
                .max(0.0);
            commands.spawn((
                OnCanvas::sized(at, Vec2::new(height / 0.75, height)),
                Node {
                    position_type: PositionType::Absolute,
                    ..default()
                },
                ImageNode {
                    image: picture.clone(),
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                Pickable::IGNORE,
                ChildOf(painter.canvas),
            ));
        }
    }
}

/// A save as the load dialog's bar shows it: its name and when, in
/// characters the game's font has.
fn bar_label(entry: &bft_sim::save::SaveEntry, messages: Option<&Messages>) -> String {
    let name = short_label(entry, messages);
    match &entry.header {
        Some(header) => {
            let when = crate::savegame::civil_time(header.created);
            format!("{name}  {}", when.trim_end_matches(" UTC"))
        }
        None => name,
    }
}

/// A save as a list row shows it: its name, or its mission's.
fn short_label(entry: &bft_sim::save::SaveEntry, messages: Option<&Messages>) -> String {
    match &entry.header {
        Some(header) if !header.label.trim().is_empty() => header.label.clone(),
        Some(header) => message_text(messages, &header.title_key),
        None => entry
            .path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default(),
    }
}

/// The smallest background image around a canvas point: the frame a row
/// or a field sits in.
fn frame_around(painter: &DialogPainter, pos: Vec2) -> Option<Rect> {
    painter
        .dialog
        .backgrounds
        .iter()
        .filter_map(|placed| {
            let size = painter.menus.art(&placed.bitmap)?.size;
            let rect = Rect::from_corners(placed.pos.as_vec2(), placed.pos.as_vec2() + size);
            rect.contains(pos).then_some(rect)
        })
        // Not the full-height strip down the side.
        .filter(|rect| rect.min.y >= 0.0)
        .min_by(|a, b| (a.width() * a.height()).total_cmp(&(b.width() * b.height())))
}

/// How far text at `pos` can run before its frame's edge.
fn room_right_of(painter: &DialogPainter, pos: Vec2) -> f32 {
    // A few pixels short of the frame's drawn border.
    const MARGIN: f32 = 8.0;
    frame_around(painter, pos).map_or(DIALOG_CANVAS.x - pos.x, |frame| {
        (frame.max.x - pos.x - MARGIN).max(8.0)
    })
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

/// The key columns, left to right.
const KEY_COLUMNS: [&str; 4] = [
    "CMD_KEYA_BTN",
    "CMD_KEYB_BTN",
    "CMD_MOUSE_BTN",
    "CMD_JOY_BTN",
];

fn spawn_commands(commands: &mut Commands, painter: &DialogPainter, ctx: &PageContext) {
    let messages = ctx.messages();
    let view = &ctx.commands_view;
    let (set_key, actions) = COMMAND_SETS[view.set % COMMAND_SETS.len()];
    let selected = view.selected.min(actions.len().saturating_sub(1));

    painter.controls(commands, |control| match control.name.as_str() {
        "CMD_RESUME_BTN" => Wiring::action(UiAction::Back),
        "CMD_SET_BTN" => Wiring {
            action: Some(UiAction::NextCommandSet),
            text: Some(message_text(messages, set_key)),
            ..default()
        },
        "CMD_NEXT_SET_BTN" => Wiring::action(UiAction::NextCommandSet),
        "CMD_PREV_CMD_BTN" => Wiring::action(UiAction::ScrollCommands(-1)),
        "CMD_NEXT_CMD_BTN" => Wiring::action(UiAction::ScrollCommands(1)),
        "CMD_RESET_BTN" => Wiring::action(UiAction::ResetBindings),
        // The table is drawn below.
        "CMD_COMMAND_BTN" => Wiring::hidden(),
        name if KEY_COLUMNS.contains(&name) => Wiring::hidden(),
        _ => Wiring::inert(),
    });

    let list = |name: &str| {
        painter
            .dialog
            .control(name)
            .and_then(|control| match &control.kind {
                ControlKind::List(style) => Some((control, style)),
                _ => None,
            })
    };
    let rows_shown = |style: &ListStyle| style.size[0].max(1) as usize;

    if let Some((control, style)) = list("CMD_COMMAND_BTN") {
        for (row, action) in actions
            .iter()
            .enumerate()
            .skip(view.scroll)
            .take(rows_shown(style))
        {
            let font = if row == selected {
                style.selected_font
            } else {
                style.list_font
            };
            let pos = control.pos.as_vec2()
                + Vec2::new(0.0, (row - view.scroll) as f32 * style.spacing as f32);
            let entity = painter.text(commands, pos, font, command_name(*action, messages));
            commands.entity(entity).insert((
                Button,
                crate::ui::theme::UiButton(UiAction::PickCommand(row as u16)),
                crate::ui::theme::ArtButton,
                Pickable::default(),
            ));
        }
    }

    for (column, name) in KEY_COLUMNS.iter().enumerate() {
        let Some((control, style)) = list(name) else {
            continue;
        };
        let row_pos = |row: usize| {
            control.pos.as_vec2()
                + Vec2::new(0.0, (row - view.scroll) as f32 * style.spacing as f32)
        };
        // The highlight behind the selected row.
        if (view.scroll..view.scroll + rows_shown(style)).contains(&selected)
            && let Some(bitmap) = &control.bitmap
        {
            let image = if ctx.rebind.action == actions.get(selected).copied() {
                control
                    .activate
                    .as_ref()
                    .and_then(|visual| visual.bitmap.as_deref())
                    .unwrap_or(&bitmap.name)
            } else {
                &bitmap.name
            };
            painter.image(commands, image, row_pos(selected) + bitmap.offset.as_vec2());
        }
        for (row, action) in actions
            .iter()
            .enumerate()
            .skip(view.scroll)
            .take(rows_shown(style))
        {
            let text = binding_in_column(&ctx.settings.input, *action, column)
                .map(column_label)
                .unwrap_or_default();
            let font = if row == selected {
                style.selected_font
            } else {
                style.list_font
            };
            let entity = painter.text(commands, row_pos(row), font, text);
            // A click on any cell of a row asks for that command's new
            // button, as the simple settings panel does.
            commands.entity(entity).insert((
                OnCanvas::sized(
                    row_pos(row),
                    Vec2::new(
                        control
                            .size(|n| painter.menus.art(n).map(|a| a.size.as_uvec2()))
                            .map_or(80.0, |size| size.x as f32),
                        style.spacing as f32,
                    ),
                ),
                Button,
                crate::ui::theme::UiButton(UiAction::Rebind(*action)),
                crate::ui::theme::ArtButton,
                Pickable::default(),
            ));
        }
    }

    let description = actions
        .get(selected)
        .and_then(|action| command_keys(*action))
        .and_then(|(_, verbose)| messages?.get(&verbose).map(str::to_string))
        .unwrap_or_default();
    if let Some(primitive) = painter.dialog.primitive("CMD_DESCRIPTION") {
        let width = primitive.wordwrap.map_or(300.0, |size| size.x as f32);
        painter.wrapped(
            commands,
            primitive.pos.as_vec2(),
            width,
            primitive.font.unwrap_or(0),
            description,
        );
    }
    if ctx.rebind.is_capturing()
        && let Some(primitive) = painter.dialog.primitive("PRESS_A_KEY")
    {
        painter.text(
            commands,
            primitive.pos.as_vec2(),
            primitive.font.unwrap_or(0),
            "Press a button...",
        );
    }
}

// ---------------------------------------------------------------------------
// Credits
// ---------------------------------------------------------------------------

/// The credits: the original's roll, then ours — this project, the data it
/// reads, mech3ax and the libraries — in the roll's own styles.
fn spawn_credits(commands: &mut Commands, painter: &DialogPainter, dialog: &Dialog) {
    painter.controls(commands, |control| match control.name.as_str() {
        "BACK" => Wiring::action(UiAction::Back),
        // At the same place as Back: the roll's end-of-game exit.
        "QUIT" => Wiring::hidden(),
        _ => Wiring::inert(),
    });

    let Some((control, roll)) = dialog
        .controls
        .iter()
        .find_map(|control| match &control.kind {
            ControlKind::Scroller(roll) => Some((control, roll)),
            _ => None,
        })
    else {
        return;
    };
    spawn_roll(commands, painter, control.pos.as_vec2(), roll);
}

/// Our credits as roll lines: headings in the roll's heading style (the
/// first line's font), text in its name style, wrapped to the window.
fn our_credit_lines(roll: &Scroller, char_width: f32) -> Vec<(String, i32, i32, u32)> {
    let heading_font = roll.lines.first().map_or(0, |line| line.font);
    let name = roll
        .lines
        .iter()
        .map(|line| line.font)
        .find(|font| *font != heading_font)
        .unwrap_or(heading_font);
    let indent = roll.lines.iter().map(|line| line.x).min().unwrap_or(0);
    let width = (roll.rect.width() - indent) as f32;
    let per_line = ((width / char_width.max(1.0)) as usize).max(12);

    let mut lines = Vec::new();
    for (index, (heading, body)) in crate::ui::menus::CREDITS.iter().enumerate() {
        lines.push((
            heading.to_string(),
            indent,
            if index == 0 { 40 } else { 16 },
            heading_font,
        ));
        for line in wrap_words(body, per_line) {
            lines.push((line, indent, 0, name));
        }
    }
    lines
}

/// Break text into lines of at most `width` characters, at spaces.
pub fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > width {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

fn spawn_roll(commands: &mut Commands, painter: &DialogPainter, at: Vec2, roll: &Scroller) {
    let window_rect = Rect::new(
        roll.rect.min.x as f32,
        roll.rect.min.y as f32,
        roll.rect.max.x as f32,
        roll.rect.max.y as f32,
    );
    let window = commands
        .spawn((
            OnCanvas::sized(at + window_rect.min, window_rect.size()),
            Node {
                position_type: PositionType::Absolute,
                overflow: Overflow::clip(),
                ..default()
            },
            // Over the roll's own picture, which is the control's bitmap.
            ZIndex(1),
            Pickable::IGNORE,
            ChildOf(painter.canvas),
        ))
        .id();

    let size_of = |font: u32| painter.font(font).map_or(14.0, |font| font.size as f32);
    let mut lines: Vec<(String, i32, i32, u32)> = roll
        .lines
        .iter()
        .map(|line| (line.text.clone(), line.x, line.gap, line.font))
        .collect();
    let body_size = size_of(lines.get(1).map_or(0, |line| line.3));
    lines.extend(our_credit_lines(roll, body_size * 0.55));

    let column = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                ..default()
            },
            Pickable::IGNORE,
            ChildOf(window),
        ))
        .id();
    let mut y = 0.0;
    for (text, x, gap, font) in lines {
        y += gap as f32;
        let line = painter.text(commands, Vec2::new(x as f32, y), font, text);
        commands.entity(line).insert(ChildOf(column));
        y += size_of(font) * 1.2;
    }
    commands.entity(column).insert((
        OnCanvas::sized(
            Vec2::new(0.0, window_rect.height()),
            Vec2::new(window_rect.width(), y),
        ),
        CreditRoll {
            rate: roll.rate.max(0.001),
            window: window_rect.height(),
            column: y,
            since: f32::NAN,
        },
    ));
}

/// Scroll the credits: one canvas pixel per `SCROLL_RATE` seconds, from
/// below the window until the last line has left it, and round again.
fn roll_credits(time: Res<Time<Real>>, mut rolls: Query<(&mut CreditRoll, &mut OnCanvas)>) {
    let now = time.elapsed_secs();
    for (mut roll, mut placed) in &mut rolls {
        if roll.since.is_nan() {
            roll.since = now;
        }
        let travel = roll.window + roll.column;
        let moved = ((now - roll.since) / roll.rate) % travel.max(1.0);
        placed.pos.y = roll.window - moved;
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// The pages' systems.
pub fn plugin(app: &mut App) {
    app.init_resource::<SaveBrowser>()
        .init_resource::<CommandsView>()
        .init_resource::<MessageBox>()
        .init_resource::<crate::savegame::SaveThumbnails>()
        .add_systems(OnEnter(AppState::FrontEnd), open_front_end_start)
        .add_systems(
            OnEnter(MenuScreen::Load),
            |mut listing: ResMut<SaveListing>, mut browser: ResMut<SaveBrowser>| {
                // However the screen was reached: the saves as they are now.
                listing.refresh();
                *browser = SaveBrowser::default();
            },
        )
        .add_systems(OnExit(AppState::FrontEnd), close_message_box)
        .add_systems(OnExit(AppState::InGame), close_message_box)
        .add_systems(
            OnEnter(MenuScreen::MoreSettings),
            |mut commands: Commands, fonts: Res<UiFonts>, paths: Option<Res<DataPaths>>| {
                crate::ui::menus::spawn_settings_screen(
                    &mut commands,
                    &fonts,
                    paths.as_deref(),
                    MenuScreen::MoreSettings,
                );
            },
        )
        .add_systems(
            Update,
            (
                handle_page_clicks,
                restyle_front_end.run_if(in_state(AppState::FrontEnd)),
                show_front_end.run_if(in_state(AppState::FrontEnd)),
                show_message_box,
                roll_credits,
                open_cheats.run_if(in_state(AppState::InGame)),
            )
                .chain()
                .after(crate::ui::theme::dispatch_clicks),
        );
}

/// Go to the screen the pause menu asked for.
fn open_front_end_start(
    mut commands: Commands,
    start: Option<Res<FrontEndStart>>,
    mut screen: ResMut<NextState<MenuScreen>>,
) {
    if let Some(start) = start {
        screen.set(start.0);
        commands.remove_resource::<FrontEndStart>();
    }
}

fn close_message_box(mut question: ResMut<MessageBox>) {
    question.0 = None;
}

/// When the menus switch between the original and the simple ones — the
/// style settles, or the player flips the setting — show the current
/// screen again in the new style: the simple screens are spawned on
/// entering one.
fn restyle_front_end(
    style: Res<MenuStyle>,
    settings: Res<Settings>,
    screen: Res<State<MenuScreen>>,
    mut next: ResMut<NextState<MenuScreen>>,
    mut last: Local<Option<Option<bool>>>,
) {
    let now = match &*style {
        MenuStyle::Pending => None,
        style => Some(original_menus(Some(style), Some(&settings)).is_some()),
    };
    // A screen change already asked for (the pause menu's New Game) goes
    // first; this looks again once it has landed.
    if matches!(*next, NextState::Pending(_) | NextState::PendingIfNeq(_)) {
        return;
    }
    let before = last.replace(now);
    match (before, now) {
        // Nothing to switch to yet.
        (_, None) => return,
        (Some(before), now) if before == now => return,
        // Settled before the front end was entered: its screens were
        // drawn in the right style on the way in.
        (None, Some(_)) if !style.is_changed() => return,
        _ => {}
    }
    let current = *screen.get();
    // Pages only the original has lead back to where the simple menus
    // have them.
    let target = match (now, current) {
        (Some(false), MenuScreen::MoreSettings) => MenuScreen::Settings,
        (
            Some(false),
            MenuScreen::NewGame
            | MenuScreen::Controls
            | MenuScreen::Commands
            | MenuScreen::ConfirmQuit,
        ) => MenuScreen::Main,
        _ => current,
    };
    next.set(target);
}

/// Draw the current front-end screen's original page, and draw it again
/// when what it shows changes.
fn show_front_end(
    mut commands: Commands,
    ctx: PageContext,
    screen: Res<State<MenuScreen>>,
    pages: Query<Entity, With<FrontEndPage>>,
    mut drawn_original: Local<Option<bool>>,
) {
    let page = Page::of_screen(*screen.get());
    let original = original_menus(Some(&ctx.style), Some(&ctx.settings)).is_some();
    let dirty = screen.is_changed()
        || *drawn_original != Some(original)
        || page.is_some_and(|page| ctx.changed(page));
    *drawn_original = Some(original);
    if !dirty {
        return;
    }
    for entity in &pages {
        commands.entity(entity).despawn();
    }
    let Some(page) = page else {
        return;
    };
    if let Some(root) = spawn_page(&mut commands, &ctx, page, Place::FrontEnd, false) {
        commands
            .entity(root)
            .insert((FrontEndPage, DespawnOnExit(AppState::FrontEnd)));
    }
}

/// The message box over the save dialogs.
fn show_message_box(
    mut commands: Commands,
    question: Res<MessageBox>,
    ctx: PageContext,
    state: Res<State<AppState>>,
    boxes: Query<Entity, With<MessageBoxRoot>>,
) {
    if !question.is_changed() && !ctx.style.is_changed() {
        return;
    }
    for entity in &boxes {
        commands.entity(entity).despawn();
    }
    let Some(question) = &question.0 else {
        return;
    };
    let Some(menus) = original_menus(Some(&ctx.style), Some(&ctx.settings)) else {
        return;
    };
    let Some(dialog) = menus.dialog("MESSAGEBOX") else {
        return;
    };
    let messages = ctx.messages();
    let (root, painter) = DialogPainter::open(
        &mut commands,
        menus,
        dialog,
        messages,
        &ctx.fonts,
        Backdrop::Clear,
        20,
    );
    // The box is modal: the backdrop takes the clicks the dialog below
    // would have had.
    commands
        .entity(root)
        .insert((MessageBoxRoot, Pickable::default()));
    match state.get() {
        AppState::FrontEnd => commands
            .entity(root)
            .insert(DespawnOnExit(AppState::FrontEnd)),
        _ => commands
            .entity(root)
            .insert(DespawnOnExit(AppState::InGame)),
    };
    painter.controls(&mut commands, |control| match control.name.as_str() {
        "MB_OK" => Wiring::action(UiAction::BoxOk),
        "MB_CANCEL" => Wiring::action(UiAction::BoxCancel),
        _ => Wiring::inert(),
    });
    let key = match question {
        Question::Overwrite { .. } => "MSG_SAVE_OVERWRITE_MSG",
        Question::Delete { .. } => "MSG_DELETE_MSG",
    };
    if let Some(primitive) = dialog.primitive("MESSAGE") {
        let width = primitive.wordwrap.map_or(200.0, |size| size.x as f32);
        painter.wrapped(
            &mut commands,
            primitive.pos.as_vec2(),
            width,
            primitive.font.unwrap_or(0),
            message_text(messages, key),
        );
    }
}

/// What the pages' own actions do; the front end's and the pause menu's
/// handlers move between pages.
// The pages' one junction: every action that is not a page change.
#[allow(clippy::too_many_arguments)]
fn handle_page_clicks(
    mut clicks: MessageReader<UiClick>,
    style: Res<MenuStyle>,
    mut browser: ResMut<SaveBrowser>,
    mut view: ResMut<CommandsView>,
    mut question: ResMut<MessageBox>,
    mut listing: ResMut<SaveListing>,
    mut settings: ResMut<Settings>,
    fields: Query<(&DialogField, &EditableText)>,
    mut writers: PageWriters,
    mut campaign_state: CampaignState,
    mut pause: Option<ResMut<PauseMenuOpen>>,
    mut cheats: Cheats,
) {
    let field = |which: DialogField| {
        fields
            .iter()
            .find(|(field, _)| **field == which)
            .map(|(_, text)| text.value().to_string().trim().to_string())
    };

    for click in clicks.read() {
        match click.0 {
            UiAction::Page(Page::Saves) => {
                listing.refresh();
                *browser = SaveBrowser {
                    selected: 0,
                    saving: true,
                };
            }
            UiAction::Page(Page::Loads) | UiAction::Open(MenuScreen::Load) => {
                listing.refresh();
                *browser = SaveBrowser {
                    selected: 0,
                    saving: false,
                };
            }
            UiAction::BrowseSaves(step) => {
                let rows = save_rows(&listing, browser.saving);
                browser.selected = browser
                    .selected
                    .saturating_add_signed(isize::from(step))
                    .min(rows.saturating_sub(1));
            }
            UiAction::PickSave(row) => browser.selected = usize::from(row),
            UiAction::CommitSave => {
                let entry = save_entry(browser.selected, browser.saving)
                    .and_then(|index| listing.entries.get(index));
                if browser.saving {
                    let label = field(DialogField::SaveName).unwrap_or_default();
                    match entry {
                        Some(entry) => {
                            question.0 = Some(Question::Overwrite {
                                path: entry.path.clone(),
                                label,
                            });
                        }
                        None => {
                            writers
                                .saves
                                .write(SaveRequest::Labelled { label, over: None });
                            if let Some(pause) = pause.as_mut() {
                                pause.0 = false;
                            }
                        }
                    }
                } else if let Some(entry) = entry.filter(|entry| entry.is_loadable()) {
                    writers.loads.write(LoadRequest::Path(entry.path.clone()));
                }
            }
            UiAction::DeleteSave => {
                if let Some(entry) = save_entry(browser.selected, browser.saving)
                    .and_then(|index| listing.entries.get(index))
                {
                    question.0 = Some(Question::Delete {
                        path: entry.path.clone(),
                    });
                }
            }
            UiAction::BoxOk => match question.0.take() {
                Some(Question::Overwrite { path, label }) => {
                    writers.saves.write(SaveRequest::Labelled {
                        label,
                        over: Some(path),
                    });
                    if let Some(pause) = pause.as_mut() {
                        pause.0 = false;
                    }
                }
                Some(Question::Delete { path }) => {
                    if let Err(e) = std::fs::remove_file(&path) {
                        warn!("cannot delete {}: {e}", path.display());
                    }
                    // Its picture goes with it; there may be none.
                    let _ = std::fs::remove_file(crate::savegame::thumbnail_path(&path));
                    listing.refresh();
                    let rows = save_rows(&listing, browser.saving);
                    browser.selected = browser.selected.min(rows.saturating_sub(1));
                }
                None => {}
            },
            UiAction::BoxCancel => question.0 = None,
            UiAction::NextCommandSet => {
                *view = CommandsView {
                    set: (view.set + 1) % COMMAND_SETS.len(),
                    ..default()
                };
            }
            UiAction::ScrollCommands(step) => {
                let (_, actions) = COMMAND_SETS[view.set % COMMAND_SETS.len()];
                view.selected = view
                    .selected
                    .saturating_add_signed(isize::from(step))
                    .min(actions.len().saturating_sub(1));
                keep_in_view(&mut view, command_rows(&style));
            }
            UiAction::PickCommand(row) => {
                view.selected = usize::from(row);
                keep_in_view(&mut view, command_rows(&style));
            }
            UiAction::Rebind(action) => {
                let (_, actions) = COMMAND_SETS[view.set % COMMAND_SETS.len()];
                if let Some(row) = actions.iter().position(|a| *a == action) {
                    view.selected = row;
                    keep_in_view(&mut view, command_rows(&style));
                }
            }
            UiAction::SetDifficulty(level) => campaign_state.set_difficulty(level),
            UiAction::NewCampaign => {
                if let Some(name) = field(DialogField::PlayerName).filter(|n| !n.is_empty())
                    && settings.player_name != name
                {
                    settings.player_name = name;
                }
                if let Some(first) = campaign_state.first_mission() {
                    writers.start.write(RequestStartMission(first));
                }
            }
            UiAction::SubmitCheat => {
                let code = field(DialogField::CheatCode).unwrap_or_default();
                cheats.apply(&code);
                if let Some(pause) = pause.as_mut() {
                    pause.0 = false;
                }
            }
            _ => {}
        }
    }
}

/// Scroll the commands table so the selected row shows among the `rows`
/// the table has room for.
fn keep_in_view(view: &mut CommandsView, rows: usize) {
    let rows = rows.max(1);
    if view.selected < view.scroll {
        view.scroll = view.selected;
    } else if view.selected >= view.scroll + rows {
        view.scroll = view.selected + 1 - rows;
    }
}

/// How many rows the commands table shows: its `LISTSIZE`. Without the
/// dialog, every row (nothing scrolls).
fn command_rows(style: &MenuStyle) -> usize {
    let list = match style {
        MenuStyle::Original(menus) => menus
            .dialogs
            .get("COMMANDS_DIALOG")
            .and_then(|dialog| dialog.control("CMD_COMMAND_BTN")),
        _ => None,
    };
    match list.map(|control| &control.kind) {
        Some(ControlKind::List(style)) => style.size[0] as usize,
        _ => usize::MAX,
    }
}

/// The requests the pages make.
#[derive(SystemParam)]
struct PageWriters<'w> {
    saves: MessageWriter<'w, SaveRequest>,
    loads: MessageWriter<'w, LoadRequest>,
    start: MessageWriter<'w, RequestStartMission>,
}

/// The campaign's difficulty and first mission.
#[derive(SystemParam)]
struct CampaignState<'w> {
    campaign: Option<Res<'w, Campaign>>,
    difficulty: Option<ResMut<'w, Difficulty>>,
    progress: Option<ResMut<'w, Progress>>,
    unlock_all: Option<Res<'w, UnlockAll>>,
}

impl CampaignState<'_> {
    fn first_mission(&self) -> Option<bft_assets::catalog::LevelId> {
        self.campaign.as_ref()?.missions.first().map(|m| m.id)
    }

    /// Set and keep the difficulty, as the simple campaign screen does.
    fn set_difficulty(&mut self, level: Difficulty) {
        if let Some(difficulty) = self.difficulty.as_mut() {
            difficulty.set_if_neq(level);
        }
        if let Some(progress) = self.progress.as_mut()
            && progress.difficulty != level
        {
            progress.difficulty = level;
            // `--unlock-all` must never write the save (FR-M7).
            if !self.unlock_all.as_ref().is_some_and(|flag| flag.0)
                && let Err(e) = progress.save()
            {
                warn!("cannot save the difficulty: {e}");
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Cheats
// ---------------------------------------------------------------------------

/// The original's cheat words, as its string table spells them. Two do
/// something here: `MSG_CHEAT_HEALTH` heals the tank to full,
/// `MSG_CHEAT_AMMO` fills every carried weapon. The invincibility, timer
/// and frame-rate words are recognised and refused: nothing in the sim
/// makes the tank invulnerable yet.
pub const CHEATS: [&str; 6] = [
    "MSG_CHEAT_AMMO",
    "MSG_CHEAT_HEALTH",
    "MSG_CHEAT_INVINCIBLE",
    "MSG_CHEAT_TIMER",
    "MSG_CHEAT_FRAMERATE",
    "MSG_CHEAT_GOD",
];

/// Which cheat a typed word is: its key, matched without regard to case.
pub fn cheat_for(word: &str, messages: Option<&Messages>) -> Option<&'static str> {
    let word = word.trim();
    if word.is_empty() {
        return None;
    }
    CHEATS.into_iter().find(|key| {
        messages
            .and_then(|m| m.get(key))
            .is_some_and(|w| w.eq_ignore_ascii_case(word))
    })
}

/// What the cheat dialog changes.
#[derive(SystemParam)]
struct Cheats<'w, 's> {
    players: Query<'w, 's, (&'static mut Health, &'static mut WeaponLoadout), With<PlayerVehicle>>,
    weapons: Option<Res<'w, WeaponTable>>,
    globals: Option<Res<'w, Globals>>,
    tables: Res<'w, Assets<Messages>>,
    log: Option<ResMut<'w, crate::ui::hud::HudLog>>,
    time: Res<'w, Time<Real>>,
}

impl Cheats<'_, '_> {
    fn apply(&mut self, word: &str) {
        let messages = globals::messages(self.globals.as_deref(), &self.tables);
        let cheat = cheat_for(word, messages);
        let now = self.time.elapsed_secs();
        match cheat {
            Some("MSG_CHEAT_HEALTH") => {
                for (mut health, _) in &mut self.players {
                    health.current = health.max;
                }
                self.say(word.to_string(), now);
            }
            Some("MSG_CHEAT_AMMO") => {
                for (_, mut loadout) in &mut self.players {
                    for slot in loadout.slots.iter_mut() {
                        for state in [slot.weak.as_mut(), slot.strong.as_mut()]
                            .into_iter()
                            .flatten()
                        {
                            if let Some(limit) = self
                                .weapons
                                .as_ref()
                                .and_then(|table| table.weapons.get(state.weapon))
                                .map(|weapon| weapon.ammo_limit)
                            {
                                state.ammo = state.ammo.max(limit);
                            }
                        }
                    }
                }
                self.say(word.to_string(), now);
            }
            Some(_) => self.say(format!("{word}: not in this version"), now),
            None => {}
        }
    }

    fn say(&mut self, line: String, now: f32) {
        if let Some(log) = self.log.as_mut() {
            log.push(line, now);
        }
    }
}

/// Ctrl+X in play: the original's cheat entry (manual, "Other
/// Controls"). Only with the original menus, whose dialog it is; the pause
/// menu's handler opens it.
fn open_cheats(
    keys: Res<ButtonInput<KeyCode>>,
    style: Res<MenuStyle>,
    settings: Res<Settings>,
    mode: Option<Res<State<GameMode>>>,
    gate: crate::ui::GameplayGate,
    mut clicks: MessageWriter<UiClick>,
) {
    let ctrl = keys.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    if !(ctrl && keys.just_pressed(KeyCode::KeyX)) || !gate.enabled() {
        return;
    }
    if mode.is_some_and(|mode| *mode.get() != GameMode::Campaign) {
        return;
    }
    if !can_draw(&style, Some(&settings), Page::Cheat, Place::InGame) {
        return;
    }
    clicks.write(UiClick(UiAction::Page(Page::Cheat)));
}

/// Whether the front end or pause menu on screen is an original page, for
/// tests.
pub fn original_page_shown(world: &mut World, dialog: &str) -> bool {
    world
        .query::<&DialogRoot>()
        .iter(world)
        .any(|root| root.name == dialog)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_is_in_exactly_one_command_set() {
        for action in Action::all() {
            let homes = COMMAND_SETS
                .iter()
                .filter(|(_, actions)| actions.contains(&action))
                .count();
            assert_eq!(homes, 1, "{action:?}");
        }
    }

    #[test]
    fn the_table_columns_split_keys_mouse_and_pad() {
        let mut bindings = InputBindings::default();
        bindings.map.insert(
            Action::Fire,
            vec![
                Binding::Key(KeyCode::KeyF),
                Binding::Mouse(MouseButton::Left),
                Binding::Key(KeyCode::KeyG),
            ],
        );
        assert_eq!(
            binding_in_column(&bindings, Action::Fire, 0),
            Some(Binding::Key(KeyCode::KeyF))
        );
        assert_eq!(
            binding_in_column(&bindings, Action::Fire, 1),
            Some(Binding::Key(KeyCode::KeyG))
        );
        assert_eq!(
            binding_in_column(&bindings, Action::Fire, 2),
            Some(Binding::Mouse(MouseButton::Left))
        );
        assert_eq!(binding_in_column(&bindings, Action::Fire, 3), None);
    }

    #[test]
    fn words_wrap_at_spaces() {
        assert_eq!(
            wrap_words("one two three four", 9),
            vec!["one two", "three", "four"]
        );
        assert_eq!(wrap_words("", 9), Vec::<String>::new());
    }

    #[test]
    fn cheats_are_the_string_table_words_in_any_case() {
        let mut messages = Messages::default();
        messages
            .entries
            .insert("MSG_CHEAT_HEALTH".into(), "Heal".into());
        assert_eq!(
            cheat_for(" hEaL ", Some(&messages)),
            Some("MSG_CHEAT_HEALTH")
        );
        assert_eq!(cheat_for("nope", Some(&messages)), None);
        assert_eq!(cheat_for("Heal", None), None);
    }

    #[test]
    fn every_front_end_page_maps_to_its_screen_and_back() {
        for screen in [
            MenuScreen::Main,
            MenuScreen::NewGame,
            MenuScreen::Settings,
            MenuScreen::Controls,
            MenuScreen::Commands,
            MenuScreen::Credits,
            MenuScreen::ConfirmQuit,
            MenuScreen::Load,
        ] {
            let page = Page::of_screen(screen).expect("an original page");
            assert_eq!(page.screen(), Some(screen));
            assert!(page.dialog(Place::FrontEnd).is_some());
        }
        assert_eq!(Page::of_screen(MenuScreen::Campaign), None);
        assert_eq!(Page::Main.dialog(Place::InGame), Some("MAINMENU1"));
    }

    // -----------------------------------------------------------------------
    // The flows, on a made-up dialog file in the real one's shape.
    // -----------------------------------------------------------------------

    use crate::ui::dialog::OriginalMenus;
    use crate::ui::dialog::render::DialogControl;
    use crate::ui::theme::{ClickGuard, UiButton};
    use bft_assets::catalog::{LevelCatalog, LevelId};
    use bft_assets::dialog::parse_dialogs;
    use bft_sim::mission::MissionDef;
    use serde_json::json;

    fn button(name: &str, x: i32, y: i32) -> serde_json::Value {
        json!([
            name,
            [
                "POSITION",
                [x, y],
                "BITMAP",
                ["b"],
                "ROLLOVER",
                ["BITMAP", ["b_h"], "SOUND", ["snd_over", 1.0]],
                "ACTIVATE",
                ["SOUND", ["snd_press", 1.0]]
            ]
        ])
    }

    /// A dialog with plain buttons, more controls (name/body pairs, flat)
    /// and more keys for its body.
    fn dialog(
        buttons: &[&str],
        more_controls: serde_json::Value,
        more_body: serde_json::Value,
    ) -> serde_json::Value {
        let mut controls = Vec::new();
        for (index, name) in buttons.iter().enumerate() {
            let pair = button(name, 100, 20 + 40 * index as i32);
            controls.push(pair[0].clone());
            controls.push(pair[1].clone());
        }
        if let serde_json::Value::Array(more) = more_controls {
            controls.extend(more);
        }
        let mut body = vec![
            json!("FONTS"),
            json!([[0, "Face", 16, [255, 255, 255], 1, 500]]),
            json!("BACKGROUND_IMAGES"),
            json!([["strip", 0, -144]]),
            json!("BUTTONS"),
            serde_json::Value::Array(controls),
        ];
        if let serde_json::Value::Array(more) = more_body {
            body.extend(more);
        }
        body.extend([json!("CURSOR"), json!(["BITMAP", ["cursor"]])]);
        serde_json::Value::Array(body)
    }

    fn menus() -> OriginalMenus {
        let rows: Vec<serde_json::Value> = (0..9)
            .flat_map(|row| {
                let y = if row < 3 {
                    90 + 20 * row
                } else {
                    200 + 20 * row
                };
                [
                    json!(format!("LIST_{row}")),
                    json!(["POSITION", [230, y], "FONT", 0]),
                ]
            })
            .collect();
        let field = |name: &str| {
            json!([
                name,
                [
                    "POSITION",
                    [180, 83],
                    "BITMAP",
                    ["b"],
                    "LABEL",
                    ["", 14, 4, 0]
                ]
            ])
        };
        let mut new_game = vec![
            json!("INTENSITY"),
            json!([
                "RADIO",
                [
                    ["POSITION", [1, 1], "BITMAP", ["b"]],
                    ["POSITION", [1, 2], "BITMAP", ["b"]],
                    ["POSITION", [1, 3], "BITMAP", ["b"]]
                ]
            ]),
        ];
        new_game.extend(field("NAME").as_array().cloned().unwrap_or_default());
        let file = json!([[
            "MAINMENU0",
            dialog(
                &[
                    "NEWGAME", "LOADGAME", "OPTIONS", "CONTROLS", "CREDITS", "QUIT"
                ],
                json!([]),
                json!([])
            ),
            "MAINMENU1",
            dialog(
                &["BACK", "NEWGAME", "SAVEGAME", "LOADGAME", "QUIT"],
                json!([]),
                json!([])
            ),
            "NEWGAMEPANEL",
            dialog(
                &["BACK", "START"],
                serde_json::Value::Array(new_game),
                json!([])
            ),
            "SAVE_GAME_DIALOG",
            dialog(
                &[
                    "BACK",
                    "SAVE",
                    "PREV_GAME_BTN",
                    "NEXT_GAME_BTN",
                    "DELETE_BTN"
                ],
                field("GAMENAME"),
                json!(["PRIMITIVES", rows])
            ),
            "MESSAGEBOX",
            dialog(
                &["MB_OK", "MB_CANCEL"],
                json!([]),
                json!([
                    "PRIMITIVES",
                    [
                        "MESSAGE",
                        ["POSITION", [393, 230], "FONT", 0, "WORDWRAP", [100, 40]]
                    ]
                ])
            )
        ]]);
        let set = parse_dialogs(&file).expect("the made-up file parses");
        OriginalMenus::assemble(set, Vec::new(), None, |_| {
            Some(crate::ui::hud::original::HudArt {
                image: Handle::default(),
                size: Vec2::new(40.0, 20.0),
            })
        })
    }

    fn app(style: MenuStyle) -> App {
        let mut app = App::new();
        app.add_plugins((bevy::MinimalPlugins, bevy::state::app::StatesPlugin));
        app.init_state::<AppState>()
            .add_sub_state::<MenuScreen>()
            .add_sub_state::<GameMode>()
            .insert_resource(style)
            .insert_resource(Settings::default())
            .insert_resource(InputBindings::default())
            .insert_resource(Difficulty::Normal)
            .insert_resource(LevelCatalog::default())
            .insert_resource(Campaign {
                missions: (1..=2)
                    .map(|id| MissionDef {
                        id: LevelId(id),
                        level: LevelId(id),
                        title_key: String::new(),
                        briefing_video: None,
                        briefing_layout: id as u8,
                    })
                    .collect(),
            })
            .init_resource::<UiFonts>()
            .init_resource::<Assets<Messages>>()
            .init_resource::<Assets<Image>>()
            .init_resource::<SaveListing>()
            .init_resource::<RebindState>()
            .init_resource::<PauseMenuOpen>()
            .init_resource::<ClickGuard>()
            .init_resource::<ButtonInput<KeyCode>>()
            .init_resource::<ButtonInput<MouseButton>>()
            .add_message::<UiClick>()
            .add_message::<bevy::picking::events::Pointer<bevy::picking::events::Click>>()
            .add_message::<RequestStartMission>()
            .add_message::<crate::ui::RequestPractice>()
            .add_message::<crate::ui::RequestViewer>()
            .add_message::<crate::ui::RequestHost>()
            .add_message::<crate::ui::RequestJoin>()
            .add_message::<SaveRequest>()
            .add_message::<LoadRequest>()
            .add_systems(Update, crate::ui::theme::dispatch_clicks);
        crate::ui::menus::plugin(&mut app);
        super::super::render::plugin(&mut app);
        plugin(&mut app);
        app
    }

    fn shown(app: &mut App) -> Vec<String> {
        let mut names: Vec<String> = app
            .world_mut()
            .query::<&DialogRoot>()
            .iter(app.world())
            .map(|root| root.name.clone())
            .collect();
        names.sort();
        names
    }

    /// Click a control of the dialog on screen as a player would: through
    /// the action its button carries.
    fn click(app: &mut App, control: &str) {
        let action = app
            .world_mut()
            .query::<(&DialogControl, &UiButton)>()
            .iter(app.world())
            .find(|(c, _)| c.name == control)
            .map(|(_, button)| button.0)
            .unwrap_or_else(|| panic!("no wired {control} on screen"));
        app.world_mut().write_message(UiClick(action));
        app.update();
        app.update();
    }

    /// The first steps of a new game in the original's dialogs: the main
    /// menu, New Game, the intensity, Start — mission 1.
    #[test]
    fn the_front_end_runs_through_the_original_dialogs() {
        let mut app = app(MenuStyle::Original(Box::new(menus())));
        app.insert_state(AppState::FrontEnd);
        app.update();
        app.update();
        assert_eq!(shown(&mut app), vec!["MAINMENU0"]);

        click(&mut app, "NEWGAME");
        assert_eq!(
            *app.world().resource::<State<MenuScreen>>().get(),
            MenuScreen::NewGame
        );
        assert_eq!(shown(&mut app), vec!["NEWGAMEPANEL"]);

        click(&mut app, "INTENSITY[2]");
        assert_eq!(*app.world().resource::<Difficulty>(), Difficulty::Hard);

        click(&mut app, "START");
        let starts: Vec<RequestStartMission> = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<RequestStartMission>>()
            .drain()
            .collect();
        assert_eq!(starts, vec![RequestStartMission(LevelId(1))]);

        click(&mut app, "BACK");
        assert_eq!(shown(&mut app), vec!["MAINMENU0"]);
    }

    /// Without the original menus, the simple front end is what shows.
    #[test]
    fn the_simple_menus_show_without_the_original() {
        let mut app = app(MenuStyle::Fallback);
        app.insert_state(AppState::FrontEnd);
        app.update();
        app.update();

        assert!(shown(&mut app).is_empty());
        let has_campaign_button = app
            .world_mut()
            .query::<&UiButton>()
            .iter(app.world())
            .any(|button| button.0 == UiAction::Open(MenuScreen::Campaign));
        assert!(has_campaign_button);
    }

    /// Switching the menus off in the settings redraws the screen in
    /// the simple style.
    #[test]
    fn turning_the_original_menus_off_redraws_the_front_end() {
        let mut app = app(MenuStyle::Original(Box::new(menus())));
        app.insert_state(AppState::FrontEnd);
        app.update();
        app.update();
        assert_eq!(shown(&mut app), vec!["MAINMENU0"]);

        app.world_mut().resource_mut::<Settings>().menus.original = false;
        app.update();
        app.update();
        assert!(shown(&mut app).is_empty());
    }

    /// The save dialog's name bar names a new save; an existing one asks
    /// first, in the message box.
    #[test]
    fn a_named_save_is_requested_from_the_save_dialog() {
        let mut app = app(MenuStyle::Original(Box::new(menus())));
        app.insert_state(AppState::InGame);
        app.update();

        app.world_mut()
            .write_message(UiClick(UiAction::Page(Page::Saves)));
        app.update();
        assert!(app.world().resource::<SaveBrowser>().saving);

        // Draw the page as the pause menu would, and type a name.
        app.world_mut()
            .run_system_once(|mut commands: Commands, ctx: PageContext| {
                spawn_page(&mut commands, &ctx, Page::Saves, Place::InGame, false);
            })
            .expect("the page draws");
        let field = app
            .world_mut()
            .query::<(Entity, &DialogField)>()
            .iter(app.world())
            .find(|(_, field)| **field == DialogField::SaveName)
            .map(|(entity, _)| entity)
            .expect("the name bar is a field");
        app.world_mut()
            .entity_mut(field)
            .insert(EditableText::new("Before the dam"));

        app.world_mut().write_message(UiClick(UiAction::CommitSave));
        app.update();
        let saves: Vec<SaveRequest> = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<SaveRequest>>()
            .drain()
            .collect();
        assert_eq!(
            saves,
            vec![SaveRequest::Labelled {
                label: "Before the dam".into(),
                over: None
            }]
        );

        // Over an existing save: the box asks, OK saves over it.
        app.world_mut().resource_mut::<SaveListing>().entries = vec![bft_sim::save::SaveEntry {
            path: PathBuf::from("old.ron"),
            header: None,
            version: None,
            problem: None,
        }];
        app.world_mut().resource_mut::<SaveBrowser>().selected = 1;
        app.world_mut().write_message(UiClick(UiAction::CommitSave));
        app.update();
        assert!(matches!(
            app.world().resource::<MessageBox>().0,
            Some(Question::Overwrite { .. })
        ));
        app.update();
        assert!(shown(&mut app).contains(&"MESSAGEBOX".to_string()));

        app.world_mut().write_message(UiClick(UiAction::BoxOk));
        app.update();
        let saves: Vec<SaveRequest> = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<SaveRequest>>()
            .drain()
            .collect();
        assert_eq!(
            saves,
            vec![SaveRequest::Labelled {
                label: "Before the dam".into(),
                over: Some(PathBuf::from("old.ron"))
            }]
        );
        assert!(app.world().resource::<MessageBox>().0.is_none());
    }

    use bevy::ecs::system::RunSystemOnce;
}
