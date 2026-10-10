//! Enemies: where they come from, what they decide, and how they drive.
//!
//! # The data is the AI
//!
//! Nothing here invents a behaviour. Per campaign mission, `aiv{,_easy,_hard}
//! .zrd` places every enemy instance, `net_NN.zrd` gives its group an
//! activation radius, an attack radius, dwell times, a pursuit standoff, a
//! return range, an `attack_strategy` and a waypoint graph, and `ai.zrd`
//! defines the fixed emplacements by gamez node-name glob (READER-SCHEMA
//! §3.1–3.4). All of it is read in [`table`]. What this crate adds is the
//! steering that carries those numbers out, and its own knobs live in
//! [`tuning`] and `assets/tuning/ai.ron` with `// approximation` on each.
//!
//! # Two kinds of enemy, one weapon path
//!
//! A **vehicle** enemy is a clone of the level's template subtree for its
//! kind, carrying the same [`Vehicle`] controller the player has with
//! [`Controlled::Ai`], a [`WeaponLoadout`] from its `vehicle.zrd` row and an
//! [`AiBrain`]. It is driven by writing [`PlayerInput`], so every force,
//! suspension ray and form rule that applies to the player applies to it
//! unchanged (FR-A5) — and so a bug in the controller shows up on 84 enemies
//! rather than hiding behind a separate code path.
//!
//! A **turret** is a gamez node that already exists in the world. It does not
//! move; it yaws one child node, pitches another and fires from a third, all
//! named by its `ai.zrd` `PARTS`. Both write [`FireRequest`], and the weapons
//! do not know which was which.
//!
//! Air units — VTOLs, helicopters, parachutes — are **anim-scripted set
//! pieces**, not agents (DESIGN §7.5), so there is no air brain. The helicopter's
//! *guns* are turrets like any other, matched by the `hel_*1` globs.
//!
//! # One tick of a brain
//!
//! In [`SimSet::Ai`], after damage so a brain never steers a corpse:
//!
//! 1. [`perceive`] — every `reaction_ticks`, and phased across entities:
//!    an awake brain looks for the nearest hostile inside the detection
//!    range with clear line of sight; a sleeping one only measures how far
//!    the nearest hostile is, which is all waking needs.
//! 2. [`think`] — the state machine, which is the data's:
//!    [`Dormant`](AiState::Dormant) until the player is inside the net's
//!    `activate_rad` or a hit lands — walking the net as
//!    [`Roam`](AiState::Roam) meanwhile once the player is inside our own
//!    `roam_range` — [`Patrol`](AiState::Patrol) along the graph,
//!    [`Engage`](AiState::Engage) applying the authored strategy,
//!    [`Return`](AiState::Return) beyond `return_range`,
//!    [`Hide`](AiState::Hide) for `defilade`/`hide` nets.
//! 3. [`steer`] — a desired point becomes throttle and steering, with three
//!    whisker rays turning it away from walls.
//! 4. [`aim`] — lead the target, add the error, write `aim_point` so the
//!    existing turret slew and ballistic solution do the rest, and pull the
//!    trigger when the barrel is near enough.
//! 5. [`drive_turrets`] — the same for the emplacements, which skip 3.

use avian3d::prelude::*;
use bevy::prelude::*;

use crate::damage::{Drops, Health, Team};
use crate::schedule::SimSet;
use crate::vehicle::tuning::{VehicleAiDefaults, VehicleDef, VehicleKindId, VehicleTable};
use crate::vehicle::{Controlled, PlayerInput, Turret, TurretRate, Vehicle, VehicleForm};
use crate::weapons::{Muzzle, WeaponLoadout, WeaponTable, loadout_from_vehicle};

pub mod brain;
pub mod overrides;
pub mod table;
pub mod templates;
pub mod tuning;

pub use brain::{
    aim, avoid_walls, desired_point, drive_toward, drive_turrets, has_line_of_sight, lay_mine,
    lead_point, nearest_visible, next_state, perceive, scatter, steer, think, wake_buddies,
    wake_on_damage,
};
pub use overrides::{AiKindOverride, AiOverrides, NetOverride, TurretOverride};
pub use table::{
    AiError, AiTable, AttackStrategy, NetDef, NetNode, NetType, TurretDef, UnitPlacement,
    build_ai_table,
};
pub use templates::{ClonedFrom, ClonedTemplate, LevelTemplates, Template, clone_template};
pub use tuning::SteeringTuning;

