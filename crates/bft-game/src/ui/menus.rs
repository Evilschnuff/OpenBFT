//! The front end: main menu, campaign and briefing, multiplayer, settings,
//! credits (FR-U2, FR-M5).
//!
//! Every level plays or opens in the viewer, through these screens: campaign
//! missions from the [`Campaign`] with their locks from [`Progress`],
//! multiplayer maps from the rest of the catalog.
//!
//! # The briefing
//!
//! Selecting a mission loads its reader archive and builds the same
//! [`MissionScript`] the game will, so the briefing lists exactly the
//! objectives the mission starts with — the `INITIAL_STATE ACTIVE` ones —
//! as `MSG_M<n>_O<k>_SUMMARY` and `_OBJ` text (docs/data/messages.md). The hidden
//! objectives stay hidden, as the file intends
//! (`ObjectiveState::Inactive`).

use std::net::SocketAddr;

use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use bevy::text::{EditableText, EditableTextFilter, TextCursorStyle};
use bft_assets::DataPaths;
use bft_assets::catalog::{LevelCatalog, LevelEntry, LevelId, LevelKind, MultiplayerMode};
use bft_assets::messages::Messages;
use bft_assets::reader::ReaderData;
use bft_sim::mission::{Campaign, MissionScript, Progress, build_mission_script};
use bft_sim::vehicle::Difficulty;

use super::bindings::{Action, ActionInput};
use super::dialog::screens::{Page, fallback_page};
use super::settings::Settings;
use super::settings_screen::{RebindState, spawn_settings_panel};
use super::theme::{
    self, ACCENT, BODY_SIZE, CYAN, Disabled, ERROR, GREEN, HEADING_SIZE, SMALL_SIZE, Selected,
    TEXT, TEXT_DIM, TITLE_SIZE, UiAction, UiClick, UiFonts, button, label_text, message_text,
    screen_root, title_text,
};
use super::{
    MenuScreen, RequestHost, RequestJoin, RequestPractice, RequestStartMission, RequestViewer,
};
use crate::boot::UnlockAll;
use crate::globals::{self, Globals};
use crate::loading::LoadFailure;

/// The port a hosted game listens on unless the player types another.
///
/// There is no multiplayer to choose one yet; this is a free, unprivileged
/// port that no common service claims.
pub const DEFAULT_PORT: u16 = 27_815;

/// The front end's systems.
pub fn plugin(app: &mut App) {
    app.init_resource::<CampaignSelection>()
        .init_resource::<MultiplayerSelection>()
        .add_systems(
            OnEnter(MenuScreen::Main),
            spawn_main.run_if(fallback_page(Page::Main)),
        )
        .add_systems(
            OnEnter(MenuScreen::Campaign),
            (choose_default_mission, spawn_campaign).chain(),
        )
        .add_systems(OnEnter(MenuScreen::Multiplayer), spawn_multiplayer)
        .add_systems(OnExit(MenuScreen::Multiplayer), keep_player_name)
        .add_systems(
            OnEnter(MenuScreen::Settings),
            spawn_settings.run_if(fallback_page(Page::Options)),
        )
        .add_systems(
            OnEnter(MenuScreen::Credits),
            spawn_credits.run_if(fallback_page(Page::Credits)),
        )
        .add_systems(
            Update,
            (
                handle_menu_clicks,
                back_on_escape,
                load_briefing,
                refresh_campaign,
                refresh_multiplayer,
            )
                .chain()
                .after(theme::dispatch_clicks)
                .run_if(in_state(bft_sim::states::AppState::FrontEnd)),
        );
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn spawn_main(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    failure: Option<Res<LoadFailure>>,
    catalog: Res<LevelCatalog>,
    mut saves: ResMut<crate::savegame::SaveListing>,
) {
    let failure = failure.and_then(|failure| failure.0.clone());
    saves.refresh();
    let has_saves = saves.entries.iter().any(|entry| entry.is_loadable());

    commands
        .spawn((screen_root(), DespawnOnExit(MenuScreen::Main)))
        .insert(Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            row_gap: px(10),
            ..default()
        })
        .with_children(|root| {
            root.spawn(title_text("OPEN-BFT", 56.0, &fonts));
            root.spawn((
                label_text("Recoil, rebuilt", BODY_SIZE, TEXT_DIM),
                Node {
                    margin: UiRect::bottom(px(28)),
                    ..default()
                },
            ));

            let has_campaign = catalog.campaign().next().is_some();
            let has_maps = catalog.levels.iter().any(|entry| !is_campaign(entry));

            let mut campaign = root.spawn(button(
                "Campaign",
                UiAction::Open(MenuScreen::Campaign),
                320.0,
            ));
            if !has_campaign {
                campaign.insert(Disabled);
            }
            let mut load = root.spawn(button("Load game", UiAction::Open(MenuScreen::Load), 320.0));
            if !has_saves {
                load.insert(Disabled);
            }
            let mut multiplayer = root.spawn(button(
                "Multiplayer",
                UiAction::Open(MenuScreen::Multiplayer),
                320.0,
            ));
            if !has_maps {
                multiplayer.insert(Disabled);
            }
            root.spawn(button(
                "Settings",
                UiAction::Open(MenuScreen::Settings),
                320.0,
            ));
            root.spawn(button(
                "Credits",
                UiAction::Open(MenuScreen::Credits),
                320.0,
            ));
            root.spawn(button("Quit", UiAction::Quit, 320.0));

            if let Some(failure) = failure {
                root.spawn((
                    label_text(failure, BODY_SIZE, ERROR),
                    Node {
                        margin: UiRect::top(px(20)),
                        max_width: px(720),
                        ..default()
                    },
                ));
            }
        });
}

