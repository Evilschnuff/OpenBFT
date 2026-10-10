//! The resolved vehicle tuning table: `vehicle.zrd` and `player.zrd` values
//! with `assets/tuning/vehicle.ron` overrides applied.
//!
//! # Where every number comes from
//!
//! Primary is the reader (DESIGN §5.3, READER-SCHEMA §2.1/§2.2). Difficulty
//! selects a whole file — `vehicle_easy.zrd`, `vehicle.zrd`,
//! `vehicle_hard.zrd` — rather than scaling anything at runtime. The RON file
//! is an override layer in which every leaf is optional, so an empty file is
//! the correct default.
//!
//! A handful of constants our raycast controller needs have **no reader
//! equivalent** — spring stiffness, engine force, a lateral-grip coefficient.
//! Those are derived from reader values by [`ModeTuning::derive`], every one
//! marked `// approximation` with the field it came from, and are overridable
//! per kind and form in the RON file. Re-tuning happens there, never in code.
//!
//! # What the data actually looks like
//!
//! Measured over the 25 kinds and 28 mode blocks of the shipped
//! `vehicle.zrd` (docs/data/reader.md):
//!
//! - **`platform`, `collision`, `friction`, `chas_*` are on 5 of 28 mode
//!   blocks**, not all of them: only the player's four modes and `stealth`'s
//!   `basic_mode` carry a suspension quad and a collision hull. Enemy modes
//!   have `rates`, `turns`, damping, `mass` and the soil slowdowns and
//!   nothing else, so those fields are `Option` and the derivation falls back
//!   to a default hull.
//! - **`mode_alt` is spelled `mode_alts` on 3 blocks** (`rumv`, `rumv_easy`,
//!   `rumv_2`, all `track_mode`). Both spellings are read.
//! - **`stealth` has a `basic_mode`**, a fifth mode key
//!   [`VehicleForm`] does not model. It is the intro/multiplayer camera
//!   stand-in, never driven, so its mode block is skipped with a `debug!` and
//!   the kind keeps its common values.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bft_assets::ReaderValue;

use super::VehicleForm;
use super::overrides::VehicleOverrides;

/// Which reader files a difficulty selects.
///
/// Difficulty picks a whole file (`vehicle_easy.zrd`), it does not scale
/// values at runtime (READER-SCHEMA §1).
// Saved with the player's progress (`Progress`), so it needs
// derives rather than a hand-rolled mapping in the save code.
#[derive(
    Resource, Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize,
)]
pub enum Difficulty {
    Easy,
    #[default]
    Normal,
    Hard,
}

impl Difficulty {
    /// The `vehicle*.zrd` entry this difficulty reads.
    pub fn vehicle_entry(&self) -> &'static str {
        match self {
            Difficulty::Easy => "vehicle_easy.zrd",
            Difficulty::Normal => "vehicle.zrd",
            Difficulty::Hard => "vehicle_hard.zrd",
        }
    }
}

/// A `vehicle.zrd` top-level key: `"bft"`, `"scou"`, `"rumv_easy"`, ….
#[derive(Component, Clone, PartialEq, Eq, Hash, Debug)]
pub struct VehicleKindId(pub String);

impl VehicleKindId {
    /// The player's kind.
    pub fn player() -> VehicleKindId {
        VehicleKindId(PLAYER_KIND.to_string())
    }

    /// The key as a string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The player vehicle's `vehicle.zrd` key.
pub const PLAYER_KIND: &str = "bft";

/// What [`build_vehicle_table`] could not do.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum TableError {
    /// The archive has no `vehicle*.zrd` for this difficulty.
    #[error("the reader archive has no `{0}` entry")]
    NoVehicleEntry(&'static str),
    /// `player.zrd` is missing, so there is no gravity to play with.
    #[error("the reader archive has no `player.zrd` entry")]
    NoPlayerEntry,
    /// The player's own kind is absent: every shipped archive has it, so this
    /// is a different file than the one this code understands.
    #[error("`{0}` has no `{PLAYER_KIND}` vehicle")]
    NoPlayerVehicle(&'static str),
}

/// Everything the vehicle controller reads, resolved once per level load.
#[derive(Resource, Debug, Clone)]
pub struct VehicleTable {
    /// Every `vehicle.zrd` kind, including the 23 enemy ones the AI uses.
    pub kinds: HashMap<VehicleKindId, VehicleDef>,
    /// `player.zrd`: gravity, the slope limit and the camera constants.
    pub player: PlayerConstants,
}

impl VehicleTable {
    /// One kind's definition.
    pub fn kind(&self, id: &VehicleKindId) -> Option<&VehicleDef> {
        self.kinds.get(id)
    }

    /// The player's definition, which every shipped archive has.
    pub fn player_def(&self) -> Option<&VehicleDef> {
        self.kinds.get(&VehicleKindId::player())
    }
}

/// One `vehicle.zrd` kind: the `common_mode` block plus its mode blocks.
#[derive(Debug, Clone)]
pub struct VehicleDef {
    /// `name`, joined from the reader's word list ("Sub Tank"). Empty for the
    /// player, whose entry carries no name.
    pub name: String,
    /// `health[0]`: hit points.
    pub health: f32,
    /// `health[1]`, where the entry has two: the player's second bar.
    pub shield: Option<f32>,
    /// One entry per `<form>_mode` block the kind has.
    pub modes: HashMap<VehicleForm, ModeTuning>,
    /// `camback`: three chase-camera offsets in hull space, one per camera
    /// mode.
    pub camback: [Vec3; 3],
    /// `aimy`: turret aim height(s) above the hull origin.
    pub aim_y: [f32; 2],
    /// How fast the turret traverses and elevates, rad/s.
    ///
    /// No reader field: `vehicle.zrd` states the hull's `turns` and nothing
    /// says how fast a barrel moves. `None` takes
    /// [`TurretRate`](crate::vehicle::TurretRate)'s default; `vehicle.ron`
    /// can set it per kind.
    pub turret_rate: Option<[f32; 2]>,
    /// `weapons`: the 9-column rows, for the weapons.
    pub weapons: Vec<VehicleWeaponRow>,
    /// The AI ranges, for the AI.
    pub ai: VehicleAiDefaults,
    /// `pickups` and `nanite`: what this kind drops when destroyed.
    pub drops: Vec<DropDef>,
    /// `sounds`: the common (not per-mode) sound names, for the sound layer.
    pub sounds: VehicleSounds,
    /// `track_switch`: the three speeds at which the player's tread texture
    /// animation changes stage (`CycledTexture`).
    pub track_switch: Option<[f32; 3]>,
}

impl VehicleDef {
    /// The tuning for one form, or `None` when the kind has no such mode.
    pub fn mode(&self, form: VehicleForm) -> Option<&ModeTuning> {
        self.modes.get(&form)
    }

    /// The forms this kind can be in, which is which mode blocks it has.
    pub fn forms(&self) -> impl Iterator<Item = VehicleForm> + '_ {
        self.modes.keys().copied()
    }
}

/// One `<form>_mode` block, plus the controller constants derived from it.
#[derive(Debug, Clone, PartialEq)]
pub struct ModeTuning {
    /// `platform`: the four hull-space corners of the suspension quad. Only
    /// the player's modes and `stealth` carry one (5 of 28 blocks).
    pub platform: Option<[Vec3; 4]>,
    /// `collision`: the 12 hull-space collision hull points. Same 5 blocks.
    pub collision: Vec<Vec3>,
    /// `mode_alt`/`mode_alts`: ride-height offset — track 0, amphib −0.1,
    /// hover 0.5, sub −0.5.
    pub mode_alt: f32,
    /// `rates`: `[a, b]`. See [`Self::max_speed`] for which is which.
    pub rates: [f32; 2],
    /// `turns`: turn rate stationary and moving, rad/s.
    pub turns: [f32; 2],
    /// `turn_damping`: yaw damping constant.
    pub turn_damping: f32,
    /// `rate_damping`: speed damping constants.
    pub rate_damping: [f32; 2],
    /// `friction`: longitudinal, lateral, rolling.
    pub friction: Option<[f32; 3]>,
    /// `mass`, in the reader's "tonnes" (2.2–2.5).
    pub mass: Option<f32>,
    /// `a_damping`: angular damping.
    pub a_damping: Option<f32>,
    /// `chas_smooth`: visual chassis smoothing.
    pub chas_smooth: Option<f32>,
    /// `chas_pitch`: chassis pitch response (gain, per-speed, clamp).
    pub chas_pitch: Option<[f32; 3]>,
    /// `chas_roll`: chassis roll response.
    pub chas_roll: Option<[f32; 3]>,
    /// `collision_d`: collision damage factor and threshold.
    pub collision_d: [f32; 2],
    /// `quicksand_slowdown`: speed multiplier on `Soil::Quicksand`.
    pub quicksand_slowdown: Option<f32>,
    /// `lava_slowdown`: speed multiplier on `Soil::Lava`.
    pub lava_slowdown: Option<f32>,
    /// `ok_to_amphib`: whether this mode may transform to amphib.
    pub ok_to_amphib: bool,
    /// `alt_control`: hover altitude controller gains.
    pub alt_control: Option<[f32; 3]>,
    /// `gun_pitch`: turret pitch limits in radians, where the mode has them.
    pub gun_pitch: Option<[f32; 2]>,
    /// `hover_wave`/`amphib_wave`/`sub_wave`: bobbing oscillation parameters.
    pub wave: Option<[f32; 7]>,
    /// `t2a_anims` … `s2a_anims`: the ("leave old", "enter new") anim pair
    /// for each transition, keyed by (from, to). Only the player has them.
    pub transitions: HashMap<(VehicleForm, VehicleForm), [String; 2]>,
    /// `sounds`: this mode's engine, skid, collide and land sounds.
    pub sounds: ModeSounds,

