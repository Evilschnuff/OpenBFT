//! Selection, firing, projectile movement and hit resolution.
//!
//! The table in the parent module says what a weapon *is*; this says what
//! happens when the trigger is pulled. One path serves the player and the AI:
//! both write [`FireRequest`], and nothing downstream knows which it was.
//!
//! # Order within a tick
//!
//! Everything here runs in [`SimSet::Weapons`], before physics:
//!
//! 1. [`select_weapons`] reads `PlayerInput.select_slot`/`toggle_strong`;
//! 2. [`request_fire`] turns a held `fire` into a [`FireRequest`];
//! 3. [`fire_weapons`] checks cooldown and ammo and spawns the projectile —
//!    or, for a `Beam`, resolves the hit outright;
//! 4. [`move_projectiles`] integrates the movers and casts each projectile's
//!    swept segment, so a fast round cannot tunnel through a wall between
//!    ticks;
//! 5. [`trip_mines`] sets off the mines something drove over, and the
//!    remote ones their layer has called for.
//!
//! Hit *consequences* — damage, splash, despawn — are in [`resolve_hits`],
//! which runs in [`SimSet::Damage`] after physics, so a hit produced by
//! avian's own contact events and one produced by the swept ray land in the
//! same place.
//!
//! # Why the sweep rather than colliders alone
//!
//! The pulse gun leaves the muzzle at 140 m/s, which is 2.3 m per tick
//! against walls well under a metre thick. A kinematic body with a small
//! sphere collider would pass straight through. So each projectile casts a
//! ray over the segment it is about to travel and takes the first hit on it;
//! the collider stays for the proximity cases (mines) and for anything the
//! ray starts already inside.

use avian3d::prelude::*;
use bevy::prelude::*;

use super::{AmmoState, Mover, WeaponLoadout, WeaponTable};
use crate::damage::{AimResult, DamageEvent, DamageKind, Health, NodeDamage, Team};
use crate::level::{GameLayer, HitBoxOf, NodeName};
use crate::physics::SurfaceInfo;
use crate::schedule::SimSet;
use crate::vehicle::{PlayerInput, Turret};

/// Ask a shooter to fire what it has selected.
///
/// Written by the player's input and by the AI alike.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct FireRequest {
    pub shooter: Entity,
    /// Unit vector the shot goes along, in world space.
    pub aim_dir: Vec3,
    /// Where a tether-guided missile is being steered, when one is in flight.
    pub tether_aim: Option<Vec3>,
    /// Where the shot leaves from, in world space, when the shooter knows
    /// better than its [`Muzzle`]: a turret's `firepoint`, which turns with
    /// its parts. `None` uses the shooter's transform and [`Muzzle`].
    pub from: Option<Vec3>,
    /// The secondary trigger: the pulse gun, whatever is selected
    /// ([`WeaponLoadout::trigger`]).
    pub secondary: bool,
}

/// Entities a shooter's own shots pass through: the colliders of its parts.
///
/// A turret's [`Health`] is on its node, but what a shot hits is the model
/// nodes under it — and its wall. Excluding only the shooter let a turret's
/// every shot hit its own barrel on the first step: the sound played, the
/// projectile was gone before it was drawn.
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct OwnParts(pub Vec<Entity>);

/// The entity whose [`Health`] a hit on `entity` bills: the entity itself or
/// its nearest ancestor with one.
///
/// A turret's colliders are its model nodes, several levels under the node
/// carrying its `Health`; billing only the entity hit made turrets
/// indestructible.
pub fn health_owner(
    entity: Entity,
    healths: impl Fn(Entity) -> bool,
    parents: &Query<&ChildOf>,
) -> Option<Entity> {
    let mut cursor = entity;
    for _ in 0..64 {
        if healths(cursor) {
            return Some(cursor);
        }
        cursor = parents.get(cursor).ok()?.parent();
    }
    None
}

/// A projectile came into the world.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct ProjectileSpawned {
    pub entity: Entity,
    pub weapon: usize,
}

/// A projectile hit something, or reached the end of its range.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct ProjectileHit {
    pub projectile: Entity,
    pub at: Vec3,
    pub normal: Vec3,
    /// The entity hit, where it was an entity rather than empty air.
    pub hit: Option<Entity>,
    /// That entity's node name, when it is a piece of level geometry —
    /// which is what the anim VM's `WeaponHit` defs are keyed by.
    pub hit_node: Option<String>,
    pub surface: super::Surface,
    pub weapon: usize,
}

/// A weapon went off: one per shot, beams included, whether or not a
/// projectile was spawned.
///
/// For what happens at the muzzle rather than downrange: the weapon's
/// `FIRE/ANIMATION` (`call_fire_animations`) and the gun's own
/// recoil and scroll on the player's model.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct WeaponFired {
    pub shooter: Entity,
    /// Index into [`WeaponTable`].
    pub weapon: usize,
    /// The model node the shot left from, when the shooter has one: the
    /// barrel of a [`TwinBarrels`] pair that fired, else its [`Firepoint`].
    pub firepoint: Option<Entity>,
    /// Which of a [`TwinBarrels`] pair fired, 0 or 1.
    pub barrel: Option<usize>,
    /// Where the shot left, in world space.
    pub origin: Vec3,
    /// Which way, unit length.
    pub direction: Vec3,
}

/// A trigger pull or a selection the loadout refused: what `weapons.zrd`'s
/// `NO_AMMO_WARNING`, `TRIGGER_INACTIVE` and `WEAPON_INACTIVE` sounds are
/// for.
///
/// Written on every refused tick while the trigger is held, as the requests
/// are; the listener decides how often to say so.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct TriggerRefused {
    pub shooter: Entity,
    pub reason: Refusal,
}

/// Why a [`TriggerRefused`] was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Refusal {
    /// The weapon is out of rounds: `NO_AMMO_WARNING`.
    NoAmmo,
    /// The trigger's weapon is not on the turret: `TRIGGER_INACTIVE`.
    TriggerInactive,
    /// The slot asked for holds nothing to select: `WEAPON_INACTIVE`.
    WeaponInactive,
}

/// The selected weapon changed.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct WeaponSelected {
    pub entity: Entity,
    pub slot: u8,
    pub strong: bool,
}

/// A projectile in flight.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct Projectile {
    /// Index into [`WeaponTable::weapons`].
    pub weapon: usize,
    /// Who fired it, so they are not hurt by their own muzzle.
    pub owner: Entity,
    /// The owner's team at the time of firing, so splash can tell friend
    /// from foe even after the owner is gone.
    pub team: Team,
    /// Seconds since it was fired.
    pub age: f32,
    /// What a homing missile is steering at.
    pub target: Option<Entity>,
    /// Whether a mine has finished arming.
    pub armed: bool,
    /// A `FLYOUT_HEALTH` projectile's own health, so it can be shot down.
    pub health: Option<f32>,
    /// Current velocity, m/s. Integrated here rather than by avian: a
    /// projectile is kinematic and its mover owns its path.
    pub velocity: Vec3,
    /// Where a mine came to rest, once it has.
    pub landed: Option<Landing>,
}