// ---------------------------------------------------------------------------
// Components
// ---------------------------------------------------------------------------

/// What an enemy is doing.
///
/// The states are the data's, not a generic FSM: each one is entered and left
/// on a value `net_NN.zrd` states. There is no `Flee`,
/// because no reader field describes one and the original's enemies press an
/// attack until they die.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AiState {
    /// Asleep: the player has not come within the net's `activate_rad`, and
    /// no buddy has woken the group.
    Dormant,
    /// Still asleep in the data's sense — not hunting, not shooting — but
    /// walking its net, because the player is inside
    /// [`SteeringTuning::roam_range`]. Ours, not the data's: see there.
    Roam,
    /// Walking the net's waypoint graph.
    Patrol,
    /// A target is acquired and the net's `attack_strategy` is running.
    Engage,
    /// Past `return_range` from home: going back, ignoring the target.
    Return,
    /// In cover, for a `defilade` or `hide` net's `hide_times`.
    Hide,
    /// Destroyed. The wreck stays; the brain stops.
    Dead,
}

impl AiState {
    /// Whether the brain drives the hull in this state.
    pub fn moves(&self) -> bool {
        matches!(
            self,
            AiState::Roam | AiState::Patrol | AiState::Engage | AiState::Return
        )
    }

    /// Whether `activate_rad`, a buddy or a hit has woken it: it hunts.
    pub fn awake(&self) -> bool {
        !matches!(self, AiState::Dormant | AiState::Roam | AiState::Dead)
    }

    /// Whether the brain may pull the trigger in this state.
    pub fn shoots(&self) -> bool {
        matches!(self, AiState::Engage)
    }
}

/// An enemy vehicle's brain.
#[derive(Component, Debug, Clone)]
pub struct AiBrain {
    /// The 1-based `net_NN` index this unit walks, or `None` for a unit whose
    /// `aiv.zrd` record states net 0.
    pub net: Option<usize>,
    pub state: AiState,
    /// What it is shooting at.
    pub target: Option<Entity>,
    /// Seconds in the current state, for the dwell timers.
    pub timer: f32,
    /// Which node of the net it is walking toward.
    pub waypoint: Option<usize>,
    /// Which way along the graph it is walking, so a chain is patrolled back
    /// and forth rather than the brain turning round at every node.
    pub forward: bool,
    /// Where it started: the `aiv.zrd` position, which `return_range` is
    /// measured from.
    pub home: Vec3,
    /// The net's `attack_strategy`, or `None` for the 13 nets that state none
    /// and for a unit with no net at all.
    pub strategy: Option<AttackStrategy>,
    /// The `vehicle.zrd` `common_mode` ranges, which stand in wherever the
    /// unit has no net to read them from.
    pub defaults: VehicleAiDefaults,
    /// Consecutive perception passes with no line of sight to the target.
    pub los_lost: u32,
    /// Seconds since the target was last seen, against `not_pursuit_dwell`.
    pub since_seen: f32,
    /// This brain's perception phase, so 50 enemies do not all cast on the
    /// same tick.
    pub phase: u32,
    /// The steering knobs for this kind, `ai.ron`'s per-kind layer applied.
    pub steering: SteeringTuning,
    /// The engagement range from the unit's own `vehicle.zrd` weapon row
    /// (column `d`: 325, 275 or 175 m).
    pub weapon_range: f32,
}

impl AiBrain {
    /// A brain for a placement, with its kind's ranges and steering.
    pub fn new(
        placement: &UnitPlacement,
        net: Option<&NetDef>,
        def: Option<&VehicleDef>,
        steering: SteeringTuning,
        phase: u32,
    ) -> AiBrain {
        AiBrain {
            net: placement.net,
            // Everything starts asleep, including a unit with no net: an
            // enemy 2 km away must not be steering before the player has
            // come near it.
            state: AiState::Dormant,
            target: None,
            timer: 0.0,
            waypoint: None,
            forward: true,
            home: placement.pos,
            strategy: net.and_then(|net| net.strategy),
            defaults: def.map(|def| def.ai.clone()).unwrap_or_default(),
            los_lost: 0,
            since_seen: f32::MAX,
            phase,
            steering,
            weapon_range: def
                .and_then(|def| def.weapons.first())
                .map(|row| row.range)
                .unwrap_or(0.0),
        }
    }

