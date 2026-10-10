//! [`AppState::Loading`]: hand the pending level to the loader and wait.
//!
//! The loading itself is the level loader's
//! ([`level_view::start_level_load`](crate::level_view::start_level_load) and
//! [`poll_level_load`](crate::level_view::poll_level_load)); this state only
//! triggers it, shows a screen while it runs, and decides where to go when it
//! finishes. The division matters because the loader is also what a
//! "restart the mission" path can use, without a state change.
//!
//! **Failure is detected by absence.** The level loader drops the
//! [`LoadedLevel`] resource when a level's `gamez` fails, so it does not
//! retry every frame. That means this state sees "the request went in and the
//! resource is gone", which is the failure signal, and returns to the front
//! end with the reason on screen.

use bevy::prelude::*;
use bft_assets::catalog::LevelCatalog;
use bft_sim::states::{AppState, PendingLevel};

use crate::level_view::{LoadLevel, LoadedLevel};

/// How long a level may take before the state gives up, in seconds.
///
/// NFR-10 budgets 5 s for a cached level; this is the "something is wrong"
/// limit, not the target, and it exists so a level whose assets never settle
/// returns to a menu instead of holding a blank screen for good.
const LOAD_TIMEOUT: f32 = 60.0;

/// The loading state.
pub struct LoadingPlugin;

impl Plugin for LoadingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoadFailure>()
            .add_systems(
                OnEnter(AppState::Loading),
                (request_level, spawn_loading_screen).chain(),
            )
            .add_systems(OnExit(AppState::Loading), |mut commands: Commands| {
                commands.remove_resource::<BriefingHold>();
            })
            .add_systems(
                Update,
                finish_loading
                    .run_if(in_state(AppState::Loading))
                    // After the level loader's systems, so a level that finished
                    // loading this frame is seen this frame rather than next.
                    .after(crate::level_view::poll_level_load),
            );
    }
}

/// Why the last load failed, for the front end to show. Empty when it did not.
#[derive(Resource, Debug, Clone, Default)]
pub struct LoadFailure(pub Option<String>);

/// How long the current load has been running.
#[derive(Resource, Debug, Clone, Copy)]
struct LoadingSince(f32);

/// Hold the level at the briefing until the player proceeds (the original's
/// transport screen says "PROCEED" once the level is in and waits). Inserted by whatever shows the briefing; while it is held a
/// loaded level does not start and does not time out.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BriefingHold {
    pub released: bool,
}

/// The fallback loading screen, which the original briefing replaces.
#[derive(Component, Debug, Clone, Copy)]
pub struct LoadingScreen;

/// How far a level's load has come, 0–1: its assets settled, one way or
/// the other, and then its spawn.
pub fn level_progress(loaded: &LoadedLevel, server: &AssetServer) -> f32 {
    use bevy::asset::LoadState;
    let mut ids = vec![loaded.data.id().untyped()];
    ids.extend(loaded.textures.iter().map(|handle| handle.id().untyped()));
    ids.extend(loaded.zmap.as_ref().map(|handle| handle.id().untyped()));
    ids.extend(loaded.interp.as_ref().map(|handle| handle.id().untyped()));
    ids.extend(loaded.reader.as_ref().map(|handle| handle.id().untyped()));
    ids.extend(loaded.anim.as_ref().map(|handle| handle.id().untyped()));
    let settled = ids
        .iter()
        .filter(|id| {
            matches!(
                server.load_state(**id),
                LoadState::Loaded | LoadState::Failed(_)
            )
        })
        .count();
    let steps = ids.len() + 1;
    (settled + usize::from(loaded.is_spawned())) as f32 / steps as f32
}

/// Turn the [`PendingLevel`] into the level loader's [`LoadLevel`] request.
fn request_level(
    mut commands: Commands,
    pending: Option<Res<PendingLevel>>,
    catalog: Res<LevelCatalog>,
    time: Res<Time>,
    mut failure: ResMut<LoadFailure>,
    mut next: ResMut<NextState<AppState>>,
) {
    failure.0 = None;

    let Some(pending) = pending else {
        // Nothing asked for a level: a bug in whoever set the state, but the
        // user gets a menu rather than a dead screen.
        error!("entered Loading with no PendingLevel");
        next.set(AppState::FrontEnd);
        return;
    };

    if catalog.level(pending.id).is_none() {
        failure.0 = Some(format!(
            "level {} is not in the extraction cache",
            pending.id
        ));
        error!("{}", failure.0.as_deref().unwrap_or_default());
        next.set(AppState::FrontEnd);
        return;
    }

    commands.insert_resource(LoadLevel(pending.id));
    commands.insert_resource(LoadingSince(time.elapsed_secs()));
}