fn is_campaign(entry: &LevelEntry) -> bool {
    matches!(entry.kind, LevelKind::Campaign { .. })
}

/// A level's display name: its `MSG_CAMPAIGN<N>_NAME` title, falling back to
/// the directory name when there is no string table.
pub fn level_title(entry: &LevelEntry, messages: Option<&Messages>) -> String {
    messages
        .and_then(|table| table.get(&entry.title_key))
        .map(str::to_string)
        .unwrap_or_else(|| entry.name.clone())
}

// ---------------------------------------------------------------------------
// Campaign
// ---------------------------------------------------------------------------

/// The mission the campaign screen has selected, and its reader archive.
#[derive(Resource, Debug, Clone, Default)]
pub struct CampaignSelection {
    pub mission: Option<LevelId>,
    /// Loaded to build the briefing; `None` until a mission is selected.
    reader: Option<Handle<ReaderData>>,
    /// The briefing, once the reader has loaded: `(summary, text)` pairs.
    briefing: Option<Vec<(String, String)>>,
}

#[derive(Component, Debug, Clone, Copy)]
struct MissionButton(LevelId);

#[derive(Component, Debug, Clone, Copy)]
struct StartButton;

#[derive(Component, Debug, Clone, Copy)]
struct BriefingTitle;

#[derive(Component, Debug, Clone, Copy)]
struct BriefingBody;

#[derive(Component, Debug, Clone, Copy)]
struct DifficultyLabel;

/// Select the furthest mission the player may play, so "Campaign → Start"
/// continues the campaign.
fn choose_default_mission(
    mut selection: ResMut<CampaignSelection>,
    campaign: Option<Res<Campaign>>,
    progress: Option<Res<Progress>>,
) {
    let Some(campaign) = campaign else {
        return;
    };
    let unlocked = |id: LevelId| progress.as_ref().is_none_or(|p| p.is_unlocked(id));
    let done = |id: LevelId| {
        progress
            .as_ref()
            .is_some_and(|p| p.completed.contains(&id.0))
    };

    let pick = campaign
        .missions
        .iter()
        .find(|mission| unlocked(mission.id) && !done(mission.id))
        .or_else(|| campaign.missions.iter().rev().find(|m| unlocked(m.id)))
        .map(|mission| mission.id);

    if selection.mission.is_none() || selection.mission.is_some_and(|id| !unlocked(id)) {
        select_mission(&mut selection, pick);
    }
}

fn select_mission(selection: &mut CampaignSelection, mission: Option<LevelId>) {
    if selection.mission != mission {
        selection.mission = mission;
        selection.reader = None;
        selection.briefing = None;
    }
}

