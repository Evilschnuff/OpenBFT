//! [`AppState::InGame`]: build the vehicle table, spawn the player, tear down.
//!
//! This is the client half of the player's vehicle: the table has to be built from
//! assets, and `bft-sim` has no asset server of its own to wait on. The
//! sequence is
//!
//! 1. at startup, ask for the global reader archive and the two tuning files
//!    (they are the same for every level, so they load once, not per level);
//! 2. when all three have landed, build the [`VehicleTable`] for the current
//!    [`Difficulty`] and hand `player.zrd`'s constants to the physics'
//!    [`PhysicsTuning`](bft_sim::physics::PhysicsTuning);
//! 3. on entering `InGame`, spawn the player vehicle at the level's start and
//!    a [`ChaseCamera`](crate::chase_camera::ChaseCamera) behind it.
//!
//! The table is rebuilt when the difficulty changes, because difficulty
//! selects a whole reader file rather than scaling values (READER-SCHEMA §1).

use bevy::prelude::*;
use bft_assets::anim::AnimSet;
use bft_assets::catalog::LevelCatalog;
use bft_assets::reader::ReaderData;
use bft_sim::cinematic::hand_off_pose;
use bft_sim::damage::{AimResult, Health, Team};
use bft_sim::mission::Progress;
use bft_sim::pickups::{PickupTable, build_pickup_table, pickup_placements, spawn_pickups};
use bft_sim::states::{AppState, GameMode};
use bft_sim::vehicle::{
    Difficulty, FormSet, PlayerVehicle, VehicleOverrides, VehicleTable, apply_player_constants,
    build_vehicle_table, spawn_player_vehicle,
};
use bft_sim::weapons::{
    DEFAULT_TETHER_MAX_ALTITUDE, Muzzle, WeaponOverrides, WeaponTable, build_weapon_table,
    player_loadout,
};

use crate::camera_tuning::CameraOverrides;
use crate::chase_camera::spawn_chase_camera;
use crate::level_view::LoadedLevel;
use crate::viewer::ScreenshotRun;

/// Where `assets/tuning/vehicle.ron` lives, as the asset server names it.
pub const VEHICLE_TUNING: &str = "tuning/vehicle.ron";
/// Where `assets/tuning/camera.ron` lives.
pub const CAMERA_TUNING: &str = "tuning/camera.ron";
/// Where `assets/tuning/weapons.ron` lives.
pub const WEAPONS_TUNING: &str = "tuning/weapons.ron";

/// How far above the reader's start position the vehicle is placed.
///
/// // approximation: `aiv.zrd`'s y is "ground height" (READER-SCHEMA §3.2)
/// and the hull origin is at the track contact patch, so dropping it in
/// exactly on the ground interpenetrates the terrain for a tick. A couple of
/// metres lets the suspension catch it.
pub const SPAWN_CLEARANCE: f32 = 2.0;

/// The handles the table is built from, held so they are not dropped.
#[derive(Resource, Debug, Clone)]
pub struct TuningHandles {
    /// The global reader archive: `vehicle*.zrd` and `player.zrd`.
    pub reader: Option<Handle<ReaderData>>,
    pub vehicle: Handle<VehicleOverrides>,
    pub camera: Handle<CameraOverrides>,
    pub weapons: Handle<WeaponOverrides>,
    /// Which difficulty the current [`VehicleTable`] was built for, so a
    /// change rebuilds it.
    pub built_for: Option<Difficulty>,
}

/// The `InGame` state: tuning, the player vehicle and its camera.
pub struct PlayPlugin;

impl Plugin for PlayPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<VehicleOverrides>()
            .init_asset::<CameraOverrides>()
            .init_asset::<WeaponOverrides>()
            .register_asset_loader(bft_assets::RonAssetLoader::<VehicleOverrides>::new(&[
                "vehicle.ron",
            ]))
            .register_asset_loader(bft_assets::RonAssetLoader::<CameraOverrides>::new(&[
                "camera.ron",
            ]))
            .register_asset_loader(bft_assets::RonAssetLoader::<WeaponOverrides>::new(&[
                "weapons.ron",
            ]))
            .add_systems(Startup, request_tuning)
            .add_systems(Update, build_table)
            .add_systems(OnEnter(AppState::InGame), (spawn_player, place_pickups))
            .add_systems(OnExit(AppState::InGame), despawn_player)
            // `play <level> --screenshot` captures the vehicle unattended,
            // with nobody holding a key down. The viewer's own run
            // does the same for the viewer and the menu; the countdown is separate
            // because that one is private to `viewer`.
            .add_systems(
                Update,
                capture_in_game
                    .run_if(in_state(AppState::InGame))
                    .run_if(resource_exists::<ScreenshotRun>),
            );
    }
}

