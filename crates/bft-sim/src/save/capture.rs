//! Reading a running mission into a [`SaveFile`].
//!
//! Two passes. The first gives every entity a save could mention a stable
//! [`SaveRef`] — level nodes by id, enemies by instance, copies and projectiles
//! by their place in the file — and the second writes the records, turning
//! each `Entity` they hold into its `SaveRef`. An entity with no `SaveRef` is written
//! as nothing, which is what a reader does with a reference it cannot
//! resolve anyway.

use std::collections::{HashMap, HashSet};

use avian3d::prelude::{AngularVelocity, LinearVelocity};
use bevy::prelude::*;
use bft_assets::anim::AnimActivation;

use super::format::*;
use super::{Fnv64, PendingAnimRestore, SaveBaseline};
use crate::ai::{AiBrain, AiTick, Enemy, TurretAi};
use crate::anim::{
    AnimCamera, AnimClock, AnimInstance, AnimLights, AnimMotions, AnimOpacity, AnimPlaced,
    AnimProgram, AnimRest, AnimScreenFx, ConnectorEnd,
};
use crate::cinematic::PlayerControl;
use crate::crater::Cratered;
use crate::damage::{Burning, Health};
use crate::level::{LevelRoot, LodSwitch, ModelRef, NodeActive, NodeId, NodeName, WorldEnv};
use crate::mission::{MissionClock, MissionState, Progress, Score};
use crate::physics::SimRng;
use crate::pickups::{Pickup, RespawnTimer};
use crate::states::GameMode;
use crate::vehicle::tuning::Difficulty;
use crate::vehicle::{PlayerVehicle, Turret, Vehicle};
use crate::weapons::{Projectile, TwinBarrels, WeaponLoadout, WeaponTable};

/// What the caller knows that the world does not.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CaptureMeta {
    /// The name the load list shows.
    pub label: String,
    /// The catalog's directory name, `"m1"`.
    pub level_name: String,
    /// `MSG_CAMPAIGN1_NAME`.
    pub title_key: String,
    /// `"open-bft 0.1.0"`.
    pub written_by: String,
    /// Unix seconds.
    pub created: u64,
}

/// Why a save cannot be made now (SAVEGAME §8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SaveRefusal {
    #[error("there is no campaign mission running")]
    NoMission,
    #[error("saving is not available in multiplayer")]
    Multiplayer,
    #[error("cannot save while the BFT is being transported")]
    Cinematic,
    #[error("the mission is over")]
    MissionOver,
    #[error("there is no player to save")]
    NoPlayer,
}

impl SaveRefusal {
    /// The original's message for it, where it had one.
    pub fn message_key(&self) -> Option<&'static str> {
        match self {
            SaveRefusal::Multiplayer => Some("MSG_CANT_SAVE_MP"),
            SaveRefusal::Cinematic => Some("MSG_CANT_SAVE"),
            _ => None,
        }
    }
}

/// Whether the world may be saved now.
pub fn can_save(world: &World) -> Result<(), SaveRefusal> {
    save_refusal(
        world
            .get_resource::<State<GameMode>>()
            .map(|mode| mode.get()),
        world.get_resource::<MissionState>(),
        world.get_resource::<PlayerControl>(),
    )
    .map_or(Ok(()), Err)
}

/// Why a save would be refused, from the three things that decide it.
///
/// For a menu that wants to grey its button out without world access.
pub fn save_refusal(
    mode: Option<&GameMode>,
    state: Option<&MissionState>,
    control: Option<&PlayerControl>,
) -> Option<SaveRefusal> {
    if mode.is_some_and(|mode| *mode != GameMode::Campaign) {
        return Some(SaveRefusal::Multiplayer);
    }
    let Some(state) = state else {
        return Some(SaveRefusal::NoMission);
    };
    if state.outcome.is_some() || state.dying {
        return Some(SaveRefusal::MissionOver);
    }
    if control.is_some_and(PlayerControl::is_cinematic) {
        return Some(SaveRefusal::Cinematic);
    }
    None
}