// Six parameters: the screen needs the campaign, its progress, the strings
// and the fonts, and each is read once.
#[allow(clippy::too_many_arguments)]
fn spawn_campaign(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    campaign: Option<Res<Campaign>>,
    progress: Option<Res<Progress>>,
    globals: Option<Res<Globals>>,
    tables: Res<Assets<Messages>>,
    catalog: Res<LevelCatalog>,
) {
    let messages = globals::messages(globals.as_deref(), &tables);

    commands
        .spawn((screen_root(), DespawnOnExit(MenuScreen::Campaign)))
        .with_children(|root| {
            root.spawn(title_text("Campaign", TITLE_SIZE, &fonts));
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: px(24),
                width: percent(100),
                max_width: px(1100),
                flex_grow: 1.0,
                ..default()
            })
            .with_children(|columns| {
                // The mission list.
                columns
                    .spawn(Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: px(6),
                        width: px(340),
                        flex_shrink: 0.0,
                        ..default()
                    })
                    .with_children(|list| {
                        for (index, mission) in campaign
                            .iter()
                            .flat_map(|campaign| campaign.missions.iter())
                            .enumerate()
                        {
                            let title = catalog
                                .level(mission.id)
                                .map(|entry| level_title(entry, messages))
                                .unwrap_or_else(|| mission.id.to_string());
                            let unlocked =
                                progress.as_ref().is_none_or(|p| p.is_unlocked(mission.id));
                            let done = progress
                                .as_ref()
                                .is_some_and(|p| p.completed.contains(&mission.id.0));
                            let suffix = match (unlocked, done) {
                                (false, _) => "   [locked]",
                                (true, true) => "   [done]",
                                (true, false) => "",
                            };

                            let mut row = list.spawn((
                                button(
                                    format!("{}   {title}{suffix}", index + 1),
                                    UiAction::SelectMission(mission.id),
                                    340.0,
                                ),
                                MissionButton(mission.id),
                            ));
                            row.insert(Node {
                                width: px(340),
                                min_height: px(34),
                                padding: UiRect::axes(px(14), px(6)),
                                justify_content: JustifyContent::FlexStart,
                                align_items: AlignItems::Center,
                                border: UiRect::all(px(1)),
                                ..default()
                            });
                            if !unlocked {
                                row.insert(Disabled);
                            }
                        }

                        list.spawn(Node {
                            height: px(12),
                            ..default()
                        });
                        list.spawn(button("", UiAction::CycleDifficulty, 340.0))
                            .with_children(|button| {
                                button.spawn((label_text("", BODY_SIZE, TEXT), DifficultyLabel));
                            });
                        list.spawn(button("Back", UiAction::Back, 340.0));
                    });

                // The briefing.
                columns
                    .spawn(theme::panel())
                    .insert(Node {
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::all(px(20)),
                        row_gap: px(12),
                        border: UiRect::left(px(3)),
                        flex_grow: 1.0,
                        ..default()
                    })
                    .with_children(|briefing| {
                        briefing.spawn((title_text("", HEADING_SIZE, &fonts), BriefingTitle));
                        briefing.spawn((
                            Text::default(),
                            TextFont::from_font_size(BODY_SIZE),
                            TextColor(TEXT),
                            Pickable::IGNORE,
                            BriefingBody,
                            Node {
                                flex_grow: 1.0,
                                ..default()
                            },
                        ));
                        briefing
                            .spawn(Node {
                                flex_direction: FlexDirection::Row,
                                column_gap: px(8),
                                ..default()
                            })
                            .with_children(|buttons| {
                                buttons.spawn((
                                    button("Start mission", UiAction::StartMission, 220.0),
                                    StartButton,
                                ));
                                buttons.spawn(button("View level", UiAction::ViewSelected, 160.0));
                            });
                    });
            });
        });
}

/// Ask for the selected mission's reader archive, and build the briefing
/// once it lands.
fn load_briefing(
    mut selection: ResMut<CampaignSelection>,
    catalog: Res<LevelCatalog>,
    server: Option<Res<AssetServer>>,
    readers: Option<Res<Assets<ReaderData>>>,
    globals: Option<Res<Globals>>,
    tables: Res<Assets<Messages>>,
) {
    let Some(mission) = selection.mission else {
        return;
    };
    if selection.briefing.is_some() {
        return;
    }
    let (Some(server), Some(readers)) = (server, readers) else {
        return;
    };

    let Some(handle) = selection.reader.clone() else {
        if let Some(entry) = catalog.level(mission) {
            selection.reader = Some(server.load::<ReaderData>(entry.reader.clone()));
        }
        return;
    };

    if let Some(reader) = readers.get(&handle) {
        let messages = globals::messages(globals.as_deref(), &tables);
        selection.briefing = Some(briefing_lines(
            build_mission_script(reader).as_ref(),
            messages,
        ));
    } else if server.load_state(&handle).is_failed() {
        selection.briefing = Some(Vec::new());
    }
}

