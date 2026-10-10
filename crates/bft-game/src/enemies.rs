//! The client half of the AI: load `ai.ron`, build the table, spawn the
//! level's enemies and turrets, hide the prototypes they are cloned from.
//!
//! `bft-sim` owns every decision an enemy makes; none of that is here. What
//! is here is the wiring the sim cannot do for itself, for the same reason
//! `play.rs` exists for the player's vehicle: the AI table is built from reader assets
//! and `bft-sim` has no asset server to wait on.
//!
//! The sequence is
//!
//! 1. at startup, ask for `assets/tuning/ai.ron` (one file for every level);
//! 2. when it lands, install the [`SteeringTuning`] it describes;
//! 3. on entering `InGame`, build the [`AiTable`] from the *mission's* reader
//!    archive and spawn what it places;
//! 4. on leaving, take the enemies down with the level.
//!
//! # Hiding the prototypes
//!
//! The level spawner spawns every one of a level's 174–220 root trees, including the
//! ~170 that are prototypes rather than scenery, so that anim and interp can
//! address their nodes by name. `bft-sim` marks those roots
//! [`Template`](bft_sim::ai::Template) and this module hides them —
//! `Visibility` is a `bevy_render` type and the sim builds without a renderer
//! (NFR-4), so the marker is there and the hiding is here.

use bevy::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::level::LevelData;
use bft_assets::reader::ReaderData;
use bft_sim::ai::templates::LevelTemplates;
use bft_sim::ai::{
    AiBrain, AiOverrides, AiTable, EnemySpawned, EnemyTables, EnemyTemplates, SteeringTuning,
    Template, build_ai_table, spawn_enemy, spawn_turrets,
};
use bft_sim::level::{NodeIndex, NodeName};
use bft_sim::states::AppState;
use bft_sim::vehicle::VehicleTable;
use bft_sim::vehicle::tuning::Difficulty;
use bft_sim::weapons::WeaponTable;

use crate::level_view::LoadedLevel;

/// Where `assets/tuning/ai.ron` lives, as the asset server names it.
pub const AI_TUNING: &str = "tuning/ai.ron";

/// The `ai.ron` handle, held so it is not dropped, and whether it is applied.
#[derive(Resource, Debug, Clone)]
pub struct AiTuningHandle {
    pub overrides: Handle<AiOverrides>,
    /// Whether [`SteeringTuning`] has been built from it yet.
    pub applied: bool,
}

/// The enemy half of `GamePlugin`.
pub struct EnemiesPlugin;

impl Plugin for EnemiesPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<AiOverrides>()
            .register_asset_loader(bft_assets::RonAssetLoader::<AiOverrides>::new(&["ai.ron"]))
            .add_systems(Startup, request_ai_tuning)
            .add_systems(Update, (apply_ai_tuning, restyle_brains).chain())
            .add_observer(hide_template_root)
            .add_observer(make_clone_visible)
            // After `play.rs`'s `spawn_player`, which is in the same
            // `OnEnter` set: an enemy's perception looks for a `Team(0)`
            // vehicle, and one spawned on the same frame is found either
            // way, but the *log line* reads better in order.
            .add_systems(
                OnEnter(AppState::InGame),
                populate_level.after(crate::play::spawn_player),
            )
            .add_systems(
                Update,
                report_enemy_placement.run_if(in_state(AppState::InGame)),
            )
            .add_systems(OnExit(AppState::InGame), despawn_enemies);
    }
}

/// Ask for `ai.ron`.
fn request_ai_tuning(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(AiTuningHandle {
        overrides: server.load::<AiOverrides>(AI_TUNING),
        applied: false,
    });
}

