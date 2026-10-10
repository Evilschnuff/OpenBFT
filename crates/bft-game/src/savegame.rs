//! In-mission saves: F8 and F9, the menus' save lists, and the load flow.
//!
//! The format and the capture/restore are `bft_sim::save`'s
//! (`docs/SAVEGAME.md`). This module is the client half: it knows where saves
//! live, which level a save needs and how to get there, and how to tell the
//! player what happened.
//!
//! # A load
//!
//! 1. [`LoadRequest`] reads the file, sets the save's difficulty, holds the
//!    sim with [`RestoreGate`] and asks for a *fresh* load of the save's level
//!    — the loaded one is torn down even when it is the same mission, since
//!    the save is a difference from the level as the data has it.
//! 2. The level loads and the mission is set up as for a new game.
//! 3. [`after_mission_setup`] records the save baseline and, with a
//!    [`PendingLoad`] waiting, restores it. The sim's first tick then calls
//!    `load_control` instead of the intro, as the original did.
//!
//! Every mission records the baseline in step 3, loaded or not, because a
//! save made later is a difference from it.
//!
//! # Pictures
//!
//! Each save keeps a small picture of the game beside it (`<save>.png`),
//! so the save dialog can show where it was made. A quick
//! save takes it there and then; a save from the menu uses the view as it
//! was when the pause menu opened ([`GameSnapshot`]), which the menu holds
//! back from drawing for the frame or two the capture takes.

use std::path::{Path, PathBuf};

use bevy::asset::RenderAssetUsages;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bft_assets::catalog::{LevelCatalog, LevelKind};
use bft_assets::messages::Messages;
use bft_sim::level::DespawnLevel;
use bft_sim::mission::{MissionScript, MissionState};
use bft_sim::save::{
    self, CaptureMeta, QUICKSAVE_FILE, RestoreGate, SaveBaseline, SaveEntry, SaveFile, SaveRefusal,
};
use bft_sim::states::{AppState, GameMode, PendingLevel};
use bft_sim::vehicle::Difficulty;

use crate::globals::{self, Globals};
use crate::loading::LoadFailure;
use crate::mission_setup::MissionReady;
use crate::ui::bindings::{Action, ActionInput};
use crate::ui::hud::HudLog;
use crate::ui::settings_screen::RebindState;

/// Save the running mission.
#[derive(Message, Debug, Clone, PartialEq)]
pub enum SaveRequest {
    /// F8: over the quick save.
    Quick,
    /// A new file in the save list.
    New,
    /// Over an existing file.
    Overwrite(PathBuf),
    /// A save the player named, in the original's save dialog: a new file,
    /// or over `over`.
    Labelled {
        label: String,
        over: Option<PathBuf>,
    },
}

/// Load a save.
#[derive(Message, Debug, Clone, PartialEq)]
pub enum LoadRequest {
    /// F9: the quick save.
    Quick,
    Path(PathBuf),
}

/// A save read and waiting for its level to be set up.
#[derive(Resource, Debug, Clone)]
pub struct PendingLoad(pub SaveFile);

/// `play --load <file>`: load this once the front end is up.
#[derive(Resource, Debug, Clone)]
pub struct LoadOnBoot(pub PathBuf);

/// The saves on disk, as the menus list them. Refreshed when a list opens
/// and after every save.
#[derive(Resource, Debug, Clone, Default)]
pub struct SaveListing {
    pub entries: Vec<SaveEntry>,
}

impl SaveListing {
    /// Read the save directory again.
    pub fn refresh(&mut self) {
        self.entries = save::saves_dir()
            .map(|dir| save::list_saves(&dir))
            .unwrap_or_default();
    }
}

/// The game view as it was when the pause menu last opened, small.
#[derive(Resource, Debug, Clone, Default)]
pub struct GameSnapshot(pub Option<Image>);

/// A snapshot is being captured: the pause menu waits for it, for at most
/// this many more frames.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapshotPending(pub u8);

/// The size a save's picture is cut down to, in pixels: enough for the save
/// dialog's corner at any scale that dialog is drawn at.
const THUMBNAIL: (u32, u32) = (480, 360);

