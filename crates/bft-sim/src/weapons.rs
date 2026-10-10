//! The resolved weapon table, firing, and projectile movement.
//!
//! # Where every number comes from
//!
//! Primary is `weapons.zrd`'s `BALLISTICS` block (READER-SCHEMA §2.3): 19
//! weapons keyed `wep_<slot>.<variant>`, slot 1–9 by weak/strong, plus the
//! enemy-only `wep_9.2`. `assets/tuning/weapons.ron` is an override layer in
//! which every leaf is optional, so an empty file is the correct default.
//!
//! Who fires what comes from elsewhere: the *player's* loadout is
//! `vehicle.zrd`'s `weapons` rows (the vehicle table parses them into
//! [`VehicleWeaponRow`](crate::vehicle::tuning::VehicleWeaponRow)), and an AI
//! turret's rate is `ai.zrd`'s own `FIRE_RATE`. `WeaponDef`'s
//! own `fire_rate` is only the fallback for a shooter with no row.
//!
//! # What the data actually looks like
//!
//! Measured over the shipped `weapons.zrd` (docs/data/reader.md); see also
//! [`WeaponDef`]'s field docs. The three that matter most:
//!
//! - `LOCK_ON` is a **float** (0.1, 1.0, 2.0), not a mode enum. It reads as
//!   a lock cone half-angle or acquisition rate, not "0 none / 1 lock / 2
//!   small", because `wep_7.1` — the arc sabre, a `BEAM` — carries 0.0 while
//!   `wep_8.1` (tether-guided) carries 0.1.
//! - There is a `PITCH_RATE` key, on the two
//!   tether-guided weapons, and a `FREEZE` block on the freon cannon
//!   alongside the `HEAT`/`DESIGNATE` ones.
//! - `IMPACT`'s children are **not all surfaces**: `default`, `water` and
//!   `quicksand` are, but `HEAT`, `FREEZE`, `DESIGNATE` and
//!   `ANIMATION_ALWAYS` sit at the same level and are per-weapon effects.
//!
//! # `FIRE_RATE`'s two units
//!
//! READER-SCHEMA §5.2. The split is settled by comparing each
//! weapon's `FIRE_RATE` with the player's own `interval` column for the same
//! weapon in `vehicle.zrd`: the two pulse guns read 12 against intervals of
//! 0.15 and 0.12 s, so 12 is shots per second; every other weapon reads
//! 0.5–2.0 against intervals of 1.0–2.0 s, so those are seconds between
//! shots. [`WeaponDef::shot_interval`] applies the split; the field itself
//! stays raw.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bft_assets::ReaderValue;

use crate::vehicle::tuning::VehicleDef;

pub mod ballistics;
pub mod firing;
pub mod overrides;

pub use ballistics::{launch_pitch, launch_velocity};
pub use firing::{
    FireRequest, Firepoint, Guided, Landing, MineBounced, Muzzle, OwnParts, Projectile,
    ProjectileHit, ProjectileSpawned, Refusal, Steering, TriggerRefused, TwinBarrels, WeaponFired,
    WeaponSelected, WeaponsPlugin, health_owner, projectile_body, tethered_velocity,
};
pub use overrides::{WeaponDefOverride, WeaponOverrides};

/// The `weapons.zrd` entry of the global reader archive.
pub const WEAPONS_ENTRY: &str = "weapons.zrd";

/// The key under which the 19 weapons live.
pub const BALLISTICS: &str = "BALLISTICS";

/// `WeaponSetMaxTetherAltitude` in all 13 missions, in metres.
///
/// The interp value is the authority ([`build_weapon_table`] takes it as an
/// argument); this is the measured default for a table built without one.
pub const DEFAULT_TETHER_MAX_ALTITUDE: f32 = 30.0;

/// The `AMMO_LIMIT` the data uses for "unlimited".
///
/// Read as an integer, never an `f32`: 123 456 789 rounds to 123 456 792 as a
/// float, the same trap as the same column in `vehicle.zrd`.
pub const UNLIMITED_AMMO: i64 = 123_456_789;

/// `FIRE_RATE` at or above this reads as shots per second, below it as
/// seconds between shots.
///
/// The data has no value between 2.0 and 12.0, so any threshold in that gap
/// gives the same split; 3.0 is the middle of it. See the module docs for how
/// the split was settled.
pub const FIRE_RATE_HZ_THRESHOLD: f32 = 3.0;

/// What [`build_weapon_table`] could not do.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum LoadError {
    /// The archive has no `weapons.zrd`, so this is not the global archive.
    #[error("the reader archive has no `{WEAPONS_ENTRY}` entry")]
    NoWeaponsEntry,
    /// `weapons.zrd` has no `BALLISTICS`, so it is a different file than the
    /// one this code understands.
    #[error("`{WEAPONS_ENTRY}` has no `{BALLISTICS}` block")]
    NoBallistics,
    /// Not one weapon parsed. Every shipped archive has 19.
    #[error("`{WEAPONS_ENTRY}`'s `{BALLISTICS}` holds no weapon this can read")]
    NoWeapons,
}

/// Every weapon, resolved from `weapons.zrd` with `weapons.ron` applied.
#[derive(Resource, Debug, Clone)]
pub struct WeaponTable {
    /// In file order, which is `wep_1.0` … `wep_9.2`.
    pub weapons: Vec<WeaponDef>,
    /// Weapon id to index into [`Self::weapons`].
    pub by_id: HashMap<String, usize>,
    /// The four top-level warning sounds.
    pub warnings: WeaponWarnings,
    /// `WeaponSetMaxTetherAltitude`, in metres: how high a tether-guided
    /// missile may be steered (docs/data/interp.md).
    pub tether_max_altitude: f32,
}

impl WeaponTable {
    /// One weapon by its `wep_S.V` id.
    pub fn get(&self, id: &str) -> Option<&WeaponDef> {
        self.index(id).map(|index| &self.weapons[index])
    }

    /// One weapon's index, which is what every component stores.
    pub fn index(&self, id: &str) -> Option<usize> {
        self.by_id.get(id).copied()
    }

    /// One weapon by index, for a component that already holds one.
    pub fn by_index(&self, index: usize) -> Option<&WeaponDef> {
        self.weapons.get(index)
    }

    /// The weapon in a slot and variant, if the data has one.
    pub fn slot(&self, slot: u8, strong: bool) -> Option<&WeaponDef> {
        self.get(&format!("wep_{slot}.{}", u8::from(strong)))
    }
}

/// `weapons.zrd`'s four top-level warning sounds, as `snd_*` names.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WeaponWarnings {
    /// `LOCK_ON_WARNING`: something has a missile lock on you.
    pub lock_on: String,
    /// `NO_AMMO_WARNING`: the trigger was pulled on an empty weapon.
    pub no_ammo: String,
    /// `TRIGGER_INACTIVE`.
    pub trigger_inactive: String,
    /// `WEAPON_INACTIVE`.
    pub weapon_inactive: String,
}

