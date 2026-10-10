//! avian3d on the fixed timestep, surface soil, and the ground probe.
//!
//! Three things live here:
//!
//! - [`PhysicsSetupPlugin`] puts avian's schedule inside
//!   [`SimSet::Physics`](crate::schedule::SimSet::Physics) and sets gravity
//!   from [`PhysicsTuning`];
//! - [`layers`] turns the collision matrix into `CollisionLayers`
//!   presets, so no caller writes a bitmask by hand;
//! - [`SurfaceInfo`] and [`probe_ground`] answer "what am I standing on",
//!   which is the question the vehicle asks every tick.
//!
//! # Soil is per triangle, not per entity
//!
//! A single Recoil model mixes water and rock polygons — m1's `water02` is
//! one node whose model carries both — so a collider's soil is a *list*, one
//! entry per collider triangle, and a hit resolves through the triangle
//! index. The order is [`bft_assets::geometry::material_per_triangle`]'s,
//! which is group by group and **not** polygon order; deriving it by hand
//! mis-attributes every triangle after the first material change
//! (docs/data/models.md).
//!
//! # Why the probe re-casts
//!
//! avian 0.7's [`RayHitData`] carries `entity`, `distance` and `normal` and
//! no triangle id, so a `SpatialQuery` hit alone cannot name a triangle.
//! [`probe_ground`] therefore casts once through the spatial query to find
//! *which* collider was hit, then re-casts that collider's shape through
//! parry — which does return `FeatureId::Face(index)` — to name the
//! triangle.

use avian3d::parry::shape::FeatureId;
use avian3d::prelude::*;
use bevy::prelude::*;
use bft_assets::materials::Soil;

use crate::level::GameLayer;
use crate::schedule::SimSet;

/// Gravity and the slope limit, as the reader states them.
///
/// The values are `player.zrd`'s (docs/data/water.md and docs/data/reader.md):
/// the original's floaty feel is **high** gravity plus strong engines, not low
/// gravity, so a 9.81 default would be wrong in a way that looks plausible. The
/// vehicle table rebuilds this resource from its `PlayerConstants` once it
/// parses the reader; until then [`Default`] is the measured table.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct PhysicsTuning {
    /// `nom_gravity`, in m/s².
    pub gravity: f32,
    /// `wat_gravity`: what a body in water falls at.
    pub water_gravity: f32,
    /// `qsd_gravity`: what a body in quicksand sinks at.
    pub quicksand_gravity: f32,
    /// The steepest rideable slope, as the cosine of its angle from up.
    /// The vehicle table replaces it from `player.zrd`'s max slope.
    pub max_slope_cos: f32,
}

impl Default for PhysicsTuning {
    fn default() -> Self {
        PhysicsTuning {
            gravity: 40.0,
            water_gravity: 9.0,
            quicksand_gravity: 3.0,
            // approximation: derived from `player.zrd`'s max slope, which
            // the vehicle table parses. 45° until it does.
            max_slope_cos: std::f32::consts::FRAC_1_SQRT_2,
        }
    }
}

/// Which soil each triangle of this entity's collider is made of.
///
/// Parallel to the collider's triangle list, so `soil_by_triangle[i]` is the
/// soil of triangle `i` — the index parry reports as `FeatureId::Face(i)`.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct SurfaceInfo {
    /// Index = triangle index in the collider.
    pub soil_by_triangle: Vec<Soil>,
    /// The soil of the most triangles, for a cheap answer when the exact
    /// triangle does not matter (a debug overlay, a whole-node query).
    pub dominant: Soil,
}

impl SurfaceInfo {
    /// Build from one soil per triangle, computing [`Self::dominant`].
    pub fn new(soil_by_triangle: Vec<Soil>) -> SurfaceInfo {
        SurfaceInfo {
            dominant: dominant_soil(&soil_by_triangle),
            soil_by_triangle,
        }
    }

