//! [`AppState::Boot`]: find the data, build the cache, load the globals.
//!
//! The binary has already resolved the paths ([`DataPaths`]) before the `App`
//! was built, because Bevy needs the cache directory to register the
//! `cache://` asset source. What is left for boot is the part that can take
//! time or fail visibly: checking the cache, running `unzbd` over whatever is
//! missing, and rebuilding the catalog from the result.
//!
//! Extraction runs on [`AsyncComputeTaskPool`], not the main schedule: a
//! first run is ~76 `unzbd` invocations over a 450 MB install and the window
//! must keep drawing its progress (NFR-10).

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, block_on, futures_lite::future};
use bft_assets::catalog::LevelCatalog;
use bft_assets::{CacheDir, CacheManifest, DataPaths, ExtractProgress, cache, extract};
use bft_sim::states::{AppState, PendingLevel};

use crate::globals::{self, Globals};

/// How far boot has got. Shown on the boot screen and printed.
#[derive(Resource, Debug, Clone)]
pub struct BootStatus {
    /// Which step is running.
    pub phase: BootPhase,
    /// A line for the user: what is happening, or what went wrong.
    pub message: String,
    /// `0.0`–`1.0` when the step knows how much is left, else `None`.
    pub progress: Option<f32>,
}

impl Default for BootStatus {
    fn default() -> BootStatus {
        BootStatus {
            phase: BootPhase::FindingData,
            message: "Looking for a Recoil install...".to_string(),
            progress: None,
        }
    }
}

impl BootStatus {
    /// Move to a phase with a message and no progress fraction.
    pub fn set(&mut self, phase: BootPhase, message: impl Into<String>) {
        self.phase = phase;
        self.message = message.into();
        self.progress = None;
    }

    /// Whether boot has stopped, one way or the other.
    pub fn is_finished(&self) -> bool {
        matches!(self.phase, BootPhase::Done | BootPhase::Failed)
    }
}

/// The steps boot goes through, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootPhase {
    /// Locating the install (FR-D1).
    FindingData,
    /// Checking what the cache already holds against the install.
    Validating,
    /// Running `unzbd` over what is missing or stale (FR-D3).
    Extracting,
    /// Reading the globals every level needs.
    LoadingGlobals,
    /// Ready; the next state has been chosen.
    Done,
    /// Boot cannot continue. [`BootStatus::message`] says why, and the
    /// application stays here rather than going on to an empty front end.
    Failed,
}

/// `--unlock-all` (FR-M7): open every mission and form for this run.
///
/// A resource rather than a field on the plugin because `bft-bin` parses the
/// command line before the `App` exists and inserts it alongside the other
/// CLI-derived resources.
#[derive(Resource, Debug, Clone, Copy, Default)]
pub struct UnlockAll(pub bool);

/// Boot: the status resource, the screen, and the steps.
pub struct BootPlugin;

impl Plugin for BootPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<BootStatus>()
            .add_systems(OnEnter(AppState::Boot), (spawn_boot_screen, start_boot))
            .add_systems(
                Update,
                (poll_extraction, update_boot_screen, leave_boot)
                    .chain()
                    .run_if(in_state(AppState::Boot)),
            );
    }
}

/// The extraction running on the task pool, if one is.
///
/// The counter is shared with the task so the main schedule can read progress
/// without waiting on it; the task owns the `unzbd` child processes.
#[derive(Resource)]
struct ExtractTask {
    task: Task<Result<CacheManifest, String>>,
    done: Arc<AtomicUsize>,
    total: usize,
    cache: CacheDir,
}