/// One `BALLISTICS` entry.
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponDef {
    /// The reader key: `"wep_2.1"`.
    pub id: String,
    /// HUD slot, 1–9, from the id.
    pub slot: u8,
    /// 0 weak, 1 strong, 2 the enemy-only `wep_9.2`.
    pub variant: u8,
    /// `NAME`: the short HUD name ("MDM").
    pub hud_name: String,
    /// `DESC`: the `MSG_DESC_*` message key for the long name.
    pub desc_key: String,
    /// `KILL_VERB`: the multiplayer kill line's verb.
    pub kill_verb: KillVerb,
    /// How the projectile moves, derived from the flags below.
    pub mover: Mover,
    /// `DAMAGE`: hit points removed by a direct hit, 1.2–140.
    pub damage: f32,
    /// `AMMO_LIMIT`: the most that can be carried. [`u32::MAX`] where the
    /// data says [`UNLIMITED_AMMO`].
    pub ammo_limit: u32,
    /// `RANGE`: how far the projectile travels before expiring, metres.
    pub range: f32,
    /// `VELOCITY`: muzzle speed, m/s. Absent on the three `BEAM` weapons
    /// (hitscan has no speed) — and there only.
    pub velocity: Option<f32>,
    /// `GRAVITY`: 9.8 on the ballistic weapons, 0 elsewhere. Note this is
    /// **not** `player.zrd`'s 40 m/s²: projectiles fall at their own rate.
    pub gravity: f32,
    /// `ACCELERATION`: a homing missile's thrust, m/s².
    pub acceleration: Option<f32>,
    /// `TURN_RATE`: how fast a homing missile can turn, rad/s.
    pub turn_rate: Option<f32>,
    /// `PITCH_RATE`: the tether-guided pair's separate vertical turn rate.
    /// Present on `wep_8.1` and `wep_9.1` only.
    pub pitch_rate: Option<f32>,
    /// `FIRE_RATE`, **raw**: shots per second for the pulse guns (12) and
    /// seconds between shots for everything else. Use
    /// [`Self::shot_interval`] rather than this.
    pub fire_rate: f32,
    /// `IMPACT_PROXIMITY`: splash and mine-trigger radius, metres. 0 for the
    /// direct-hit guns, 256 for the two nukes.
    pub splash_radius: f32,
    /// `IMPACT_TYPE`: 0 or 2, 2 on the area weapons. Meaning unsettled.
    pub impact_type: u8,
    /// `CRATER`: radius and depth of the terrain deformation.
    pub crater: Option<(u32, u32)>,
    /// `FLYOUT_HEALTH`: the projectile can be shot down, with this much
    /// health.
    pub flyout_health: Option<u32>,
    /// `RELOAD`: only one may be in flight or placed at a time.
    pub reload_one: bool,
    /// `REMOTE_DETONATE`: a second trigger pull detonates it.
    pub remote_detonate: bool,
    /// `TETHER_GUIDED`: steered by the player's aim while in flight.
    pub tether_guided: bool,
    /// `LOCK_ON`, as the float the data holds — **not** a mode enum; see the
    /// module docs. 0.1 on the tether-guided pair, 1.0 on the lock-on pair,
    /// 2.0 on the enemy missile, 0.0 on the arc sabre.
    pub lock_on: Option<f32>,
    /// `MULTI_TARGET`: the arc sabre chains to several targets.
    pub multi_target: bool,
    /// `CATCHES_FIRE`: napalm sets what it hits burning.
    pub catches_fire: bool,
    /// `QUICKSAND`: creates a quicksand patch of this radius and depth.
    pub quicksand: Option<(u32, u32)>,
    /// `BEAM`: `[?, width, ?]` on the three hitscan weapons.
    pub beam: Option<[f32; 3]>,
    /// `MINE`: stationary once placed.
    pub mine: bool,
    /// `FIRE`: the muzzle effect.
    pub fire: FireFx,
    /// `FLYOUT`: the projectile's model and trail.
    pub flyout: FlyoutFx,
    /// `IMPACT`'s surface children, keyed by the `Soil` the hit polygon has.
    pub impact: HashMap<Surface, ImpactFx>,
    /// `IMPACT/ANIMATION_ALWAYS`: play the impact animation even when the
    /// projectile hit nothing. The two nukes.
    pub impact_always: bool,
    /// `IMPACT/HEAT`: six floats, on the laser sabre.
    pub heat: Option<[f32; 6]>,
    /// `IMPACT/DESIGNATE`: six floats, on the laser designator.
    pub designate: Option<[f32; 6]>,
    /// `IMPACT/FREEZE`: six floats, on the freon cannon.
    pub freeze: Option<[f32; 6]>,
    /// `BOUNCE/SOUND`: the remote HE mine's bounce sound.
    pub bounce_sound: Option<String>,
}

impl WeaponDef {
    /// Seconds between shots, resolving `FIRE_RATE`'s two units.
    ///
    /// The fallback for a shooter with no per-vehicle `interval` column; the
    /// player always has one and uses that instead. See the module docs.
    pub fn shot_interval(&self) -> f32 {
        if self.fire_rate >= FIRE_RATE_HZ_THRESHOLD {
            1.0 / self.fire_rate
        } else {
            self.fire_rate
        }
    }

    /// Whether this weapon's ammunition is effectively endless.
    pub fn is_unlimited(&self) -> bool {
        self.ammo_limit == u32::MAX
    }

    /// How long a projectile lives before it expires, in seconds.
    ///
    /// `RANGE` is a distance and the mover needs a time, so the two are
    /// divided. A hitscan or stationary weapon has no flight time and gets
    /// [`f32::INFINITY`]; a mine's lifetime is the level's.
    pub fn flight_time(&self) -> f32 {
        match self.velocity {
            Some(velocity) if velocity > 0.0 && !matches!(self.mover, Mover::Mine) => {
                self.range / velocity
            }
            _ => f32::INFINITY,
        }
    }
}

/// How a weapon's projectile moves.
///
/// Derived from the flags rather than stored: the data has no "type" column,
/// only the presence of `BEAM`, `MINE`, `TETHER_GUIDED`, `LOCK_ON`/`TURN_RATE`
/// and `GRAVITY`. The order of the tests matters — the arc sabre carries both
/// `BEAM` and `LOCK_ON`, the mines carry both `MINE` and `GRAVITY`, and the
/// tether-guided pair carry `TURN_RATE` and a `LOCK_ON` of 0.1 as well.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mover {
    /// Constant velocity along the aim direction.
    Straight,
    /// Constant velocity plus the weapon's own `GRAVITY`.
    Ballistic,
    /// Steers toward a target at `TURN_RATE`.
    Homing,
    /// Steered by its owner while [`firing::Guided`]: turns at a fraction of
    /// `TURN_RATE` sideways and `PITCH_RATE` up and down, set by
    /// [`PlayerInput::guide`](crate::vehicle::PlayerInput::guide). Once let
    /// go it flies on straight. `wep_8.1` and `wep_9.1`.
    Tethered,
    /// Falls and bounces until it lies still, arms, then waits: a proximity
    /// mine for something hostile to drive over it, a remote one for its
    /// layer's detonate key. `IMPACT_PROXIMITY` is its blast, not its
    /// trigger ([`firing::trip_mines`]).
    Mine,
    /// Hitscan: the hit is resolved on the tick the trigger is pulled.
    Beam,
}

impl Mover {
    /// Which mover a parsed weapon's flags imply.
    ///
    /// Public so a test can check the derivation without building a whole
    /// [`WeaponDef`], and so the rule lives in exactly one place.
    pub fn derive(beam: bool, mine: bool, tethered: bool, homing: bool, gravity: f32) -> Mover {
        if beam {
            Mover::Beam
        } else if mine {
            Mover::Mine
        } else if tethered {
            // Before homing: the pair also carry `TURN_RATE` and `LOCK_ON`
            // 0.1, and read as homing they locked onto whatever was ahead
            // instead of going where they were steered.
            Mover::Tethered
        } else if homing {
            Mover::Homing
        } else if gravity > 0.0 {
            Mover::Ballistic
        } else {
            Mover::Straight
        }
    }
}

/// `KILL_VERB`: a message key, or the one weapon that spells the word out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillVerb {
    /// `MSG_KILL_VERB_*`, resolved through the message table.
    Key(String),
    /// `wep_6.1`'s literal `"executed"`, and `wep_9.2`'s absence.
    Literal(String),
}

impl KillVerb {
    /// A `KILL_VERB` string as the data spells it.
    ///
    /// Everything starting `MSG_` is a key; the laser sabre's `"executed"` is
    /// not, and is the only literal in the file.
    pub fn parse(text: &str) -> KillVerb {
        if text.starts_with("MSG_") {
            KillVerb::Key(text.to_string())
        } else {
            KillVerb::Literal(text.to_string())
        }
    }
}

impl Default for KillVerb {
    /// What `wep_9.2`, the only weapon with no `KILL_VERB`, gets.
    fn default() -> KillVerb {
        KillVerb::Literal(String::new())
    }
}

/// Which `IMPACT` block a hit uses, chosen from the hit polygon's `Soil`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Surface {
    /// Everything that is not liquid.
    #[default]
    Default,
    /// `Soil::Water`.
    Water,
    /// `Soil::Quicksand`.
    Quicksand,
}

