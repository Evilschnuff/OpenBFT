//! The two in-game overlays: the pause menu and the mission result.
//!
//! Both stop the simulation through [`Paused`], which gates every `SimSet`, and both stop gameplay input through
//! [`gameplay_input_enabled`](super::gameplay_input_enabled), so a click on
//! "Resume" does not also fire the gun.
//!
//! # When the result screen appears
//!
//! The mission sends `MissionEnded(Won)` from the exit animation's
//! `Callback 99` — the VTOL has already carried the player off — so a win is
//! shown at once. `MissionEnded(Lost)` is sent as the death cinematic *starts*
//! (the `bft_00` animation, docs/data/anims.md); the screen waits for that
//! animation to finish, with a timeout so a level whose death animation never
//! runs still ends.

use bevy::prelude::*;
use bft_assets::DataPaths;
use bft_assets::messages::Messages;
use bft_sim::anim::{AnimInstance, AnimProgram};
use bft_sim::mission::{Campaign, MissionEnded, MissionScript, MissionState, Outcome};
use bft_sim::states::{AppState, Paused};

use super::MenuScreen;
use super::bindings::{Action, ActionInput};
use super::dialog::screens::{FrontEndStart, Page, PageContext, Place, spawn_page};
use super::saves::{ListMode, spawn_save_list};
use super::settings_screen::{RebindState, spawn_settings_panel};
use super::theme::{
    self, BODY_SIZE, Disabled, ERROR, GREEN, HEADING_SIZE, SMALL_SIZE, TEXT, TEXT_DIM, TITLE_SIZE,
    UiAction, UiClick, UiFonts, button, label_text, message_text, title_text,
};
use super::{PauseMenuOpen, RequestStartMission};
use crate::devtools::free_camera::FreeCameraActive;
use crate::globals::{self, Globals};
use crate::savegame::{SaveListing, SaveRequest};
use bft_sim::cinematic::PlayerControl;
use bft_sim::save::SaveRefusal;
use bft_sim::states::GameMode;

/// The longest the result screen waits for the death cinematic, seconds.
///
/// Presentation timing: the shipped `bft_00` death animations run a few
/// seconds; this only matters when one never starts.
const DEATH_SCREEN_TIMEOUT: f32 = 10.0;

/// The result screen's state.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct MissionEndScreen {
    /// How the mission ended, once it has.
    pub outcome: Option<Outcome>,
    /// When `MissionEnded` arrived, in real seconds.
    since: f32,
    /// Whether the screen is up.
    shown: bool,
}

impl MissionEndScreen {
    /// Whether the result screen is up.
    pub fn is_showing(&self) -> bool {
        self.shown
    }
}

/// What the pause overlay is showing.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum PauseView {
    #[default]
    Buttons,
    Settings,
    /// The save list: overwrite one, or make a new one.
    Saves,
    /// "Overwrite save game?" for this entry of the listing.
    Confirm(u16),
    /// The load list.
    Loads,
    /// The original menus' pages, which only they reach.
    Controls,
    Commands,
    Credits,
    ConfirmQuit,
    /// The simple settings panel, behind the original options.
    MoreSettings,
    Cheat,
}

impl PauseView {
    /// The original page that draws this view.
    fn page(self) -> Option<Page> {
        Some(match self {
            PauseView::Buttons => Page::Main,
            PauseView::Settings => Page::Options,
            PauseView::Saves => Page::Saves,
            PauseView::Loads => Page::Loads,
            PauseView::Controls => Page::Controls,
            PauseView::Commands => Page::Commands,
            PauseView::Credits => Page::Credits,
            PauseView::ConfirmQuit => Page::ConfirmQuit,
            PauseView::Cheat => Page::Cheat,
            PauseView::Confirm(_) | PauseView::MoreSettings => return None,
        })
    }

    /// Where Back and Escape lead from here.
    fn back(self) -> PauseView {
        match self {
            PauseView::Commands => PauseView::Controls,
            PauseView::MoreSettings => PauseView::Settings,
            _ => PauseView::Buttons,
        }
    }
}

#[derive(Component, Debug, Clone, Copy)]
struct PauseOverlay;

#[derive(Component, Debug, Clone, Copy)]
struct ResultOverlay;