/// The briefing's objectives: those the mission starts with, as
/// `(summary, text)`.
pub fn briefing_lines(
    script: Option<&MissionScript>,
    messages: Option<&Messages>,
) -> Vec<(String, String)> {
    script
        .map(|script| {
            script
                .objectives
                .iter()
                .filter(|objective| objective.initially_active)
                .map(|objective| {
                    (
                        message_text(messages, &objective.summary_key),
                        message_text(messages, &objective.text_key),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Keep the campaign screen's selection, briefing, start button and
/// difficulty in step.
// Eight parameters: every query is one widget of the screen.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn refresh_campaign(
    mut commands: Commands,
    selection: Res<CampaignSelection>,
    difficulty: Option<Res<Difficulty>>,
    progress: Option<Res<Progress>>,
    catalog: Res<LevelCatalog>,
    globals: Option<Res<Globals>>,
    tables: Res<Assets<Messages>>,
    missions: Query<(Entity, &MissionButton, Has<Selected>)>,
    starts: Query<(Entity, Has<Disabled>), With<StartButton>>,
    mut titles: Query<&mut Text, (With<BriefingTitle>, Without<BriefingBody>)>,
    mut bodies: Query<&mut Text, (With<BriefingBody>, Without<BriefingTitle>)>,
    mut labels: Query<
        &mut Text,
        (
            With<DifficultyLabel>,
            Without<BriefingTitle>,
            Without<BriefingBody>,
        ),
    >,
) {
    for (entity, button, selected) in &missions {
        let want = selection.mission == Some(button.0);
        if want && !selected {
            commands.entity(entity).insert(Selected);
        } else if !want && selected {
            commands.entity(entity).remove::<Selected>();
        }
    }

    let playable = selection
        .mission
        .is_some_and(|id| progress.as_ref().is_none_or(|p| p.is_unlocked(id)));
    for (entity, disabled) in &starts {
        if playable && disabled {
            commands.entity(entity).remove::<Disabled>();
        } else if !playable && !disabled {
            commands.entity(entity).insert(Disabled);
        }
    }

    let messages = globals::messages(globals.as_deref(), &tables);
    let title = selection
        .mission
        .and_then(|id| catalog.level(id))
        .map(|entry| level_title(entry, messages))
        .unwrap_or_default();
    for mut text in &mut titles {
        text.set_if_neq(Text::new(title.clone()));
    }

    let body = match (&selection.mission, &selection.briefing) {
        (None, _) => "Choose a mission.".to_string(),
        (Some(_), None) => "Loading the briefing...".to_string(),
        (Some(_), Some(lines)) if lines.is_empty() => {
            "This mission has no briefing in the data.".to_string()
        }
        (Some(_), Some(lines)) => lines
            .iter()
            .map(|(summary, text)| format!("{summary}\n{text}"))
            .collect::<Vec<_>>()
            .join("\n\n"),
    };
    for mut text in &mut bodies {
        text.set_if_neq(Text::new(body.clone()));
    }

    let difficulty = difficulty.map(|d| *d).unwrap_or_default();
    for mut text in &mut labels {
        text.set_if_neq(Text::new(format!("Difficulty: {difficulty:?}")));
    }
}

// ---------------------------------------------------------------------------
// Multiplayer
// ---------------------------------------------------------------------------

/// The map the multiplayer screen has selected, and what happened last.
#[derive(Resource, Debug, Clone, Default)]
pub struct MultiplayerSelection {
    pub map: Option<LevelId>,
    status: Option<(String, bool)>,
}

#[derive(Component, Debug, Clone, Copy)]
struct MapButton(LevelId);

#[derive(Component, Debug, Clone, Copy)]
struct MapTitle;

#[derive(Component, Debug, Clone, Copy)]
struct MultiplayerStatus;

/// Which of the three text fields this is.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Port,
    Address,
}

fn spawn_multiplayer(
    mut commands: Commands,
    fonts: Res<UiFonts>,
    catalog: Res<LevelCatalog>,
    globals: Option<Res<Globals>>,
    tables: Res<Assets<Messages>>,
    settings: Res<Settings>,
    mut selection: ResMut<MultiplayerSelection>,
) {
    let messages = globals::messages(globals.as_deref(), &tables);
    let maps: Vec<&LevelEntry> = catalog.levels.iter().filter(|e| !is_campaign(e)).collect();
    if selection
        .map
        .is_none_or(|id| !maps.iter().any(|e| e.id == id))
    {
        selection.map = maps.first().map(|entry| entry.id);
    }
    selection.status = None;

    commands
        .spawn((screen_root(), DespawnOnExit(MenuScreen::Multiplayer)))
        .with_children(|root| {
            root.spawn(title_text("Multiplayer", TITLE_SIZE, &fonts));
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: px(24),
                width: percent(100),
                max_width: px(1100),
                ..default()
            })
            .with_children(|columns| {
                columns
                    .spawn(Node {
                        flex_direction: FlexDirection::Column,
                        row_gap: px(6),
                        width: px(340),
                        flex_shrink: 0.0,
                        ..default()
                    })
                    .with_children(|list| {
                        for entry in &maps {
                            list.spawn((
                                button(
                                    format!(
                                        "{}   {}",
                                        level_title(entry, messages),
                                        mode_label(entry)
                                    ),
                                    UiAction::SelectMap(entry.id),
                                    340.0,
                                ),
                                MapButton(entry.id),
                            ));
                        }
                        list.spawn(Node {
                            height: px(12),
                            ..default()
                        });
                        list.spawn(button("Back", UiAction::Back, 340.0));
                    });

                columns
                    .spawn(theme::panel())
                    .insert(Node {
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::all(px(20)),
                        row_gap: px(12),
                        border: UiRect::left(px(3)),
                        flex_grow: 1.0,
                        ..default()
                    })
                    .with_children(|panel| {
                        panel.spawn((title_text("", HEADING_SIZE, &fonts), MapTitle));
                        field_row(panel, "Player name", Field::Name, &settings.player_name, 24);
                        field_row(panel, "Port", Field::Port, &DEFAULT_PORT.to_string(), 5);
                        field_row(panel, "Join address", Field::Address, "127.0.0.1:27815", 40);
                        panel
                            .spawn(Node {
                                flex_direction: FlexDirection::Row,
                                flex_wrap: FlexWrap::Wrap,
                                column_gap: px(8),
                                row_gap: px(8),
                                ..default()
                            })
                            .with_children(|buttons| {
                                buttons.spawn(button("Host", UiAction::Host, 150.0));
                                buttons.spawn(button("Join", UiAction::Join, 150.0));
                                buttons.spawn(button("Practice alone", UiAction::Practice, 200.0));
                                buttons.spawn(button("View level", UiAction::ViewSelected, 160.0));
                            });
                        panel.spawn((label_text("", SMALL_SIZE, TEXT_DIM), MultiplayerStatus));
                        panel.spawn(label_text(
                            "Practice plays the map offline, without other players. \
                             m7-m12 ship m1's automap as a placeholder, so no map is shown \
                             on them (docs/data/quirks.md).",
                            SMALL_SIZE,
                            TEXT_DIM,
                        ));
                    });
            });
        });
}

/// A labelled one-line text field.
fn field_row(
    parent: &mut ChildSpawnerCommands,
    label: &str,
    field: Field,
    initial: &str,
    max: usize,
) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(12),
            ..default()
        })
        .with_children(|row| {
            row.spawn((
                label_text(label, BODY_SIZE, TEXT),
                Node {
                    width: px(160),
                    ..default()
                },
            ));
            let mut input = row.spawn((
                field,
                Node {
                    width: px(320),
                    padding: UiRect::axes(px(8), px(4)),
                    border: UiRect::all(px(1)),
                    ..default()
                },
                BorderColor::all(CYAN),
                BackgroundColor(Color::srgb(0.05, 0.06, 0.08)),
                EditableText {
                    max_characters: Some(max),
                    allow_newlines: false,
                    ..EditableText::new(initial)
                },
                TextLayout::no_wrap(),
                TextFont::from_font_size(BODY_SIZE),
                TextColor(TEXT),
                TextCursorStyle::default(),
            ));
            if field == Field::Port {
                input.insert(EditableTextFilter::new(|c| c.is_ascii_digit()));
            }
        });
}

