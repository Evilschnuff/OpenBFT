//! [`AppState::Viewer`]: the free-fly level inspector (FR-T1).
//!
//! The viewer is the way to look at what was loaded, so it does three
//! things and nothing else: put the camera somewhere the level actually is,
//! let `F1` show what was loaded, and let `Tab` walk the catalog without
//! going back through the menu.
//!
//! # Where the camera starts
//!
//! Not at the origin, and not at the `bft_00` node either. The mission's
//! `aiv.zrd` states where the player starts and which way they face
//! (READER-SCHEMA §3.2), and [`start_viewpoint`] puts the camera over their
//! shoulder — the view the level was built to open on. The `bft_00` *node* is
//! a build-script placeholder: it is 240 units out in m1 (in the water rather
//! than by the hangar) and an identity at the origin in the other twelve.
//!
//! [`viewpoint`] is the fallback for a level with no readable start, framing
//! the whole thing from its partition grid instead.

use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bft_assets::axes::{Axis, AxisConvention};
use bft_assets::catalog::{LevelCatalog, LevelId};
use bft_assets::reader::{PlayerStart, ReaderData};
use bft_sim::level::{NodeIndex, PartitionGrid, WorldEnv};
use bft_sim::states::{AppState, PendingLevel};

use crate::camera::FlyCamera;
use crate::devtools::DevPanelOpen;
use crate::level_view::LoadedLevel;

/// The `bft_00` node, which is **not** the player start.
///
/// Kept named because it is the obvious wrong answer and worth warning about.
/// The node is a build-script placeholder: m1's carries a
/// `RotateTranslateScale` at (2417, 0, 1117) rotated -70 degrees, and the
/// other twelve carry `transform: "Initial"`, an identity at the origin.
/// The real start is `aiv.zrd`'s `bft_00` record (READER-SCHEMA §3.2), which
/// [`ReaderData::player_start`] reads — in m1 that is
/// (2222.1, 3.9, 1294.2) facing 166.5 degrees, on the shore by the hangar,
/// about 240 units from where the node claims.
pub const PLAYER_START_NODE: &str = "bft_00";

/// How far the camera stands behind the player start, in world units.
///
/// The viewer opens over the player's shoulder rather than framing the whole
/// level: the start is where the level is meant to be seen from, and a wide
/// standoff on a level that fogs out at 320 units shows nothing anyway.
const START_STANDOFF: f32 = 28.0;
/// How far above the start the camera sits.
const START_HEIGHT: f32 = 11.0;

/// How far back and up from the viewpoint the camera sits, as a fraction of
/// the level's diagonal.
///
/// A tuning choice, not data: it exists to frame the level on entry, and the
/// user flies from there.
const FRAMING_DISTANCE: f32 = 0.12;
/// The least the camera backs off, in world units, for a level whose area is
/// tiny or missing.
const MIN_FRAMING: f32 = 60.0;
/// How high above the target the camera sits, as a multiple of the standoff.
///
/// Below 1, so the view is an oblique overlook rather than a top-down map.
/// The levels are wide and low and their interest is at ground level — m1's
/// shoreline and pier read well from a shallow angle and turn into open water
/// from a steep one — but the camera still has to clear the terrain, which
/// the `top` floor in [`viewpoint`] takes care of independently.
const HEIGHT_RATIO: f32 = 0.55;
/// The most of the level's *clear* distance the framing may use.
///
/// Measured against `fog_range.0`, where fog begins, not `fog_range.1`, where
/// it is total. The two differ a lot between levels — m1 fogs from 550, m5
/// and m11 from 320 — and a camera placed against the far plane looks
/// through mostly-opaque fog at everything it is meant to be showing: on m5
/// and m11, a flat white screen.
const FOG_FRACTION: f32 = 0.5;

/// The viewer: camera placement, the info panel, and the level-cycling keys.
pub struct ViewerPlugin;