/// Frames the pause menu waits for a snapshot before drawing anyway.
const SNAPSHOT_WAIT: u8 = 10;

/// The picture kept beside a save.
pub fn thumbnail_path(save: &Path) -> PathBuf {
    save.with_extension("png")
}

/// Capture the game view as it is this frame into [`GameSnapshot`].
pub fn request_snapshot(commands: &mut Commands) {
    commands.insert_resource(SnapshotPending(SNAPSHOT_WAIT));
    commands.spawn(Screenshot::primary_window()).observe(
        |captured: On<ScreenshotCaptured>,
         mut commands: Commands,
         mut images: ResMut<Assets<Image>>| {
            commands.insert_resource(GameSnapshot(thumbnail(&captured.image)));
            if let Some(blurred) = blurred(&captured.image) {
                commands.insert_resource(crate::ui::dialog::GameBackdrop(images.add(blurred)));
            }
            commands.remove_resource::<SnapshotPending>();
        },
    );
}

/// A captured frame cut down to a save's picture, `None` when it cannot be
/// read.
pub fn thumbnail(image: &Image) -> Option<Image> {
    let small = image
        .clone()
        .try_into_dynamic()
        .ok()?
        .thumbnail(THUMBNAIL.0, THUMBNAIL.1);
    Some(Image::from_dynamic(
        small,
        true,
        RenderAssetUsages::default(),
    ))
}

/// A captured frame, small and blurred: the in-game menus' backdrop. The
/// window's own shape is kept, since it is drawn over the whole window.
pub fn blurred(image: &Image) -> Option<Image> {
    let view = image.clone().try_into_dynamic().ok()?;
    // Small first: the blur is cheaper and the stretch softens it further.
    let small = view.thumbnail(BACKDROP_WIDTH, BACKDROP_WIDTH);
    Some(Image::from_dynamic(
        small.blur(BACKDROP_BLUR),
        true,
        RenderAssetUsages::default(),
    ))
}

/// The in-game backdrop's width before stretching, and its blur (a
/// Gaussian's sigma, in those pixels): presentation, ours.
const BACKDROP_WIDTH: u32 = 320;
const BACKDROP_BLUR: f32 = 3.0;

/// Write a picture beside a save.
fn write_thumbnail(image: &Image, save: &Path) {
    let path = thumbnail_path(save);
    let written = image
        .clone()
        .try_into_dynamic()
        .map_err(|e| e.to_string())
        .and_then(|picture| picture.to_rgb8().save(&path).map_err(|e| e.to_string()));
    if let Err(e) = written {
        warn!("cannot write {}: {e}", path.display());
    }
}

/// Give up waiting on a snapshot that never comes (no renderer, a failed
/// capture), so the pause menu is never held for good.
pub(crate) fn expire_snapshot(mut commands: Commands, pending: Option<ResMut<SnapshotPending>>) {
    if let Some(mut pending) = pending {
        match pending.0.checked_sub(1) {
            Some(left) => pending.0 = left,
            None => commands.remove_resource::<SnapshotPending>(),
        }
    }
}

/// Read a save's picture, if it has one.
pub fn load_thumbnail(save: &Path) -> Option<Image> {
    let bytes = std::fs::read(thumbnail_path(save)).ok()?;
    Image::from_buffer(
        &bytes,
        bevy::image::ImageType::Extension("png"),
        bevy::image::CompressedImageFormats::NONE,
        true,
        bevy::image::ImageSampler::linear(),
        RenderAssetUsages::default(),
    )
    .ok()
}

/// The pictures of the listed saves, read once each.
#[derive(Resource, Debug, Clone, Default)]
pub struct SaveThumbnails(pub bevy::platform::collections::HashMap<PathBuf, Option<Handle<Image>>>);

impl SaveThumbnails {
    /// A save's picture, if it has one and it has been read.
    pub fn get(&self, save: &Path) -> Option<&Handle<Image>> {
        self.0.get(save)?.as_ref()
    }
}