/// What a map is for, in brackets.
fn mode_label(entry: &LevelEntry) -> &'static str {
    match entry.kind {
        LevelKind::Multiplayer {
            mode: MultiplayerMode::Race,
        } => "[race]",
        LevelKind::Multiplayer {
            mode: MultiplayerMode::Teleport,
        } => "[arena]",
        _ => "[deathmatch]",
    }
}

// Eight parameters: every query is one widget of the screen.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn refresh_multiplayer(
    mut commands: Commands,
    selection: Res<MultiplayerSelection>,
    catalog: Res<LevelCatalog>,
    globals: Option<Res<Globals>>,
    tables: Res<Assets<Messages>>,
    maps: Query<(Entity, &MapButton, Has<Selected>)>,
    mut titles: Query<&mut Text, (With<MapTitle>, Without<MultiplayerStatus>)>,
    mut statuses: Query<(&mut Text, &mut TextColor), (With<MultiplayerStatus>, Without<MapTitle>)>,
) {
    for (entity, button, selected) in &maps {
        let want = selection.map == Some(button.0);
        if want && !selected {
            commands.entity(entity).insert(Selected);
        } else if !want && selected {
            commands.entity(entity).remove::<Selected>();
        }
    }

    let messages = globals::messages(globals.as_deref(), &tables);
    let title = selection
        .map
        .and_then(|id| catalog.level(id))
        .map(|entry| level_title(entry, messages))
        .unwrap_or_else(|| "No multiplayer maps".to_string());
    for mut text in &mut titles {
        text.set_if_neq(Text::new(title.clone()));
    }

    let (status, error) = selection.status.clone().unwrap_or_default();
    for (mut text, mut colour) in &mut statuses {
        text.set_if_neq(Text::new(status.clone()));
        colour.set_if_neq(TextColor(if error { ERROR } else { GREEN }));
    }
}