impl Plugin for ViewerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PanelVisible>()
            .add_systems(
                OnEnter(AppState::Viewer),
                (
                    spawn_viewer_camera,
                    // With the dev tools on, `F1` is their inspector, which
                    // carries this panel's readout; two panels on one key
                    // would fight.
                    spawn_panel.run_if(not(resource_exists::<DevPanelOpen>)),
                )
                    .chain(),
            )
            .add_systems(OnExit(AppState::Viewer), despawn_viewer_level)
            .add_systems(
                OnEnter(AppState::Viewer),
                reset_screenshot_countdown.run_if(resource_exists::<ScreenshotRun>),
            )
            // The front end is worth capturing too, and `play --screenshot`
            // is the only way to reach it without a keyboard.
            .add_systems(
                OnEnter(AppState::FrontEnd),
                reset_screenshot_countdown.run_if(resource_exists::<ScreenshotRun>),
            )
            .add_systems(
                Update,
                front_end_screenshot
                    .run_if(in_state(AppState::FrontEnd))
                    .run_if(resource_exists::<ScreenshotRun>),
            )
            .add_systems(
                Update,
                (
                    (toggle_panel, update_panel).run_if(not(resource_exists::<DevPanelOpen>)),
                    cycle_level,
                    leave_viewer,
                    automatic_screenshot.run_if(resource_exists::<ScreenshotRun>),
                )
                    .run_if(in_state(AppState::Viewer)),
            )
            // `F12` is not viewer-only: the menu, the boot screen and the
            // game are worth capturing too, and a key that works in three
            // states and not the fourth is a key nobody trusts. This is the
            // one handler for all of them, so one press writes one file.
            .add_systems(Update, take_screenshot);
    }
}

/// Take a screenshot of each level as it opens, then leave.
///
/// A screenshot of every level is the quickest check of the loader, and
/// pressing `F12` thirteen times is not a repeatable way to get them. With
/// this set, the viewer waits [`Self::settle_frames`] frames for the level to
/// finish streaming in, writes a PNG into [`Self::directory`], and then
/// either cycles to the next level or exits.
#[derive(Resource, Debug, Clone)]
pub struct ScreenshotRun {
    /// Where the PNGs go.
    pub directory: std::path::PathBuf,
    /// Frames to wait after a level is spawned before capturing.
    ///
    /// Meshes, materials and textures are all built from observers and asset
    /// loads that land over several frames, so an immediate capture catches a
    /// half-built level.
    pub settle_frames: u32,
    /// Keep going through the whole catalog rather than just this level.
    pub all_levels: bool,
}

impl Default for ScreenshotRun {
    fn default() -> ScreenshotRun {
        ScreenshotRun {
            directory: std::path::PathBuf::from("."),
            // Generous: a cold shader cache costs the first level several
            // frames and a blank PNG is worse than a slow one.
            settle_frames: 120,
            all_levels: false,
        }
    }
}

/// How long the current automated capture has been waiting.
#[derive(Resource, Debug, Clone, Copy, Default)]
struct ScreenshotCountdown {
    frames: u32,
    captured: bool,
}

/// Whether the `F1` panel is up. Survives a level change, so cycling with
/// `Tab` does not keep reopening it.
#[derive(Resource, Debug, Clone, Copy)]
pub struct PanelVisible(pub bool);

impl Default for PanelVisible {
    fn default() -> PanelVisible {
        // On by default: the first thing a viewer is for is reading what was
        // loaded, and `F1` puts it away.
        PanelVisible(true)
    }
}

/// Marks the viewer's own camera, so leaving the state takes it with it.
#[derive(Component, Debug, Clone, Copy)]
pub struct ViewerCamera;

/// Marks the info panel's text.
#[derive(Component)]
struct PanelText;