/// Capture the running mission.
pub fn capture(world: &mut World, meta: CaptureMeta) -> Result<SaveFile, SaveRefusal> {
    can_save(world)?;

    let player = {
        let mut players = world.query_filtered::<Entity, With<PlayerVehicle>>();
        players.iter(world).next().ok_or(SaveRefusal::NoPlayer)?
    };

    let refs = Refs::build(world, player);
    // Owned, because the records below query the world mutably. A save is
    // rare enough that the copy costs nothing worth noticing.
    let weapons = world.get_resource::<WeaponTable>().cloned();
    let weapons = weapons.as_ref();

    let mut save = SaveFile {
        format: FORMAT_TAG.to_string(),
        version: CURRENT_VERSION,
        header: header(world, meta),
        campaign: campaign(world),
        world: world_record(world),
        nodes: node_records(world, &refs),
        craters: crater_records(world),
        player: None,
        enemies: Vec::new(),
        turrets: Vec::new(),
        pickups: Vec::new(),
        projectiles: Vec::new(),
        anims: AnimsRecord::default(),
    };

    save.player = vehicle_record(world, player, weapons).map(|vehicle| PlayerRecord {
        vehicle,
        twin_next: world
            .get::<TwinBarrels>(player)
            .map(|twins| twins.next as u8),
    });
    save.enemies = enemy_records(world, &refs, weapons);
    save.turrets = turret_records(world, &refs, weapons);
    save.pickups = refs
        .pickups
        .iter()
        .filter_map(|entity| pickup_record(world, *entity))
        .collect();
    save.projectiles = refs
        .projectiles
        .iter()
        .filter_map(|entity| projectile_record(world, *entity, &refs, weapons))
        .collect();
    save.anims = anim_records(world, &refs);

    Ok(save)
}

// ---------------------------------------------------------------------------
// References
// ---------------------------------------------------------------------------

/// Every entity a record may mention, and what it is called in the file.
pub(super) struct Refs {
    map: HashMap<Entity, SaveRef>,
    /// The copies' instance entities, in file order.
    copies: Vec<Entity>,
    pickups: Vec<Entity>,
    projectiles: Vec<Entity>,
    /// The enemy-model parts, for the node records.
    parts: Vec<(Entity, SaveRef)>,
    level_roots: HashSet<Entity>,
}

impl Refs {
    fn build(world: &mut World, player: Entity) -> Refs {
        let mut map = HashMap::new();
        map.insert(player, SaveRef::Player);

        let mut nodes = world.query::<(Entity, &NodeId)>();
        for (entity, id) in nodes.iter(world) {
            map.insert(entity, SaveRef::Node(id.0));
        }

        let mut parts = Vec::new();
        let mut enemies = world.query::<(Entity, &Enemy)>();
        let enemies: Vec<(Entity, String)> = enemies
            .iter(world)
            .map(|(entity, enemy)| (entity, enemy.instance.clone()))
            .collect();
        for (hull, instance) in enemies {
            map.insert(hull, SaveRef::Enemy(instance.clone()));
            for (index, part) in named_subtree(world, hull).into_iter().enumerate() {
                let reference = SaveRef::EnemyPart(instance.clone(), index as u32);
                map.insert(part, reference.clone());
                parts.push((part, reference));
            }
        }

        // Instances: the definitions' own first, then the copies in a stable
        // order so two saves of the same state number them alike.
        let mut instances = world.query::<(Entity, &AnimInstance)>();
        let mut copies = Vec::new();
        for (entity, instance) in instances.iter(world) {
            match instance.copy {
                None => {
                    map.insert(entity, SaveRef::Anim(instance.def as u32));
                }
                Some(_) => copies.push((instance.def, entity)),
            }
        }
        copies.sort_by_key(|(def, entity)| (*def, entity.to_bits()));
        let copies: Vec<Entity> = copies.into_iter().map(|(_, entity)| entity).collect();
        for (index, entity) in copies.iter().enumerate() {
            map.insert(*entity, SaveRef::Copy(index as u32));
            let Some(copy) = world.get::<AnimInstance>(*entity).and_then(|i| i.copy) else {
                continue;
            };
            if copy.owned {
                for (part, node) in named_subtree(world, copy.root).into_iter().enumerate() {
                    map.insert(node, SaveRef::CopyPart(index as u32, part as u32));
                }
            }
        }

        let mut pickups = world.query::<(Entity, &Pickup)>();
        let mut pickups: Vec<(String, Entity)> = pickups
            .iter(world)
            .map(|(entity, pickup)| (pickup.instance.clone(), entity))
            .collect();
        pickups.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.to_bits().cmp(&b.1.to_bits())));
        let pickups: Vec<Entity> = pickups.into_iter().map(|(_, entity)| entity).collect();
        for (index, entity) in pickups.iter().enumerate() {
            map.insert(*entity, SaveRef::Pickup(index as u32));
        }

        let mut projectiles = world.query_filtered::<Entity, With<Projectile>>();
        let mut projectiles: Vec<Entity> = projectiles.iter(world).collect();
        projectiles.sort_by_key(|entity| entity.to_bits());
        for (index, entity) in projectiles.iter().enumerate() {
            map.insert(*entity, SaveRef::Projectile(index as u32));
        }

        let mut roots = world.query_filtered::<Entity, With<LevelRoot>>();
        let level_roots = roots.iter(world).collect();

        Refs {
            map,
            copies,
            pickups,
            projectiles,
            parts,
            level_roots,
        }
    }

    fn get(&self, entity: Entity) -> Option<SaveRef> {
        self.map.get(&entity).cloned()
    }

    fn end(&self, end: &ConnectorEnd) -> EndRecord {
        EndRecord {
            node: end.node.and_then(|node| self.get(node)),
            offset: end.offset.to_array(),
            frozen: end.frozen,
        }
    }
}