/// The overlays' systems.
pub fn plugin(app: &mut App) {
    app.init_resource::<MissionEndScreen>()
        .init_resource::<PauseView>()
        // What the original pages are drawn from; the dialog
        // plugin registers the same, and either may come first. Without an
        // install the style settles on the fallback and these stay unused.
        .init_resource::<super::dialog::MenuStyle>()
        .init_resource::<super::dialog::screens::SaveBrowser>()
        .init_resource::<super::dialog::screens::CommandsView>()
        .init_resource::<super::settings::Settings>()
        .init_resource::<crate::savegame::SaveThumbnails>()
        // The save lists the pause menu shows and the saves it asks for;
        // `SaveGamePlugin` registers the same, and either may come first.
        .init_resource::<SaveListing>()
        .add_message::<SaveRequest>()
        .add_systems(OnExit(AppState::InGame), reset_overlays)
        .add_systems(Update, crate::savegame::expire_snapshot)
        .add_systems(
            Update,
            (
                toggle_pause,
                handle_overlay_clicks,
                show_pause,
                note_mission_end,
                show_result,
            )
                .chain()
                .after(theme::dispatch_clicks)
                .run_if(in_state(AppState::InGame)),
        );
}

fn reset_overlays(mut end: ResMut<MissionEndScreen>, mut view: ResMut<PauseView>) {
    *end = MissionEndScreen::default();
    *view = PauseView::Buttons;
}

/// `Pause` opens and closes the menu, or backs out of its settings.
///
/// Every game is local, since there is no multiplayer yet; a networked game
/// should not set [`Paused`] — a client cannot pause the server.
// Eight parameters: the key, the four things that may claim it first, the
// two it sets, and the snapshot it asks for.
#[allow(clippy::too_many_arguments)]
fn toggle_pause(
    mut commands: Commands,
    input: ActionInput,
    rebind: Res<RebindState>,
    end: Res<MissionEndScreen>,
    mut open: ResMut<PauseMenuOpen>,
    mut view: ResMut<PauseView>,
    free_camera: Option<Res<FreeCameraActive>>,
    control: Option<Res<bft_sim::cinematic::PlayerControl>>,
) {
    if end.is_showing() || rebind.is_capturing() || !input.just_pressed(Action::Pause) {
        return;
    }
    // During a cinematic `Pause` skips the intro (`cinematic_view`), and an
    // exit is a few seconds from the result screen.
    if control.is_some_and(|control| control.is_cinematic()) {
        return;
    }
    // In free camera `Pause` means "back to the game", and the dev tools
    // handle it. `is_changed` covers the frame it did so: the flag is already
    // off by the time this runs, and the same press must not open the menu.
    if free_camera.is_some_and(|free| free.0 || free.is_changed()) {
        return;
    }
    if open.0 && *view == PauseView::Cheat {
        open.0 = false;
        *view = PauseView::Buttons;
    } else if open.0 && *view != PauseView::Buttons {
        *view = view.back();
    } else {
        open.0 = !open.0;
        *view = PauseView::Buttons;
        if open.0 {
            // The view without the menu, for the saves made from it.
            crate::savegame::request_snapshot(&mut commands);
        }
    }
}