/// Install the steering knobs once the file has landed.
///
/// Until it does, `AiPlugin`'s compiled defaults drive — which is what a
/// headless server and a unit test use anyway, so a missing file is a warning
/// at most and never a reason not to play.
fn apply_ai_tuning(
    mut commands: Commands,
    handle: Option<ResMut<AiTuningHandle>>,
    overrides: Res<Assets<AiOverrides>>,
) {
    let Some(mut handle) = handle else {
        return;
    };
    if handle.applied {
        return;
    }
    let Some(overrides) = overrides.get(&handle.overrides) else {
        return;
    };

    let tuning = overrides.steering();
    info!(
        "ai tuning: {} tick reaction, {:.2} lead, {:.1}° aim error",
        tuning.reaction_ticks, tuning.lead_factor, tuning.aim_error_deg
    );

    commands.insert_resource(tuning);
    handle.applied = true;
}

/// Hand a changed [`SteeringTuning`] to every brain already in the world.
///
/// A brain carries its own copy — the per-kind layer of `ai.ron` goes on top
/// of the mission-wide values at spawn — so editing the resource alone, as
/// the dev panel does, would only reach enemies spawned afterwards. The
/// per-kind layer is re-applied here the way `populate_level` applies it.
pub fn restyle_brains(
    tuning: Option<Res<SteeringTuning>>,
    handle: Option<Res<AiTuningHandle>>,
    overrides: Res<Assets<AiOverrides>>,
    mut brains: Query<(&mut AiBrain, &bft_sim::ai::Enemy)>,
) {
    let Some(tuning) = tuning else {
        return;
    };
    if !tuning.is_changed() {
        return;
    }
    let overrides = handle
        .as_ref()
        .and_then(|handle| overrides.get(&handle.overrides));

    for (mut brain, enemy) in &mut brains {
        let mut steering = *tuning;
        if let Some(overrides) = overrides {
            overrides.for_kind(enemy.kind.as_str()).apply(&mut steering);
        }
        if brain.steering != steering {
            brain.steering = steering;
        }
    }
}

/// Build the mission's AI table and spawn what it places.
///
/// Per mission and per difficulty, so it happens on entering the level rather
/// than when the global tables are built — the same split `place_pickups`
/// makes for `puppies.zrd`.
#[allow(clippy::too_many_arguments)]
fn populate_level(
    mut commands: Commands,
    difficulty: Res<Difficulty>,
    loaded: Option<Res<LoadedLevel>>,
    archives: Res<Assets<ReaderData>>,
    levels: Res<Assets<LevelData>>,
    overrides: Res<Assets<AiOverrides>>,
    handle: Option<Res<AiTuningHandle>>,
    axes: Option<Res<AxisConvention>>,
    vehicles: Option<Res<VehicleTable>>,
    weapons: Option<Res<WeaponTable>>,
    tuning: Option<Res<SteeringTuning>>,
    nodes: Option<Res<NodeIndex>>,
    names: Query<(Entity, &NodeName)>,
    children: Query<&Children>,
    node_names: Query<&NodeName>,
    mut spawned: MessageWriter<EnemySpawned>,
) {
    let Some(loaded) = loaded else {
        return;
    };
    let Some(archive) = loaded
        .reader
        .as_ref()
        .and_then(|handle| archives.get(handle))
    else {
        warn!("no mission reader archive; this level gets no enemies");
        return;
    };

    let ai_overrides = handle
        .as_ref()
        .and_then(|handle| overrides.get(&handle.overrides))
        .cloned()
        .unwrap_or_default();

    let table = match build_ai_table(archive, *difficulty, &ai_overrides) {
        Ok(table) => table,
        Err(e) => {
            error!("cannot build this level's AI table: {e}");
            return;
        }
    };

    let Some(vehicles) = vehicles else {
        error!("entering InGame with no vehicle table; no enemies can spawn");
        commands.insert_resource(table);
        return;
    };

    let steering = tuning.map(|tuning| *tuning).unwrap_or_default();

    // The templates, where the level's own data has landed. A level with no
    // prototypes still spawns working enemies; they just have no model.
    let level = levels.get(&loaded.data);
    let axes = axes
        .map(|axes| *axes)
        .unwrap_or(AxisConvention::GAMEZ_DEFAULT);
    let templates = level.map(LevelTemplates::from_level);

    let enemy_templates = level
        .zip(templates.as_ref())
        .map(|(level, templates)| EnemyTemplates {
            level,
            axes: &axes,
            templates,
        });

    let mut count = 0usize;
    for (phase, placement) in table.enemies().enumerate() {
        let entity = spawn_enemy(
            &mut commands,
            placement,
            EnemyTables {
                vehicles: &vehicles,
                weapons: weapons.as_deref(),
                ai: &table,
            },
            enemy_templates.as_ref(),
            // The per-kind layer, so a drone and a trencher can steer
            // differently without either being in the game's own data.
            {
                let mut steering = steering;
                ai_overrides
                    .for_kind(placement.kind.as_str())
                    .apply(&mut steering);
                steering
            },
            // The perception phase: spread across the cadence so 84 enemies
            // do not all cast their line-of-sight rays on the same tick.
            phase as u32,
        );

        spawned.write(EnemySpawned(entity));
        count += 1;
    }

    // The turrets are nodes that already exist, so they are matched by name
    // against `ai.zrd`'s globs rather than spawned.
    let turret_nodes: Vec<(String, Entity)> = names
        .iter()
        .filter(|(_, name)| table.turret_for(&name.0).is_some())
        .map(|(entity, name)| (name.0.clone(), entity))
        .collect();

    let empty = NodeIndex::default();
    let turrets = spawn_turrets(
        &mut commands,
        nodes.as_deref().unwrap_or(&empty),
        &turret_nodes,
        &table,
        weapons.as_deref(),
        &children,
        &node_names,
    );

    info!(
        "{count} enemy vehicle(s) and {} turret(s) from {} ({} net(s), {} \
         turret def(s), {} template(s))",
        turrets.len(),
        bft_sim::ai::table::aiv_entry(*difficulty),
        table.nets.len(),
        table.turrets.len(),
        templates.as_ref().map(LevelTemplates::len).unwrap_or(0),
    );

    if let Some(templates) = templates {
        commands.insert_resource(templates);
    }
    commands.insert_resource(table);
}

