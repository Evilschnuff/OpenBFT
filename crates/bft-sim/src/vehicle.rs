//! The vehicle: forms, the raycast-suspension controller, form switching and
//! the turret.
//!
//! One [`Vehicle`] component with a [`VehicleForm`] drives an avian dynamic
//! body. The controller is a "raycast vehicle": N suspension rays hold the
//! hull above the ground with spring-damper forces at the
//! [`platform`](tuning::ModeTuning::platform) corners, and engine, steering
//! and lateral-grip forces act in the hull frame on top (DESIGN §7.2).
//!
//! # The four forms and where their behaviour comes from
//!
//! The forms are named after the data: `vehicle.zrd` gives each vehicle a
//! `common_mode` block plus one `<form>_mode` block per form it has, and the
//! player has all four (READER-SCHEMA §2.1).
//!
//! | Form | Ground | Reader |
//! |---|---|---|
//! | [`Track`](VehicleForm::Track) | suspension on ground, hard lateral grip, turns in place | `track_mode`, `mode_alt 0` |
//! | [`Hover`](VehicleForm::Hover) | altitude controller, soft grip, crosses water | `hover_mode`, `mode_alt 0.5`, `alt_control` |
//! | [`Amphib`](VehicleForm::Amphib) | rides the liquid surface | `amphib_mode`, `mode_alt -0.1` |
//! | [`Sub`](VehicleForm::Sub) | no suspension, thrust and dive, clamped between floor and surface | `sub_mode`, `mode_alt -0.5`, `gun_pitch` |
//!
//! # Units, and what is still a guess
//!
//! Everything the reader states is used as stated: metres, seconds, radians
//! for `gun_pitch`, degrees for headings, 40 m/s² of gravity. The two
//! unit questions of READER-SCHEMA §5.1 are read here as:
//!
//! - **`rates` is `[top speed, reverse speed]` in m/s.** The alternative,
//!   `[top speed, acceleration]`, makes the sub's `[25, 55]` accelerate
//!   harder than the tank's `[48, 65]` while being half as fast, which no
//!   vehicle in the game behaves like.
//! - **`turns` is `[stationary, moving]` in rad/s**, used directly as a yaw
//!   rate target. The player's track `[2.5, 3.5]` gives a 2.5 rad/s spin in
//!   place, which matches "turns in place when stopped".
//!
//! `friction` is *not* used as a coefficient: its 40–300 range is not one.
//! `friction[1]` is read as a lateral-grip strength through a divisor, and
//! `friction[0]`/`friction[2]` are unused so far. Every constant with no
//! reader source is marked `// approximation` in [`tuning`] and is
//! overridable in `assets/tuning/vehicle.ron`.

use avian3d::dynamics::rigid_body::forces::{ForcesItem, WriteRigidBodyForces};
use avian3d::prelude::*;
use bevy::ecs::query::QueryData;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bft_assets::materials::Soil;

use crate::level::GameLayer;
use crate::physics::{GroundProbe, PhysicsTuning, SurfaceQuery, layers, probe_ground};
use crate::schedule::SimSet;
use crate::water::{LiquidKind, WaterSample, WaterVolume, water_at};

pub mod input;
pub mod overrides;
pub mod tuning;

pub use input::{Controlled, PlayerInput};
pub use overrides::VehicleOverrides;
pub use tuning::{
    CameraConstants, Difficulty, DropDef, ModeSounds, ModeTuning, PlayerConstants,
    SuspensionTuning, TableError, VehicleAiDefaults, VehicleDef, VehicleKindId, VehicleSounds,
    VehicleTable, VehicleWeaponRow, build_vehicle_table,
};

/// Which form a vehicle is in. The four `vehicle.zrd` mode keys.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub enum VehicleForm {
    Track,
    Hover,
    Amphib,
    Sub,
}

impl VehicleForm {
    /// Every form, in the order [`FormSet`] and the manual cycle use.
    ///
    /// **Track, Amphib, Sub, Hover** — not Track → Amphib → Hover → Sub,
    /// which makes the sub unreachable: the cycle skips any form the transition
    /// graph forbids from where you are, the graph has no hover↔sub pair, and
    /// so from Amphib the cycle lands on Hover and can never get back to Sub
    /// without going through Amphib again. Putting Sub next to Amphib — the
    /// only form it connects to (READER-SCHEMA §2.1) — makes one press from
    /// Amphib over deep water dive, which is the behaviour the design
    /// describes.
    pub const ALL: [VehicleForm; 4] = [
        VehicleForm::Track,
        VehicleForm::Amphib,
        VehicleForm::Sub,
        VehicleForm::Hover,
    ];

    /// The `vehicle.zrd` mode key, without the `_mode` suffix.
    pub fn mode_key(&self) -> &'static str {
        match self {
            VehicleForm::Track => "track",
            VehicleForm::Hover => "hover",
            VehicleForm::Amphib => "amphib",
            VehicleForm::Sub => "sub",
        }
    }

    /// The form a `vehicle.zrd` mode key names.
    ///
    /// `None` for `basic`, `stealth`'s camera-stand-in mode, which is a real
    /// key and not a drivable form.
    pub fn from_mode_key(key: &str) -> Option<VehicleForm> {
        match key {
            "track" => Some(VehicleForm::Track),
            "hover" => Some(VehicleForm::Hover),
            "amphib" => Some(VehicleForm::Amphib),
            "sub" => Some(VehicleForm::Sub),
            _ => None,
        }
    }

    /// Its bit in a [`FormSet`].
    fn bit(&self) -> u8 {
        match self {
            VehicleForm::Track => 1,
            VehicleForm::Amphib => 2,
            VehicleForm::Hover => 4,
            VehicleForm::Sub => 8,
        }
    }

    /// Whether this form rides on a liquid surface rather than on ground.
    pub fn is_surface_form(&self) -> bool {
        matches!(self, VehicleForm::Amphib)
    }
}

/// Which transitions the data allows, as the key set states it.
///
/// track↔amphib, track↔hover, amphib↔hover, amphib↔sub. **There is no direct
/// track↔sub**: the sub is reachable only from amphib (READER-SCHEMA §2.1).
pub const TRANSITIONS: [(VehicleForm, VehicleForm); 8] = [
    (VehicleForm::Track, VehicleForm::Amphib),
    (VehicleForm::Amphib, VehicleForm::Track),
    (VehicleForm::Track, VehicleForm::Hover),
    (VehicleForm::Hover, VehicleForm::Track),
    (VehicleForm::Amphib, VehicleForm::Hover),
    (VehicleForm::Hover, VehicleForm::Amphib),
    (VehicleForm::Amphib, VehicleForm::Sub),
    (VehicleForm::Sub, VehicleForm::Amphib),
];

/// Whether the data allows going straight from one form to another.
pub fn transition_allowed(from: VehicleForm, to: VehicleForm) -> bool {
    TRANSITIONS.contains(&(from, to))
}

/// Which forms a vehicle has unlocked.
///
/// A bitflag rather than a set because it replicates as one byte. Track is always set: it is the form the player starts in and the
/// only one 19 of 25 kinds have.
// `Serialize`/`Deserialize` because `Progress` saves which forms the player
// has unlocked; the bitset is the whole of that state.
#[derive(
    Component, Clone, Copy, Default, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize,
)]
pub struct FormSet(pub u8);

impl FormSet {
    /// Just the form every vehicle has.
    pub fn track_only() -> FormSet {
        FormSet(VehicleForm::Track.bit())
    }

    /// Every form, for a debug spawn or a level that grants them all.
    pub fn all() -> FormSet {
        FormSet(VehicleForm::ALL.iter().map(VehicleForm::bit).sum())
    }

    /// From a list of forms, always including Track.
    pub fn of(forms: impl IntoIterator<Item = VehicleForm>) -> FormSet {
        let mut set = FormSet::track_only();
        for form in forms {
            set.unlock(form);
        }
        set
    }

    /// Whether this form is unlocked.
    pub fn has(&self, form: VehicleForm) -> bool {
        self.0 & form.bit() != 0
    }

    /// Unlock a form; the three chip pickups do this (DESIGN §7.2).
    pub fn unlock(&mut self, form: VehicleForm) {
        self.0 |= form.bit();
    }

    /// The unlocked forms, in cycle order.
    pub fn forms(&self) -> impl Iterator<Item = VehicleForm> + '_ {
        VehicleForm::ALL.into_iter().filter(|form| self.has(*form))
    }
}

/// A vehicle: which kind it is, what form it is in, what it may become.
#[derive(Component, Debug, Clone)]
pub struct Vehicle {
    pub kind: VehicleKindId,
    pub form: VehicleForm,
    pub unlocked: FormSet,
}

impl Vehicle {
    /// The player's BFT in Track form with these unlocks.
    pub fn player(unlocked: FormSet) -> Vehicle {
        Vehicle {
            kind: VehicleKindId::player(),
            form: VehicleForm::Track,
            unlocked,
        }
    }
}

/// The turret's aim, in radians relative to the hull.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct Turret {
    /// Yaw about the hull's up axis.
    pub yaw: f32,
    /// Pitch, positive up, clamped by the mode's `gun_pitch` where it has one.
    pub pitch: f32,
}

/// What the vehicle is standing in and on, written every tick by the
/// controller for the camera, the HUD and the weapons to read.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct GroundState {
    /// Whether any suspension ray found ground within its travel.
    pub grounded: bool,
    /// The soil under the vehicle, from the deepest-compressed ray.
    pub soil: Option<Soil>,
    /// The ground normal, averaged over the rays that hit. [`Vec3::Y`] when
    /// airborne, so a caller never divides by a zero vector.
    pub normal: Vec3,
    /// The liquid the hull origin is in, from
    /// [`water_at`](crate::water::water_at).
    pub in_water: Option<WaterSample>,
    /// The collider under the deepest-compressed ray: what the vehicle stands
    /// on, which the cinematics' `carry_on_support` moves it with.
    pub support: Option<Entity>,
    /// The ground under the suspension rays as a plane in the hull's own
    /// frame, for the model to put its tracks on. `None` when
    /// no ray hit.
    pub contact: Option<GroundContact>,
}

/// The ground under a hull, in the hull's frame.
///
/// The springs hold the hull origin — which is where `bft_00` has its tracks
/// — some way off the ground, and let it pitch and roll on them. The model
/// uses this to put the tracks back on the ground and keep the pitching for
/// the upper body.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundContact {
    /// The ground's up direction, in hull space.
    pub normal: Vec3,
    /// Where the ground plane crosses the hull's own up axis, in hull space:
    /// how far below the origin the ground is, negated.
    pub height: f32,
}

