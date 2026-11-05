//! The `open-bft` binary: argument parsing and plugin registration, nothing
//! else. Every behaviour it reaches for lives in one of the three libraries.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use bevy::diagnostic::{FrameTimeDiagnosticsPlugin, LogDiagnosticsPlugin};
use bevy::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::{AssetsPlugin, DataPaths, ExtractProgress, LevelId, extract};
use bft_game::GamePlugin;
use bft_game::devtools::DevToolsPlugin;
use bft_game::devtools::inspect::{self, InspectTarget};
use bft_game::viewer::ScreenshotRun;
use bft_sim::SimPlugin;
use clap::{Args, Parser, Subcommand};

/// An open rebuild of Zipper Interactive's *Recoil* (1999).
#[derive(Parser)]
#[command(name = "open-bft", version)]
struct Cli {
    #[command(flatten)]
    paths: PathArgs,

    /// How GameZ axes map onto Bevy's, as three comma-separated axes naming
    /// where Bevy's x, y and z come from: `x,y,z` (the default), `x,y,-z`,
    /// `x,z,y`, ...
    ///
    /// The shipped default is the identity, which the real data supports
    /// (docs/data/coordinates.md and docs/data/models.md). This override exists so a wrong call
    /// can be disproved in the viewer in seconds rather than by a rebuild.
    #[arg(long, value_name = "X,Y,Z", global = true)]
    axes: Option<String>,

    /// Print frames per second and frame time to the terminal once a
    /// second: a readout that needs no window, for a headset run or one
    /// over ssh.
    #[arg(long, global = true)]
    log_fps: bool,

    #[command(subcommand)]
    command: Option<Command>,
}

impl Cli {
    /// The axis convention this run uses.
    ///
    /// An unparseable or degenerate override is a warning and the default,
    /// not an exit: the point of the flag is fast experimentation, and
    /// refusing to start over a typo would defeat that.
    fn axes(&self) -> AxisConvention {
        let Some(text) = &self.axes else {
            return AxisConvention::GAMEZ_DEFAULT;
        };

        match AxisConvention::parse(text) {
            Some(axes) => axes,
            None => {
                eprintln!(
                    "open-bft: --axes {text} is not a permutation of x, y and z \
                     (try `x,y,-z`); using the default"
                );
                AxisConvention::GAMEZ_DEFAULT
            }
        }
    }
}

/// Where the game data, its cache and `unzbd` are.
///
/// Each one overrides the environment, the config file and the conventional
/// locations, in that order (FR-D1).
#[derive(Args, Clone)]
struct PathArgs {
    /// The Recoil install directory. Overrides BFT_DATA_DIR and the config.
    #[arg(long, value_name = "DIR", global = true)]
    data_dir: Option<PathBuf>,
    /// Where to keep extracted data. Defaults to the platform cache dir.
    #[arg(long, value_name = "DIR", global = true)]
    cache_dir: Option<PathBuf>,
    /// The mech3ax `unzbd` binary. Overrides BFT_UNZBD and the config.
    #[arg(long, value_name = "FILE", global = true)]
    unzbd: Option<PathBuf>,
}

impl PathArgs {
    /// Resolve every path, leaving the reporting to the caller.
    ///
    /// A windowed run puts the message in the window; the batch subcommands
    /// print it and exit, which is what [`resolve`](Self::resolve) does.
    fn resolve_quietly(&self) -> Result<DataPaths, bft_assets::DataRootError> {
        DataPaths::discover(
            self.data_dir.as_deref(),
            self.cache_dir.as_deref(),
            self.unzbd.as_deref(),
        )
    }

    /// Resolve every path, or print FR-D1's message and exit 2.
    fn resolve(&self) -> Result<DataPaths, ExitCode> {
        DataPaths::discover(
            self.data_dir.as_deref(),
            self.cache_dir.as_deref(),
            self.unzbd.as_deref(),
        )
        .map_err(|e| {
            eprintln!("open-bft: {e}");
            // 2 for "cannot find the data", distinct from 1 for "something
            // went wrong while using it".
            ExitCode::from(2)
        })
    }
}