/// Where a level's camera should start, and what it should look at.
///
/// Returns `(eye, target)` in world space.
///
/// - `bounds` is the bounding box of the geometry actually spawned. It is the
///   primary input, because a level's *declared* area is not where its
///   content is: m1 declares 0–4096 x 0–4352 for terrain that occupies
///   x 1287–2769, and m5's declared centre is above nothing at all.
/// - `start` is a point to centre on, when the caller has one. The player
///   start is *not* passed here — [`start_viewpoint`] handles that case —
///   so in practice this is `None` and the bounds decide.
/// - `fog_near` is the distance the level's fog starts at, which caps how far
///   out the camera may stand and still see the level clearly.
pub fn viewpoint(
    bounds: Option<(Vec3, Vec3)>,
    start: Option<Vec3>,
    fog_near: Option<f32>,
) -> (Vec3, Vec3) {
    let (centre, span, top) = match bounds {
        Some((min, max)) => (
            (min + max) * 0.5,
            Vec2::new(max.x - min.x, max.z - min.z).length(),
            max.y,
        ),
        None => (Vec3::ZERO, 0.0, 0.0),
    };

    // The player start is where the level is *interesting*; the centre of the
    // geometry is the fallback.
    let target = start.unwrap_or(centre);
    let mut back = (span * FRAMING_DISTANCE).max(MIN_FRAMING);

    // Stay well inside where the level's fog starts, or the opening view is a
    // wall of fog colour with the level invisible behind it. The budget is
    // spent on the whole eye-to-target distance, which the height dominates
    // once `HEIGHT_RATIO` is above 1, not on the horizontal standoff alone.
    if let Some(near) = fog_near.filter(|near| *near > 0.0) {
        let diagonal = Vec2::new(1.0, HEIGHT_RATIO.max(1.0)).length();
        back = back.min(near * FOG_FRACTION / diagonal);
    }

    // Stand off *away from* the level's centre and look back in, rather than
    // always from +z. m1's player start is on the shore facing open sea, so a
    // fixed direction opens on water with the level behind the camera; from
    // outside looking in, the content is in frame whichever way the level is
    // laid out.
    let outward = (target - centre)
        .with_y(0.0)
        .try_normalize()
        .unwrap_or(Vec3::Z);
    // Well above the terrain, not just above the target. The partition
    // grid's height band understates how tall a level actually is — m11's
    // bands top out at y 20 while its cliffs are visibly higher — so the
    // clearance is a multiple of the standoff rather than a margin over the
    // measured top, and the measured top is only a floor under it.
    let clearance = (back * HEIGHT_RATIO).max(MIN_FRAMING);
    // The terrain top is a *floor*, not an additional offset: clearing it
    // keeps the camera out of a hillside, but adding the clearance on top of
    // it again tips a wide, low level into a top-down map.
    let eye_y = (target.y + clearance).max(top + MIN_FRAMING * 0.25);
    let eye = (target + outward * back).with_y(eye_y);

    (eye, target)
}

/// The width of one partition cell, in world units (docs/data/nodes.md).
pub(crate) const CELL_SIZE: f32 = 256.0;

/// Where a level's terrain is, from the world node's partition grid.
///
/// The partition grid is the only thing in `gamez` that states the level's
/// extent in **world space and in all three axes**. The two obvious
/// alternatives are both wrong here:
///
/// - *Entity translations* collapse to a point, because only m1 places its
///   nodes; the other twelve keep every node at the origin with the geometry
///   in the mesh vertices.
/// - *Node bounding boxes* are in the node's own local space (m11's span
///   -1749..1749 about the origin) and include the parentless template
///   subtrees — the enemy, pickup and effect models DESIGN §4.4 says are not
///   part of the world. Framing on those opens the viewer on a floating
///   template aircraft, which is what m5 and m11 did.
///
/// Cells carry their own height band, so the box is the terrain's, not a
/// guess at it.
pub fn level_bounds(partitions: &PartitionGrid, axes: &AxisConvention) -> Option<(Vec3, Vec3)> {
    let mut bounds: Option<(Vec3, Vec3)> = None;

    for cell in partitions.cells() {
        // An empty cell still spans its column of the world, but a level's
        // grid covers its whole declared area whether or not anything stands
        // there — so only cells holding something count as "where the level
        // is".
        if cell.nodes.is_empty() {
            continue;
        }

        let min = Vec3::new(cell.x as f32, cell.y_min, cell.z as f32);
        let max = Vec3::new(
            cell.x as f32 + CELL_SIZE,
            cell.y_max,
            cell.z as f32 + CELL_SIZE,
        );

        for corner in [axes.point(min), axes.point(max)] {
            bounds = Some(match bounds {
                Some((lo, hi)) => (lo.min(corner), hi.max(corner)),
                None => (corner, corner),
            });
        }
    }

    bounds
}

/// Where the camera goes when the level states a player start.
///
/// Behind and above the player, looking the way they face — the view the
/// mission was built to open on. Framing the whole level is the fallback for
/// when there is no start to use, not the preference: the levels fog out
/// between 320 and 900 units, so there is often no vantage point that shows
/// more than this one does.
pub fn start_viewpoint(start: &PlayerStart) -> (Vec3, Vec3) {
    let facing = start.facing();
    // A little above the player's own height, looking slightly down at where
    // they would be driving.
    let eye = start.position - facing * START_STANDOFF + Vec3::Y * START_HEIGHT;
    let target = start.position + facing * START_STANDOFF * 0.5;

    (eye, target)
}