/// Ask for the reader archive and the two tuning files.
fn request_tuning(mut commands: Commands, catalog: Res<LevelCatalog>, server: Res<AssetServer>) {
    let reader = catalog
        .global_reader
        .clone()
        .map(|path| server.load::<ReaderData>(path));

    if reader.is_none() {
        warn!(
            "no global reader archive in the cache: vehicles will use the \
             built-in `player.zrd` values and no kinds. Run `open-bft extract`."
        );
    }

    commands.insert_resource(TuningHandles {
        reader,
        vehicle: server.load::<VehicleOverrides>(VEHICLE_TUNING),
        camera: server.load::<CameraOverrides>(CAMERA_TUNING),
        weapons: server.load::<WeaponOverrides>(WEAPONS_TUNING),
        built_for: None,
    });
}

/// Build the table once its inputs are loaded, and again when the difficulty
/// changes.
fn build_table(
    mut commands: Commands,
    handles: Option<ResMut<TuningHandles>>,
    difficulty: Res<Difficulty>,
    archives: Res<Assets<ReaderData>>,
    overrides: Res<Assets<VehicleOverrides>>,
    weapon_overrides: Res<Assets<WeaponOverrides>>,
) {
    let Some(mut handles) = handles else {
        return;
    };
    if handles.built_for == Some(*difficulty) {
        return;
    }

    let Some(reader) = handles.reader.as_ref().and_then(|h| archives.get(h)) else {
        return;
    };
    // An absent override asset is not an error only once the load has
    // settled; `Assets::get` returning `None` while it is still in flight is
    // the normal case, so the table waits for it rather than building
    // without it and never rebuilding.
    let Some(overrides) = overrides.get(&handles.vehicle) else {
        return;
    };
    let Some(weapon_overrides) = weapon_overrides.get(&handles.weapons) else {
        return;
    };

    match build_vehicle_table(reader, *difficulty, overrides) {
        Ok(table) => {
            info!(
                "vehicle table: {} kind(s) from {}, gravity {} m/s²",
                table.kinds.len(),
                difficulty.vehicle_entry(),
                table.player.gravity
            );
            // The physics owns `Gravity`; this only sets the tuning it reads.
            apply_player_constants(&mut commands, &table.player);
            commands.insert_resource(table);
            handles.built_for = Some(*difficulty);

            // The weapon and pickup tables are difficulty-independent —
            // `weapons.zrd` and `pickup.zrd` have no `_easy`/`_hard`
            // variants — but they are built here because this is where the
            // reader archive has landed.
            //
            // `DEFAULT_TETHER_MAX_ALTITUDE` rather than the interp value:
            // `WeaponSetMaxTetherAltitude` is 30.0 in all 13 missions
            // and nothing loads the interp scripts
            // yet. Swap in `runtime_bindings(..).max_tether_altitude` when
            // something does.
            match build_weapon_table(reader, weapon_overrides, DEFAULT_TETHER_MAX_ALTITUDE) {
                Ok(weapons) => {
                    info!("weapon table: {} weapon(s)", weapons.weapons.len());
                    commands.insert_resource(weapons);
                }
                Err(e) => error!("cannot build the weapon table: {e}"),
            }

            match build_pickup_table(reader) {
                Ok(pickups) => {
                    info!("pickup table: {} kind(s)", pickups.kinds.len());
                    commands.insert_resource(pickups);
                }
                Err(e) => error!("cannot build the pickup table: {e}"),
            }
        }
        Err(e) => {
            error!("cannot build the vehicle table: {e}");
            // Marked as built so the error is logged once rather than every
            // frame; changing the difficulty retries.
            handles.built_for = Some(*difficulty);
        }
    }
}

/// A development override for the player start: `x,z` or `x,y,z[,heading]`.
///
/// `BFT_START=2131,6,1618,180 cargo run -- play m1` drops the player there
/// instead of at `aiv.zrd`'s `bft_00`. It exists because checking a change by
/// eye often means standing somewhere specific — next to m1's nearest enemy
/// group, which is 336 m from the real start and so off screen from it — and
/// the alternative is driving there by hand every time.
///
/// Unset is the normal case and the only one a player ever sees. A malformed
/// value warns and is ignored rather than moving the start somewhere arbitrary.
pub const START_OVERRIDE_VAR: &str = "BFT_START";

