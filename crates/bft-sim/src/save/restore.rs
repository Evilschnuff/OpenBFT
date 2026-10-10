//! Writing a [`SaveFile`] into a freshly loaded level.
//!
//! Phase one finds or makes every entity a record can name: the level's
//! nodes, the player and enemies the load spawned, and — new — the copies'
//! subtrees and instances, the pickups and the projectiles, so that a record
//! naming a copy's part or a shell resolves no matter which comes first.
//! Phase two writes the state. Anything a record names that does not resolve
//! is skipped and counted in the [`RestoreReport`], never a panic: a save
//! made against slightly different data should load as far as it can.

use std::collections::HashMap;

use avian3d::prelude::{
    AngularVelocity, ColliderDisabled, LinearVelocity, Position, RigidBodyDisabled, Rotation,
};
use bevy::prelude::*;
use rand::SeedableRng;

use super::capture::{fingerprint, named_subtree};
use super::format::*;
use super::{PendingAnimRestore, RestoreGate};
use crate::ai::{AiBrain, AiTick, Enemy, TurretAi};
use crate::anim::apply::LightAnimState;
use crate::anim::{
    AnimCamera, AnimClock, AnimCopy, AnimInstance, AnimLight, AnimMotion, AnimOpacity, AnimPlaced,
    AnimProgram, AnimRest, AnimScreenFx, CondFrame, ConnectorEnd, MotionOwner, SeqCursor,
};
use crate::cinematic::PlayerTeleported;
use crate::crater::{Crater, GroundSource, cut_components, recut};
use crate::damage::{Burning, Health, Team};
use crate::level::{
    LevelLight, LevelRoot, LodSwitch, ModelRef, NodeActive, NodeIndex, NodeName, WorldEnv,
};
use crate::mission::{MissionClock, MissionState, Score};
use crate::physics::SimRng;
use crate::pickups::{Pickup, RespawnTimer, TakenPickup, pickup_bundle};
use crate::vehicle::{FormSet, PlayerInput, PlayerVehicle, Turret, Vehicle};
use crate::weapons::{
    AmmoState, Landing, Projectile, ProjectileSpawned, TwinBarrels, WeaponLoadout, WeaponTable,
    projectile_body,
};

/// What a restore did, for the log and for tests.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RestoreReport {
    /// Whether the save was made against the same game data.
    pub fingerprint_matches: bool,
    /// Records that could not be applied, one line each.
    pub skipped: Vec<String>,
}

impl RestoreReport {
    fn skip(&mut self, why: impl Into<String>) {
        let why = why.into();
        warn!("restore: {why}");
        self.skipped.push(why);
    }
}

/// Write `save` into the world and let the sim run again.
///
/// The world must be a freshly loaded level of the same mission, set up and
/// not yet stepped, with [`SaveBaseline::record`](super::SaveBaseline::record)
/// already called.
pub fn restore(world: &mut World, save: &SaveFile) -> RestoreReport {
    let mut report = RestoreReport {
        fingerprint_matches: fingerprint(world) == save.header.fingerprint,
        ..default()
    };
    if !report.fingerprint_matches {
        warn!(
            "this save was made against different game data; loading what still matches \
             (saved {:?}, loaded {:?})",
            save.header.fingerprint,
            fingerprint(world)
        );
    }

    let mut refs = Resolver::existing(world);

    // Phase one: the entities the save brings with it.
    clear_dynamic(world);
    spawn_copy_entities(world, save, &mut refs);
    spawn_pickups(world, save, &mut refs);
    let projectiles = reserve_projectiles(world, save, &mut refs);

    // Phase two: state.
    apply_nodes(world, save, &refs, &mut report);
    apply_craters(world, save, &mut report);
    apply_vehicles(world, save, &refs, &mut report);
    apply_turrets(world, save, &refs, &mut report);
    apply_projectiles(world, save, &refs, projectiles, &mut report);
    apply_anims(world, save, &refs, &mut report);
    apply_world(world, save, &refs, &mut report);

    world.remove_resource::<RestoreGate>();
    // The view follows the hull; it should jump there, not fly from the
    // mission's start.
    world.write_message(PlayerTeleported {
        by: "load".to_string(),
    });

    info!(
        "save restored: {} node(s), {} enem(ies), {} copies, {} projectile(s), {} skipped",
        save.nodes.len(),
        save.enemies.len(),
        save.anims.copies.len(),
        save.projectiles.len(),
        report.skipped.len()
    );
    report
}

