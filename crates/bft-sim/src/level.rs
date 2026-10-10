//! [`LevelData`] → an entity hierarchy: transforms, colliders, markers.
//!
//! This is the sim side of spawning, so it runs without a renderer: it makes
//! entities carrying [`NodeId`], [`NodeName`], `Transform`, colliders and the
//! markers `bft-game` observes to attach meshes and materials (NFR-4). A
//! dedicated server spawns exactly the same hierarchy minus the drawing.
//!
//! # The hierarchy comes from parents, not children
//!
//! `child_indices` omits 83–874 edges per level while never contradicting
//! `parent_indices`, so [`spawn_level`] walks parents and uses `children` only
//! to order siblings (docs/data/nodes.md). A level has 180–220
//! root trees, not one, so every root is parented to a synthetic
//! [`LevelRoot`] entity that owns the level.

use avian3d::prelude::*;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::geometry::{Triangulated, material_per_triangle, triangulate};
use bft_assets::level::{LevelData, NodeDesc, PartitionKey, TransformDesc};
use bft_assets::materials::MaterialDesc;
use bft_assets::models::ModelType;

use crate::colliders::collider_from;
use crate::physics::SurfaceInfo;

/// A node's id: **its position in [`LevelData::nodes`]**, which is what
/// `parent_indices`, `child_indices` and the partition grid all index. The
/// emitted `index` field is 0 on every node and is not it (docs/data/nodes.md).
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub u32);

/// The node's name from the data, which is how interp (docs/data/interp.md) and
/// anim (docs/data/anims.md) refer to it. Not unique: m1 has several `sparky`
/// and `plights`.
#[derive(Component, Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeName(pub String);

/// The node's `zone_id`, for the nodes that have one.
///
/// Pairs with the anim VM's `Execution::ByZone` and `objectives.zrd`'s
/// `HINT_ZONE`, which use the same ids (docs/data/nodes.md). Absent on a node
/// whose `zone_id` is −1.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeZone(pub i32);

/// The node's `active` flag, as the anim VM owns it.
///
/// `ObjectActiveState` events toggle this, and it means visibility **and**
/// collision for the node's subtree (docs/data/anims.md). It is a component of
/// its own rather than Bevy's `Visibility` because the sim has no renderer
/// (NFR-4) and because mission objectives watch it: `bft-game` mirrors it
/// onto `Visibility`, and the colliders read it.
///
/// **Every node of every level starts active** (docs/data/nodes.md), so this
/// is `true` until an `ObjectActiveState` says otherwise.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeActive(pub bool);

impl Default for NodeActive {
    fn default() -> NodeActive {
        NodeActive(true)
    }
}

/// This entity draws [`LevelData::models`]`[0]`'s model. `bft-game` observes
/// it and attaches the meshes; a server ignores it.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ModelRef(pub usize);

/// The synthetic parent of every root tree, and the entity a level is
/// despawned through.
#[derive(Component, Debug, Clone, Copy)]
pub struct LevelRoot;

/// Level geometry: never moves, so it can be culled and batched as static.
#[derive(Component, Debug, Clone, Copy)]
pub struct LevelStatic;

/// A `Lod` node: its children are visible while the camera is within the band.
#[derive(Component, Debug, Clone, Copy)]
pub struct LodSwitch {
    pub near: f32,
    pub far: f32,
    /// mech3ax's `field00`. Kept for experiment; the switch does not read it
    /// (docs/data/nodes.md).
    pub level: bool,
}

/// A point light carried by a model.
///
/// **The level spawn attaches none.** Recoil's model lights are weapon
/// and effect templates (freon, impacts, sparks, debris), not level lighting
/// (docs/data/nodes.md), so they come to life when something fires; the anim VM's
/// lights use this shape.
#[derive(Component, Debug, Clone, Copy)]
pub struct LevelLight {
    pub color: Color,
    pub near: f32,
    pub far: f32,
}

/// The level's environment: fog and extent, for `bft-game` to put on a camera.
#[derive(Resource, Debug, Clone, PartialEq)]
pub struct WorldEnv {
    pub fog_color: Color,
    /// `(near, far)`: fog starts at `near`, full at `far`.
    pub fog_range: (f32, f32),
    /// `(clear, full)` altitude. **`clear > full`** in every level.
    pub fog_altitude: (f32, f32),
    /// The world's x/z extent, as `(left, top, right, bottom)`.
    pub area: (i32, i32, i32, i32),
}

/// The level's sun: one directional light plus an ambient term.
///
/// From the single `Light` node, whose `diffuse`/`ambient` split sums to 1.0
/// in all 13 levels and whose colour is always black — so [`SunLight::color`]
/// is the substituted white (docs/data/nodes.md).
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct SunLight {
    /// The direction the light travels, from the node's Euler `orientation`.
    pub direction: Dir3,
    pub color: Color,
    /// Directional share, 0–1.
    pub diffuse: f32,
    /// Ambient share, 0–1.
    pub ambient: f32,
}

/// The world's partition grid, kept for later AI and culling use.
///
/// Nothing reads it yet. It is stored because the grid is the only place the
/// data says which nodes share a 256×256 column, which is what a broad-phase
/// or an AI region query wants.
#[derive(Resource, Debug, Clone, Default)]
pub struct PartitionGrid {
    /// Cells in row-major order, z then x.
    cells: Vec<PartitionCell>,
    columns: usize,
    rows: usize,
}