    /// How close the player has to be to wake this unit, in metres.
    ///
    /// The net's `activate_rad` where there is a net, else the kind's
    /// `activation` (or `activation2`, which the two drone kinds use
    /// instead). Both are in the data; neither is ours.
    pub fn activate_rad(&self, net: Option<&NetDef>) -> f32 {
        net.map(|net| net.activate_rad)
            .filter(|rad| *rad > 0.0)
            .or(self.defaults.activation)
            .unwrap_or(0.0)
    }

    /// [`Self::activate_rad`] with [`SteeringTuning::activation_scale`]
    /// applied: the radius the state machine actually wakes at.
    pub fn wake_rad(&self, net: Option<&NetDef>) -> f32 {
        self.activate_rad(net) * self.steering.activation_scale.max(0.0)
    }

    /// How close before it opens fire, in metres.
    ///
    /// The net's `attack_rad`, else the weapon row's own engagement range.
    pub fn attack_rad(&self, net: Option<&NetDef>) -> f32 {
        net.map(|net| net.attack_rad)
            .filter(|rad| *rad > 0.0)
            .unwrap_or(self.weapon_range)
    }

    /// How far from [`Self::home`] before it turns back, in metres.
    ///
    /// Zero — which 10 nets state — means no leash, and is returned as
    /// [`f32::INFINITY`] so the comparison reads the same either way.
    pub fn return_range(&self, net: Option<&NetDef>) -> f32 {
        let stated = net
            .map(|net| net.return_range)
            .filter(|range| *range > 0.0)
            .or(self.defaults.return_range);

        match stated {
            Some(range) if range > 0.0 => range,
            _ => f32::INFINITY,
        }
    }

    /// Seconds of lost contact before pursuit is dropped.
    pub fn not_pursuit_dwell(&self, net: Option<&NetDef>) -> f32 {
        net.map(|net| net.not_pursuit_dwell)
            .filter(|dwell| *dwell > 0.0)
            .or(self.defaults.not_pursuit_dwell)
            .unwrap_or(0.0)
    }

    /// The `[min, max]` standoff this unit keeps while attacking, in metres.
    ///
    /// The net's `pursuit_params`/`pursuit_range` where it states one — 165
    /// of 470 nets state `[0, 0]`, which is "no opinion" — else the kind's
    /// own `pursuit_range`, which every enemy kind has.
    pub fn pursuit(&self, net: Option<&NetDef>) -> [f32; 2] {
        net.map(|net| net.pursuit)
            .filter(|pursuit| pursuit[1] > 0.0)
            .or(self.defaults.pursuit_range)
            .unwrap_or([0.0, 0.0])
    }
}

/// An enemy vehicle, and which `aiv.zrd` record it came from.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct Enemy {
    /// The `aiv.zrd` key: `"scou_03"`.
    pub instance: String,
    /// The `vehicle.zrd` kind.
    pub kind: VehicleKindId,
}

/// A fixed emplacement from `ai.zrd`: a turret, a SAM site, a gun on a
/// helicopter.
#[derive(Component, Debug, Clone)]
pub struct TurretAi {
    /// Index into [`AiTable::turrets`].
    pub def: usize,
    /// The child node the `PARTS` list names as the yaw part, where it has
    /// one — the two-element form has none and only elevates.
    pub yaw_node: Option<Entity>,
    /// The child node to pitch.
    pub pitch_node: Option<Entity>,
    /// The `firepoint` nodes to fire from, in `PARTS` order.
    pub muzzles: Vec<Entity>,
    /// Every node of its own subtree, which its line-of-sight ray ignores.
    ///
    /// A turret is level geometry: its origin sits at ground level inside its
    /// own `base` (or the wall it is set into), so a ray from there that did
    /// not ignore the emplacement hit it within four metres and no turret in
    /// m1 ever saw the player.
    pub own_nodes: Vec<Entity>,
    /// Which muzzle fires next, for the one entry with two.
    pub next_muzzle: usize,
    /// Seconds until it may fire again.
    pub fire_timer: f32,
    /// Seconds left in the current burst, from `FIRE_LIMITS`.
    pub burst_left: f32,
    /// Seconds left of the pause after a burst, from `FIRE_DWELL`.
    pub dwell_left: f32,
    /// Current yaw and pitch of the parts, radians.
    pub aim: Vec2,
    /// What it is shooting at.
    pub target: Option<Entity>,
    /// Its perception phase.
    pub phase: u32,
    /// Whether a `DEACTIVATE` condition has switched it off.
    pub deactivated: bool,
}