// ---------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------

/// `SaveRef` → the entity it names in this world.
struct Resolver {
    map: HashMap<SaveRef, Entity>,
    level_root: Option<Entity>,
}

impl Resolver {
    /// Everything the fresh level already has.
    fn existing(world: &mut World) -> Resolver {
        let mut map = HashMap::new();

        // Only level nodes carry a `NodeId`; a clone never does, so an id is
        // one entity.
        let mut ids = world.query::<(Entity, &crate::level::NodeId)>();
        for (entity, id) in ids.iter(world) {
            map.insert(SaveRef::Node(id.0), entity);
        }

        let mut players = world.query_filtered::<Entity, With<PlayerVehicle>>();
        if let Some(player) = players.iter(world).next() {
            map.insert(SaveRef::Player, player);
        }

        let mut enemies = world.query::<(Entity, &Enemy)>();
        let enemies: Vec<(Entity, String)> = enemies
            .iter(world)
            .map(|(entity, enemy)| (entity, enemy.instance.clone()))
            .collect();
        for (hull, instance) in enemies {
            for (index, part) in named_subtree(world, hull).into_iter().enumerate() {
                map.insert(SaveRef::EnemyPart(instance.clone(), index as u32), part);
            }
            map.insert(SaveRef::Enemy(instance), hull);
        }

        let mut instances = world.query::<(Entity, &AnimInstance)>();
        for (entity, instance) in instances.iter(world) {
            if instance.copy.is_none() {
                map.insert(SaveRef::Anim(instance.def as u32), entity);
            }
        }

        let mut roots = world.query_filtered::<Entity, With<LevelRoot>>();
        let level_root = roots.iter(world).next();

        Resolver { map, level_root }
    }

    fn get(&self, reference: &SaveRef) -> Option<Entity> {
        self.map.get(reference).copied()
    }

    fn opt(&self, reference: Option<&SaveRef>) -> Option<Entity> {
        reference.and_then(|reference| self.get(reference))
    }

    fn end(&self, end: &EndRecord) -> ConnectorEnd {
        ConnectorEnd {
            node: self.opt(end.node.as_ref()),
            offset: Vec3::from_array(end.offset),
            frozen: end.frozen,
        }
    }
}

// ---------------------------------------------------------------------------
// Phase one
// ---------------------------------------------------------------------------

/// Take away what the save replaces wholesale: the level's own pickups, and
/// anything transient a fresh level should not have but might.
fn clear_dynamic(world: &mut World) {
    let mut doomed: Vec<Entity> = Vec::new();

    let mut pickups = world.query_filtered::<Entity, Or<(With<Pickup>, With<TakenPickup>)>>();
    doomed.extend(pickups.iter(world));
    let mut projectiles = world.query_filtered::<Entity, With<Projectile>>();
    doomed.extend(projectiles.iter(world));
    let mut instances = world.query::<(Entity, &AnimInstance)>();
    for (entity, instance) in instances.iter(world) {
        if let Some(copy) = instance.copy {
            doomed.push(entity);
            if copy.owned {
                doomed.push(copy.root);
            }
        }
    }

    for entity in doomed {
        if let Ok(entity) = world.get_entity_mut(entity) {
            entity.despawn();
        }
    }
}