/// Take the enemies and their brains down with the level.
///
/// The turret *nodes* are level geometry and go with the level root; only
/// their components would linger, and despawning the node would take a piece
/// of the level with it. So a turret is left alone here and the level's own
/// teardown removes it.
fn despawn_enemies(mut commands: Commands, enemies: Query<Entity, With<bft_sim::ai::Enemy>>) {
    let mut count = 0usize;
    for entity in &enemies {
        commands.entity(entity).despawn();
        count += 1;
    }

    commands.remove_resource::<AiTable>();
    commands.remove_resource::<LevelTemplates>();

    if count > 0 {
        info!("despawned {count} enemy vehicle(s)");
    }
}

/// Hide a prototype root, so it is not drawn where the build script parked it.
///
/// An observer on the marker rather than a pass over the level, so it runs
/// whoever spawned the root and in whatever order relative to
/// [`make_node_visible`](crate::level_view::make_node_visible) — `Visibility`
/// is inserted by that observer on every node, and `Hidden` here replaces it
/// on the ~170 roots that are prototypes. Their children keep
/// `Visibility::Inherited` and follow.
pub fn hide_template_root(add: On<Add, Template>, mut commands: Commands) {
    commands.entity(add.entity).insert(Visibility::Hidden);
}

/// Give a cloned template node a `Visibility`, which its prototype got from
/// [`make_node_visible`](crate::level_view::make_node_visible).
///
/// That observer keys on [`NodeId`](bft_sim::level::NodeId), which a clone
/// deliberately does not carry — an id names one node of the level and two
/// entities claiming it would make the index ambiguous. So the clone's nodes
/// need their own, or Bevy reports B0004 for every model under a parent with
/// no `Visibility` and nothing is drawn.
pub fn make_clone_visible(add: On<Add, NodeName>, mut commands: Commands) {
    commands
        .entity(add.entity)
        .insert_if_new(Visibility::default());
}