/// Decide what boot has to do, and start it.
///
/// The data root is already resolved — the binary could not have built the
/// asset source otherwise — so the failure this handles is the *cache* being
/// unusable and unbuildable, which is where a user without `unzbd` lands
/// (FR-D3).
fn start_boot(
    mut commands: Commands,
    mut status: ResMut<BootStatus>,
    paths: Option<Res<DataPaths>>,
) {
    let Some(paths) = paths else {
        // No `DataPaths` means `AssetsPlugin` was never added, which no
        // supported entry point does. Failing here beats a front end whose
        // every level is missing.
        fail(
            &mut status,
            "no Recoil install was found.\n\nPoint the engine at one with \
             --data-dir <path>, the BFT_DATA_DIR environment variable, or the \
             data_root key in the config file.",
        );
        return;
    };

    status.set(
        BootPhase::Validating,
        format!("Checking the cache at {}...", paths.cache.path.display()),
    );

    let manifest = match cache::load_manifest(&paths.cache) {
        Ok(manifest) => manifest,
        Err(e) => {
            // A corrupt manifest is recoverable: plan against none and
            // re-extract everything.
            warn!("cannot read the cache manifest ({e}); re-extracting");
            None
        }
    };

    let plan = match extract::plan(&paths.root, &paths.cache, manifest.as_ref()) {
        Ok(plan) => plan,
        Err(e) => {
            fail(&mut status, format!("cannot read the install: {e}"));
            return;
        }
    };

    let stale = plan.stale_count();
    if stale == 0 {
        status.set(BootPhase::LoadingGlobals, "Loading game data...");
        return;
    }

    let Some(unzbd) = paths.unzbd.clone() else {
        fail(
            &mut status,
            format!(
                "{stale} of {} archives are not in the cache, and no `unzbd` \
                 was found to extract them.\n\nPass --unzbd <file>, set \
                 BFT_UNZBD, put it on PATH, or run `open-bft extract` to see \
                 the commands to run by hand.",
                plan.jobs.len()
            ),
        );
        return;
    };

    status.phase = BootPhase::Extracting;
    status.message = format!("Extracting {stale} archive(s) with unzbd...");
    status.progress = Some(0.0);

    // Owned copies: the task outlives this system and cannot borrow the world.
    let done = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&done);
    let cache = paths.cache.clone();
    let task_cache = cache.clone();

    let task = AsyncComputeTaskPool::get().spawn(async move {
        extract::run(&plan, &unzbd, &task_cache, move |progress| {
            // `Started` rather than `Finished`, so the count names the job
            // being worked on; `Skipped` too, since an up-to-date job is a
            // step the user does not wait for.
            if matches!(
                progress,
                ExtractProgress::Finished { .. } | ExtractProgress::Skipped { .. }
            ) {
                counter.fetch_add(1, Ordering::Relaxed);
            }
            if let ExtractProgress::Failed { reason, .. } = &progress {
                // Not fatal: FR-D3 says one broken archive must not block the
                // rest, and the catalog simply omits what did not extract.
                warn!("extraction job failed: {reason}");
            }
        })
        .map_err(|e| e.to_string())
    });

    commands.insert_resource(ExtractTask {
        task,
        done,
        total: stale,
        cache,
    });
}

/// Watch the extraction task and rebuild the catalog when it finishes.
fn poll_extraction(
    mut commands: Commands,
    mut status: ResMut<BootStatus>,
    task: Option<ResMut<ExtractTask>>,
    mut catalog: ResMut<LevelCatalog>,
) {
    let Some(mut task) = task else {
        return;
    };

    let done = task.done.load(Ordering::Relaxed);
    status.progress = Some(done as f32 / task.total.max(1) as f32);
    status.message = format!("Extracting... {done}/{}", task.total);

    let Some(result) = block_on(future::poll_once(&mut task.task)) else {
        return;
    };

    match result {
        Ok(manifest) => {
            info!(
                "extraction finished: {} entries by {}",
                manifest.entries.len(),
                manifest.mech3ax_version
            );
            *catalog = LevelCatalog::from_manifest(&manifest, &task.cache);
            status.set(BootPhase::LoadingGlobals, "Loading game data...");
        }
        Err(e) => {
            // Some archives may still have landed, so read back whatever the
            // manifest records rather than throwing the run away.
            warn!("extraction reported errors: {e}");
            if let Ok(Some(manifest)) = cache::load_manifest(&task.cache) {
                *catalog = LevelCatalog::from_manifest(&manifest, &task.cache);
            }

            if catalog.levels.is_empty() {
                fail(
                    &mut status,
                    format!("extraction failed and no level is playable:\n\n{e}"),
                );
            } else {
                warn!(
                    "continuing with the {} level(s) that extracted",
                    catalog.levels.len()
                );
                status.set(BootPhase::LoadingGlobals, "Loading game data...");
            }
        }
    }

    commands.remove_resource::<ExtractTask>();
}