/// What the binary was asked to do. `play` is the default.
#[derive(Subcommand)]
enum Command {
    /// Play the game (the default when no subcommand is given).
    Play {
        /// Drive straight into this level, e.g. `m1`, skipping the menu.
        ///
        /// Without it the front end opens and its "Play" buttons do the same
        /// thing; this is how the vehicle is checked by eye in one command.
        level: Option<String>,
        /// The same as the positional level, spelled as
        /// `play --level <id> --unlock-all --dev` spells it.
        #[arg(
            long = "level",
            value_name = "LEVEL",
            id = "level_flag",
            conflicts_with = "level"
        )]
        level_flag: Option<String>,
        /// Write a PNG into this directory and exit.
        #[arg(long, value_name = "DIR", num_args = 0..=1, default_missing_value = ".")]
        screenshot: Option<PathBuf>,
        /// Draw the physics overlay: collider outlines and water volumes.
        #[arg(long)]
        debug_physics: bool,
        /// Open every mission and vehicle form and mount every weapon full,
        /// without reading or writing the save.
        ///
        /// FR-M7. The campaign normally unlocks a mission when the one before
        /// it is won, a form when its chip is found and a weapon when its
        /// pickup is; this is how a later mission is reached in one command.
        #[arg(long)]
        unlock_all: bool,
        /// Add the dev tools: `F1` inspector, overlays, Ctrl+click to select.
        /// Always on in a debug build.
        #[arg(long)]
        dev: bool,
        /// Load a saved game once the front end is up: a save file's path,
        /// or `quick` for the quick save (docs/SAVEGAME.md).
        #[arg(long, value_name = "FILE", conflicts_with_all = ["level", "level_flag"])]
        load: Option<PathBuf>,
        /// Look through a VR headset, over OpenXR. Needs a build with the
        /// `vr` feature (`cargo run --features vr -- play --vr`); without a
        /// headset runtime answering, the game runs flat.
        #[arg(long)]
        vr: bool,
    },
    /// Open a level in the free-fly viewer.
    View {
        /// The level to open, e.g. `m1`.
        level: String,
        /// Write a PNG of each level opened into this directory and exit.
        ///
        /// Screenshots are derived from game data, so they are never
        /// committed; this produces them reproducibly.
        #[arg(long, value_name = "DIR", num_args = 0..=1, default_missing_value = ".")]
        screenshot: Option<PathBuf>,
        /// With --screenshot, walk the whole catalog instead of one level.
        #[arg(long, requires = "screenshot")]
        all: bool,
        /// Draw the physics overlay: collider outlines and water volumes.
        ///
        /// The dev tools have the in-game toggle; this is how "colliders
        /// outline the visible geometry and water volumes appear where water
        /// is visible" is checked without them.
        #[arg(long)]
        debug_physics: bool,
        /// Add the dev tools: `F1` inspector, overlays, Ctrl+click to select.
        /// Always on in a debug build.
        #[arg(long)]
        dev: bool,
        /// Look through a VR headset, over OpenXR. Needs a build with the
        /// `vr` feature (`cargo run --features vr -- play --vr`); without a
        /// headset runtime answering, the game runs flat.
        #[arg(long)]
        vr: bool,
    },
    /// Run a headless dedicated server.
    Server,
    /// Extract a Recoil install into the asset cache.
    Extract {
        /// Print the commands that would run, without running them.
        #[arg(long)]
        dry_run: bool,
        /// Re-extract everything, ignoring what the cache already holds.
        #[arg(long)]
        force: bool,
    },
    /// Print what the loaders make of a piece of extracted data, without a
    /// window.
    Inspect {
        #[command(subcommand)]
        what: InspectCommand,
    },
}

/// What `inspect` prints (FR-T2).
#[derive(Subcommand)]
enum InspectCommand {
    /// A level's statistics and its node tree: names, kinds, models, flags.
    Level {
        /// The level, e.g. `m1` or `1`.
        level: String,
    },
    /// A texture archive: every texture's size and alpha.
    Textures {
        /// A level (`m1`) for its own archive, or an archive name or the
        /// start of one (`rtexture2`, `m1-rtexture4`).
        archive: String,
    },
    /// A reader archive's entries, or one entry as JSON.
    Reader {
        /// The level, or `global` for the shared archive (the default).
        level: Option<String>,
        /// An entry to print in full, e.g. `aiv.zrd` or `vehicle`.
        entry: Option<String>,
    },
    /// The interp scripts, or one mission's runtime bindings.
    Interp {
        /// The mission whose bindings to print.
        level: Option<String>,
    },
    /// The message table.
    Messages {
        /// Only keys or texts containing this.
        filter: Option<String>,
    },
    /// A level's anim definitions, or every definition with one name.
    Anim {
        /// The level, e.g. `m1`.
        level: String,
        /// An `anim_name` to print in full.
        name: Option<String>,
    },
}