/// The current text of one field.
fn field_value(fields: &Query<(&Field, &EditableText)>, which: Field) -> Option<String> {
    fields
        .iter()
        .find(|(field, _)| **field == which)
        .map(|(_, text)| text.value().to_string())
}

/// Store the player name when leaving the screen.
fn keep_player_name(fields: Query<(&Field, &EditableText)>, mut settings: ResMut<Settings>) {
    if let Some(name) = field_value(&fields, Field::Name) {
        let name = name.trim().to_string();
        if !name.is_empty() && settings.player_name != name {
            settings.player_name = name;
        }
    }
}

/// Parse the port field.
pub fn parse_port(text: &str) -> Result<u16, String> {
    match text.trim().parse::<u16>() {
        Ok(0) | Err(_) => Err(format!("\"{text}\" is not a port (1-65535)")),
        Ok(port) => Ok(port),
    }
}

/// Parse the join field: `host:port`, or a bare address on the default port.
pub fn parse_address(text: &str) -> Result<SocketAddr, String> {
    let text = text.trim();
    text.parse::<SocketAddr>()
        .or_else(|_| {
            text.parse::<std::net::IpAddr>()
                .map(|ip| SocketAddr::new(ip, DEFAULT_PORT))
        })
        .map_err(|_| format!("\"{text}\" is not an address like 192.168.1.20:{DEFAULT_PORT}"))
}

// ---------------------------------------------------------------------------
// Settings and credits
// ---------------------------------------------------------------------------

pub(crate) fn spawn_settings_screen(
    commands: &mut Commands,
    fonts: &UiFonts,
    paths: Option<&DataPaths>,
    screen: MenuScreen,
) {
    let root = commands
        .spawn((screen_root(), DespawnOnExit(screen)))
        .with_children(|root| {
            root.spawn(title_text("Settings", TITLE_SIZE, fonts));
        })
        .id();
    spawn_settings_panel(commands, root, paths);
}

fn spawn_settings(mut commands: Commands, fonts: Res<UiFonts>, paths: Option<Res<DataPaths>>) {
    spawn_settings_screen(
        &mut commands,
        &fonts,
        paths.as_deref(),
        MenuScreen::Settings,
    );
}

/// The credits: licences and attribution.
pub(crate) const CREDITS: &[(&str, &str)] = &[
    (
        "open-bft",
        "An open reimplementation of Recoil (Zipper Interactive, 1999) in Rust \
         and Bevy. Licensed under the EUPL-1.2.",
    ),
    (
        "Your game data",
        "Every level, texture, sound, string and the Eurostile font are read from \
         your own Recoil install at run time. None of it is part of open-bft or \
         distributed with it.",
    ),
    (
        "mech3ax",
        "The game's archives are extracted with mech3ax by TerranMechworks \
         (github.com/TerranMechworks/mech3ax), EUPL-1.2. Without its reverse \
         engineering of the formats this project could not exist.",
    ),
    (
        "Libraries",
        "Bevy (MIT / Apache-2.0), avian3d (MIT / Apache-2.0), serde, ron, zip, \
         rand, clap, directories, thiserror and their dependencies, each under \
         its own licence.",
    ),
    (
        "Art",
        "The original menus, HUD and cursors are drawn from your install's \
         own art.",
    ),
];