/// Read the pictures of saves newly in the listing, and forget the gone.
fn read_thumbnails(
    listing: Res<SaveListing>,
    mut thumbnails: ResMut<SaveThumbnails>,
    mut images: ResMut<Assets<Image>>,
) {
    if !listing.is_changed() {
        return;
    }
    thumbnails
        .0
        .retain(|path, _| listing.entries.iter().any(|entry| entry.path == *path));
    for entry in &listing.entries {
        // A picture not there last time may be now: a quick save's is
        // written a frame or two after the save.
        if thumbnails.get(&entry.path).is_some() {
            continue;
        }
        let handle = load_thumbnail(&entry.path).map(|image| images.add(image));
        thumbnails.0.insert(entry.path.clone(), handle);
    }
}

/// Save and load.
pub struct SaveGamePlugin;

impl Plugin for SaveGamePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SaveRequest>()
            .add_message::<LoadRequest>()
            .init_resource::<SaveListing>()
            .init_resource::<GameSnapshot>()
            .init_resource::<SaveThumbnails>()
            .add_systems(Update, read_thumbnails)
            .add_systems(
                Update,
                (
                    quick_keys
                        .run_if(in_state(AppState::InGame).or_else(in_state(AppState::FrontEnd))),
                    perform_saves.run_if(in_state(AppState::InGame)),
                    start_loads,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                after_mission_setup
                    .run_if(in_state(AppState::InGame))
                    .run_if(resource_added::<MissionReady>)
                    .after(crate::mission_setup::setup_mission),
            )
            .add_systems(
                OnEnter(AppState::FrontEnd),
                (drop_stale_load, load_on_boot).chain(),
            );
    }
}

/// F8 saves, F9 loads (the original's keys, manual "Other Controls").
fn quick_keys(
    input: ActionInput,
    rebind: Res<RebindState>,
    state: Res<State<AppState>>,
    mut saves: MessageWriter<SaveRequest>,
    mut loads: MessageWriter<LoadRequest>,
) {
    if rebind.is_capturing() {
        return;
    }
    if *state.get() == AppState::InGame && input.just_pressed(Action::QuickSave) {
        saves.write(SaveRequest::Quick);
    }
    if input.just_pressed(Action::QuickLoad) {
        loads.write(LoadRequest::Quick);
    }
}

/// Carry out this frame's save requests.
///
/// Exclusive, because a capture reads the whole world.
fn perform_saves(world: &mut World) {
    let requests: Vec<SaveRequest> = world
        .resource_mut::<bevy::ecs::message::Messages<SaveRequest>>()
        .drain()
        .collect();
    for request in requests {
        let line = match save_now(world, &request) {
            Ok(()) => "Game saved.".to_string(),
            Err(SaveError::Refused(refusal)) => refusal_text(world, refusal),
            Err(SaveError::File(e)) => {
                error!("cannot save: {e}");
                format!("Save failed: {e}")
            }
        };
        say(world, line);
        world.resource_mut::<SaveListing>().refresh();
    }
}

enum SaveError {
    Refused(SaveRefusal),
    File(save::SaveFileError),
}