/// An enemy came into the world.
///
/// A `Message`, not Bevy 0.19's observer `Event`: several systems may read
/// the stream (a HUD threat count, a spawn sound, scripted waves), which is
/// what `Message` is. `FormChanged` and `Destroyed` are the same.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct EnemySpawned(pub Entity);

/// How many ticks have been stepped, for the perception cadence.
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AiTick(pub u64);

// ---------------------------------------------------------------------------
// Spawning
// ---------------------------------------------------------------------------

/// How far above the reader's y an enemy is dropped in.
///
/// // approximation: the same reason and the same value as the player's
/// `SPAWN_CLEARANCE` — `aiv.zrd`'s y is ground height and a hull origin
/// placed exactly on it interpenetrates the terrain for a tick.
pub const SPAWN_CLEARANCE: f32 = 2.0;

/// Where an enemy's muzzle sits when its template names no firepoint.
///
/// // approximation: `aimy` is the turret's aim height above the hull origin
/// and the hull is about 7 m long, so this is the barrel's tip on a vehicle
/// the size of the player's. Every shipped enemy template does carry a
/// firepoint node, so this is the fallback for a level that does not.
pub const DEFAULT_MUZZLE_FORWARD: f32 = 3.5;

/// Spawn one enemy vehicle from its `aiv.zrd` record.
///
/// The body is the vehicle module's, built the way [`spawn_player_vehicle`] builds
/// the player's, and then given the AI's own components. It is *not* a call
/// to that function: the player's carries [`PlayerVehicle`] and
/// [`Controlled::Local`] and unlocks forms from a [`FormSet`], none of which
/// an enemy has — an enemy is in the one form its `vehicle.zrd` entry gives
/// it and never transforms.
///
/// The model is cloned from the level's template subtree for the kind, where
/// the level carries one. A level only carries the prototypes its own
/// `aiv.zrd` places, so a missing template is a warning and an invisible but
/// otherwise working enemy, not a failure: the simulation is the authority
/// and the mesh is presentation.
pub fn spawn_enemy(
    commands: &mut Commands,
    placement: &UnitPlacement,
    tables: EnemyTables<'_>,
    templates: Option<&EnemyTemplates<'_>>,
    steering: SteeringTuning,
    phase: u32,
) -> Entity {
    let EnemyTables {
        vehicles,
        weapons,
        ai,
    } = tables;

    let def = vehicles.kind(&placement.kind);
    if def.is_none() {
        warn!(
            "`{}` names vehicle kind `{}`, which vehicle.zrd has no entry for; \
             it spawns with no physics or weapons",
            placement.instance,
            placement.kind.as_str()
        );
    }

    // The one form this kind has. Every enemy entry carries exactly one mode
    // block (`track`, `hover`, `amphib` or `sub`) — the player is the only
    // kind with four — so there is no choice to make and no transforming.
    let form = def
        .and_then(|def| def.forms().next())
        .unwrap_or(VehicleForm::Track);
    let mode = def.and_then(|def| def.mode(form));

    let net = placement.net.and_then(|index| ai.net(index));
    let brain = AiBrain::new(placement, net, def, steering, phase);

    let (half, centre) = crate::vehicle::hull_box(mode);
    let mass = mode
        .and_then(|mode| mode.mass)
        .unwrap_or(crate::vehicle::tuning::DEFAULT_MASS)
        * crate::vehicle::tuning::MASS_UNIT_KG;
    let angular_damping =
        mode.and_then(|mode| mode.a_damping).unwrap_or(40.0) / crate::vehicle::A_DAMPING_DIVISOR;

    let at = Transform {
        translation: placement.pos + Vec3::Y * SPAWN_CLEARANCE,
        rotation: Quat::from_rotation_y(placement.heading),
        ..default()
    };

    let health = def.map(|def| def.health).unwrap_or(1.0);
    let loadout = match (weapons, def) {
        (Some(weapons), Some(def)) => enemy_loadout(def, weapons),
        _ => WeaponLoadout::default(),
    };
    let drops = def.map(|def| def.drops.clone()).unwrap_or_default();

    let hull = commands
        .spawn((
            Name::new(placement.instance.clone()),
            Enemy {
                instance: placement.instance.clone(),
                kind: placement.kind.clone(),
            },
            Vehicle {
                kind: placement.kind.clone(),
                form,
                unlocked: crate::vehicle::FormSet::of([form]),
            },
            brain,
            // The whole point of the AI path: an enemy is a vehicle that
            // something else fills the input of.
            Controlled::Ai,
            PlayerInput::default(),
            Turret::default(),
            def.and_then(|def| def.turret_rate)
                .map(|[yaw, pitch]| TurretRate { yaw, pitch })
                .unwrap_or_default(),
            crate::vehicle::GroundState::airborne(),
            crate::vehicle::Traction::default(),
            at,
            (
                Team::ENEMY,
                Health::new(health),
                Drops(drops),
                loadout,
                // // approximation: the real muzzle is the template's own
                // firepoint node (`fpnt_c` on the scout), which is attached
                // by `attach_enemy_model` once the clone exists. This is
                // where a shot comes from until then and for a level with no
                // template.
                Muzzle(Vec3::new(
                    0.0,
                    def.map(|def| def.aim_y[0]).unwrap_or(1.5),
                    -DEFAULT_MUZZLE_FORWARD,
                )),
            ),
            (
                RigidBody::Dynamic,
                Collider::compound(vec![(
                    centre,
                    Quat::IDENTITY,
                    Collider::cuboid(half.x * 2.0, half.y * 2.0, half.z * 2.0),
                )]),
                Mass(mass),
                crate::vehicle::center_of_mass(mode),
                NoAutoCenterOfMass,
                AngularDamping(angular_damping),
                LinearDamping(crate::vehicle::LINEAR_DAMPING),
                GravityScale(1.0),
                crate::physics::layers::vehicle(),
            ),
        ))
        .id();

    if let Some(templates) = templates {
        attach_enemy_model(commands, hull, placement, templates);
    }

    hull
}

