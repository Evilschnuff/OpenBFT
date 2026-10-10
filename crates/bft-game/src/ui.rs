//! Player-facing UI: the front end, the HUD, the pause and mission-end
//! overlays, and the settings behind them.
//!
//! # The flow
//!
//! [`AppState::FrontEnd`] has a [`MenuScreen`] sub-state: `Main` leads to
//! `Campaign` (mission list, briefing, Start), `Multiplayer` (map list, host,
//! join, practice), `Settings` and `Credits`. Starting anything is a message
//! — [`RequestStartMission`] for the campaign, [`RequestHost`] and
//! [`RequestJoin`] for multiplayer — so the Retry and Next buttons on the
//! mission-end screen go through exactly the path the menu does.
//!
//! In [`AppState::InGame`] the [`hud`] draws health, weapons, the automap
//! and messages; `Pause` opens the [`overlays`]' pause menu, and
//! `MissionEnded` their result screen. Both stop the sim through
//! [`Paused`] and stop gameplay input through [`gameplay_input_enabled`].
//!
//! # Why Bevy UI and not egui
//!
//! Player-facing screens are Bevy UI: they ship,
//! they take the game's font and cursor, and they need no second immediate-
//! mode layer on top of the game. egui stays for the developer
//! tools, where layout speed matters more than looks.

use std::net::SocketAddr;

use bevy::prelude::*;
use bft_assets::catalog::{LevelCatalog, LevelKind};
use bft_sim::level::DespawnLevel;
use bft_sim::mission::{MissionId, Progress};
use bft_sim::states::{AppState, GameMode, Paused, PendingLevel};
use bft_sim::vehicle::Difficulty;

use crate::level_view::LoadedLevel;

pub mod automap;
pub mod bindings;
pub mod cursor;
pub mod dialog;
pub mod hud;
pub mod menus;
pub mod overlays;
pub mod saves;
pub mod settings;
pub mod settings_screen;
pub mod theme;

pub use bindings::{Action, ActionInput, Binding, InputBindings};
pub use hud::{HudHealth, HudMap, HudMessages, HudRoot, HudVisible, HudWeapon};
pub use settings::{
    HudSettings, Settings, SettingsFile, VideoSettings, WindowModeSetting, load_settings,
    save_settings,
};

/// Which front-end screen is showing.
#[derive(SubStates, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
#[source(AppState = AppState::FrontEnd)]
pub enum MenuScreen {
    #[default]
    Main,
    Campaign,
    Multiplayer,
    Settings,
    Credits,
    /// The saved games, to load one.
    Load,
    /// The original menus' pages, which the fallback menus do not have: the
    /// new game panel, the controls and commands dialogs, "really quit?", and
    /// the fallback settings panel behind the original options.
    NewGame,
    Controls,
    Commands,
    ConfirmQuit,
    MoreSettings,
}

/// Whether the pause menu is up.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PauseMenuOpen(pub bool);

/// Start (or restart) a campaign mission.
///
/// A [`Message`] rather than an `Event`: since Bevy 0.17
/// `Event` means an observer trigger, and this is a request any number of
/// systems may read on their own schedule, which is what `Message` is.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestStartMission(pub MissionId);

/// Host a multiplayer game. Nothing consumes it yet: there is no multiplayer.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestHost {
    pub level: bft_assets::catalog::LevelId,
    pub port: u16,
}

/// Join a multiplayer game. Nothing consumes it yet: there is no multiplayer.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestJoin {
    pub addr: SocketAddr,
}

/// Play a multiplayer map offline, alone: the practice button.
///
/// Without multiplayer it is the only way to drive m7–m13 from the menu, which the skeleton front end offered and this one
/// must not take away.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestPractice(pub bft_assets::catalog::LevelId);

/// Open a level in the free-fly viewer (FR-T1).
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestViewer(pub bft_assets::catalog::LevelId);