    // Derived. Not in the data; see `derive` for the mapping, and override
    // them per kind and form in `assets/tuning/vehicle.ron`.
    /// Suspension ray origins and spring constants.
    pub suspension: SuspensionTuning,
    /// Forward engine force, in newtons.
    pub engine_force: f32,
    /// Top forward speed, m/s.
    pub max_speed: f32,
    /// Top reverse speed, m/s.
    pub reverse_speed: f32,
    /// How much lateral force the contact patch can carry **while sliding**,
    /// from `friction[1]`. The lower of the two coefficients.
    pub lateral_grip: f32,
    /// How much it can carry **before breaking loose**, from `friction[0]`.
    /// Always the higher of the two; see
    /// [`lateral_impulse`](super::lateral_impulse) for why the data's
    /// `friction` reads as static and kinetic rather than longitudinal and
    /// lateral.
    pub static_grip: f32,
    /// How much control remains off the ground, 0..1.
    pub air_control: f32,
    /// Turns [`Self::static_grip`] and [`Self::lateral_grip`] into a lateral
    /// force per unit mass. [`GRIP_FORCE_SCALE`](super::GRIP_FORCE_SCALE) by
    /// default.
    pub grip_force_scale: f32,
    /// How far a slide has to decay, as a fraction of the speed that broke
    /// it, before the tracks bite again.
    /// [`REGRIP_FRACTION`](super::REGRIP_FRACTION) by default.
    pub regrip_fraction: f32,
    /// Below this forward speed, m/s, the hull turns at `turns[0]` rather
    /// than `turns[1]`. [`STATIONARY_SPEED`](super::STATIONARY_SPEED) by
    /// default.
    pub stationary_speed: f32,
    /// How fast the hull rights itself toward world up with nothing under
    /// it, in rad/s (the spring's natural frequency). [`UPRIGHT_RATE_AIR`]
    /// by default.
    pub upright_rate_air: f32,
    /// The same toward the ground's normal, with the suspension on it.
    /// [`UPRIGHT_RATE_GROUND`] by default.
    pub upright_rate_ground: f32,
    /// The furthest the hull may lean from world up, in radians, however it
    /// is hit. [`MAX_TILT`] by default.
    pub max_tilt: f32,
}

/// The raycast suspension's own parameters, all derived.
#[derive(Debug, Clone, PartialEq)]
pub struct SuspensionTuning {
    /// Ray origins in hull space: the `platform` corners where there are
    /// some, a default quad where there are not.
    ///
    /// These are at the **contact patch** (`platform` is at y = 0), which is
    /// where the spring pushes. The cast itself starts [`Self::ray_lift`]
    /// above them; see that field for why.
    pub rays: Vec<Vec3>,
    /// How far above each ray origin the cast actually starts, in metres.
    ///
    /// The rays describe the contact patch, which sits below the collider
    /// box — but a ray *cast* from there can end up underground. On a hard
    /// landing the solver stops the box at the terrain and the hull keeps
    /// going until it does, which puts the contact patch below the surface;
    /// a downward ray from there finds nothing, the spring reads as fully
    /// extended, and the hull sits dead on its collider with no suspension
    /// at all.
    ///
    /// So the cast is lifted to the collider's floor, which the solver
    /// guarantees is never below the terrain. [`Self::max_len`] includes the
    /// lift, so the reach below the hull and the compression at any given
    /// height are both unchanged — only the origin moves out of harm's way.
    pub ray_lift: f32,
    /// Rest length: how far the hull floats above the ground.
    pub rest: f32,
    /// Spring constant, N/m per ray.
    pub stiffness: f32,
    /// Damper constant, Ns/m per ray.
    pub damping: f32,
    /// How far a ray looks before it reports "nothing under me".
    pub max_len: f32,
}

/// One `weapons` row: `[weapon_id, a, b, ammo, interval, c, d, e, f]`.
///
/// `b` is "initially owned" and `d` is an engagement range; `a`, `c`, `e`,
/// `f` are unknown and kept in [`Self::raw`] (READER-SCHEMA §2.1).
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleWeaponRow {
    pub weapon: String,
    pub owned: bool,
    /// Rounds. Read as an integer rather than through [`Self::raw`]: the
    /// player's unlimited weapon carries 123 456 789, which does not survive
    /// an f32.
    pub ammo: u32,
    pub interval: f32,
    pub range: f32,
    /// The whole row's numeric columns, so nothing is lost while their
    /// meaning is unsettled.
    pub raw: [f32; 8],
}

/// The AI ranges of `common_mode`, consumed by the AI.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VehicleAiDefaults {
    pub activation: Option<f32>,
    pub pursuit_range: Option<[f32; 2]>,
    pub not_pursuit_dwell: Option<f32>,
    pub return_range: Option<f32>,
}

/// One death drop: a `pickup.zrd` key, or `"NANITE"` for the `nanite` pair.
#[derive(Debug, Clone, PartialEq)]
pub struct DropDef {
    pub kind: String,
    pub amount: u32,
}