/// Read [`START_OVERRIDE_VAR`], or `None`.
fn start_override() -> Option<(Vec3, Quat)> {
    let text = std::env::var(START_OVERRIDE_VAR).ok()?;

    let numbers: Option<Vec<f32>> = text
        .split(',')
        .map(|part| part.trim().parse().ok())
        .collect();

    let (position, heading_deg) = match numbers.as_deref() {
        // `x,z`: the ground height is whatever the suspension finds, so a
        // generous y and a drop in is the useful default.
        Some([x, z]) => (Vec3::new(*x, START_OVERRIDE_ALTITUDE, *z), 0.0),
        Some([x, y, z]) => (Vec3::new(*x, *y, *z), 0.0),
        Some([x, y, z, heading]) => (Vec3::new(*x, *y, *z), *heading),
        _ => {
            warn!("{START_OVERRIDE_VAR}=`{text}` is not `x,z` or `x,y,z[,heading]`; ignoring it");
            return None;
        }
    };

    info!("{START_OVERRIDE_VAR} puts the player at {position:?} facing {heading_deg}°");

    Some((position, Quat::from_rotation_y(heading_deg.to_radians())))
}

/// How high an `x,z` override starts, in metres.
///
/// Above m1's tallest terrain, so the vehicle always falls onto the ground
/// rather than starting inside it.
const START_OVERRIDE_ALTITUDE: f32 = 200.0;

/// Spawn the player's vehicle at the level's start, and a camera behind it.
#[allow(clippy::too_many_arguments)]
pub fn spawn_player(
    mut commands: Commands,
    table: Option<Res<VehicleTable>>,
    weapons: Option<Res<WeaponTable>>,
    pickups: Option<Res<PickupTable>>,
    loaded: Option<Res<LoadedLevel>>,
    readers: Res<Assets<ReaderData>>,
    cameras: Res<Assets<CameraOverrides>>,
    handles: Option<Res<TuningHandles>>,
    progress: Option<Res<Progress>>,
    mode: Option<Res<State<GameMode>>>,
    anims: Res<Assets<AnimSet>>,
) {
    let Some(table) = table else {
        error!("entering InGame with no vehicle table; the reader archive did not load");
        return;
    };

    let start = loaded
        .as_ref()
        .and_then(|loaded| loaded.reader.as_ref())
        .and_then(|handle| readers.get(handle))
        .and_then(ReaderData::player_start);

    // Where the intro hands over: in the original the player *is* `bft_00`,
    // and m2–m6's start animations leave it in the landed VTOL's bay. Two of
    // the five `aiv.zrd` starts sit 15–18 m off, outside the ramp. m1's intro does not place it — the BFT stands on the
    // trailer, which is `aiv.zrd`'s start.
    let hand_off = loaded
        .as_ref()
        .and_then(|loaded| loaded.anim.as_ref())
        .and_then(|handle| anims.get(handle))
        .and_then(hand_off_pose);

    let dropped_in = |(position, rotation): (Vec3, Quat)| Transform {
        translation: position + Vec3::Y * SPAWN_CLEARANCE,
        rotation,
        ..default()
    };
    let at = if let Some(start) = start_override() {
        dropped_in(start)
    } else if let Some(pose) = hand_off {
        // On the bay floor exactly, as the script places it: the hull is
        // parked for the fly-in and the reset puts it back here at hand-off.
        pose
    } else if let Some(start) = start {
        dropped_in((start.position, start.rotation()))
    } else {
        // The last fallback. It is a bad place to play
        // from — the whole point of reading `aiv.zrd` is that the node is 240
        // units out or at the origin — so it is loud.
        warn!("no player start for this level; spawning at the origin");
        Transform::from_xyz(0.0, SPAWN_CLEARANCE, 0.0)
    };

    // Track only: the amphib, hover and sub chips are pickups now
    // (`PUP_AMPHIB`, `PUP_HOVER`, `PUP_SUB` — `pu037`-`pu039`), so the
    // campaign unlocks its forms by finding them, the way the original does.
    // Deathmatch maps place the chips too.
    let vehicle = spawn_player_vehicle(&mut commands, at, &table, FormSet::track_only());

    // The weapons half of the player: the RFPG, and in the campaign whatever
    // the save says was mounted by the missions before. Every other weapon
    // is carried unmounted, so its ammunition can be picked up before the
    // gun is. Deathmatch starts everyone equal.
    let player_def = table.player_def();
    let campaign = mode.is_none_or(|mode| *mode.get() == GameMode::Campaign);
    let saved = progress
        .as_ref()
        .filter(|_| campaign)
        .map(|progress| progress.mounted.as_slice())
        .unwrap_or_default();
    // `--unlock-all` is a test run: everything mounted and full.
    let full = progress
        .as_ref()
        .is_some_and(|progress| progress.unlock_all);
    let loadout = match (&weapons, player_def) {
        (Some(weapons), Some(def)) => player_loadout(def, weapons, saved, full),
        _ => {
            warn!("no weapon table; the player spawns unarmed");
            default()
        }
    };
    if pickups.is_none() {
        warn!("no pickup table; nothing placed in the level can be collected");
    }

    let health = player_def.map(|def| def.health).unwrap_or(140.0);

    commands.entity(vehicle).insert((
        loadout,
        Health::new(health),
        Team::PLAYER,
        AimResult::default(),
        // The fallback. The player model overwrites this with its own
        // `fpnt_c` as soon as the subtree is cloned; a level with no `bft_00`
        // template keeps it, sitting at the turret's aim height, forward of
        // the hull's centre and clear of its collider.
        Muzzle(Vec3::new(
            0.0,
            player_def.map(|def| def.aim_y[0]).unwrap_or(1.5),
            -3.5,
        )),
    ));

    let camera_overrides = handles
        .as_ref()
        .and_then(|handles| cameras.get(&handles.camera))
        .cloned()
        .unwrap_or_default();

    spawn_chase_camera(&mut commands, vehicle, &camera_overrides);

    info!(
        "player spawned at {:.1}, {:.1}, {:.1}",
        at.translation.x, at.translation.y, at.translation.z
    );
}