impl InspectCommand {
    /// The library's target, with the level names parsed.
    fn target(&self) -> Result<InspectTarget, String> {
        let level = |text: &str| text.parse::<LevelId>();

        Ok(match self {
            InspectCommand::Level { level: id } => InspectTarget::Level(level(id)?),
            InspectCommand::Textures { archive } => InspectTarget::Textures(archive.clone()),
            InspectCommand::Reader { level: id, entry } => InspectTarget::Reader {
                level: match id.as_deref() {
                    None | Some("global") => None,
                    Some(id) => Some(level(id)?),
                },
                entry: entry.clone(),
            },
            InspectCommand::Interp { level: id } => InspectTarget::Interp {
                level: id.as_deref().map(level).transpose()?,
            },
            InspectCommand::Messages { filter } => InspectTarget::Messages {
                filter: filter.clone(),
            },
            InspectCommand::Anim { level: id, name } => InspectTarget::Anim {
                level: level(id)?,
                name: name.clone(),
            },
        })
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let axes = cli.axes();

    // `play` with no subcommand and no flags, for the bare `open-bft` case.
    let play = Command::Play {
        level: None,
        level_flag: None,
        screenshot: None,
        debug_physics: false,
        unlock_all: false,
        dev: false,
        load: None,
        vr: false,
    };
    let result = match cli.command.as_ref().unwrap_or(&play) {
        Command::Play {
            level,
            level_flag,
            screenshot,
            debug_physics,
            unlock_all,
            dev,
            load,
            vr,
        } => match level
            .as_deref()
            .or(level_flag.as_deref())
            .map(str::parse::<LevelId>)
            .transpose()
        {
            Ok(id) => run_client(
                &cli.paths,
                &match id {
                    Some(id) => format!("open-bft — {id}"),
                    None => "open-bft".to_string(),
                },
                id.map(bft_game::states::PendingLevel::in_game),
                axes,
                screenshot.as_ref().map(|directory| {
                    let mut run = ScreenshotRun {
                        directory: directory.clone(),
                        all_levels: false,
                        ..default()
                    };
                    // `BFT_SCREENSHOT_FRAMES=1500 open-bft play m2
                    // --screenshot` captures 25 s in rather than 2: how an
                    // intro's hand-off is checked by eye.
                    if let Some(frames) = std::env::var("BFT_SCREENSHOT_FRAMES")
                        .ok()
                        .and_then(|text| text.parse().ok())
                    {
                        run.settle_frames = frames;
                    }
                    run
                }),
                ClientFlags {
                    debug_physics: *debug_physics,
                    unlock_all: *unlock_all,
                    dev: *dev,
                    vr: *vr,
                    log_fps: cli.log_fps,
                },
                load.as_deref().map(save_path),
            ),
            Err(e) => {
                eprintln!("open-bft: {e}");
                return ExitCode::FAILURE;
            }
        },
        Command::View {
            level,
            screenshot,
            all,
            debug_physics,
            dev,
            vr,
        } => match level.parse::<LevelId>() {
            Ok(id) => run_client(
                &cli.paths,
                &format!("open-bft — {id}"),
                Some(bft_game::states::PendingLevel::viewer(id)),
                axes,
                screenshot.as_ref().map(|directory| ScreenshotRun {
                    directory: directory.clone(),
                    all_levels: *all,
                    ..default()
                }),
                ClientFlags {
                    debug_physics: *debug_physics,
                    // The viewer plays no mission, so there is nothing to
                    // unlock.
                    unlock_all: false,
                    dev: *dev,
                    vr: *vr,
                    log_fps: cli.log_fps,
                },
                None,
            ),
            Err(e) => {
                eprintln!("open-bft: {e}");
                return ExitCode::FAILURE;
            }
        },
        Command::Server => run_server(&cli.paths, axes),
        Command::Extract { dry_run, force } => run_extract(&cli.paths, *dry_run, *force),
        Command::Inspect { what } => match what.target() {
            Ok(target) => run_inspect(&cli.paths, axes, &target),
            Err(e) => {
                eprintln!("open-bft: {e}");
                return ExitCode::FAILURE;
            }
        },
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(code) => code,
    }
}

/// Build the cache with `unzbd` (FR-D3), or say what it would do.
fn run_extract(args: &PathArgs, dry_run: bool, force: bool) -> Result<(), ExitCode> {
    let paths = args.resolve()?;

    println!(
        "Data root: {} ({})",
        paths.root.path.display(),
        paths.root.version
    );
    println!("Cache:     {}", paths.cache.path.display());

    let manifest = match bft_assets::cache::load_manifest(&paths.cache) {
        Ok(manifest) => manifest,
        Err(e) => {
            eprintln!("open-bft: {e}");
            return Err(ExitCode::FAILURE);
        }
    };
    if let Some(manifest) = &manifest {
        println!(
            "Extracted by {} on {}",
            manifest.mech3ax_version, manifest.created
        );
    }

    // `--force` plans against no manifest, so every job counts as stale.
    let plan = match extract::plan(
        &paths.root,
        &paths.cache,
        if force { None } else { manifest.as_ref() },
    ) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("open-bft: {e}");
            return Err(ExitCode::FAILURE);
        }
    };

    let stale = plan.stale_count();
    println!(
        "Plan:      {} job(s), {stale} to run, {} up to date",
        plan.jobs.len(),
        plan.jobs.len() - stale
    );

    // Without `unzbd` the plan is still useful: FR-D3 lets the user populate
    // the cache by hand, and these are the commands that would do it.
    let Some(unzbd) = paths.unzbd.clone() else {
        println!();
        if dry_run {
            print_commands(&plan, std::path::Path::new("unzbd"), &paths.cache);
            return Ok(());
        }
        eprintln!(
            "open-bft: no `unzbd` found. Pass --unzbd <file>, set BFT_UNZBD, put it \
             on PATH, or run these commands yourself:\n"
        );
        print_commands(&plan, std::path::Path::new("unzbd"), &paths.cache);
        return Err(ExitCode::FAILURE);
    };
    println!("unzbd:     {}", unzbd.display());

    if dry_run {
        println!();
        print_commands(&plan, &unzbd, &paths.cache);
        return Ok(());
    }

    if stale == 0 {
        println!("\nNothing to do.");
        return Ok(());
    }

    println!();
    let result = extract::run(&plan, &unzbd, &paths.cache, progress_bar(stale));

    match result {
        Ok(manifest) => {
            println!(
                "\nDone: {} entries, extracted by {}.",
                manifest.entries.len(),
                manifest.mech3ax_version
            );
            Ok(())
        }
        Err(e) => {
            eprintln!("\nopen-bft: {e}");
            eprintln!(
                "\nThe cache holds everything that did extract; the levels above are \
                 the ones to expect trouble from."
            );
            Err(ExitCode::FAILURE)
        }
    }
}