/// The `common_mode` `sounds` block: the player's, and nobody else's.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VehicleSounds {
    pub skid: Option<String>,
    pub weapon_up: Option<String>,
    pub weapon_down: Option<String>,
    pub weapon_select: Option<String>,
    pub pinging: Option<String>,
}

/// A mode block's `sounds`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModeSounds {
    pub engine: Option<String>,
    pub skid: Option<String>,
    pub collide: Option<String>,
    pub land: Option<String>,
    pub pitch_scale: Option<f32>,
    pub volume_scale: Option<f32>,
}

/// `player.zrd`: the player's physics and camera constants.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerConstants {
    /// `nom_gravity`: **40 m/s²**, not 9.81 (READER-SCHEMA §2.2).
    pub gravity: f32,
    /// `wat_gravity`: 9.
    pub water_gravity: f32,
    /// `qsd_gravity`: 3.
    pub quicksand_gravity: f32,
    /// `qsand_sink`: how fast a body sinks in quicksand.
    pub quicksand_sink: f32,
    /// `lava_sink`.
    pub lava_sink: f32,
    /// `max_slope`: the cosine of the steepest climbable slope (0.8192 ≈ 35°).
    pub max_slope_cos: f32,
    /// The chase and first-person camera constants.
    pub camera: CameraConstants,
    /// `burning_anim`: the anim name and its duration.
    pub burning_anim: (String, f32),
    /// `low_shield_snd`: warning sound, threshold, alarm.
    pub low_shield: (String, f32, String),
}

impl Default for PlayerConstants {
    /// The shipped `player.zrd` values, so a table built without a reader is
    /// still the game's numbers rather than invented ones.
    fn default() -> PlayerConstants {
        PlayerConstants {
            gravity: 40.0,
            water_gravity: 9.0,
            quicksand_gravity: 3.0,
            quicksand_sink: 0.4,
            lava_sink: 0.3,
            max_slope_cos: 0.8192,
            camera: CameraConstants::default(),
            burning_anim: ("burning_model".to_string(), 4.0),
            low_shield: ("snd_lowshield".to_string(), 10.0, "snd_warn3".to_string()),
        }
    }
}

/// `player.zrd`'s camera keys.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraConstants {
    /// `camera_zone`: the dead-zone fraction before the camera follows the
    /// turret.
    pub zone: f32,
    /// `max_cam_yaw_rate`, rad/s.
    pub max_yaw_rate: f32,
    /// `mouse_push`: sensitivity, fine and coarse.
    pub mouse_push: [f32; 2],
    /// `camera_elastic`: the chase spring.
    pub elastic: f32,
    /// `max_cam_tether_angle`, degrees.
    pub max_tether_angle_deg: f32,
    /// `underwater_cam`: fog density, near, far, RGB, tint.
    pub underwater: [f32; 8],
    /// `fp_cam_el_rate`: first-person elevation rate.
    pub fp_el_rate: f32,
    /// `fp_cam_el_lim`: first-person elevation limits, radians.
    pub fp_el_lim: [f32; 2],
}

impl Default for CameraConstants {
    /// The shipped `player.zrd` values.
    fn default() -> CameraConstants {
        CameraConstants {
            zone: 0.7,
            max_yaw_rate: 0.87,
            mouse_push: [0.002, 0.01],
            elastic: 0.04,
            max_tether_angle_deg: 20.0,
            underwater: [0.2, 20.0, 340.0, 104.0, 30.0, 85.0, 115.0, 0.3],
            fp_el_rate: 3.5,
            fp_el_lim: [-0.6, 0.5],
        }
    }
}

// ---------------------------------------------------------------------------
// Derived constants
// ---------------------------------------------------------------------------

/// The suspension quad a mode with no `platform` gets.
///
/// // approximation: the player's `track_mode` `platform`, ±2.25 × −2.4…2.34,
/// which matches the 5.7 × 7.4 m model (docs/data/coordinates.md). Enemy modes carry no
/// quad at all (23 of 28 blocks), and every enemy kind is a vehicle of
/// roughly the player's size, so borrowing the player's is closer than
/// inventing one.
pub const DEFAULT_PLATFORM: [Vec3; 4] = [
    Vec3::new(-2.25, 0.0, -2.4),
    Vec3::new(2.25, 0.0, -2.4),
    Vec3::new(2.25, 0.0, 2.34),
    Vec3::new(-2.25, 0.0, 2.34),
];

/// The collision hull a mode with no `collision` gets: the platform quad
/// raised to the hull height the data uses (`y = 1` on every real point).
///
/// // approximation: from `DEFAULT_PLATFORM` and the `collision` y of 1.0.
pub const DEFAULT_HULL_HEIGHT: f32 = 1.0;

/// The spring's length at rest below the hull origin, in metres; `mode_alt`
/// is added on top of this.
///
/// **The hull origin is the track contact patch**: `bft_00`, which hangs off
/// the hull at the identity, has its tracks at y = 0. At 1.0 — "the
/// `collision` ring sits a metre up" — the springs held the *origin* a metre
/// up, plus the 0.6 m of travel `SUSPENSION_STIFFNESS_FACTOR` leaves in hand:
/// the drawn BFT rode 1.6 m off the ground, its hull box 2.6–5.4 m up, and
/// it could not pass under the landed VTOL's door frame (measured). At 0 the origin settles about 0.2 m up on
/// [`SUSPENSION_TRAVEL`]; the ring's height is
/// [`HULL_FLOOR_Y`](super::HULL_FLOOR_Y)'s business, not the spring's.
pub const SUSPENSION_REST: f32 = 0.0;

/// How far past the rest length a suspension ray still looks, in metres.
///
/// // approximation: no reader field corresponds. Short and stiff, because
/// the hull rides on its tracks: [`SUSPENSION_STIFFNESS_FACTOR`] leaves half
/// of it in hand, so the tracks' collider
/// ([`track_collider`](super::track_collider)) settles 0.2 m off the ground
/// and takes anything the springs cannot.
///
/// It was 1.2 m, for m1's shoreline. That much soft travel held the hull
/// 0.6 m up — so ledges up to 1.5 m passed under the hull floor and the
/// springs lifted the tank onto them — and soaked up a crater's far wall
/// rather than throwing the tank off it.
pub const SUSPENSION_TRAVEL: f32 = 0.4;

/// How far above the contact patch each suspension ray is cast from.
///
/// Equal to the collider box's floor in hull space, so the whole ray lies
/// inside or above the collider and its origin can never be pushed below the
/// terrain. See [`SuspensionTuning::ray_lift`] for the failure this avoids.
///
/// Not an approximation: it is the box floor, and the two have to agree or
/// the ray starts outside the collider again.
///
/// This does not make the cast immune to tunnelling in principle — a body
/// moving more than the ray's whole length in one tick would still skip it —
/// but that needs about 190 m/s, which is a 460 m free fall at the reader's
/// 40 m/s² and not reachable from anything the player can drive off. The
/// collider stops the hull either way; the failure would be a tick without
/// suspension, not a fall through the world. A shape cast would close even
/// that, at a cost this does not justify.
pub const SUSPENSION_RAY_LIFT: f32 = super::HULL_FLOOR_Y;