impl GroundContact {
    /// The plane through hull-space ground points with this normal.
    ///
    /// The height is taken at the points' centroid and carried to the hull's
    /// up axis along the plane, so an uneven spread of hits does not tilt it.
    pub fn fit(points: &[Vec3], normal: Vec3) -> Option<GroundContact> {
        if points.is_empty() || normal.y <= 1e-3 {
            return None;
        }
        let normal = normal.normalize();
        let centroid = points.iter().copied().sum::<Vec3>() / points.len() as f32;
        let height = centroid.y + (normal.x * centroid.x + normal.z * centroid.z) / normal.y;
        Some(GroundContact { normal, height })
    }
}

impl GroundState {
    /// The state of a vehicle with nothing under it.
    pub fn airborne() -> GroundState {
        GroundState {
            grounded: false,
            soil: None,
            normal: Vec3::Y,
            in_water: None,
            support: None,
            contact: None,
        }
    }

    /// Whether the vehicle is in deep enough water for the sub.
    pub fn is_divable(&self) -> bool {
        self.in_water.is_some_and(|water| {
            water.kind == LiquidKind::Water
                && water
                    .floor_y
                    .is_some_and(|floor| water.surface_y - floor >= crate::water::SHALLOW_DEPTH)
        })
    }

    /// Whether the vehicle is in water of any depth.
    pub fn is_in_water(&self) -> bool {
        self.in_water
            .is_some_and(|water| water.kind == LiquidKind::Water)
    }
}

/// Whether the tracks are gripping or sliding, and how hard.
///
/// # Why this is a state and not a calculation
///
/// Real traction has **two** coefficients and **two** thresholds: it takes
/// more force to break a contact patch loose than to keep it sliding, so once
/// traction goes it stays gone until the slide slows well below the speed
/// that broke it. That gap is what makes a skid commit rather than flicker,
/// and it cannot be computed from the current tick alone — a model with one
/// threshold re-grips the instant lateral speed dips under it, which reads as
/// "very steerable" however low the limit is set.
///
/// So the slide is a latched state on the vehicle, entered when the lateral
/// load exceeds static grip and left only when the slide has decayed past
/// [`REGRIP_FRACTION`] of the speed that broke it.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct Traction {
    /// Whether the contact patch is currently sliding.
    pub sliding: bool,
    /// How fast it is sliding sideways, m/s, **signed** along the hull's
    /// right axis: positive means being carried to the right. For the HUD,
    /// the skid sound and any tyre-mark effects.
    pub slip_speed: f32,
    /// The sideways speed at the moment traction broke, m/s.
    ///
    /// The re-grip threshold is a fraction of *this*, not of a fixed number:
    /// a slide entered gently should end sooner than one entered at speed,
    /// and a threshold in absolute m/s would make a fast skid end almost
    /// immediately relative to how hard it started.
    pub broke_at: f32,
}

impl Traction {
    /// Whether the vehicle is sliding fast enough to be worth reacting to.
    ///
    /// The threshold keeps a vehicle that is technically past its grip limit
    /// by a fraction of a metre per second from screeching.
    pub fn is_skidding(&self) -> bool {
        self.sliding && self.slip_speed.abs() > SKID_REPORT_SPEED
    }
}

/// How slow a slide has to get, as a fraction of the speed that broke
/// traction, before the tracks bite again.
///
/// // approximation: the reader gives two friction values but no re-grip
/// rule. Well under 1 so the slide commits — at 1.0 the vehicle would
/// re-grip the moment it dipped below the breaking speed, which is the
/// single-threshold model this replaced and the one that felt too
/// steerable.
pub const REGRIP_FRACTION: f32 = 0.5;

/// The slowest slide worth calling a skid, m/s.
///
/// // approximation: no reader field corresponds. Below this the slip is
/// the ordinary give of a hard turn, not a skid anyone should hear.
pub const SKID_REPORT_SPEED: f32 = 1.5;

/// The locally controlled player vehicle. Exactly one per client.
#[derive(Component, Debug, Clone, Copy)]
pub struct PlayerVehicle;

/// A place a vehicle may start.
#[derive(Component, Debug, Clone, Copy)]
pub struct SpawnPoint {
    pub kind: SpawnKind,
    /// Yaw in radians, already converted from the reader's degrees.
    pub heading: f32,
}

/// Which kind of start a [`SpawnPoint`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpawnKind {
    /// `aiv.zrd`'s `bft_00`.
    Player,
    /// `net.zrd`'s ring of 8, by index.
    Deathmatch(u8),
}

/// A form change that happened this tick.
///
/// [`Self::anims`] carries the transition's two anim names — "leave the old
/// form", "enter the new one" — for the anim VM to play and its handlers to
/// hook events off. Empty strings when the data has no pair for the
/// transition, which is every kind but the player.
///
/// A `Message`, not Bevy 0.19's `Event`: the two split in this version, and
/// `Event` is now the *observer* kind, delivered immediately to one handler.
/// A form change is read as a stream by several systems — the anim VM,
/// the HUD, the sound layer — which is what `Message` is.
#[derive(Message, Debug, Clone)]
pub struct FormChanged {
    pub entity: Entity,
    pub from: VehicleForm,
    pub to: VehicleForm,
    pub anims: [String; 2],
}

// ---------------------------------------------------------------------------
// Spawning
// ---------------------------------------------------------------------------

/// Where the top of the hull box sits above the vehicle origin: the top of
/// `bft_00`'s node bbox, which runs y 0…2.825 (measured).
///
/// [`HULL_FLOOR_Y`] *plus* the model's whole height would put its top at
/// 3.8 m — a metre of phantom hull over the turret — and wedge it under the
/// VTOL's `interior_ceiling` (3.72 m clear of `interior_floor`) before it
/// reached the ramp.
pub const HULL_TOP_Y: f32 = 2.825;

/// How tall the hull box is, in metres.
///
/// // approximation: the `collision` hull is 12 points all at `y = 1` — a
/// horizontal ring, not a solid — so it states the hull's *footprint* and
/// says nothing about its height. The box is the ring's x/z extent, from
/// [`HULL_FLOOR_Y`] up to the model's top, [`HULL_TOP_Y`].
pub const HULL_HEIGHT: f32 = HULL_TOP_Y - HULL_FLOOR_Y;

/// Where the bottom of the hull box sits above the vehicle origin, and the
/// top of the tracks' collider below it.
///
/// The `collision` ring's own `y` is 1.0, and the box starts there. Below
/// it, [`track_collider`] reaches down to the contact patch; the springs
/// hold that 0.2 m off the ground (`SUSPENSION_TRAVEL`), so it clears the
/// terrain on the flat and meets whatever the springs cannot take.
///
/// It is not [`tuning::SUSPENSION_REST`], which is 0 (see there): the ring
/// keeps its own height.
pub const HULL_FLOOR_Y: f32 = 1.0;

/// The hull collider a mode's `collision` ring describes.
///
/// **The ring is a hand-authored outline, not a box.** The player's four
/// modes each state twelve hull-space points at `y = 1`: six at the front and
/// back corners (`z = ±3.34`) and six bulging out at the waist — for `track`,
/// `x = ±2.83` at `z = 0` against `±2.70` at the corners. So the footprint is
/// a chamfered hull, widest in the middle, and it differs per form: amphib is
/// narrow (±2.4), hover is the widest (±3.75), sub the narrowest (±2.2).
///
/// An axis-aligned box over those points — which is what [`hull_box`] returns
/// — extends the widest point to every corner,
/// making the vehicle fatter at its ends than the original intended it. This
/// extrudes the real outline instead.
///
/// The height is still an approximation: every point is at `y = 1`, so the
/// ring is a flat ring and states the footprint only. See [`HULL_HEIGHT`].
///
/// Falls back to [`hull_box`]'s cuboid for the 23 of 28 mode blocks with no
/// ring, which is every enemy — they have no authored outline to honour.
pub fn hull_collider(mode: Option<&ModeTuning>) -> Collider {
    let (offset, upper) = upper_hull_collider(mode);
    Collider::compound(vec![
        (offset, Quat::IDENTITY, upper),
        (Vec3::ZERO, Quat::IDENTITY, track_collider()),
    ])
}

/// The hull above the tracks: the `collision` ring, extruded from
/// [`HULL_FLOOR_Y`] to [`HULL_TOP_Y`], and where it sits in hull space.
///
/// Not a compound itself, because it goes into one and parry does not nest
/// them.
fn upper_hull_collider(mode: Option<&ModeTuning>) -> (Vec3, Collider) {
    let points = mode.map(|mode| mode.collision.as_slice()).unwrap_or(&[]);
    let hull_box_collider = || {
        let (half, centre) = hull_box(mode);
        (
            centre,
            Collider::cuboid(half.x * 2.0, half.y * 2.0, half.z * 2.0),
        )
    };

    // Fewer than three points cannot bound an area, so there is nothing to
    // extrude and the box is all the data supports.
    if points.len() < 3 {
        return hull_box_collider();
    }

    // Extrude the outline from the hull floor to the hull top. The ring's own
    // `y` is dropped: it is 1.0 on every point of every mode, which is the
    // plane the outline was drawn on and not a height.
    let floor = HULL_FLOOR_Y;
    let top = HULL_FLOOR_Y + HULL_HEIGHT;

    let mut vertices = Vec::with_capacity(points.len() * 2);
    for point in points {
        vertices.push(Vec3::new(point.x, floor, point.z));
        vertices.push(Vec3::new(point.x, top, point.z));
    }

    // `convex_hull` rather than a hand-built mesh: the ring's winding is not
    // stated anywhere and a convex hull does not care, where a trimesh would
    // end up inside out. The points are convex already, so nothing is lost.
    match Collider::convex_hull(vertices) {
        Some(hull) => (Vec3::ZERO, hull),
        None => {
            warn!("this mode's `collision` ring does not bound a hull; using its box");
            hull_box_collider()
        }
    }
}

/// Half the width across both tracks' outer faces, in metres.
///
/// `bft_00`'s `left_morphs`/`right_morphs` sit at x = ±1.55 and their track
/// meshes reach 1.15 m outward from there (measured).
pub const TRACK_HALF_WIDTH: f32 = 2.70;

/// The side profile of `bft_00`'s tracks, as `(|z|, y)` in hull space, from
/// the flat run on the ground round the nose to the hull floor.
///
/// Measured from the `ltracks` mesh (y + 0.35 for its node's offset): flat
/// on the contact patch to z = ±2.38, then a rounded nose — 0.06 m up at
/// 2.67, 0.17 at 2.94, 0.38 at 3.28, 0.49 at the tip, 3.33. The last point
/// carries the nose up to the `collision` ring's corner at [`HULL_FLOOR_Y`].
/// Front and back are the same within a few centimetres, so one profile
/// serves both.
pub const TRACK_PROFILE: [(f32, f32); 6] = [
    (2.38, 0.0),
    (2.67, 0.06),
    (2.94, 0.17),
    (3.28, 0.38),
    (3.33, 0.49),
    (3.34, HULL_FLOOR_Y),
];

