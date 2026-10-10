//! Turrets against a real level: its geometry, its colliders, its `ai.zrd`.
//!
//! They skip (not fail) when `BFT_DATA_DIR` is unset or the cache has not
//! been built. No game data enters the repository.
//!
//! Build the cache first with `open-bft extract`.

use bevy::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::level::{LevelData, parse_gamez};
use bft_sim::ai::{AiTable, TurretAi, build_ai_table, spawn_turrets};
use bft_sim::damage::{Health, Team};
use bft_sim::level::{NodeIndex, NodeName};
use bft_sim::schedule::SIM_HZ;
use bft_sim::vehicle::tuning::Difficulty;
use bft_sim::weapons::{
    DEFAULT_TETHER_MAX_ALTITUDE, FireRequest, WeaponOverrides, WeaponTable, build_weapon_table,
};

fn cache() -> Option<std::path::PathBuf> {
    let root = bft_assets::test_support::data_root()?;
    match bft_assets::DataPaths::discover(Some(&root), None, None) {
        Ok(paths) if paths.cache.path.join("reader").is_dir() => Some(paths.cache.path),
        Ok(paths) => {
            eprintln!(
                "no extraction cache at {}; run `open-bft extract`. Skipping.",
                paths.cache.path.display()
            );
            None
        }
        Err(e) => {
            eprintln!("cannot resolve the data paths ({e}); skipping");
            None
        }
    }
}

fn reader(cache: &std::path::Path, file: &str) -> bft_assets::ReaderData {
    let bytes = std::fs::read(cache.join("reader").join(file)).expect("the archive is readable");
    bft_assets::reader::parse_reader_data(&bytes).expect("a real reader archive parses")
}

/// A headless app with a real mission's level, colliders and turret brains.
fn level_with_turrets(cache: &std::path::Path, mission: &str) -> (App, Vec<Entity>) {
    let table: AiTable = build_ai_table(
        &reader(cache, &format!("{mission}-zrdr.rdr.zip")),
        Difficulty::Normal,
        &Default::default(),
    )
    .expect("the AI table builds");
    let weapons: WeaponTable = build_weapon_table(
        &reader(cache, "zrdr.rdr.zip"),
        &WeaponOverrides::default(),
        DEFAULT_TETHER_MAX_ALTITUDE,
    )
    .expect("the weapon table builds");
    let bytes = std::fs::read(cache.join(format!("gamez/{mission}-gamez.gamez.zip")))
        .expect("the gamez archive is readable");
    let level: LevelData = parse_gamez(&bytes, "gamez.zip").expect("a real gamez parses");

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bft_sim::SimPlugin);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / SIM_HZ),
    ));
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.insert_resource(AxisConvention::GAMEZ_DEFAULT);
    app.finish();
    app.update();

    app.world_mut()
        .run_system_cached_with(
            |In(level): In<LevelData>, mut commands: Commands, axes: Res<AxisConvention>| {
                bft_sim::level::spawn_level_into(&mut commands, &level, &axes);
            },
            level,
        )
        .expect("the level spawns");
    app.update();

    app.insert_resource(weapons);
    let turrets = app
        .world_mut()
        .run_system_cached_with(
            |In(table): In<AiTable>,
             mut commands: Commands,
             weapons: Res<WeaponTable>,
             nodes: Res<NodeIndex>,
             all: Query<(Entity, &NodeName)>,
             children: Query<&Children>,
             names: Query<&NodeName>| {
                let matched: Vec<(String, Entity)> = all
                    .iter()
                    .filter(|(_, name)| table.turret_for(&name.0).is_some())
                    .map(|(entity, name)| (name.0.clone(), entity))
                    .collect();
                let turrets = spawn_turrets(
                    &mut commands,
                    &nodes,
                    &matched,
                    &table,
                    Some(&weapons),
                    &children,
                    &names,
                );
                commands.insert_resource(table);
                turrets
            },
            table,
        )
        .expect("the turrets spawn");

    // Transforms propagate and the colliders land in the spatial index.
    for _ in 0..5 {
        app.update();
    }
    (app, turrets)
}

/// Every one of m1's ground turrets sees, tracks and shoots a target in the
/// open sky above it.
///
/// The line-of-sight ray used to start at the emplacement's origin, at ground
/// level inside its own `base`, and hit the turret itself within four
/// metres: no turret in m1 ever acquired the player.
#[test]
fn m1s_turrets_see_and_shoot_a_target_in_the_open() {
    let Some(cache) = cache() else { return };
    let (mut app, turrets) = level_with_turrets(&cache, "m1");

    let ground: Vec<Entity> = turrets
        .into_iter()
        .filter(|entity| {
            app.world()
                .get::<NodeName>(*entity)
                .is_some_and(|name| name.0.starts_with("tur_"))
        })
        .collect();
    assert_eq!(ground.len(), 10, "m1 places ten `tur_*` emplacements");

    let mut silent = Vec::new();
    for turret in ground {
        let name = app
            .world()
            .get::<NodeName>(turret)
            .expect("named")
            .0
            .clone();
        let muzzle = app
            .world()
            .get::<TurretAi>(turret)
            .and_then(|ai| ai.muzzles.first().copied())
            .and_then(|muzzle| app.world().get::<GlobalTransform>(muzzle))
            .expect("every m1 turret has a firepoint")
            .translation();

        // Up and to one side: nothing in m1 stands 25 m above a turret.
        let at = muzzle + Vec3::new(8.0, 25.0, 0.0);
        let target = app
            .world_mut()
            .spawn((
                Team::PLAYER,
                Health::new(100.0),
                Transform::from_translation(at),
                GlobalTransform::from_translation(at),
            ))
            .id();

        let mut shots = 0;
        for _ in 0..(3.0 * SIM_HZ) as usize {
            app.update();
            shots += app
                .world()
                .resource::<Messages<FireRequest>>()
                .iter_current_update_messages()
                .filter(|request| request.shooter == turret)
                .count();
        }
        if shots == 0 {
            silent.push(name);
        }
        app.world_mut().entity_mut(target).despawn();
    }

    assert!(silent.is_empty(), "never fired: {silent:?}");
}