/// The whole player-facing UI.
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        // The settings first: the input map, the audio and the window all
        // read their part of them from the first frame.
        if !app.world().contains_resource::<SettingsFile>() {
            app.init_resource::<SettingsFile>();
        }
        if !app.world().contains_resource::<Settings>() {
            let file = app.world().resource::<SettingsFile>().0.clone();
            let settings = file
                .as_deref()
                .map(settings::load_settings_or_default)
                .unwrap_or_default();
            app.insert_resource(settings);
        }
        let settings = app.world().resource::<Settings>().clone();
        app.insert_resource(settings.input.clone());
        if !app
            .world()
            .contains_resource::<bft_assets::config::AudioSettings>()
        {
            app.insert_resource(settings.audio.clone());
        }

        theme::install_fonts(app);
        dialog::install_splash(app);

        app.add_sub_state::<MenuScreen>()
            .init_resource::<PauseMenuOpen>()
            .init_resource::<theme::ClickGuard>()
            .add_message::<theme::UiClick>()
            .add_message::<RequestStartMission>()
            .add_message::<RequestHost>()
            .add_message::<RequestJoin>()
            .add_message::<RequestPractice>()
            .add_message::<RequestViewer>()
            .add_systems(
                Update,
                (
                    theme::dispatch_clicks,
                    theme::button_colours,
                    settings::sync_settings,
                    settings::apply_texture_filter,
                    start_requested_level,
                    adopt_saved_difficulty.run_if(resource_added::<Progress>),
                ),
            )
            .add_systems(OnExit(AppState::InGame), leave_game);

        settings_screen::plugin(app);
        menus::plugin(app);
        hud::plugin(app);
        automap::plugin(app);
        overlays::plugin(app);
        saves::plugin(app);
        cursor::plugin(app);
        dialog::plugin(app);
    }
}

/// Whether gameplay input — driving, aiming, firing, the HUD toggles —
/// should be read: in game, and no menu or result screen over it.
pub fn gameplay_input_enabled(gate: GameplayGate) -> bool {
    gate.enabled()
}

/// What [`gameplay_input_enabled`] reads, for systems that need the answer
/// alongside other work.
#[derive(bevy::ecs::system::SystemParam)]
pub struct GameplayGate<'w> {
    state: Option<Res<'w, State<AppState>>>,
    pause: Option<Res<'w, PauseMenuOpen>>,
    ended: Option<Res<'w, overlays::MissionEndScreen>>,
    free_camera: Option<Res<'w, crate::devtools::free_camera::FreeCameraActive>>,
    control: Option<Res<'w, bft_sim::cinematic::PlayerControl>>,
}

impl GameplayGate<'_> {
    /// In game, with no pause menu or result screen up, the camera not flying
    /// free, and no cinematic running.
    pub fn enabled(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| *state.get() == AppState::InGame)
            && !self.pause.as_ref().is_some_and(|pause| pause.0)
            && !self.ended.as_ref().is_some_and(|ended| ended.is_showing())
            && !self.free_camera.as_ref().is_some_and(|free| free.0)
            && !self
                .control
                .as_ref()
                .is_some_and(|control| control.is_cinematic())
    }
}

/// Turn the start requests into a pending level and a state change.
// Seven parameters: the three requests, the catalog that says what kind of
// level each is, and the three things a start writes.
#[allow(clippy::too_many_arguments)]
fn start_requested_level(
    mut commands: Commands,
    mut missions: MessageReader<RequestStartMission>,
    mut practice: MessageReader<RequestPractice>,
    mut viewer: MessageReader<RequestViewer>,
    catalog: Res<LevelCatalog>,
    mut next: ResMut<NextState<AppState>>,
    mut mode: ResMut<NextState<GameMode>>,
) {
    // Last request wins; there is never more than one a frame in practice.
    let mut pending = None;
    for request in missions.read() {
        pending = Some((PendingLevel::in_game(request.0), GameMode::Campaign));
    }
    for request in practice.read() {
        pending = Some((PendingLevel::in_game(request.0), GameMode::Deathmatch));
    }
    for request in viewer.read() {
        pending = Some((PendingLevel::viewer(request.0), GameMode::Campaign));
    }

    let Some((level, game_mode)) = pending else {
        return;
    };
    if catalog.level(level.id).is_none() {
        warn!("level {} is not in the catalog", level.id);
        return;
    }

    info!("starting level {} ({game_mode:?})", level.id);
    if level.target == AppState::InGame {
        // `GameMode` is a sub-state of `InGame`; setting it now applies when
        // loading enters the game.
        mode.set(game_mode);
        // A mission starts from the level as the data has it. Retry asks for
        // the level already loaded, which the loader would otherwise reuse —
        // with every prop the last attempt broke still broken and the anim
        // instances spawned a second time.
        commands.trigger(DespawnLevel);
    }
    commands.insert_resource(level);
    next.set(AppState::Loading);
}