/// The tracks as a solid: the running gear the springs ride on, and what
/// meets a ledge.
///
/// **Why the hull needs it.** The suspension rays only see the ground under
/// the `platform` corners, and the springs will lift the hull onto anything
/// those rays land on. With no solid below [`HULL_FLOOR_Y`], a ledge lower
/// than the hull floor never touches the vehicle: the front rays find its top
/// and the tank walks up a 1.5 m wall. And a slope rising under the front of
/// the hull meets the box's square edge before the rays have lifted the nose,
/// which stops a tank at the far wall of a crater instead of launching it.
///
/// The tracks' own rounded nose is what a real tank climbs with: a step
/// that meets its shallow lower part pushes the hull up over it, one that
/// meets the steep upper part stops it. One convex hull spans both tracks;
/// the belly between them is above the tracks' tops anyway.
pub fn track_collider() -> Collider {
    let mut vertices = Vec::with_capacity(TRACK_PROFILE.len() * 4);
    for (z, y) in TRACK_PROFILE {
        for x in [-TRACK_HALF_WIDTH, TRACK_HALF_WIDTH] {
            vertices.push(Vec3::new(x, y, -z));
            vertices.push(Vec3::new(x, y, z));
        }
    }
    Collider::convex_hull(vertices).expect("the track profile bounds a solid")
}

/// The hull box a mode's `collision` ring describes.
///
/// Returns the half-extents and the centre offset in hull space. Falls back
/// to the player's track footprint for the 23 of 28 mode blocks that carry no
/// `collision` ring.
pub fn hull_box(mode: Option<&ModeTuning>) -> (Vec3, Vec3) {
    let points = mode.map(|mode| mode.collision.as_slice()).unwrap_or(&[]);

    let (min, max) = match points {
        [] => (
            // // approximation: the player's `track_mode` collision ring,
            // ±2.83 × ±3.34.
            Vec3::new(-2.83, 0.0, -3.34),
            Vec3::new(2.83, 0.0, 3.34),
        ),
        points => points.iter().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(min, max), point| (min.min(*point), max.max(*point)),
        ),
    };

    let half = Vec3::new(
        (max.x - min.x) / 2.0,
        HULL_HEIGHT / 2.0,
        (max.z - min.z) / 2.0,
    );
    let centre = Vec3::new(
        (max.x + min.x) / 2.0,
        HULL_FLOOR_Y + HULL_HEIGHT / 2.0,
        (max.z + min.z) / 2.0,
    );

    (half, centre)
}

/// Angular damping, so the hull settles rather than tumbling.
///
/// The data's `a_damping` is 4–40 and is not avian's dimensionless per-second
/// damping — 40 would freeze the body solid — so it is scaled by this.
/// // approximation: derived from `a_damping`, divided to land the player's
/// 40 on 4.0, which is stable and still lets the hull pitch over a rise.
pub const A_DAMPING_DIVISOR: f32 = 10.0;

/// How high the centre of mass sits above the vehicle origin: the middle of
/// the track band.
///
/// `bft_00`'s track meshes (`ltracks`, `rtracks`) are 0.95 m tall and stand
/// on the contact patch, so the tracks fill y 0…0.95 (measured) — right up
/// to the collider's floor at [`HULL_FLOOR_Y`]. Left to avian, the centre of
/// mass would be the collider's, around 1.9 m up, and every hit on the upper
/// hull would have that much leverage to roll it.
///
/// // approximation: the player's tracks, used for every vehicle. Weight in
/// the running gear is the shape of a tank; no reader field states it.
pub const CENTER_OF_MASS_Y: f32 = HULL_FLOOR_Y / 2.0;

/// The centre of mass for a mode's hull, in hull space: under the middle of
/// its footprint, at [`CENTER_OF_MASS_Y`].
pub fn center_of_mass(mode: Option<&ModeTuning>) -> CenterOfMass {
    let (_, centre) = hull_box(mode);
    CenterOfMass(Vec3::new(centre.x, CENTER_OF_MASS_Y, centre.z))
}

/// Linear damping on the hull.
///
/// // approximation: no reader field corresponds. Small, because the drive
/// system does its own speed limiting; this only bleeds off drift.
pub const LINEAR_DAMPING: f32 = 0.1;

/// Spawn the player's vehicle, with its collider, turret and ground state.
///
/// `at` is where and which way it faces — from `aiv.zrd`'s `bft_00`
/// (READER-SCHEMA §3.2), never the `bft_00` *node* transform, which is a
/// build-script placeholder (docs/data/nodes.md).
pub fn spawn_player_vehicle(
    commands: &mut Commands,
    at: Transform,
    table: &VehicleTable,
    unlocked: FormSet,
) -> Entity {
    let vehicle = Vehicle::player(unlocked);
    let def = table.kind(&vehicle.kind);
    let mode = def.and_then(|def| def.mode(vehicle.form));

    let mass = mode
        .and_then(|mode| mode.mass)
        .unwrap_or(tuning::DEFAULT_MASS)
        * tuning::MASS_UNIT_KG;
    let angular_damping = mode.and_then(|mode| mode.a_damping).unwrap_or(40.0) / A_DAMPING_DIVISOR;

    let hull = commands
        .spawn((
            Name::new("player vehicle"),
            vehicle,
            PlayerVehicle,
            Controlled::Local,
            PlayerInput::default(),
            Turret::default(),
            def.and_then(|def| def.turret_rate)
                .map(|[yaw, pitch]| TurretRate { yaw, pitch })
                .unwrap_or_default(),
            GroundState::airborne(),
            Traction::default(),
            at,
            // Nested, because a spawn tuple holds at most 15 components and
            // the body's own make a natural group.
            (
                RigidBody::Dynamic,
                // The mode's own `collision` outline, extruded — not a box
                // over its extent. Its vertices are already in hull space
                // (the origin is at the track contact patch, the way
                // `platform` at y = 0 and `collision` at y = 1 state it), so
                // no offset transform is needed. `ColliderTransform` looks
                // like the component for that and is not: avian recomputes it
                // from the hierarchy every step, so a hand-set value is
                // overwritten.
                hull_collider(mode),
                Mass(mass),
                // Low, in the tracks, and not averaged with anything a
                // child may add.
                center_of_mass(mode),
                NoAutoCenterOfMass,
                AngularDamping(angular_damping),
                LinearDamping(LINEAR_DAMPING),
                // Spawned explicitly, not left to avian's default: the water
                // and quicksand regimes scale this every tick, and a body
                // without the component would silently keep falling at
                // `nom_gravity` wherever it was (`apply_gravity_scale`).
                GravityScale(1.0),
                layers::vehicle(),
            ),
        ))
        .id();

    // The turret is a child so its yaw composes with the hull's rotation
    // without the controller having to recompose it every tick.
    //
    // A stand-in: on the player, `bft_game::player_model` despawns this and
    // marks the model's own `turret` node instead. A vehicle with no model keeps it.
    let turret = commands
        .spawn((
            Name::new("turret"),
            TurretPart,
            Transform::from_translation(Vec3::new(
                0.0,
                def.map(|def| def.aim_y[0]).unwrap_or(0.0),
                0.0,
            )),
            ChildOf(hull),
        ))
        .id();

    commands.entity(hull).insert(AimJoints {
        yaw: turret,
        pitch: None,
    });

    hull
}

/// The turret's yaw joint, a child of the hull.
///
/// On a vehicle with a model this is the real `…/chassis/turret` node, which
/// `bft_game::player_model` marks when it attaches `bft_00`; on one without, it is the empty
/// transform spawned above, which the camera can still aim along.
///
/// When a [`GunPart`] exists below it this carries **yaw only**. When one does
/// not it carries yaw and pitch together, because a vehicle with no barrel to
/// pitch still has to point somewhere. [`aim_turret`] decides which.
#[derive(Component, Debug, Clone, Copy)]
pub struct TurretPart;

/// The barrel's pitch joint: the model's `…/turret/gun` node.
///
/// Separate from [`TurretPart`] because the real subtree separates them —
/// `turret` and `gun` are two translation-only nodes, one under the other, and
/// pitching the outer one would tip the turret ring and its doors with the
/// barrel (docs/data/nodes.md). Only a vehicle with a model has one.
#[derive(Component, Debug, Clone, Copy)]
pub struct GunPart;

/// Which entities carry this vehicle's aim, resolved once when it is built.
///
/// Scanning the hull's **direct** children for the turret works for the
/// stand-in and silently fails for a real model: the `turret` node is four
/// deep (`bft_00/body/chassis/turret`). Rather than walk the
/// hierarchy every tick, whoever builds the vehicle records the joints here.
///
/// `yaw` is required; `pitch` is `None` on a vehicle whose model has no
/// separate barrel, and then `yaw` carries both rotations.
#[derive(Component, Debug, Clone, Copy)]
pub struct AimJoints {
    /// The entity marked [`TurretPart`]: the turret ring.
    pub yaw: Entity,
    /// The entity marked [`GunPart`]: the barrel, if there is one.
    pub pitch: Option<Entity>,
}

/// Where the barrel pivots, in hull space: the aim joint's position as posed
/// now.
///
/// [`aim_turret`] measures its angles from here, and the weapons'
/// `follow_firepoints` keeps it on the gun joint as the turret turns. A
/// vehicle without one aims from its origin.
#[derive(Component, Debug, Clone, Copy, Default, PartialEq)]
pub struct AimPivot(pub Vec3);

/// `entity`'s transform relative to its ancestor `ancestor`, composed from
/// the local transforms.
///
/// `None` if `ancestor` is not above it. Reads [`Transform`] rather than
/// `GlobalTransform` so it sees joints turned earlier in the same tick.
pub fn in_ancestor_space(
    entity: Entity,
    ancestor: Entity,
    locals: &Query<(&Transform, Option<&ChildOf>)>,
) -> Option<Transform> {
    let mut composed = Transform::IDENTITY;
    let mut at = entity;

    while at != ancestor {
        let (local, parent) = locals.get(at).ok()?;
        composed = *local * composed;
        at = parent?.parent();
    }

    Some(composed)
}

// ---------------------------------------------------------------------------
// The controller
// ---------------------------------------------------------------------------

/// How slow counts as "stationary" for turn-in-place, in m/s.
///
/// // approximation: `turns[0]` is the stationary turn rate and `turns[1]`
/// the moving one, and the data does not say where the line is. Two metres a
/// second is a walking pace against a 48 m/s top speed.
pub const STATIONARY_SPEED: f32 = 2.0;

/// How strongly the buoyancy spring pulls a surface form to its ride height.
///
/// // approximation: no reader field corresponds. As a multiple of gravity,
/// so a heavier vehicle floats the same.
pub const BUOYANCY_STIFFNESS: f32 = 6.0;

/// How much of the vertical velocity buoyancy damps out, per second.
///
/// // approximation: enough that an amphib entering water settles rather
/// than porpoising.
pub const BUOYANCY_DAMPING: f32 = 3.0;