/// Spawn the fly camera where the level starts, or failing that where it is.
fn spawn_viewer_camera(
    mut commands: Commands,
    existing: Query<Entity, With<Camera3d>>,
    world: Option<Res<WorldEnv>>,
    loaded: Option<Res<LoadedLevel>>,
    readers: Res<Assets<ReaderData>>,
    partitions: Option<Res<PartitionGrid>>,
    axes: Option<Res<AxisConvention>>,
) {
    // A camera from an earlier state (or an earlier viewer session that did
    // not tear down) would fight this one for the render target.
    for entity in &existing {
        commands.entity(entity).despawn();
    }

    let start = loaded
        .as_ref()
        .and_then(|loaded| loaded.reader.as_ref())
        .and_then(|handle| readers.get(handle))
        .and_then(bft_assets::reader::ReaderData::player_start);

    let (eye, target) = match start.as_ref() {
        Some(start) => {
            info!(
                "player start ({:.0}, {:.0}, {:.0}) facing {:.0} degrees",
                start.position.x, start.position.y, start.position.z, start.heading_deg
            );
            start_viewpoint(start)
        }
        None => {
            // No reader archive, or it would not parse. The level still has
            // to be showable, so fall back to framing its extent.
            warn!("no player start for this level; framing the whole level instead");
            let axes = axes.map(|axes| *axes).unwrap_or_default();
            let bounds = partitions.and_then(|partitions| level_bounds(&partitions, &axes));
            let fog_near = world.map(|world| world.fog_range.0);
            viewpoint(bounds, None, fog_near)
        }
    };

    info!(
        "viewer camera at ({:.0}, {:.0}, {:.0}) looking at ({:.0}, {:.0}, {:.0})",
        eye.x, eye.y, eye.z, target.x, target.y, target.z
    );

    let (fly, transform) = FlyCamera::looking_at(eye, target);
    commands.spawn((Camera3d::default(), transform, fly, ViewerCamera));
}

/// Tear the level and the camera down on the way out.
///
/// The camera is despawned here rather than with `DespawnOnExit` because the
/// same system has to trigger the level teardown, and the two must happen in
/// one place or a half-torn-down level survives into the menu.
fn despawn_viewer_level(
    mut commands: Commands,
    cameras: Query<Entity, With<ViewerCamera>>,
    next: Res<NextState<AppState>>,
) {
    for entity in &cameras {
        commands.entity(entity).despawn();
    }

    // Cycling to another level keeps the loaded one until the new one is
    // ready, because `start_level_load` despawns the old one itself; only a
    // real exit tears it down here.
    if let NextState::Pending(AppState::Loading) = *next {
        return;
    }

    commands.trigger(bft_sim::level::DespawnLevel);
    commands.remove_resource::<LoadedLevel>();
}

/// The `F1` panel: what was loaded, where the camera is, which axes are in use.
///
/// Bevy UI rather than egui, deliberately: the full inspector is the dev
/// tools', which bring `bevy_egui` with them. This is four lines of text, and a
/// dependency for four lines of text is a dependency to justify later.
fn spawn_panel(mut commands: Commands, visible: Res<PanelVisible>) {
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: px(8),
            left: px(8),
            padding: UiRect::all(px(10)),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
        if visible.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        },
        DespawnOnExit(AppState::Viewer),
        children![(
            Text::default(),
            TextFont::from_font_size(13.0),
            TextColor(Color::srgb(0.85, 0.88, 0.95)),
            PanelText,
        )],
    ));
}