/// A mine lying on the ground.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Landing {
    /// The ground's normal under it, which it spins about.
    pub normal: Vec3,
    /// The surface it lies on, for the explosion's `IMPACT` block.
    pub surface: super::Surface,
}

/// A tether-guided missile its owner is steering.
///
/// Put on at launch and taken off when the owner lets go
/// ([`PlayerInput::release_guidance`]), dies or is gone; the missile then
/// flies on straight. The presentation reads it to put the view on the
/// missile, and the owner can guide only one at a time.
///
/// Not saved: a missile restored from a save flies straight.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guided;

/// A mine hit something it bounces off rather than explodes on.
///
/// What `IMPACT/<surface>/BOUNCE_SOUND` and `BOUNCE/SOUND` play on.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct MineBounced {
    pub projectile: Entity,
    pub at: Vec3,
    pub surface: super::Surface,
    pub weapon: usize,
}

/// Where a shooter's projectiles come out, in hull space.
///
/// On the player this is the model's `fpnt_c` node, composed down from the
/// hull with the gun centred (`bft_game::player_model`). Enemies and turrets still place
/// it from the vehicle's `aimy`, because their firepoint names vary by kind
/// and nothing in the data resolves them.
///
/// A shooter with a [`Firepoint`] has this rewritten every tick by
/// [`follow_firepoints`], so it turns with the turret and the barrel. Without
/// one it is a fixed hull offset: right when the gun points forward, and
/// drifting as the turret turns.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Muzzle(pub Vec3);

/// The model node a shooter's shots leave from: the player's `fpnt_c`.
///
/// [`follow_firepoints`] reads its pose each tick into [`Muzzle`], and the
/// pose of the vehicle's gun joint into [`AimPivot`](crate::vehicle::AimPivot).
/// Without it, a turret turned 90° fired from the hull's nose, three metres
/// to the side of its own line of fire.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Firepoint(pub Entity);

/// A pair of barrels that some weapons fire from in turn, instead of from the
/// [`Muzzle`]: the player's pulse-gun cannons either side of the turret, with
/// `fpnt_r` and `fpnt_l` at their tips.
///
/// Each shot leaves the next barrel and is aimed at the cursor's point, so
/// the two streams converge on it. The vehicle's interval is the pair's:
/// each barrel fires at half of it.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct TwinBarrels {
    /// The two firepoint nodes.
    pub nodes: [Entity; 2],
    /// Where they are in hull space, as posed now. Kept by
    /// [`follow_firepoints`].
    pub at: [Vec3; 2],
    /// Which barrel fires next.
    pub next: usize,
    /// The `NAME`s of the weapons that fire from them (`RFPG`, `ERFPG`).
    pub weapons: Vec<String>,
}

impl TwinBarrels {
    /// Barrels at these nodes for these weapons, starting on the first.
    pub fn new(nodes: [Entity; 2], weapons: Vec<String>) -> TwinBarrels {
        TwinBarrels {
            nodes,
            at: [Vec3::ZERO; 2],
            next: 0,
            weapons,
        }
    }
}

/// How long after firing the owner cannot be hit by its own projectile.
///
/// // approximation: no reader field states it. Long enough that a round
/// leaving the muzzle is clear of the hull at the slowest muzzle speed in
/// the data (60 m/s over a 7.4 m vehicle is about 0.06 s), with margin.
pub const OWNER_IMMUNITY: f32 = 0.15;

/// The radius of a projectile's collider, in metres.
///
/// // approximation: the models are 0.2–1 m across and no reader field states
/// a collision radius. Small enough not to clip a doorway it should fly
/// through, large enough that the proximity of a mine is the thing that
/// triggers it rather than the collider.
pub const PROJECTILE_RADIUS: f32 = 0.15;

/// A projectile's body: kinematic, a small sensor sphere, on the projectile
/// layer. Shared with a save's restore.
pub fn projectile_body() -> impl Bundle {
    (
        RigidBody::Kinematic,
        Collider::sphere(PROJECTILE_RADIUS),
        Sensor,
        crate::physics::layers::projectile(),
    )
}

/// How long a mine takes to arm after being dropped, in seconds.
///
/// // approximation: no reader field corresponds. Long enough that a mine
/// dropped while reversing does not kill the vehicle that laid it.
pub const MINE_ARM_TIME: f32 = 1.0;

/// How much of a mine's speed into a surface comes back out of it.
///
/// // approximation: `BOUNCE` has a sound and no numbers. The manual says the
/// launcher can "bounce them from around corners", so a wall has to send one
/// back; a mine lobbed onto flat ground should hop once or twice and stop.
pub const MINE_RESTITUTION: f32 = 0.35;

/// How much of a mine's speed along a surface survives a bounce.
///
/// // approximation, as [`MINE_RESTITUTION`].
pub const MINE_FRICTION: f32 = 0.6;

/// Below this speed after a bounce, a mine on ground stops and lies there.
///
/// // approximation: slow enough that a lobbed mine hops once or twice.
pub const MINE_REST_SPEED: f32 = 4.0;

/// A surface whose normal's `y` is at least this is ground a mine can lie
/// on; anything steeper it only bounces off.
pub const MINE_GROUND_NORMAL_Y: f32 = 0.7;

/// How fast a lying mine turns, radians per second.
///
/// // approximation: the flyout models carry no anim of their own. Slow
/// enough to read as "armed", not as a propeller.
pub const MINE_SPIN_RATE: f32 = 1.5;

/// How close, across the ground, something hostile must come to set a
/// proximity mine off.
///
/// // approximation: `IMPACT_PROXIMITY` (48 m on the P-HEM) is the blast, not
/// the trigger — a mine that went off at 48 m would never be run over. Four
/// metres is inside half a hull length of the smallest vehicle's centre.
pub const MINE_TRIGGER_RADIUS: f32 = 4.0;

/// How far above or below a mine something may be and still trip it.
///
/// // approximation: a vehicle's origin sits a metre or two above the ground
/// it drives on; this is generous so a hover still trips one.
pub const MINE_TRIGGER_HEIGHT: f32 = 4.0;

/// How much of a splash weapon's damage is left at the edge of its radius.
///
/// // approximation: no reader field states a falloff curve. A linear fade to
/// this fraction, rather than to zero, keeps a near-miss meaningful — which
/// is how the original's mortars feel.
pub const SPLASH_EDGE_FRACTION: f32 = 0.25;

/// Half-angle of the cone a homing weapon acquires its target in, radians.
///
/// // approximation: `LOCK_ON` is a float whose unit is unsettled
/// (READER-SCHEMA §2.3 marks it I), so it is not read as this angle. 15° is
/// a lock that needs aiming but forgives a moving target.
pub const LOCK_CONE: f32 = 0.26;