/// How far below the surface and above the floor a sub may go, in metres.
///
/// // approximation: the sub's `mode_alt` is −0.5 and the hull is 2.8 m
/// tall, so a metre of clearance keeps it out of both.
pub const SUB_MARGIN: f32 = 1.0;

/// How fast the sub climbs and dives at full pitch input, in m/s.
///
/// // approximation: `rates[0]` is a horizontal speed and the data states no
/// vertical one. A third of the sub's 25 m/s is a controllable dive.
pub const SUB_VERTICAL_SPEED: f32 = 8.0;

/// How hard a Track-form vehicle in water is held back.
///
/// // approximation: the reader has `quicksand_slowdown` and `lava_slowdown`
/// but no water slowdown; `wat_gravity` is the water rule for gravity and the
/// tank is meant to be a bad boat. Multiplies engine force and grip.
pub const WATER_SLOWDOWN: f32 = 0.35;

/// The soil-driven speed multiplier for a mode.
///
/// Water has no reader slowdown, so [`WATER_SLOWDOWN`] stands in; quicksand
/// and lava use the mode's own values (READER-SCHEMA §2.1).
pub fn soil_speed_multiplier(mode: &ModeTuning, soil: Option<Soil>) -> f32 {
    match soil {
        Some(Soil::Quicksand) => mode.quicksand_slowdown.unwrap_or(1.0),
        Some(Soil::Lava) => mode.lava_slowdown.unwrap_or(1.0),
        Some(Soil::Water) => WATER_SLOWDOWN,
        _ => 1.0,
    }
}

/// Which gravity a vehicle falls at, given what it is in.
///
/// Returned as a *scale* on avian's global gravity, which is
/// `PhysicsTuning::gravity` (the physics owns the global; this only scales a
/// body's share of it). A surface or sub form in water is weightless: its own
/// buoyancy or depth controller holds it.
pub fn gravity_scale(form: VehicleForm, ground: &GroundState, tuning: &PhysicsTuning) -> f32 {
    if form == VehicleForm::Sub && ground.is_divable() {
        return 0.0;
    }

    match ground.in_water.map(|water| water.kind) {
        Some(LiquidKind::Water) if form.is_surface_form() => 0.0,
        Some(LiquidKind::Water) => tuning.water_gravity / tuning.gravity,
        Some(LiquidKind::Quicksand) => tuning.quicksand_gravity / tuning.gravity,
        _ => 1.0,
    }
}

/// The controller: one system per concern, chained in [`SimSet::Vehicle`].
pub struct VehiclePlugin;

impl Plugin for VehiclePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<FormChanged>()
            .add_message::<VehicleCollided>()
            .init_resource::<Difficulty>()
            .add_systems(
                FixedUpdate,
                (
                    sense_ground,
                    switch_forms,
                    aim_turret,
                    apply_gravity_scale,
                    drive,
                )
                    .chain()
                    // No `in_state` here: `SimSet::Vehicle` already runs only
                    // while playing and unpaused (`sim_running`).
                    .in_set(SimSet::Vehicle),
            )
            // Between the solver and the writeback, so the transform never
            // shows a lean the limit takes back.
            .add_systems(
                FixedUpdate,
                limit_tilt
                    .after(PhysicsSystems::StepSimulation)
                    .before(PhysicsSystems::Writeback)
                    .in_set(SimSet::Physics),
            )
            .add_systems(FixedUpdate, collision_damage.in_set(SimSet::Damage));
    }
}

/// Probe the ground under every vehicle and sample the liquid it is in.
///
/// Runs before the drive system so everything downstream reads one
/// consistent answer for the tick.
pub fn sense_ground(
    spatial: SpatialQuery,
    surfaces: SurfaceQuery,
    volumes: Query<&WaterVolume>,
    table: Option<Res<VehicleTable>>,
    mut vehicles: Query<(&Vehicle, &Transform, &mut GroundState)>,
) {
    for (vehicle, transform, mut ground) in &mut vehicles {
        let mode = table
            .as_ref()
            .and_then(|table| table.kind(&vehicle.kind))
            .and_then(|def| def.mode(vehicle.form));

        *ground = probe_under(&spatial, &surfaces, &volumes, transform, mode);
    }
}

/// The ground and liquid state under one hull.
fn probe_under(
    spatial: &SpatialQuery,
    surfaces: &SurfaceQuery,
    volumes: &Query<&WaterVolume>,
    transform: &Transform,
    mode: Option<&ModeTuning>,
) -> GroundState {
    let mut state = GroundState {
        // The water sample is independent of the ground: `probe_ground`
        // filters to `GameLayer::Level` and water is a sensor on its own
        // layer, so a sub in a pool sees both at once.
        in_water: water_at(volumes, transform.translation),
        ..GroundState::airborne()
    };

    let Some(mode) = mode else {
        return state;
    };

    let mut normal = Vec3::ZERO;
    let mut hits = 0u32;
    let mut deepest = f32::MAX;
    let mut points = Vec::with_capacity(mode.suspension.rays.len());
    let to_hull = transform.rotation.inverse();
    let down = transform.rotation * Vec3::NEG_Y;

    for ray in &mode.suspension.rays {
        // The same geometry `suspension` casts with, so "grounded" means
        // exactly "a spring is touching something" — including the lift,
        // or the two would disagree about whether the hull is on the ground.
        // Along the hull's own down, as the springs are: a world-down ray
        // needs 1/cos of the slope more reach, and on a 40° slope misses
        // ground the springs are standing on.
        let origin = transform.transform_point(*ray + Vec3::Y * mode.suspension.ray_lift);
        let probe = GroundProbe {
            origin,
            dir: down,
            max: mode.suspension.max_len,
        };
        let Some(hit) = probe_ground(spatial, surfaces, probe) else {
            continue;
        };

        hits += 1;
        normal += hit.normal;
        points.push(to_hull * (hit.point - transform.translation));

        if hit.distance < deepest {
            deepest = hit.distance;
            state.soil = Some(hit.soil);
            state.support = Some(hit.entity);
        }
    }

    if hits > 0 {
        state.grounded = true;
        state.normal = normal.normalize_or(Vec3::Y);
        state.contact = GroundContact::fit(&points, to_hull * state.normal);
    }

    state
}

/// Scale each vehicle's share of gravity by what it is standing in.
///
/// avian's global `Gravity` is the physics' and stays at `nom_gravity`; a
/// body in water gets a [`GravityScale`] instead, so the reader's three
/// gravities all come off one global (READER-SCHEMA §2.2).
pub fn apply_gravity_scale(
    tuning: Res<PhysicsTuning>,
    mut vehicles: Query<(&Vehicle, &GroundState, &mut GravityScale)>,
) {
    for (vehicle, ground, mut scale) in &mut vehicles {
        let wanted = gravity_scale(vehicle.form, ground, &tuning);
        if scale.0 != wanted {
            scale.0 = wanted;
        }
    }
}

/// Turn the turret toward what the player is pointing at.
///
/// Two paths, and which one runs depends on whether anything set
/// [`PlayerInput::aim_point`]:
///
///  - **A world point** (the cursor's ray, `bft_game::aim`'s `aim_point`): the
///    turret slews toward the yaw and pitch that hit it, capped at
///    [`TurretRate`]. For an arcing weapon the pitch is the ballistic
///    solution, so the *shell* lands on the cursor rather than the barrel
///    pointing at it.
///  - **A rate** (`turret_delta`): the fallback, and what an AI brain or a
///    gamepad writes. Integrated as before.
///
/// The slew is what stops the turret snapping: a step in the aim point is
/// crossed at a finite rate, so the barrel visibly travels.
pub fn aim_turret(
    time: Res<Time<Fixed>>,
    table: Option<Res<VehicleTable>>,
    weapons: Option<Res<crate::weapons::WeaponTable>>,
    mut vehicles: AimingVehicles,
    mut parts: TurretJoints,
    mut guns: GunJoints,
) {
    let dt = time.delta_secs();

    for (vehicle, input, hull, mut turret, loadout, rate, joints, pivot) in &mut vehicles {
        let limits = table
            .as_ref()
            .and_then(|table| table.kind(&vehicle.kind))
            .and_then(|def| def.mode(vehicle.form))
            .and_then(|mode| mode.gun_pitch)
            // // approximation: only the sub states `gun_pitch`. The other
            // forms get a symmetric range wide enough to aim up a cliff.
            .unwrap_or(DEFAULT_GUN_PITCH);

        match input.aim_point {
            Some(point) => {
                // The selected weapon decides whether the shot arcs. No
                // loadout or no table means the straight-line angle, which is
                // what a weaponless vehicle should do anyway.
                let (speed, gravity) = loadout
                    .and_then(|loadout| loadout.selected_ammo())
                    .zip(weapons.as_ref())
                    .and_then(|(ammo, table)| table.by_index(ammo.weapon))
                    .map(|def| (def.velocity.unwrap_or(0.0), def.gravity))
                    .unwrap_or((0.0, 0.0));

                if let Some((yaw, pitch)) = aim_angles(
                    hull,
                    pivot.map_or(Vec3::ZERO, |pivot| pivot.0),
                    point,
                    speed,
                    gravity,
                ) {
                    let rate = rate.copied().unwrap_or_default();
                    turret.yaw = wrap_angle(step_angle(turret.yaw, yaw, rate.yaw * dt));
                    turret.pitch = step_angle(turret.pitch, pitch, rate.pitch * dt)
                        .clamp(limits[0], limits[1]);
                }
            }
            None => {
                turret.yaw = wrap_angle(turret.yaw + input.turret_delta.x);
                turret.pitch = (turret.pitch + input.turret_delta.y).clamp(limits[0], limits[1]);
            }
        }

        // Write the aim onto the joints the vehicle recorded, so the model —
        // and the camera, which looks along it — follows without recomposing
        // the hull rotation. Neither joint is the hull, so the queries stay
        // disjoint.
        //
        // Read off `AimJoints` rather than scanned for: on a real model the
        // turret is four levels down, where a scan of the hull's direct
        // children would silently find nothing.
        let Some(joints) = joints else {
            continue;
        };

        match joints.pitch {
            // Two joints: yaw the ring, pitch the barrel. Pitching the ring
            // would tip the doors and the turret polys with it.
            Some(gun) => {
                if let Ok(mut transform) = parts.get_mut(joints.yaw) {
                    transform.rotation = Quat::from_rotation_y(turret.yaw);
                }
                if let Ok(mut gun) = guns.get_mut(gun) {
                    gun.rotation = Quat::from_rotation_x(turret.pitch);
                }
            }
            // One joint: the stand-in, or any vehicle whose model
            // has no separate barrel. It carries both rotations, because the
            // camera looks along it and has to point up.
            None => {
                if let Ok(mut transform) = parts.get_mut(joints.yaw) {
                    transform.rotation =
                        Quat::from_euler(EulerRot::YXZ, turret.yaw, turret.pitch, 0.0);
                }
            }
        }
    }
}