/// The named nodes under `root`, breadth-first, `root` first if it is one:
/// the part numbering of SAVEGAME §5.
pub(super) fn named_subtree(world: &World, root: Entity) -> Vec<Entity> {
    let mut found = Vec::new();
    let mut queue = std::collections::VecDeque::from([root]);
    while let Some(entity) = queue.pop_front() {
        if world.get::<NodeName>(entity).is_some() {
            found.push(entity);
        }
        if let Some(children) = world.get::<Children>(entity) {
            queue.extend(children.iter());
        }
    }
    found
}

// ---------------------------------------------------------------------------
// Header, campaign, world
// ---------------------------------------------------------------------------

fn header(world: &mut World, meta: CaptureMeta) -> SaveHeader {
    let level = world
        .get_resource::<MissionState>()
        .map(|state| state.mission.0)
        .unwrap_or_default();
    SaveHeader {
        written_by: meta.written_by,
        created: meta.created,
        label: meta.label,
        level,
        level_name: meta.level_name,
        title_key: meta.title_key,
        difficulty: world
            .get_resource::<Difficulty>()
            .copied()
            .unwrap_or_default()
            .into(),
        play_time: world
            .get_resource::<AnimClock>()
            .map_or(0.0, |clock| clock.0),
        fingerprint: fingerprint(world),
    }
}

/// What game data this world was loaded from (SAVEGAME §6, `header`).
pub fn fingerprint(world: &mut World) -> Fingerprint {
    let mut nodes = world.query::<(&NodeId, &NodeName)>();
    let mut nodes: Vec<(u32, String)> = nodes
        .iter(world)
        .map(|(id, name)| (id.0, name.0.clone()))
        .collect();
    nodes.sort();

    let mut node_hash = Fnv64::default();
    for (id, name) in &nodes {
        node_hash.write(&id.to_le_bytes());
        node_hash.write(name.as_bytes());
        node_hash.write(&[0]);
    }

    let mut anim_hash = Fnv64::default();
    let anims = world.get_resource::<AnimProgram>().map_or(0, |program| {
        for def in &program.0.defs {
            anim_hash.write(def.anim_name.as_bytes());
            anim_hash.write(&[0]);
        }
        program.0.defs.len()
    });

    Fingerprint {
        nodes: nodes.len() as u32,
        node_hash: node_hash.finish(),
        anims: anims as u32,
        anim_hash: anim_hash.finish(),
    }
}