/// The tables [`spawn_enemy`] reads.
///
/// A struct rather than a tuple: three same-shaped references
/// in a row are easy to transpose at a call site, and the weapon table can be
/// absent while a headless test is still worth running.
#[derive(Clone, Copy)]
pub struct EnemyTables<'a> {
    pub vehicles: &'a VehicleTable,
    pub weapons: Option<&'a WeaponTable>,
    pub ai: &'a AiTable,
}

/// What [`spawn_enemy`] needs to clone a body from.
#[derive(Clone, Copy)]
pub struct EnemyTemplates<'a> {
    pub level: &'a bft_assets::level::LevelData,
    pub axes: &'a bft_assets::axes::AxisConvention,
    pub templates: &'a LevelTemplates,
}

/// Clone the kind's template subtree under an enemy hull.
///
/// Also moves the [`Muzzle`] onto the template's own firepoint where it has
/// one, which is the difference between a scout's shots leaving its barrel
/// and leaving a guessed point in front of its nose.
fn attach_enemy_model(
    commands: &mut Commands,
    hull: Entity,
    placement: &UnitPlacement,
    templates: &EnemyTemplates<'_>,
) {
    let kind = placement.kind.as_str();
    let Some(root) = templates.templates.root(kind) else {
        // A level carries only the prototypes its own `aiv.zrd` places, so
        // this means the two files disagree — worth hearing about, not worth
        // refusing to spawn over.
        warn!(
            "this level has no `{kind}` template subtree, so `{}` spawns with no model",
            placement.instance
        );
        return;
    };

    let cloned = templates::clone_template(
        commands,
        templates.level,
        templates.axes,
        root,
        // Identity: the clone is a child of the hull, which already carries
        // the placement's position and heading.
        Transform::IDENTITY,
        &format!("{} model", placement.instance),
    );

    commands.entity(cloned.root).insert(ChildOf(hull));
    attach_aim_joints(commands, hull, &cloned);
}

/// The names an enemy model's shot leaves from, most specific first.
///
/// `firepoint` on the tanks, `fpnt_c` on the scout, and the drone and
/// hovertank have only the pair `fpnt_l`/`fpnt_r`, of which the left one
/// stands in for both.
const ENEMY_MUZZLE_NODES: [&str; 3] = ["firepoint", "fpnt_c", "fpnt_l"];