fn save_now(world: &mut World, request: &SaveRequest) -> Result<(), SaveError> {
    save::can_save(world).map_err(SaveError::Refused)?;

    let created = save::unix_now();
    let dir = save::saves_dir().map_err(SaveError::File)?;
    let path = match request {
        SaveRequest::Quick => dir.join(QUICKSAVE_FILE),
        SaveRequest::New => save::new_save_path(&dir, created),
        SaveRequest::Overwrite(path) => path.clone(),
        SaveRequest::Labelled {
            over: Some(path), ..
        } => path.clone(),
        SaveRequest::Labelled { over: None, .. } => save::new_save_path(&dir, created),
    };

    let level = world
        .get_resource::<MissionState>()
        .map(|state| state.mission);
    let entry = level.and_then(|level| world.resource::<LevelCatalog>().level(level).cloned());
    let title_key = entry
        .as_ref()
        .map(|entry| entry.title_key.clone())
        .unwrap_or_default();
    let label = match request {
        SaveRequest::Quick => "Quick save".to_string(),
        SaveRequest::Labelled { label, .. } if !label.trim().is_empty() => label.trim().to_string(),
        _ => text(world, &title_key),
    };

    let meta = CaptureMeta {
        label,
        level_name: entry.map(|entry| entry.name).unwrap_or_default(),
        title_key,
        written_by: format!("open-bft {}", env!("CARGO_PKG_VERSION")),
        created,
    };
    let saved = save::capture(world, meta).map_err(SaveError::Refused)?;
    save::write_save(&saved, &path).map_err(SaveError::File)?;
    info!("saved to {}", path.display());

    // Its picture: the view now for a quick save, made in play; the view
    // from before the pause menu for one made from the menu.
    match request {
        SaveRequest::Quick => {
            world.spawn(Screenshot::primary_window()).observe(
                move |captured: On<ScreenshotCaptured>| {
                    if let Some(small) = thumbnail(&captured.image) {
                        write_thumbnail(&small, &path);
                    }
                },
            );
        }
        _ => {
            if let Some(snapshot) = world.resource::<GameSnapshot>().0.clone() {
                write_thumbnail(&snapshot, &path);
            }
        }
    }
    // The listing reads pictures afresh for a save written over.
    if let Some(mut thumbnails) = world.get_resource_mut::<SaveThumbnails>() {
        thumbnails.0.clear();
    }
    Ok(())
}

/// Turn a load request into a fresh load of the save's level.
// Nine parameters: the request, the three things a load checks, and the five
// it sets on the way to `Loading`.
#[allow(clippy::too_many_arguments)]
fn start_loads(
    mut commands: Commands,
    mut requests: MessageReader<LoadRequest>,
    catalog: Res<LevelCatalog>,
    state: Res<State<AppState>>,
    mut failure: ResMut<LoadFailure>,
    mut log: ResMut<HudLog>,
    time: Res<Time<Real>>,
    mut next: ResMut<NextState<AppState>>,
    mut mode: ResMut<NextState<GameMode>>,
) {
    // Last request wins.
    let Some(request) = requests.read().last().cloned() else {
        return;
    };

    let path = match request {
        LoadRequest::Path(path) => Ok(path),
        LoadRequest::Quick => save::saves_dir().map(|dir| dir.join(QUICKSAVE_FILE)),
    };
    let loaded = path.and_then(|path| {
        let save = save::load_save(&path)?;
        Ok((path, save))
    });

    let problem = match &loaded {
        Err(save::SaveFileError::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            Some("There is no saved game to load.".to_string())
        }
        Err(e) => Some(format!("Cannot load the saved game: {e}")),
        Ok((_, save)) => {
            let level = bft_assets::catalog::LevelId(save.header.level);
            match catalog.level(level) {
                Some(entry) if matches!(entry.kind, LevelKind::Campaign { .. }) => None,
                Some(_) => Some(format!(
                    "The saved game is on {level}, which is not a campaign mission."
                )),
                None => Some(format!(
                    "The saved game needs {level}, which is not in the extraction cache."
                )),
            }
        }
    };
    if let Some(problem) = problem {
        warn!("{problem}");
        if *state.get() == AppState::InGame {
            log.push(problem, time.elapsed_secs());
        } else {
            failure.0 = Some(problem);
        }
        return;
    }
    let Ok((path, save)) = loaded else {
        return;
    };

    let level = bft_assets::catalog::LevelId(save.header.level);
    info!("loading {} ({level})", path.display());

    // The tables a level is built from depend on the difficulty, and the
    // save is a difference from the level at *its* difficulty.
    commands.insert_resource(Difficulty::from(save.header.difficulty));
    commands.insert_resource(RestoreGate);
    commands.insert_resource(PendingLoad(save));
    // Fresh, even for the mission being played: a save applies to the level
    // as the data has it, not as this session left it.
    commands.trigger(DespawnLevel);
    commands.insert_resource(PendingLevel::in_game(level));
    mode.set(GameMode::Campaign);
    next.set(AppState::Loading);
}