fn campaign(world: &World) -> CampaignRecord {
    let Some(progress) = world.get_resource::<Progress>() else {
        return CampaignRecord::default();
    };
    CampaignRecord {
        completed: progress.completed.clone(),
        unlocked: progress.unlocked.forms().map(Form::from).collect(),
        mounted: progress.mounted.clone(),
    }
}

fn world_record(world: &World) -> WorldRecord {
    let rng = world.get_resource::<SimRng>().map(|rng| {
        let seed: String = rng
            .0
            .get_seed()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        RngRecord {
            seed,
            stream: rng.0.get_stream(),
            word_pos: rng.0.get_word_pos().to_string(),
        }
    });

    let mission = world
        .get_resource::<MissionState>()
        .map(|state| MissionRecord {
            objectives: state
                .objectives
                .iter()
                .copied()
                .map(Objective::from)
                .collect(),
            outcome: state.outcome.map(MissionOutcome::from),
            started: state.started,
            dying: state.dying,
            elapsed: world.get_resource::<MissionClock>().map(|clock| clock.0),
            score: world.get_resource::<Score>().map_or(0, |score| score.0),
        });

    let fog = world.get_resource::<WorldEnv>().map(|env| FogRecord {
        color: env.fog_color.to_linear().to_f32_array(),
        range: [env.fog_range.0, env.fog_range.1],
        altitude: [env.fog_altitude.0, env.fog_altitude.1],
    });

    let pending = world.get_resource::<PendingAnimRestore>();
    let camera = pending
        .and_then(|pending| pending.camera.as_ref())
        .or(world.get_resource::<AnimCamera>())
        .filter(|camera| **camera != AnimCamera::default())
        .map(|camera| CameraRecord {
            zoom: camera.zoom.to_array(),
            clip_near: camera.clip_near,
            clip_far: camera.clip_far,
        });

    WorldRecord {
        clock: world
            .get_resource::<AnimClock>()
            .map_or(0.0, |clock| clock.0),
        ai_tick: world.get_resource::<AiTick>().map_or(0, |tick| tick.0),
        rng,
        mission,
        fog,
        camera,
        screen_tint: pending
            .and_then(|pending| pending.screen.as_ref())
            .or(world.get_resource::<AnimScreenFx>())
            .and_then(|screen| screen.tint)
            .map(|tint| tint.to_f32_array()),
    }
}

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

fn node_records(world: &mut World, refs: &Refs) -> Vec<NodeRecord> {
    let baseline = world
        .get_resource::<SaveBaseline>()
        .cloned()
        .unwrap_or_default();
    if baseline.nodes.is_empty() {
        warn!("no save baseline was recorded; every node's transform is saved");
    }

    let mut ids = world.query::<(Entity, &NodeId)>();
    let mut level: Vec<(u32, Entity)> = ids.iter(world).map(|(e, id)| (id.0, e)).collect();
    level.sort_by_key(|(id, _)| *id);

    let mut records = Vec::new();
    for (id, entity) in level {
        if let Some(record) = node_record(world, entity, SaveRef::Node(id), None, &baseline, refs) {
            records.push(record);
        }
    }
    for (entity, reference) in &refs.parts {
        let name = world.get::<NodeName>(*entity).map(|name| name.0.clone());
        if let Some(record) = node_record(world, *entity, reference.clone(), name, &baseline, refs)
        {
            records.push(record);
        }
    }
    records
}

