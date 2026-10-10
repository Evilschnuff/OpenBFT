//! Health, damage, destruction and burning.
//!
//! # What this module destroys, and what it does not
//!
//! Entities with [`Health`] — vehicles and turrets — die here: their health
//! reaches zero, [`Destroyed`] goes out, and the anim VM plays their death
//! anim while [`Drops`] spawns their pickups.
//!
//! **Level props do not.** A destructible prop in Recoil is an *anim
//! definition* with `WeaponHit` activation and a `health` threshold
//! (docs/data/anims.md): the def's own VM swaps `healthy` for `destroyed`, spawns
//! the pieces and calls the effects. So a hit on a level node emits
//! [`NodeDamage`] and the anim VM does the rest. There is no
//! `Destructible` component here, and no `DestroyEffect`.
//!
//! # Teams
//!
//! [`Team`] is a plain id, not a relationship: team 0 is the player's side
//! and team 1 the Network. Friendly fire is decided by the caller — splash
//! damages everything in radius, because in the original a badly aimed nuke
//! kills you too.

use bevy::prelude::*;

use crate::schedule::SimSet;
use crate::vehicle::tuning::DropDef;

/// How much punishment an entity has left.
///
/// Vehicles get theirs from `vehicle.zrd`'s `health`, turrets from `ai.zrd`'s
/// `HEALTH`. The player's entry is `[140, 60]`, hull and shield; only the
/// first is modelled here.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

impl Health {
    /// Full health of `max` points.
    pub fn new(max: f32) -> Health {
        Health { current: max, max }
    }

    /// How much is left, 0..1. Zero for a `max` of zero, rather than a NaN.
    pub fn fraction(&self) -> f32 {
        if self.max > 0.0 {
            (self.current / self.max).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Whether this entity is out of health.
    pub fn is_dead(&self) -> bool {
        self.current <= 0.0
    }

    /// Remove `amount` points, clamped to zero, and say whether that killed.
    ///
    /// Returns `false` for a hit on something already dead, so a second
    /// projectile landing on the same tick does not emit a second
    /// [`Destroyed`].
    pub fn apply(&mut self, amount: f32) -> bool {
        if self.is_dead() {
            return false;
        }

        self.current = (self.current - amount).max(0.0);
        self.is_dead()
    }

    /// Add `amount` points, clamped to [`Self::max`]. What a nanite pickup
    /// does.
    pub fn heal(&mut self, amount: f32) {
        self.current = (self.current + amount).min(self.max);
    }
}

/// Which side an entity is on. 0 is the player's, 1 the Network's.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Team(pub u8);

impl Team {
    /// The player's side.
    pub const PLAYER: Team = Team(0);
    /// The Network's.
    pub const ENEMY: Team = Team(1);

    /// Whether these two are on opposite sides.
    pub fn is_hostile_to(&self, other: Team) -> bool {
        *self != other
    }
}

/// Damage landing on an entity.
///
/// A `Message` rather than an observer `Event`: several systems read the
/// stream (health, the HUD, the burning state, the hit sounds) and
/// Bevy 0.19 splits the two kinds — `FormChanged` is the same.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct DamageEvent {
    pub target: Entity,
    pub amount: f32,
    /// Who fired, where there is anyone: `None` for scripted damage.
    pub source: Option<Entity>,
    /// Where the hit landed, in world space — the HUD's damage direction.
    pub at: Vec3,
    pub kind: DamageKind,
}

/// What caused a [`DamageEvent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DamageKind {
    /// A direct hit by the weapon at this index in the table.
    Projectile(usize),
    /// Area damage from the weapon at this index.
    Splash(usize),
    /// Running into something, from `collision_d`.
    Collision,
    /// The napalm fire this entity is still carrying.
    Burning,
    /// A mission script or an anim def.
    Script,
}

impl DamageKind {
    /// The weapon this damage came from, where it came from one.
    pub fn weapon(&self) -> Option<usize> {
        match self {
            DamageKind::Projectile(index) | DamageKind::Splash(index) => Some(*index),
            _ => None,
        }
    }
}