/// Print a plan as copy-pasteable shell commands, one per line.
fn print_commands(
    plan: &extract::ExtractPlan,
    unzbd: &std::path::Path,
    cache: &bft_assets::CacheDir,
) {
    for job in plan.stale() {
        println!("{}", job.command_line(unzbd, cache));
    }
}

/// A one-line progress display on stdout (NFR-10).
fn progress_bar(stale: usize) -> impl FnMut(ExtractProgress) {
    let mut done = 0usize;

    move |progress| match progress {
        ExtractProgress::Started { kind, source, .. } => {
            let name = source
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            // `\r` and no newline: the line is overwritten by the next job,
            // so a 76-job run does not scroll the terminal.
            print!("\r\x1b[K[{}/{stale}] {kind} {name}", done + 1);
            let _ = std::io::stdout().flush();
        }
        ExtractProgress::Finished { .. } => done += 1,
        ExtractProgress::Skipped { .. } => {}
        ExtractProgress::Failed { reason, .. } => {
            done += 1;
            // Failures keep their own line: they must survive the overwrite.
            println!("\r\x1b[K  failed: {reason}");
        }
    }
}

/// Print a piece of extracted data (FR-T2), with no window.
fn run_inspect(
    args: &PathArgs,
    axes: AxisConvention,
    target: &InspectTarget,
) -> Result<(), ExitCode> {
    let paths = args.resolve()?;
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());

    let result = inspect::run(paths, axes, target, &mut out).and_then(|()| match out.flush() {
        Err(e) if e.kind() != std::io::ErrorKind::BrokenPipe => Err(e.into()),
        _ => Ok(()),
    });

    result.map_err(|e| {
        eprintln!("open-bft: {e}");
        ExitCode::FAILURE
    })
}