/// Mark a cloned model's `turret` and `gun` as the hull's aim joints.
///
/// The vehicle's `aim_turret` already slews every vehicle's [`Turret`],
/// enemies included, and writes the angles onto whatever [`AimJoints`] the
/// hull carries. An enemy had none, so its gun turned in the sim — the
/// shots went where it looked — while the model's turret stayed fixed over
/// the tracks. Every enemy prototype that has a turret calls it `turret`,
/// with the barrel as its child `gun`, both at identity rotation in the data:
/// the same two-joint shape as the player's `bft_00`. The RUMV's second
/// gunner (`turret_1`) has no aim of its own in the sim and stays still.
///
/// The muzzle follows the barrel too, as the player's does: [`Firepoint`]
/// has `follow_firepoints` move [`Muzzle`] onto it every tick, and
/// [`AimPivot`] measures the aim from the gun rather than the hull's origin.
///
/// [`AimJoints`]: crate::vehicle::AimJoints
/// [`Firepoint`]: crate::weapons::Firepoint
/// [`AimPivot`]: crate::vehicle::AimPivot
fn attach_aim_joints(commands: &mut Commands, hull: Entity, cloned: &templates::ClonedTemplate) {
    let Some(turret) = cloned.find("turret") else {
        // The locust, the amphibian and the sub carry their guns on the
        // chassis; `Turret` still aims their shots.
        return;
    };
    let gun = cloned.find("gun");

    commands.entity(turret).insert(crate::vehicle::TurretPart);
    if let Some(gun) = gun {
        commands.entity(gun).insert(crate::vehicle::GunPart);
    }
    commands.entity(hull).insert((
        crate::vehicle::AimJoints {
            yaw: turret,
            pitch: gun,
        },
        crate::vehicle::AimPivot::default(),
    ));

    if let Some(muzzle) = ENEMY_MUZZLE_NODES.iter().find_map(|name| cloned.find(name)) {
        commands
            .entity(hull)
            .insert(crate::weapons::Firepoint(muzzle));
    }
}

/// Build an enemy's loadout from its single `vehicle.zrd` weapon row.
///
/// [`loadout_from_vehicle`] is what the player uses; an
/// enemy needs one more thing from it. Every enemy row's `owned` column is 1
/// and its `ammo` is a magazine (5000 rounds of pulse gun, 12 missiles), so
/// the shared builder already does the right thing — but an enemy whose row
/// names a weapon the table lacks would end up with *nothing selected*, and a
/// brain that can never fire is worse than one firing the wrong gun. So the
/// selection is forced onto whatever slot the loadout actually got.
fn enemy_loadout(def: &VehicleDef, weapons: &WeaponTable) -> WeaponLoadout {
    let mut loadout = loadout_from_vehicle(def, weapons);

    if loadout.selected_ammo().is_none() {
        // The first slot that holds anything, weak variant first.
        for slot in 1..=9u8 {
            if loadout.select(slot) || loadout.is_selectable(slot) {
                break;
            }
        }
    }

    loadout
}

/// Spawn the brains for every `ai.zrd` turret node in a level.
///
/// The nodes already exist — a turret is baked into gamez at its position
/// (READER-SCHEMA §3.1) — so this attaches components rather than creating
/// bodies. `nodes` is the level's name→entity index.
///
/// Returns the entities that became turrets.
pub fn spawn_turrets(
    commands: &mut Commands,
    nodes: &crate::level::NodeIndex,
    names: &[(String, Entity)],
    ai: &AiTable,
    weapons: Option<&WeaponTable>,
    children: &Query<&Children>,
    node_names: &Query<&crate::level::NodeName>,
) -> Vec<Entity> {
    let _ = nodes;
    let mut spawned = Vec::new();

    for (name, entity) in names {
        let Some((index, def)) = ai.turret_for(name) else {
            continue;
        };

        // A SAM site shoots the set-piece VTOL, not the player, and the VTOL
        // is an anim sequence with no entity to shoot at. Spawning its brain
        // would make it track the player, which is exactly the mission-one
        // behaviour the `TARGETS` key exists to prevent.
        if !def.targets_player() {
            continue;
        }

        let parts = resolve_parts(*entity, def, children, node_names);

        let loadout = weapons
            .map(|weapons| turret_loadout(def, weapons))
            .unwrap_or_default();

        let mut turret = commands.entity(*entity);
        turret.insert((
            TurretAi {
                def: index,
                yaw_node: parts.yaw,
                pitch_node: parts.pitch,
                muzzles: parts.muzzles,
                own_nodes: parts.subtree.clone(),
                next_muzzle: 0,
                fire_timer: 0.0,
                burst_left: 0.0,
                dwell_left: 0.0,
                aim: Vec2::ZERO,
                target: None,
                phase: spawned.len() as u32,
                deactivated: false,
            },
            Team::ENEMY,
            loadout,
            // Its shots pass through its own barrel and base.
            crate::weapons::OwnParts(parts.subtree),
        ));

        // A turret with 1e9 or 0 health cannot be shot down and gets no
        // `Health` at all, rather than a number that damage would whittle
        // away at pointlessly — and the weapons' `TargetQuery` filters on
        // `Health`, so this is also what stops a homing missile locking onto
        // the helicopter's invulnerable guns.
        if def.is_destructible() {
            turret.insert(Health::new(def.health));
        }

        spawned.push(*entity);
    }

    spawned
}