/// One node's differences from the baseline, if it has any.
fn node_record(
    world: &World,
    entity: Entity,
    node: SaveRef,
    name: Option<String>,
    baseline: &SaveBaseline,
    refs: &Refs,
) -> Option<NodeRecord> {
    let base = baseline.nodes.get(&entity);
    let mut record = NodeRecord::new(node);

    if let Some(transform) = world.get::<Transform>(entity)
        && base.is_none_or(|base| base.transform != *transform)
    {
        record.t = Some((*transform).into());
    }

    let active = world
        .get::<NodeActive>(entity)
        .is_none_or(|active| active.0);
    if base.map_or(!active, |base| base.active != active) {
        record.active = Some(active);
    }

    let parent = world.get::<ChildOf>(entity).map(ChildOf::parent);
    if let Some(base) = base
        && base.parent != parent
    {
        record.parent = match parent {
            Some(parent) if refs.level_roots.contains(&parent) => Some(Parent::LevelRoot),
            Some(parent) => match refs.get(parent) {
                Some(reference) => Some(Parent::Of(reference)),
                None => {
                    warn!("a node was re-parented onto an entity a save cannot name; not saved");
                    None
                }
            },
            None => Some(Parent::LevelRoot),
        };
    }

    record.opacity = world.get::<AnimOpacity>(entity).map(|opacity| opacity.0);
    record.rest = world.get::<AnimRest>(entity).map(|rest| rest.0.into());
    record.placed = world.get::<AnimPlaced>(entity).map(|_| true);

    if record.is_empty() {
        return None;
    }
    record.name = name;
    Some(record)
}

fn crater_records(world: &mut World) -> Vec<CraterNodeRecord> {
    let mut query = world.query::<(&NodeId, &Cratered)>();
    let mut records: Vec<CraterNodeRecord> = query
        .iter(world)
        .map(|(id, cratered)| CraterNodeRecord {
            node: id.0,
            craters: cratered
                .craters
                .iter()
                .map(|crater| CraterRecord {
                    center: crater.center.to_array(),
                    radius: crater.radius,
                    floor_radius: crater.floor_radius,
                    depth: crater.depth,
                    sides: crater.sides as u32,
                    texture: crater.texture.clone(),
                    uv_scale: crater.uv_scale.to_array(),
                })
                .collect(),
        })
        .collect();
    records.sort_by_key(|record| record.node);
    records
}

// ---------------------------------------------------------------------------
// Vehicles and turrets
// ---------------------------------------------------------------------------

fn vehicle_record(
    world: &World,
    entity: Entity,
    weapons: Option<&WeaponTable>,
) -> Option<VehicleRecord> {
    let transform = world.get::<Transform>(entity)?;
    let vehicle = world.get::<Vehicle>(entity)?;
    let turret = world.get::<Turret>(entity).copied().unwrap_or_default();

    Some(VehicleRecord {
        t: (*transform).into(),
        velocity: world
            .get::<LinearVelocity>(entity)
            .map_or([0.0; 3], |velocity| velocity.0.to_array()),
        spin: world
            .get::<AngularVelocity>(entity)
            .map_or([0.0; 3], |velocity| velocity.0.to_array()),
        form: vehicle.form.into(),
        unlocked: vehicle.unlocked.forms().map(Form::from).collect(),
        turret: [turret.yaw, turret.pitch],
        health: world.get::<Health>(entity).map(health_record),
        burning: world.get::<Burning>(entity).map(burning_record),
        loadout: world
            .get::<WeaponLoadout>(entity)
            .and_then(|loadout| loadout_record(loadout, weapons)),
    })
}

fn health_record(health: &Health) -> HealthRecord {
    HealthRecord {
        current: health.current,
        max: health.max,
    }
}

fn burning_record(burning: &Burning) -> BurningRecord {
    BurningRecord {
        remaining: burning.remaining,
        dps: burning.dps,
    }
}

/// A loadout, with its weapons named by id. `None` without a weapon table,
/// since an index alone would mean nothing to the next build.
fn loadout_record(loadout: &WeaponLoadout, weapons: Option<&WeaponTable>) -> Option<LoadoutRecord> {
    let Some(weapons) = weapons else {
        warn!("no weapon table; a loadout is not saved");
        return None;
    };
    let ammo = |state: Option<&crate::weapons::AmmoState>| {
        let state = state?;
        let def = weapons.by_index(state.weapon)?;
        Some(AmmoRecord {
            weapon: def.id.clone(),
            ammo: state.ammo,
            mounted: state.mounted,
        })
    };

    let slots = (1..=9u8)
        .filter_map(|slot| {
            let state = loadout.slot(slot)?;
            let record = SlotRecord {
                slot,
                weak: ammo(state.weak.as_ref()),
                strong: ammo(state.strong.as_ref()),
            };
            (record.weak.is_some() || record.strong.is_some()).then_some(record)
        })
        .collect();

    Some(LoadoutRecord {
        slots,
        selected: loadout.selected,
        strong: loadout.strong,
        cooldown: loadout.cooldown,
        secondary_strong: loadout.secondary_strong,
        secondary_cooldown: loadout.secondary_cooldown,
    })
}