/// m1's `tur_*` emplacements, each with its first muzzle's world position.
fn ground_turrets(app: &App, turrets: &[Entity]) -> Vec<(Entity, Vec3)> {
    turrets
        .iter()
        .copied()
        .filter(|entity| {
            app.world()
                .get::<NodeName>(*entity)
                .is_some_and(|name| name.0.starts_with("tur_"))
        })
        .map(|turret| {
            let muzzle = app
                .world()
                .get::<TurretAi>(turret)
                .and_then(|ai| ai.muzzles.first().copied())
                .and_then(|muzzle| app.world().get::<GlobalTransform>(muzzle))
                .expect("every m1 turret has a firepoint")
                .translation();
            (turret, muzzle)
        })
        .collect()
}

/// A target the turrets want, hanging in the air.
fn spawn_target(app: &mut App, at: Vec3) -> Entity {
    app.world_mut()
        .spawn((
            Team::PLAYER,
            Health::new(100.0),
            Transform::from_translation(at),
            GlobalTransform::from_translation(at),
        ))
        .id()
}

fn is_under(app: &App, mut entity: Entity, ancestor: Entity) -> bool {
    loop {
        if entity == ancestor {
            return true;
        }
        match app.world().get::<ChildOf>(entity) {
            Some(parent) => entity = parent.parent(),
            None => return false,
        }
    }
}

/// A turret's shots leave its barrel and fly: none hits the emplacement
/// that fired it.
///
/// The shot used to start at the turret's origin, inside its base, with
/// only the turret node excluded from the cast — so it hit its own model on
/// the first step. The sound played and nothing was ever drawn.
#[test]
fn m1s_turret_shots_clear_their_own_emplacement() {
    use bft_sim::weapons::{ProjectileHit, ProjectileSpawned};
    let Some(cache) = cache() else { return };
    let (mut app, turrets) = level_with_turrets(&cache, "m1");

    let mut own_hits = Vec::new();
    let mut fired = 0;
    for (turret, muzzle) in ground_turrets(&app, &turrets) {
        let name = app
            .world()
            .get::<NodeName>(turret)
            .expect("named")
            .0
            .clone();
        let target = spawn_target(&mut app, muzzle + Vec3::new(8.0, 25.0, 0.0));
        let mut mine = Vec::new();
        for _ in 0..(3.0 * SIM_HZ) as usize {
            app.update();
            let world = app.world();
            mine.extend(
                world
                    .resource::<Messages<ProjectileSpawned>>()
                    .iter_current_update_messages()
                    .filter(|spawned| {
                        world
                            .get::<bft_sim::weapons::Projectile>(spawned.entity)
                            .is_some_and(|projectile| projectile.owner == turret)
                    })
                    .map(|spawned| spawned.entity),
            );
            let hits: Vec<ProjectileHit> = world
                .resource::<Messages<ProjectileHit>>()
                .iter_current_update_messages()
                .filter(|hit| mine.contains(&hit.projectile) || hit.projectile == turret)
                .cloned()
                .collect();
            for hit in hits {
                if hit.hit.is_some_and(|entity| is_under(&app, entity, turret)) {
                    own_hits.push(name.clone());
                }
            }
        }
        fired += mine.len();
        app.world_mut().entity_mut(target).despawn();
    }

    assert!(fired > 0, "the turrets fired");
    assert!(
        own_hits.is_empty(),
        "shots hit their own emplacement: {own_hits:?}"
    );
}