/// Spring stiffness per ray, as a multiple of the force needed to hold the
/// vehicle's share of its weight at full compression.
///
/// // approximation: derived from `mass` and `player.zrd` `nom_gravity` —
/// `mass * gravity / (rays * travel)` is the stiffness that balances the
/// weight with the hull exactly at its rest height, and this factor above 1
/// leaves travel in hand so a bump compresses the spring rather than
/// bottoming the hull out. No reader field corresponds; `friction` is a
/// different quantity.
pub const SUSPENSION_STIFFNESS_FACTOR: f32 = 2.0;

/// Damping as a fraction of critical damping for the sprung mass.
///
/// // approximation: 0.5 of critical is the usual arcade compromise between
/// a bouncy hull and a dead one. No reader field corresponds.
pub const SUSPENSION_DAMPING_RATIO: f32 = 0.5;

/// How many seconds the engine takes to reach top speed from rest.
///
/// // approximation: `rates[0]` is read as top speed and `rate_damping[0]`
/// as a damping constant, neither of which states an acceleration, so the
/// engine force is sized from a time-to-top-speed instead. Two seconds to 48
/// m/s is the original's "strong engines" feel (READER-SCHEMA §2.2).
pub const TIME_TO_TOP_SPEED: f32 = 2.0;

/// The default mass, in the reader's tonnes, for a mode with no `mass`.
///
/// // approximation: the player's `track_mode` `mass`, which 23 of 28 blocks
/// also carry; the 5 without are the player's non-track modes.
pub const DEFAULT_MASS: f32 = 2.2;

/// What a reader `mass` of 2.2 weighs in kilograms.
///
/// // approximation: READER-SCHEMA reads `mass` as "tonnes" with a question
/// mark. 2.2 t for a 5.7 × 7.4 m vehicle is light for a tank and right for
/// the hovercraft the model is, so the unit is taken at face value.
pub const MASS_UNIT_KG: f32 = 1000.0;

/// How much of the sideways velocity a grip of 1.0 cancels per second.
///
/// // approximation: derived from `friction[1]` (lateral friction, 40–200 in
/// the data) divided by this. A track mode's 80 becomes 8 — a hard grip that
/// kills a slide in about an eighth of a second — and hover's 40 becomes 4.
pub const LATERAL_GRIP_DIVISOR: f32 = 10.0;

/// The sliding (kinetic) grip a mode with no `friction` gets.
///
/// // approximation: the player's `track_mode` `friction[1]` of 80 through
/// [`LATERAL_GRIP_DIVISOR`].
pub const DEFAULT_LATERAL_GRIP: f32 = 8.0;

/// The breaking-loose (static) grip a mode with no `friction` gets.
///
/// // approximation: the player's `track_mode` `friction[0]` of 150 through
/// [`LATERAL_GRIP_DIVISOR`]. Higher than [`DEFAULT_LATERAL_GRIP`], as static
/// friction always is.
pub const DEFAULT_STATIC_GRIP: f32 = 15.0;

/// How much steering and drive authority remains with no wheel on the ground.
///
/// // approximation: no reader field corresponds. Enough to aim a jump,
/// not enough to fly.
pub const AIR_CONTROL: f32 = 0.25;

/// How fast a hull in the air swings back to upright, in rad/s.
///
/// // approximation: no reader field corresponds. The original pulls the
/// tank level within about a second of leaving the ground (footage), and a
/// critically damped spring at 4 rad/s settles in about that.
pub const UPRIGHT_RATE_AIR: f32 = 4.0;

/// How fast a hull on the ground swings toward the ground's normal, in rad/s.
///
/// // approximation: no reader field corresponds. Weaker than
/// [`UPRIGHT_RATE_AIR`], because the springs already follow the ground; this
/// only takes out the roll a collision puts in.
pub const UPRIGHT_RATE_GROUND: f32 = 2.0;

/// The furthest a hull may lean from world up, in radians: 60°.
///
/// // approximation: no reader field corresponds. The original never puts
/// a vehicle on its back, and some of its slopes the player has to climb are
/// close to 45°, so the limit sits well above those and well short of
/// the 90° where a tank is on its side.
pub const MAX_TILT: f32 = std::f32::consts::FRAC_PI_3;

impl ModeTuning {
    /// Fill the derived constants from the reader values already parsed.
    ///
    /// Called by the loader after parsing and before the RON overrides, so an
    /// override always wins over a derivation. Public so a test that builds a
    /// [`ModeTuning`] by hand derives the same constants the loader would
    /// rather than a second copy of the arithmetic.
    pub fn derive(&mut self) {
        let mass = self.mass.unwrap_or(DEFAULT_MASS) * MASS_UNIT_KG;
        let rays: Vec<Vec3> = self
            .platform
            .unwrap_or(DEFAULT_PLATFORM)
            .into_iter()
            .collect();
        let ray_count = rays.len().max(1) as f32;

        // `rates[0]` as top speed and `rates[1]` as reverse is the reading
        // READER-SCHEMA §2.1 says to take until driving settles it. It is
        // also the only one that is self-consistent: the player's track
        // `[48, 65]` against the sub's `[25, 55]` makes the sub slower
        // forward and nearly as fast backwards under the other reading,
        // which no vehicle behaves like.
        self.max_speed = self.rates[0];
        self.reverse_speed = self.rates[1];

        // f = m * a, with `a` sized to reach top speed in
        // `TIME_TO_TOP_SPEED` seconds.
        self.engine_force = mass * self.max_speed / TIME_TO_TOP_SPEED;

        let rest = SUSPENSION_REST + self.mode_alt;
        // The spring is fully extended at `max_len` and compressed by the
        // whole travel when the hull is at `rest`, which is where it has to
        // hold the vehicle's weight: k * travel = m * g / rays. The factor
        // above 1 is headroom, so the hull rides a little higher than `rest`
        // and has travel left to absorb a bump rather than bottoming out.
        let stiffness =
            SUSPENSION_STIFFNESS_FACTOR * mass * DERIVE_GRAVITY / (ray_count * SUSPENSION_TRAVEL);
        // Critical damping for the mass one ray carries.
        let critical = 2.0 * (stiffness * mass / ray_count).sqrt();

        // The cast starts at the collider's floor rather than at the contact
        // patch, and reaches the same distance below the hull for it.
        let lift = SUSPENSION_RAY_LIFT;

        self.suspension = SuspensionTuning {
            rays,
            ray_lift: lift,
            rest,
            stiffness,
            damping: SUSPENSION_DAMPING_RATIO * critical,
            max_len: lift + rest + SUSPENSION_TRAVEL,
        };

        (self.static_grip, self.lateral_grip) = match self.friction {
            Some(friction) => (
                friction[0] / LATERAL_GRIP_DIVISOR,
                friction[1] / LATERAL_GRIP_DIVISOR,
            ),
            None => (DEFAULT_STATIC_GRIP, DEFAULT_LATERAL_GRIP),
        };
        self.air_control = AIR_CONTROL;
        self.grip_force_scale = super::GRIP_FORCE_SCALE;
        self.regrip_fraction = super::REGRIP_FRACTION;
        self.stationary_speed = super::STATIONARY_SPEED;
        self.upright_rate_air = UPRIGHT_RATE_AIR;
        self.upright_rate_ground = UPRIGHT_RATE_GROUND;
        self.max_tilt = MAX_TILT;
    }
}