/// Everything a shot could acquire or hurt: it has health and a side.
///
/// A named alias because three signatures repeat it, and a `QueryData` spelt
/// out four times drifts apart the moment one of them gains a filter.
pub type TargetQuery<'w, 's> =
    Query<'w, 's, (Entity, &'static GlobalTransform), (With<Health>, With<Team>)>;

/// Damage a splash of `radius` does at `distance` from its centre.
///
/// Linear from full at the centre to [`SPLASH_EDGE_FRACTION`] at the rim, and
/// zero beyond it. A radius of zero splashes nothing, which is the 8 weapons
/// whose `IMPACT_PROXIMITY` is 0.
pub fn splash_damage(full: f32, radius: f32, distance: f32) -> f32 {
    if radius <= 0.0 || distance > radius {
        return 0.0;
    }

    let t = (distance / radius).clamp(0.0, 1.0);

    full * (1.0 - t * (1.0 - SPLASH_EDGE_FRACTION))
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

/// Apply `select_slot` and `toggle_strong` to every loadout.
///
/// An empty slot is not selectable and a variant that is not owned is not
/// reachable, so neither input can leave the selection pointing at nothing.
pub fn select_weapons(
    mut selected: MessageWriter<WeaponSelected>,
    mut refused: MessageWriter<TriggerRefused>,
    mut shooters: Query<(Entity, &PlayerInput, &mut WeaponLoadout)>,
) {
    for (entity, input, mut loadout) in &mut shooters {
        let mut changed = false;

        if let Some(slot) = input.select_slot {
            if !loadout.is_selectable(slot) {
                refused.write(TriggerRefused {
                    shooter: entity,
                    reason: Refusal::WeaponInactive,
                });
            }
            changed |= loadout.select(slot);
        }
        if input.toggle_strong {
            changed |= loadout.toggle_strong();
        }

        if changed {
            selected.write(WeaponSelected {
                entity,
                slot: loadout.selected,
                strong: loadout.strong,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Firing
// ---------------------------------------------------------------------------

/// Turn a held trigger into a [`FireRequest`] along the turret's aim.
///
/// The rate limit is *not* here: a request is made on every tick the trigger
/// is held and [`fire_weapons`] drops the ones the cooldown forbids. That
/// keeps the AI's path identical — the AI writes requests too, and a
/// request that was silently rate-limited upstream would be invisible to it.
pub fn request_fire(
    mut requests: MessageWriter<FireRequest>,
    shooters: Query<(Entity, &PlayerInput, &Transform, &Turret), With<WeaponLoadout>>,
) {
    for (entity, input, transform, turret) in &shooters {
        for (held, secondary) in [(input.fire, false), (input.fire_alt, true)] {
            if held {
                requests.write(FireRequest {
                    shooter: entity,
                    aim_dir: aim_direction(transform.rotation, *turret),
                    tether_aim: None,
                    from: None,
                    secondary,
                });
            }
        }
    }
}

/// The world-space direction a hull rotation and turret angles point along.
///
/// The turret's yaw is about the hull's up axis and its pitch about the
/// hull's right axis, so the two compose onto the hull's forward — the same
/// order the chase camera uses.
pub fn aim_direction(hull: Quat, turret: Turret) -> Vec3 {
    let rotation = hull * Quat::from_rotation_y(turret.yaw) * Quat::from_rotation_x(turret.pitch);

    (rotation * Vec3::NEG_Z).normalize_or(Vec3::NEG_Z)
}

/// Move each [`Muzzle`] onto its [`Firepoint`], each [`TwinBarrels`] onto
/// its nodes, and each [`AimPivot`](crate::vehicle::AimPivot) onto its gun
/// joint, as posed now.
///
/// Composed from the local transforms rather than read off
/// `GlobalTransform`, which is only propagated once a frame: `aim_turret` has
/// just turned the joints this tick, and the shot has to leave from where
/// the barrel is, not where it was drawn.
#[allow(clippy::type_complexity)]
pub fn follow_firepoints(
    mut shooters: Query<(
        Entity,
        Option<&Firepoint>,
        Option<&crate::vehicle::AimJoints>,
        Option<&mut Muzzle>,
        Option<&mut crate::vehicle::AimPivot>,
        Option<&mut TwinBarrels>,
    )>,
    locals: Query<(&Transform, Option<&ChildOf>)>,
) {
    for (hull, firepoint, joints, muzzle, pivot, twins) in &mut shooters {
        if let Some(mut twins) = twins {
            let at = twins.nodes.map(|node| {
                crate::vehicle::in_ancestor_space(node, hull, &locals).map(|at| at.translation)
            });
            if let [Some(first), Some(second)] = at
                && twins.at != [first, second]
            {
                twins.at = [first, second];
            }
        }

        if let (Some(firepoint), Some(mut muzzle)) = (firepoint, muzzle)
            && let Some(at) = crate::vehicle::in_ancestor_space(firepoint.0, hull, &locals)
            && muzzle.0 != at.translation
        {
            muzzle.0 = at.translation;
        }

        let gun = joints.map(|joints| joints.pitch.unwrap_or(joints.yaw));
        if let (Some(gun), Some(mut pivot)) = (gun, pivot)
            && let Some(at) = crate::vehicle::in_ancestor_space(gun, hull, &locals)
            && pivot.0 != at.translation
        {
            pivot.0 = at.translation;
        }
    }
}

/// Count down every loadout's cooldown, keeping the overshoot.
///
/// The remainder is **not** clamped to zero: an interval is rarely a whole
/// number of ticks — 12 shots a second is 5.0 ticks, a 1.2 s mortar is 72 —
/// and discarding the fraction every cycle costs a tick per shot. At 12
/// shots a second that would be a real rate of 10. [`fire_weapons`] adds the interval to whatever is left
/// rather than assigning it, so the error never accumulates.
pub fn tick_cooldowns(time: Res<Time<Fixed>>, mut loadouts: Query<&mut WeaponLoadout>) {
    let delta = time.delta_secs();

    for mut loadout in &mut loadouts {
        if loadout.cooldown > -delta {
            loadout.cooldown -= delta;
        }
        if loadout.secondary_cooldown > -delta {
            loadout.secondary_cooldown -= delta;
        }
    }
}

/// Consume [`FireRequest`]s: check cooldown and ammo, spend a round, spawn.
///
/// A `Beam` weapon has no projectile: the hit is resolved on this tick by a
/// ray and written straight to [`ProjectileHit`] with no entity in flight.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn fire_weapons(
    mut commands: Commands,
    mut requests: MessageReader<FireRequest>,
    mut spawned: MessageWriter<ProjectileSpawned>,
    mut hits: MessageWriter<ProjectileHit>,
    mut fired: MessageWriter<WeaponFired>,
    mut refused: MessageWriter<TriggerRefused>,
    table: Option<Res<WeaponTable>>,
    spatial: SpatialQuery,
    surfaces: Query<&SurfaceInfo>,
    names: Query<&NodeName>,
    hit_boxes: Query<&HitBoxOf>,
    mut shooters: Query<(
        &mut WeaponLoadout,
        &GlobalTransform,
        Option<&Muzzle>,
        Option<&Team>,
        Option<&OwnParts>,
        Option<&mut TwinBarrels>,
        Option<&PlayerInput>,
        Option<&crate::vehicle::AimPivot>,
        Option<&Firepoint>,
    )>,
    targets: TargetQuery,
    guided: Query<&Projectile, With<Guided>>,
) {
    let Some(table) = table else {
        return;
    };

    for request in requests.read() {
        let Ok((mut loadout, transform, muzzle, team, own, twins, input, pivot, firepoint)) =
            shooters.get_mut(request.shooter)
        else {
            continue;
        };
        if *loadout.trigger_cooldown_mut(request.secondary) > 0.0 {
            continue;
        }

        let Some(&mut AmmoState {
            weapon,
            ammo,
            interval,
            mounted,
        }) = loadout.trigger_ammo_mut(request.secondary)
        else {
            continue;
        };
        // Rounds for a gun the turret does not carry cannot be fired.
        if ammo == 0 || !mounted {
            refused.write(TriggerRefused {
                shooter: request.shooter,
                reason: if mounted {
                    Refusal::NoAmmo
                } else {
                    Refusal::TriggerInactive
                },
            });
            continue;
        }
        let Some(def) = table.by_index(weapon) else {
            continue;
        };
        // One guided missile at a time: the view is on it, and a second
        // would have nothing to steer it.
        if def.mover == Mover::Tethered
            && guided
                .iter()
                .any(|missile| missile.owner == request.shooter)
        {
            continue;
        }

        let hull = |local: Vec3| transform.translation() + transform.rotation() * local;
        let mut origin = request
            .from
            .unwrap_or_else(|| hull(muzzle.map(|m| m.0).unwrap_or(Vec3::ZERO)));
        let mut aim_dir = request.aim_dir;
        let mut fired_from = (request.from.is_none())
            .then(|| firepoint.map(|firepoint| firepoint.0))
            .flatten();
        let mut fired_barrel = None;

        // A twin-barrelled weapon leaves the next barrel, aimed at the
        // cursor's point. Not along the turret: with an arcing weapon
        // selected the turret is elevated for *its* shell, and a straight
        // pulse would sail over. Without a point, it converges along the
        // turret's aim at the weapon's range.
        if request.from.is_none()
            && let Some(mut twins) = twins
            && twins.weapons.contains(&def.hud_name)
        {
            let barrel = twins.next % 2;
            twins.next = (barrel + 1) % 2;
            fired_from = Some(twins.nodes[barrel]);
            fired_barrel = Some(barrel);

            let pivot = hull(pivot.map_or(Vec3::ZERO, |pivot| pivot.0));
            let converge = input
                .and_then(|input| input.aim_point)
                .filter(|_| def.gravity <= 0.0)
                .unwrap_or(pivot + request.aim_dir * def.range);

            origin = hull(twins.at[barrel]);
            aim_dir = (converge - origin).normalize_or(request.aim_dir);
        }
        let mut exclude = vec![request.shooter];
        exclude.extend(own.iter().flat_map(|own| own.0.iter().copied()));
        let team = team.copied().unwrap_or(Team::PLAYER);

        if let Some(state) = loadout.trigger_ammo_mut(request.secondary) {
            state.spend();
        }
        // The vehicle's own `interval` column, not the weapon's `FIRE_RATE`:
        // the player's row is authoritative for the player, and an AI turret
        // carries its own. `FIRE_RATE` is only the fallback.
        let interval = if interval > 0.0 {
            interval
        } else {
            def.shot_interval()
        };
        // Added to what is left over, not assigned: see `tick_cooldowns` for
        // why throwing the remainder away costs a tick a shot.
        let cooldown = loadout.trigger_cooldown_mut(request.secondary);
        *cooldown = (*cooldown + interval).max(0.0);

        fired.write(WeaponFired {
            shooter: request.shooter,
            weapon,
            firepoint: fired_from,
            barrel: fired_barrel,
            origin,
            direction: aim_dir,
        });

        if def.mover == Mover::Beam {
            // Hitscan: no entity, no flight, the hit is now.
            let hit = cast(
                &spatial,
                &surfaces,
                (&names, &hit_boxes),
                origin,
                aim_dir,
                def.range,
                &exclude,
            );
            hits.write(ProjectileHit {
                // A beam has no projectile entity; the shooter stands in so
                // `resolve_hits` has something to name, and it despawns
                // nothing because the entity has no `Projectile`.
                projectile: request.shooter,
                at: hit.point,
                normal: hit.normal,
                hit: hit.entity,
                hit_node: hit.node,
                surface: hit.surface,
                weapon,
            });
            continue;
        }

        let velocity = aim_dir * def.velocity.unwrap_or(0.0);
        let target = (def.mover == Mover::Homing)
            .then(|| acquire_target(&targets, origin, aim_dir, def.range, team))
            .flatten();

        let projectile = commands
            .spawn((
                Name::new(format!("projectile {}", def.id)),
                Projectile {
                    weapon,
                    owner: request.shooter,
                    team,
                    age: 0.0,
                    target,
                    armed: false,
                    health: def.flyout_health.map(|health| health as f32),
                    velocity,
                    landed: None,
                },
                Transform::from_translation(origin).looking_to(aim_dir, Vec3::Y),
                projectile_body(),
            ))
            .id();
        // Only a shooter with a controller can steer; an AI turret's would
        // fly straight.
        if def.mover == Mover::Tethered && input.is_some() {
            commands.entity(projectile).insert(Guided);
        }

        spawned.write(ProjectileSpawned {
            entity: projectile,
            weapon,
        });
    }
}

/// The nearest hostile target inside the aim cone, for a homing weapon.
///
/// Acquired once, at fire time: the original's missiles keep steering at what
/// they locked onto rather than re-picking mid-flight.
fn acquire_target(
    targets: &TargetQuery,
    origin: Vec3,
    aim: Vec3,
    range: f32,
    team: Team,
) -> Option<Entity> {
    let _ = team;

    targets
        .iter()
        .filter_map(|(entity, transform)| {
            let offset = transform.translation() - origin;
            let distance = offset.length();
            if distance > range || distance <= f32::EPSILON {
                return None;
            }
            // Inside the cone the shot was aimed down.
            let angle = aim.angle_between(offset / distance);
            (angle <= LOCK_CONE).then_some((entity, distance))
        })
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(entity, _)| entity)
}

/// What a cast along a segment found.
struct CastHit {
    point: Vec3,
    normal: Vec3,
    entity: Option<Entity>,
    node: Option<String>,
    surface: super::Surface,
}

/// Cast a ray at the level and at vehicles, skipping the shooter.
fn cast(
    spatial: &SpatialQuery,
    surfaces: &Query<&SurfaceInfo>,
    // The node a hit names, and the node a hit box stands in for.
    (names, hit_boxes): (&Query<&NodeName>, &Query<&HitBoxOf>),
    origin: Vec3,
    direction: Vec3,
    max: f32,
    exclude: &[Entity],
) -> CastHit {
    let miss = CastHit {
        point: origin + direction * max,
        normal: -direction,
        entity: None,
        node: None,
        surface: super::Surface::Default,
    };

    let Ok(dir) = Dir3::new(direction) else {
        return miss;
    };
    let filter =
        SpatialQueryFilter::from_mask([GameLayer::Level, GameLayer::Vehicle, GameLayer::HitBox])
            .with_excluded_entities(exclude.iter().copied());

    let Some(mut hit) = spatial.cast_ray(origin, dir, max, true, &filter) else {
        return miss;
    };
    // A hit box stands in for its node, which is what takes the hit.
    if let Ok(HitBoxOf(node)) = hit_boxes.get(hit.entity) {
        hit.entity = *node;
    }

    CastHit {
        point: origin + dir * hit.distance,
        normal: hit.normal,
        entity: Some(hit.entity),
        node: names.get(hit.entity).ok().map(|name| name.0.clone()),
        // avian's `RayHitData` names the entity but not the triangle, so the
        // exact polygon's soil is out of reach here; the collider's dominant
        // soil is what `SurfaceInfo` offers cheaply and is right for the
        // water and quicksand volumes that matter.
        surface: surfaces
            .get(hit.entity)
            .map(|info| super::Surface::from_soil(info.dominant))
            .unwrap_or_default(),
    }
}

// ---------------------------------------------------------------------------
// Movers
// ---------------------------------------------------------------------------

/// Cut the tether of every guided missile whose owner let go, died or is
/// gone.
pub fn release_guidance(
    mut commands: Commands,
    missiles: Query<(Entity, &Projectile), With<Guided>>,
    owners: Query<(&PlayerInput, Option<&Health>)>,
) {
    for (entity, missile) in &missiles {
        let holding = owners.get(missile.owner).is_ok_and(|(input, health)| {
            !input.release_guidance && health.is_none_or(|health| !health.is_dead())
        });
        if !holding {
            commands.entity(entity).try_remove::<Guided>();
        }
    }
}

/// Advance every projectile and report what it ran into.
///
/// Each mover sets the velocity for this tick; the integration and the swept
/// cast that follows are shared, so no mover can forget to check for a wall.
#[allow(clippy::too_many_arguments)]
pub fn move_projectiles(
    time: Res<Time<Fixed>>,
    mut commands: Commands,
    mut hits: MessageWriter<ProjectileHit>,
    mut bounces: MessageWriter<MineBounced>,
    table: Option<Res<WeaponTable>>,
    spatial: SpatialQuery,
    surfaces: Query<&SurfaceInfo>,
    names: Query<&NodeName>,
    hit_boxes: Query<&HitBoxOf>,
    positions: Query<&GlobalTransform>,
    mut projectiles: Query<(Entity, &mut Projectile, &mut Transform, Has<Guided>)>,
    inputs: Query<&PlayerInput>,
    own: Query<&OwnParts>,
    healths: Query<(), With<Health>>,
    parents: Query<&ChildOf>,
) {
    let Some(table) = table else {
        return;
    };
    let delta = time.delta_secs();

    for (entity, mut projectile, mut transform, guided) in &mut projectiles {
        let Some(def) = table.by_index(projectile.weapon) else {
            commands.entity(entity).despawn();
            continue;
        };

        projectile.age += delta;

        match def.mover {
            Mover::Straight => {}
            Mover::Ballistic => {
                // The weapon's own `GRAVITY` (9.8), not `player.zrd`'s 40:
                // the two are different quantities and a mortar that fell at
                // 40 m/s² would land at a quarter of its stated range.
                projectile.velocity.y -= def.gravity * delta;
            }
            Mover::Homing => {
                if let Some(steer) = homing_velocity(
                    &positions,
                    &projectile,
                    transform.translation,
                    def.turn_rate.unwrap_or(0.0),
                    def.acceleration.unwrap_or(0.0),
                    delta,
                ) {
                    projectile.velocity = steer;
                }
            }
            Mover::Tethered => {
                let guide = guided
                    .then(|| inputs.get(projectile.owner).ok())
                    .flatten()
                    .map_or(Vec2::ZERO, |input| input.guide);
                // How far below the tether's ceiling it is; none when the
                // ground is further down than the ceiling is high.
                let headroom = spatial
                    .cast_ray(
                        transform.translation,
                        Dir3::NEG_Y,
                        table.tether_max_altitude,
                        true,
                        &SpatialQueryFilter::from_mask(GameLayer::Level),
                    )
                    .map_or(0.0, |ground| table.tether_max_altitude - ground.distance);
                projectile.velocity = tethered_velocity(
                    projectile.velocity,
                    guide,
                    Steering {
                        turn_rate: def.turn_rate.unwrap_or(0.0),
                        pitch_rate: def.pitch_rate.or(def.turn_rate).unwrap_or(0.0),
                        acceleration: def.acceleration.unwrap_or(0.0),
                        top_speed: def.velocity.unwrap_or(f32::INFINITY),
                    },
                    headroom,
                    delta,
                );
            }
            Mover::Mine => {
                if !projectile.armed && projectile.age >= MINE_ARM_TIME {
                    projectile.armed = true;
                }
                // Lying on the ground: turning slowly, and waiting for
                // `trip_mines`.
                if let Some(landing) = projectile.landed {
                    transform.rotation =
                        Quat::from_axis_angle(landing.normal, MINE_SPIN_RATE * delta)
                            * transform.rotation;
                    continue;
                }
                projectile.velocity.y -= def.gravity * delta;
            }
            // A beam never becomes a projectile: `fire_weapons` resolves it.
            Mover::Beam => {}
        }

        let step = projectile.velocity * delta;
        let travelled = step.length();

        if travelled > f32::EPSILON {
            let owner_immune = projectile.age < OWNER_IMMUNITY;
            let exclude: Vec<Entity> = if owner_immune {
                std::iter::once(projectile.owner)
                    .chain(
                        own.get(projectile.owner)
                            .into_iter()
                            .flat_map(|own| own.0.iter().copied()),
                    )
                    .collect()
            } else {
                vec![entity]
            };
            let hit = cast(
                &spatial,
                &surfaces,
                (&names, &hit_boxes),
                transform.translation,
                step / travelled,
                travelled,
                &exclude,
            );

            // A mine goes off on a vehicle and bounces off everything else,
            // coming to rest once it is on ground and slow.
            let on_something_alive = hit
                .entity
                .and_then(|e| health_owner(e, |e| healths.contains(e), &parents))
                .is_some();
            if def.mover == Mover::Mine && hit.entity.is_some() && !on_something_alive {
                let normal = hit.normal.normalize_or(Vec3::Y);
                let into = normal * projectile.velocity.dot(normal);
                let along = projectile.velocity - into;
                let bounced = along * MINE_FRICTION - into * MINE_RESTITUTION;

                transform.translation = hit.point + normal * PROJECTILE_RADIUS;
                bounces.write(MineBounced {
                    projectile: entity,
                    at: hit.point,
                    surface: hit.surface,
                    weapon: projectile.weapon,
                });

                if normal.y >= MINE_GROUND_NORMAL_Y && bounced.length() < MINE_REST_SPEED {
                    projectile.velocity = Vec3::ZERO;
                    projectile.landed = Some(Landing {
                        normal,
                        surface: hit.surface,
                    });
                    // Upright on the ground, whichever way it flew in.
                    transform.rotation = Quat::from_rotation_arc(Vec3::Y, normal);
                } else {
                    projectile.velocity = bounced;
                }
                continue;
            }

            if hit.entity.is_some() {
                transform.translation = hit.point;
                hits.write(ProjectileHit {
                    projectile: entity,
                    at: hit.point,
                    normal: hit.normal,
                    hit: hit.entity,
                    hit_node: hit.node,
                    surface: hit.surface,
                    weapon: projectile.weapon,
                });
                continue;
            }

            transform.translation += step;
            if projectile.velocity.length_squared() > f32::EPSILON {
                transform.rotation = Transform::default()
                    .looking_to(projectile.velocity, Vec3::Y)
                    .rotation;
            }
        }

        // Out of range: it stops existing where it is. A mine has no flight
        // time and so never expires this way.
        if projectile.age >= def.flight_time() {
            hits.write(ProjectileHit {
                projectile: entity,
                at: transform.translation,
                normal: Vec3::Y,
                hit: None,
                hit_node: None,
                surface: super::Surface::Default,
                weapon: projectile.weapon,
            });
        }
    }
}

/// Set off the mines that should go off this tick.
///
/// A proximity mine goes off once it is armed and lying still, when
/// something hostile to its layer comes within [`MINE_TRIGGER_RADIUS`]
/// across the ground. A remote mine ignores traffic — "they will not detonate
/// until you trigger them", as the manual has it — and goes off, landed or
/// not, when its layer's [`PlayerInput::detonate`] is set.
///
/// The explosion is an ordinary [`ProjectileHit`] with nothing hit, so
/// splash, sound, effects and craters all follow the one path.
pub fn trip_mines(
    mut hits: MessageWriter<ProjectileHit>,
    table: Option<Res<WeaponTable>>,
    mines: Query<(Entity, &Projectile, &Transform)>,
    targets: Query<(&GlobalTransform, &Team, &Health), Without<Projectile>>,
    inputs: Query<&PlayerInput>,
) {
    let Some(table) = table else {
        return;
    };

    for (entity, mine, transform) in &mines {
        let Some(def) = table.by_index(mine.weapon) else {
            continue;
        };
        if def.mover != Mover::Mine {
            continue;
        }

        let at = transform.translation;
        let tripped = if def.remote_detonate {
            inputs.get(mine.owner).is_ok_and(|input| input.detonate)
        } else {
            mine.armed
                && mine.landed.is_some()
                && targets.iter().any(|(target, team, health)| {
                    let offset = target.translation() - at;
                    mine.team.is_hostile_to(*team)
                        && !health.is_dead()
                        && offset.xz().length() <= MINE_TRIGGER_RADIUS
                        && offset.y.abs() <= MINE_TRIGGER_HEIGHT
                })
        };
        if !tripped {
            continue;
        }

        let landing = mine.landed;
        hits.write(ProjectileHit {
            projectile: entity,
            at,
            normal: landing.map_or(Vec3::Y, |landing| landing.normal),
            hit: None,
            hit_node: None,
            surface: landing.map_or(super::Surface::Default, |landing| landing.surface),
            weapon: mine.weapon,
        });
    }
}

/// The steepest a tether-guided missile may climb or dive, in radians.
///
/// // approximation: no reader field corresponds. Short of vertical, where
/// yaw stops meaning anything and the view would flip.
pub const TETHER_MAX_PITCH: f32 = 1.3;

/// How a tether-guided missile turns: `weapons.zrd`'s rates for it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Steering {
    /// `TURN_RATE`, rad/s, at full deflection sideways.
    pub turn_rate: f32,
    /// `PITCH_RATE`, rad/s, at full deflection up or down.
    pub pitch_rate: f32,
    /// `ACCELERATION`, m/s²: how fast it comes up to `top_speed`.
    pub acceleration: f32,
    /// `VELOCITY`, m/s: the fastest it flies.
    ///
    /// A cap, unlike `Homing`, which accelerates for its whole flight. A
    /// guided missile that did went from 80 m/s at launch to 180 after five
    /// seconds and was barely steerable, where footage of the original shows
    /// it slow and even. Capped, it also covers exactly its `RANGE` in the
    /// flight time [`WeaponDef::flight_time`](super::WeaponDef::flight_time)
    /// gives it.
    pub top_speed: f32,
}

/// The velocity a tether-guided missile has after this tick's steering.
///
/// `guide` is [`PlayerInput::guide`]: the fraction of each rate to turn at,
/// so steering is proportional to how far off centre the cursor is. Yaw is
/// about world up, not the missile's own, so a missile in a climb turns flat
/// rather than rolling into it.
///
/// `headroom` is how far the missile is below `WeaponSetMaxTetherAltitude`,
/// in metres. It may climb only as steeply as it can still level off from,
/// at its pitch rate, before reaching that ceiling; steeper than that, up is
/// ignored and it is pushed level at the pitch rate rather than snapped
/// flat.
///
/// // approximation: the altitude is read as height above the ground under
/// the missile. The interp command names a number, 30 m, and nothing says
/// what it is measured from.
pub fn tethered_velocity(
    velocity: Vec3,
    guide: Vec2,
    steering: Steering,
    headroom: f32,
    delta: f32,
) -> Vec3 {
    let speed = velocity.length();
    let Some(direction) = velocity.try_normalize() else {
        return velocity;
    };

    // Forward is −z at yaw zero, as everywhere in the sim.
    let mut yaw = (-direction.x).atan2(-direction.z);
    let mut pitch = direction.y.clamp(-1.0, 1.0).asin();

    // Levelling off from a climb of θ at rate ω gains v(1 − cos θ)/ω of
    // height, so the steepest climb the headroom allows is where that equals
    // it.
    let ceiling_pitch = if speed > f32::EPSILON {
        (1.0 - headroom.max(0.0) * steering.pitch_rate / speed)
            .clamp(-1.0, 1.0)
            .acos()
    } else {
        0.0
    };
    let mut guide = guide.clamp(Vec2::NEG_ONE, Vec2::ONE);
    if pitch >= ceiling_pitch {
        // Up is ignored rather than cancelled by the push down below: the
        // two at the same rate held a climbing missile at its climb.
        guide.y = guide.y.min(0.0);
        pitch = (pitch - steering.pitch_rate * delta).max(ceiling_pitch.min(pitch));
    }
    // Right is a negative yaw about up.
    yaw -= guide.x * steering.turn_rate * delta;
    pitch += guide.y * steering.pitch_rate * delta;
    let pitch = pitch.clamp(-TETHER_MAX_PITCH, TETHER_MAX_PITCH);

    let direction = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0) * Vec3::NEG_Z;
    // Never slowed by the cap: a launch above it keeps its speed.
    let speed = (speed + steering.acceleration * delta).min(steering.top_speed.max(speed));
    direction * speed
}

/// The velocity a homing projectile should have after steering this tick.
///
/// `None` when it has no target, or the target is gone — in which case it
/// keeps flying straight, as the original's missiles do when their lock is
/// broken.
fn homing_velocity(
    positions: &Query<&GlobalTransform>,
    projectile: &Projectile,
    at: Vec3,
    turn_rate: f32,
    acceleration: f32,
    delta: f32,
) -> Option<Vec3> {
    let target = projectile.target?;
    let target_at = positions.get(target).ok()?.translation();

    let speed = projectile.velocity.length();
    if speed <= f32::EPSILON {
        return None;
    }

    let current = projectile.velocity / speed;
    let wanted = (target_at - at).normalize_or_zero();
    if wanted == Vec3::ZERO {
        return None;
    }

    // Rotate toward the target by at most `TURN_RATE` radians this tick.
    let angle = current.angle_between(wanted);
    let step = (turn_rate * delta).min(angle);
    let direction = if angle <= f32::EPSILON {
        wanted
    } else {
        let axis = current.cross(wanted).normalize_or_zero();
        if axis == Vec3::ZERO {
            wanted
        } else {
            Quat::from_axis_angle(axis, step) * current
        }
    };

    Some(direction * (speed + acceleration * delta))
}

// ---------------------------------------------------------------------------
// Hits
// ---------------------------------------------------------------------------

/// Turn hits into damage, splash and despawns.
///
/// Runs in [`SimSet::Damage`], so the hits a tick's movers produced and any
/// avian sensor events land together.
#[allow(clippy::too_many_arguments)]
pub fn resolve_hits(
    mut commands: Commands,
    mut hits: MessageReader<ProjectileHit>,
    mut damage: MessageWriter<DamageEvent>,
    mut node_damage: MessageWriter<NodeDamage>,
    table: Option<Res<WeaponTable>>,
    projectiles: Query<&Projectile>,
    healths: Query<(Entity, &GlobalTransform), With<Health>>,
    names: Query<&NodeName>,
    parents: Query<&ChildOf>,
) {
    let Some(table) = table else {
        return;
    };

    for hit in hits.read() {
        let Some(def) = table.by_index(hit.weapon) else {
            continue;
        };
        let owner = projectiles.get(hit.projectile).ok().map(|p| p.owner);

        // A direct hit on something with health.
        let billed = hit
            .hit
            .and_then(|entity| health_owner(entity, |e| healths.contains(e), &parents));
        if let Some(entity) = hit.hit {
            if let Some(target) = billed {
                damage.write(DamageEvent {
                    target,
                    amount: def.damage,
                    source: owner,
                    at: hit.at,
                    kind: DamageKind::Projectile(hit.weapon),
                });
            } else if let Some(name) = hit
                .hit_node
                .clone()
                .or_else(|| names.get(entity).ok().map(|name| name.0.clone()))
            {
                // A level node: the anim VM accumulates this against the
                // `WeaponHit` def's `health` threshold and does the
                // destroying. This module does not know which nodes are
                // destructible and does not need to.
                node_damage.write(NodeDamage {
                    node: entity,
                    name,
                    amount: def.damage,
                    kind: DamageKind::Projectile(hit.weapon),
                    at: hit.at,
                });
            }
        }

        // Splash, on everything in radius including the shooter: a badly
        // aimed nuke kills you too, which is how the original plays.
        if def.splash_radius > 0.0 {
            for (entity, transform) in &healths {
                // The direct hit was already billed at full damage.
                if Some(entity) == billed {
                    continue;
                }
                let amount = splash_damage(
                    def.damage,
                    def.splash_radius,
                    transform.translation().distance(hit.at),
                );
                if amount > 0.0 {
                    damage.write(DamageEvent {
                        target: entity,
                        amount,
                        source: owner,
                        at: hit.at,
                        kind: DamageKind::Splash(hit.weapon),
                    });
                }
            }
        }

        // A beam's "projectile" is its shooter, which has no `Projectile`
        // component and must survive its own shot.
        if projectiles.get(hit.projectile).is_ok() {
            commands.entity(hit.projectile).despawn();
        }
    }
}

/// Cast the aim ray each tick and record what it found.
///
/// The HUD's reticle turns green on a target this shooter can hurt,
/// which is [`Health`] plus a different [`Team`].
// Eight parameters, all ECS injections: the aim cast needs the hit-box
// lookup beside the names and surfaces it already read.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
pub fn update_aim(
    spatial: SpatialQuery,
    surfaces: Query<&SurfaceInfo>,
    names: Query<&NodeName>,
    hit_boxes: Query<&HitBoxOf>,
    table: Option<Res<WeaponTable>>,
    hostiles: Query<&Team, With<Health>>,
    parents: Query<&ChildOf>,
    mut shooters: Query<(
        Entity,
        &GlobalTransform,
        &Transform,
        &Turret,
        &WeaponLoadout,
        Option<&Team>,
        Option<&Muzzle>,
        &mut AimResult,
    )>,
) {
    for (entity, global, transform, turret, loadout, team, muzzle, mut result) in &mut shooters {
        // The selected weapon's own range, so the crosshair stops marking
        // targets a mortar cannot reach.
        let range = table
            .as_ref()
            .zip(loadout.selected_ammo())
            .and_then(|(table, ammo)| table.by_index(ammo.weapon))
            .map(|def| def.range)
            .unwrap_or(DEFAULT_AIM_RANGE);

        let hit = cast(
            &spatial,
            &surfaces,
            (&names, &hit_boxes),
            // From the barrel, as the shot will be: from the hull's centre
            // the crosshair marked what the shot would pass under.
            global.translation() + global.rotation() * muzzle.map_or(Vec3::ZERO, |m| m.0),
            aim_direction(transform.rotation, *turret),
            range,
            &[entity],
        );

        let mine = team.copied().unwrap_or(Team::PLAYER);
        *result = AimResult {
            hit: hit.entity,
            point: hit.point,
            damageable: hit
                .entity
                .and_then(|target| health_owner(target, |e| hostiles.contains(e), &parents))
                .and_then(|target| hostiles.get(target).ok())
                .is_some_and(|theirs| mine.is_hostile_to(*theirs)),
        };
    }
}

/// How far the aim ray looks when no weapon states a range.
///
/// // approximation: the longest `RANGE` in the data is the nuke's 1200 m, so
/// a shooter with no selected weapon still reaches as far as anything can.
pub const DEFAULT_AIM_RANGE: f32 = 1200.0;

/// The weapons plugin: selection, firing, movers and hit resolution.
pub struct WeaponsPlugin;

impl Plugin for WeaponsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<FireRequest>()
            .add_message::<ProjectileSpawned>()
            .add_message::<ProjectileHit>()
            .add_message::<WeaponSelected>()
            .add_message::<WeaponFired>()
            .add_message::<TriggerRefused>()
            .add_message::<MineBounced>()
            .add_systems(
                FixedUpdate,
                (
                    tick_cooldowns,
                    select_weapons,
                    follow_firepoints,
                    request_fire,
                    fire_weapons,
                    release_guidance,
                    move_projectiles,
                    trip_mines,
                    update_aim,
                )
                    .chain()
                    .in_set(SimSet::Weapons),
            )
            .add_systems(FixedUpdate, resolve_hits.in_set(SimSet::Damage));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STEERING: Steering = Steering {
        turn_rate: 1.6,
        pitch_rate: 1.2,
        acceleration: 20.0,
        top_speed: 80.0,
    };

    /// Headroom enough that the ceiling never enters into it.
    const OPEN_SKY: f32 = 1.0e6;

    #[test]
    fn an_unsteered_tethered_missile_flies_on_and_speeds_up() {
        let velocity = Vec3::new(0.0, 0.0, -60.0);

        let next = tethered_velocity(velocity, Vec2::ZERO, STEERING, OPEN_SKY, 0.5);

        assert!(
            next.normalize().angle_between(Vec3::NEG_Z) < 1e-5,
            "{next:?}"
        );
        assert!((next.length() - 70.0).abs() < 1e-3, "{next:?}");
    }

    /// `VELOCITY` is a cap: however long it flies, it is no faster.
    #[test]
    fn a_tethered_missile_never_passes_its_velocity() {
        let mut velocity = Vec3::new(0.0, 0.0, -80.0);
        for _ in 0..600 {
            velocity = tethered_velocity(velocity, Vec2::X * 0.3, STEERING, OPEN_SKY, 1.0 / 60.0);
        }

        assert!((velocity.length() - 80.0).abs() < 1e-3, "{velocity:?}");
    }

    /// Proportional: the cursor's offset is the fraction of the rate.
    #[test]
    fn steering_turns_at_the_deflected_fraction_of_the_rate() {
        let velocity = Vec3::new(0.0, 0.0, -80.0);

        let full = tethered_velocity(velocity, Vec2::X, STEERING, OPEN_SKY, 0.1);
        let half = tethered_velocity(velocity, Vec2::X * 0.5, STEERING, OPEN_SKY, 0.1);

        assert!(full.x > 0.0, "right turns toward +x: {full:?}");
        assert!((full.angle_between(velocity) - 0.16).abs() < 1e-4);
        assert!((half.angle_between(velocity) - 0.08).abs() < 1e-4);

        let up = tethered_velocity(velocity, Vec2::Y, STEERING, OPEN_SKY, 0.1);
        assert!(up.y > 0.0, "up climbs: {up:?}");
        assert!((up.angle_between(velocity) - 0.12).abs() < 1e-4);
    }

    /// At the tether's ceiling, up does nothing and a climb is pushed level;
    /// down still works.
    #[test]
    fn above_the_ceiling_the_missile_levels_off() {
        let climbing = Quat::from_rotation_x(0.5) * Vec3::NEG_Z * 80.0;

        let mut velocity = climbing;
        for _ in 0..120 {
            velocity = tethered_velocity(velocity, Vec2::Y, STEERING, 0.0, 1.0 / 60.0);
        }
        assert!(velocity.y.abs() < 1e-3, "it should be level: {velocity:?}");

        let diving = tethered_velocity(velocity, Vec2::NEG_Y, STEERING, 0.0, 0.1);
        assert!(diving.y < 0.0, "down still answers: {diving:?}");
    }

    /// Close under the ceiling only a shallow climb is allowed, so a missile
    /// can level off before it reaches it.
    #[test]
    fn little_headroom_allows_only_a_shallow_climb() {
        let mut velocity = Vec3::new(0.0, 0.0, -80.0);
        for _ in 0..600 {
            velocity = tethered_velocity(velocity, Vec2::Y, STEERING, 5.0, 1.0 / 60.0);
        }

        // 1 − cos θ = 5 × 1.2 / v.
        let pitch = velocity.normalize().y.asin();
        let allowed = (1.0 - 5.0 * 1.2 / velocity.length()).acos();
        assert!(pitch <= allowed + 1e-3, "{pitch} over {allowed}");
        assert!(pitch > 0.0, "it may still climb a little");
    }

    #[test]
    fn a_held_climb_stops_short_of_vertical() {
        let mut velocity = Vec3::new(0.0, 0.0, -80.0);
        for _ in 0..600 {
            velocity = tethered_velocity(velocity, Vec2::Y, STEERING, OPEN_SKY, 1.0 / 60.0);
        }

        let pitch = velocity.normalize().y.asin();
        assert!((pitch - TETHER_MAX_PITCH).abs() < 1e-3, "{pitch}");
    }

    #[test]
    fn splash_fades_linearly_to_the_rim_and_stops() {
        // Full at the centre.
        assert_eq!(splash_damage(100.0, 10.0, 0.0), 100.0);
        // The stated fraction at the rim.
        assert!((splash_damage(100.0, 10.0, 10.0) - 100.0 * SPLASH_EDGE_FRACTION).abs() < 1e-4);
        // Half way is half way between the two.
        let half = splash_damage(100.0, 10.0, 5.0);
        let expected = 100.0 * (1.0 + SPLASH_EDGE_FRACTION) / 2.0;
        assert!(
            (half - expected).abs() < 1e-4,
            "got {half}, wanted {expected}"
        );
        // Nothing beyond the radius.
        assert_eq!(splash_damage(100.0, 10.0, 10.01), 0.0);
        // A direct-hit weapon splashes nothing at all.
        assert_eq!(splash_damage(100.0, 0.0, 0.0), 0.0);
    }

    /// The aim is the hull's forward, turned by the turret. A vehicle facing
    /// −z with a level turret shoots along −z.
    #[test]
    fn the_aim_is_the_hull_forward_composed_with_the_turret() {
        let straight = aim_direction(Quat::IDENTITY, Turret::default());
        assert!((straight - Vec3::NEG_Z).length() < 1e-5, "got {straight:?}");

        // Ninety degrees of turret yaw turns −z into −x, the same convention
        // `PlayerStart::facing` uses.
        let yawed = aim_direction(
            Quat::IDENTITY,
            Turret {
                yaw: std::f32::consts::FRAC_PI_2,
                pitch: 0.0,
            },
        );
        assert!((yawed - Vec3::NEG_X).length() < 1e-5, "got {yawed:?}");

        // Positive pitch aims up.
        let pitched = aim_direction(
            Quat::IDENTITY,
            Turret {
                yaw: 0.0,
                pitch: std::f32::consts::FRAC_PI_4,
            },
        );
        assert!(pitched.y > 0.0, "positive pitch aims up, got {pitched:?}");

        // And the hull's own rotation composes on top.
        let hull = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let composed = aim_direction(hull, Turret::default());
        assert!((composed - Vec3::NEG_X).length() < 1e-5, "got {composed:?}");
    }

    #[test]
    fn a_zero_aim_never_produces_a_nan() {
        // `looking_to` and `Dir3::new` both reject a zero vector; the
        // fallback keeps a degenerate turret from poisoning the transform.
        let aim = aim_direction(
            Quat::IDENTITY,
            Turret {
                yaw: 0.0,
                pitch: 0.0,
            },
        );

        assert!(aim.is_finite());
        assert!((aim.length() - 1.0).abs() < 1e-5);
    }
}