/// A hit on a turret's model bills the turret, and enough of them destroy
/// it.
///
/// Its `Health` is on the `tur_*` node and its colliders on the model nodes
/// under it; billing only the entity hit made every turret indestructible.
#[test]
fn m1s_turrets_die_to_hits_on_their_model() {
    use avian3d::prelude::Collider;
    use bft_sim::damage::Destroyed;
    use bft_sim::weapons::{ProjectileHit, Surface};
    let Some(cache) = cache() else { return };
    let (mut app, turrets) = level_with_turrets(&cache, "m1");
    let weapon = app
        .world()
        .resource::<WeaponTable>()
        .index("wep_1.0")
        .expect("the pulse gun");

    let mut survivors = Vec::new();
    for (turret, muzzle) in ground_turrets(&app, &turrets) {
        let name = app
            .world()
            .get::<NodeName>(turret)
            .expect("named")
            .0
            .clone();
        let mut colliders = app.world_mut().query_filtered::<Entity, With<Collider>>();
        let part = colliders
            .iter(app.world())
            .find(|entity| *entity != turret && is_under(&app, *entity, turret))
            .expect("a turret's model has a collider");

        let mut died = false;
        for _ in 0..200 {
            app.world_mut()
                .resource_mut::<Messages<ProjectileHit>>()
                .write(ProjectileHit {
                    projectile: Entity::PLACEHOLDER,
                    at: muzzle,
                    normal: Vec3::Y,
                    hit: Some(part),
                    hit_node: None,
                    surface: Surface::Default,
                    weapon,
                });
            app.update();
            died |= app
                .world()
                .resource::<Messages<Destroyed>>()
                .iter_current_update_messages()
                .any(|destroyed| destroyed.entity == turret);
            if died {
                break;
            }
        }
        if !died {
            survivors.push(name);
        }
    }
    assert!(survivors.is_empty(), "never died: {survivors:?}");
}

/// A turret tracking a still target holds still once it is on it.
///
/// The aim scatter must not be applied to the point the parts slew toward
/// every tick, or the barrel chases a fresh random point sixty times a
/// second.
#[test]
fn m1s_turrets_hold_steady_on_a_still_target() {
    let Some(cache) = cache() else { return };
    let (mut app, turrets) = level_with_turrets(&cache, "m1");

    let mut shaking = Vec::new();
    for (turret, muzzle) in ground_turrets(&app, &turrets) {
        let name = app
            .world()
            .get::<NodeName>(turret)
            .expect("named")
            .0
            .clone();
        let target = spawn_target(&mut app, muzzle + Vec3::new(30.0, 10.0, 20.0));
        // Settle onto it, then watch.
        for _ in 0..(3.0 * SIM_HZ) as usize {
            app.update();
        }
        let mut worst = 0.0f32;
        let mut last = app.world().get::<TurretAi>(turret).expect("a turret").aim;
        for _ in 0..30 {
            app.update();
            let aim = app.world().get::<TurretAi>(turret).expect("a turret").aim;
            worst = worst.max((aim - last).abs().max_element());
            last = aim;
        }
        if worst > 1e-3 {
            shaking.push((name, worst));
        }
        app.world_mut().entity_mut(target).despawn();
    }
    assert!(
        shaking.is_empty(),
        "aim moves per tick on a still target: {shaking:?}"
    );
}

/// A destroyed turret stops shooting.
///
/// `stop_dead_brains` parks a vehicle's `AiBrain`, and a turret has none, so
/// the turret's own brain has to stop after `Destroyed` or the wreck goes on
/// firing.
#[test]
fn m1s_destroyed_turrets_stop_firing() {
    use avian3d::prelude::Collider;
    use bft_sim::damage::Destroyed;
    use bft_sim::weapons::{ProjectileHit, Surface};
    let Some(cache) = cache() else { return };
    let (mut app, turrets) = level_with_turrets(&cache, "m1");
    let weapon = app
        .world()
        .resource::<WeaponTable>()
        .index("wep_1.0")
        .expect("the pulse gun");

    let mut still_firing = Vec::new();
    for (turret, muzzle) in ground_turrets(&app, &turrets) {
        let name = app
            .world()
            .get::<NodeName>(turret)
            .expect("named")
            .0
            .clone();
        let mut colliders = app.world_mut().query_filtered::<Entity, With<Collider>>();
        let part = colliders
            .iter(app.world())
            .find(|entity| *entity != turret && is_under(&app, *entity, turret))
            .expect("a turret's model has a collider");

        let mut died = false;
        for _ in 0..200 {
            app.world_mut()
                .resource_mut::<Messages<ProjectileHit>>()
                .write(ProjectileHit {
                    projectile: Entity::PLACEHOLDER,
                    at: muzzle,
                    normal: Vec3::Y,
                    hit: Some(part),
                    hit_node: None,
                    surface: Surface::Default,
                    weapon,
                });
            app.update();
            died |= app
                .world()
                .resource::<Messages<Destroyed>>()
                .iter_current_update_messages()
                .any(|destroyed| destroyed.entity == turret);
            if died {
                break;
            }
        }
        assert!(died, "{name} died");

        // The same target the living turrets all shoot at.
        let target = spawn_target(&mut app, muzzle + Vec3::new(8.0, 25.0, 0.0));
        let mut shots = 0;
        for _ in 0..(3.0 * SIM_HZ) as usize {
            app.update();
            shots += app
                .world()
                .resource::<Messages<FireRequest>>()
                .iter_current_update_messages()
                .filter(|request| request.shooter == turret)
                .count();
        }
        if shots > 0 {
            still_firing.push((name, shots));
        }
        app.world_mut().entity_mut(target).despawn();
    }
    assert!(
        still_firing.is_empty(),
        "wrecks still firing: {still_firing:?}"
    );
}