// The overlays' one junction: every overlay button lands here.
#[allow(clippy::too_many_arguments)]
fn handle_overlay_clicks(
    mut commands: Commands,
    mut clicks: MessageReader<UiClick>,
    mut open: ResMut<PauseMenuOpen>,
    mut view: ResMut<PauseView>,
    state: Option<Res<MissionState>>,
    campaign: Option<Res<Campaign>>,
    mut start: MessageWriter<RequestStartMission>,
    mut next: ResMut<NextState<AppState>>,
    mut listing: ResMut<SaveListing>,
    mut saves: MessageWriter<SaveRequest>,
) {
    for click in clicks.read() {
        match click.0 {
            UiAction::Resume => open.0 = false,
            UiAction::PauseSettings => *view = PauseView::Settings,
            UiAction::Back => *view = view.back(),
            UiAction::Page(page) => {
                let next = match page {
                    Page::Main => PauseView::Buttons,
                    Page::Options => PauseView::Settings,
                    Page::MoreSettings => PauseView::MoreSettings,
                    Page::Controls => PauseView::Controls,
                    Page::Commands => PauseView::Commands,
                    Page::Credits => PauseView::Credits,
                    Page::ConfirmQuit => PauseView::ConfirmQuit,
                    Page::Saves => PauseView::Saves,
                    Page::Loads => PauseView::Loads,
                    Page::Cheat => {
                        open.0 = true;
                        PauseView::Cheat
                    }
                    Page::NewGame => {
                        // The new game panel is the front end's.
                        commands.insert_resource(FrontEndStart(MenuScreen::NewGame));
                        next.set(AppState::FrontEnd);
                        continue;
                    }
                };
                if matches!(next, PauseView::Saves | PauseView::Loads) {
                    listing.refresh();
                }
                *view = next;
            }
            UiAction::OpenSaves => {
                listing.refresh();
                *view = PauseView::Saves;
            }
            UiAction::OpenLoads => {
                listing.refresh();
                *view = PauseView::Loads;
            }
            UiAction::SaveNew => {
                saves.write(SaveRequest::New);
                *view = PauseView::Buttons;
            }
            UiAction::SaveOver(index) => *view = PauseView::Confirm(index),
            UiAction::ConfirmOverwrite => {
                if let PauseView::Confirm(index) = *view
                    && let Some(entry) = listing.entries.get(index as usize)
                {
                    saves.write(SaveRequest::Overwrite(entry.path.clone()));
                }
                *view = PauseView::Buttons;
            }
            UiAction::QuitToMenu => next.set(AppState::FrontEnd),
            UiAction::Retry => {
                if let Some(state) = &state {
                    start.write(RequestStartMission(state.mission));
                }
            }
            UiAction::NextMission => {
                if let Some(next_mission) = state
                    .as_ref()
                    .zip(campaign.as_ref())
                    .and_then(|(state, campaign)| campaign.next_after(state.mission))
                {
                    start.write(RequestStartMission(next_mission.id));
                }
            }
            _ => {}
        }
    }
}