/// The `DebugPanel` action (F1 by default) shows and hides the panel.
fn toggle_panel(
    input: crate::ui::ActionInput,
    mut visible: ResMut<PanelVisible>,
    mut panels: Query<&mut Visibility, With<Node>>,
    children: Query<&ChildOf, With<PanelText>>,
) {
    if !input.just_pressed(crate::ui::Action::DebugPanel) {
        return;
    }

    visible.0 = !visible.0;
    for child_of in &children {
        if let Ok(mut panel) = panels.get_mut(child_of.parent()) {
            *panel = if visible.0 {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
        }
    }
}

/// Keep the panel's text current.
fn update_panel(
    mut text: Query<&mut Text, With<PanelText>>,
    loaded: Option<Res<LoadedLevel>>,
    catalog: Res<LevelCatalog>,
    nodes: Option<Res<NodeIndex>>,
    axes: Option<Res<AxisConvention>>,
    camera: Query<&Transform, With<ViewerCamera>>,
    visible: Res<PanelVisible>,
) {
    if !visible.0 {
        return;
    }

    let Ok(mut text) = text.single_mut() else {
        return;
    };

    let name = loaded
        .as_ref()
        .and_then(|loaded| catalog.level(loaded.id))
        .map(|entry| entry.name.clone())
        .or_else(|| loaded.as_ref().map(|loaded| loaded.id.to_string()))
        .unwrap_or_else(|| "no level".to_string());
    let node_count = nodes.map(|nodes| nodes.len()).unwrap_or(0);
    let position = camera
        .single()
        .map(|transform| transform.translation)
        .unwrap_or(Vec3::ZERO);
    let axes = axes.map(|axes| *axes).unwrap_or_default();

    text.0 = format!(
        "{name}   {node_count} nodes\n\
         camera  {:.0}, {:.0}, {:.0}\n\
         axes    {}\n\
         F1 panel   Tab next level   Esc menu",
        position.x,
        position.y,
        position.z,
        axis_readout(axes),
    );
}

/// The `--axes` readout: the convention, and whether it was overridden.
///
/// `AxisConvention` has no `Display`, and giving it one for a debug panel
/// would put a presentation decision in `bft-assets`; formatting it here
/// keeps that crate about data.
pub(crate) fn axis_readout(axes: AxisConvention) -> String {
    let name = |axis: Axis| match axis {
        Axis::X => "x",
        Axis::Y => "y",
        Axis::Z => "z",
        Axis::NegX => "-x",
        Axis::NegY => "-y",
        Axis::NegZ => "-z",
    };

    format!(
        "{},{},{} ({})",
        name(axes.x),
        name(axes.y),
        name(axes.z),
        if axes == AxisConvention::GAMEZ_DEFAULT {
            "default"
        } else {
            "overridden"
        }
    )
}

/// `Tab` loads the next level in the catalog (task: viewer controls).
///
/// It goes back through [`AppState::Loading`] rather than inserting
/// `LoadLevel` directly, so a level that fails to load shows its error and
/// lands in the menu exactly as it would from a button.
fn cycle_level(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    catalog: Res<LevelCatalog>,
    loaded: Option<Res<LoadedLevel>>,
    mut next: ResMut<NextState<AppState>>,
) {
    if !keys.just_pressed(KeyCode::Tab) {
        return;
    }

    let Some(id) = next_level(&catalog, loaded.map(|loaded| loaded.id)) else {
        warn!("no other level in the catalog to cycle to");
        return;
    };

    commands.insert_resource(PendingLevel::viewer(id));
    next.set(AppState::Loading);
}

/// The level after `current` in catalog order, wrapping.
///
/// Returns `None` only for an empty catalog: a one-level catalog cycles to
/// itself, which reloads, and that is a useful thing for `Tab` to do.
pub fn next_level(catalog: &LevelCatalog, current: Option<LevelId>) -> Option<LevelId> {
    if catalog.levels.is_empty() {
        return None;
    }

    let index = current
        .and_then(|id| catalog.levels.iter().position(|entry| entry.id == id))
        .map(|index| (index + 1) % catalog.levels.len())
        .unwrap_or(0);

    Some(catalog.levels[index].id)
}

/// The `Screenshot` action (F12 by default) writes a PNG of the window into
/// `<cache>/screenshots/`.
///
/// Screenshots of the levels are derived from game data — so the viewer takes them
/// rather than the developer's window manager, and they go to the cache,
/// outside the repository, rather than into whatever directory the game was
/// started from. Without a cache (the no-data screen) they go there anyway.
fn take_screenshot(
    mut commands: Commands,
    input: crate::ui::ActionInput,
    loaded: Option<Res<LoadedLevel>>,
    catalog: Option<Res<LevelCatalog>>,
    paths: Option<Res<bft_assets::DataPaths>>,
) {
    if !input.just_pressed(crate::ui::Action::Screenshot) {
        return;
    }

    let name = loaded
        .and_then(|loaded| {
            catalog
                .as_ref()?
                .level(loaded.id)
                .map(|entry| entry.name.clone())
        })
        .unwrap_or_else(|| "open-bft".to_string());
    let path = screenshot_path(
        paths.as_ref().map(|paths| paths.cache.path.as_path()),
        &name,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default(),
    );

    if let Some(directory) = path.parent()
        && let Err(e) = std::fs::create_dir_all(directory)
    {
        warn!("cannot create {}: {e}", directory.display());
        return;
    }

    info!("writing {}", path.display());
    commands
        .spawn(Screenshot::primary_window())
        .observe(save_to_disk(path));
}

/// Where a screenshot of `name` taken at `now` goes.
///
/// The name carries the time to the millisecond, so pressing the key twice
/// never overwrites the first picture.
pub fn screenshot_path(
    cache: Option<&std::path::Path>,
    name: &str,
    now: std::time::Duration,
) -> std::path::PathBuf {
    let file = format!("{name}-{}.png", now.as_millis());

    match cache {
        Some(cache) => cache.join("screenshots").join(file),
        None => std::path::PathBuf::from(file),
    }
}

/// Capture the front end and leave, for `play --screenshot`.
fn front_end_screenshot(
    mut commands: Commands,
    run: Res<ScreenshotRun>,
    mut countdown: ResMut<ScreenshotCountdown>,
    mut exit: MessageWriter<AppExit>,
) {
    countdown.frames += 1;

    // The menu has no assets to stream, so it settles far sooner than a
    // level does; a fraction of the level budget is plenty.
    let settle = (run.settle_frames / 4).max(10);

    if countdown.frames == settle {
        let path = run.directory.join("front-end.png");
        info!("writing {}", path.display());
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        countdown.captured = true;
    }
    if countdown.frames > settle + 10 {
        exit.write(AppExit::Success);
    }
}

/// Start the countdown again for a newly opened level.
fn reset_screenshot_countdown(mut commands: Commands) {
    commands.insert_resource(ScreenshotCountdown::default());
}

/// Drive an automated `--screenshot` run: wait, capture, move on.
fn automatic_screenshot(
    mut commands: Commands,
    run: Res<ScreenshotRun>,
    mut countdown: ResMut<ScreenshotCountdown>,
    loaded: Option<Res<LoadedLevel>>,
    catalog: Res<LevelCatalog>,
    mut next: ResMut<NextState<AppState>>,
    mut exit: MessageWriter<AppExit>,
) {
    countdown.frames += 1;

    if !countdown.captured {
        if countdown.frames < run.settle_frames {
            return;
        }

        let Some(loaded) = loaded.as_ref() else {
            return;
        };
        let name = catalog
            .level(loaded.id)
            .map(|entry| entry.name.clone())
            .unwrap_or_else(|| loaded.id.to_string());
        let path = run.directory.join(format!("{name}.png"));

        info!("writing {}", path.display());
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        countdown.captured = true;
        return;
    }

    // A few frames after the request so the capture is actually written
    // before the app moves on or exits.
    if countdown.frames < run.settle_frames + 10 {
        return;
    }

    let current = loaded.as_ref().map(|loaded| loaded.id);
    let first = catalog.levels.first().map(|entry| entry.id);

    match (run.all_levels, next_level(&catalog, current)) {
        // Back at the start means every level has had its turn.
        (true, Some(id)) if Some(id) != first => {
            commands.insert_resource(PendingLevel::viewer(id));
            next.set(AppState::Loading);
        }
        _ => {
            info!("screenshot run finished");
            exit.write(AppExit::Success);
        }
    }
}

/// `Esc` leaves the viewer for the menu.
fn leave_viewer(input: crate::ui::ActionInput, mut next: ResMut<NextState<AppState>>) {
    if input.just_pressed(crate::ui::Action::Pause) {
        next.set(AppState::FrontEnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::catalog::{LevelEntry, LevelKind};

    fn entry(id: u16) -> LevelEntry {
        LevelEntry {
            id: LevelId(id),
            name: format!("m{id}"),
            title_key: String::new(),
            gamez: format!("cache://gamez/m{id}-gamez.gamez.zip").into(),
            anim: None,
            reader: format!("cache://reader/m{id}-zrdr.rdr.zip").into(),
            zmap: None,
            zmap_is_placeholder: false,
            textures: Vec::new(),
            kind: LevelKind::Campaign { campaign: 1 },
        }
    }

    /// One partition cell for a test grid.
    fn cell(
        x: i32,
        z: i32,
        y_min: f32,
        y_max: f32,
        nodes: &[u32],
    ) -> bft_sim::level::PartitionCell {
        bft_sim::level::PartitionCell {
            x,
            z,
            y_min,
            y_max,
            nodes: nodes.to_vec(),
        }
    }

    fn catalog(ids: &[u16]) -> LevelCatalog {
        LevelCatalog {
            levels: ids.iter().copied().map(entry).collect(),
            ..default()
        }
    }

    #[test]
    fn screenshots_go_to_the_cache_and_never_collide() {
        let cache = std::path::Path::new("/cache");
        let first = screenshot_path(Some(cache), "m1", std::time::Duration::from_millis(1_000));
        let second = screenshot_path(Some(cache), "m1", std::time::Duration::from_millis(1_001));

        assert_eq!(first, cache.join("screenshots").join("m1-1000.png"));
        assert_ne!(first, second);
        assert_eq!(
            screenshot_path(None, "open-bft", std::time::Duration::from_millis(5)),
            std::path::PathBuf::from("open-bft-5.png")
        );
    }

    #[test]
    fn tab_walks_the_catalog_and_wraps() {
        let catalog = catalog(&[1, 2, 13]);

        assert_eq!(next_level(&catalog, Some(LevelId(1))), Some(LevelId(2)));
        assert_eq!(next_level(&catalog, Some(LevelId(2))), Some(LevelId(13)));
        assert_eq!(next_level(&catalog, Some(LevelId(13))), Some(LevelId(1)));
    }

    /// A level that is not in the catalog (it failed to load, say) must not
    /// strand `Tab`; starting over at the first level is the useful answer.
    #[test]
    fn an_unknown_current_level_starts_the_cycle_over() {
        assert_eq!(
            next_level(&catalog(&[1, 2]), Some(LevelId(9))),
            Some(LevelId(1))
        );
        assert_eq!(next_level(&catalog(&[1, 2]), None), Some(LevelId(1)));
    }

    #[test]
    fn an_empty_catalog_has_nowhere_to_cycle() {
        assert_eq!(next_level(&catalog(&[]), None), None);
    }

    /// The camera opens behind the player, looking the way they face, so the
    /// first thing on screen is what the mission opens on.
    #[test]
    fn the_start_view_is_over_the_players_shoulder() {
        // m1: on the shore by the hangar, facing 166.5 degrees.
        let start = PlayerStart {
            position: Vec3::new(2222.1, 3.9, 1294.2),
            heading_deg: 166.5,
        };

        let (eye, target) = start_viewpoint(&start);

        assert!(eye.y > start.position.y, "the camera is above the player");
        // Behind: the eye is on the opposite side of the player from where
        // they are looking.
        let to_eye = (eye - start.position).with_y(0.0).normalize();
        assert!(
            to_eye.dot(start.facing()) < 0.0,
            "the camera should be behind the player, not in front"
        );
        // And it looks past them, not at their back.
        let to_target = (target - eye).with_y(0.0).normalize();
        assert!(
            to_target.dot(start.facing()) > 0.9,
            "the view should run the way the player faces"
        );
    }

    /// The `bft_00` node is a build-script placeholder ~240 units from the
    /// real m1 start, and an identity at the origin in the other twelve
    /// levels. Using it put the camera over open water; the reader value
    /// puts it on the shore.
    #[test]
    fn the_reader_start_is_not_the_node_placeholder() {
        let node_placeholder = Vec3::new(2417.0, 0.0, 1117.0);
        let reader = PlayerStart {
            position: Vec3::new(2222.1, 3.9, 1294.2),
            heading_deg: 166.5,
        };

        assert!(
            reader.position.distance(node_placeholder) > 200.0,
            "the two disagree by enough to matter"
        );
    }

    /// m1's player start is on the shore facing open sea, so a camera that
    /// always stands off along +z opens on empty water with the level behind
    /// it. Standing off away from the centre puts the level in frame.
    #[test]
    fn the_camera_looks_in_towards_the_level_not_out_of_it() {
        let start = Vec3::new(2417.0, 0.0, 1117.0);
        let centre = (M1_BOUNDS.0 + M1_BOUNDS.1) * 0.5;
        let (eye, target) = viewpoint(Some(M1_BOUNDS), Some(start), Some(M1_FOG_NEAR));

        // The eye is further from the centre than the target it looks at, so
        // the view direction points back into the level.
        assert!(
            eye.with_y(0.0).distance(centre) > target.distance(centre),
            "the camera should stand outside the target and look in"
        );
    }

    /// m1's terrain extent (docs/data/coordinates.md), which is what the camera has to
    /// frame. Note how far it is from the world node's *declared* area of
    /// 0–4096 x 0–4352: framing by the declaration puts the camera 875 units
    /// out, past the fog, which is why the bounds come from the spawned
    /// geometry instead.
    const M1_BOUNDS: (Vec3, Vec3) = (
        Vec3::new(1287.0, 0.0, 1201.0),
        Vec3::new(2769.0, 120.0, 3424.0),
    );
    /// Where m1's fog starts (`fog_range` 550–650).
    const M1_FOG_NEAR: f32 = 550.0;

    /// The camera has to end up near m1's player start, not at the origin
    /// (docs/data/coordinates.md).
    #[test]
    fn the_camera_frames_the_player_start_when_the_level_has_one() {
        let start = Vec3::new(2417.0, 0.0, 1117.0);
        let (eye, target) = viewpoint(Some(M1_BOUNDS), Some(start), Some(M1_FOG_NEAR));

        assert_eq!(target, start);
        assert!(
            eye.distance(start) < 1000.0,
            "the camera should frame the start, not sit across the level; got {eye:?}"
        );
        assert!(eye.y > start.y, "the camera looks down at the level");
    }

    /// Without a placed player start — which is twelve of the thirteen
    /// levels — the centre of the spawned geometry is the next best thing.
    #[test]
    fn the_camera_falls_back_to_the_centre_of_the_geometry() {
        let (_, target) = viewpoint(Some(M1_BOUNDS), None, Some(M1_FOG_NEAR));

        assert_eq!(target, (M1_BOUNDS.0 + M1_BOUNDS.1) * 0.5);
    }

    /// A camera placed by area alone lands outside m1's 650-unit fog far
    /// plane and sees nothing but fog colour, which is what the first run of
    /// this viewer actually showed.
    #[test]
    fn the_framing_stays_inside_the_levels_own_fog() {
        let start = Vec3::new(2417.0, 0.0, 1117.0);
        let (unfogged, _) = viewpoint(Some(M1_BOUNDS), Some(start), None);
        let (fogged, target) = viewpoint(Some(M1_BOUNDS), Some(start), Some(M1_FOG_NEAR));

        assert!(
            unfogged.distance(target) > fogged.distance(target),
            "the fog clamp has to actually pull the camera in"
        );
        assert!(
            fogged.distance(target) < M1_FOG_NEAR,
            "the fog-aware framing has to stay in clear air"
        );
    }

    /// A level with nothing spawned yet must still produce a usable camera
    /// rather than one at an undefined position.
    #[test]
    fn a_level_with_no_geometry_still_gets_a_framed_camera() {
        let (eye, target) = viewpoint(None, None, None);

        assert_eq!(target, Vec3::ZERO);
        assert!(eye.distance(target) >= MIN_FRAMING);
    }

    /// The camera must clear the tallest terrain, or a level whose middle is
    /// lower than its edges opens from inside a cliff (m11 did).
    #[test]
    fn the_camera_clears_the_tallest_geometry() {
        let bounds = (
            Vec3::new(0.0, 0.0, 0.0),
            // A tall level in a small footprint: the framing distance is
            // small but the terrain is not.
            Vec3::new(200.0, 400.0, 200.0),
        );
        let (eye, _) = viewpoint(Some(bounds), None, Some(320.0));

        assert!(
            eye.y > bounds.1.y,
            "the camera should be above the terrain, not inside it; got {}",
            eye.y
        );
    }

    /// A grid whose occupied cells sit in one corner must bound that corner,
    /// not the whole declared area: m11's grid covers 0–2560 in x and z but
    /// only part of it holds anything.
    #[test]
    fn the_bounds_are_the_occupied_cells_not_the_whole_grid() {
        let grid = PartitionGrid::from_cells(
            2,
            2,
            vec![
                cell(0, 0, -10.0, 20.0, &[1]),
                // Empty: spans its column but holds nothing, so it must not
                // stretch the box.
                cell(256, 0, -999.0, 999.0, &[]),
                cell(0, 256, -5.0, 40.0, &[2, 3]),
                cell(256, 256, 0.0, 0.0, &[]),
            ],
        );

        let (min, max) = level_bounds(&grid, &AxisConvention::GAMEZ_DEFAULT).unwrap();

        assert_eq!(min, Vec3::new(0.0, -10.0, 0.0));
        // x stops at one cell's width; z runs to the second row's far edge.
        assert_eq!(max, Vec3::new(256.0, 40.0, 512.0));
    }

    #[test]
    fn a_grid_with_nothing_in_it_has_no_bounds() {
        let grid = PartitionGrid::from_cells(1, 1, vec![cell(0, 0, 0.0, 0.0, &[])]);

        assert_eq!(level_bounds(&grid, &AxisConvention::GAMEZ_DEFAULT), None);
    }
}