fn spawn_credits(mut commands: Commands, fonts: Res<UiFonts>) {
    commands
        .spawn((screen_root(), DespawnOnExit(MenuScreen::Credits)))
        .with_children(|root| {
            root.spawn(title_text("Credits", TITLE_SIZE, &fonts));
            root.spawn(theme::panel())
                .insert(Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(px(20)),
                    row_gap: px(14),
                    border: UiRect::left(px(3)),
                    max_width: px(820),
                    ..default()
                })
                .with_children(|panel| {
                    for (heading, body) in CREDITS {
                        panel.spawn(label_text(*heading, BODY_SIZE, ACCENT));
                        panel.spawn(label_text(*body, BODY_SIZE, TEXT));
                    }
                });
            root.spawn(button("Back", UiAction::Back, 220.0));
        });
}

// ---------------------------------------------------------------------------
// Clicks
// ---------------------------------------------------------------------------

/// Act on the front end's buttons.
// The front end's one junction: every screen's buttons land here, so it
// holds everything any of them writes.
#[allow(clippy::too_many_arguments)]
fn handle_menu_clicks(
    mut clicks: MessageReader<UiClick>,
    current: Res<State<MenuScreen>>,
    mut screen: ResMut<NextState<MenuScreen>>,
    mut campaign: ResMut<CampaignSelection>,
    mut multiplayer: ResMut<MultiplayerSelection>,
    mut requests: MenuRequests,
    fields: Query<(&Field, &EditableText)>,
    mut settings: ResMut<Settings>,
    mut difficulty: Option<ResMut<Difficulty>>,
    mut progress: Option<ResMut<Progress>>,
    unlock_all: Option<Res<UnlockAll>>,
    mut exit: MessageWriter<AppExit>,
) {
    for click in clicks.read() {
        match click.0 {
            UiAction::Open(next) => screen.set(next),
            UiAction::Page(page) => {
                if let Some(next) = page.screen() {
                    screen.set(next);
                }
            }
            UiAction::Back => screen.set(back_from(*current.get())),
            UiAction::Quit => {
                exit.write(AppExit::Success);
            }
            UiAction::SelectMission(id) => select_mission(&mut campaign, Some(id)),
            UiAction::StartMission => {
                if let Some(id) = campaign.mission {
                    requests.start.write(RequestStartMission(id));
                }
            }
            UiAction::CycleDifficulty => {
                let next = match difficulty.as_deref().copied().unwrap_or_default() {
                    Difficulty::Easy => Difficulty::Normal,
                    Difficulty::Normal => Difficulty::Hard,
                    Difficulty::Hard => Difficulty::Easy,
                };
                if let Some(difficulty) = difficulty.as_mut() {
                    **difficulty = next;
                }
                if let Some(progress) = progress.as_mut() {
                    progress.difficulty = next;
                    // `--unlock-all` must never write the save (FR-M7).
                    if !unlock_all.as_ref().is_some_and(|flag| flag.0)
                        && let Err(e) = progress.save()
                    {
                        warn!("cannot save the difficulty: {e}");
                    }
                }
            }
            UiAction::SelectMap(id) => {
                multiplayer.map = Some(id);
                multiplayer.status = None;
            }
            UiAction::ViewSelected => {
                let level = match current.get() {
                    MenuScreen::Campaign => campaign.mission,
                    MenuScreen::Multiplayer => multiplayer.map,
                    _ => None,
                };
                if let Some(id) = level {
                    requests.viewer.write(RequestViewer(id));
                }
            }
            UiAction::Practice => {
                if let Some(id) = multiplayer.map {
                    requests.practice.write(RequestPractice(id));
                }
            }
            UiAction::Host => {
                let Some(level) = multiplayer.map else {
                    continue;
                };
                let port = field_value(&fields, Field::Port).unwrap_or_default();
                multiplayer.status = Some(match parse_port(&port) {
                    Ok(port) => {
                        requests.host.write(RequestHost { level, port });
                        (format!("Hosting {level} on port {port}..."), false)
                    }
                    Err(e) => (e, true),
                });
                remember_name(&fields, &mut settings);
            }
            UiAction::Join => {
                let address = field_value(&fields, Field::Address).unwrap_or_default();
                multiplayer.status = Some(match parse_address(&address) {
                    Ok(addr) => {
                        requests.join.write(RequestJoin { addr });
                        (format!("Joining {addr}..."), false)
                    }
                    Err(e) => (e, true),
                });
                remember_name(&fields, &mut settings);
            }
            _ => {}
        }
    }
}

/// The five requests the front end can make, bundled to keep the handler's
/// parameter list readable.
#[derive(bevy::ecs::system::SystemParam)]
struct MenuRequests<'w> {
    start: MessageWriter<'w, RequestStartMission>,
    practice: MessageWriter<'w, RequestPractice>,
    viewer: MessageWriter<'w, RequestViewer>,
    host: MessageWriter<'w, RequestHost>,
    join: MessageWriter<'w, RequestJoin>,
}