impl Surface {
    /// The reader key for this surface, as `IMPACT` spells it.
    pub fn key(&self) -> &'static str {
        match self {
            Surface::Default => "default",
            Surface::Water => "water",
            Surface::Quicksand => "quicksand",
        }
    }

    /// Which surface a soil selects.
    ///
    /// Lava has no `IMPACT` block of its own in any of the 19 weapons, so it
    /// falls back to `default` rather than getting a fourth variant.
    pub fn from_soil(soil: bft_assets::materials::Soil) -> Surface {
        use bft_assets::materials::Soil;
        match soil {
            Soil::Water => Surface::Water,
            Soil::Quicksand => Surface::Quicksand,
            _ => Surface::Default,
        }
    }
}

/// `FIRE`: what happens at the muzzle.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FireFx {
    /// `ANIMATION`: the muzzle flash, a `.flt` template node.
    pub animation: Option<String>,
    /// `SOUND`.
    pub sound: Option<String>,
    /// `RANDOM_ROTATE`: spin the muzzle flash by a random roll.
    pub random_rotate: bool,
}

/// `FLYOUT`: what the projectile looks like in the air.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FlyoutFx {
    /// `MODEL`: the projectile's own `.flt` template node.
    pub model: Option<String>,
    /// `MODEL_ANIMATION`: an anim played on that model.
    pub model_animation: Option<String>,
    /// `ANIMATION`: the trail (`smktrl1.flt`).
    pub animation: Option<String>,
    /// `SOUND`: the in-flight loop.
    pub sound: Option<String>,
}

/// One `IMPACT/<surface>` block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImpactFx {
    /// `ANIMATION`: the impact effect.
    pub animation: Option<String>,
    /// `KILL_ANIMATION`: played on a victim this hit killed.
    pub kill_animation: Option<String>,
    /// `DAMAGE_ANIMATION`: played on a victim that survived.
    pub damage_animation: Option<String>,
    /// `DAMAGE_ANIM_ON_HEALTH`: `[fraction, anim]` thresholds, in file order.
    pub damage_anim_on_health: Vec<(f32, String)>,
    /// `EFFECT`: an `effects.zrd` sprite cycle.
    pub effect: Option<String>,
    /// `SOUND`: one name, or several to pick from at random.
    pub sounds: Vec<String>,
    /// `BOUNCE_SOUND`.
    pub bounce_sound: Option<String>,
}

impl ImpactFx {
    /// The damage anim for a victim left at this fraction of its health.
    ///
    /// The thresholds are ascending in the data (0.05, 0.15, 1.0) and read as
    /// "the first one this fraction is at or below", so a nearly-dead target
    /// gets the first entry and a scratched one the last.
    pub fn damage_anim_for(&self, health_fraction: f32) -> Option<&str> {
        self.damage_anim_on_health
            .iter()
            .find(|(threshold, _)| health_fraction <= *threshold)
            .map(|(_, name)| name.as_str())
            .or(self.damage_animation.as_deref())
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Build the weapon table from a reader archive and the RON overrides.
///
/// `reader` is the global [`bft_assets::ReaderData`].
///
/// `tether_max_altitude` comes from
/// [`bft_assets::interp::runtime_bindings`]'s `max_tether_altitude`
/// ([`DEFAULT_TETHER_MAX_ALTITUDE`] in all 13 missions).
pub fn build_weapon_table(
    reader: &bft_assets::ReaderData,
    overrides: &WeaponOverrides,
    tether_max_altitude: f32,
) -> Result<WeaponTable, LoadError> {
    let weapons_entry = reader
        .read(WEAPONS_ENTRY)
        .ok_or(LoadError::NoWeaponsEntry)?;
    let ballistics = weapons_entry
        .get(BALLISTICS)
        .ok_or(LoadError::NoBallistics)?;

    let mut weapons = Vec::new();
    let mut by_id = HashMap::default();

    for (id, value) in ballistics.pairs() {
        let Some(def) = parse_weapon(id, value) else {
            warn!("{WEAPONS_ENTRY}: weapon `{id}` is not the shape this reads; skipping it");
            continue;
        };
        by_id.insert(id.to_string(), weapons.len());
        weapons.push(def);
    }

    if weapons.is_empty() {
        return Err(LoadError::NoWeapons);
    }

    let string = |key: &str| {
        weapons_entry
            .get(key)
            .and_then(|v| v.string())
            .unwrap_or_default()
            .to_string()
    };

    let mut table = WeaponTable {
        weapons,
        by_id,
        warnings: WeaponWarnings {
            lock_on: string("LOCK_ON_WARNING"),
            no_ammo: string("NO_AMMO_WARNING"),
            trigger_inactive: string("TRIGGER_INACTIVE"),
            weapon_inactive: string("WEAPON_INACTIVE"),
        },
        tether_max_altitude,
    };

    overrides.apply(&mut table);

    Ok(table)
}

/// Parse one `BALLISTICS` entry.
///
/// `None` when the id is not `wep_<slot>.<variant>`, which no shipped entry
/// is: the slot and variant have no other source and a weapon without them
/// cannot be put in a loadout.
fn parse_weapon(id: &str, value: ReaderValue<'_>) -> Option<WeaponDef> {
    let (slot, variant) = parse_id(id)?;

    let float = |key: &str| value.get(key).and_then(|v| v.float());
    let flag = |key: &str| value.get(key).and_then(|v| v.flag()).unwrap_or(false);
    let text = |key: &str| value.get(key).and_then(|v| v.string()).map(str::to_string);
    let pair = |key: &str| {
        value
            .get(key)
            .and_then(|v| v.float_array::<2>())
            .map(|[a, b]| (a as u32, b as u32))
    };

    let impact_block = value.get("IMPACT");
    let gravity = float("GRAVITY").unwrap_or(0.0);
    let beam = value.get("BEAM").and_then(|v| v.float_array::<3>());
    let mine = flag("MINE");
    let lock_on = float("LOCK_ON");
    let turn_rate = float("TURN_RATE");
    // Homing is "it steers": a turn rate, or a non-zero lock-on. The arc
    // sabre's `LOCK_ON 0.0` is neither, and it is a beam anyway.
    let homing = turn_rate.is_some() || lock_on.is_some_and(|value| value > 0.0);

    Some(WeaponDef {
        id: id.to_string(),
        slot,
        variant,
        hud_name: text("NAME").unwrap_or_default(),
        desc_key: text("DESC").unwrap_or_default(),
        kill_verb: text("KILL_VERB")
            .map(|verb| KillVerb::parse(&verb))
            .unwrap_or_default(),
        mover: Mover::derive(beam.is_some(), mine, flag("TETHER_GUIDED"), homing, gravity),
        damage: float("DAMAGE").unwrap_or(0.0),
        ammo_limit: parse_ammo_limit(value.get("AMMO_LIMIT")),
        range: float("RANGE").unwrap_or(0.0),
        velocity: float("VELOCITY"),
        gravity,
        acceleration: float("ACCELERATION"),
        turn_rate,
        pitch_rate: float("PITCH_RATE"),
        // // approximation: every shipped weapon has `FIRE_RATE`; one without
        // would otherwise fire on every tick.
        fire_rate: float("FIRE_RATE").unwrap_or(1.0),
        splash_radius: float("IMPACT_PROXIMITY").unwrap_or(0.0),
        impact_type: value
            .get("IMPACT_TYPE")
            .and_then(|v| v.int())
            .unwrap_or(0)
            .clamp(0, u8::MAX as i64) as u8,
        crater: pair("CRATER"),
        flyout_health: value
            .get("FLYOUT_HEALTH")
            .and_then(|v| v.int())
            .map(|health| health.max(0) as u32),
        reload_one: flag("RELOAD"),
        remote_detonate: flag("REMOTE_DETONATE"),
        tether_guided: flag("TETHER_GUIDED"),
        lock_on,
        multi_target: flag("MULTI_TARGET"),
        catches_fire: flag("CATCHES_FIRE"),
        quicksand: pair("QUICKSAND"),
        beam,
        mine,
        fire: parse_fire(value.get("FIRE")),
        flyout: parse_flyout(value.get("FLYOUT")),
        impact: parse_impact(impact_block),
        impact_always: impact_block
            .and_then(|v| v.get("ANIMATION_ALWAYS"))
            .and_then(|v| v.flag())
            .unwrap_or(false),
        heat: impact_block
            .and_then(|v| v.get("HEAT"))
            .and_then(|v| v.float_array::<6>()),
        designate: impact_block
            .and_then(|v| v.get("DESIGNATE"))
            .and_then(|v| v.float_array::<6>()),
        freeze: impact_block
            .and_then(|v| v.get("FREEZE"))
            .and_then(|v| v.float_array::<6>()),
        bounce_sound: value
            .get("BOUNCE")
            .and_then(|v| v.get("SOUND"))
            .and_then(|v| v.string())
            .map(str::to_string),
    })
}

/// Split `wep_<slot>.<variant>` into its two numbers.
///
/// Public so the same rule serves the loadout builder and the override file's
/// key check.
pub fn parse_id(id: &str) -> Option<(u8, u8)> {
    let rest = id.strip_prefix("wep_")?;
    let (slot, variant) = rest.split_once('.')?;

    Some((slot.parse().ok()?, variant.parse().ok()?))
}

/// `AMMO_LIMIT`, with the data's "unlimited" sentinel mapped to [`u32::MAX`].
fn parse_ammo_limit(value: Option<ReaderValue<'_>>) -> u32 {
    match value.and_then(|v| v.int()) {
        Some(UNLIMITED_AMMO) => u32::MAX,
        Some(limit) => limit.clamp(0, u32::MAX as i64) as u32,
        None => 0,
    }
}

/// `FIRE`.
fn parse_fire(value: Option<ReaderValue<'_>>) -> FireFx {
    let Some(value) = value else {
        return FireFx::default();
    };

    FireFx {
        animation: value
            .get("ANIMATION")
            .and_then(|v| v.string())
            .map(str::to_string),
        sound: value
            .get("SOUND")
            .and_then(|v| v.string())
            .map(str::to_string),
        random_rotate: value
            .get("RANDOM_ROTATE")
            .and_then(|v| v.flag())
            .unwrap_or(false),
    }
}

/// `FLYOUT`.
fn parse_flyout(value: Option<ReaderValue<'_>>) -> FlyoutFx {
    let Some(value) = value else {
        return FlyoutFx::default();
    };

    let text = |key: &str| value.get(key).and_then(|v| v.string()).map(str::to_string);

    FlyoutFx {
        model: text("MODEL"),
        model_animation: text("MODEL_ANIMATION"),
        animation: text("ANIMATION"),
        sound: text("SOUND"),
    }
}

/// `IMPACT`'s three surface children.
///
/// The other children — `HEAT`, `FREEZE`, `DESIGNATE`, `ANIMATION_ALWAYS` —
/// sit at the same level and are read separately by [`parse_weapon`]; they
/// are skipped here rather than mistaken for a fourth surface.
fn parse_impact(value: Option<ReaderValue<'_>>) -> HashMap<Surface, ImpactFx> {
    let mut out = HashMap::default();
    let Some(value) = value else {
        return out;
    };

    for surface in [Surface::Default, Surface::Water, Surface::Quicksand] {
        let Some(block) = value.get(surface.key()) else {
            continue;
        };
        out.insert(surface, parse_impact_fx(block));
    }

    out
}

/// One `IMPACT/<surface>` block.
fn parse_impact_fx(value: ReaderValue<'_>) -> ImpactFx {
    let text = |key: &str| value.get(key).and_then(|v| v.string()).map(str::to_string);

    ImpactFx {
        animation: text("ANIMATION"),
        kill_animation: text("KILL_ANIMATION"),
        damage_animation: text("DAMAGE_ANIMATION"),
        damage_anim_on_health: value
            .get("DAMAGE_ANIM_ON_HEALTH")
            .map(|list| {
                list.items()
                    .iter()
                    .filter_map(|entry| {
                        Some((entry.nth(0)?.float()?, entry.nth(1)?.string()?.to_string()))
                    })
                    .collect()
            })
            .unwrap_or_default(),
        effect: text("EFFECT"),
        sounds: value
            .get("SOUND")
            .map(|v| v.strings().into_iter().map(str::to_string).collect())
            .unwrap_or_default(),
        bounce_sound: text("BOUNCE_SOUND"),
    }
}

// ---------------------------------------------------------------------------
// Loadouts
// ---------------------------------------------------------------------------

/// What a vehicle is carrying, and which slot is selected.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct WeaponLoadout {
    /// Slots 1–9, at indices 0–8.
    pub slots: [SlotState; 9],
    /// The selected slot, 1–9.
    pub selected: u8,
    /// Whether the strong variant of the selected slot is up.
    pub strong: bool,
    /// Seconds left before the selected weapon may fire again.
    pub cooldown: f32,
    /// Which pulse gun the secondary trigger fires: the variant last
    /// selected in [`SECONDARY_SLOT`].
    pub secondary_strong: bool,
    /// Seconds left before the secondary trigger may fire again.
    pub secondary_cooldown: f32,
}

/// The slot the secondary trigger always fires: the pulse guns (RFPG and
/// ERFPG), whatever is selected.
///
/// // approximation: no reader field says which weapon is the secondary. The
/// pulse guns are the ones with unlimited ammo, and the only ones with a
/// pair of barrels on the turret (`RFPG_R`/`RFPG_L`), which fire whatever
/// the main gun is.
pub const SECONDARY_SLOT: u8 = 1;

impl Default for WeaponLoadout {
    /// Empty, with slot 1 selected: the slot the player's pulse gun is in.
    fn default() -> WeaponLoadout {
        WeaponLoadout {
            slots: Default::default(),
            selected: 1,
            strong: false,
            cooldown: 0.0,
            secondary_strong: false,
            secondary_cooldown: 0.0,
        }
    }
}

impl WeaponLoadout {
    /// One slot's state, by the slot number the HUD uses (1–9).
    pub fn slot(&self, slot: u8) -> Option<&SlotState> {
        self.slots.get(slot.checked_sub(1)? as usize)
    }