/// Spawn, rebuild or remove the pause overlay to match the state, and hold
/// the sim still while it is up.
#[allow(clippy::too_many_arguments)]
fn show_pause(
    mut commands: Commands,
    open: Res<PauseMenuOpen>,
    view: Res<PauseView>,
    end: Res<MissionEndScreen>,
    mut paused: ResMut<Paused>,
    fonts: Res<UiFonts>,
    paths: Option<Res<DataPaths>>,
    overlays: Query<Entity, With<PauseOverlay>>,
    listing: Res<SaveListing>,
    save: SaveGate,
    globals: Option<Res<Globals>>,
    tables: Res<Assets<Messages>>,
    pages: PageContext,
    snapshot: Option<Res<crate::savegame::SnapshotPending>>,
    mut held: Local<bool>,
) {
    let page = view.page();
    let redraw = open.0 && page.is_some_and(|page| pages.changed(page));
    // Held back while the view behind is captured, then drawn.
    let released = *held && snapshot.is_none();
    if !open.is_changed() && !view.is_changed() && !redraw && !released {
        return;
    }
    *held = false;

    for overlay in &overlays {
        commands.entity(overlay).despawn();
    }
    // The result screen owns `Paused` while it is up.
    if !end.is_showing() {
        paused.set_if_neq(Paused(open.0));
    }
    if !open.0 {
        return;
    }
    if snapshot.is_some() {
        *held = true;
        return;
    }

    // The original's page for this view, when it can be drawn.
    if let Some(page) = page
        && let Some(root) = spawn_page(
            &mut commands,
            &pages,
            page,
            Place::InGame,
            save.refusal().is_some(),
        )
    {
        commands
            .entity(root)
            .insert((PauseOverlay, DespawnOnExit(AppState::InGame)));
        return;
    }

    let root = commands
        .spawn((
            overlay_root(),
            PauseOverlay,
            DespawnOnExit(AppState::InGame),
        ))
        .id();
    match *view {
        PauseView::Buttons => {
            let refusal = save.refusal();
            let can_load = listing.entries.iter().any(|entry| entry.is_loadable());
            let messages = globals::messages(globals.as_deref(), &tables);
            commands.entity(root).with_children(|root| {
                root.spawn(title_text("Paused", TITLE_SIZE, &fonts));
                root.spawn(button("Resume", UiAction::Resume, 300.0));
                let mut save_button = root.spawn(button("Save game", UiAction::OpenSaves, 300.0));
                if refusal.is_some() {
                    save_button.insert(Disabled);
                }
                if let Some(refusal) = refusal {
                    let why = match refusal.message_key() {
                        Some(key) => message_text(messages, key),
                        None => refusal.to_string(),
                    };
                    root.spawn(label_text(why, SMALL_SIZE, TEXT_DIM));
                }
                let mut load_button = root.spawn(button("Load game", UiAction::OpenLoads, 300.0));
                if !can_load {
                    load_button.insert(Disabled);
                }
                root.spawn(button("Settings", UiAction::PauseSettings, 300.0));
                root.spawn(button("Quit to menu", UiAction::QuitToMenu, 300.0));
            });
        }
        PauseView::Saves | PauseView::Loads => {
            let mode = if *view == PauseView::Saves {
                ListMode::Save
            } else {
                ListMode::Load
            };
            let title = match mode {
                ListMode::Save => "Save game",
                ListMode::Load => "Load game",
            };
            let messages = globals::messages(globals.as_deref(), &tables);
            commands.entity(root).with_children(|root| {
                root.spawn(title_text(title, TITLE_SIZE, &fonts));
                spawn_save_list(root, &listing, mode, messages);
                root.spawn(button("Back", UiAction::Back, 520.0));
            });
        }
        PauseView::Confirm(index) => {
            let messages = globals::messages(globals.as_deref(), &tables);
            let which = listing
                .entries
                .get(index as usize)
                .map(|entry| crate::savegame::describe(entry, messages))
                .unwrap_or_default();
            commands.entity(root).with_children(|root| {
                root.spawn(title_text(
                    message_text(messages, "MSG_SAVE_OVERWRITE_HDR"),
                    TITLE_SIZE,
                    &fonts,
                ));
                root.spawn(label_text(
                    message_text(messages, "MSG_SAVE_OVERWRITE_MSG"),
                    BODY_SIZE,
                    TEXT,
                ));
                root.spawn(label_text(which, SMALL_SIZE, TEXT_DIM));
                root.spawn(button("Overwrite", UiAction::ConfirmOverwrite, 300.0));
                root.spawn(button("Cancel", UiAction::OpenSaves, 300.0));
            });
        }
        PauseView::Settings | PauseView::MoreSettings => {
            commands.entity(root).with_children(|root| {
                root.spawn(title_text("Settings", TITLE_SIZE, &fonts));
            });
            spawn_settings_panel(&mut commands, root, paths.as_deref());
        }
        // Only the original menus open these; without them, back to the
        // buttons.
        PauseView::Controls
        | PauseView::Commands
        | PauseView::Credits
        | PauseView::ConfirmQuit
        | PauseView::Cheat => {
            commands.entity(root).despawn();
            commands.queue(|world: &mut World| {
                world
                    .resource_mut::<PauseView>()
                    .set_if_neq(PauseView::Buttons);
            });
        }
    }
}

/// What decides whether the pause menu's Save button works.
#[derive(bevy::ecs::system::SystemParam)]
struct SaveGate<'w> {
    mode: Option<Res<'w, State<GameMode>>>,
    state: Option<Res<'w, MissionState>>,
    control: Option<Res<'w, PlayerControl>>,
    script: Option<Res<'w, MissionScript>>,
}

impl SaveGate<'_> {
    fn refusal(&self) -> Option<SaveRefusal> {
        crate::savegame::save_refusal_now(
            self.mode.as_deref(),
            self.state.as_deref(),
            self.control.as_deref(),
            self.script.as_deref(),
        )
    }
}

/// A full-window dimmer with its content centred.
fn overlay_root() -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            row_gap: px(12),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
        // Over the HUD.
        GlobalZIndex(10),
    )
}