    /// The soil of one triangle, or [`Soil::Default`] when the index is out
    /// of range — a collider rebuilt without its `SurfaceInfo` should read as
    /// ordinary ground rather than panic mid-tick.
    pub fn soil(&self, triangle: usize) -> Soil {
        self.soil_by_triangle
            .get(triangle)
            .copied()
            .unwrap_or_default()
    }

    /// Whether any triangle is a liquid, which is what the water pass looks
    /// for before doing any real work.
    pub fn has_liquid(&self) -> bool {
        self.soil_by_triangle.iter().any(is_liquid)
    }
}

/// Whether a soil is one of the three the reader keys physics on.
///
/// `Soil::Fire` is deliberately not here: its 24 polygons per level are the
/// shared weapon-effect templates (`fire1`, `fireblast`), not terrain
/// (docs/data/water.md).
pub fn is_liquid(soil: &Soil) -> bool {
    matches!(soil, Soil::Water | Soil::Lava | Soil::Quicksand)
}

/// The soil of the most triangles, `Default` for an empty list.
fn dominant_soil(soils: &[Soil]) -> Soil {
    let mut best = Soil::Default;
    let mut best_count = 0usize;

    for candidate in [
        Soil::Default,
        Soil::Water,
        Soil::Lava,
        Soil::Quicksand,
        Soil::Fire,
        Soil::Seafloor,
        Soil::Dirt,
        Soil::Mud,
        Soil::Grass,
        Soil::Concrete,
        Soil::Snow,
        Soil::Mech,
        Soil::Silt,
        Soil::NoSlip,
    ] {
        let count = soils.iter().filter(|soil| **soil == candidate).count();

        if count > best_count {
            best = candidate;
            best_count = count;
        }
    }

    best
}

/// The collision matrix, as `CollisionLayers` presets.
///
/// The table reads (row collides with column):
///
/// | | Level | Vehicle | Projectile | Pickup | Trigger | Water |
/// |---|---|---|---|---|---|---|
/// | Vehicle | yes | yes | yes | sensor | sensor | sensor |
/// | Projectile | yes | yes | no | no | no | sensor |
/// | Pickup | no | sensor | no | no | no | no |
///
/// "sensor" is an interaction that reports but does not push, which is the
/// [`Sensor`] component's job on the *other* body — the mask still has to
/// name the pair, or no contact is generated at all. So a pickup's filter
/// names `Vehicle` and the pickup entity carries `Sensor`.
pub mod layers {
    use super::*;

    /// Static level geometry: collides with everything that moves.
    pub fn level() -> CollisionLayers {
        CollisionLayers::new(
            GameLayer::Level,
            [GameLayer::Vehicle, GameLayer::Projectile, GameLayer::Level],
        )
    }

    /// A vehicle: solid against the world, sensing pickups, triggers, water.
    pub fn vehicle() -> CollisionLayers {
        CollisionLayers::new(
            GameLayer::Vehicle,
            [
                GameLayer::Level,
                GameLayer::Vehicle,
                GameLayer::Projectile,
                GameLayer::Pickup,
                GameLayer::Trigger,
                GameLayer::Water,
            ],
        )
    }

    /// A projectile: hits the world and vehicles, senses water, ignores its
    /// own kind (two rockets never collide with each other).
    pub fn projectile() -> CollisionLayers {
        CollisionLayers::new(
            GameLayer::Projectile,
            [GameLayer::Level, GameLayer::Vehicle, GameLayer::Water],
        )
    }

    /// A node's hit box ([`crate::level::HitBoxOf`]): weapon casts see it,
    /// nothing collides with it.
    pub fn hit_box() -> CollisionLayers {
        CollisionLayers::new(GameLayer::HitBox, GameLayer::Projectile)
    }

    /// A pickup: senses vehicles only, and nothing else sees it.
    pub fn pickup() -> CollisionLayers {
        CollisionLayers::new(GameLayer::Pickup, GameLayer::Vehicle)
    }