/// `--load`'s argument as a path: `quick` means the quick save.
fn save_path(argument: &std::path::Path) -> PathBuf {
    if argument == std::path::Path::new("quick")
        && let Ok(dir) = bft_sim::save::saves_dir()
    {
        return dir.join(bft_sim::save::QUICKSAVE_FILE);
    }
    argument.to_path_buf()
}

/// The switches a windowed run takes from the command line.
#[derive(Clone, Copy)]
struct ClientFlags {
    /// Start with the collider and water overlays on.
    debug_physics: bool,
    /// FR-M7: everything open, nothing saved.
    unlock_all: bool,
    /// Add the dev tools in a release build.
    dev: bool,
    /// Look through a VR headset.
    vr: bool,
    /// Print the frame rate to the terminal once a second.
    log_fps: bool,
}

/// A windowed app: presentation plus the simulation behind it.
fn run_client(
    args: &PathArgs,
    title: &str,
    pending: Option<bft_game::states::PendingLevel>,
    axes: AxisConvention,
    screenshot: Option<ScreenshotRun>,
    flags: ClientFlags,
    load: Option<PathBuf>,
) -> Result<(), ExitCode> {
    let ClientFlags {
        debug_physics,
        unlock_all,
        dev,
        vr,
        log_fps,
    } = flags;

    if vr && !VR_BUILT {
        eprintln!(
            "open-bft: --vr needs a build with the `vr` feature \
             (`cargo run --features vr -- play --vr`), on Linux or Windows"
        );
        return Err(ExitCode::FAILURE);
    }

    // A windowed run shows its failure in the window as well as on stderr
    // (FR-D1: `cargo run` without data shows the boot failure message on
    // screen). A user who double-clicked the binary has
    // no terminal to read, so exiting quietly here would tell them nothing.
    let paths = match args.resolve_quietly() {
        Ok(paths) => paths,
        Err(e) => {
            return run_no_data(
                title,
                &e.to_string(),
                screenshot.as_ref().map(|run| run.directory.clone()),
            );
        }
    };

    // The player's settings, read before the `App` exists: the texture
    // filter is baked into the loader when `AssetsPlugin` registers it, and
    // the window should open at the saved size and mode rather than resize
    // on its first frame. `UiPlugin` reads the same file again for the rest.
    let settings = bft_game::ui::load_settings();
    let mut window = Window {
        title: title.into(),
        ..default()
    };
    settings.video.apply_to(&mut window);

    let mut app = App::new();
    app.insert_resource(settings.clone());

    // `view <level>` and `play <level>` ask for a level up front, the first
    // targeting the viewer and the second the game; bare `play` waits for the
    // front end to ask. Boot reads the request and goes straight to `Loading`
    // instead of the menu when it is there.
    if let Some(pending) = pending {
        app.insert_resource(pending);
    }

    if let Some(screenshot) = screenshot {
        app.insert_resource(screenshot);
    }

    // Before the plugins, so the overlay is on for the very first frame a
    // level draws; `PhysicsSetupPlugin` keeps an existing resource.
    if debug_physics {
        app.insert_resource(bft_sim::physics::DebugFlags {
            physics: true,
            water: true,
            ..default()
        });
    }

    // Boot reads it when it loads the saved progress (FR-M7).
    app.insert_resource(bft_game::boot::UnlockAll(unlock_all));

    if let Some(load) = load {
        app.insert_resource(bft_game::savegame::LoadOnBoot(load));
    }

    app
        // Before the plugins, so a loader or a spawn system reading the
        // resource never sees the default when an override was asked for.
        .insert_resource(axes)
        // Before `DefaultPlugins`: Bevy builds asset sources when
        // `AssetPlugin` is built, so `cache://` has to be registered first.
        .add_plugins(AssetsPlugin::new(paths).with_texture_filter(settings.texture_filter()))
        .add_plugins(default_plugins(window, vr))
        .add_plugins(GamePlugin);

    #[cfg(all(feature = "vr", not(target_os = "macos")))]
    if vr {
        app.add_plugins(bft_xr::VrPlugin);
    }

    // The dev tools: in every debug build, and in a release one when asked.
    if dev || cfg!(debug_assertions) {
        app.add_plugins(DevToolsPlugin);
    }

    if log_fps {
        // The dev tools may have added the frame-time diagnostics already.
        if !app.is_plugin_added::<FrameTimeDiagnosticsPlugin>() {
            app.add_plugins(FrameTimeDiagnosticsPlugin::default());
        }
        app.add_plugins(LogDiagnosticsPlugin::filtered(
            [
                FrameTimeDiagnosticsPlugin::FPS,
                FrameTimeDiagnosticsPlugin::FRAME_TIME,
            ]
            .into_iter()
            .collect(),
        ));
    }

    app.run();

    Ok(())
}