/// Play at the difficulty the save file records.
///
/// `Progress` carries it and the vehicle table is built for the
/// `Difficulty` resource, which nothing set from the save until the campaign
/// screen's difficulty button needed the two to agree.
fn adopt_saved_difficulty(progress: Res<Progress>, mut difficulty: ResMut<Difficulty>) {
    difficulty.set_if_neq(progress.difficulty);
}

/// Which game mode a level is played in when chosen from a list.
///
/// m1–m6 are the campaign and everything else is a multiplayer map
/// (docs/data/install.md); there are no race or teleporter modes yet.
pub fn game_mode_for(kind: LevelKind) -> GameMode {
    match kind {
        LevelKind::Campaign { .. } => GameMode::Campaign,
        _ => GameMode::Deathmatch,
    }
}

/// Clear the overlays and, when leaving for the menu, the level.
///
/// Retry, Next and a load go `InGame → Loading → InGame`, and whoever asked
/// for the next level has already torn this one down; only a real exit tears
/// it down here — the same rule the viewer follows.
fn leave_game(
    mut commands: Commands,
    next: Res<NextState<AppState>>,
    mut paused: ResMut<Paused>,
    mut pause_open: ResMut<PauseMenuOpen>,
) {
    paused.0 = false;
    pause_open.0 = false;

    if let NextState::Pending(AppState::Loading) = *next {
        return;
    }
    commands.trigger(DespawnLevel);
    commands.remove_resource::<LoadedLevel>();
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::catalog::{LevelEntry, LevelId, MultiplayerMode};

    pub(crate) fn entry(id: u16, kind: LevelKind) -> LevelEntry {
        LevelEntry {
            id: LevelId(id),
            name: format!("m{id}"),
            title_key: format!("MSG_CAMPAIGN{id}_NAME"),
            gamez: format!("cache://gamez/m{id}-gamez.gamez.zip").into(),
            anim: None,
            reader: format!("cache://reader/m{id}-zrdr.rdr.zip").into(),
            zmap: None,
            zmap_is_placeholder: false,
            textures: Vec::new(),
            kind,
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::MinimalPlugins, bevy::state::app::StatesPlugin));
        app.init_state::<AppState>()
            .add_sub_state::<GameMode>()
            .add_message::<RequestStartMission>()
            .add_message::<RequestPractice>()
            .add_message::<RequestViewer>()
            .insert_resource(LevelCatalog {
                levels: vec![
                    entry(1, LevelKind::Campaign { campaign: 1 }),
                    entry(
                        7,
                        LevelKind::Multiplayer {
                            mode: MultiplayerMode::Deathmatch,
                        },
                    ),
                ],
                ..default()
            })
            .add_systems(Update, start_requested_level);
        app
    }

    #[test]
    fn a_mission_request_loads_the_level_for_play() {
        let mut app = app();

        app.world_mut()
            .write_message(RequestStartMission(LevelId(1)));
        app.update();
        app.update();

        assert_eq!(
            app.world().resource::<PendingLevel>(),
            &PendingLevel::in_game(LevelId(1))
        );
        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::Loading
        );
    }

    #[test]
    fn practice_on_a_multiplayer_map_is_deathmatch_and_viewer_is_the_viewer() {
        let mut app = app();

        app.world_mut().write_message(RequestViewer(LevelId(7)));
        app.update();
        assert_eq!(
            app.world().resource::<PendingLevel>(),
            &PendingLevel::viewer(LevelId(7))
        );

        assert_eq!(
            game_mode_for(LevelKind::Multiplayer {
                mode: MultiplayerMode::Race
            }),
            GameMode::Deathmatch
        );
        assert_eq!(
            game_mode_for(LevelKind::Campaign { campaign: 2 }),
            GameMode::Campaign
        );
    }

    #[test]
    fn a_level_the_catalog_lacks_is_refused() {
        let mut app = app();

        app.world_mut()
            .write_message(RequestStartMission(LevelId(9)));
        app.update();

        assert!(app.world().get_resource::<PendingLevel>().is_none());
    }
}