/// The child entities a turret's `PARTS` names.
struct TurretParts {
    yaw: Option<Entity>,
    pitch: Option<Entity>,
    muzzles: Vec<Entity>,
    subtree: Vec<Entity>,
}

/// Find a turret's parts by name in its own subtree.
///
/// By search, not by path: `firepoint` is `pturret.flt/healthy/turret/gun/
/// firepoint` — a grandchild of the pitch part — and the depth differs per
/// model (`a2gun.flt` has it two levels up). Breadth-first, so the `healthy`
/// subtree's parts are found before the `destroyed` wreck's identically
/// named pieces.
fn resolve_parts(
    root: Entity,
    def: &TurretDef,
    children: &Query<&Children>,
    names: &Query<&crate::level::NodeName>,
) -> TurretParts {
    let find = |wanted: &str| find_descendant(root, wanted, children, names);

    TurretParts {
        yaw: def.yaw_part().and_then(find),
        pitch: def.pitch_part().and_then(find),
        muzzles: def
            .muzzle_parts()
            .iter()
            .filter_map(|name| find(name))
            .collect(),
        subtree: descendants(root, children),
    }
}

/// `root` and everything under it.
fn descendants(root: Entity, children: &Query<&Children>) -> Vec<Entity> {
    let mut all = Vec::new();
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        all.push(entity);
        if let Ok(children) = children.get(entity) {
            stack.extend(children.iter());
        }
    }
    all
}

/// The shallowest descendant of `root` with this node name.
pub fn find_descendant(
    root: Entity,
    name: &str,
    children: &Query<&Children>,
    names: &Query<&crate::level::NodeName>,
) -> Option<Entity> {
    let mut queue = std::collections::VecDeque::from([root]);

    while let Some(entity) = queue.pop_front() {
        if names.get(entity).is_ok_and(|node| node.0 == name) {
            return Some(entity);
        }
        if let Ok(children) = children.get(entity) {
            queue.extend(children.iter());
        }
    }

    None
}

/// A turret's loadout: its one `ai.zrd` weapon, with its own interval.
///
/// The interval is the turret's `FIRE_RATE`, which is seconds between shots
/// throughout this file — see [`TurretDef::fire_interval`] — and it goes into
/// the [`AmmoState`](crate::weapons::AmmoState) the same way a vehicle row's
/// does, so the weapons' cooldown needs no turret case.
pub fn turret_loadout(def: &TurretDef, weapons: &WeaponTable) -> WeaponLoadout {
    let mut loadout = WeaponLoadout::default();

    let Some(index) = weapons.index(&def.weapon) else {
        warn!(
            "ai.zrd `{}` names weapon `{}`, which weapons.zrd has no entry for",
            def.glob, def.weapon
        );
        return loadout;
    };

    let Some((slot, variant)) = crate::weapons::parse_id(&def.weapon) else {
        return loadout;
    };

    let state = crate::weapons::AmmoState {
        weapon: index,
        // No `AMMO` means no stated magazine, which reads as unlimited: the
        // 14 entries without one are the helicopter guns and the buoys, none
        // of which is meant to run dry.
        ammo: def.ammo.unwrap_or(u32::MAX),
        interval: def.fire_interval,
        mounted: true,
    };

    if let Some(slot_state) = loadout.slot_mut(slot) {
        if variant == 0 {
            slot_state.weak = Some(state);
        } else {
            slot_state.strong = Some(state);
        }
    }

    loadout.selected = slot;
    loadout.strong = variant != 0;
    loadout
}

// ---------------------------------------------------------------------------
// The plugin
// ---------------------------------------------------------------------------