    /// A trigger volume: senses vehicles only.
    pub fn trigger() -> CollisionLayers {
        CollisionLayers::new(GameLayer::Trigger, GameLayer::Vehicle)
    }

    /// A liquid surface: sensed by vehicles and projectiles, never solid.
    pub fn water() -> CollisionLayers {
        CollisionLayers::new(
            GameLayer::Water,
            [GameLayer::Vehicle, GameLayer::Projectile],
        )
    }
}

/// A downward (or any-direction) ray for finding the ground.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundProbe {
    pub origin: Vec3,
    /// Need not be normalised; [`probe_ground`] normalises it and returns
    /// `None` for a zero direction rather than producing a NaN.
    pub dir: Vec3,
    /// How far to look, in metres.
    pub max: f32,
}

impl GroundProbe {
    /// Straight down from `origin`, the probe every suspension ray wants.
    pub fn down(origin: Vec3, max: f32) -> GroundProbe {
        GroundProbe {
            origin,
            dir: Vec3::NEG_Y,
            max,
        }
    }
}

/// What a [`GroundProbe`] found.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GroundHit {
    pub point: Vec3,
    pub normal: Vec3,
    pub distance: f32,
    /// The soil of the exact triangle hit, or [`Soil::Default`] when the
    /// collider carries no [`SurfaceInfo`].
    pub soil: Soil,
    pub entity: Entity,
}

/// What [`probe_ground`] needs to name the triangle a hit landed on.
///
/// A `QueryData` alias rather than three loose arguments: the probe has to
/// re-cast the hit collider in its own local space, so it needs the shape and
/// the transform as well as the soil list.
pub type SurfaceQuery<'w, 's> = Query<
    'w,
    's,
    (
        &'static Collider,
        &'static GlobalTransform,
        &'static SurfaceInfo,
    ),
>;

/// Cast a ray at the level and report where it hit and what that is made of.
///
/// Two casts, for the reason in the module docs: avian's [`RayHitData`] names
/// the entity but not the triangle, so the collider is re-cast through parry
/// to read `FeatureId::Face`. The second cast is against one shape rather
/// than the whole world, so it costs a single BVH descent.
///
/// The filter is [`GameLayer::Level`] only: water is a sensor on its own
/// layer and must not answer "where is the ground" (docs/data/water.md — a sub
/// rides *under* the water surface).
pub fn probe_ground<'w, 's>(
    spatial: &SpatialQuery,
    surfaces: &SurfaceQuery<'w, 's>,
    probe: GroundProbe,
) -> Option<GroundHit> {
    let direction = Dir3::new(probe.dir).ok()?;
    let filter = SpatialQueryFilter::from_mask(GameLayer::Level);

    let hit = spatial.cast_ray(probe.origin, direction, probe.max, true, &filter)?;

    Some(GroundHit {
        point: probe.origin + direction * hit.distance,
        normal: hit.normal,
        distance: hit.distance,
        soil: soil_at(surfaces, hit.entity, probe.origin, direction, probe.max),
        entity: hit.entity,
    })
}

/// The soil of the triangle this ray hits on one entity's collider.
///
/// [`Soil::Default`] when the entity has no [`SurfaceInfo`] (a vehicle hull,
/// a destructible prop) or when parry reports a vertex or edge rather than a
/// face, which a ray grazing exactly along a shared edge can do.
fn soil_at<'w, 's>(
    surfaces: &SurfaceQuery<'w, 's>,
    entity: Entity,
    origin: Vec3,
    direction: Dir3,
    max: f32,
) -> Soil {
    let Ok((collider, transform, surface)) = surfaces.get(entity) else {
        return Soil::Default;
    };

    let Some(triangle) = hit_triangle(collider, transform, origin, direction, max) else {
        return surface.dominant;
    };

    surface.soil(triangle)
}