/// The turret rings [`aim_turret`] yaws.
type TurretJoints<'w, 's> =
    Query<'w, 's, &'static mut Transform, (With<TurretPart>, Without<Vehicle>)>;

/// The barrels [`aim_turret`] pitches, disjoint from the rings above them.
type GunJoints<'w, 's> =
    Query<'w, 's, &'static mut Transform, (With<GunPart>, Without<TurretPart>, Without<Vehicle>)>;

/// Everything [`aim_turret`] needs off a vehicle to point its gun.
///
/// A named type because the tuple is eight wide: the hull and its form for
/// the pitch limits, the input for the aim point, the turret to write, the
/// loadout to know whether the shot arcs, the rate to cap the slew, the
/// joints to turn, and the pivot to measure the angles from.
type AimingVehicles<'w, 's> = Query<
    'w,
    's,
    (
        &'static Vehicle,
        &'static PlayerInput,
        &'static Transform,
        &'static mut Turret,
        Option<&'static crate::weapons::WeaponLoadout>,
        Option<&'static TurretRate>,
        Option<&'static AimJoints>,
        Option<&'static AimPivot>,
    ),
>;

/// How fast a turret may turn, in radians per second.
///
/// // approximation: no reader field corresponds. `vehicle.zrd` states the
/// *hull's* `turns` and `ai.zrd` a turret's `FIRE_RATE`, but nothing says how
/// fast a barrel traverses. These are quick enough to follow a cursor across
/// the screen without feeling sticky, and slow enough that a flick of the
/// mouse visibly travels rather than teleporting.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct TurretRate {
    /// Traverse, rad/s.
    pub yaw: f32,
    /// Elevation, rad/s.
    pub pitch: f32,
}

impl Default for TurretRate {
    fn default() -> TurretRate {
        TurretRate {
            yaw: 2.5,
            pitch: 1.5,
        }
    }
}

/// The turret angles, relative to the hull, that point at a world position.
///
/// With a `gravity` above zero the pitch is the ballistic solution for a
/// projectile of muzzle speed `speed` — the angle whose *arc* passes through
/// the target — so a mortar elevates rather than aiming flat and falling
/// short. Otherwise it is the straight line to the target.
///
/// `from` is the hull-space point the barrel pivots about ([`AimPivot`]).
/// Measured from the hull's origin instead, the barrel points *parallel* to
/// the line that hits the target, and misses by the pivot's offset: on the
/// player, 2.25 m above the hull's centre.
///
/// Returns `None` when the target cannot be reached at that speed, so the
/// caller can hold the turret where it is and the HUD can say so.
pub fn aim_angles(
    hull: &Transform,
    from: Vec3,
    target: Vec3,
    speed: f32,
    gravity: f32,
) -> Option<(f32, f32)> {
    // Into hull space: the turret's angles are relative to the hull, and
    // `inverse` handles a vehicle that is itself pitched over a rise.
    let local = hull.rotation.inverse() * (target - hull.translation) - from;

    // The hull's forward is −Z, so a target dead ahead is yaw zero.
    let yaw = (-local.x).atan2(-local.z);

    let pitch = if gravity > 0.0 && speed > 0.0 {
        crate::weapons::launch_pitch(local, speed, gravity)?
    } else {
        let flat = local.xz().length();
        local.y.atan2(flat)
    };

    Some((wrap_angle(yaw), pitch))
}

/// Move `current` toward `wanted` by at most `max_step`, the short way round.
///
/// Stops exactly on the target rather than overshooting and oscillating by
/// one step for ever, which is what a bare `current + step * signum` does.
pub fn step_angle(current: f32, wanted: f32, max_step: f32) -> f32 {
    let error = shortest_angle(wanted - current);

    if error.abs() <= max_step {
        wanted
    } else {
        current + max_step * error.signum()
    }
}

/// The signed shortest way round from one angle to another.
fn shortest_angle(angle: f32) -> f32 {
    let wrapped = angle.rem_euclid(std::f32::consts::TAU);

    if wrapped > std::f32::consts::PI {
        wrapped - std::f32::consts::TAU
    } else {
        wrapped
    }
}

/// The turret pitch range for a form with no `gun_pitch`, in radians.
///
/// // approximation: the sub's `[-0.1736, 0.5]` is the only stated pair
/// (−10°…+29°). A ground form needs more elevation to shoot up a slope, so
/// this is wider and still short of vertical.
pub const DEFAULT_GUN_PITCH: [f32; 2] = [-0.4, 0.7];

/// Keep an angle in −π…π, so a turret spun in one direction does not grow a
/// number that loses precision.
fn wrap_angle(angle: f32) -> f32 {
    let wrapped = angle.rem_euclid(std::f32::consts::TAU);

    if wrapped > std::f32::consts::PI {
        wrapped - std::f32::consts::TAU
    } else {
        wrapped
    }
}

/// Manual and automatic form switching.
///
/// Manual: `next_form` cycles the unlocked forms in [`VehicleForm::ALL`]
/// order, skipping any the transition graph does not allow from here and any
/// the world does not allow (Sub out of shallow water).
///
/// Automatic: entering water in Track form with Amphib unlocked switches to
/// Amphib, and leaving it switches back — which is what makes
/// "transformations function as barriers" work (DESIGN §7.2, docs/data/water.md).
pub fn switch_forms(
    table: Option<Res<VehicleTable>>,
    mut vehicles: Query<(Entity, &mut Vehicle, &PlayerInput, &GroundState)>,
    mut changed: MessageWriter<FormChanged>,
) {
    for (entity, mut vehicle, input, ground) in &mut vehicles {
        let def = table.as_ref().and_then(|table| table.kind(&vehicle.kind));

        let wanted = if input.next_form {
            next_form(&vehicle, ground, def)
        } else {
            automatic_form(&vehicle, ground, def)
        };

        let Some(to) = wanted else {
            continue;
        };
        if to == vehicle.form {
            continue;
        }

        let from = vehicle.form;
        let anims = def
            .and_then(|def| def.mode(from))
            .and_then(|mode| mode.transitions.get(&(from, to)))
            .cloned()
            .unwrap_or_default();

        vehicle.form = to;
        changed.write(FormChanged {
            entity,
            from,
            to,
            anims,
        });
    }
}

/// The next form `next_form` should cycle to, if any.
fn next_form(
    vehicle: &Vehicle,
    ground: &GroundState,
    def: Option<&VehicleDef>,
) -> Option<VehicleForm> {
    let start = VehicleForm::ALL
        .iter()
        .position(|form| *form == vehicle.form)
        .unwrap_or(0);

    (1..=VehicleForm::ALL.len())
        .map(|step| VehicleForm::ALL[(start + step) % VehicleForm::ALL.len()])
        .find(|form| form_reachable(vehicle, *form, ground, def))
}

/// Whether a form may be entered from where the vehicle is now.
fn form_reachable(
    vehicle: &Vehicle,
    form: VehicleForm,
    ground: &GroundState,
    def: Option<&VehicleDef>,
) -> bool {
    if form == vehicle.form || !vehicle.unlocked.has(form) {
        return false;
    }
    if !transition_allowed(vehicle.form, form) {
        return false;
    }
    // A kind can only be in a form it has a mode block for.
    if def.is_some_and(|def| def.mode(form).is_none()) {
        return false;
    }
    // The sub needs water with a floor under it (docs/data/water.md): the shallow
    // sea has nothing to dive into.
    if form == VehicleForm::Sub && !ground.is_divable() {
        return false;
    }

    true
}

/// The automatic water transition, if it applies this tick.
fn automatic_form(
    vehicle: &Vehicle,
    ground: &GroundState,
    def: Option<&VehicleDef>,
) -> Option<VehicleForm> {
    let mode = def.and_then(|def| def.mode(vehicle.form));

    match vehicle.form {
        // Into the water: only if the *mode* says so. `ok_to_amphib` is 1 on
        // the player's track mode and 0 on every enemy's, which is exactly
        // the rule.
        VehicleForm::Track
            if ground.is_in_water()
                && vehicle.unlocked.has(VehicleForm::Amphib)
                && mode.is_some_and(|mode| mode.ok_to_amphib) =>
        {
            Some(VehicleForm::Amphib)
        }
        // Out of the water, back to tracks. Not from Sub: surfacing is the
        // player's decision, and Sub→Track is not an allowed transition
        // anyway.
        VehicleForm::Amphib if !ground.is_in_water() && ground.grounded => Some(VehicleForm::Track),
        _ => None,
    }
}

/// Everything the drive system reads and writes per vehicle.
///
/// A `QueryData` struct rather than a tuple: `Forces` is itself query data
/// and the tuple form is long enough to say nothing about what any item is
/// for.
///
/// **The pose and velocity come from `Forces`, not from separate
/// components.** `Forces` already borrows `Position`, `Rotation`,
/// `LinearVelocity` and `AngularVelocity` mutably, so naming any of them
/// again here is a query conflict Bevy panics on at schedule build —
/// and `Position`/`Rotation` are avian's authoritative pose anyway, with
/// `Transform` written back from them after the step.
#[derive(QueryData)]
#[query_data(mutable)]
pub struct DrivenVehicle {
    vehicle: &'static Vehicle,
    input: &'static PlayerInput,
    ground: &'static GroundState,
    traction: &'static mut Traction,
    forces: Forces,
    mass: &'static ComputedMass,
    angular_damping: Option<&'static AngularDamping>,
}

/// The controller proper: suspension, drive, steering, grip, and the water
/// and sub regimes.
pub fn drive(
    time: Res<Time<Fixed>>,
    spatial: SpatialQuery,
    surfaces: SurfaceQuery,
    table: Option<Res<VehicleTable>>,
    mut vehicles: Query<DrivenVehicle>,
) {
    let Some(table) = table else {
        return;
    };
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }

    for mut driven in &mut vehicles {
        let Some(mode) = table
            .kind(&driven.vehicle.kind)
            .and_then(|def| def.mode(driven.vehicle.form))
        else {
            continue;
        };

        let body = Body {
            position: driven.forces.position().0,
            rotation: driven.forces.rotation().0,
            velocity: driven.forces.linear_velocity(),
            angular: driven.forces.angular_velocity(),
            mass: driven.mass.value(),
            angular_damping: driven.angular_damping.map_or(0.0, |damping| damping.0),
        };
        let (input, ground, form) = (driven.input, driven.ground, driven.vehicle.form);
        let traction = *driven.traction;
        let forces = &mut driven.forces;

        let updated = match form {
            VehicleForm::Sub if ground.is_divable() => {
                sub_forces(forces, &body, input, ground, mode, dt, traction)
            }
            form => {
                if form.is_surface_form() && ground.is_in_water() {
                    buoyancy(forces, &body, ground, mode);
                } else {
                    suspension(forces, &body, mode, &spatial, &surfaces);
                }
                ground_forces(forces, &body, input, ground, mode, dt, traction)
            }
        };
        upright(forces, &body, ground, mode);

        // Write the latched slide state back, so the next tick knows whether
        // the tracks are still loose.
        *driven.traction = updated;
    }
}