/// Enter the target state once the level is spawned, or bail out.
// Nine parameters because this is where every input to "is the level ready,
// and if not why not" meets: the request, the result, the clock, the two
// things it writes and the briefing's hold. Splitting it would hide the
// ordering that matters.
#[allow(clippy::too_many_arguments)]
fn finish_loading(
    mut commands: Commands,
    pending: Option<Res<PendingLevel>>,
    loaded: Option<Res<LoadedLevel>>,
    request: Option<Res<LoadLevel>>,
    since: Option<Res<LoadingSince>>,
    time: Res<Time>,
    mut failure: ResMut<LoadFailure>,
    mut next: ResMut<NextState<AppState>>,
    hold: Option<Res<BriefingHold>>,
) {
    let Some(pending) = pending else {
        return;
    };

    match loaded {
        // Ready, and the briefing is still up: the player proceeds.
        Some(loaded) if loaded.is_spawned() && hold.is_some_and(|hold| !hold.released) => {}
        Some(loaded) if loaded.is_spawned() => {
            commands.remove_resource::<BriefingHold>();
            let elapsed = since.map(|since| time.elapsed_secs() - since.0);
            match elapsed {
                // NFR-10's budget, reported rather than asserted: the number
                // is the thing a slow machine or a cold cache shows up in.
                Some(elapsed) => info!("level {} ready in {elapsed:.2} s", loaded.id),
                None => info!("level {} ready", loaded.id),
            }
            commands.remove_resource::<LoadingSince>();
            next.set(pending.target_state());
        }
        // Still loading.
        Some(_) => check_timeout(&mut commands, since, &time, &mut failure, &mut next),
        None => {
            // The request has not been picked up yet — `start_level_load`
            // runs in `Update` and may not have had a turn — so an absent
            // `LoadedLevel` is only a failure once the request is gone too.
            if request.is_some() {
                return;
            }
            failure.0 = Some(format!(
                "level {} failed to load; see the log for the reason",
                pending.id
            ));
            error!("{}", failure.0.as_deref().unwrap_or_default());
            commands.remove_resource::<LoadingSince>();
            next.set(AppState::FrontEnd);
        }
    }
}

/// Give up on a load that is never going to settle.
fn check_timeout(
    commands: &mut Commands,
    since: Option<Res<LoadingSince>>,
    time: &Time,
    failure: &mut LoadFailure,
    next: &mut NextState<AppState>,
) {
    let Some(since) = since else {
        return;
    };
    if time.elapsed_secs() - since.0 < LOAD_TIMEOUT {
        return;
    }

    failure.0 = Some(format!(
        "loading timed out after {LOAD_TIMEOUT:.0} s; the cache may be incomplete"
    ));
    error!("{}", failure.0.as_deref().unwrap_or_default());
    commands.remove_resource::<LoadingSince>();
    commands.remove_resource::<LoadLevel>();
    next.set(AppState::FrontEnd);
}