/// Record the baseline, then restore a waiting save.
///
/// Runs once per level, on the frame its mission is set up and before the
/// sim has stepped it (a load holds the sim with [`RestoreGate`]).
pub fn after_mission_setup(world: &mut World) {
    SaveBaseline::record(world);

    let Some(PendingLoad(save)) = world.remove_resource::<PendingLoad>() else {
        return;
    };
    let ready = world.resource::<MissionReady>().0;
    if ready.0 != save.header.level {
        warn!(
            "a save for level {} arrived on level {ready}; not restoring it",
            save.header.level
        );
        world.remove_resource::<RestoreGate>();
        return;
    }

    let report = save::restore(world, &save);
    let line = if report.skipped.is_empty() {
        "Game loaded.".to_string()
    } else {
        format!(
            "Game loaded; {} part(s) of the save no longer match this level.",
            report.skipped.len()
        )
    };
    say(world, line);
}

/// A load that never reached its level must not leave the next one held.
fn drop_stale_load(mut commands: Commands) {
    commands.remove_resource::<PendingLoad>();
    commands.remove_resource::<RestoreGate>();
}

/// `play --load`, once the front end is up.
fn load_on_boot(
    mut commands: Commands,
    request: Option<Res<LoadOnBoot>>,
    mut loads: MessageWriter<LoadRequest>,
) {
    if let Some(request) = request {
        loads.write(LoadRequest::Path(request.0.clone()));
        commands.remove_resource::<LoadOnBoot>();
    }
}

/// A line in the HUD log.
fn say(world: &mut World, line: String) {
    info!("{line}");
    let now = world.resource::<Time<Real>>().elapsed_secs();
    world.resource_mut::<HudLog>().push(line, now);
}

/// A `MSG_*` key through the string table, from exclusive-system land.
fn text(world: &World, key: &str) -> String {
    let globals = world.get_resource::<Globals>();
    let tables = world.resource::<Assets<Messages>>();
    globals::text(globals, tables, key)
}

/// What a refused save says: the original's message where it had one.
fn refusal_text(world: &World, refusal: SaveRefusal) -> String {
    match refusal.message_key() {
        Some(key) => text(world, key),
        None => capitalised(&refusal.to_string()),
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => format!("{}{}.", first.to_uppercase(), chars.as_str()),
        None => String::new(),
    }
}

/// Whether the pause menu's Save button works, and if not why.
pub fn save_refusal_now(
    mode: Option<&State<GameMode>>,
    state: Option<&MissionState>,
    control: Option<&bft_sim::cinematic::PlayerControl>,
    script: Option<&MissionScript>,
) -> Option<SaveRefusal> {
    // A level with no script is no mission, whatever the mode says.
    let state = state.filter(|_| script.is_some());
    save::save_refusal(mode.map(State::get), state, control)
}

/// A save's line in a list: its label, mission and when.
pub fn describe(entry: &SaveEntry, messages: Option<&Messages>) -> String {
    let Some(header) = &entry.header else {
        let name = entry
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        return format!("{name} (unreadable)");
    };
    let title = crate::ui::theme::message_text(messages, &header.title_key);
    let when = civil_time(header.created);
    let minutes = (header.play_time / 60.0).floor() as u32;
    let label = if header.label.is_empty() || header.label == title {
        title.clone()
    } else {
        format!("{} — {title}", header.label)
    };
    format!("{label} · {when} · {minutes} min in")
}

/// `YYYY-MM-DD HH:MM` in UTC, from Unix seconds.
///
/// Days to a civil date by Howard Hinnant's algorithm, so the list needs no
/// date crate for one line of text.
pub fn civil_time(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let seconds = unix % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        seconds / 3_600,
        seconds % 3_600 / 60
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_time_is_utc_and_zero_padded() {
        assert_eq!(civil_time(0), "1970-01-01 00:00 UTC");
        // Single-digit month, day, hour and minute.
        assert_eq!(civil_time(1_941_350_700), "2031-07-09 08:05 UTC");
        // A leap day.
        assert_eq!(civil_time(951_782_400), "2000-02-29 00:00 UTC");
    }

    #[test]
    fn a_refusal_reads_as_a_sentence() {
        assert_eq!(capitalised("the mission is over"), "The mission is over.");
    }
}