/// The parts of a rigid body the force helpers need, gathered once.
///
/// avian's `Position`/`Rotation`, not the `Transform`: the solver reads and
/// writes those, and the transform is derived from them after the step, so a
/// force computed from the transform is a tick stale.
struct Body {
    position: Vec3,
    rotation: Quat,
    velocity: Vec3,
    angular: Vec3,
    mass: f32,
    /// avian's own [`AngularDamping`], per second, which the solver applies
    /// on top of anything the controller does.
    angular_damping: f32,
}

impl Body {
    /// Forward in world space, which is the hull's −z (Bevy's convention,
    /// and the one the levels use — docs/data/coordinates.md).
    fn forward(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Z
    }

    fn right(&self) -> Vec3 {
        self.rotation * Vec3::X
    }

    fn up(&self) -> Vec3 {
        self.rotation * Vec3::Y
    }

    /// A hull-space point in world space.
    fn point(&self, local: Vec3) -> Vec3 {
        self.position + self.rotation * local
    }

    /// Speed along the hull's forward axis, signed.
    fn forward_speed(&self) -> f32 {
        self.velocity.dot(self.forward())
    }
}

/// Spring-damper forces at each suspension ray.
///
/// Each ray is cast from its `platform` corner, raised by the rest length, in
/// the hull's down direction — not world down, so a hull on a slope pushes
/// along its own axis and does not slide sideways off a spring.
fn suspension(
    forces: &mut ForcesItem<'_, '_>,
    body: &Body,
    mode: &ModeTuning,
    spatial: &SpatialQuery,
    surfaces: &SurfaceQuery,
) {
    let up = body.up();
    let tuning = &mode.suspension;

    for ray in &tuning.rays {
        // The cast starts `ray_lift` above the `platform` corner — at the
        // collider's floor, not at the contact patch — because a ray cast
        // from the patch can be pushed below the terrain on a hard landing
        // and then finds nothing at all. `max_len` includes the lift, so
        // compression at a given height is unchanged. See
        // `SuspensionTuning::ray_lift`.
        //
        // Casting from inside the collider is safe: the probe filters to
        // `GameLayer::Level` and the hull is on `GameLayer::Vehicle`, so it
        // never hits itself.
        let origin = body.point(*ray + Vec3::Y * tuning.ray_lift);
        let probe = GroundProbe {
            origin,
            dir: -up,
            max: tuning.max_len,
        };

        let Some(hit) = probe_ground(spatial, surfaces, probe) else {
            continue;
        };

        let compression = tuning.max_len - hit.distance;
        if compression <= 0.0 {
            continue;
        }

        // The damper acts on the velocity of this corner along the ray, not
        // on the body's centre velocity, or a pitching hull is undamped.
        let corner = body.velocity + body.angular.cross(origin - body.position);
        let closing = corner.dot(up);

        let force = tuning.stiffness * compression - tuning.damping * closing;
        // A suspension may push, never pull: a spring that pulls the hull
        // down onto a ledge is how a raycast vehicle sticks to ceilings.
        if force <= 0.0 {
            continue;
        }

        forces.apply_force_at_point(up * force, origin);
    }
}

/// Swing the hull's up axis back toward level: world up in the air, the
/// ground's normal on it.
///
/// The original's vehicles never tumble. Its tanks leave a ramp and settle
/// level in the air, and a collision rocks them rather than rolling them —
/// the behaviour of a sim that steers orientation itself and integrates only
/// the trajectory. This keeps avian's rigid body, and so its collision
/// response, and adds that steering on top as a critically damped spring on
/// the lean. [`limit_tilt`] is the hard stop behind it.
///
/// An angular *acceleration*, like the steering in [`ground_forces`] and
/// for the same reason: the data states no inertia, and a rate in rad/s
/// should mean the same on a 0.8 t drone as on a 4.2 t trench digger.
/// Not scaled by `air_control`: that is the driver's authority, and this
/// is the vehicle's own.
fn upright(forces: &mut ForcesItem<'_, '_>, body: &Body, ground: &GroundState, mode: &ModeTuning) {
    // Ground steeper than the tilt limit — a wall the rays caught with the
    // hull already leaning into it — is not a surface to align with.
    let on_ground = ground.grounded && ground.normal.y >= mode.max_tilt.cos();
    let (target, rate) = if on_ground {
        (ground.normal, mode.upright_rate_ground)
    } else {
        (Vec3::Y, mode.upright_rate_air)
    };
    if rate <= 0.0 {
        return;
    }

    let up = body.up();
    let axis = up.cross(target);
    let angle = axis.length().atan2(up.dot(target));
    // Upside down exactly, the cross product vanishes; any horizontal axis
    // turns it back, and the hull's own right is one.
    let axis = axis.try_normalize().unwrap_or_else(|| body.right());

    // Only the lean is damped. The spin about the hull's own up axis is the
    // steering's, and damping it here would fight `turns`.
    let lean_rate = body.angular - up * body.angular.dot(up);
    // Critical in total: the body's `AngularDamping` from `a_damping`
    // already takes some out, and doubling up would slow the levelling
    // well past the rate asked for.
    let damping = (2.0 * rate - body.angular_damping).max(0.0);

    forces.apply_angular_acceleration(axis * angle * rate * rate - lean_rate * damping);
}

/// Put back any hull leaning past its mode's `max_tilt`, after the step.
///
/// [`upright`] is the feel; this is the guarantee. A hard enough hit turns a
/// hull further in one step than a spring can answer, and a tank on its back
/// cannot drive, so after avian's solver and before its writeback any lean
/// beyond the limit is rotated back onto it — about the centre of mass, so
/// the hull does not swing into the ground — and the part of the spin still
/// carrying it over is taken out.
///
/// Measured from world up, not from the ground, so a climb up a slope that
/// is well inside the limit is left alone. Parked hulls are the cinematics'
/// to pose.
pub fn limit_tilt(
    table: Option<Res<VehicleTable>>,
    mut hulls: Query<
        (
            &Vehicle,
            &mut Position,
            &mut Rotation,
            &mut AngularVelocity,
            &ComputedCenterOfMass,
        ),
        Without<crate::cinematic::Parked>,
    >,
) {
    let Some(table) = table else {
        return;
    };

    for (vehicle, mut position, mut rotation, mut angular, com) in &mut hulls {
        let Some(mode) = table
            .kind(&vehicle.kind)
            .and_then(|def| def.mode(vehicle.form))
        else {
            continue;
        };

        let Some((corrected, axis)) = clamp_tilt(rotation.0, mode.max_tilt) else {
            continue;
        };

        // Pivot on the centre of mass, where the solver pivots too.
        let pivot = position.0 + rotation.0 * com.0;
        position.0 = pivot - corrected * com.0;
        rotation.0 = corrected;

        // `axis` turns the hull toward upright; spin the other way about it
        // is what is still tipping it over.
        let over = angular.0.dot(axis);
        if over < 0.0 {
            angular.0 -= axis * over;
        }
    }
}

/// A rotation leaning more than `max_tilt` from world up, turned back onto
/// the limit, and the axis it was turned about. `None` when it is within.
pub fn clamp_tilt(rotation: Quat, max_tilt: f32) -> Option<(Quat, Vec3)> {
    let up = rotation * Vec3::Y;
    let tilt = up.y.clamp(-1.0, 1.0).acos();
    if tilt <= max_tilt {
        return None;
    }

    // Upside down exactly, any horizontal axis will do; the hull's right is
    // the one that rolls it back onto its tracks.
    let axis = up
        .cross(Vec3::Y)
        .try_normalize()
        .unwrap_or_else(|| rotation * Vec3::X);
    let corrected = (Quat::from_axis_angle(axis, tilt - max_tilt) * rotation).normalize();
    Some((corrected, axis))
}

/// Hold a surface form at the liquid surface.
///
/// A spring toward `surface_y + mode_alt` rather than an Archimedes
/// integration: the data states a ride-height offset per mode (−0.1 for
/// amphib) and nothing about displacement, so the offset is the target.
fn buoyancy(forces: &mut ForcesItem<'_, '_>, body: &Body, ground: &GroundState, mode: &ModeTuning) {
    let Some(water) = ground.in_water else {
        return;
    };

    let target = water.surface_y + mode.mode_alt;
    let error = target - body.position.y;

    let acceleration = BUOYANCY_STIFFNESS * error - BUOYANCY_DAMPING * body.velocity.y;
    forces.apply_force(Vec3::Y * acceleration * body.mass);

    // Bobbing, from the mode's `*_wave` parameters, is not here: it is a
    // visual oscillation and applying it as a force here would
    // fight the spring above.
}

/// Engine, steering and lateral grip, in the hull frame.
fn ground_forces(
    forces: &mut ForcesItem<'_, '_>,
    body: &Body,
    input: &PlayerInput,
    ground: &GroundState,
    mode: &ModeTuning,
    dt: f32,
    traction: Traction,
) -> Traction {
    // Off the ground (and not floating), authority drops to `air_control`.
    let floating = ground.is_in_water() && mode.mode_alt <= 0.5;
    let authority = if ground.grounded || floating {
        1.0
    } else {
        mode.air_control
    };
    let soil = soil_speed_multiplier(mode, ground.soil);

    // Drive. `rates[0]` forward, `rates[1]` in reverse, both scaled by the
    // soil. The clamp is on speed, not force, so a downhill run does not
    // accelerate for ever.
    let speed = body.forward_speed();
    let limit = if input.throttle >= 0.0 {
        mode.max_speed * soil
    } else {
        mode.reverse_speed * soil
    };

    if input.throttle.abs() > f32::EPSILON && speed.abs() < limit {
        let force = body.forward() * input.throttle * mode.engine_force * authority * soil;
        forces.apply_force(force);
    }

    // Brake: no throttle and moving means the drivetrain drags, which is what
    // `rate_damping` is for. An acceleration for the same reason as the
    // steering: `rate_damping` is a per-second rate, not a force.
    if input.throttle.abs() <= f32::EPSILON {
        let damping = mode.rate_damping[0].max(0.0);
        forces.apply_linear_acceleration(-body.forward() * speed * damping * authority);
    }

    // Steering: a yaw *rate* target, `turns[0]` stopped and `turns[1]`
    // moving, closed at `turn_damping`.
    //
    // As an angular *acceleration*, not a torque: a torque would have to be
    // divided by the hull's moment of inertia to mean anything, and the
    // inertia of the box we approximate the hull with is not a number the
    // data states. `turns` is in rad/s and `turn_damping` is how fast the
    // error is closed, so `error * turn_damping` is already rad/s² and the
    // vehicle turns at the stated rate whatever it weighs — which is what an
    // arcade controller wants and what "turns in place when stopped" needs.
    let turn_rate = if speed.abs() < mode.stationary_speed {
        mode.turns[0]
    } else {
        mode.turns[1]
    };
    let up = body.up();
    let wanted_yaw = -input.steer * turn_rate * authority;
    let actual_yaw = body.angular.dot(up);
    let yaw_error = wanted_yaw - actual_yaw;

    forces.apply_angular_acceleration(up * yaw_error * mode.turn_damping);

    // Lateral grip: two coefficients and two thresholds, so a skid commits
    // rather than catching itself every tick. See `lateral_impulse`.
    let right = body.right();
    let sideways = body.velocity.dot(right);
    let (impulse, traction) =
        lateral_impulse(sideways, body.mass, mode, soil, authority, dt, traction);

    forces.apply_linear_impulse(-right * impulse);

    traction
}