    /// One slot's state, mutably.
    pub fn slot_mut(&mut self, slot: u8) -> Option<&mut SlotState> {
        self.slots.get_mut(slot.checked_sub(1)? as usize)
    }

    /// The selected weapon's ammunition state.
    pub fn selected_ammo(&self) -> Option<&AmmoState> {
        self.slot(self.selected)?.variant(self.strong)
    }

    /// The selected weapon's ammunition state, mutably.
    pub fn selected_ammo_mut(&mut self) -> Option<&mut AmmoState> {
        let (selected, strong) = (self.selected, self.strong);
        self.slot_mut(selected)?.variant_mut(strong)
    }

    /// The slot and variant a trigger fires: the selection for the primary,
    /// the pulse gun for the secondary.
    ///
    /// The secondary fires the pulse gun last selected, or the other one
    /// when that has run dry. Only a mounted pulse gun fires. With the pulse gun selected, both triggers
    /// fire the same weapon.
    pub fn trigger(&self, secondary: bool) -> Option<(u8, bool)> {
        if !secondary || self.selected == SECONDARY_SLOT {
            return Some((self.selected, self.strong));
        }

        let slot = self.slot(SECONDARY_SLOT)?;
        let ready = |strong: bool| slot.variant(strong).is_some_and(AmmoState::is_ready);
        let preferred = self.secondary_strong;

        [
            (ready(preferred), preferred),
            (ready(!preferred), !preferred),
            (slot.is_mounted(preferred), preferred),
            (slot.is_mounted(!preferred), !preferred),
        ]
        .into_iter()
        .find(|(ok, _)| *ok)
        .map(|(_, strong)| (SECONDARY_SLOT, strong))
    }

    /// The ammunition a trigger fires, mutably.
    pub fn trigger_ammo_mut(&mut self, secondary: bool) -> Option<&mut AmmoState> {
        let (slot, strong) = self.trigger(secondary)?;
        self.slot_mut(slot)?.variant_mut(strong)
    }

    /// The cooldown a trigger waits on. The secondary has its own unless the
    /// pulse gun is selected, when the two triggers are one gun and share
    /// it, so holding both does not fire at twice the rate.
    pub fn trigger_cooldown_mut(&mut self, secondary: bool) -> &mut f32 {
        if secondary && self.selected != SECONDARY_SLOT {
            &mut self.secondary_cooldown
        } else {
            &mut self.cooldown
        }
    }

    /// Whether a slot has a weapon mounted, in either variant.
    ///
    /// What "empty slots are not selectable" means: mounting the weak variant
    /// makes the slot selectable even with no strong one, and ammunition for
    /// a weapon that is not mounted does not.
    pub fn is_selectable(&self, slot: u8) -> bool {
        self.slot(slot)
            .is_some_and(|state| state.is_mounted(false) || state.is_mounted(true))
    }