/// The loading screen: the level's title from the
/// string table, and the game's own "please stand by" line.
fn spawn_loading_screen(
    mut commands: Commands,
    pending: Option<Res<PendingLevel>>,
    catalog: Res<LevelCatalog>,
    globals: Option<Res<crate::globals::Globals>>,
    tables: Option<Res<Assets<bft_assets::messages::Messages>>>,
    fonts: Option<Res<crate::ui::theme::UiFonts>>,
) {
    use crate::ui::theme::{BACKGROUND, BODY_SIZE, TEXT_DIM, TITLE_SIZE, message_text, title_text};

    let messages = tables
        .as_deref()
        .and_then(|tables| crate::globals::messages(globals.as_deref(), tables));
    let title = pending
        .and_then(|pending| catalog.level(pending.id))
        .map(|entry| crate::ui::menus::level_title(entry, messages))
        .unwrap_or_default();
    // "Loading Next Mission... Please Stand By..." — the original's line; the
    // key itself when there is no table, which reads badly, so plain words.
    let line = messages
        .map(|table| message_text(Some(table), "MSG_LOAD_NEXT_MISSION"))
        .unwrap_or_else(|| "Loading...".to_string());
    let fonts = fonts.map(|fonts| fonts.clone()).unwrap_or_default();

    commands
        .spawn((
            Node {
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                row_gap: px(12),
                ..default()
            },
            BackgroundColor(BACKGROUND),
            DespawnOnExit(AppState::Loading),
            LoadingScreen,
        ))
        .with_children(|root| {
            root.spawn(title_text(title, TITLE_SIZE, &fonts));
            root.spawn((
                Text::new(line),
                TextFont::from_font_size(BODY_SIZE),
                TextColor(TEXT_DIM),
            ));
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::catalog::{LevelEntry, LevelId, LevelKind};

    fn app_with_catalog(levels: Vec<LevelEntry>) -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::MinimalPlugins,
            bevy::state::app::StatesPlugin,
            bevy::log::LogPlugin::default(),
        ));
        app.init_state::<AppState>();
        app.init_resource::<LoadFailure>();
        app.insert_resource(LevelCatalog {
            levels,
            ..default()
        });
        // The real schedule, not a convenient one: `request_level` runs once
        // on entering the state and `finish_loading` every frame while in it.
        // Collapsing the two into `Update` would let the request re-run each
        // frame and paper over exactly the ordering this state has to get
        // right.
        app.add_systems(OnEnter(AppState::Loading), request_level);
        app.add_systems(Update, finish_loading.run_if(in_state(AppState::Loading)));
        app.insert_state(AppState::Loading);
        app
    }

    fn m1() -> LevelEntry {
        LevelEntry {
            id: LevelId(1),
            name: "m1".to_string(),
            title_key: "MSG_CAMPAIGN1_NAME".to_string(),
            gamez: "cache://gamez/m1-gamez.gamez.zip".into(),
            anim: None,
            reader: "cache://reader/m1-zrdr.rdr.zip".into(),
            zmap: None,
            zmap_is_placeholder: false,
            textures: Vec::new(),
            kind: LevelKind::Campaign { campaign: 1 },
        }
    }

    #[test]
    fn a_level_the_cache_does_not_have_goes_back_to_the_menu_with_a_reason() {
        let mut app = app_with_catalog(Vec::new());
        app.insert_resource(PendingLevel::viewer(LevelId(1)));

        app.update();
        app.update();

        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::FrontEnd
        );
        let failure = app.world().resource::<LoadFailure>();
        assert!(
            failure
                .0
                .as_deref()
                .unwrap_or_default()
                .contains("not in the extraction cache"),
            "got {:?}",
            failure.0
        );
    }

    /// The level loader drops `LoadedLevel` when a `gamez` fails, so "the request
    /// was taken and nothing came back" is the only failure signal there is.
    #[test]
    fn a_dropped_loaded_level_is_read_as_a_failure() {
        let mut app = app_with_catalog(vec![m1()]);
        app.insert_resource(PendingLevel::viewer(LevelId(1)));

        // First update requests the level.
        app.update();
        assert!(app.world().get_resource::<LoadLevel>().is_some());

        // Stand in for the level loader: take the request, fail, and leave
        // no `LoadedLevel` behind.
        app.world_mut().remove_resource::<LoadLevel>();
        app.update();
        // A second update, because `NextState` is applied in `StateTransition`,
        // which runs *before* `Update`: the system that asked for the change
        // cannot see it land in the same frame.
        app.update();

        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::FrontEnd
        );
        assert!(app.world().resource::<LoadFailure>().0.is_some());
    }

    #[test]
    fn a_spawned_level_enters_the_target_state() {
        let mut app = app_with_catalog(vec![m1()]);
        app.insert_resource(PendingLevel::viewer(LevelId(1)));

        app.update();

        // Stand in for a successful load.
        app.world_mut().remove_resource::<LoadLevel>();
        let root = app.world_mut().spawn_empty().id();
        app.insert_resource(LoadedLevel {
            root: Some(root),
            ..LoadedLevel::loading(
                LevelId(1),
                Handle::default(),
                Vec::new(),
                None,
                None,
                None,
                None,
            )
        });
        app.update();
        app.update();

        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::Viewer
        );
        assert!(app.world().resource::<LoadFailure>().0.is_none());
    }

    /// `PendingLevel::in_game` has to reach `InGame`, not the viewer, or
    /// the player's vehicle has nowhere to land.
    #[test]
    fn the_target_is_honoured_rather_than_always_the_viewer() {
        let mut app = app_with_catalog(vec![m1()]);
        app.insert_resource(PendingLevel::in_game(LevelId(1)));

        app.update();
        app.world_mut().remove_resource::<LoadLevel>();
        let root = app.world_mut().spawn_empty().id();
        app.insert_resource(LoadedLevel {
            root: Some(root),
            ..LoadedLevel::loading(
                LevelId(1),
                Handle::default(),
                Vec::new(),
                None,
                None,
                None,
                None,
            )
        });
        app.update();
        app.update();

        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::InGame
        );
    }

    /// With the briefing up, a ready level waits for the player
    /// to proceed, and does not time out while it waits.
    #[test]
    fn a_held_briefing_keeps_a_ready_level_until_released() {
        let mut app = app_with_catalog(vec![m1()]);
        app.insert_resource(PendingLevel::in_game(LevelId(1)));
        app.insert_resource(BriefingHold::default());

        app.update();
        app.world_mut().remove_resource::<LoadLevel>();
        let root = app.world_mut().spawn_empty().id();
        app.insert_resource(LoadedLevel {
            root: Some(root),
            ..LoadedLevel::loading(
                LevelId(1),
                Handle::default(),
                Vec::new(),
                None,
                None,
                None,
                None,
            )
        });
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::Loading
        );

        app.world_mut().resource_mut::<BriefingHold>().released = true;
        app.update();
        app.update();
        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::InGame
        );
        assert!(app.world().get_resource::<BriefingHold>().is_none());
    }
}