/// The AI: perception, the state machine, steering, aiming and the turrets.
///
/// Everything is in [`SimSet::Ai`], which is after `SimSet::Damage` — a brain
/// must not steer a hull that died earlier in the same tick — and the order
/// within the set is the order of one decision: count the tick, look, think,
/// wake the group, steer, aim, run the emplacements.
pub struct AiPlugin;

impl Plugin for AiPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<EnemySpawned>()
            .init_resource::<AiTick>()
            // The compiled defaults until `bft-game` has loaded `ai.ron`, so
            // a headless test and a dedicated server both have one.
            .init_resource::<SteeringTuning>()
            .add_systems(
                FixedUpdate,
                (
                    brain::tick_ai,
                    brain::stop_dead_brains,
                    brain::perceive,
                    brain::wake_on_damage,
                    brain::think,
                    brain::wake_buddies,
                    brain::steer,
                    brain::aim,
                    brain::drive_turrets,
                )
                    .chain()
                    .in_set(SimSet::Ai),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A turret's `FIRE_RATE` goes into the loadout as the interval the
    /// weapons' cooldown reads, so a SAM site's 30 is half a minute between
    /// missiles rather than 30 a second.
    #[test]
    fn a_turret_loadout_carries_the_defs_own_interval_and_magazine() {
        let weapons = weapon_table();
        let def = TurretDef {
            weapon: "wep_9.2".to_string(),
            ammo: Some(12),
            fire_interval: 30.0,
            ..table::tests_support::turret_def()
        };

        let loadout = turret_loadout(&def, &weapons);

        let ammo = loadout.selected_ammo().expect("the turret is armed");
        assert_eq!(ammo.ammo, 12);
        assert_eq!(ammo.interval, 30.0);
        assert_eq!(loadout.selected, 9, "wep_9.2 is in slot 9");
        assert!(loadout.strong, "and is not the weak variant");
    }

    /// The 14 entries with no `AMMO` are the helicopter guns and the buoys.
    #[test]
    fn a_turret_with_no_stated_magazine_never_runs_dry() {
        let def = TurretDef {
            weapon: "wep_1.0".to_string(),
            ammo: None,
            ..table::tests_support::turret_def()
        };

        let loadout = turret_loadout(&def, &weapon_table());

        assert_eq!(
            loadout.selected_ammo().map(|ammo| ammo.ammo),
            Some(u32::MAX)
        );
    }

    #[test]
    fn a_turret_naming_an_unknown_weapon_gets_an_empty_loadout_rather_than_a_panic() {
        let def = TurretDef {
            weapon: "wep_99.9".to_string(),
            ..table::tests_support::turret_def()
        };

        let loadout = turret_loadout(&def, &weapon_table());

        assert!(loadout.selected_ammo().is_none());
    }

    /// Two weapons, one in each of the slots the turret data actually uses.
    fn weapon_table() -> WeaponTable {
        use bevy::platform::collections::HashMap;

        let weapons = vec![weapon("wep_1.0", 1, 0), weapon("wep_9.2", 9, 2)];
        let by_id = weapons
            .iter()
            .enumerate()
            .map(|(index, def)| (def.id.clone(), index))
            .collect::<HashMap<_, _>>();

        WeaponTable {
            weapons,
            by_id,
            warnings: default(),
            tether_max_altitude: 30.0,
        }
    }

    fn weapon(id: &str, slot: u8, variant: u8) -> crate::weapons::WeaponDef {
        crate::weapons::WeaponDef {
            id: id.to_string(),
            slot,
            variant,
            hud_name: id.to_string(),
            desc_key: String::new(),
            kill_verb: default(),
            mover: crate::weapons::Mover::Straight,
            damage: 1.0,
            ammo_limit: 1000,
            range: 300.0,
            velocity: Some(100.0),
            gravity: 0.0,
            acceleration: None,
            turn_rate: None,
            pitch_rate: None,
            fire_rate: 0.1,
            splash_radius: 0.0,
            impact_type: 0,
            crater: None,
            flyout_health: None,
            reload_one: false,
            remote_detonate: false,
            tether_guided: false,
            lock_on: None,
            multi_target: false,
            catches_fire: false,
            quicksand: None,
            beam: None,
            mine: false,
            fire: default(),
            flyout: default(),
            impact: default(),
            impact_always: false,
            heat: None,
            designate: None,
            freeze: None,
            bounce_sound: None,
        }
    }
}