/// One cell: a column of the world and the nodes standing in it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PartitionCell {
    /// The cell's world x corner, a multiple of 256.
    pub x: i32,
    pub z: i32,
    /// The cell's height band.
    pub y_min: f32,
    pub y_max: f32,
    /// The nodes in this cell, by [`NodeId`].
    pub nodes: Vec<u32>,
}

impl PartitionGrid {
    /// Build the grid from a level's world description.
    pub fn from_level(level: &LevelData) -> PartitionGrid {
        let columns = level.world.partition_columns;
        let cells: Vec<PartitionCell> = level
            .world
            .partitions
            .iter()
            .map(|cell| PartitionCell {
                x: cell.x,
                z: cell.z,
                y_min: cell.y_min,
                y_max: cell.y_max,
                nodes: cell.nodes.clone(),
            })
            .collect();

        PartitionGrid {
            // A level with no partition grid has no rows, rather than a
            // division by zero.
            rows: cells.len().checked_div(columns).unwrap_or(0),
            columns,
            cells,
        }
    }

    /// How many cells across and down.
    pub fn size(&self) -> (usize, usize) {
        (self.columns, self.rows)
    }

    /// One cell by grid indices, as `area_partition` names them.
    pub fn cell(&self, key: PartitionKey) -> Option<&PartitionCell> {
        let (x, z) = (usize::try_from(key.x).ok()?, usize::try_from(key.z).ok()?);

        if x >= self.columns {
            return None;
        }

        self.cells.get(z * self.columns + x)
    }

    /// Every cell, row-major in z then x.
    pub fn cells(&self) -> &[PartitionCell] {
        &self.cells
    }

    /// A grid from cells laid out row-major in z then x.
    ///
    /// For tests and tools that need a grid without a `LevelData` behind it;
    /// [`from_level`](Self::from_level) is how a real level builds one.
    pub fn from_cells(columns: usize, rows: usize, cells: Vec<PartitionCell>) -> PartitionGrid {
        PartitionGrid {
            cells,
            columns,
            rows,
        }
    }
}

/// Node id → the entity spawned for it.
///
/// Every node gets an entity, including the ones with no model and the four
/// scene-setup singletons, so anim and interp can find any node by id or name.
#[derive(Resource, Debug, Clone, Default)]
pub struct NodeIndex {
    by_id: HashMap<u32, Entity>,
    by_name: HashMap<String, Vec<Entity>>,
}

impl NodeIndex {
    /// Add a node to the index under an id and a name.
    ///
    /// [`spawn_level`] is the only caller in a real run; it is public so a
    /// headless test can build the three or four nodes it cares about without
    /// a `gamez` archive.
    pub fn insert(&mut self, id: u32, name: &str, entity: Entity) {
        self.by_id.insert(id, entity);
        self.by_name
            .entry(name.to_string())
            .or_default()
            .push(entity);
    }

    /// The entity for a node id.
    pub fn get(&self, id: u32) -> Option<Entity> {
        self.by_id.get(&id).copied()
    }