/// Which triangle of a collider a world-space ray hits.
///
/// The ray is moved into the collider's local frame rather than the shape
/// into the world, because parry's `cast_ray_and_get_normal` takes a pose and
/// a trimesh's BVH is built in local space.
pub fn hit_triangle(
    collider: &Collider,
    transform: &GlobalTransform,
    origin: Vec3,
    direction: Dir3,
    max: f32,
) -> Option<usize> {
    let (_, rotation, translation) = transform.to_scale_rotation_translation();
    let pose = avian3d::parry::math::Pose::from_parts(translation, rotation);

    let intersection = collider.shape_scaled().cast_ray_and_get_normal(
        &pose,
        &avian3d::parry::query::Ray::new(origin, *direction),
        max,
        true,
    )?;

    match intersection.feature {
        FeatureId::Face(index) => Some(face_to_triangle(collider, index)),
        // A vertex or edge hit: the caller falls back to the dominant soil.
        _ => None,
    }
}

/// parry reports a trimesh's back face as `triangle_count + index`, so a ray
/// hitting a surface from below names a face id past the end of the list.
/// Both ids mean the same triangle.
fn face_to_triangle(collider: &Collider, face: u32) -> usize {
    let face = face as usize;

    match collider.shape_scaled().as_trimesh() {
        Some(trimesh) => face % trimesh.num_triangles().max(1),
        None => face,
    }
}

/// The seed every [`SimRng`] stream starts from.
///
/// 0 in a local game; a multiplayer host replaces it before the first tick so
/// every client draws the same numbers (DESIGN §9).
#[derive(Resource, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SimSeed(pub u64);

/// The simulation's only randomness: a seeded ChaCha8 stream.
///
/// Stepped inside `FixedUpdate` and never from a render system, so a replay
/// of the same inputs draws the same numbers.
#[derive(Resource, Debug, Clone)]
pub struct SimRng(pub rand_chacha::ChaCha8Rng);

impl SimRng {
    /// A stream from a seed.
    pub fn from_seed(seed: SimSeed) -> SimRng {
        use rand::SeedableRng;

        SimRng(rand_chacha::ChaCha8Rng::seed_from_u64(seed.0))
    }
}

impl Default for SimRng {
    fn default() -> Self {
        SimRng::from_seed(SimSeed::default())
    }
}

/// Which debug overlays are on.
///
/// The resource lives here so `bft-sim` can be built against it; the dev
/// tools own the UI that toggles it, and `bft-game` draws from it (it needs
/// gizmos, which are a render feature this crate does not have).
///
/// It lives here rather than in `bft_game::devtools` because a dedicated
/// server carries the flags too; `bft_game::devtools` re-exports it. Every field is off by default and each overlay costs nothing while
/// its flag is off.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DebugFlags {
    /// Outline colliders.
    pub physics: bool,
    /// Outline water, lava and quicksand volumes.
    ///
    /// Separate from [`Self::physics`]; `--debug-physics` turns both on.
    pub water: bool,
    /// Outline the player's hull, its suspension rays and where its turret
    /// points.
    ///
    /// Off by default, like [`Self::physics`]: the vehicle draws its real
    /// `bft_00` subtree, so this is a debug aid.
    pub vehicle: bool,
    /// Outline every drawn node's bounding box.
    pub node_bboxes: bool,
    /// Draw the level's 256 m partition grid.
    pub partitions: bool,
    /// Draw each enemy's line to its target and its state.
    pub ai: bool,
    /// Mark the vehicle spawn points and their headings.
    pub spawns: bool,
    /// Mark the anim instances that are running, at their root nodes.
    pub anim: bool,
    /// Show the network statistics. Nothing draws it: there is no
    /// multiplayer yet.
    pub net: bool,
    /// Show the frame rate while the inspector is closed.
    pub fps: bool,
}

/// avian3d, on the sim's fixed timestep and inside its schedule.
pub struct PhysicsSetupPlugin;