/// Spawn each copy's instance entity and, for a cloned subtree, its nodes.
fn spawn_copy_entities(world: &mut World, save: &SaveFile, refs: &mut Resolver) {
    for (index, copy) in save.anims.copies.iter().enumerate() {
        let index = index as u32;
        let mut instance = world.spawn(Name::new(format!("anim:{} copy", copy.instance.name)));
        if let Some(root) = refs.level_root {
            instance.insert(ChildOf(root));
        }
        refs.map.insert(SaveRef::Copy(index), instance.id());

        let CopySubtree::Owned(nodes) = &copy.subtree else {
            continue;
        };
        let mut spawned: Vec<Entity> = Vec::with_capacity(nodes.len());
        for (part, node) in nodes.iter().enumerate() {
            let parent = node
                .parent
                .and_then(|parent| spawned.get(parent as usize).copied())
                .or(refs.level_root);
            let mut entity = world.spawn((Transform::from(node.t), NodeActive(node.active)));
            if let Some(name) = &node.name {
                entity.insert((NodeName(name.clone()), Name::new(name.clone())));
            }
            if let Some(parent) = parent {
                entity.insert(ChildOf(parent));
            }
            if let Some(model) = node.model {
                entity.insert(ModelRef(model as usize));
            }
            if let Some(lod) = node.lod {
                entity.insert(LodSwitch {
                    near: lod.near,
                    far: lod.far,
                    level: lod.level,
                });
            }
            if let Some(opacity) = node.opacity {
                entity.insert(AnimOpacity(opacity));
            }
            if let Some(rest) = node.rest {
                entity.insert(AnimRest(rest.into()));
            }
            if node.placed {
                entity.insert(AnimPlaced);
            }
            let entity = entity.id();
            spawned.push(entity);
            refs.map
                .insert(SaveRef::CopyPart(index, part as u32), entity);
        }
    }
}

fn spawn_pickups(world: &mut World, save: &SaveFile, refs: &mut Resolver) {
    for (index, record) in save.pickups.iter().enumerate() {
        let pickup = Pickup {
            kind: record.kind.clone(),
            amount: record.amount,
            respawn: record.respawn,
            instance: record.instance.clone(),
        };
        let mut entity = world.spawn(pickup_bundle(pickup, Vec3::from_array(record.at)));
        if let Some(remaining) = record.respawn_in {
            entity.insert(RespawnTimer { remaining });
        }
        refs.map.insert(SaveRef::Pickup(index as u32), entity.id());
    }
}