/// Whether this build can look through a headset: the `vr` feature, on a
/// platform with OpenXR (`bft-xr` is empty on macOS).
const VR_BUILT: bool = cfg!(all(feature = "vr", not(target_os = "macos")));

/// Bevy's default plugins for a windowed run, with OpenXR rendering in
/// place of the flat renderer when `vr` asks for it.
fn default_plugins(window: Window, vr: bool) -> bevy::app::PluginGroupBuilder {
    let plugins = DefaultPlugins
        .set(WindowPlugin {
            primary_window: Some(window),
            ..default()
        })
        // An absolute path, because Bevy's default resolves against
        // `CARGO_MANIFEST_DIR` — `crates/bft-bin`, which holds no assets —
        // or the executable's directory. Neither finds this workspace's
        // `assets/`, and the tuning files live there.
        .set(bevy::asset::AssetPlugin {
            file_path: bft_assets::asset_directory().to_string_lossy().into_owned(),
            ..default()
        });

    #[cfg(all(feature = "vr", not(target_os = "macos")))]
    if vr {
        return bft_xr::with_xr(plugins);
    }
    // `run_client` refuses `--vr` without the feature before getting here.
    let _ = vr;
    plugins
}

/// Open a window that says why the engine cannot start (FR-D1).
///
/// A whole `App` rather than a dialog because Bevy is already the only
/// windowing this binary has, and the message is the same text the terminal
/// gets. Exits 2, as `open-bft extract` does for the same failure.
fn run_no_data(title: &str, message: &str, screenshot: Option<PathBuf>) -> Result<(), ExitCode> {
    eprintln!("open-bft: {message}");

    let text = format!("open-bft cannot start.\n\n{message}");

    let mut app = App::new();

    // The same `--screenshot` path the viewer uses, so this screen can be
    // captured unattended.
    if let Some(directory) = screenshot {
        let mut frames = 0u32;
        app.add_systems(
            Update,
            move |mut commands: Commands, mut exit: MessageWriter<AppExit>| {
                frames += 1;
                if frames == 30 {
                    commands
                        .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
                        .observe(bevy::render::view::screenshot::save_to_disk(
                            directory.join("no-data.png"),
                        ));
                }
                if frames > 40 {
                    exit.write(AppExit::Success);
                }
            },
        );
    }

    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: title.into(),
            ..default()
        }),
        ..default()
    }))
    .add_systems(Startup, move |mut commands: Commands| {
        commands.spawn(Camera2d);
        commands.spawn((
            Node {
                width: percent(100),
                height: percent(100),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                padding: UiRect::all(px(48)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.05, 0.05, 0.07)),
            children![(
                Text::new(text.clone()),
                TextFont::from_font_size(15.0),
                TextColor(Color::srgb(1.0, 0.6, 0.5)),
            )],
        ));
    })
    .run();

    Err(ExitCode::from(2))
}

/// A headless app: no window, no renderer, no `bft-game`.
fn run_server(args: &PathArgs, axes: AxisConvention) -> Result<(), ExitCode> {
    let paths = args.resolve()?;

    App::new()
        // The server builds colliders from the same convention the client
        // builds meshes from, or the two disagree about where the world is.
        .insert_resource(axes)
        // `MinimalPlugins` carries neither `LogPlugin` nor `AssetPlugin`, so
        // the server names both. Order matters twice over: logging is set up
        // first so `AssetsPlugin::build`'s "run `open-bft extract`" warning
        // has somewhere to go (NFR-9), and `AssetsPlugin` comes before
        // `AssetPlugin`, which is what builds the `cache://` source.
        .add_plugins(MinimalPlugins)
        .add_plugins(bevy::log::LogPlugin::default())
        .add_plugins(AssetsPlugin::new(paths))
        .add_plugins(bevy::asset::AssetPlugin::default())
        .add_plugins(SimPlugin)
        .run();

    Ok(())
}