/// Damage landing on a **level node** rather than an entity.
///
/// Consumed by the anim VM, which accumulates it per node and
/// activates the matching `WeaponHit` / `CollideHit` def once its `health`
/// threshold is reached (docs/data/anims.md). This module does not know which
/// nodes are destructible and does not need to: it reports every hit and the
/// VM ignores the ones no def watches.
#[derive(Message, Debug, Clone, PartialEq)]
pub struct NodeDamage {
    /// The level node entity that was hit.
    pub node: Entity,
    /// That node's name, which is what the anim defs are keyed by.
    pub name: String,
    pub amount: f32,
    pub kind: DamageKind,
    pub at: Vec3,
}

/// An entity's health reached zero.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct Destroyed {
    pub entity: Entity,
    /// Who killed it, for the multiplayer kill line and for score.
    pub by: Option<Entity>,
    pub at: Vec3,
    /// Which weapon landed the killing blow, so the anim VM can pick the
    /// right `KILL_ANIMATION` (`kill_mortar`, …).
    pub weapon: Option<usize>,
}

/// What this entity leaves behind when destroyed.
///
/// From `vehicle.zrd`'s `pickups` and `nanite` keys, which the vehicle table
/// parses into [`DropDef`].
#[derive(Component, Debug, Clone, Default, PartialEq)]
pub struct Drops(pub Vec<DropDef>);

/// This entity is on fire and losing health.
///
/// Set by a `CATCHES_FIRE` weapon (napalm, `wep_3.1`) and cleared when the
/// timer runs out. `player.zrd`'s `burning_anim` is `["burning_model", 4]` —
/// the anim name and **four seconds**, which is where [`Self::for_player`]'s
/// duration comes from.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct Burning {
    /// Seconds of fire left.
    pub remaining: f32,
    /// Damage per second while it burns.
    pub dps: f32,
}

/// How much a burning entity loses per second.
///
/// // approximation: no reader field states it. `player.zrd`'s `burning_anim`
/// gives the duration (4 s) and napalm's own `DAMAGE` is 3.0, so burning for
/// the whole duration costs about as much again as the hit that started it.
pub const BURNING_DPS: f32 = 3.0;

impl Burning {
    /// A fire of the duration `player.zrd`'s `burning_anim` states.
    pub fn for_player(duration: f32) -> Burning {
        Burning {
            remaining: duration,
            dps: BURNING_DPS,
        }
    }
}

/// The result of this tick's aim ray, for the HUD's crosshair.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct AimResult {
    /// What the ray hit, where it hit anything.
    pub hit: Option<Entity>,
    /// Where it landed, or the end of the ray when it hit nothing.
    pub point: Vec3,
    /// Whether that is something this shooter can hurt: it has [`Health`] and
    /// is on another [`Team`].
    pub damageable: bool,
}