/// Report where the enemies actually ended up, once they have settled.
///
/// The check that matters is "a real level spawns its enemies at plausible
/// positions", and a screenshot answers that only for whatever is in frame.
/// This answers it for all 83: how far
/// each ended up from the `aiv.zrd` record that placed it, and how many got a
/// model out of the level's template subtrees.
///
/// Runs once, a second after the level comes up — long enough for the
/// suspension to catch them, short enough that no brain has driven anywhere.
fn report_enemy_placement(
    time: Res<Time>,
    mut done: Local<Option<f32>>,
    enemies: Query<(&bft_sim::ai::Enemy, &AiBrain, &Transform, Option<&Children>)>,
    models: Query<(), With<bft_sim::ai::ClonedFrom>>,
) {
    let elapsed = time.elapsed_secs();
    let started = *done.get_or_insert(elapsed);

    if elapsed - started < PLACEMENT_REPORT_DELAY || enemies.is_empty() {
        return;
    }
    // A sentinel far in the future, so this never runs twice.
    *done = Some(f32::MAX);

    let mut drifted = 0usize;
    let mut sunk = 0usize;
    let mut with_model = 0usize;
    let mut worst = 0.0f32;

    for (enemy, brain, transform, children) in &enemies {
        // `home` is the `aiv.zrd` position; the spawn adds `SPAWN_CLEARANCE`
        // above it and the suspension settles back down onto the ground.
        let offset = transform.translation - brain.home;
        let horizontal = offset.xz().length();

        worst = worst.max(horizontal);
        if horizontal > PLACEMENT_DRIFT_LIMIT {
            drifted += 1;
            debug!(
                "`{}` is {horizontal:.1} m from where aiv.zrd put it",
                enemy.instance
            );
        }
        if offset.y < -PLACEMENT_DRIFT_LIMIT {
            sunk += 1;
            debug!(
                "`{}` fell {:.1} m through the ground",
                enemy.instance, -offset.y
            );
        }
        if children.is_some_and(|children| children.iter().any(|child| models.contains(child))) {
            with_model += 1;
        }
    }

    info!(
        "enemy placement: {} of {} have a model, {drifted} drifted over {PLACEMENT_DRIFT_LIMIT} m \
         (worst {worst:.1}), {sunk} fell through the ground",
        with_model,
        enemies.iter().len(),
    );
}

/// How long after the level comes up the placement report runs, in seconds.
const PLACEMENT_REPORT_DELAY: f32 = 1.0;

/// How far an enemy may settle from its record before it is worth a line.
///
/// // approximation: the hull is 5.7 x 7.4 m and it is dropped in from
/// `SPAWN_CLEARANCE` above the reader's ground height, so sliding a hull's
/// length down a slope is ordinary. More than that means it was placed
/// somewhere it cannot rest.
const PLACEMENT_DRIFT_LIMIT: f32 = 8.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// The path the loader is registered for has to match the file in the
    /// repo, or the tuning waits for an asset that never lands.
    #[test]
    fn the_tuning_path_names_the_file_in_the_repo() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets")
            .canonicalize()
            .expect("the assets directory is in the repo");

        assert!(root.join(AI_TUNING).is_file(), "{AI_TUNING}");
    }

    /// And it has to parse, with the loader's own `deny_unknown_fields`, and
    /// the steering it describes has to be one a brain can run.
    ///
    /// The file need not be empty: the dev panel saves real tuning into it,
    /// as it does into `vehicle.ron`.
    #[test]
    fn the_shipped_tuning_file_parses() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets")
            .join(AI_TUNING);
        let text = std::fs::read_to_string(&path).expect("the file is readable");

        let overrides: AiOverrides =
            ron::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));

        let tuning = overrides.steering();
        assert!(tuning.reaction_ticks >= 1 && tuning.los_lost_ticks >= 1);
        assert!(
            tuning.fire_cone() > tuning.aim_error(),
            "or a brain never fires the shot its own error spoiled"
        );
    }
}