    /// The weapons mounted on the turret, as indices into
    /// [`WeaponTable::weapons`]: what the save carries to the next mission.
    pub fn mounted_weapons(&self) -> impl Iterator<Item = usize> + '_ {
        self.slots
            .iter()
            .flat_map(|slot| [slot.weak, slot.strong])
            .flatten()
            .filter(|state| state.mounted)
            .map(|state| state.weapon)
    }

    /// Select a slot, keeping the strong flag only if that variant exists.
    ///
    /// Returns whether anything changed, so the caller knows when to emit
    /// [`WeaponSelected`].
    pub fn select(&mut self, slot: u8) -> bool {
        if !self.is_selectable(slot) {
            return false;
        }

        // The strong variant if it is up and mounted, and whichever is
        // mounted when only one is.
        let state = self.slot(slot).copied().unwrap_or_default();
        let strong = (self.strong && state.is_mounted(true)) || !state.is_mounted(false);
        let changed = self.selected != slot || self.strong != strong;

        self.selected = slot;
        self.strong = strong;
        self.remember_pulse_gun();
        changed
    }

    /// Selecting a pulse gun makes it the secondary's too.
    fn remember_pulse_gun(&mut self) {
        if self.selected == SECONDARY_SLOT {
            self.secondary_strong = self.strong;
        }
    }

    /// Flip between the weak and strong variant of the selected slot.
    ///
    /// Returns `false` when the other variant is not owned, which leaves the
    /// selection alone rather than pointing it at an empty half-slot.
    pub fn toggle_strong(&mut self) -> bool {
        let wanted = !self.strong;
        if !self
            .slot(self.selected)
            .is_some_and(|state| state.is_mounted(wanted))
        {
            return false;
        }

        self.strong = wanted;
        self.remember_pulse_gun();
        true
    }
}

/// One HUD slot: the weak weapon, the strong one, or both.
///
/// Ammunition is **per variant**, not shared: the data gives each variant its
/// own `AMMO_LIMIT` and its own pickup kind.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SlotState {
    /// Variant 0.
    pub weak: Option<AmmoState>,
    /// Variant 1.
    pub strong: Option<AmmoState>,
}

impl SlotState {
    /// The variant's state, by the strong flag.
    pub fn variant(&self, strong: bool) -> Option<&AmmoState> {
        if strong {
            self.strong.as_ref()
        } else {
            self.weak.as_ref()
        }
    }

    /// The variant's state, mutably.
    pub fn variant_mut(&mut self, strong: bool) -> Option<&mut AmmoState> {
        if strong {
            self.strong.as_mut()
        } else {
            self.weak.as_mut()
        }
    }

    /// Whether the variant is mounted on the turret.
    pub fn is_mounted(&self, strong: bool) -> bool {
        self.variant(strong).is_some_and(|state| state.mounted)
    }

    /// Whether neither variant is owned.
    pub fn is_empty(&self) -> bool {
        self.weak.is_none() && self.strong.is_none()
    }
}

/// One owned weapon's ammunition and rate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmmoState {
    /// Index into [`WeaponTable::weapons`].
    pub weapon: usize,
    /// Rounds carried. [`u32::MAX`] is unlimited and is not decremented.
    pub ammo: u32,
    /// Seconds between shots, from this vehicle's own weapon row.
    pub interval: f32,
    /// Whether the gun itself is on the turret. Ammunition can be carried
    /// for a weapon that is not: the player picks up rounds wherever they
    /// lie, and can fire them once the weapon's own pickup mounts it.
    pub mounted: bool,
}

impl AmmoState {
    /// Whether this can be fired: mounted, with rounds left.
    pub fn is_ready(&self) -> bool {
        self.mounted && self.has_ammo()
    }

    /// Whether there is anything to fire.
    pub fn has_ammo(&self) -> bool {
        self.ammo > 0
    }

    /// Spend one round, unless the supply is unlimited.
    pub fn spend(&mut self) {
        if self.ammo != u32::MAX {
            self.ammo = self.ammo.saturating_sub(1);
        }
    }

    /// Add rounds, saturating at the weapon's own limit.
    pub fn add(&mut self, amount: u32, limit: u32) {
        if self.ammo == u32::MAX {
            return;
        }
        self.ammo = self.ammo.saturating_add(amount).min(limit);
    }
}

/// The weapons the player's turret carries from the start, by `weapons.zrd`
/// id: the RFPG alone.
///
/// // approximation: `vehicle.zrd`'s player row marks the ERFPG owned
/// (column `b`) as well, but in the game it is a mount the player finds.
/// Anything else comes from a weapon pickup, or from the save.
pub const PLAYER_STARTING_MOUNTS: [&str; 1] = ["wep_1.0"];

/// The player's loadout: every weapon in its `vehicle.zrd` rows, mounted only
/// where [`PLAYER_STARTING_MOUNTS`] or `saved` (the save's mounts, by id)
/// name it.
///
/// Every row gets a state, mounted or not, because ammunition is picked up
/// for weapons the turret does not carry yet, and each row's own `interval`
/// is the rate the player fires that weapon at. A mounted weapon starts with
/// the row's rounds, an unmounted one with none.
///
/// `full` is `--unlock-all`'s arsenal: every weapon mounted and loaded to its
/// `AMMO_LIMIT`.
pub fn player_loadout(
    def: &VehicleDef,
    table: &WeaponTable,
    saved: &[String],
    full: bool,
) -> WeaponLoadout {
    let mut loadout = WeaponLoadout::default();

    for row in &def.weapons {
        let Some(index) = table.index(&row.weapon) else {
            warn!(
                "vehicle weapon row names `{}`, which weapons.zrd has no entry for",
                row.weapon
            );
            continue;
        };
        let weapon = &table.weapons[index];
        let mounted = full
            || PLAYER_STARTING_MOUNTS.contains(&row.weapon.as_str())
            || saved.contains(&row.weapon);
        let state = AmmoState {
            weapon: index,
            ammo: match (mounted, row.ammo as i64 == UNLIMITED_AMMO) {
                (false, _) => 0,
                (true, true) => u32::MAX,
                // `ammo_limit` is already `u32::MAX` for an unlimited weapon.
                (true, false) if full => weapon.ammo_limit,
                (true, false) => row.ammo.min(weapon.ammo_limit),
            },
            interval: row.interval,
            mounted,
        };

        let Some(slot) = loadout.slot_mut(weapon.slot) else {
            continue;
        };
        match weapon.variant {
            1 => slot.strong = Some(state),
            _ => slot.weak = Some(state),
        }
    }

    if let Some(slot) = (1..=9).find(|slot| loadout.is_selectable(*slot)) {
        loadout.select(slot);
    }

    loadout
}