/// The gravity the suspension is sized against.
///
/// `player.zrd`'s `nom_gravity`, hard-coded here rather than threaded through
/// because a spring is sized once at load and the vehicle's *actual* gravity
/// changes with the soil it is over (water, quicksand). Sizing against the
/// nominal value and letting the water regime push the hull down is what
/// makes a tank sink in water instead of floating on a stiffer spring.
const DERIVE_GRAVITY: f32 = 40.0;

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Build the resolved table from a reader archive and the RON overrides.
///
/// `reader` is the global [`bft_assets::ReaderData`].
pub fn build_vehicle_table(
    reader: &bft_assets::ReaderData,
    difficulty: Difficulty,
    overrides: &VehicleOverrides,
) -> Result<VehicleTable, TableError> {
    let entry = difficulty.vehicle_entry();
    let vehicles = reader
        .read(entry)
        .ok_or(TableError::NoVehicleEntry(entry))?;
    let player = reader.read("player.zrd").ok_or(TableError::NoPlayerEntry)?;

    let mut kinds = HashMap::default();

    for (name, value) in vehicles.pairs() {
        let Some(def) = parse_vehicle(name, value) else {
            warn!("{entry}: vehicle `{name}` is not the shape this reads; skipping it");
            continue;
        };
        kinds.insert(VehicleKindId(name.to_string()), def);
    }

    if !kinds.contains_key(&VehicleKindId::player()) {
        return Err(TableError::NoPlayerVehicle(entry));
    }

    let mut table = VehicleTable {
        kinds,
        player: parse_player_constants(player),
    };

    overrides.apply(&mut table);

    Ok(table)
}

/// Parse one `vehicle.zrd` kind.
///
/// `None` only when there is no `common_mode`, which no shipped entry lacks.
fn parse_vehicle(name: &str, value: ReaderValue<'_>) -> Option<VehicleDef> {
    let common = value.get("common_mode")?;

    let health = common.get("health").map(|v| v.floats()).unwrap_or_default();
    let camback = common
        .get("camback")
        .and_then(|v| vec3_array::<3>(v))
        // // approximation: 24 of 25 kinds have `camback`; the one without
        // is `stealth`, which is never chased. The player's first offset
        // stands in so a camera always has somewhere to be.
        .unwrap_or([
            Vec3::new(0.0, 4.0, 12.75),
            Vec3::new(0.0, 3.5, 2.25),
            Vec3::new(0.0, 3.5, 2.25),
        ]);

    let mut modes = HashMap::default();

    for (key, block) in value.pairs() {
        let Some(mode_name) = key.strip_suffix("_mode") else {
            continue;
        };
        if mode_name == "common" {
            continue;
        }
        let Some(form) = VehicleForm::from_mode_key(mode_name) else {
            // `basic_mode` on `stealth`, the camera stand-in: a real mode key
            // that is not a form anything drives.
            debug!("vehicle `{name}`: ignoring `{key}`, which is not a drivable form");
            continue;
        };

        modes.insert(form, parse_mode(block));
    }

    Some(VehicleDef {
        name: common
            .get("name")
            .map(|v| v.strings().join(" "))
            .unwrap_or_default(),
        health: health.first().copied().unwrap_or(1.0),
        shield: health.get(1).copied(),
        modes,
        camback,
        // No reader key: `vehicle.ron` supplies it or the default stands.
        turret_rate: None,
        aim_y: common
            .get("aimy")
            .and_then(|v| v.float_array::<2>())
            .unwrap_or([0.0, 0.0]),
        weapons: common
            .get("weapons")
            .map(|v| v.items().iter().filter_map(parse_weapon_row).collect())
            .unwrap_or_default(),
        ai: VehicleAiDefaults {
            activation: common.get("activation").and_then(|v| v.float()),
            pursuit_range: common
                .get("pursuit_range")
                .and_then(|v| v.float_array::<2>()),
            not_pursuit_dwell: common.get("not_pursuit_dwell").and_then(|v| v.float()),
            return_range: common.get("return_range").and_then(|v| v.float()),
        },
        drops: parse_drops(common),
        sounds: parse_vehicle_sounds(common),
        track_switch: common
            .get("track_switch")
            .and_then(|v| v.float_array::<3>()),
    })
}

/// Parse one `<form>_mode` block and derive the controller constants.
fn parse_mode(block: ReaderValue<'_>) -> ModeTuning {
    let mut tuning = ModeTuning {
        platform: block.get("platform").and_then(|v| vec3_array::<4>(v)),
        collision: block
            .get("collision")
            .map(|v| v.items().iter().filter_map(|p| vec3(*p)).collect())
            .unwrap_or_default(),
        // `mode_alts` is the same key spelled differently on 3 of 28 blocks.
        mode_alt: block
            .get("mode_alt")
            .or_else(|| block.get("mode_alts"))
            .and_then(|v| v.float())
            .unwrap_or(0.0),
        rates: block
            .get("rates")
            .and_then(|v| v.float_array::<2>())
            // // approximation: every shipped block has `rates`; a block
            // without one would otherwise be immovable.
            .unwrap_or([12.0, 40.0]),
        turns: block
            .get("turns")
            .and_then(|v| v.float_array::<2>())
            .unwrap_or([1.0, 3.0]),
        turn_damping: block
            .get("turn_damping")
            .and_then(|v| v.float())
            .unwrap_or(12.0),
        rate_damping: block
            .get("rate_damping")
            .and_then(|v| v.float_array::<2>())
            .unwrap_or([4.0, 1.25]),
        friction: block.get("friction").and_then(|v| v.float_array::<3>()),
        mass: block.get("mass").and_then(|v| v.float()),
        a_damping: block.get("a_damping").and_then(|v| v.float()),
        chas_smooth: block.get("chas_smooth").and_then(|v| v.float()),
        chas_pitch: block.get("chas_pitch").and_then(|v| v.float_array::<3>()),
        chas_roll: block.get("chas_roll").and_then(|v| v.float_array::<3>()),
        collision_d: block
            .get("collision_d")
            .and_then(|v| v.float_array::<2>())
            .unwrap_or([0.35, 0.0]),
        quicksand_slowdown: block.get("quicksand_slowdown").and_then(|v| v.float()),
        lava_slowdown: block.get("lava_slowdown").and_then(|v| v.float()),
        ok_to_amphib: block
            .get("ok_to_amphib")
            .and_then(|v| v.flag())
            .unwrap_or(false),
        alt_control: block.get("alt_control").and_then(|v| v.float_array::<3>()),
        gun_pitch: block.get("gun_pitch").and_then(|v| v.float_array::<2>()),
        wave: ["hover_wave", "amphib_wave", "sub_wave"]
            .into_iter()
            .find_map(|key| block.get(key).and_then(|v| v.float_array::<7>())),
        transitions: parse_transitions(block),
        sounds: parse_mode_sounds(block),

        // Replaced by `derive` below; a zero here would be a silent
        // immobility bug, so nothing reads these before it runs.
        suspension: SuspensionTuning {
            rays: Vec::new(),
            ray_lift: 0.0,
            rest: 0.0,
            stiffness: 0.0,
            damping: 0.0,
            max_len: 0.0,
        },
        engine_force: 0.0,
        max_speed: 0.0,
        reverse_speed: 0.0,
        lateral_grip: 0.0,
        static_grip: 0.0,
        air_control: 0.0,
        grip_force_scale: 0.0,
        regrip_fraction: 0.0,
        stationary_speed: 0.0,
        upright_rate_air: 0.0,
        upright_rate_ground: 0.0,
        max_tilt: 0.0,
    };

    tuning.derive();
    tuning
}