/// Record the end of the mission.
fn note_mission_end(
    time: Res<Time<Real>>,
    mut ended: MessageReader<MissionEnded>,
    mut end: ResMut<MissionEndScreen>,
) {
    for event in ended.read() {
        if end.outcome.is_none() {
            end.outcome = Some(event.0);
            end.since = time.elapsed_secs();
        }
    }
}

/// Whether the death cinematic has finished: no instance of the mission's
/// death animation is still running.
pub fn death_cinematic_over(
    script: Option<&MissionScript>,
    program: Option<&AnimProgram>,
    instances: impl IntoIterator<Item = AnimInstance>,
) -> bool {
    let (Some(script), Some(program)) = (script, program) else {
        return true;
    };
    !instances.into_iter().any(|instance| {
        instance.is_running()
            && program
                .0
                .defs
                .get(instance.def)
                .is_some_and(|def| def.anim_name == script.death_anim)
    })
}

/// Put the result screen up when it is time.
// The screen needs the outcome, the death animation's progress, and the
// strings and fonts to draw with.
#[allow(clippy::too_many_arguments)]
fn show_result(
    mut commands: Commands,
    time: Res<Time<Real>>,
    mut end: ResMut<MissionEndScreen>,
    mut open: ResMut<PauseMenuOpen>,
    mut paused: ResMut<Paused>,
    state: Option<Res<MissionState>>,
    script: Option<Res<MissionScript>>,
    program: Option<Res<AnimProgram>>,
    instances: Query<&AnimInstance>,
    campaign: Option<Res<Campaign>>,
    globals: Option<Res<Globals>>,
    tables: Res<Assets<Messages>>,
    fonts: Res<UiFonts>,
) {
    let Some(outcome) = end.outcome else {
        return;
    };
    if end.shown {
        return;
    }
    if outcome == Outcome::Lost
        && time.elapsed_secs() - end.since < DEATH_SCREEN_TIMEOUT
        && !death_cinematic_over(
            script.as_deref(),
            program.as_deref(),
            instances.iter().cloned(),
        )
    {
        return;
    }

    end.shown = true;
    open.0 = false;
    paused.set_if_neq(Paused(true));

    let messages = globals::messages(globals.as_deref(), &tables);
    let next = state
        .as_ref()
        .zip(campaign.as_ref())
        .and_then(|(state, campaign)| campaign.next_after(state.mission));
    let final_mission = script.as_ref().is_some_and(|script| script.final_mission);

    let (heading, colour, detail) = match outcome {
        Outcome::Won => (
            "Mission complete".to_string(),
            GREEN,
            if next.is_none() || final_mission {
                message_text(messages, "MSG_CAMPAIGN_DONE")
            } else {
                String::new()
            },
        ),
        Outcome::Lost => (
            message_text(messages, "MSG_GAME_OVER"),
            ERROR,
            String::new(),
        ),
    };
    let summary = result_summary(state.as_deref(), script.as_deref());

    commands
        .spawn((
            overlay_root(),
            ResultOverlay,
            DespawnOnExit(AppState::InGame),
        ))
        .with_children(|root| {
            root.spawn(title_text(heading, TITLE_SIZE, &fonts))
                .insert(TextColor(colour));
            if !detail.is_empty() {
                root.spawn(label_text(detail, HEADING_SIZE, TEXT));
            }
            root.spawn(label_text(summary, BODY_SIZE, TEXT_DIM));
            root.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: px(10),
                margin: UiRect::top(px(16)),
                ..default()
            })
            .with_children(|buttons| {
                if outcome == Outcome::Won && next.is_some() && !final_mission {
                    buttons.spawn(button("Next mission", UiAction::NextMission, 200.0));
                }
                buttons.spawn(button("Retry", UiAction::Retry, 160.0));
                buttons.spawn(button("Menu", UiAction::QuitToMenu, 160.0));
            });
        });
}