/// Build a loadout from a vehicle's `weapons` rows.
///
/// A row's `owned` column (`b` in READER-SCHEMA §2.1) decides whether the
/// weapon starts in the loadout; its `ammo` column is the **starting** count,
/// not the magazine — the player's `wep_2.0` row carries 3, against an
/// `AMMO_LIMIT` of 30. A row naming a weapon the table does not have is
/// skipped with a warning.
pub fn loadout_from_vehicle(def: &VehicleDef, table: &WeaponTable) -> WeaponLoadout {
    let mut loadout = WeaponLoadout::default();

    for row in &def.weapons {
        let Some(index) = table.index(&row.weapon) else {
            warn!(
                "vehicle weapon row names `{}`, which weapons.zrd has no entry for",
                row.weapon
            );
            continue;
        };
        if !row.owned {
            continue;
        }

        let weapon = &table.weapons[index];
        let state = AmmoState {
            weapon: index,
            // The row carries the same "unlimited" sentinel the weapon's own
            // `AMMO_LIMIT` does — the player's `wep_1.0` row is 123 456 789 —
            // so it goes through the same mapping rather than being carried
            // as a literal count that `spend` would decrement.
            ammo: if row.ammo as i64 == UNLIMITED_AMMO {
                u32::MAX
            } else {
                row.ammo.min(weapon.ammo_limit)
            },
            interval: row.interval,
            mounted: true,
        };

        let Some(slot) = loadout.slot_mut(weapon.slot) else {
            warn!(
                "weapon `{}` is in slot {}, which no HUD slot matches",
                weapon.id, weapon.slot
            );
            continue;
        };

        // Variant 2 — the enemy-only `wep_9.2` — has no strong/weak half, so
        // it takes the weak one: an enemy carries exactly one weapon and
        // never toggles.
        match weapon.variant {
            1 => slot.strong = Some(state),
            _ => slot.weak = Some(state),
        }
    }

    // Start on the lowest slot that holds something, so a vehicle whose only
    // weapon is in slot 3 does not start pointing at an empty slot 1.
    if let Some(slot) = (1..=9).find(|slot| loadout.is_selectable(*slot)) {
        loadout.select(slot);
    }

    loadout
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).expect("valid test JSON")
    }

    /// `wep_1.0` as the real file writes it, trimmed to the scalar keys.
    const RFPG: &str = r#"["DESC", ["MSG_DESC_RFPG"], "NAME", ["RFPG"],
        "KILL_VERB", ["MSG_KILL_VERB_DEFEATED"], "AMMO_LIMIT", [123456789],
        "DAMAGE", [1.2], "FIRE_RATE", [12.0], "IMPACT_PROXIMITY", [0.0],
        "IMPACT_TYPE", [0], "RANGE", [700.0], "VELOCITY", [120.0]]"#;

    fn weapon(id: &str, body: &str) -> WeaponDef {
        let json = parse(body);
        parse_weapon(id, ReaderValue::new(&json)).expect("the test weapon parses")
    }

    #[test]
    fn the_id_carries_the_slot_and_variant() {
        assert_eq!(parse_id("wep_2.1"), Some((2, 1)));
        assert_eq!(parse_id("wep_9.2"), Some((9, 2)));
        assert_eq!(parse_id("nonsense"), None);
        assert_eq!(parse_id("wep_2"), None);
    }

    /// The player's unlimited weapon carries 123 456 789, which does not
    /// survive an f32 — the same trap as in `vehicle.zrd`.
    #[test]
    fn unlimited_ammo_becomes_u32_max_rather_than_a_rounded_float() {
        let def = weapon("wep_1.0", RFPG);

        assert_eq!(def.ammo_limit, u32::MAX);
        assert!(def.is_unlimited());
    }

    #[test]
    fn a_plain_gun_is_a_straight_mover() {
        let def = weapon("wep_1.0", RFPG);

        assert_eq!(def.mover, Mover::Straight);
        assert_eq!(def.slot, 1);
        assert_eq!(def.variant, 0);
        assert_eq!(def.hud_name, "RFPG");
        assert_eq!(def.desc_key, "MSG_DESC_RFPG");
        assert_eq!(def.velocity, Some(120.0));
    }

    /// Every branch of the derivation, in the order that matters: the arc
    /// sabre has `BEAM` *and* `LOCK_ON`, and a mine has `MINE` *and*
    /// `GRAVITY`.
    #[test]
    fn the_mover_is_derived_from_the_flags_in_precedence_order() {
        assert_eq!(Mover::derive(true, false, false, true, 0.0), Mover::Beam);
        assert_eq!(Mover::derive(false, true, false, false, 9.8), Mover::Mine);
        assert_eq!(
            Mover::derive(false, false, true, true, 0.0),
            Mover::Tethered
        );
        assert_eq!(Mover::derive(false, false, false, true, 0.0), Mover::Homing);
        assert_eq!(
            Mover::derive(false, false, false, false, 9.8),
            Mover::Ballistic
        );
        assert_eq!(
            Mover::derive(false, false, false, false, 0.0),
            Mover::Straight
        );
    }

    /// `wep_7.1`, the arc sabre: a beam whose `LOCK_ON` is 0.0, which must
    /// not make it a homing missile.
    #[test]
    fn a_zero_lock_on_does_not_make_a_beam_home() {
        let def = weapon(
            "wep_7.1",
            r#"["NAME", ["ARC"], "BEAM", [0.0, 4.0, 0], "LOCK_ON", [0.0],
                "MULTI_TARGET", [1], "DAMAGE", [65.0], "FIRE_RATE", [1.5],
                "RANGE", [256.0], "AMMO_LIMIT", [40], "IMPACT_PROXIMITY", [0.0]]"#,
        );

        assert_eq!(def.mover, Mover::Beam);
        assert_eq!(def.beam, Some([0.0, 4.0, 0.0]));
        assert_eq!(def.lock_on, Some(0.0));
        assert!(def.multi_target);
        // A beam has no VELOCITY, and so no finite flight time.
        assert_eq!(def.velocity, None);
        assert_eq!(def.flight_time(), f32::INFINITY);
    }

    /// `wep_5.0`: a mine is stationary even though it carries `GRAVITY`.
    #[test]
    fn a_mine_stays_a_mine_despite_its_gravity() {
        let def = weapon(
            "wep_5.0",
            r#"["NAME", ["P-HEM"], "MINE", [1], "RELOAD", [1],
                "REMOTE_DETONATE", [1], "GRAVITY", [9.8], "VELOCITY", [60.0],
                "RANGE", [160.0], "IMPACT_PROXIMITY", [40.0], "DAMAGE", [36.0],
                "FIRE_RATE", [1.0], "AMMO_LIMIT", [30], "CRATER", [10, 15],
                "BOUNCE", ["SOUND", ["snd_bncehard"]]]"#,
        );

        assert_eq!(def.mover, Mover::Mine);
        assert!(def.mine);
        assert!(def.reload_one);
        assert!(def.remote_detonate);
        assert_eq!(def.crater, Some((10, 15)));
        assert_eq!(def.bounce_sound.as_deref(), Some("snd_bncehard"));
        // Mines do not expire by range: they sit until something trips them.
        assert_eq!(def.flight_time(), f32::INFINITY);
    }

    /// The `FIRE_RATE` split of READER-SCHEMA §5.2.
    #[test]
    fn fire_rate_reads_as_hertz_for_guns_and_seconds_for_launchers() {
        // The pulse gun: 12 shots a second.
        let mut def = weapon("wep_1.0", RFPG);
        assert!((def.shot_interval() - 1.0 / 12.0).abs() < 1e-6);

        // A mortar: 0.5 seconds between shots, not two shots a second.
        def.fire_rate = 0.5;
        assert_eq!(def.shot_interval(), 0.5);

        // And the SAM site's 20, the file's highest.
        def.fire_rate = 20.0;
        assert!((def.shot_interval() - 0.05).abs() < 1e-6);
    }

    /// `IMPACT`'s non-surface children must not be read as a fourth surface.
    #[test]
    fn the_special_impact_blocks_are_not_surfaces() {
        let def = weapon(
            "wep_6.0",
            r#"["NAME", ["LASERD"], "BEAM", [1.0, 10.0, 0], "DAMAGE", [140.0],
                "FIRE_RATE", [1.2], "RANGE", [1024.0], "AMMO_LIMIT", [40],
                "IMPACT_PROXIMITY", [0.0], "GRAVITY", [0.0],
                "IMPACT", ["DESIGNATE", [1.4, 2.7, 0.514, 1.0, 0.0275, 1.25],
                           "default", ["ANIMATION", ["ric_red.flt"], "SOUND", ["snd_lsimpact"]],
                           "water", ["ANIMATION", ["splash1.flt"]],
                           "quicksand", ["ANIMATION", null]]]"#,
        );

        assert_eq!(def.impact.len(), 3, "three surfaces, not four");
        assert_eq!(
            def.designate,
            Some([1.4, 2.7, 0.514, 1.0, 0.0275, 1.25]),
            "DESIGNATE is a weapon-level block"
        );
        assert_eq!(
            def.impact[&Surface::Default].sounds,
            vec!["snd_lsimpact".to_string()]
        );
        // A `quicksand` block whose only key is null is still a block.
        assert!(def.impact.contains_key(&Surface::Quicksand));
        assert_eq!(def.impact[&Surface::Quicksand].animation, None);
    }

    /// `wep_1.0`'s real `SOUND` list: three ricochets to choose between.
    #[test]
    fn an_impact_sound_may_be_one_name_or_several() {
        let def = weapon(
            "wep_1.0",
            r#"["NAME", ["RFPG"], "DAMAGE", [1.2], "FIRE_RATE", [12.0],
                "RANGE", [700.0], "VELOCITY", [120.0], "AMMO_LIMIT", [999],
                "IMPACT_PROXIMITY", [0.0],
                "IMPACT", ["default", ["KILL_ANIMATION", ["kill_rfpg"],
                    "DAMAGE_ANIM_ON_HEALTH", [[0.05, "greenparts"],
                                              [0.15, "md_impact.flt"],
                                              [1.0, "sm_impact.flt"]],
                    "SOUND", ["snd_ricsizzle1", "snd_ricsizzle2", "snd_ricsizzle3"]],
                    "water", ["SOUND", ["snd_smsplash"]]]]"#,
        );

        let default = &def.impact[&Surface::Default];
        assert_eq!(default.sounds.len(), 3);
        assert_eq!(default.kill_animation.as_deref(), Some("kill_rfpg"));
        assert_eq!(def.impact[&Surface::Water].sounds, vec!["snd_smsplash"]);

        // The thresholds are ascending, and the first one at or above the
        // victim's remaining fraction wins.
        assert_eq!(default.damage_anim_for(0.02), Some("greenparts"));
        assert_eq!(default.damage_anim_for(0.10), Some("md_impact.flt"));
        assert_eq!(default.damage_anim_for(0.90), Some("sm_impact.flt"));
    }

    /// `wep_6.1`'s `"executed"` is the one literal verb in the file.
    #[test]
    fn a_kill_verb_is_a_message_key_unless_it_is_a_word() {
        assert_eq!(
            KillVerb::parse("MSG_KILL_VERB_BLASTED"),
            KillVerb::Key("MSG_KILL_VERB_BLASTED".to_string())
        );
        assert_eq!(
            KillVerb::parse("executed"),
            KillVerb::Literal("executed".to_string())
        );
    }

    #[test]
    fn a_soil_chooses_its_impact_surface() {
        use bft_assets::materials::Soil;

        assert_eq!(Surface::from_soil(Soil::Water), Surface::Water);
        assert_eq!(Surface::from_soil(Soil::Quicksand), Surface::Quicksand);
        assert_eq!(Surface::from_soil(Soil::Grass), Surface::Default);
        // Lava has no IMPACT block in any weapon, so it is not a fourth
        // surface: it falls back to `default`.
        assert_eq!(Surface::from_soil(Soil::Lava), Surface::Default);
    }

    /// The whole file's shape, in miniature.
    fn table_json() -> Value {
        parse(
            r#"[["VERSION", [2],
                "LOCK_ON_WARNING", ["snd_lo_warn"],
                "NO_AMMO_WARNING", ["snd_ammo_out"],
                "TRIGGER_INACTIVE", ["snd_trigger_click"],
                "WEAPON_INACTIVE", ["snd_trigger_click"],
                "BALLISTICS", [
                  "wep_1.0", ["NAME", ["RFPG"], "AMMO_LIMIT", [123456789],
                              "DAMAGE", [1.2], "FIRE_RATE", [12.0],
                              "RANGE", [700.0], "VELOCITY", [120.0],
                              "IMPACT_PROXIMITY", [0.0]],
                  "wep_2.0", ["NAME", ["HEM"], "AMMO_LIMIT", [30],
                              "DAMAGE", [18.0], "FIRE_RATE", [0.5],
                              "RANGE", [1024.0], "VELOCITY", [100.0],
                              "GRAVITY", [9.8], "IMPACT_PROXIMITY", [16.0]],
                  "wep_2.1", ["NAME", ["MDM"], "AMMO_LIMIT", [15],
                              "DAMAGE", [24.0], "FIRE_RATE", [0.5],
                              "RANGE", [1024.0], "VELOCITY", [200.0],
                              "GRAVITY", [9.8], "IMPACT_PROXIMITY", [20.0]]]]]"#,
        )
    }

    fn table() -> WeaponTable {
        let json = table_json();
        let archive = bft_assets::ReaderData::from_entries(
            [(WEAPONS_ENTRY.to_string(), json)].into_iter().collect(),
        );

        build_weapon_table(
            &archive,
            &WeaponOverrides::default(),
            DEFAULT_TETHER_MAX_ALTITUDE,
        )
        .expect("the test archive builds")
    }

    #[test]
    fn the_table_keeps_file_order_and_indexes_by_id() {
        let table = table();

        assert_eq!(table.weapons.len(), 3);
        assert_eq!(table.weapons[0].id, "wep_1.0");
        assert_eq!(table.index("wep_2.1"), Some(2));
        assert_eq!(
            table.get("wep_2.0").map(|w| w.mover),
            Some(Mover::Ballistic)
        );
        assert_eq!(
            table.slot(2, true).map(|w| w.hud_name.as_str()),
            Some("MDM")
        );
        assert_eq!(table.warnings.no_ammo, "snd_ammo_out");
        assert_eq!(table.tether_max_altitude, 30.0);
    }

    #[test]
    fn an_archive_without_weapons_is_an_error_rather_than_an_empty_table() {
        let empty = bft_assets::ReaderData::default();

        assert_eq!(
            build_weapon_table(&empty, &WeaponOverrides::default(), 30.0).unwrap_err(),
            LoadError::NoWeaponsEntry
        );
    }

    #[test]
    fn a_weapons_entry_without_ballistics_is_an_error() {
        let json = parse(r#"[["VERSION", [2]]]"#);
        let archive = bft_assets::ReaderData::from_entries(
            [(WEAPONS_ENTRY.to_string(), json)].into_iter().collect(),
        );

        assert_eq!(
            build_weapon_table(&archive, &WeaponOverrides::default(), 30.0).unwrap_err(),
            LoadError::NoBallistics
        );
    }

    // -- loadouts -----------------------------------------------------------

    use crate::vehicle::tuning::VehicleWeaponRow;

    fn row(weapon: &str, owned: bool, ammo: u32, interval: f32) -> VehicleWeaponRow {
        VehicleWeaponRow {
            weapon: weapon.to_string(),
            owned,
            ammo,
            interval,
            range: 100.0,
            raw: [0.0; 8],
        }
    }

    fn vehicle_with(rows: Vec<VehicleWeaponRow>) -> VehicleDef {
        VehicleDef {
            name: String::new(),
            health: 140.0,
            shield: Some(60.0),
            modes: Default::default(),
            camback: [Vec3::ZERO; 3],
            aim_y: [0.0; 2],
            turret_rate: None,
            weapons: rows,
            ai: Default::default(),
            drops: Vec::new(),
            sounds: Default::default(),
            track_switch: None,
        }
    }

    /// The player's real situation: 18 rows, of which only the two pulse guns
    /// are owned at the start.
    #[test]
    fn only_owned_rows_enter_the_loadout() {
        let table = table();
        let def = vehicle_with(vec![
            row("wep_1.0", true, 123_456_789, 0.15),
            row("wep_2.0", false, 3, 1.2),
            row("wep_2.1", false, 2, 1.7),
        ]);

        let loadout = loadout_from_vehicle(&def, &table);

        assert!(loadout.is_selectable(1));
        assert!(!loadout.is_selectable(2), "an unowned row is not a weapon");
        assert_eq!(loadout.selected, 1);
        assert_eq!(loadout.selected_ammo().map(|a| a.ammo), Some(u32::MAX));
        assert_eq!(loadout.selected_ammo().map(|a| a.interval), Some(0.15));
    }

    /// A loadout with both pulse guns and the HE mortar, mortar selected.
    fn pulse_and_mortar(erfpg_ammo: u32) -> WeaponLoadout {
        let ammo = |weapon, ammo| AmmoState {
            weapon,
            ammo,
            interval: 0.1,
            mounted: true,
        };
        let mut loadout = WeaponLoadout::default();
        loadout.slots[0] = SlotState {
            weak: Some(ammo(0, u32::MAX)),
            strong: Some(ammo(1, erfpg_ammo)),
        };
        loadout.slots[1].weak = Some(ammo(2, 3));
        assert!(loadout.select(2));
        loadout
    }

    /// The secondary trigger fires the pulse gun last selected, whatever is
    /// selected now, and the primary fires the selection.
    #[test]
    fn the_secondary_fires_the_pulse_gun_last_selected() {
        let mut loadout = pulse_and_mortar(50);
        assert_eq!(loadout.trigger(false), Some((2, false)));
        assert_eq!(loadout.trigger(true), Some((1, false)), "RFPG by default");

        assert!(loadout.select(1));
        assert!(loadout.toggle_strong());
        assert!(loadout.select(2));
        assert_eq!(loadout.trigger(true), Some((1, true)), "ERFPG, remembered");
    }

    /// A dry ERFPG hands the secondary back to the RFPG rather than
    /// clicking empty.
    #[test]
    fn a_dry_pulse_gun_hands_the_secondary_to_the_other() {
        let mut loadout = pulse_and_mortar(0);
        loadout.secondary_strong = true;

        assert_eq!(loadout.trigger(true), Some((1, false)));
    }

    /// With the pulse gun selected both triggers are one gun, on one clock.
    #[test]
    fn a_selected_pulse_gun_shares_its_cooldown_with_the_secondary() {
        let mut loadout = pulse_and_mortar(50);
        *loadout.trigger_cooldown_mut(true) = 0.5;
        assert_eq!(loadout.cooldown, 0.0, "the mortar's clock is its own");

        assert!(loadout.select(1));
        *loadout.trigger_cooldown_mut(true) = 0.25;
        assert_eq!(loadout.cooldown, 0.25);
    }

    /// The row's ammo is the *starting* count and the weapon's `AMMO_LIMIT`
    /// the ceiling: the player starts with 3 HE mortars against a limit of 30.
    #[test]
    fn the_row_gives_the_starting_ammo_and_the_weapon_the_limit() {
        let table = table();
        let def = vehicle_with(vec![row("wep_2.0", true, 3, 1.2)]);

        let loadout = loadout_from_vehicle(&def, &table);

        assert_eq!(
            loadout.slot(2).and_then(|s| s.weak).map(|a| a.ammo),
            Some(3)
        );
        assert_eq!(table.get("wep_2.0").map(|w| w.ammo_limit), Some(30));
        // And it starts on slot 2, the only one it has.
        assert_eq!(loadout.selected, 2);
    }

    /// The player's turret carries the RFPG alone, whatever the rows' `owned`
    /// column says; every other weapon is there unmounted and empty, ready
    /// for ammunition.
    #[test]
    fn the_player_starts_with_the_rfpg_mounted_and_nothing_else() {
        let table = table();
        let def = vehicle_with(vec![
            row("wep_1.0", true, 123_456_789, 0.15),
            row("wep_2.0", false, 3, 1.2),
            row("wep_2.1", true, 2, 1.7),
        ]);

        let loadout = player_loadout(&def, &table, &[], false);

        assert_eq!(loadout.selected, 1);
        assert!(loadout.slot(1).is_some_and(|slot| slot.is_mounted(false)));
        let mortar = loadout.slot(2).copied().expect("slot 2");
        assert!(!mortar.is_mounted(false) && !mortar.is_mounted(true));
        assert_eq!(mortar.weak.map(|state| state.ammo), Some(0));
        assert_eq!(mortar.strong.map(|state| state.interval), Some(1.7));
        assert!(!loadout.is_selectable(2));
    }

    /// The save's mounts come back mounted, with the row's starting rounds.
    #[test]
    fn saved_mounts_carry_into_the_next_mission() {
        let table = table();
        let def = vehicle_with(vec![
            row("wep_1.0", true, 123_456_789, 0.15),
            row("wep_2.0", false, 3, 1.2),
            row("wep_2.1", false, 2, 1.7),
        ]);

        let mut loadout = player_loadout(&def, &table, &["wep_2.1".to_string()], false);

        assert!(loadout.select(2));
        assert!(
            loadout.strong,
            "only the MDM is mounted, so that is selected"
        );
        assert_eq!(loadout.selected_ammo().map(|state| state.ammo), Some(2));
        assert_eq!(
            loadout.mounted_weapons().collect::<Vec<_>>(),
            vec![
                table.index("wep_1.0").expect("in the table"),
                table.index("wep_2.1").expect("in the table"),
            ]
        );
    }

    /// `--unlock-all` mounts everything and fills it to the weapon's limit.
    #[test]
    fn the_full_arsenal_mounts_every_weapon_to_its_limit() {
        let table = table();
        let def = vehicle_with(vec![
            row("wep_1.0", true, 123_456_789, 0.15),
            row("wep_2.0", false, 3, 1.2),
            row("wep_2.1", false, 2, 1.7),
        ]);

        let loadout = player_loadout(&def, &table, &[], true);

        let pulse = loadout.slot(1).and_then(|slot| slot.weak).expect("RFPG");
        assert!(pulse.mounted);
        assert_eq!(pulse.ammo, u32::MAX, "still unlimited");
        for (strong, id) in [(false, "wep_2.0"), (true, "wep_2.1")] {
            let state = loadout
                .slot(2)
                .and_then(|slot| slot.variant(strong).copied())
                .expect("a mortar");
            assert!(state.mounted, "{id} is mounted");
            assert_eq!(
                Some(state.ammo),
                table.get(id).map(|weapon| weapon.ammo_limit),
                "{id} is full"
            );
        }
    }

    /// Weak and strong keep separate counts: the data gives each its own
    /// `AMMO_LIMIT` and its own pickup kind.
    #[test]
    fn the_two_variants_of_a_slot_do_not_share_ammo() {
        let table = table();
        let def = vehicle_with(vec![
            row("wep_2.0", true, 3, 1.2),
            row("wep_2.1", true, 2, 1.7),
        ]);

        let mut loadout = loadout_from_vehicle(&def, &table);

        assert!(loadout.toggle_strong());
        assert_eq!(loadout.selected_ammo().map(|a| a.ammo), Some(2));
        loadout
            .selected_ammo_mut()
            .expect("the strong variant")
            .spend();
        assert_eq!(loadout.selected_ammo().map(|a| a.ammo), Some(1));

        assert!(loadout.toggle_strong());
        assert_eq!(
            loadout.selected_ammo().map(|a| a.ammo),
            Some(3),
            "untouched"
        );
    }

    #[test]
    fn an_empty_slot_is_not_selectable_and_the_selection_survives() {
        let table = table();
        let def = vehicle_with(vec![row("wep_1.0", true, 100, 0.15)]);

        let mut loadout = loadout_from_vehicle(&def, &table);

        assert!(!loadout.select(5), "slot 5 holds nothing");
        assert_eq!(loadout.selected, 1, "the old selection stands");
    }

    /// Toggling into a variant that is not owned must not point the selection
    /// at nothing.
    #[test]
    fn toggling_to_an_unowned_variant_is_refused() {
        let table = table();
        let def = vehicle_with(vec![row("wep_2.0", true, 3, 1.2)]);

        let mut loadout = loadout_from_vehicle(&def, &table);

        assert!(!loadout.toggle_strong());
        assert!(!loadout.strong);
        assert_eq!(loadout.selected_ammo().map(|a| a.ammo), Some(3));
    }

    /// Selecting a slot whose strong half is missing drops the strong flag
    /// rather than selecting an empty half.
    #[test]
    fn selecting_a_slot_without_a_strong_half_clears_the_flag() {
        let table = table();
        let def = vehicle_with(vec![
            row("wep_1.0", true, 100, 0.15),
            row("wep_2.0", true, 3, 1.2),
            row("wep_2.1", true, 2, 1.7),
        ]);

        let mut loadout = loadout_from_vehicle(&def, &table);
        loadout.select(2);
        assert!(loadout.toggle_strong());
        assert!(loadout.strong);

        loadout.select(1);
        assert!(!loadout.strong, "slot 1 has no strong variant here");
        assert!(loadout.selected_ammo().is_some());
    }

    #[test]
    fn unlimited_ammo_is_never_spent() {
        let mut state = AmmoState {
            weapon: 0,
            ammo: u32::MAX,
            interval: 0.15,
            mounted: true,
        };

        state.spend();
        assert_eq!(state.ammo, u32::MAX);
        state.add(50, u32::MAX);
        assert_eq!(state.ammo, u32::MAX);
    }

    #[test]
    fn picked_up_ammo_saturates_at_the_weapons_limit() {
        let mut state = AmmoState {
            weapon: 0,
            ammo: 28,
            interval: 1.2,
            mounted: true,
        };

        state.add(30, 30);
        assert_eq!(state.ammo, 30);
    }
}