/// The eight transition keys, as (key, from, to).
///
/// Only the player's four mode blocks carry them, and all four carry the same
/// eight — the graph is a property of the *vehicle*, not of the mode you are
/// in (measured: 4 blocks × 8 keys, identical values).
const TRANSITION_KEYS: [(&str, VehicleForm, VehicleForm); 8] = [
    ("t2a_anims", VehicleForm::Track, VehicleForm::Amphib),
    ("a2t_anims", VehicleForm::Amphib, VehicleForm::Track),
    ("t2h_anims", VehicleForm::Track, VehicleForm::Hover),
    ("h2t_anims", VehicleForm::Hover, VehicleForm::Track),
    ("a2h_anims", VehicleForm::Amphib, VehicleForm::Hover),
    ("h2a_anims", VehicleForm::Hover, VehicleForm::Amphib),
    ("a2s_anims", VehicleForm::Amphib, VehicleForm::Sub),
    ("s2a_anims", VehicleForm::Sub, VehicleForm::Amphib),
];

fn parse_transitions(block: ReaderValue<'_>) -> HashMap<(VehicleForm, VehicleForm), [String; 2]> {
    let mut transitions = HashMap::default();

    for (key, from, to) in TRANSITION_KEYS {
        let Some(value) = block.get(key) else {
            continue;
        };
        let names = value.strings();
        if let [leave, enter] = names.as_slice() {
            transitions.insert((from, to), [leave.to_string(), enter.to_string()]);
        }
    }

    transitions
}

fn parse_weapon_row(row: &ReaderValue<'_>) -> Option<VehicleWeaponRow> {
    let weapon = row.nth(0)?.string()?.to_string();

    let mut raw = [0.0; 8];
    for (index, slot) in raw.iter_mut().enumerate() {
        *slot = row.nth(index + 1).and_then(|v| v.float()).unwrap_or(0.0);
    }

    Some(VehicleWeaponRow {
        weapon,
        // Column `b`: 1 for what the vehicle starts with (READER-SCHEMA §2.1).
        owned: raw[1] != 0.0,
        // Read as an integer, not out of `raw`: the player's unlimited RFPG
        // carries 123 456 789 rounds, which an f32 cannot hold exactly
        // (it lands on 123 456 792). Every other column is small enough
        // that the float round-trip is lossless.
        ammo: row
            .nth(3)
            .and_then(|v| v.int())
            .unwrap_or(0)
            .clamp(0, u32::MAX as i64) as u32,
        interval: raw[3],
        range: raw[5],
        raw,
    })
}

fn parse_drops(common: ReaderValue<'_>) -> Vec<DropDef> {
    let mut drops = Vec::new();

    if let Some(pickup) = common.get("pickups")
        && let (Some(kind), Some(amount)) = (
            pickup.nth(0).and_then(|v| v.string()),
            pickup.nth(1).and_then(|v| v.float()),
        )
    {
        drops.push(DropDef {
            kind: kind.to_string(),
            amount: amount.max(0.0) as u32,
        });
    }

    // `nanite` is `[count, value]`: `count` drops of `value` health each. The
    // kind is the reader's own word for it and has no `pickup.zrd` key.
    if let Some(nanite) = common.get("nanite").and_then(|v| v.float_array::<2>()) {
        drops.push(DropDef {
            kind: NANITE_DROP.to_string(),
            amount: nanite[0].max(0.0) as u32,
        });
    }

    drops
}

/// The [`DropDef::kind`] of a `nanite` drop, which is not a `pickup.zrd` key.
pub const NANITE_DROP: &str = "NANITE";

fn parse_vehicle_sounds(common: ReaderValue<'_>) -> VehicleSounds {
    let Some(sounds) = common.get("sounds") else {
        return VehicleSounds::default();
    };
    let name = |key: &str| sounds.get(key).and_then(|v| v.string()).map(str::to_string);

    VehicleSounds {
        skid: name("skid"),
        weapon_up: name("weapon_up"),
        weapon_down: name("weapon_down"),
        weapon_select: name("weapon_select"),
        pinging: name("pinging"),
    }
}

fn parse_mode_sounds(block: ReaderValue<'_>) -> ModeSounds {
    let Some(sounds) = block.get("sounds") else {
        return ModeSounds::default();
    };
    let name = |key: &str| sounds.get(key).and_then(|v| v.string()).map(str::to_string);

    ModeSounds {
        engine: name("engine"),
        skid: name("skid"),
        collide: name("collide"),
        land: name("land"),
        pitch_scale: sounds.get("pitch_scale").and_then(|v| v.float()),
        volume_scale: sounds.get("volume_scale").and_then(|v| v.float()),
    }
}

/// Parse `player.zrd`, falling back per key to the shipped value.
fn parse_player_constants(player: ReaderValue<'_>) -> PlayerConstants {
    let defaults = PlayerConstants::default();
    let float =
        |key: &str, fallback: f32| player.get(key).and_then(|v| v.float()).unwrap_or(fallback);

    PlayerConstants {
        gravity: float("nom_gravity", defaults.gravity),
        water_gravity: float("wat_gravity", defaults.water_gravity),
        quicksand_gravity: float("qsd_gravity", defaults.quicksand_gravity),
        quicksand_sink: float("qsand_sink", defaults.quicksand_sink),
        lava_sink: float("lava_sink", defaults.lava_sink),
        max_slope_cos: float("max_slope", defaults.max_slope_cos),
        camera: CameraConstants {
            zone: float("camera_zone", defaults.camera.zone),
            max_yaw_rate: float("max_cam_yaw_rate", defaults.camera.max_yaw_rate),
            mouse_push: player
                .get("mouse_push")
                .and_then(|v| v.float_array::<2>())
                .unwrap_or(defaults.camera.mouse_push),
            elastic: float("camera_elastic", defaults.camera.elastic),
            max_tether_angle_deg: float(
                "max_cam_tether_angle",
                defaults.camera.max_tether_angle_deg,
            ),
            underwater: player
                .get("underwater_cam")
                .and_then(|v| v.float_array::<8>())
                .unwrap_or(defaults.camera.underwater),
            fp_el_rate: float("fp_cam_el_rate", defaults.camera.fp_el_rate),
            fp_el_lim: player
                .get("fp_cam_el_lim")
                .and_then(|v| v.float_array::<2>())
                .unwrap_or(defaults.camera.fp_el_lim),
        },
        burning_anim: player
            .get("burning_anim")
            .and_then(|v| Some((v.nth(0)?.string()?.to_string(), v.nth(1)?.float()?)))
            .unwrap_or(defaults.burning_anim),
        low_shield: player
            .get("low_shield_snd")
            .and_then(|v| {
                Some((
                    v.nth(0)?.string()?.to_string(),
                    v.nth(1)?.float()?,
                    v.nth(2)?.string()?.to_string(),
                ))
            })
            .unwrap_or(defaults.low_shield),
    }
}

/// One `[x, y, z]` as a [`Vec3`].
fn vec3(value: ReaderValue<'_>) -> Option<Vec3> {
    value.float_array::<3>().map(Vec3::from_array)
}