    /// Every entity whose node has this name. Names repeat — m1 has four
    /// `sparky` nodes — so interp and anim bindings must expect several
    /// (docs/data/nodes.md).
    pub fn by_name(&self, name: &str) -> &[Entity] {
        self.by_name.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    /// How many nodes were spawned.
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// The result of spawning a level.
#[derive(Debug)]
pub struct SpawnedLevel {
    /// The synthetic [`LevelRoot`] every root tree hangs from.
    pub root: Entity,
    pub nodes: NodeIndex,
    pub world_env: WorldEnv,
    /// `None` only for a level with no `Light` node, which no shipped level
    /// is.
    pub sun: Option<SunLight>,
    pub partitions: PartitionGrid,
    /// How many liquid volumes the water pass spawned.
    pub water_volumes: usize,
}

/// Which collision layers a body belongs to.
///
/// One enum for the whole game: a `CollisionLayers` mask is a bitfield over
/// *this* type, so a second enum elsewhere would produce masks that silently
/// do not line up. The matrix that says which pairs interact is
/// [`physics::layers`](crate::physics::layers).
#[derive(PhysicsLayer, Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GameLayer {
    #[default]
    Default,
    /// Static level geometry: what the vehicle rides on and bumps into.
    Level,
    /// Player and enemy vehicles.
    Vehicle,
    /// Projectiles in flight.
    Projectile,
    /// Pickups, which sense a vehicle rather than colliding with it.
    Pickup,
    /// Trigger volumes (the anim VM's `bvol*` zones).
    Trigger,
    /// Water, lava and quicksand surfaces: sensors, never solid.
    Water,
    /// The box a node with `intersect_bbox` is shot through ([`HitBoxOf`]):
    /// only weapon casts see it.
    HitBox,
}

/// A node's hit box: the box of its model, which a weapon hits in place of
/// the model's own surface.
///
/// The node flag `intersect_bbox` says the original engine tested hits
/// against the node's bounding box. m1's 23 palm groups are the reason it
/// matters: the palms are camera-facing billboards with no surface at all,
/// and the one solid node in each group (`g10`) is two crossed quads. Shot
/// at the quads, a tree was a paper-thin cross; its box is 10 × 17 × 8 m.
/// A sensor, so it never stops a vehicle: what the model's surface blocked
/// before, it still blocks. The node it belongs to is what takes the hit.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct HitBoxOf(pub Entity);

/// Spawn a whole level as an entity hierarchy.
///
/// Returns the root entity and the node index; the caller inserts
/// [`WorldEnv`], [`SunLight`] and [`PartitionGrid`] as resources (or
/// [`spawn_level_into`] does it for them).
///
/// Every node becomes exactly one entity, whatever its variant and whether or
/// not it has a model — 148 of the game's models have no polygons at all and
/// their nodes still carry transforms and children (docs/data/models.md).
pub fn spawn_level(
    commands: &mut Commands,
    level: &LevelData,
    axes: &AxisConvention,
) -> SpawnedLevel {
    // No `Visibility` anywhere in this function: it is a `bevy_render` type
    // and this crate builds without a renderer (NFR-4). `bft-game`'s
    // `level_view` adds it along with the meshes; a server never needs it.
    let root = commands
        .spawn((LevelRoot, Name::new("level"), Transform::IDENTITY))
        .id();

    // Reserve every entity first, so a parent's id is known before its
    // children are built and the tree can be walked in one pass in any order.
    let entities: Vec<Entity> = level
        .nodes
        .iter()
        .map(|_| commands.spawn_empty().id())
        .collect();

    let mut index = NodeIndex::default();
    let mut collider_count = 0usize;
    // What a crater needs to cut a node again later: its model's triangles,
    // shared between the nodes that instance it, and the level's materials.
    let surfaces = std::sync::Arc::new(crate::crater::LevelSurfaces::from_materials(
        &level.materials,
    ));
    let mut triangulations: HashMap<usize, std::sync::Arc<Triangulated>> = HashMap::default();
    let mut skipped_colliders = 0usize;

    for (id, node) in level.nodes.iter().enumerate() {
        let entity = entities[id];
        let common = node.common();

        // A root's parent is the level entity, so the whole level moves and
        // despawns as one (there are 174-220 roots per level, docs/data/nodes.md).
        let parent = match common.parent {
            Some(parent) => entities
                .get(parent as usize)
                .copied()
                // The loader already drops out-of-range parents, so this is
                // belt and braces rather than a case the data produces.
                .unwrap_or(root),
            None => root,
        };

        let mut entity_commands = commands.entity(entity);

        entity_commands.insert((
            NodeId(id as u32),
            NodeName(common.name.clone()),
            Name::new(common.name.clone()),
            node_transform(node, axes),
            ChildOf(parent),
            // The data has `active: true` on every node of every level, so
            // the VM's starting point is "everything on" and each def's
            // `reset_state` turns off what should not be (docs/data/anims.md).
            NodeActive(common.flags.active),
        ));

        if let Some(zone) = common.zone_id {
            entity_commands.insert(NodeZone(zone));
        }

        // A root that is not the World's is a prototype, not part of the scene: the ~170 pickup,
        // enemy and effect subtrees the AI and the anim VM clone from. It is still spawned and
        // still indexed by name — anim (docs/data/anims.md) and interp (docs/data/interp.md)
        // address template nodes — but `bft-game` hides it. The marker goes on the root alone:
        // visibility is inherited, so marking all 25–49 nodes of each subtree would buy nothing.
        if common.parent.is_none() && id as u32 != level.world_node {
            entity_commands.insert(crate::ai::templates::Template);
        }

        match node {
            NodeDesc::Object3d(_) | NodeDesc::Empty(_) | NodeDesc::World(_) => {}
            NodeDesc::Lod(lod) => {
                entity_commands.insert(LodSwitch {
                    near: lod.range.0,
                    far: lod.range.1,
                    level: lod.level,
                });
            }
            // The sun becomes a resource, not a component on this entity:
            // `bft-game` puts one directional light on the scene rather than
            // lighting from a node buried in the hierarchy.
            NodeDesc::Light(_) => {}
            // The original renderer's settings. Spawned for completeness so
            // anim can address them by name, but nothing reads them.
            NodeDesc::Camera(_) | NodeDesc::Display(_) | NodeDesc::Window(_) => {}
        }

        if let Some(model_index) = common.model_index {
            entity_commands.insert((ModelRef(model_index), LevelStatic));

            // Collision is the node flags' call, not the model's: they are
            // what interp's SetAltitudeSurface/SetIntersectSurface set, and
            // docs/data/nodes.md names them the collision authority.
            if common.flags.collides() {
                // One triangulation for both the shape and its soil, so the
                // two can never come from different passes over the model —
                // the soil list is indexed by collider triangle and would be
                // meaningless against a differently ordered mesh
                // (docs/data/models.md).
                match level.models.get(model_index) {
                    Some(model) => {
                        let triangulated = triangulations
                            .entry(model_index)
                            .or_insert_with(|| std::sync::Arc::new(triangulate(model, axes)))
                            .clone();

                        match collider_from(model.model_type, &triangulated) {
                            Some(collider) => {
                                collider_count += 1;
                                entity_commands.insert((
                                    collider,
                                    RigidBody::Static,
                                    // The preset, not a hand-written mask:
                                    // the physics' matrix decides who the
                                    // level collides with.
                                    crate::physics::layers::level(),
                                ));

                                if let Some(surface) =
                                    surface_info(model.model_type, &triangulated, &level.materials)
                                {
                                    entity_commands.insert(surface);
                                }
                                // Only the deformable ground keeps what a
                                // crater needs to cut it.
                                if common.flags.can_modify {
                                    entity_commands.insert(crate::crater::GroundSource {
                                        model_type: model.model_type,
                                        triangulated,
                                        surfaces: surfaces.clone(),
                                    });
                                }
                            }
                            // A facade or a polygon-less light carrier: no
                            // surface to collide with, so no component rather
                            // than an empty trimesh (docs/data/models.md).
                            None => skipped_colliders += 1,
                        }
                    }
                    None => skipped_colliders += 1,
                }
            }
            if common.flags.intersect_bbox
                && let Some(hit_box) = hit_box(common.model_bbox, axes)
            {
                commands.spawn((
                    HitBoxOf(entity),
                    hit_box,
                    Sensor,
                    crate::physics::layers::hit_box(),
                    ChildOf(entity),
                ));
            }
        }

        index.insert(id as u32, &common.name, entity);
    }

    let world_env = WorldEnv {
        fog_color: Color::linear_rgb(
            level.world.fog_color[0],
            level.world.fog_color[1],
            level.world.fog_color[2],
        ),
        fog_range: level.world.fog_range,
        fog_altitude: level.world.fog_altitude,
        area: level.world.area,
    };

    let sun = level.sun.as_ref().map(|sun| sun_light(sun, axes));

    // Water last, because a region welds triangles across nodes and belongs
    // to the level rather than to any one of them (docs/data/water.md).
    let water_volumes =
        crate::water::spawn_water(commands, root, &crate::water::find_water(level, axes));

    info!(
        "spawned {} nodes under the level root ({} roots), {collider_count} colliders, \
         {skipped_colliders} model nodes with no collision surface, \
         {water_volumes} liquid volumes",
        level.nodes.len(),
        level.root_nodes.len(),
    );

    SpawnedLevel {
        root,
        nodes: index,
        world_env,
        water_volumes,
        sun,
        partitions: PartitionGrid::from_level(level),
    }
}

/// [`spawn_level`], plus inserting the resources it produced.
///
/// This is what a caller with a `World` wants; `spawn_level` is separate so a
/// test can spawn without owning the resources.
pub fn spawn_level_into(
    commands: &mut Commands,
    level: &LevelData,
    axes: &AxisConvention,
) -> Entity {
    let spawned = spawn_level(commands, level, axes);

    commands.insert_resource(spawned.nodes);
    commands.insert_resource(spawned.world_env);
    commands.insert_resource(spawned.partitions);

    match spawned.sun {
        Some(sun) => commands.insert_resource(sun),
        // A level with no sun leaves the previous one in place, which would
        // be a confusing bug; remove it instead.
        None => commands.remove_resource::<SunLight>(),
    }

    spawned.root
}

/// The sun, as a direction plus the two shares.
fn sun_light(sun: &bft_assets::level::LightDesc, axes: &AxisConvention) -> SunLight {
    // The node stores Euler angles; the light travels along the rotated
    // forward axis. Bevy's forward is -z, which is what a `DirectionalLight`
    // shines along.
    let rotation = axes.rotation(sun.orientation);
    let direction = Dir3::new(rotation * Vec3::NEG_Z).unwrap_or(Dir3::NEG_Y);
    let color = sun.effective_color();

    SunLight {
        direction,
        color: Color::linear_rgb(color[0], color[1], color[2]),
        diffuse: sun.diffuse,
        ambient: sun.ambient,
    }
}

/// The per-triangle soil of a model's collider, or `None` when it has none.
///
/// The order is [`material_per_triangle`]'s, which is the order
/// [`collider_indices`](bft_assets::geometry::collider_indices) merges the
/// groups in — **not** polygon order. Going through that function rather than
/// re-deriving it is the whole point: a hand-rolled walk over `groups` that
/// got the order wrong would attribute the wrong soil to every triangle after
/// the first material change, which is the difference between driving on
/// water and driving on rock (docs/data/models.md).
pub fn surface_info(
    model_type: ModelType,
    triangulated: &Triangulated,
    materials: &[MaterialDesc],
) -> Option<SurfaceInfo> {
    let soil_by_triangle: Vec<bft_assets::materials::Soil> =
        material_per_triangle(model_type, triangulated)
            .iter()
            .map(|index| {
                materials
                    .get(*index as usize)
                    .map(MaterialDesc::soil)
                    // A polygon with no material lands in group 0; a level
                    // with no materials at all is a test fixture. Neither is
                    // a liquid.
                    .unwrap_or_default()
            })
            .collect();

    if soil_by_triangle.is_empty() {
        return None;
    }

    Some(SurfaceInfo::new(soil_by_triangle))
}

/// One node's local transform, through the axis convention.
pub fn node_transform(node: &NodeDesc, axes: &AxisConvention) -> Transform {
    match node {
        // The `Light` node's own transform is its `translate`; its
        // `orientation` is the sun's direction, not the entity's rotation.
        NodeDesc::Light(_) => Transform::IDENTITY,
        _ => transform_of(node_transform_desc(node), axes),
    }
}

fn node_transform_desc(node: &NodeDesc) -> TransformDesc {
    match node {
        NodeDesc::Object3d(object) => object.transform,
        _ => TransformDesc::Initial,
    }
}

/// Convert one [`TransformDesc`] under an axis convention.
pub fn transform_of(transform: TransformDesc, axes: &AxisConvention) -> Transform {
    match transform {
        TransformDesc::Initial => Transform::IDENTITY,
        TransformDesc::Matrix {
            rotation,
            translation,
        } => {
            // The convention's permutation has to be applied on both sides of
            // the rotation: `axes . M . axes^-1` is the same rotation
            // expressed in the destination basis. For the identity convention
            // this is exactly `M`.
            let basis = axes_matrix(axes);
            let rotated = basis * rotation * basis.transpose();

            Transform {
                translation: axes.point(translation),
                rotation: Quat::from_mat3(&orthonormalise(rotated)),
                scale: Vec3::ONE,
            }
        }
        TransformDesc::RotateTranslateScale {
            rotate,
            translate,
            scale,
        } => Transform {
            translation: axes.point(translate),
            rotation: axes.rotation(rotate),
            // A scale is a per-axis magnitude, so it permutes but does not
            // change sign.
            scale: axes.point(scale).abs(),
        },
    }
}

/// The axis convention as a matrix, for conjugating a rotation.
fn axes_matrix(axes: &AxisConvention) -> Mat3 {
    Mat3::from_cols(
        axes.point(Vec3::X),
        axes.point(Vec3::Y),
        axes.point(Vec3::Z),
    )
}

/// Re-orthonormalise a rotation so `Quat::from_mat3` cannot produce a NaN.
///
/// Every shipped matrix is already orthonormal to f32 precision (all 1307 of
/// m1's have determinant 1.0 and unit rows), so this only guards against data
/// another game might have.
fn orthonormalise(m: Mat3) -> Mat3 {
    let x = m.x_axis.try_normalize().unwrap_or(Vec3::X);
    let z = x.cross(m.y_axis).try_normalize().unwrap_or(Vec3::Z);
    let y = z.cross(x);

    Mat3::from_cols(x, y, z)
}

/// Tear a level down: the root and every node under it, and the resources.
///
/// Triggered as an event so any system can ask for it without owning the
/// root entity (`commands.trigger(DespawnLevel)`).
#[derive(Event, Debug, Clone, Copy, Default)]
pub struct DespawnLevel;

/// The observer for [`DespawnLevel`].
pub fn despawn_level(
    _trigger: On<DespawnLevel>,
    mut commands: Commands,
    roots: Query<Entity, With<LevelRoot>>,
) {
    for root in &roots {
        // Despawning a parent despawns its children in Bevy 0.19, so this
        // takes every node with it.
        commands.entity(root).despawn();
    }

    commands.remove_resource::<NodeIndex>();
    commands.remove_resource::<WorldEnv>();
    commands.remove_resource::<PartitionGrid>();
    commands.remove_resource::<SunLight>();
}

/// The level-spawning side of `SimPlugin`.
pub struct LevelPlugin;

impl Plugin for LevelPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(despawn_level)
            .init_resource::<StaticPoses>()
            .add_systems(
                FixedUpdate,
                (
                    remember_static_poses.in_set(crate::schedule::SimSet::Weapons),
                    restore_static_poses.in_set(crate::schedule::SimSet::Damage),
                ),
            );
    }
}