fn enemy_records(
    world: &mut World,
    refs: &Refs,
    weapons: Option<&WeaponTable>,
) -> Vec<EnemyRecord> {
    let mut query = world.query::<(Entity, &Enemy, &AiBrain)>();
    let enemies: Vec<(Entity, Enemy, AiBrain)> = query
        .iter(world)
        .map(|(entity, enemy, brain)| (entity, enemy.clone(), brain.clone()))
        .collect();

    let mut records: Vec<EnemyRecord> = enemies
        .into_iter()
        .filter_map(|(entity, enemy, brain)| {
            Some(EnemyRecord {
                instance: enemy.instance,
                kind: enemy.kind.0,
                vehicle: vehicle_record(world, entity, weapons)?,
                brain: BrainRecord {
                    mode: brain.state.into(),
                    target: brain.target.and_then(|target| refs.get(target)),
                    timer: brain.timer,
                    waypoint: brain.waypoint.map(|waypoint| waypoint as u32),
                    forward: brain.forward,
                    los_lost: brain.los_lost,
                    since_seen: brain.since_seen,
                },
            })
        })
        .collect();
    records.sort_by(|a, b| a.instance.cmp(&b.instance));
    records
}

fn turret_records(
    world: &mut World,
    refs: &Refs,
    weapons: Option<&WeaponTable>,
) -> Vec<TurretRecord> {
    let mut query = world.query::<(Entity, &NodeId, &TurretAi)>();
    let turrets: Vec<(Entity, u32, TurretAi)> = query
        .iter(world)
        .map(|(entity, id, turret)| (entity, id.0, turret.clone()))
        .collect();

    let mut records: Vec<TurretRecord> = turrets
        .into_iter()
        .map(|(entity, node, turret)| TurretRecord {
            node,
            next_muzzle: turret.next_muzzle as u32,
            fire_timer: turret.fire_timer,
            burst_left: turret.burst_left,
            dwell_left: turret.dwell_left,
            aim: turret.aim.to_array(),
            target: turret.target.and_then(|target| refs.get(target)),
            deactivated: turret.deactivated,
            health: world.get::<Health>(entity).map(health_record),
            burning: world.get::<Burning>(entity).map(burning_record),
            loadout: world
                .get::<WeaponLoadout>(entity)
                .and_then(|loadout| loadout_record(loadout, weapons)),
        })
        .collect();
    records.sort_by_key(|record| record.node);
    records
}

// ---------------------------------------------------------------------------
// Pickups and projectiles
// ---------------------------------------------------------------------------

fn pickup_record(world: &World, entity: Entity) -> Option<PickupRecord> {
    let pickup = world.get::<Pickup>(entity)?;
    let at = world.get::<Transform>(entity)?.translation;
    Some(PickupRecord {
        kind: pickup.kind.clone(),
        amount: pickup.amount,
        instance: pickup.instance.clone(),
        at: at.to_array(),
        respawn: pickup.respawn,
        respawn_in: world
            .get::<RespawnTimer>(entity)
            .map(|timer| timer.remaining),
    })
}

fn projectile_record(
    world: &World,
    entity: Entity,
    refs: &Refs,
    weapons: Option<&WeaponTable>,
) -> Option<ProjectileRecord> {
    let projectile = world.get::<Projectile>(entity)?;
    let transform = world.get::<Transform>(entity)?;
    let weapon = weapons?.by_index(projectile.weapon)?.id.clone();
    Some(ProjectileRecord {
        weapon,
        owner: refs.get(projectile.owner),
        team: projectile.team.0,
        t: (*transform).into(),
        velocity: projectile.velocity.to_array(),
        age: projectile.age,
        target: projectile.target.and_then(|target| refs.get(target)),
        armed: projectile.armed,
        health: projectile.health,
        landed: projectile.landed.map(|landing| LandingRecord {
            normal: landing.normal.to_array(),
            surface: landing.surface.into(),
        }),
    })
}