/// Put the level's pickups where `puppies.zrd` says they go.
///
/// Placement is per mission and per difficulty (READER-SCHEMA §3.5), so it
/// happens on entering the level rather than when the tables are built.
fn place_pickups(
    mut commands: Commands,
    difficulty: Res<Difficulty>,
    loaded: Option<Res<LoadedLevel>>,
    archives: Res<Assets<ReaderData>>,
) {
    let Some(archive) = loaded
        .as_ref()
        .and_then(|loaded| loaded.reader.as_ref())
        .and_then(|handle| archives.get(handle))
    else {
        warn!("no mission reader archive; this level gets no pickups");
        return;
    };

    match pickup_placements(archive, *difficulty) {
        Ok(placements) => {
            let spawned = spawn_pickups(&mut commands, &placements);
            info!("placed {} pickup(s)", spawned.len());
        }
        Err(e) => error!("cannot place this level's pickups: {e}"),
    }
}

/// Take the player, its camera and the level's pickups down when the game
/// ends.
fn despawn_player(
    mut commands: Commands,
    players: Query<Entity, With<PlayerVehicle>>,
    cameras: Query<Entity, With<crate::chase_camera::ChaseCamera>>,
    pickups: Query<Entity, With<bft_sim::pickups::Pickup>>,
    projectiles: Query<Entity, With<bft_sim::weapons::Projectile>>,
) {
    for entity in players
        .iter()
        .chain(cameras.iter())
        .chain(pickups.iter())
        .chain(projectiles.iter())
    {
        commands.entity(entity).despawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The paths the tuning loaders are registered for have to match the
    /// files that exist, or the table waits for an asset that never lands.
    #[test]
    fn the_tuning_paths_name_the_files_in_the_repo() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets")
            .canonicalize()
            .expect("the assets directory is in the repo");

        assert!(root.join(VEHICLE_TUNING).is_file(), "{VEHICLE_TUNING}");
        assert!(root.join(CAMERA_TUNING).is_file(), "{CAMERA_TUNING}");
        assert!(root.join(WEAPONS_TUNING).is_file(), "{WEAPONS_TUNING}");
    }
}

/// How long the in-game capture has been waiting.
#[derive(Resource, Debug, Clone, Copy, Default)]
struct InGameCountdown {
    frames: u32,
    captured: bool,
}

/// Write a PNG of the game once it has settled, then leave.
///
/// The same settle budget as the viewer's: meshes, materials and textures all
/// land over several frames, and the vehicle needs a few more to drop onto
/// its suspension rather than being caught mid-air.
fn capture_in_game(
    mut commands: Commands,
    run: Res<ScreenshotRun>,
    mut countdown: Local<InGameCountdown>,
    mut exit: MessageWriter<AppExit>,
) {
    countdown.frames += 1;

    if !countdown.captured {
        if countdown.frames < run.settle_frames {
            return;
        }

        let path = run.directory.join("in-game.png");
        info!("writing {}", path.display());
        commands
            .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
            .observe(bevy::render::view::screenshot::save_to_disk(path));
        countdown.captured = true;
        return;
    }

    // A few frames after the request so the capture is written before the
    // app exits.
    if countdown.frames > run.settle_frames + 10 {
        info!("screenshot run finished");
        exit.write(AppExit::Success);
    }
}