/// How much sideways momentum the tracks take out this tick, and whether
/// they are still gripping.
///
/// # Static and kinetic grip
///
/// Traction has two coefficients, not one. It takes more force to break a
/// contact patch loose (**static**) than to keep it sliding (**kinetic**),
/// and the gap between them is what makes a skid commit: once traction is
/// gone the vehicle keeps sliding even though the force that would now be
/// needed to hold it is lower than the force that broke it.
///
/// A single-threshold model — what this was — re-grips the instant lateral
/// speed dips below its limit, so the vehicle catches itself every few ticks
/// and stays pointable all the way through what should be a slide. That is
/// the "still feels very steerable" symptom, and no amount of lowering the
/// single threshold fixes it; it makes the vehicle loose everywhere instead
/// of committed anywhere.
///
/// So there are two thresholds:
///
/// - **Breaking loose:** while gripping, the tracks can supply
///   `friction[0]` worth of lateral force. Ask for more and traction goes.
/// - **Re-gripping:** while sliding, they supply only `friction[1]`, and the
///   tracks do not bite again until the slide has decayed to
///   [`REGRIP_FRACTION`] of the speed that broke it.
///
/// # Reading `friction` as static and kinetic
///
/// READER-SCHEMA §2.1 reads `friction` as `[longitudinal, lateral, rolling]`
/// with an (I) confidence. **That is probably wrong**, and this reads it as
/// `[static, kinetic, rolling]` instead. The evidence, over all five mode
/// blocks in the game that carry the key:
///
/// | | `[0]` | `[1]` | `[1]/[0]` |
/// |---|---|---|---|
/// | `bft` track | 150 | 80 | 0.53 |
/// | `bft` amphib | 140 | 90 | 0.64 |
/// | `bft` hover | 80 | 40 | 0.50 |
/// | `bft` sub | 300 | 200 | 0.67 |
/// | `subt` sub | 120 | 80 | 0.67 |
///
/// `[0] > [1]` in every case and the ratio never leaves 0.50–0.67, which is
/// exactly where kinetic/static friction sits physically. Under the old
/// reading the numbers say a *submarine* has a longitudinal friction of 300,
/// which no amount of squinting makes sense of. Five samples is not proof,
/// so this stays an inference — but it is a better-supported one, it gives
/// the two coefficients the behaviour needs, and it predicts the feel the
/// original has. Recorded in docs/data/reader.md.
///
/// Returns the impulse to apply against the slide, and the traction state
/// after this tick.
pub fn lateral_impulse(
    sideways: f32,
    mass: f32,
    mode: &ModeTuning,
    soil: f32,
    authority: f32,
    dt: f32,
    traction: Traction,
) -> (f32, Traction) {
    let speed = sideways.abs();

    // What it would take to stop the slide dead this tick.
    let wanted = speed * mass;

    // The two limits, static and kinetic.
    let scale = mode.grip_force_scale * mass * soil * authority * dt;
    let static_limit = mode.static_grip * scale;
    let kinetic_limit = mode.lateral_grip * scale;

    let mut next = Traction {
        sliding: traction.sliding,
        slip_speed: sideways,
        broke_at: traction.broke_at,
    };

    if traction.sliding {
        // Sliding: the tracks bite again only once the slide has decayed to
        // a fraction of the speed that broke it. That gap is the hysteresis,
        // and it is what makes the skid commit instead of flickering back
        // into grip the moment the load dips.
        if speed <= traction.broke_at * mode.regrip_fraction {
            next.sliding = false;
            next.broke_at = 0.0;
            return (wanted.min(static_limit) * sideways.signum(), next);
        }

        // While sliding only kinetic friction acts, which is the lower of
        // the two — so the slide persists rather than being hauled straight
        // back in line.
        return (wanted.min(kinetic_limit) * sideways.signum(), next);
    }

    // Gripping: hold until the load exceeds what static friction can carry.
    if wanted > static_limit {
        next.sliding = true;
        next.broke_at = speed;
        return (kinetic_limit * sideways.signum(), next);
    }

    (wanted * sideways.signum(), next)
}

/// Turns a form's grip into a lateral force per unit mass.
///
/// // approximation: `friction` is 40–300 in the data, which is not a
/// friction coefficient, and the reader states no unit. Through
/// [`LATERAL_GRIP_DIVISOR`](tuning::LATERAL_GRIP_DIVISOR) the player's tracks
/// come out at 15 static and 8 kinetic, so this scale puts the force they
/// carry at 90 m/s² before breaking loose and 48 while sliding — a little
/// over two, and a little over one, times the reader's own gravity.
///
/// Chosen so breakaway lands **inside** the usable speed range. A turn at
/// `turns[1]` needs `v × ω` of lateral acceleration, so the tracks hold a
/// full-rate turn up to 90 / 3.5 ≈ 26 m/s of the BFT's 48, and hover to 16
/// of its 20. The water forms carry more grip while being far slower, so
/// their limits sit above their own top speeds and they never slide.
///
/// The breaking threshold is `friction[0]`, about 1.9 times `friction[1]`,
/// so a scale calibrated against `friction[1]` (12) would put breakaway
/// above top speed for all four forms and nothing would skid at all.
///
/// This is the default of [`ModeTuning::grip_force_scale`], the first number
/// to reach for in `assets/tuning/vehicle.ron` (`grip_force_scale`) if the
/// slide comes on too early or too late.
pub const GRIP_FORCE_SCALE: f32 = 6.0;

/// The sub: thrust along the hull, vertical thrust from `pitch`, clamped
/// between the floor and the surface.
fn sub_forces(
    forces: &mut ForcesItem<'_, '_>,
    body: &Body,
    input: &PlayerInput,
    ground: &GroundState,
    mode: &ModeTuning,
    dt: f32,
    traction: Traction,
) -> Traction {
    let Some(water) = ground.in_water else {
        return traction;
    };

    // Horizontal thrust and steering, as on the ground but with no
    // suspension holding it up.
    let traction = ground_forces(forces, body, input, ground, mode, dt, traction);

    // Depth: `pitch` is a velocity target, and the clamp is a hard wall at
    // both ends rather than a spring, so the sub cannot be flown out of the
    // water or into the sea floor.
    let y = body.position.y;
    let ceiling = water.surface_y + mode.mode_alt - SUB_MARGIN;
    let floor = water.floor_y.unwrap_or(y) + SUB_MARGIN;

    let mut wanted = input.pitch * SUB_VERTICAL_SPEED;
    if y >= ceiling && wanted > 0.0 {
        wanted = 0.0;
    }
    if y <= floor && wanted < 0.0 {
        wanted = 0.0;
    }

    // Push back in when outside the clamp, so a sub that surfaced through a
    // transition sinks to where it belongs instead of sitting on top.
    if y > ceiling {
        wanted = -(y - ceiling).min(SUB_VERTICAL_SPEED);
    } else if y < floor {
        wanted = (floor - y).min(SUB_VERTICAL_SPEED);
    }

    let error = wanted - body.velocity.y;
    forces.apply_force(Vec3::Y * error * body.mass / dt.max(f32::EPSILON) * SUB_DEPTH_RESPONSE);

    traction
}

/// How much of the depth-velocity error the sub corrects per tick.
///
/// // approximation: no reader field corresponds. Below 1 so the correction
/// is a firm push rather than a teleport, which would fight the solver.
pub const SUB_DEPTH_RESPONSE: f32 = 0.25;

/// The impulse below which a collision is a bump and not damage.
///
/// // approximation: `collision_d` is `[factor, threshold]` and the player's
/// threshold is 0, which cannot be a newton-second figure or every scrape
/// would hurt. The factor scales the impulse into damage and this is the
/// floor under it.
pub const MIN_COLLISION_IMPULSE: f32 = 200.0;

/// A vehicle ran into something hard enough to hurt: what `vehicle.zrd`'s
/// `collide` sound is for.
#[derive(Message, Debug, Clone, Copy, PartialEq)]
pub struct VehicleCollided {
    pub entity: Entity,
    /// Closing speed times mass, the same measure the damage floor uses.
    pub impulse: f32,
}

/// Report collisions hard enough to hurt.
///
/// It logs the damage rather than raising a `DamageEvent`, and writes
/// [`VehicleCollided`] for the sound.
pub fn collision_damage(
    mut collisions: MessageReader<CollisionStart>,
    mut collided: MessageWriter<VehicleCollided>,
    table: Option<Res<VehicleTable>>,
    vehicles: Query<(&Vehicle, &LinearVelocity, &ComputedMass)>,
) {
    let Some(table) = table else {
        return;
    };

    for collision in collisions.read() {
        for entity in [collision.collider1, collision.collider2] {
            let Ok((vehicle, velocity, mass)) = vehicles.get(entity) else {
                continue;
            };
            let Some(mode) = table
                .kind(&vehicle.kind)
                .and_then(|def| def.mode(vehicle.form))
            else {
                continue;
            };

            // An approximation of the contact impulse: avian reports contacts
            // separately and are not read here. Closing
            // speed times mass is the right order of magnitude for "did that
            // hurt".
            let impulse = velocity.0.length() * mass.value();
            if impulse < MIN_COLLISION_IMPULSE.max(mode.collision_d[1]) {
                continue;
            }

            debug!(
                "collision damage {:.1} on {entity} (impulse {impulse:.0})",
                impulse * mode.collision_d[0] / MIN_COLLISION_IMPULSE
            );
            collided.write(VehicleCollided { entity, impulse });
        }
    }
}

/// The collision layer a vehicle hull is on, re-exported for convenience.
pub const VEHICLE_LAYER: GameLayer = GameLayer::Vehicle;