fn remember_name(fields: &Query<(&Field, &EditableText)>, settings: &mut Settings) {
    if let Some(name) = field_value(fields, Field::Name) {
        let name = name.trim().to_string();
        if !name.is_empty() && settings.player_name != name {
            settings.player_name = name;
        }
    }
}

/// Escape (the `Pause` action) backs out of a sub-screen, unless a rebind is
/// waiting for it or a text field has the keyboard.
fn back_on_escape(
    input: ActionInput,
    rebind: Res<RebindState>,
    focus: Option<Res<InputFocus>>,
    fields: Query<(), With<EditableText>>,
    current: Res<State<MenuScreen>>,
    mut screen: ResMut<NextState<MenuScreen>>,
) {
    if rebind.is_capturing() || !input.just_pressed(Action::Pause) {
        return;
    }
    if focus
        .and_then(|focus| focus.get())
        .is_some_and(|entity| fields.contains(entity))
    {
        return;
    }
    if *current.get() != MenuScreen::Main {
        screen.set(back_from(*current.get()));
    }
}

/// Where Back and Escape lead: up one page, which for most is the main
/// menu.
pub fn back_from(screen: MenuScreen) -> MenuScreen {
    match screen {
        MenuScreen::Commands => MenuScreen::Controls,
        MenuScreen::MoreSettings => MenuScreen::Settings,
        _ => MenuScreen::Main,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_sim::mission::{Completion, ObjectiveDef};

    fn objective(index: u8, active: bool) -> ObjectiveDef {
        ObjectiveDef {
            index,
            picture: String::new(),
            summary_key: format!("MSG_M1_O{}_SUMMARY", index + 1),
            text_key: format!("MSG_M1_O{}_OBJ", index + 1),
            complete_key: format!("MSG_M1_O{}_COMP", index + 1),
            initially_active: active,
            completion: Completion::ExitAnim,
            read_sound: None,
            hint: None,
        }
    }

    /// The briefing shows what the mission starts with and hides what the
    /// file marks `INACTIVE`, resolved through the table.
    #[test]
    fn the_briefing_lists_only_the_objectives_the_mission_starts_with() {
        let script = MissionScript {
            objectives: vec![objective(0, true), objective(1, false), objective(2, true)],
            read_time: 8.0,
            review_delay: 4.0,
            review_sound: String::new(),
            objective_sound: String::new(),
            new_game_anim: String::new(),
            load_game_anim: String::new(),
            movers: Vec::new(),
            death_anim: String::new(),
            final_mission: false,
        };
        let mut messages = Messages::default();
        messages
            .entries
            .insert("MSG_M1_O1_SUMMARY".into(), "M1: Short Circuit".into());

        let lines = briefing_lines(Some(&script), Some(&messages));

        assert_eq!(
            lines,
            vec![
                ("M1: Short Circuit".into(), "MSG_M1_O1_OBJ".into()),
                ("MSG_M1_O3_SUMMARY".into(), "MSG_M1_O3_OBJ".into()),
            ]
        );
    }

    #[test]
    fn ports_and_addresses_parse_or_say_why_not() {
        assert_eq!(parse_port(" 7777 "), Ok(7777));
        assert!(parse_port("0").is_err());
        assert!(parse_port("70000").is_err());

        assert_eq!(
            parse_address("10.0.0.2:9000"),
            Ok("10.0.0.2:9000".parse().unwrap())
        );
        assert_eq!(
            parse_address("10.0.0.2"),
            Ok(SocketAddr::new("10.0.0.2".parse().unwrap(), DEFAULT_PORT))
        );
        assert!(parse_address("somewhere").is_err());
    }

    #[test]
    fn the_default_mission_is_the_next_one_to_play() {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins)
            .init_resource::<CampaignSelection>()
            .insert_resource(Campaign {
                missions: (1..=3)
                    .map(|id| bft_sim::mission::MissionDef {
                        id: LevelId(id),
                        level: LevelId(id),
                        title_key: String::new(),
                        briefing_video: None,
                        briefing_layout: id as u8,
                    })
                    .collect(),
            })
            .insert_resource(Progress {
                completed: vec![1],
                ..Progress::default()
            })
            .add_systems(Update, choose_default_mission);

        app.update();

        assert_eq!(
            app.world().resource::<CampaignSelection>().mission,
            Some(LevelId(2))
        );
    }
}