/// Exactly `N` `[x, y, z]` triples.
fn vec3_array<const N: usize>(value: ReaderValue<'_>) -> Option<[Vec3; N]> {
    let items = value.items();
    if items.len() != N {
        return None;
    }

    let mut out = [Vec3::ZERO; N];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = vec3(item)?;
    }

    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A reader archive holding the two entries the table is built from.
    fn archive(vehicle: serde_json::Value, player: serde_json::Value) -> bft_assets::ReaderData {
        let mut entries = std::collections::BTreeMap::new();
        entries.insert("vehicle.zrd".to_string(), vehicle);
        entries.insert("player.zrd".to_string(), player);
        bft_assets::ReaderData::from_entries(entries)
    }

    /// The player's `track_mode`, abbreviated to the keys that matter, in the
    /// shape and with the values the real file has.
    fn bft_track() -> serde_json::Value {
        json!([
            "mode",
            "track",
            "platform",
            [
                [-2.25, 0.0, -2.4],
                [2.25, 0.0, -2.4],
                [2.25, 0.0, 2.34],
                [-2.25, 0.0, 2.34]
            ],
            "collision",
            [[-2.7, 1.0, -3.34], [2.7, 1.0, 3.34]],
            "ok_to_amphib",
            1,
            "mode_alt",
            0.0,
            "rates",
            [48.0, 65.0],
            "turns",
            [2.5, 3.5],
            "turn_damping",
            12.0,
            "rate_damping",
            [4.0, 1.25],
            "friction",
            [150.0, 80.0, 0.02],
            "collision_d",
            [0.35, 0.0],
            "mass",
            2.2,
            "a_damping",
            40.0,
            "quicksand_slowdown",
            0.4,
            "lava_slowdown",
            0.2,
            "t2a_anims",
            ["tracks_to_none", "none_to_amphib"],
            "a2s_anims",
            ["amphib_to_none", "body_to_sub"],
            "sounds",
            ["engine", "snd_vehtrk_eng", "pitch_scale", 0.5]
        ])
    }

    fn bft() -> serde_json::Value {
        json!([
            "common_mode",
            [
                "camback",
                [[0.0, 4.0, 12.75], [0.0, 3.5, 2.25], [0.0, 3.5, 2.25]],
                "aimy",
                [3.0, 3.0],
                "health",
                [140.0, 60.0],
                "track_switch",
                [10.0, 25.0, 45.0],
                "weapons",
                [["wep_1.0", 1, 1, 123456789, 0.15, 5, 100, 1, 2]],
                "sounds",
                ["skid", "snd_skid", "pinging", "snd_ping"]
            ],
            "track_mode",
            bft_track(),
            "sub_mode",
            [
                "mode",
                "sub",
                "rates",
                [25.0, 55.0],
                "mode_alt",
                -0.5,
                "gun_pitch",
                [-0.1736, 0.5],
                "turns",
                [1.0, 3.0],
                "turn_damping",
                12.0,
                "rate_damping",
                [4.0, 2.0],
                "collision_d",
                [0.35, 0.0],
                "sub_wave",
                [0.7, 0.0, 0.01, 0.9, 0.0, 0.07, 0.001]
            ]
        ])
    }

    fn player_zrd() -> serde_json::Value {
        json!([
            "camera_zone",
            0.7,
            "max_cam_yaw_rate",
            0.87,
            "mouse_push",
            [0.002, 0.01],
            "nom_gravity",
            40.0,
            "wat_gravity",
            9.0,
            "qsd_gravity",
            3.0,
            "max_slope",
            0.8192,
            "burning_anim",
            ["burning_model", 4],
            "low_shield_snd",
            ["snd_lowshield", 10, "snd_warn3"]
        ])
    }

    fn table() -> VehicleTable {
        build_vehicle_table(
            &archive(json!([["bft", bft()]]), json!([player_zrd()])),
            Difficulty::Normal,
            &VehicleOverrides::default(),
        )
        .expect("the table builds")
    }

    #[test]
    fn the_player_gets_one_mode_per_mode_block() {
        let table = table();
        let bft = table.player_def().expect("the player is there");

        let mut forms: Vec<_> = bft.forms().collect();
        forms.sort_by_key(|form| format!("{form:?}"));
        assert_eq!(forms, vec![VehicleForm::Sub, VehicleForm::Track]);
    }

    #[test]
    fn the_readers_own_values_arrive_unchanged() {
        let table = table();
        let track = table
            .player_def()
            .and_then(|def| def.mode(VehicleForm::Track))
            .expect("track mode");

        assert_eq!(track.rates, [48.0, 65.0]);
        assert_eq!(track.turns, [2.5, 3.5]);
        assert_eq!(track.friction, Some([150.0, 80.0, 0.02]));
        assert_eq!(track.mass, Some(2.2));
        assert_eq!(track.mode_alt, 0.0);
        assert!(track.ok_to_amphib);
        assert_eq!(track.quicksand_slowdown, Some(0.4));
        assert_eq!(
            track.platform.expect("a quad")[0],
            Vec3::new(-2.25, 0.0, -2.4)
        );
    }

    /// The one-element-list collapse, on the keys that actually have it.
    #[test]
    fn collapsed_scalars_read_as_scalars() {
        let table = table();
        let track = table
            .player_def()
            .and_then(|def| def.mode(VehicleForm::Track))
            .expect("track mode");

        assert_eq!(track.turn_damping, 12.0);
        assert_eq!(track.a_damping, Some(40.0));
        assert_eq!(track.lava_slowdown, Some(0.2));
    }

    #[test]
    fn the_transition_anims_are_keyed_by_the_pair_they_join() {
        let table = table();
        let track = table
            .player_def()
            .and_then(|def| def.mode(VehicleForm::Track))
            .expect("track mode");

        assert_eq!(
            track
                .transitions
                .get(&(VehicleForm::Track, VehicleForm::Amphib)),
            Some(&["tracks_to_none".to_string(), "none_to_amphib".to_string()])
        );
        assert_eq!(
            track
                .transitions
                .get(&(VehicleForm::Amphib, VehicleForm::Sub)),
            Some(&["amphib_to_none".to_string(), "body_to_sub".to_string()])
        );
    }

    #[test]
    fn the_derived_constants_follow_the_reader_values_they_are_derived_from() {
        let table = table();
        let track = table
            .player_def()
            .and_then(|def| def.mode(VehicleForm::Track))
            .expect("track mode");

        // `rates[0]` is read as top speed.
        assert_eq!(track.max_speed, 48.0);
        assert_eq!(track.reverse_speed, 65.0);
        // `friction` through the divisor, both coefficients.
        assert_eq!(track.lateral_grip, 8.0);
        assert_eq!(track.static_grip, 15.0);
        // The suspension rays are the platform corners.
        assert_eq!(track.suspension.rays.len(), 4);
        assert_eq!(track.suspension.rays[0], Vec3::new(-2.25, 0.0, -2.4));
        // `mode_alt` moves the rest length, and the ray looks a travel past
        // it — plus the lift, since the cast starts at the collider's floor
        // rather than at the contact patch the rays describe.
        assert_eq!(track.suspension.rest, SUSPENSION_REST);
        assert_eq!(track.suspension.ray_lift, SUSPENSION_RAY_LIFT);
        assert_eq!(
            track.suspension.max_len,
            SUSPENSION_RAY_LIFT + SUSPENSION_REST + SUSPENSION_TRAVEL
        );
        // The reach *below the contact patch* is what the spring actually
        // sees, and lifting the origin must not have changed it.
        let reach = track.suspension.max_len - track.suspension.ray_lift;
        assert!(
            (reach - (SUSPENSION_REST + SUSPENSION_TRAVEL)).abs() < 1e-5,
            "{reach}"
        );
        assert!(track.engine_force > 0.0);
        assert!(track.suspension.stiffness > 0.0);
        assert!(track.suspension.damping > 0.0);
    }

    /// `mode_alt −0.5` on the sub has to reach the rest length, or a form's
    /// ride height is silently the same as every other's.
    #[test]
    fn the_ride_height_offset_moves_the_suspension_rest_length() {
        let table = table();
        let sub = table
            .player_def()
            .and_then(|def| def.mode(VehicleForm::Sub))
            .expect("sub mode");

        assert_eq!(sub.mode_alt, -0.5);
        assert_eq!(sub.suspension.rest, SUSPENSION_REST - 0.5);
        assert_eq!(sub.gun_pitch, Some([-0.1736, 0.5]));
        assert_eq!(sub.wave.expect("sub_wave")[0], 0.7);
    }

    /// 23 of 28 mode blocks have no `platform`; they must still get rays.
    #[test]
    fn a_mode_without_a_platform_falls_back_rather_than_having_no_rays() {
        let table = table();
        let sub = table
            .player_def()
            .and_then(|def| def.mode(VehicleForm::Sub))
            .expect("sub mode");

        assert_eq!(sub.platform, None);
        assert_eq!(sub.suspension.rays.len(), 4);
        assert_eq!(sub.lateral_grip, DEFAULT_LATERAL_GRIP);
    }

    /// `mode_alts` is the same key: 3 of 28 blocks spell it that way.
    #[test]
    fn the_plural_spelling_of_mode_alt_is_the_same_key() {
        let vehicle = json!([[
            "bft",
            [
                "common_mode",
                ["health", [1.0]],
                "track_mode",
                [
                    "mode",
                    "track",
                    "mode_alts",
                    0.5,
                    "rates",
                    [12.0, 40.0],
                    "turns",
                    [1.0, 3.0],
                    "turn_damping",
                    12.0,
                    "rate_damping",
                    [4.0, 1.0],
                    "collision_d",
                    [0.3, 0.0]
                ]
            ]
        ]]);

        let table = build_vehicle_table(
            &archive(vehicle, json!([player_zrd()])),
            Difficulty::Normal,
            &VehicleOverrides::default(),
        )
        .expect("builds");

        let track = table
            .player_def()
            .and_then(|def| def.mode(VehicleForm::Track))
            .expect("track mode");
        assert_eq!(track.mode_alt, 0.5);
    }

    /// `stealth`'s `basic_mode` is a real key and not a drivable form.
    #[test]
    fn an_unknown_mode_block_is_skipped_without_losing_the_kind() {
        let vehicle = json!([[
            "bft",
            bft(),
            "stealth",
            [
                "common_mode",
                ["health", [10000.0, 10000.0]],
                "basic_mode",
                [
                    "mode",
                    "basic",
                    "rates",
                    [45.0, 65.0],
                    "turns",
                    [2.5, 3.5],
                    "turn_damping",
                    20.0,
                    "rate_damping",
                    [20.0, 20.0],
                    "collision_d",
                    [0.5, 0.15]
                ]
            ]
        ]]);

        let table = build_vehicle_table(
            &archive(vehicle, json!([player_zrd()])),
            Difficulty::Normal,
            &VehicleOverrides::default(),
        )
        .expect("builds");

        let stealth = table
            .kind(&VehicleKindId("stealth".to_string()))
            .expect("the kind survives");
        assert!(stealth.modes.is_empty(), "no drivable form");
        assert_eq!(stealth.health, 10000.0);
        assert_eq!(stealth.shield, Some(10000.0));
    }

    #[test]
    fn the_player_constants_are_the_readers_forty_not_earths_ten() {
        let table = table();

        assert_eq!(table.player.gravity, 40.0);
        assert_eq!(table.player.water_gravity, 9.0);
        assert_eq!(table.player.max_slope_cos, 0.8192);
        assert_eq!(table.player.camera.zone, 0.7);
        assert_eq!(
            table.player.burning_anim,
            ("burning_model".to_string(), 4.0)
        );
        assert_eq!(table.player.low_shield.1, 10.0);
    }

    #[test]
    fn the_weapon_rows_keep_the_columns_whose_meaning_is_unsettled() {
        let table = table();
        let row = &table.player_def().expect("player").weapons[0];

        assert_eq!(row.weapon, "wep_1.0");
        assert!(row.owned);
        assert_eq!(row.ammo, 123456789);
        assert_eq!(row.interval, 0.15);
        assert_eq!(row.range, 100.0);
        assert_eq!(row.raw, [1.0, 1.0, 123456789.0, 0.15, 5.0, 100.0, 1.0, 2.0]);
    }

    #[test]
    fn an_archive_without_the_player_is_an_error_not_an_empty_table() {
        let result = build_vehicle_table(
            &archive(json!([["scou", bft()]]), json!([player_zrd()])),
            Difficulty::Normal,
            &VehicleOverrides::default(),
        );

        assert_eq!(
            result.unwrap_err(),
            TableError::NoPlayerVehicle("vehicle.zrd")
        );
    }

    #[test]
    fn each_difficulty_names_its_own_file() {
        assert_eq!(Difficulty::Easy.vehicle_entry(), "vehicle_easy.zrd");
        assert_eq!(Difficulty::Normal.vehicle_entry(), "vehicle.zrd");
        assert_eq!(Difficulty::Hard.vehicle_entry(), "vehicle_hard.zrd");

        let missing = build_vehicle_table(
            &archive(json!([["bft", bft()]]), json!([player_zrd()])),
            Difficulty::Hard,
            &VehicleOverrides::default(),
        );
        assert_eq!(
            missing.unwrap_err(),
            TableError::NoVehicleEntry("vehicle_hard.zrd")
        );
    }

    #[test]
    fn the_death_drops_carry_both_the_pickup_and_the_nanites() {
        let vehicle = json!([[
            "bft",
            bft(),
            "scou",
            [
                "common_mode",
                [
                    "health",
                    [6.0],
                    "name",
                    ["Scout"],
                    "pickups",
                    ["ERFPG_AMMO", 50],
                    "nanite",
                    [3, 28],
                    "activation",
                    300.0,
                    "pursuit_range",
                    [60.0, 90.0],
                    "return_range",
                    175.0
                ]
            ]
        ]]);

        let table = build_vehicle_table(
            &archive(vehicle, json!([player_zrd()])),
            Difficulty::Normal,
            &VehicleOverrides::default(),
        )
        .expect("builds");

        let scout = table
            .kind(&VehicleKindId("scou".to_string()))
            .expect("the scout");
        assert_eq!(scout.name, "Scout");
        assert_eq!(
            scout.drops,
            vec![
                DropDef {
                    kind: "ERFPG_AMMO".to_string(),
                    amount: 50
                },
                DropDef {
                    kind: NANITE_DROP.to_string(),
                    amount: 3
                },
            ]
        );
        assert_eq!(scout.ai.activation, Some(300.0));
        assert_eq!(scout.ai.pursuit_range, Some([60.0, 90.0]));
        assert_eq!(scout.ai.return_range, Some(175.0));
    }
}