/// "Objectives completed: 3 of 4", the first half of the original's
/// `MSG_REVIEW_LINE1` (the accuracy half needs shot counting nobody does
/// yet).
pub fn result_summary(state: Option<&MissionState>, script: Option<&MissionScript>) -> String {
    let (Some(state), Some(_)) = (state, script) else {
        return String::new();
    };
    let done = state
        .objectives
        .iter()
        .filter(|objective| **objective == bft_sim::mission::ObjectiveState::Complete)
        .count();
    format!("Objectives completed: {done} of {}", state.objectives.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::catalog::LevelId;
    use bft_sim::mission::ObjectiveState;
    use bft_sim::states::GameMode;

    fn script() -> MissionScript {
        MissionScript {
            objectives: Vec::new(),
            read_time: 8.0,
            review_delay: 1.0,
            review_sound: String::new(),
            objective_sound: String::new(),
            new_game_anim: String::new(),
            load_game_anim: String::new(),
            movers: Vec::new(),
            death_anim: "bft_00".into(),
            final_mission: false,
        }
    }

    #[test]
    fn with_no_death_animation_the_cinematic_is_already_over() {
        assert!(death_cinematic_over(Some(&script()), None, Vec::new()));
        assert!(death_cinematic_over(None, None, Vec::new()));
    }

    #[test]
    fn the_summary_counts_completed_objectives() {
        let mut state = MissionState::new(LevelId(1), &script());
        state.objectives = vec![
            ObjectiveState::Complete,
            ObjectiveState::Active,
            ObjectiveState::Complete,
        ];

        assert_eq!(
            result_summary(Some(&state), Some(&script())),
            "Objectives completed: 2 of 3"
        );
    }

    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::MinimalPlugins, bevy::state::app::StatesPlugin));
        app.init_state::<AppState>()
            .add_sub_state::<GameMode>()
            .init_resource::<Paused>()
            .init_resource::<PauseMenuOpen>()
            .init_resource::<RebindState>()
            .init_resource::<UiFonts>()
            .init_resource::<Assets<Messages>>()
            .init_resource::<ButtonInput<KeyCode>>()
            .insert_resource(super::super::InputBindings::default())
            .add_message::<UiClick>()
            .add_message::<MissionEnded>()
            .add_message::<RequestStartMission>()
            .add_message::<bevy::picking::events::Pointer<bevy::picking::events::Click>>()
            .init_resource::<theme::ClickGuard>()
            .add_systems(Update, theme::dispatch_clicks);
        plugin(&mut app);
        app.insert_state(AppState::InGame);
        app.update();
        app
    }

    /// "Pause → resume": Escape pauses the sim,
    /// Resume unpauses it.
    #[test]
    fn pause_stops_the_sim_and_resume_starts_it() {
        let mut app = app();

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        assert!(app.world().resource::<PauseMenuOpen>().0);
        assert!(app.world().resource::<Paused>().0);

        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(KeyCode::Escape);
            keys.clear();
        }
        app.world_mut().write_message(UiClick(UiAction::Resume));
        app.update();
        assert!(!app.world().resource::<PauseMenuOpen>().0);
        assert!(!app.world().resource::<Paused>().0);
    }

    /// A win shows the result screen at once and Retry restarts the
    /// same mission.
    #[test]
    fn a_win_shows_the_result_and_retry_restarts_the_mission() {
        let mut app = app();
        app.insert_resource(MissionState::new(LevelId(2), &script()));

        app.world_mut().write_message(MissionEnded(Outcome::Won));
        app.update();
        assert!(app.world().resource::<MissionEndScreen>().is_showing());
        assert!(app.world().resource::<Paused>().0);

        app.world_mut().write_message(UiClick(UiAction::Retry));
        app.update();
        let requests: Vec<RequestStartMission> = app
            .world_mut()
            .resource_mut::<bevy::ecs::message::Messages<RequestStartMission>>()
            .drain()
            .collect();
        assert_eq!(requests, vec![RequestStartMission(LevelId(2))]);
    }

    /// The menu waits for the picture of the game behind it, but not for
    /// good: with no renderer to take one, it still comes up.
    #[test]
    fn the_pause_menu_is_drawn_even_when_no_snapshot_comes() {
        let mut app = app();
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();
        assert!(app.world().resource::<Paused>().0, "paused at once");
        {
            let mut keys = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
            keys.release(KeyCode::Escape);
            keys.clear();
        }

        for _ in 0..15 {
            app.update();
        }
        let overlays = app
            .world_mut()
            .query::<&PauseOverlay>()
            .iter(app.world())
            .count();
        assert_eq!(overlays, 1);
    }
}