impl Default for AimResult {
    fn default() -> AimResult {
        AimResult {
            hit: None,
            point: Vec3::ZERO,
            damageable: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Systems
// ---------------------------------------------------------------------------

/// Apply this tick's [`DamageEvent`]s and emit [`Destroyed`] for the kills.
///
/// Damage is applied in arrival order and an entity that dies partway through
/// stops taking it — [`Health::apply`] returns `false` for an already-dead
/// target — so two projectiles landing on the same tick produce one
/// [`Destroyed`], with the weapon that actually killed it.
pub fn apply_damage(
    mut damage: MessageReader<DamageEvent>,
    mut destroyed: MessageWriter<Destroyed>,
    mut healths: Query<&mut Health>,
) {
    for event in damage.read() {
        let Ok(mut health) = healths.get_mut(event.target) else {
            // A projectile can outlive its target by a tick. Not an error.
            continue;
        };

        if health.apply(event.amount) {
            destroyed.write(Destroyed {
                entity: event.target,
                by: event.source,
                at: event.at,
                weapon: event.kind.weapon(),
            });
        }
    }
}

/// Burn down the [`Burning`] timer and bill its damage.
///
/// Runs before [`apply_damage`] in the same set, so a fire that finishes a
/// target kills it on the tick it does rather than the one after.
pub fn burn(
    time: Res<Time<Fixed>>,
    mut commands: Commands,
    mut damage: MessageWriter<DamageEvent>,
    mut burning: Query<(Entity, &mut Burning, &GlobalTransform)>,
) {
    let delta = time.delta_secs();

    for (entity, mut fire, transform) in &mut burning {
        let seconds = delta.min(fire.remaining);
        if seconds > 0.0 {
            damage.write(DamageEvent {
                target: entity,
                amount: fire.dps * seconds,
                source: None,
                at: transform.translation(),
                kind: DamageKind::Burning,
            });
        }

        fire.remaining -= delta;
        if fire.remaining <= 0.0 {
            commands.entity(entity).remove::<Burning>();
        }
    }
}

/// The damage plugin: health, burning and the message streams.
///
/// Destruction *effects* are not here. The anim VM plays the death anim and
/// handles level props through [`NodeDamage`], and `bft-game` the explosion;
/// they read [`Destroyed`].
pub struct DamagePlugin;

impl Plugin for DamagePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<DamageEvent>()
            .add_message::<NodeDamage>()
            .add_message::<Destroyed>()
            .add_systems(
                FixedUpdate,
                (burn, apply_damage).chain().in_set(SimSet::Damage),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_clamps_at_zero_and_reports_the_kill_once() {
        let mut health = Health::new(10.0);

        assert!(!health.apply(4.0));
        assert_eq!(health.current, 6.0);
        assert!(health.apply(20.0), "the blow that kills reports it");
        assert_eq!(health.current, 0.0, "clamped, never negative");
        assert!(
            !health.apply(20.0),
            "a second hit on a corpse is not a second kill"
        );
    }

    #[test]
    fn healing_stops_at_max() {
        let mut health = Health::new(140.0);
        health.apply(100.0);

        health.heal(20.0);
        assert_eq!(health.current, 60.0);
        health.heal(1000.0);
        assert_eq!(health.current, 140.0);
    }

    #[test]
    fn the_fraction_survives_a_zero_max() {
        assert_eq!(Health::new(0.0).fraction(), 0.0);
        assert_eq!(Health::new(140.0).fraction(), 1.0);

        let mut health = Health::new(140.0);
        health.apply(70.0);
        assert!((health.fraction() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn teams_are_hostile_to_everyone_but_themselves() {
        assert!(Team::PLAYER.is_hostile_to(Team::ENEMY));
        assert!(!Team::PLAYER.is_hostile_to(Team(0)));
        assert!(Team(2).is_hostile_to(Team(1)));
    }

    #[test]
    fn only_weapon_damage_names_a_weapon() {
        assert_eq!(DamageKind::Projectile(7).weapon(), Some(7));
        assert_eq!(DamageKind::Splash(7).weapon(), Some(7));
        assert_eq!(DamageKind::Collision.weapon(), None);
        assert_eq!(DamageKind::Burning.weapon(), None);
        assert_eq!(DamageKind::Script.weapon(), None);
    }

    /// One simulation tick, in seconds.
    const TICK: f32 = 1.0 / crate::schedule::SIM_HZ as f32;

    /// A headless app with just the damage systems, one fixed tick per
    /// [`tick`].
    ///
    /// The systems run in `FixedUpdate`, where the plugin puts them, so
    /// `Time<Fixed>` really advances; a copy in `Update` would read a delta
    /// of zero and every burning test would pass for the wrong reason.
    ///
    /// `TimeUpdateStrategy::ManualDuration` rather than nudging
    /// `Time<Virtual>`: `TimePlugin` recomputes virtual time from real time
    /// every update, so a hand-set virtual delta is discarded before
    /// `FixedUpdate` sees it (`tests/physics_step.rs` found the same).
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
            std::time::Duration::from_secs_f64(1.0 / crate::schedule::SIM_HZ),
        ));
        app.insert_resource(Time::<Fixed>::from_hz(crate::schedule::SIM_HZ));
        app.add_message::<DamageEvent>()
            .add_message::<NodeDamage>()
            .add_message::<Destroyed>()
            .add_systems(FixedUpdate, (burn, apply_damage).chain());
        // Without `finish`, `MinimalPlugins` never installs its schedule
        // runner and `FixedUpdate` silently never runs. And the *first*
        // `update` runs no fixed tick at all — virtual time has not
        // accumulated one yet — so it is spent here rather than swallowing
        // the first tick of every test. `tests/physics_step.rs`'s
        // `physics_app` primes itself the same way.
        app.finish();
        app.update();
        app
    }

    /// Run exactly one fixed tick.
    fn tick(app: &mut App) {
        app.update();
    }

    #[test]
    fn damage_reaching_zero_emits_destroyed_with_the_killing_weapon() {
        let mut app = app();
        let target = app.world_mut().spawn(Health::new(10.0)).id();

        app.world_mut().write_message(DamageEvent {
            target,
            amount: 18.0,
            source: None,
            at: Vec3::new(1.0, 2.0, 3.0),
            kind: DamageKind::Projectile(4),
        });
        tick(&mut app);

        let kills: Vec<Destroyed> = app
            .world_mut()
            .resource_mut::<Messages<Destroyed>>()
            .drain()
            .collect();

        assert_eq!(kills.len(), 1);
        assert_eq!(kills[0].entity, target);
        assert_eq!(kills[0].weapon, Some(4), "the anim events pick KILL_ANIMATION");
        assert_eq!(kills[0].at, Vec3::new(1.0, 2.0, 3.0));
    }

    /// Two projectiles landing on the same tick: one death, credited to the
    /// one that actually finished it.
    #[test]
    fn a_second_hit_on_the_same_tick_does_not_kill_twice() {
        let mut app = app();
        let target = app.world_mut().spawn(Health::new(10.0)).id();

        for (amount, weapon) in [(6.0, 1usize), (6.0, 2), (6.0, 3)] {
            app.world_mut().write_message(DamageEvent {
                target,
                amount,
                source: None,
                at: Vec3::ZERO,
                kind: DamageKind::Projectile(weapon),
            });
        }
        tick(&mut app);

        let kills: Vec<Destroyed> = app
            .world_mut()
            .resource_mut::<Messages<Destroyed>>()
            .drain()
            .collect();

        assert_eq!(kills.len(), 1);
        assert_eq!(kills[0].weapon, Some(2), "the second shot took it to zero");
    }

    #[test]
    fn damage_to_a_despawned_target_is_dropped_rather_than_panicking() {
        let mut app = app();
        let gone = app.world_mut().spawn_empty().id();
        app.world_mut().entity_mut(gone).despawn();

        app.world_mut().write_message(DamageEvent {
            target: gone,
            amount: 100.0,
            source: None,
            at: Vec3::ZERO,
            kind: DamageKind::Projectile(0),
        });
        tick(&mut app);

        assert_eq!(app.world().resource::<Messages<Destroyed>>().len(), 0);
    }

    /// Napalm's fire: it bills once per tick and clears itself when the
    /// timer runs out.
    #[test]
    fn burning_bills_per_tick_and_expires() {
        let mut app = app();
        let target = app
            .world_mut()
            .spawn((
                Health::new(100.0),
                Burning {
                    remaining: 2.0 * TICK,
                    dps: 60.0,
                },
                GlobalTransform::default(),
            ))
            .id();

        tick(&mut app);
        let after_one = app.world().get::<Health>(target).expect("alive").current;
        assert!(after_one < 100.0, "it burns on the first tick");
        assert!(app.world().get::<Burning>(target).is_some());

        tick(&mut app);
        assert!(
            app.world().get::<Burning>(target).is_none(),
            "the fire goes out when the timer does"
        );

        let after_two = app.world().get::<Health>(target).expect("alive").current;
        tick(&mut app);
        assert_eq!(
            app.world().get::<Health>(target).expect("alive").current,
            after_two,
            "and stops costing health"
        );
    }

    /// The last partial tick must not bill a whole one, or a 4-second fire
    /// costs more than four seconds of damage.
    #[test]
    fn the_final_partial_tick_bills_only_what_is_left() {
        let mut app = app();
        let target = app
            .world_mut()
            .spawn((
                Health::new(100.0),
                // A third of a tick left, at 60 damage a second.
                Burning {
                    remaining: TICK / 3.0,
                    dps: 60.0,
                },
                GlobalTransform::default(),
            ))
            .id();

        tick(&mut app);

        let lost = 100.0 - app.world().get::<Health>(target).expect("alive").current;
        assert!(
            (lost - 60.0 * TICK / 3.0).abs() < 1e-4,
            "billed {lost}, wanted a third of a tick"
        );
    }
}