// ---------------------------------------------------------------------------
// The anim VM
// ---------------------------------------------------------------------------

fn anim_records(world: &mut World, refs: &Refs) -> AnimsRecord {
    // Names and activations only: the query below needs the world mutably.
    let Some(defs) = world.get_resource::<AnimProgram>().map(|program| {
        program
            .0
            .defs
            .iter()
            .map(|def| (def.anim_name.clone(), def.activation))
            .collect::<Vec<(String, AnimActivation)>>()
    }) else {
        return AnimsRecord::default();
    };

    let mut query = world.query::<&AnimInstance>();
    let mut instances: Vec<AnimRecord> = query
        .iter(world)
        .filter(|instance| instance.copy.is_none())
        .filter_map(|instance| {
            let (name, activation) = defs.get(instance.def)?;
            (!is_fresh(instance, *activation)).then(|| anim_record(instance, name, refs))
        })
        .collect();
    instances.sort_by_key(|record| record.def);

    let copies = refs
        .copies
        .iter()
        .filter_map(|entity| {
            let instance = world.get::<AnimInstance>(*entity)?;
            let copy = instance.copy?;
            let (name, _) = defs.get(instance.def)?;
            let subtree = if copy.owned {
                CopySubtree::Owned(clone_nodes(world, copy.root))
            } else {
                CopySubtree::On(refs.get(copy.root)?)
            };
            Some(CopyRecord {
                instance: anim_record(instance, name, refs),
                subtree,
            })
        })
        .collect();

    // A restore's motions and lights wait for the first tick; a save made
    // before it must still carry them.
    let pending = world.get_resource::<PendingAnimRestore>();
    let mut motions: Vec<MotionRecord> = world
        .get_resource::<AnimMotions>()
        .into_iter()
        .flat_map(|motions| motions.0.iter())
        .chain(
            pending
                .into_iter()
                .flat_map(|pending| pending.motions.iter().map(|(node, motion)| (node, motion))),
        )
        .filter_map(|(node, motion)| {
            Some(MotionRecord {
                node: refs.get(*node)?,
                owner: refs.get(motion.owner.instance)?,
                seq: (motion.owner.seq != usize::MAX).then_some(motion.owner.seq as u32),
                event: motion.owner.event as u32,
                velocity: motion.velocity.to_array(),
                acceleration: motion.acceleration.to_array(),
                gravity: motion.gravity,
                lands: motion.lands,
                spin: motion.spin.to_array(),
                spin_rate: motion.spin_rate.to_array(),
                tumble: motion.tumble,
                tumble_rate: motion.tumble_rate,
                growth: motion.growth.to_array(),
                growth_rate: motion.growth_rate.to_array(),
                run_time: motion.run_time,
                bounce_sequence: motion.bounce_sequence.clone(),
                bounce_sound: motion.bounce_sound.clone(),
                age: motion.age,
                world: motion
                    .world
                    .map(|(position, velocity)| (position.to_array(), velocity.to_array())),
            })
        })
        .collect();
    motions.sort_by(|a, b| format!("{:?}", a.node).cmp(&format!("{:?}", b.node)));

    let mut lights: Vec<LightRecord> = world
        .get_resource::<AnimLights>()
        .into_iter()
        .flat_map(|lights| lights.0.iter())
        .chain(
            pending
                .into_iter()
                .flat_map(|pending| pending.lights.iter().map(|(key, light)| (key, light))),
        )
        .filter_map(|((owner, name), light)| {
            Some(LightRecord {
                owner: refs.get(*owner)?,
                name: name.clone(),
                active: light.active,
                color: light.light.color.to_linear().to_f32_array(),
                near: light.light.near,
                far: light.light.far,
                at: light
                    .at
                    .and_then(|(node, offset)| Some((refs.get(node)?, offset.to_array()))),
                animation: light.animation.map(|animation| LightAnimationRecord {
                    range_rate: animation.range_rate.to_array(),
                    color_rate: animation.color_rate.to_array(),
                    remaining: animation.remaining,
                }),
            })
        })
        .collect();
    lights.sort_by(|a, b| {
        format!("{:?}", a.owner)
            .cmp(&format!("{:?}", b.owner))
            .then_with(|| a.name.cmp(&b.name))
    });

    AnimsRecord {
        instances,
        copies,
        motions,
        lights,
    }
}