/// Finish boot: check the catalog has something in it, then choose the next
/// state.
///
/// "Loading globals" waits on the message table, the global reader and the
/// sound bank; the catalog itself is built by `AssetsPlugin::finish` or by
/// [`poll_extraction`].
// Eight parameters because leaving boot is where every precondition for
// playing meets: the phase, the catalog, the globals it waits on, the level
// the command line asked for, and the two flags that decide what to load.
#[allow(clippy::too_many_arguments)]
fn leave_boot(
    mut commands: Commands,
    mut status: ResMut<BootStatus>,
    catalog: Res<LevelCatalog>,
    pending: Option<Res<PendingLevel>>,
    globals: Option<Res<Globals>>,
    server: Option<Res<AssetServer>>,
    unlock_all: Option<Res<UnlockAll>>,
    mut next: ResMut<NextState<AppState>>,
) {
    if status.phase != BootPhase::LoadingGlobals {
        return;
    }

    if catalog.levels.is_empty() {
        fail(
            &mut status,
            "the extraction cache holds no levels.\n\nRun `open-bft extract` \
             to build it from a Recoil install.",
        );
        return;
    }

    // The globals — the string table and the global reader — are asked for
    // once and then waited on. A test app with no `AssetServer` skips
    // straight through, which is what lets the phase tests below stay about
    // the phase machine.
    if let Some(server) = server.as_deref() {
        let Some(globals) = globals else {
            status.set(BootPhase::LoadingGlobals, "Loading the game's data...");
            globals::start_loading_globals(&mut commands, &catalog, server);
            commands.insert_resource(globals::load_progress(
                unlock_all.is_some_and(|flag| flag.0),
            ));
            return;
        };

        if !globals.is_settled(server) {
            return;
        }
    }

    status.set(
        BootPhase::Done,
        format!("{} level(s) ready.", catalog.levels.len()),
    );

    // A level asked for on the command line skips the front end.
    match pending {
        Some(pending) => {
            info!("boot done; loading level {}", pending.id);
            next.set(AppState::Loading);
        }
        None => {
            info!(
                "boot done; {} level(s) in the catalog",
                catalog.levels.len()
            );
            next.set(AppState::FrontEnd);
        }
    }
}

/// Record a boot failure: on screen, and on stderr for a user who ran from a
/// terminal (FR-D1 asks for both).
fn fail(status: &mut BootStatus, message: impl Into<String>) {
    let message = message.into();
    error!("{message}");
    status.phase = BootPhase::Failed;
    status.message = message;
    status.progress = None;
}

/// Marks the boot screen's text, so [`update_boot_screen`] can find it.
#[derive(Component)]
struct BootMessage;

/// Marks the progress bar's fill.
#[derive(Component)]
struct BootProgress;

/// The boot screen, in the menus' style: the title, the
/// status line, and a progress bar that shows while `unzbd` runs.
///
/// With the game's splash it is the original's screen: the
/// RECOIL wallpaper, with the status and bar small along the bottom.
fn spawn_boot_screen(
    mut commands: Commands,
    status: Res<BootStatus>,
    fonts: Option<Res<crate::ui::theme::UiFonts>>,
    splash: Option<Res<crate::ui::dialog::Splash>>,
) {
    use crate::ui::theme::{ACCENT, BACKGROUND, BODY_SIZE, TEXT, TEXT_DIM, title_text};

    let fonts = fonts.map(|fonts| fonts.clone()).unwrap_or_default();
    let with_splash = splash.is_some();

    let root = commands
        .spawn((
            Node {
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                justify_content: if with_splash {
                    JustifyContent::FlexEnd
                } else {
                    JustifyContent::Center
                },
                align_items: AlignItems::Center,
                row_gap: px(16),
                padding: UiRect::all(px(48)),
                ..default()
            },
            BackgroundColor(if with_splash {
                Color::BLACK
            } else {
                BACKGROUND
            }),
            DespawnOnExit(AppState::Boot),
        ))
        .id();
    if let Some(splash) = &splash {
        crate::ui::dialog::render::spawn_fitted_picture(
            &mut commands,
            root,
            splash.0.clone(),
            false,
        );
    }
    commands.entity(root).with_children(|root| {
        if !with_splash {
            root.spawn(title_text("OPEN-BFT", 56.0, &fonts));
        }
        root.spawn((
            Text::new(status.message.clone()),
            TextFont::from_font_size(BODY_SIZE),
            TextColor(TEXT),
            TextLayout::justify(Justify::Center),
            Node {
                max_width: px(760),
                padding: UiRect::axes(px(12), px(6)),
                ..default()
            },
            // Readable over the wallpaper.
            BackgroundColor(if with_splash {
                Color::srgba(0.0, 0.0, 0.0, 0.6)
            } else {
                Color::NONE
            }),
            BootMessage,
        ));
        root.spawn((
            Node {
                width: px(420),
                height: px(6),
                border: UiRect::all(px(1)),
                ..default()
            },
            BorderColor::all(TEXT_DIM),
        ))
        .with_children(|bar| {
            bar.spawn((
                Node {
                    width: percent(0),
                    height: percent(100),
                    ..default()
                },
                BackgroundColor(ACCENT),
                BootProgress,
            ));
        });
    });
}