/// Hand [`PlayerConstants`] to the physics' [`PhysicsTuning`].
///
/// Gravity, the water and quicksand gravities and the slope limit all come
/// from `player.zrd`, and the physics' `apply_gravity` carries the change
/// into avian's `Gravity` on the next tick. Never write `Gravity` directly.
pub fn apply_player_constants(commands: &mut Commands, player: &PlayerConstants) {
    commands.insert_resource(PhysicsTuning {
        gravity: player.gravity,
        water_gravity: player.water_gravity,
        quicksand_gravity: player.quicksand_gravity,
        max_slope_cos: player.max_slope_cos,
    });
}

/// The transition anim pair for a form change, for the anim VM.
pub fn transition_anims(
    def: &VehicleDef,
    from: VehicleForm,
    to: VehicleForm,
) -> Option<&[String; 2]> {
    def.mode(from)
        .and_then(|mode| mode.transitions.get(&(from, to)))
}

/// Every transition the player's data states, for a test or a debug panel.
pub fn stated_transitions(def: &VehicleDef) -> HashMap<(VehicleForm, VehicleForm), [String; 2]> {
    let mut all = HashMap::default();

    for mode in def.modes.values() {
        for (pair, anims) in &mode.transitions {
            all.insert(*pair, anims.clone());
        }
    }

    all
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hull at the origin facing −Z, the resting orientation.
    fn hull() -> Transform {
        Transform::from_xyz(0.0, 0.0, 0.0)
    }

    /// A target dead ahead is yaw zero: the hull's forward is −Z.
    #[test]
    fn a_target_ahead_needs_no_traverse() {
        let (yaw, pitch) = aim_angles(&hull(), Vec3::ZERO, Vec3::new(0.0, 0.0, -100.0), 0.0, 0.0)
            .expect("a straight weapon always solves");

        assert!(yaw.abs() < 1e-5, "{yaw}");
        assert!(pitch.abs() < 1e-5, "{pitch}");
    }

    /// Left and right traverse in opposite directions, and a target above
    /// the hull elevates.
    #[test]
    fn the_angles_have_the_signs_the_turret_expects() {
        let right = aim_angles(&hull(), Vec3::ZERO, Vec3::new(50.0, 0.0, -50.0), 0.0, 0.0)
            .expect("solves")
            .0;
        let left = aim_angles(&hull(), Vec3::ZERO, Vec3::new(-50.0, 0.0, -50.0), 0.0, 0.0)
            .expect("solves")
            .0;

        assert!(right < 0.0, "a target to the right is a negative yaw");
        assert!(left > 0.0, "and to the left a positive one");
        assert!((right + left).abs() < 1e-5, "and they are symmetric");

        let up = aim_angles(&hull(), Vec3::ZERO, Vec3::new(0.0, 50.0, -50.0), 0.0, 0.0)
            .expect("solves")
            .1;
        assert!(up > 0.0, "a target above the hull elevates the barrel");
    }

    /// The angles are relative to the *hull*: a vehicle that has turned needs
    /// a different traverse to point at the same world position.
    #[test]
    fn the_angles_are_relative_to_a_turned_hull() {
        let mut turned = hull();
        turned.rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);

        let target = Vec3::new(0.0, 0.0, -100.0);
        let straight = aim_angles(&hull(), Vec3::ZERO, target, 0.0, 0.0)
            .expect("solves")
            .0;
        let rotated = aim_angles(&turned, Vec3::ZERO, target, 0.0, 0.0)
            .expect("solves")
            .0;

        assert!(straight.abs() < 1e-5);
        // The hull turned a quarter turn, so the turret has to come back by
        // one to point at the same place.
        assert!(
            (rotated.abs() - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
            "{rotated}"
        );
    }

    /// The point of the whole exercise: an arcing weapon elevates above the
    /// straight line so the shell lands on the target rather than short.
    #[test]
    fn an_arcing_weapon_aims_higher_than_a_straight_one() {
        let target = Vec3::new(0.0, 0.0, -200.0);

        let straight = aim_angles(&hull(), Vec3::ZERO, target, 60.0, 0.0)
            .expect("solves")
            .1;
        let arcing = aim_angles(&hull(), Vec3::ZERO, target, 60.0, 9.8)
            .expect("200 m is in range")
            .1;

        assert!(straight.abs() < 1e-5, "a flat shot at a level target");
        assert!(
            arcing > straight,
            "the mortar must elevate: {arcing} vs {straight}"
        );
    }

    /// Out of range has no angle, so the caller holds the turret still
    /// rather than pointing it somewhere meaningless.
    #[test]
    fn an_unreachable_target_has_no_solution() {
        // Well past v²/g for this speed.
        let target = Vec3::new(0.0, 0.0, -5000.0);

        assert!(aim_angles(&hull(), Vec3::ZERO, target, 60.0, 9.8).is_none());
        // The same target is fine for a weapon that does not fall.
        assert!(aim_angles(&hull(), Vec3::ZERO, target, 60.0, 0.0).is_some());
    }

    /// The slew is what stops the turret snapping: a big step is crossed at
    /// the rate limit, and it takes as many ticks as the arithmetic says.
    #[test]
    fn the_turret_crosses_a_step_at_its_rate_rather_than_snapping() {
        let rate = TurretRate::default();
        let dt = 1.0 / 60.0;
        let target = 1.0f32;

        let after_one = step_angle(0.0, target, rate.yaw * dt);
        assert!(
            (after_one - rate.yaw * dt).abs() < 1e-6,
            "one tick moves exactly one step, got {after_one}"
        );

        // And it arrives, in about `target / rate` seconds.
        let mut yaw = 0.0;
        let mut ticks = 0;
        while (yaw - target).abs() > 1e-6 && ticks < 600 {
            yaw = step_angle(yaw, target, rate.yaw * dt);
            ticks += 1;
        }
        let expected = (target / rate.yaw / dt).ceil() as i32;
        assert_eq!(ticks, expected, "took {ticks} ticks, expected {expected}");
    }

    /// Arriving must be exact, not an overshoot that oscillates for ever.
    #[test]
    fn the_slew_settles_exactly_on_the_target() {
        let target = 0.5f32;
        // A step far larger than the remaining error.
        let yaw = step_angle(0.49, target, 1.0);

        assert_eq!(yaw, target);
    }

    /// The short way round: a turret at +170° asked for −170° turns 20°
    /// through the back rather than 340° the long way.
    #[test]
    fn the_turret_takes_the_short_way_round() {
        let current = 170f32.to_radians();
        let wanted = -170f32.to_radians();

        let stepped = step_angle(current, wanted, 0.1);

        assert!(
            stepped > current,
            "should have gone forward past pi, got {stepped}"
        );
    }

    // -----------------------------------------------------------------
    // The two joints
    // -----------------------------------------------------------------

    /// Build a world holding one vehicle aiming at `target`, with or without
    /// a `GunPart` under its `TurretPart`, and run [`aim_turret`] once.
    ///
    /// Returns the two joints' rotations, so a test can say which angle
    /// landed where.
    fn aim_once(target: Vec3, with_gun: bool) -> (Quat, Option<Quat>) {
        let mut app = App::new();
        app.insert_resource(Time::<Fixed>::default());

        // A full second, so the slew reaches the target rather than
        // creeping one step towards it: this is about *where* the angles
        // go, not how fast.
        app.world_mut()
            .resource_mut::<Time<Fixed>>()
            .advance_by(std::time::Duration::from_secs(1));

        let input = PlayerInput {
            aim_point: Some(target),
            ..default()
        };

        let hull = app
            .world_mut()
            .spawn((
                Vehicle::player(FormSet::track_only()),
                input,
                Transform::IDENTITY,
                Turret::default(),
            ))
            .id();

        // Nested the way the real model nests them — a spacer entity between
        // the hull and the ring — so a search of the hull's *direct* children,
        // which finds a stand-in and misses a four-deep model, fails here.
        let body = app
            .world_mut()
            .spawn((Transform::IDENTITY, ChildOf(hull)))
            .id();
        let turret = app
            .world_mut()
            .spawn((TurretPart, Transform::IDENTITY, ChildOf(body)))
            .id();

        let gun = with_gun.then(|| {
            app.world_mut()
                .spawn((GunPart, Transform::IDENTITY, ChildOf(turret)))
                .id()
        });

        app.world_mut().entity_mut(hull).insert(AimJoints {
            yaw: turret,
            pitch: gun,
        });

        app.add_systems(Update, aim_turret);
        app.update();

        (
            app.world()
                .get::<Transform>(turret)
                .expect("spawned")
                .rotation,
            gun.map(|gun| app.world().get::<Transform>(gun).expect("spawned").rotation),
        )
    }

    /// With a model, yaw goes on the turret ring and pitch on the barrel.
    ///
    /// The real subtree separates them (docs/data/nodes.md), and folding pitch
    /// into the ring would tip the turret's doors and polys with the gun.
    #[test]
    fn a_model_pitches_the_barrel_and_not_the_turret_ring() {
        // Up and to the left, so both angles are non-zero and distinguishable.
        let (turret, gun) = aim_once(Vec3::new(-50.0, 40.0, -50.0), true);
        let gun = gun.expect("this vehicle has a gun joint");

        let (yaw, ring_pitch, _) = turret.to_euler(EulerRot::YXZ);
        let (gun_yaw, pitch, _) = gun.to_euler(EulerRot::YXZ);

        assert!(
            yaw > 0.1,
            "the ring yaws towards a target on the left: {yaw}"
        );
        assert!(ring_pitch.abs() < 1e-5, "and does not pitch: {ring_pitch}");

        assert!(pitch > 0.1, "the barrel elevates: {pitch}");
        assert!(gun_yaw.abs() < 1e-5, "and does not yaw: {gun_yaw}");
    }

    /// The turret is not a direct child of the hull on a real model — it is
    /// four levels down — so a search of the hull's own children would find
    /// nothing and the gun would never move.
    ///
    /// `aim_once` nests a spacer entity for exactly this reason. This asserts
    /// the joint actually turned, which such a search would silently fail to
    /// do while every other assertion still passed.
    #[test]
    fn a_turret_nested_below_the_hull_still_receives_the_aim() {
        let (turret, _) = aim_once(Vec3::new(-50.0, 0.0, -50.0), true);

        let (yaw, _, _) = turret.to_euler(EulerRot::YXZ);
        assert!(
            yaw.abs() > 0.1,
            "a turret two levels below the hull should still be aimed, got {yaw}"
        );
    }

    /// Without a model the stand-in carries both, so a vehicle that never
    /// gets a subtree still points where it is aiming.
    #[test]
    fn a_vehicle_with_no_gun_joint_keeps_both_rotations_on_the_turret() {
        let (turret, gun) = aim_once(Vec3::new(-50.0, 40.0, -50.0), false);

        assert!(gun.is_none(), "there is no gun joint to write to");

        let (yaw, pitch, _) = turret.to_euler(EulerRot::YXZ);
        assert!(yaw > 0.1, "the stand-in yaws: {yaw}");
        assert!(pitch > 0.1, "and pitches: {pitch}");
    }
}