/// The local transforms of the level's static bodies as the step found them.
#[derive(Resource, Debug, Default)]
pub struct StaticPoses(HashMap<Entity, Transform>);

/// Level geometry hanging under another node.
type StaticChildren = (With<LevelStatic>, With<ChildOf>);

/// Level geometry whose transform something wrote since the last look.
type MovedStatics = (With<LevelStatic>, Changed<Transform>);

/// Remember where the level's static bodies are before the physics step.
///
/// Taken after the previous tick's anim ops, so a script's move of a door or
/// a VTOL is what gets remembered.
pub fn remember_static_poses(
    mut poses: ResMut<StaticPoses>,
    bodies: Query<(Entity, &Transform, &RigidBody), StaticChildren>,
) {
    poses.0.clear();
    poses.0.extend(
        bodies
            .iter()
            .filter(|(_, _, body)| body.is_static())
            .map(|(entity, transform, _)| (entity, *transform)),
    );
}

/// Undo what avian's writeback did to a static body's local transform.
///
/// avian writes every body whose `Position` changed back into its local
/// transform through its parent's `GlobalTransform`, and a static body's
/// `Position` changes whenever a script moves an ancestor. The parent's
/// global pose lags such a move, and the round trip drifts: m3's VTOL floor
/// was rolled 15° by the end of its 18 s fly-in, and the BFT could not drive
/// out of it. Physics never moves a static body, so the pose
/// before the step is the right one.
pub fn restore_static_poses(
    poses: Res<StaticPoses>,
    mut bodies: Query<(Entity, &mut Transform), MovedStatics>,
) {
    for (entity, mut transform) in &mut bodies {
        if let Some(before) = poses.0.get(&entity)
            && *transform != *before
        {
            *transform = *before;
        }
    }
}