impl Plugin for PhysicsSetupPlugin {
    fn build(&self, app: &mut App) {
        // `Time<Fixed>` is set by `SchedulePlugin` and is not
        // re-set here; two plugins owning one timestep is how it drifts.
        let tuning = app
            .world()
            .get_resource::<PhysicsTuning>()
            .copied()
            .unwrap_or_default();

        app.insert_resource(tuning)
            .init_resource::<SimSeed>()
            .init_resource::<DebugFlags>()
            .add_plugins(PhysicsPlugins::new(FixedUpdate))
            // Gravity at build time, not from a system: the very first step
            // must already fall at 40 m/s², and a `SimSet::Physics` system
            // would not have run before it (and would not run at all while
            // the sim is paused).
            .insert_resource(Gravity(Vec3::NEG_Y * tuning.gravity))
            // Seed the stream once the seed resource is final, not at build
            // time: `bft-bin` and the multiplayer client both set `SimSeed`
            // after adding the plugins.
            .add_systems(PreStartup, seed_rng)
            // Ungated, and before the step: the vehicle table rebuilds the tuning
            // from `player.zrd` mid-run, and a paused sim must still pick the
            // new value up so it is right when play resumes.
            .add_systems(FixedPreUpdate, apply_gravity);

        // Avian configured `PhysicsSystems::*` in `FixedUpdate` when its
        // plugins were added; nesting them in `SimSet::Physics` is what puts
        // the step between the vehicle's forces and damage resolution, and
        // what makes the sim's pause condition stop the solver too.
        app.configure_sets(
            FixedUpdate,
            (
                PhysicsSystems::First,
                PhysicsSystems::Prepare,
                PhysicsSystems::StepSimulation,
                PhysicsSystems::Writeback,
                PhysicsSystems::Last,
            )
                .in_set(SimSet::Physics),
        );
    }
}

/// Seed [`SimRng`] from [`SimSeed`].
fn seed_rng(mut commands: Commands, seed: Res<SimSeed>) {
    commands.insert_resource(SimRng::from_seed(*seed));
}