/// Whether an instance is as the level spawned it, and so needs no record.
///
/// `pending_reset` is left out: the first tick clears it on every instance,
/// and a restore clears it on every instance it does not have a record for.
pub(super) fn is_fresh(instance: &AnimInstance, activation: AnimActivation) -> bool {
    instance.armed == (activation != AnimActivation::OnCall)
        && instance.running.is_empty()
        && instance.damage == 0.0
        && instance.reset_at.is_none()
        && !instance.invalidated
        && !instance.started
        && !instance.completed
        && instance.input.is_none()
        && instance.input_ends.is_none()
        && instance.played_on.is_none()
        && instance.operand_scope.is_none()
        && !instance.skip
        && !instance.ran_out
}

fn anim_record(instance: &AnimInstance, name: &str, refs: &Refs) -> AnimRecord {
    AnimRecord {
        def: instance.def as u32,
        name: name.to_string(),
        armed: instance.armed,
        invalidated: instance.invalidated,
        started: instance.started,
        completed: instance.completed,
        damage: instance.damage,
        reset_at: instance.reset_at,
        running: instance
            .running
            .iter()
            .map(|cursor| CursorRecord {
                seq: cursor.seq as u32,
                event: cursor.event as u32,
                t_anim: cursor.t_anim,
                t_seq: cursor.t_seq,
                t_event: cursor.t_event,
                started: cursor.started,
                t_running: cursor.t_running,
                loops_left: cursor.loops_left,
                conds: cursor
                    .cond_stack
                    .iter()
                    .map(|frame| (frame.taking, frame.satisfied))
                    .collect(),
            })
            .collect(),
        input: instance.input.and_then(|input| refs.get(input)),
        input_ends: instance
            .input_ends
            .map(|(from, to)| [refs.end(&from), refs.end(&to)]),
        played_on: instance.played_on.and_then(|node| refs.get(node)),
        scope: instance.operand_scope.as_ref().map(|scope| {
            let mut entries: Vec<ScopeEntry> = scope
                .iter()
                .map(|(name, node)| ScopeEntry {
                    name: name.clone(),
                    node: node.and_then(|node| refs.get(node)),
                })
                .collect();
            entries.sort_by(|a, b| a.name.cmp(&b.name));
            entries
        }),
        skip: instance.skip,
        ran_out: instance.ran_out,
        pending_reset: instance.pending_reset,
    }
}

/// A copy's cloned subtree, breadth-first from its root.
fn clone_nodes(world: &World, root: Entity) -> Vec<CloneNode> {
    let nodes = named_subtree(world, root);
    let index: HashMap<Entity, u32> = nodes
        .iter()
        .enumerate()
        .map(|(i, entity)| (*entity, i as u32))
        .collect();

    nodes
        .iter()
        .map(|entity| {
            let entity = *entity;
            CloneNode {
                parent: world
                    .get::<ChildOf>(entity)
                    .and_then(|parent| index.get(&parent.parent()).copied()),
                name: world.get::<NodeName>(entity).map(|name| name.0.clone()),
                model: world.get::<ModelRef>(entity).map(|model| model.0 as u32),
                lod: world.get::<LodSwitch>(entity).map(|lod| LodRecord {
                    near: lod.near,
                    far: lod.far,
                    level: lod.level,
                }),
                t: world
                    .get::<Transform>(entity)
                    .copied()
                    .unwrap_or_default()
                    .into(),
                active: world
                    .get::<NodeActive>(entity)
                    .is_none_or(|active| active.0),
                opacity: world.get::<AnimOpacity>(entity).map(|opacity| opacity.0),
                rest: world.get::<AnimRest>(entity).map(|rest| rest.0.into()),
                placed: world.get::<AnimPlaced>(entity).is_some(),
            }
        })
        .collect()
}