/// A node's hit box, from its model's bounds in its own frame: the collider
/// and where it sits. `None` for bounds with no extent.
fn hit_box(model_bbox: (Vec3, Vec3), axes: &AxisConvention) -> Option<(Collider, Transform)> {
    let (a, b) = (axes.point(model_bbox.0), axes.point(model_bbox.1));
    let (min, max) = (a.min(b), a.max(b));
    let size = max - min;
    if !size.is_finite() || size.max_element() <= 0.0 {
        return None;
    }
    // A flat box (a sign, a quad) still needs some depth to be hit.
    let size = size.max(Vec3::splat(0.1));
    Some((
        Collider::cuboid(size.x, size.y, size.z),
        Transform::from_translation((min + max) / 2.0),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::level::parse_gamez;
    use bft_assets::test_support::minimal_gamez;

    fn minimal_level() -> LevelData {
        parse_gamez(&minimal_gamez(), "test.gamez.zip").expect("the fixture loads")
    }

    /// An app with just enough of Bevy to spawn into.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), LevelPlugin));
        app.insert_resource(AxisConvention::GAMEZ_DEFAULT);
        app
    }

    fn spawn_minimal(app: &mut App) -> Entity {
        let level = minimal_level();
        let root = app
            .world_mut()
            .run_system_cached_with(
                |In(level): In<LevelData>, mut commands: Commands| -> Entity {
                    spawn_level_into(&mut commands, &level, &AxisConvention::GAMEZ_DEFAULT)
                },
                level,
            )
            .expect("the system runs");
        app.update();
        root
    }

    #[test]
    fn every_node_becomes_one_entity_under_the_level_root() {
        let mut app = test_app();
        let root = spawn_minimal(&mut app);

        let index = app.world().resource::<NodeIndex>();
        assert_eq!(index.len(), 3, "one entity per node, models or not");

        // As many node entities as nodes.
        let nodes = app.world_mut().query::<&NodeId>().iter(app.world()).count();
        assert_eq!(nodes, 3);

        assert!(app.world().get::<LevelRoot>(root).is_some());
        // And the root is not itself a node.
        assert!(app.world().get::<NodeId>(root).is_none());
    }

    #[test]
    fn the_hierarchy_follows_parent_indices_through_to_the_level_root() {
        let mut app = test_app();
        let root = spawn_minimal(&mut app);
        let index = app.world().resource::<NodeIndex>().clone();

        let world = index.get(0).expect("the world node");
        let group = index.get(1).expect("the group");
        let floor = index.get(2).expect("the floor");

        // world -> group -> floor, and the world hangs off the level root.
        assert_eq!(
            app.world().get::<ChildOf>(world).map(|c| c.parent()),
            Some(root)
        );
        assert_eq!(
            app.world().get::<ChildOf>(group).map(|c| c.parent()),
            Some(world)
        );
        assert_eq!(
            app.world().get::<ChildOf>(floor).map(|c| c.parent()),
            Some(group)
        );
    }

    #[test]
    fn a_node_is_findable_by_name_as_well_as_by_id() {
        let mut app = test_app();
        spawn_minimal(&mut app);
        let index = app.world().resource::<NodeIndex>();

        let floor = index.by_name("floor");
        assert_eq!(floor.len(), 1);
        assert_eq!(index.get(2), Some(floor[0]));

        assert_eq!(
            app.world().get::<NodeName>(floor[0]).map(|n| n.0.as_str()),
            Some("floor")
        );
        assert!(index.by_name("nothing-named-this").is_empty());
    }

    #[test]
    fn a_matrix_transform_reaches_the_entity_as_a_translation() {
        let mut app = test_app();
        spawn_minimal(&mut app);
        let index = app.world().resource::<NodeIndex>().clone();
        let floor = index.get(2).expect("the floor");

        let transform = app.world().get::<Transform>(floor).expect("a transform");
        assert_eq!(transform.translation, Vec3::new(10.0, 0.0, 20.0));
        assert_eq!(transform.scale, Vec3::ONE);
    }

    #[test]
    fn a_model_bearing_node_gets_a_model_ref_and_the_others_do_not() {
        let mut app = test_app();
        spawn_minimal(&mut app);
        let index = app.world().resource::<NodeIndex>().clone();

        assert_eq!(
            app.world()
                .get::<ModelRef>(index.get(2).unwrap())
                .map(|m| m.0),
            Some(0)
        );
        assert!(
            app.world()
                .get::<LevelStatic>(index.get(2).unwrap())
                .is_some()
        );
        // The group and the world have no model.
        assert!(app.world().get::<ModelRef>(index.get(1).unwrap()).is_none());
        assert!(app.world().get::<ModelRef>(index.get(0).unwrap()).is_none());
    }

    #[test]
    fn a_flagged_surface_gets_a_static_collider() {
        let mut app = test_app();
        spawn_minimal(&mut app);
        let index = app.world().resource::<NodeIndex>().clone();
        let floor = index.get(2).expect("the floor");

        // The fixture's floor sets both surface flags, as real terrain does.
        assert!(app.world().get::<Collider>(floor).is_some());
        assert!(matches!(
            app.world().get::<RigidBody>(floor),
            Some(RigidBody::Static)
        ));
        assert!(app.world().get::<CollisionLayers>(floor).is_some());
    }

    /// The water volumes hold **world-space** coordinates, spawned
    /// under the level root at the identity. A root that carried a transform
    /// would move the node geometry and leave the water behind, and nothing
    /// else would notice — so the assumption is pinned here rather than left
    /// as a comment in `water.rs`.
    #[test]
    fn the_level_root_is_the_identity_because_water_is_in_world_space() {
        let mut app = test_app();
        let root = spawn_minimal(&mut app);

        assert_eq!(
            app.world().get::<Transform>(root).copied(),
            Some(Transform::IDENTITY)
        );
    }

    /// A collider and its soil must arrive together: a collider without
    /// `SurfaceInfo` reads as `Soil::Default` everywhere, which is a vehicle
    /// driving on water and never knowing.
    #[test]
    fn a_flagged_surface_carries_its_soil_alongside_the_collider() {
        let mut app = test_app();
        spawn_minimal(&mut app);
        let index = app.world().resource::<NodeIndex>().clone();
        let floor = index.get(2).expect("the floor");

        let surface = app
            .world()
            .get::<SurfaceInfo>(floor)
            .expect("a collider's soil");

        // The fixture's quad is one `Soil::Default` material, two triangles.
        assert_eq!(surface.soil_by_triangle.len(), 2);
        assert_eq!(surface.dominant, bft_assets::materials::Soil::Default);
    }

    #[test]
    fn a_model_node_without_the_surface_flags_draws_but_does_not_collide() {
        // The flags are the collision authority, so a model alone is not
        // enough (docs/data/nodes.md).
        let nodes = format!(
            "[{},{}]",
            bft_assets::test_support::WORLD_NODE,
            r#"{ "name": "decoration", "model_index": 0, "parent_indices": [0],
                 "flags": { "active": true, "tree_valid": true },
                 "data": { "Object3d": { "transform": "Initial" } } }"#
        );
        let bytes = bft_assets::test_support::gamez_from(
            r#"[{ "Textured": { "texture_index": 0 } }]"#,
            r#"[{ "name": "wall.tif" }]"#,
            bft_assets::test_support::MINIMAL_MODEL,
            &nodes,
        );
        let level = parse_gamez(&bytes, "test").expect("loads");

        let mut app = test_app();
        app.world_mut()
            .run_system_cached_with(
                |In(level): In<LevelData>, mut commands: Commands| {
                    spawn_level_into(&mut commands, &level, &AxisConvention::GAMEZ_DEFAULT);
                },
                level,
            )
            .expect("runs");
        app.update();

        let index = app.world().resource::<NodeIndex>().clone();
        let decoration = index.get(1).expect("the node");

        assert!(
            app.world().get::<ModelRef>(decoration).is_some(),
            "still drawn"
        );
        assert!(
            app.world().get::<Collider>(decoration).is_none(),
            "but not collidable"
        );
    }

    #[test]
    fn the_world_node_becomes_the_world_env_resource() {
        let mut app = test_app();
        spawn_minimal(&mut app);

        let env = app.world().resource::<WorldEnv>();
        assert_eq!(env.fog_range, (100.0, 200.0));
        assert_eq!(env.area, (0, 0, 512, 512));
        // Inverted, as every shipped level is.
        assert!(env.fog_altitude.0 > env.fog_altitude.1);
    }

    #[test]
    fn the_partition_grid_becomes_a_resource_indexed_the_way_nodes_name_it() {
        let mut app = test_app();
        spawn_minimal(&mut app);

        let grid = app.world().resource::<PartitionGrid>();
        assert_eq!(grid.size(), (1, 1));

        let cell = grid.cell(PartitionKey { x: 0, z: 0 }).expect("one cell");
        assert_eq!((cell.x, cell.z), (0, 0));
        assert!(grid.cell(PartitionKey { x: 1, z: 0 }).is_none());
    }

    #[test]
    fn a_lod_node_becomes_a_lod_switch_carrying_its_band() {
        let nodes = format!(
            "[{},{}]",
            bft_assets::test_support::WORLD_NODE,
            r#"{ "name": "l1", "parent_indices": [0],
                 "flags": { "active": true, "tree_valid": true },
                 "data": { "Lod": { "field00": 1, "range": { "min": 0.0, "max": 128.0 } } } }"#
        );
        let bytes = bft_assets::test_support::gamez_from("[]", "[]", "[]", &nodes);
        let level = parse_gamez(&bytes, "test").expect("loads");

        let mut app = test_app();
        app.world_mut()
            .run_system_cached_with(
                |In(level): In<LevelData>, mut commands: Commands| {
                    spawn_level_into(&mut commands, &level, &AxisConvention::GAMEZ_DEFAULT);
                },
                level,
            )
            .expect("runs");
        app.update();

        let index = app.world().resource::<NodeIndex>().clone();
        let lod = app
            .world()
            .get::<LodSwitch>(index.get(1).expect("the lod"))
            .expect("a LodSwitch");

        assert_eq!((lod.near, lod.far), (0.0, 128.0));
        assert!(lod.level);
    }

    #[test]
    fn the_light_node_becomes_the_sun_resource_pointing_where_it_looks() {
        let nodes = format!(
            "[{},{}]",
            bft_assets::test_support::WORLD_NODE,
            // Straight down: no yaw, a quarter turn of pitch.
            r#"{ "name": "sunlight", "parent_indices": [0], "data": { "Light": {
                 "orientation": { "x": -1.5707963, "y": 0.0, "z": 0.0 },
                 "diffuse": 0.55, "ambient": 0.45,
                 "color": { "r": 0.0, "g": 0.0, "b": 0.0 },
                 "range": { "min": 480.0, "max": 540.0 } } } }"#
        );
        let bytes = bft_assets::test_support::gamez_from("[]", "[]", "[]", &nodes);
        let level = parse_gamez(&bytes, "test").expect("loads");

        let mut app = test_app();
        app.world_mut()
            .run_system_cached_with(
                |In(level): In<LevelData>, mut commands: Commands| {
                    spawn_level_into(&mut commands, &level, &AxisConvention::GAMEZ_DEFAULT);
                },
                level,
            )
            .expect("runs");
        app.update();

        let sun = *app.world().resource::<SunLight>();
        assert_eq!(sun.diffuse, 0.55);
        assert_eq!(sun.ambient, 0.45);
        // A -90 degrees pitch turns Bevy's forward (-z) to straight down.
        assert!(
            (sun.direction.as_vec3() - Vec3::NEG_Y).length() < 1e-5,
            "{:?}",
            sun.direction
        );
        // Black in the data, white in the resource.
        assert_eq!(sun.color, Color::linear_rgb(1.0, 1.0, 1.0));
    }

    #[test]
    fn despawning_takes_every_node_and_every_resource_with_it() {
        let mut app = test_app();
        spawn_minimal(&mut app);
        assert_eq!(
            app.world_mut().query::<&NodeId>().iter(app.world()).count(),
            3
        );

        app.world_mut().trigger(DespawnLevel);
        app.update();

        assert_eq!(
            app.world_mut().query::<&NodeId>().iter(app.world()).count(),
            0
        );
        assert_eq!(
            app.world_mut()
                .query::<&LevelRoot>()
                .iter(app.world())
                .count(),
            0
        );
        assert!(app.world().get_resource::<NodeIndex>().is_none());
        assert!(app.world().get_resource::<WorldEnv>().is_none());
        assert!(app.world().get_resource::<PartitionGrid>().is_none());
    }

    #[test]
    fn a_mirroring_axis_convention_moves_a_transform_with_the_geometry() {
        let mirrored = AxisConvention::parse("x,y,-z").expect("a permutation");

        let transform = transform_of(
            TransformDesc::Matrix {
                rotation: Mat3::IDENTITY,
                translation: Vec3::new(1.0, 2.0, 3.0),
            },
            &mirrored,
        );

        assert_eq!(transform.translation, Vec3::new(1.0, 2.0, -3.0));
    }

    #[test]
    fn a_rotation_conjugated_by_the_convention_stays_a_rotation() {
        // Under a mirroring convention the naive `axes.point` of each column
        // would flip the determinant and `Quat::from_mat3` would produce
        // nonsense; conjugating keeps it proper.
        let mirrored = AxisConvention::parse("x,y,-z").expect("a permutation");
        let quarter_about_y = Mat3::from_rotation_y(std::f32::consts::FRAC_PI_2);

        let transform = transform_of(
            TransformDesc::Matrix {
                rotation: quarter_about_y,
                translation: Vec3::ZERO,
            },
            &mirrored,
        );

        assert!(
            transform.rotation.is_normalized(),
            "{:?}",
            transform.rotation
        );
        // Mirroring z reverses the sense of a y rotation.
        let turned = transform.rotation * Vec3::X;
        assert!((turned - Vec3::Z).length() < 1e-5, "{turned:?}");
    }

    #[test]
    fn a_rotate_translate_scale_transform_keeps_all_three_parts() {
        let transform = transform_of(
            TransformDesc::RotateTranslateScale {
                rotate: Vec3::new(0.0, -1.221_730_5, 0.0),
                translate: Vec3::new(2417.0, 0.0, 1117.0),
                scale: Vec3::new(2.0, 2.0, 2.0),
            },
            &AxisConvention::GAMEZ_DEFAULT,
        );

        assert_eq!(transform.translation, Vec3::new(2417.0, 0.0, 1117.0));
        assert_eq!(transform.scale, Vec3::splat(2.0));
        // m1's `bft_00`: -70 degrees about y.
        let expected = Quat::from_rotation_y(-1.221_730_5);
        assert!(transform.rotation.abs_diff_eq(expected, 1e-6));
    }

    #[test]
    fn a_degenerate_matrix_does_not_produce_a_nan_rotation() {
        // No Recoil node has one, but a zero matrix would otherwise reach
        // `Quat::from_mat3` and poison every child's global transform.
        let transform = transform_of(
            TransformDesc::Matrix {
                rotation: Mat3::ZERO,
                translation: Vec3::ZERO,
            },
            &AxisConvention::GAMEZ_DEFAULT,
        );

        assert!(transform.rotation.is_finite());
        assert!(transform.rotation.is_normalized());
    }
}