/// Keep the screen's text and bar in step with the status.
fn update_boot_screen(
    status: Res<BootStatus>,
    mut text: Query<(&mut Text, &mut TextColor), With<BootMessage>>,
    mut bars: Query<&mut Node, With<BootProgress>>,
) {
    use crate::ui::theme::{ERROR, TEXT};

    if !status.is_changed() {
        return;
    }

    for (mut text, mut colour) in &mut text {
        text.0 = match status.progress {
            Some(fraction) if status.phase == BootPhase::Extracting => {
                format!("{}\n{:.0}%", status.message, fraction * 100.0)
            }
            _ => status.message.clone(),
        };
        colour.0 = if status.phase == BootPhase::Failed {
            ERROR
        } else {
            TEXT
        };
    }

    let fraction = match status.phase {
        BootPhase::Extracting => status.progress.unwrap_or(0.0),
        BootPhase::LoadingGlobals => 0.95,
        BootPhase::Done => 1.0,
        _ => 0.0,
    };
    for mut bar in &mut bars {
        bar.width = percent(fraction * 100.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::catalog::{LevelEntry, LevelId, LevelKind};

    /// A minimal app with the state machine and boot's own systems, but no
    /// window: enough to drive the phases.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::MinimalPlugins,
            bevy::state::app::StatesPlugin,
            bevy::log::LogPlugin::default(),
        ));
        app.init_state::<AppState>();
        app.init_resource::<BootStatus>();
        app.init_resource::<LevelCatalog>();
        app.add_systems(Update, leave_boot.run_if(in_state(AppState::Boot)));
        app
    }

    fn one_level() -> LevelCatalog {
        LevelCatalog {
            levels: vec![LevelEntry {
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
            }],
            ..default()
        }
    }

    #[test]
    fn an_empty_catalog_fails_boot_rather_than_showing_an_empty_menu() {
        let mut app = test_app();
        app.world_mut().resource_mut::<BootStatus>().phase = BootPhase::LoadingGlobals;

        app.update();

        let status = app.world().resource::<BootStatus>();
        assert_eq!(status.phase, BootPhase::Failed);
        assert!(
            status.message.contains("open-bft extract"),
            "the failure has to say how to fix it, got {:?}",
            status.message
        );
    }

    #[test]
    fn boot_goes_to_the_front_end_when_nothing_asked_for_a_level() {
        let mut app = test_app();
        app.insert_resource(one_level());
        app.world_mut().resource_mut::<BootStatus>().phase = BootPhase::LoadingGlobals;

        app.update();
        app.update();

        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::FrontEnd
        );
    }

    /// `open-bft view m1` must not stop at the menu on the way.
    #[test]
    fn a_pending_level_skips_the_front_end() {
        let mut app = test_app();
        app.insert_resource(one_level());
        app.insert_resource(PendingLevel::viewer(LevelId(1)));
        app.world_mut().resource_mut::<BootStatus>().phase = BootPhase::LoadingGlobals;

        app.update();
        app.update();

        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::Loading
        );
    }

    #[test]
    fn a_failed_boot_stays_failed_and_never_reaches_a_menu() {
        let mut app = test_app();
        // No levels, so the first update fails boot.
        app.world_mut().resource_mut::<BootStatus>().phase = BootPhase::LoadingGlobals;

        app.update();
        app.update();
        app.update();

        assert_eq!(
            *app.world().resource::<State<AppState>>().get(),
            AppState::Boot,
            "a failed boot must hold the screen that says why"
        );
    }
}