/// Keep avian's `Gravity` in step with [`PhysicsTuning`] when it changes.
///
/// The plugin sets the initial value; this only carries a *later* change
/// across — the vehicle table rebuilds the tuning once it parses `player.zrd`, and
/// the vehicle's water and quicksand regimes scale a body's `GravityScale`
/// against this global rather than replacing it.
fn apply_gravity(tuning: Res<PhysicsTuning>, mut gravity: ResMut<Gravity>) {
    if !tuning.is_changed() {
        return;
    }

    gravity.0 = Vec3::NEG_Y * tuning.gravity;
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::geometry::{material_per_triangle, triangulate};
    use bft_assets::materials::MaterialDesc;
    use bft_assets::models::{ModelDesc, ModelType, PolygonDesc, PolygonMaterialDesc};

    /// Two quads: one of material 0, one of material 1, in that polygon
    /// order. The soil lookup has to survive the *group* reordering the
    /// merge does, which is the trap docs/data/models.md names.
    fn two_material_model() -> ModelDesc {
        let quad = |base: u32, material_index: u32| PolygonDesc {
            vertex_indices: vec![base, base + 1, base + 2, base + 3],
            materials: vec![PolygonMaterialDesc {
                material_index,
                uv_coords: None,
            }],
            ..Default::default()
        };

        ModelDesc {
            vertices: vec![
                // quad 0, at y = 0
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(0.0, 0.0, 1.0),
                Vec3::new(1.0, 0.0, 1.0),
                Vec3::new(1.0, 0.0, 0.0),
                // quad 1, at y = 1
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 1.0, 1.0),
                Vec3::new(1.0, 1.0, 1.0),
                Vec3::new(1.0, 1.0, 0.0),
            ],
            polygons: vec![quad(0, 0), quad(4, 1)],
            ..Default::default()
        }
    }

    fn soils() -> Vec<MaterialDesc> {
        vec![
            MaterialDesc::Colored {
                color: [1.0, 1.0, 1.0],
                alpha: 255,
                soil: Soil::Default,
            },
            MaterialDesc::Colored {
                color: [0.0, 0.0, 1.0],
                alpha: 255,
                soil: Soil::Water,
            },
        ]
    }

    #[test]
    fn soil_follows_the_collider_triangle_order_not_the_polygon_order() {
        let model = two_material_model();
        let axes = bft_assets::axes::AxisConvention::GAMEZ_DEFAULT;
        let triangulated = triangulate(&model, &axes);

        let surface = crate::level::surface_info(ModelType::Default, &triangulated, &soils())
            .expect("a surface");

        // Four triangles, two per quad, grouped by material: the first two
        // are the `Default` quad, the last two the `Water` one.
        assert_eq!(surface.soil_by_triangle.len(), 4);
        assert_eq!(
            surface.soil_by_triangle,
            vec![Soil::Default, Soil::Default, Soil::Water, Soil::Water]
        );

        // And the mapping is exactly the geometry crate's, not a re-derivation.
        let expected: Vec<Soil> = material_per_triangle(ModelType::Default, &triangulated)
            .iter()
            .map(|index| soils()[*index as usize].soil())
            .collect();
        assert_eq!(surface.soil_by_triangle, expected);
    }

    #[test]
    fn the_dominant_soil_is_the_one_with_the_most_triangles() {
        assert_eq!(
            SurfaceInfo::new(vec![Soil::Water, Soil::Water, Soil::Default]).dominant,
            Soil::Water
        );
        assert_eq!(SurfaceInfo::new(Vec::new()).dominant, Soil::Default);
    }

    #[test]
    fn an_out_of_range_triangle_reads_as_ground_rather_than_panicking() {
        let surface = SurfaceInfo::new(vec![Soil::Lava]);

        assert_eq!(surface.soil(0), Soil::Lava);
        assert_eq!(surface.soil(7), Soil::Default);
    }

    /// The matrix in [`layers`]' docs, read back off the presets. A mask that
    /// disagrees with the table is a bug nothing else would notice until a
    /// projectile flew through a wall.
    #[test]
    fn the_collision_matrix_matches_the_package_table() {
        let (level, vehicle, projectile) =
            (layers::level(), layers::vehicle(), layers::projectile());
        let (pickup, trigger, water) = (layers::pickup(), layers::trigger(), layers::water());

        // Vehicle row: yes, yes, yes, sensor, sensor, sensor — all six interact.
        for other in [level, vehicle, projectile, pickup, trigger, water] {
            assert!(vehicle.interacts_with(other), "vehicle vs {other:?}");
        }

        // Projectile row: level yes, vehicle yes, projectile no, pickup no,
        // trigger no, water sensor.
        assert!(projectile.interacts_with(level));
        assert!(projectile.interacts_with(vehicle));
        assert!(!projectile.interacts_with(projectile));
        assert!(!projectile.interacts_with(pickup));
        assert!(!projectile.interacts_with(trigger));
        assert!(projectile.interacts_with(water));

        // Pickup row: only the vehicle, and only as a sensor.
        assert!(!pickup.interacts_with(level));
        assert!(pickup.interacts_with(vehicle));
        assert!(!pickup.interacts_with(projectile));
        assert!(!pickup.interacts_with(pickup));
        assert!(!pickup.interacts_with(trigger));
        assert!(!pickup.interacts_with(water));
    }

    #[test]
    fn gravity_is_the_readers_forty_not_earths_ten() {
        let tuning = PhysicsTuning::default();

        assert_eq!(tuning.gravity, 40.0);
        assert_eq!(tuning.water_gravity, 9.0);
        assert_eq!(tuning.quicksand_gravity, 3.0);
    }

    #[test]
    fn the_rng_is_reproducible_from_its_seed() {
        use rand::RngExt;

        let draw = |seed: u64| {
            let mut rng = SimRng::from_seed(SimSeed(seed));
            (0..4).map(|_| rng.0.random::<u32>()).collect::<Vec<_>>()
        };

        assert_eq!(draw(0), draw(0), "one seed, one stream");
        assert_ne!(draw(0), draw(1), "different seeds diverge");
    }
}