/// Reserve an entity per projectile, so records can name them before their
/// state is written.
fn reserve_projectiles(world: &mut World, save: &SaveFile, refs: &mut Resolver) -> Vec<Entity> {
    save.projectiles
        .iter()
        .enumerate()
        .map(|(index, _)| {
            let entity = world.spawn_empty().id();
            refs.map.insert(SaveRef::Projectile(index as u32), entity);
            entity
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Phase two
// ---------------------------------------------------------------------------

fn apply_nodes(world: &mut World, save: &SaveFile, refs: &Resolver, report: &mut RestoreReport) {
    for record in &save.nodes {
        let Some(entity) = refs.get(&record.node) else {
            report.skip(format!("no node {:?} in this level", record.node));
            continue;
        };
        if let Some(name) = &record.name
            && world
                .get::<NodeName>(entity)
                .is_none_or(|found| &found.0 != name)
        {
            report.skip(format!("{:?} is not `{name}` in this level", record.node));
            continue;
        }

        let parent = match &record.parent {
            None => None,
            Some(Parent::LevelRoot) => refs.level_root,
            Some(Parent::Of(reference)) => {
                let parent = refs.get(reference);
                if parent.is_none() {
                    report.skip(format!("no parent {reference:?} for {:?}", record.node));
                }
                parent
            }
        };

        let mut node = world.entity_mut(entity);
        if let Some(t) = record.t {
            node.insert(Transform::from(t));
        }
        if let Some(active) = record.active {
            node.insert(NodeActive(active));
        }
        if let Some(parent) = parent
            && parent != entity
        {
            node.insert(ChildOf(parent));
        }
        match record.opacity {
            Some(opacity) => {
                node.insert(AnimOpacity(opacity));
            }
            None => {
                node.remove::<AnimOpacity>();
            }
        }
        if let Some(rest) = record.rest {
            node.insert(AnimRest(rest.into()));
        }
        if record.placed == Some(true) {
            node.insert(AnimPlaced);
        }
    }
}

fn apply_craters(world: &mut World, save: &SaveFile, report: &mut RestoreReport) {
    for record in &save.craters {
        let Some(entity) = world
            .get_resource::<NodeIndex>()
            .and_then(|nodes| nodes.get(record.node))
        else {
            report.skip(format!("no crater ground node {}", record.node));
            continue;
        };
        let (Some(source), Some(global)) = (
            world.get::<GroundSource>(entity).cloned(),
            world.get::<GlobalTransform>(entity).copied(),
        ) else {
            report.skip(format!("node {} does not take craters", record.node));
            continue;
        };

        let craters: Vec<Crater> = record
            .craters
            .iter()
            .map(|crater| Crater {
                center: Vec3::from_array(crater.center),
                radius: crater.radius,
                floor_radius: crater.floor_radius,
                depth: crater.depth,
                sides: crater.sides.max(3) as usize,
                texture: crater.texture.clone(),
                uv_scale: Vec2::from_array(crater.uv_scale),
            })
            .collect();
        let Some((applied, cut)) = recut(&source, &global.affine(), &craters) else {
            report.skip(format!("the craters on node {} no longer cut", record.node));
            continue;
        };
        if applied.len() != craters.len() {
            report.skip(format!(
                "{} of node {}'s craters no longer cut",
                craters.len() - applied.len(),
                record.node
            ));
        }

        let (collider, surface, cratered) = cut_components(&source, applied, cut);
        let mut node = world.entity_mut(entity);
        if let Some(collider) = collider {
            node.insert(collider);
        }
        if let Some(surface) = surface {
            node.insert(surface);
        }
        node.insert(cratered);
    }
}

fn apply_vehicles(world: &mut World, save: &SaveFile, refs: &Resolver, report: &mut RestoreReport) {
    let weapons = world.get_resource::<WeaponTable>().cloned();

    if let Some(player) = &save.player {
        match refs.get(&SaveRef::Player) {
            Some(hull) => {
                apply_vehicle(world, hull, &player.vehicle, weapons.as_ref(), report);
                if let (Some(next), Some(mut twins)) =
                    (player.twin_next, world.get_mut::<TwinBarrels>(hull))
                {
                    twins.next = usize::from(next) % 2;
                }
            }
            None => report.skip("the level has no player to restore"),
        }
    }

    for enemy in &save.enemies {
        let Some(hull) = refs.get(&SaveRef::Enemy(enemy.instance.clone())) else {
            report.skip(format!("no enemy `{}` in this level", enemy.instance));
            continue;
        };
        apply_vehicle(world, hull, &enemy.vehicle, weapons.as_ref(), report);

        let brain = &enemy.brain;
        let dead = brain.mode == AiMode::Dead;
        let target = refs.opt(brain.target.as_ref());
        if let Some(mut live) = world.get_mut::<AiBrain>(hull) {
            live.state = brain.mode.into();
            live.target = target;
            live.timer = brain.timer;
            live.waypoint = brain.waypoint.map(|waypoint| waypoint as usize);
            live.forward = brain.forward;
            live.los_lost = brain.los_lost;
            live.since_seen = brain.since_seen;
        }
        if dead {
            // What `stop_dead_brains` does on the tick an enemy dies: the
            // wreck is the death anim's to draw, and must not block or catch.
            let mut hull = world.entity_mut(hull);
            hull.insert((ColliderDisabled, RigidBodyDisabled, PlayerInput::default()));
        }
    }
}

fn apply_vehicle(
    world: &mut World,
    hull: Entity,
    record: &VehicleRecord,
    weapons: Option<&WeaponTable>,
    report: &mut RestoreReport,
) {
    let transform = Transform::from(record.t);
    let loadout = record.loadout.as_ref().and_then(|saved| {
        let current = world
            .get::<WeaponLoadout>(hull)
            .cloned()
            .unwrap_or_default();
        restore_loadout(saved, &current, weapons, report)
    });

    let mut entity = world.entity_mut(hull);
    entity.insert((
        transform,
        Position(transform.translation),
        Rotation(transform.rotation),
        LinearVelocity(Vec3::from_array(record.velocity)),
        AngularVelocity(Vec3::from_array(record.spin)),
        Turret {
            yaw: record.turret[0],
            pitch: record.turret[1],
        },
    ));
    if let Some(mut vehicle) = entity.get_mut::<Vehicle>() {
        vehicle.form = record.form.into();
        let mut unlocked = FormSet::default();
        for form in &record.unlocked {
            unlocked.unlock((*form).into());
        }
        // Whatever it drives in is unlocked, even from a save that forgot.
        unlocked.unlock(vehicle.form);
        vehicle.unlocked = unlocked;
    }
    if let Some(health) = record.health {
        entity.insert(Health {
            current: health.current,
            max: health.max,
        });
    }
    match record.burning {
        Some(burning) => {
            entity.insert(Burning {
                remaining: burning.remaining,
                dps: burning.dps,
            });
        }
        None => {
            entity.remove::<Burning>();
        }
    }
    if let Some(loadout) = loadout {
        entity.insert(loadout);
    }
}

/// A saved loadout, rebuilt against this build's weapon table.
///
/// The fire interval is not in the file (SAVEGAME §6): it is taken from the
/// loadout the load just built, which read it from this vehicle's own row,
/// and from the table's shot interval for a weapon that row lacks.
fn restore_loadout(
    saved: &LoadoutRecord,
    current: &WeaponLoadout,
    weapons: Option<&WeaponTable>,
    report: &mut RestoreReport,
) -> Option<WeaponLoadout> {
    let Some(weapons) = weapons else {
        report.skip("no weapon table; a loadout is not restored");
        return None;
    };

    let interval_of = |index: usize| -> f32 {
        current
            .slots
            .iter()
            .flat_map(|slot| [slot.weak, slot.strong])
            .flatten()
            .find(|state| state.weapon == index)
            .map(|state| state.interval)
            .or_else(|| weapons.by_index(index).map(|def| def.shot_interval()))
            .unwrap_or(1.0)
    };
    let mut ammo = |record: &Option<AmmoRecord>| -> Option<AmmoState> {
        let record = record.as_ref()?;
        let Some(index) = weapons.index(&record.weapon) else {
            report.skip(format!("no weapon `{}` in this build", record.weapon));
            return None;
        };
        Some(AmmoState {
            weapon: index,
            ammo: record.ammo,
            interval: interval_of(index),
            mounted: record.mounted,
        })
    };

    let mut loadout = WeaponLoadout {
        selected: saved.selected.clamp(1, 9),
        strong: saved.strong,
        cooldown: saved.cooldown,
        secondary_strong: saved.secondary_strong,
        secondary_cooldown: saved.secondary_cooldown,
        ..default()
    };
    for record in &saved.slots {
        let weak = ammo(&record.weak);
        let strong = ammo(&record.strong);
        if let Some(slot) = loadout.slot_mut(record.slot) {
            slot.weak = weak;
            slot.strong = strong;
        }
    }
    Some(loadout)
}

fn apply_turrets(world: &mut World, save: &SaveFile, refs: &Resolver, report: &mut RestoreReport) {
    let weapons = world.get_resource::<WeaponTable>().cloned();

    for record in &save.turrets {
        let Some(entity) = refs.get(&SaveRef::Node(record.node)) else {
            report.skip(format!("no turret node {}", record.node));
            continue;
        };
        let target = refs.opt(record.target.as_ref());
        let loadout = record.loadout.as_ref().and_then(|saved| {
            let current = world
                .get::<WeaponLoadout>(entity)
                .cloned()
                .unwrap_or_default();
            restore_loadout(saved, &current, weapons.as_ref(), report)
        });

        let mut node = world.entity_mut(entity);
        let Some(mut turret) = node.get_mut::<TurretAi>() else {
            report.skip(format!(
                "node {} is not a turret in this level",
                record.node
            ));
            continue;
        };
        turret.next_muzzle = record.next_muzzle as usize;
        turret.fire_timer = record.fire_timer;
        turret.burst_left = record.burst_left;
        turret.dwell_left = record.dwell_left;
        turret.aim = Vec2::from_array(record.aim);
        turret.target = target;
        turret.deactivated = record.deactivated;

        if let Some(health) = record.health {
            node.insert(Health {
                current: health.current,
                max: health.max,
            });
        }
        match record.burning {
            Some(burning) => {
                node.insert(Burning {
                    remaining: burning.remaining,
                    dps: burning.dps,
                });
            }
            None => {
                node.remove::<Burning>();
            }
        }
        if let Some(loadout) = loadout {
            node.insert(loadout);
        }
    }
}

fn apply_projectiles(
    world: &mut World,
    save: &SaveFile,
    refs: &Resolver,
    reserved: Vec<Entity>,
    report: &mut RestoreReport,
) {
    let weapons = world.get_resource::<WeaponTable>().cloned();

    for (record, entity) in save.projectiles.iter().zip(reserved) {
        let index = weapons
            .as_ref()
            .and_then(|table| table.index(&record.weapon));
        let (Some(index), Some(owner)) = (index, refs.opt(record.owner.as_ref())) else {
            report.skip(format!(
                "a `{}` projectile has no weapon or owner",
                record.weapon
            ));
            world.despawn(entity);
            continue;
        };
        world.entity_mut(entity).insert((
            Name::new(format!("projectile {}", record.weapon)),
            Projectile {
                weapon: index,
                owner,
                team: Team(record.team),
                age: record.age,
                target: refs.opt(record.target.as_ref()),
                armed: record.armed,
                health: record.health,
                velocity: Vec3::from_array(record.velocity),
                landed: record.landed.map(|landing| Landing {
                    normal: Vec3::from_array(landing.normal),
                    surface: landing.surface.into(),
                }),
            },
            Transform::from(record.t),
            projectile_body(),
        ));
        // So the presentation gives it its model and its flight effects.
        world.write_message(ProjectileSpawned {
            entity,
            weapon: index,
        });
    }
}

fn apply_anims(world: &mut World, save: &SaveFile, refs: &Resolver, report: &mut RestoreReport) {
    let Some(program) = world.get_resource::<AnimProgram>().cloned() else {
        if !save.anims.instances.is_empty() || !save.anims.copies.is_empty() {
            report.skip("this level has no animation program; the scripts are not restored");
        }
        return;
    };
    let defs = &program.0.defs;

    // Every definition's own instance back to how the level spawned it, less
    // the pending reset its first tick would have cleared long ago.
    let mut instances = world.query::<&mut AnimInstance>();
    for mut instance in instances.iter_mut(world) {
        if instance.copy.is_some() {
            continue;
        }
        let Some(def) = defs.get(instance.def) else {
            continue;
        };
        let (root, prototype) = (instance.root, instance.prototype);
        *instance = AnimInstance::new(instance.def, def.activation, root);
        instance.prototype = prototype;
        instance.pending_reset = false;
    }

    for record in &save.anims.instances {
        let Some(def_index) = resolve_def(defs, record, report) else {
            continue;
        };
        let Some(entity) = refs.get(&SaveRef::Anim(def_index as u32)) else {
            report.skip(format!("no instance of `{}`", record.name));
            continue;
        };
        let Some(mut instance) = world.get_mut::<AnimInstance>(entity) else {
            continue;
        };
        write_instance(&mut instance, record, refs);
    }

    for (index, copy) in save.anims.copies.iter().enumerate() {
        let Some(entity) = refs.get(&SaveRef::Copy(index as u32)) else {
            continue;
        };
        let def_index = resolve_def(defs, &copy.instance, report);
        let (root, owned) = match &copy.subtree {
            CopySubtree::On(reference) => (refs.get(reference), false),
            CopySubtree::Owned(_) => (refs.get(&SaveRef::CopyPart(index as u32, 0)), true),
        };
        let (Some(def_index), Some(root)) = (def_index, root) else {
            report.skip(format!(
                "the copy of `{}` has nothing to play on",
                copy.instance.name
            ));
            world.despawn(entity);
            if owned && let Some(root) = root {
                world.despawn(root);
            }
            continue;
        };
        let Some(def) = defs.get(def_index) else {
            continue;
        };

        let mut instance = AnimInstance::new(def_index, def.activation, Some(root));
        write_instance(&mut instance, &copy.instance, refs);
        instance.copy = Some(AnimCopy { root, owned });
        world.entity_mut(entity).insert(instance);
    }
}

/// The definition a record means: its index, if the name still matches, else
/// the only definition of that name.
fn resolve_def(
    defs: &[bft_assets::anim::AnimDefDesc],
    record: &AnimRecord,
    report: &mut RestoreReport,
) -> Option<usize> {
    let index = record.def as usize;
    if defs
        .get(index)
        .is_some_and(|def| def.anim_name == record.name)
    {
        return Some(index);
    }
    let mut named = defs
        .iter()
        .enumerate()
        .filter(|(_, def)| def.anim_name == record.name);
    match (named.next(), named.next()) {
        (Some((index, _)), None) => Some(index),
        _ => {
            report.skip(format!(
                "animation `{}` (definition {}) is not in this level",
                record.name, record.def
            ));
            None
        }
    }
}

fn write_instance(instance: &mut AnimInstance, record: &AnimRecord, refs: &Resolver) {
    instance.armed = record.armed;
    instance.invalidated = record.invalidated;
    instance.started = record.started;
    instance.completed = record.completed;
    instance.damage = record.damage;
    instance.reset_at = record.reset_at;
    instance.running = record
        .running
        .iter()
        .map(|cursor| SeqCursor {
            seq: cursor.seq as usize,
            event: cursor.event as usize,
            t_anim: cursor.t_anim,
            t_seq: cursor.t_seq,
            t_event: cursor.t_event,
            started: cursor.started,
            t_running: cursor.t_running,
            loops_left: cursor.loops_left,
            cond_stack: cursor
                .conds
                .iter()
                .map(|(taking, satisfied)| CondFrame {
                    taking: *taking,
                    satisfied: *satisfied,
                })
                .collect(),
        })
        .collect();
    instance.input = refs.opt(record.input.as_ref());
    instance.input_ends = record
        .input_ends
        .as_ref()
        .map(|[from, to]| (refs.end(from), refs.end(to)));
    instance.played_on = refs.opt(record.played_on.as_ref());
    instance.operand_scope = record.scope.as_ref().map(|scope| {
        scope
            .iter()
            .map(|entry| (entry.name.clone(), refs.opt(entry.node.as_ref())))
            .collect()
    });
    instance.skip = record.skip;
    instance.ran_out = record.ran_out;
    instance.pending_reset = record.pending_reset;
}

fn apply_world(world: &mut World, save: &SaveFile, refs: &Resolver, report: &mut RestoreReport) {
    let record = &save.world;

    world.insert_resource(AnimClock(record.clock));
    world.insert_resource(AiTick(record.ai_tick));

    if let Some(rng) = &record.rng {
        match rng_from(rng) {
            Some(restored) => world.insert_resource(SimRng(restored)),
            None => report.skip("the random stream is unreadable; it starts afresh"),
        }
    }

    if let Some(mission) = &record.mission
        && let Some(mut state) = world.get_resource_mut::<MissionState>()
    {
        if mission.objectives.len() == state.objectives.len() {
            state.objectives = mission.objectives.iter().copied().map(Into::into).collect();
        } else {
            report.skip(format!(
                "the save has {} objective(s) and the mission {}; objectives start afresh",
                mission.objectives.len(),
                state.objectives.len()
            ));
        }
        state.outcome = mission.outcome.map(Into::into);
        state.dying = mission.dying;
        // Not started: the first tick calls the load entry animation.
        state.started = false;
        state.resumed = true;

        world.insert_resource(MissionClock(mission.elapsed.unwrap_or(record.clock)));
        world.insert_resource(Score(mission.score));
    }

    if let Some(fog) = record.fog
        && let Some(mut env) = world.get_resource_mut::<WorldEnv>()
    {
        env.fog_color = Color::LinearRgba(LinearRgba::from_f32_array(fog.color));
        env.fog_range = (fog.range[0], fog.range[1]);
        env.fog_altitude = (fog.altitude[0], fog.altitude[1]);
    }

    let motions = save
        .anims
        .motions
        .iter()
        .filter_map(|motion| {
            let node = refs.get(&motion.node)?;
            let instance = refs.get(&motion.owner)?;
            Some((
                node,
                AnimMotion {
                    owner: MotionOwner {
                        instance,
                        seq: motion.seq.map_or(usize::MAX, |seq| seq as usize),
                        event: motion.event as usize,
                    },
                    velocity: Vec3::from_array(motion.velocity),
                    acceleration: Vec3::from_array(motion.acceleration),
                    gravity: motion.gravity,
                    lands: motion.lands,
                    spin: Vec3::from_array(motion.spin),
                    spin_rate: Vec3::from_array(motion.spin_rate),
                    tumble: motion.tumble,
                    tumble_rate: motion.tumble_rate,
                    growth: Vec3::from_array(motion.growth),
                    growth_rate: Vec3::from_array(motion.growth_rate),
                    run_time: motion.run_time,
                    bounce_sequence: motion.bounce_sequence.clone(),
                    bounce_sound: motion.bounce_sound.clone(),
                    age: motion.age,
                    world: motion
                        .world
                        .map(|(p, v)| (Vec3::from_array(p), Vec3::from_array(v))),
                },
            ))
        })
        .collect();

    let lights = save
        .anims
        .lights
        .iter()
        .filter_map(|light| {
            let owner = refs.get(&light.owner)?;
            Some((
                (owner, light.name.clone()),
                AnimLight {
                    active: light.active,
                    light: LevelLight {
                        color: Color::LinearRgba(LinearRgba::from_f32_array(light.color)),
                        near: light.near,
                        far: light.far,
                    },
                    at: light.at.as_ref().and_then(|(node, offset)| {
                        Some((refs.get(node)?, Vec3::from_array(*offset)))
                    }),
                    animation: light.animation.map(|animation| LightAnimState {
                        range_rate: Vec2::from_array(animation.range_rate),
                        color_rate: Vec3::from_array(animation.color_rate),
                        remaining: animation.remaining,
                    }),
                },
            ))
        })
        .collect();

    world.insert_resource(PendingAnimRestore {
        motions,
        lights,
        camera: Some(match record.camera {
            Some(camera) => AnimCamera {
                zoom: Vec2::from_array(camera.zoom),
                clip_near: camera.clip_near,
                clip_far: camera.clip_far,
            },
            None => AnimCamera::default(),
        }),
        screen: Some(AnimScreenFx {
            tint: record.screen_tint.map(LinearRgba::from_f32_array),
        }),
    });
}

/// The ChaCha8 stream a record describes.
fn rng_from(record: &RngRecord) -> Option<rand_chacha::ChaCha8Rng> {
    if record.seed.len() != 64 {
        return None;
    }
    let mut seed = [0u8; 32];
    for (i, byte) in seed.iter_mut().enumerate() {
        *byte = u8::from_str_radix(record.seed.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    let word_pos: u128 = record.word_pos.parse().ok()?;
    let mut rng = rand_chacha::ChaCha8Rng::from_seed(seed);
    rng.set_stream(record.stream);
    rng.set_word_pos(word_pos);
    Some(rng)
}
