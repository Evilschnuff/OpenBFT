//! Water, lava and quicksand volumes, built from polygon soil (strategy B).
//!
//! docs/data/water.md settled this: node names are a debug aid, **soil is the
//! authority**. `Soil::Water`, `Lava` and `Quicksand` are set on exactly the
//! liquid polygons (1–2 % of a level), and the original reader keys gravity,
//! slowdowns and weapon impacts on them. So a volume is built from the
//! triangles, not from the `water*` / `underwater*` / `lava*` names — many
//! liquid polygons sit in anonymous `gNNNN` group nodes and would be missed
//! by a name scan.
//!
//! # The two regimes
//!
//! - **Shallow** (`waterNN`, m1 zone 32): `Soil::Water` polygons that *are*
//!   the altitude surface. Nothing underneath, so the sub is not allowed:
//!   [`WaterVolume::floor_y`] is `None`.
//! - **Deep** (zone 14): a `Soil::Water` surface polygon with *no* collision
//!   flags (`waterupNN`) over a `Soil::Default` floor 24 m below that has
//!   them (`underwaterNN`). The sub's volume is between the two, so
//!   `floor_y` is the floor's height.
//!
//! Both are found the same way: group the liquid triangles into connected
//! regions, take their height as the surface, and look down for an
//! altitude-surface triangle. A floor within [`SHALLOW_DEPTH`] means there is
//! no room to dive, which is the shallow case.
//!
//! # Water is a sensor, never ground
//!
//! Every liquid triangle becomes a [`GameLayer::Water`] sensor collider
//! whether or not its node carries `altitude_surface`, because the amphib
//! rides on `waterup*` surfaces the data does not flag (docs/data/water.md's inference).
//! Only flagged triangles are also `Layer::Level`, and that part is the
//! level spawn's.

use avian3d::prelude::*;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::geometry::{material_per_triangle, triangulate};
use bft_assets::level::LevelData;
use bft_assets::materials::Soil;

use crate::level::{GameLayer, LevelStatic, NodeId};
use crate::physics::layers;

/// How deep the floor has to be below a liquid surface for the volume to
/// count as divable.
///
/// No floor within 2 m means shallow. m1's deep
/// pools are 24 m; its shallow sea rides on the polygon itself, so the two
/// cases are nowhere near this line.
pub const SHALLOW_DEPTH: f32 = 2.0;

/// How far below a region's centroid to look for a floor.
///
/// Comfortably past m1's 24 m pools without casting the height of a level.
pub const FLOOR_SEARCH: f32 = 200.0;

/// How far apart two liquid triangles may be and still count as one region.
///
/// Connectivity is by shared vertex, but the data's water polygons are
/// authored as separate meshes that meet at a seam, so an exact match would
/// split one pool into several. A centimetre is well under the smallest
/// water polygon and well over f32 noise at level coordinates.
pub const WELD_EPSILON: f32 = 0.01;

/// Which liquid a volume is.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LiquidKind {
    Water,
    Lava,
    Quicksand,
}

impl LiquidKind {
    /// The kind a soil names, or `None` for a solid surface.
    pub fn from_soil(soil: Soil) -> Option<LiquidKind> {
        match soil {
            Soil::Water => Some(LiquidKind::Water),
            Soil::Lava => Some(LiquidKind::Lava),
            Soil::Quicksand => Some(LiquidKind::Quicksand),
            _ => None,
        }
    }
}

/// One connected body of liquid.
#[derive(Component, Debug, Clone, PartialEq)]
pub struct WaterVolume {
    /// The height of the liquid surface. Flat in every level checked.
    pub surface_y: f32,
    /// The floor below it, or `None` for a shallow volume the sub may not
    /// enter.
    pub floor_y: Option<f32>,
    /// The region's x/z footprint: the liquid triangles projected down.
    pub footprint: Vec<[Vec2; 3]>,
    pub kind: LiquidKind,
}

impl WaterVolume {
    /// Whether an x/z point is inside the footprint.
    pub fn contains_xz(&self, point: Vec2) -> bool {
        self.footprint
            .iter()
            .any(|triangle| point_in_triangle(point, triangle))
    }

    /// Whether a sub may be in this volume: there has to be a floor, and
    /// room between it and the surface.
    pub fn is_divable(&self) -> bool {
        self.floor_y
            .is_some_and(|floor| self.surface_y - floor >= SHALLOW_DEPTH)
    }
}

/// What [`water_at`] found at a position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaterSample {
    pub surface_y: f32,
    /// `None` = shallow: no floor below, so no sub.
    pub floor_y: Option<f32>,
    pub kind: LiquidKind,
}

/// The liquid at a world position, if any.
///
/// "At" means inside the footprint in x/z and at or below the surface in y —
/// a vehicle flying over the sea is not in it. When volumes overlap in x/z
/// (a lava flow running into water), the one whose surface is nearest above
/// `pos` wins, which is the one actually around the body.
pub fn water_at(volumes: &Query<&WaterVolume>, pos: Vec3) -> Option<WaterSample> {
    let xz = pos.xz();

    volumes
        .iter()
        .filter(|volume| pos.y <= volume.surface_y && volume.contains_xz(xz))
        .min_by(|a, b| {
            (a.surface_y - pos.y)
                .abs()
                .total_cmp(&(b.surface_y - pos.y).abs())
        })
        .map(|volume| WaterSample {
            surface_y: volume.surface_y,
            floor_y: volume.floor_y,
            kind: volume.kind,
        })
}

/// One liquid triangle in world space, as the pass collects it.
#[derive(Debug, Clone, Copy)]
struct LiquidTriangle {
    kind: LiquidKind,
    corners: [Vec3; 3],
    /// The node it came from, for the debug overlay and the name check.
    node: u32,
}

impl LiquidTriangle {
    fn centroid(&self) -> Vec3 {
        (self.corners[0] + self.corners[1] + self.corners[2]) / 3.0
    }
}

/// Everything the water pass produced, for the caller to spawn and log.
#[derive(Debug, Clone, Default)]
pub struct WaterRegions {
    /// One entry per connected region, each ready to become a
    /// [`WaterVolume`] plus its collider triangles.
    pub regions: Vec<WaterRegion>,
    /// Nodes whose name says water but whose polygons carry no liquid soil,
    /// and the other way round: a consistency check.
    pub name_mismatches: Vec<String>,
}

/// One region: the volume and the geometry it was built from.
#[derive(Debug, Clone, PartialEq)]
pub struct WaterRegion {
    pub volume: WaterVolume,
    /// World-space triangle corners, for the sensor collider.
    pub triangles: Vec<[Vec3; 3]>,
    /// The nodes that contributed, for the debug overlay.
    pub nodes: Vec<u32>,
}

/// Find every liquid region in a level (strategy B).
///
/// Pure over [`LevelData`]: no `World`, no physics, so it is testable on a
/// hand-built two-box level and runs before anything is spawned. The floor
/// search is a geometric ray against the level's own altitude-surface
/// triangles rather than a `SpatialQuery`, for the same reason — the
/// colliders do not exist yet at spawn time.
pub fn find_water(level: &LevelData, axes: &AxisConvention) -> WaterRegions {
    let (liquid, floors) = collect_surfaces(level, axes);
    let name_mismatches = check_names(level, &liquid);

    let groups = group_connected(&liquid);
    let regions = groups
        .into_iter()
        .filter_map(|indices| build_region(&liquid, &indices, &floors))
        .collect();

    WaterRegions {
        regions,
        name_mismatches,
    }
}

/// Walk every model-bearing node once, splitting its triangles into the
/// liquid ones (any node, flagged or not) and the ground ones (flagged only).
fn collect_surfaces(
    level: &LevelData,
    axes: &AxisConvention,
) -> (Vec<LiquidTriangle>, Vec<[Vec3; 3]>) {
    let transforms = world_transforms(level, axes);
    let mut liquid = Vec::new();
    let mut floors = Vec::new();

    for (id, node) in level.nodes.iter().enumerate() {
        let common = node.common();
        let Some(model_index) = common.model_index else {
            continue;
        };
        let Some(model) = level.models.get(model_index) else {
            continue;
        };
        // The floor a sub sits on is an `altitude_surface`; a wall it bumps
        // into (`intersect_surface` only) is not something to land on, which
        // is how DESIGN §7.1 reads `intersect_surface` alone.
        let is_floor = common.flags.altitude_surface;

        if model.polygons.is_empty() {
            continue;
        }

        let triangulated = triangulate(model, axes);
        let materials = material_per_triangle(model.model_type, &triangulated);

        if materials.is_empty() {
            continue;
        }

        let to_world = transforms[id];
        let (positions, indices) =
            bft_assets::geometry::collider_indices(model.model_type, &triangulated);

        for (triangle, corner_indices) in indices.iter().enumerate() {
            let corners = [
                to_world.transform_point(positions[corner_indices[0] as usize]),
                to_world.transform_point(positions[corner_indices[1] as usize]),
                to_world.transform_point(positions[corner_indices[2] as usize]),
            ];
            let soil = materials
                .get(triangle)
                .and_then(|index| level.materials.get(*index as usize))
                .map(|material| material.soil())
                .unwrap_or_default();

            match LiquidKind::from_soil(soil) {
                Some(kind) => liquid.push(LiquidTriangle {
                    kind,
                    corners,
                    node: id as u32,
                }),
                // Only solid, flagged geometry is a candidate floor: a
                // `Soil::Water` triangle is never what a sub rests on.
                None if is_floor => floors.push(corners),
                None => {}
            }
        }
    }

    (liquid, floors)
}

/// Each node's world transform, composed down the parent chain.
///
/// `parent_indices` is the hierarchy authority (docs/data/nodes.md) and a
/// node's parent always precedes it in the array in the shipped levels; the
/// fallback handles data where it does not, by walking up on demand.
fn world_transforms(level: &LevelData, axes: &AxisConvention) -> Vec<GlobalTransform> {
    let local: Vec<Transform> = level
        .nodes
        .iter()
        .map(|node| crate::level::node_transform(node, axes))
        .collect();
    let mut world: Vec<Option<GlobalTransform>> = vec![None; level.nodes.len()];

    for id in 0..level.nodes.len() {
        resolve_transform(level, &local, &mut world, id, 0);
    }

    world
        .into_iter()
        .map(|transform| transform.unwrap_or_default())
        .collect()
}

/// Compose one node's world transform, memoised.
///
/// `depth` guards against the cycle m6's out-of-range parent could make if a
/// future loader stopped dropping it; the shipped data is a forest.
fn resolve_transform(
    level: &LevelData,
    local: &[Transform],
    world: &mut Vec<Option<GlobalTransform>>,
    id: usize,
    depth: usize,
) -> GlobalTransform {
    if let Some(transform) = world[id] {
        return transform;
    }

    let parent = level.nodes[id]
        .common()
        .parent
        .map(|parent| parent as usize);
    let transform = match parent {
        Some(parent) if parent < level.nodes.len() && depth < level.nodes.len() => {
            resolve_transform(level, local, world, parent, depth + 1) * local[id]
        }
        _ => GlobalTransform::from(local[id]),
    };

    world[id] = Some(transform);
    transform
}

/// Group liquid triangles into connected regions by welded shared vertices.
///
/// Union-find over a vertex hash: triangles sharing a corner (to
/// [`WELD_EPSILON`]) are one region, and so are triangles of different kinds
/// that touch — which is why the kind is decided per region below rather than
/// used to split regions. Lava and water do not in fact share a seam in any shipped
/// level, but a level that did should give one region with one behaviour
/// rather than two overlapping ones.
fn group_connected(liquid: &[LiquidTriangle]) -> Vec<Vec<usize>> {
    let mut parents: Vec<usize> = (0..liquid.len()).collect();
    let mut by_vertex: HashMap<[i64; 3], usize> = HashMap::new();

    for (index, triangle) in liquid.iter().enumerate() {
        for corner in triangle.corners {
            match by_vertex.entry(weld_key(corner)) {
                bevy::platform::collections::hash_map::Entry::Occupied(entry) => {
                    union(&mut parents, index, *entry.get());
                }
                bevy::platform::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(index);
                }
            }
        }
    }

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();

    for index in 0..liquid.len() {
        groups
            .entry(find(&mut parents, index))
            .or_default()
            .push(index);
    }

    let mut groups: Vec<Vec<usize>> = groups.into_values().collect();
    // Deterministic order: the sim must spawn the same volumes in the same
    // order on every machine (DESIGN §9), and a `HashMap`'s iteration is not.
    groups.sort_unstable_by_key(|group| group[0]);
    groups
}

/// A vertex's cell in a [`WELD_EPSILON`] grid, as a hashable key.
fn weld_key(point: Vec3) -> [i64; 3] {
    [
        (point.x / WELD_EPSILON).round() as i64,
        (point.y / WELD_EPSILON).round() as i64,
        (point.z / WELD_EPSILON).round() as i64,
    ]
}

fn find(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }

    index
}

fn union(parents: &mut [usize], a: usize, b: usize) {
    let (a, b) = (find(parents, a), find(parents, b));

    if a != b {
        // Lower index wins, so the group key is stable across runs.
        let (keep, drop) = if a < b { (a, b) } else { (b, a) };
        parents[drop] = keep;
    }
}

/// Turn one group of triangle indices into a [`WaterRegion`].
fn build_region(
    liquid: &[LiquidTriangle],
    indices: &[usize],
    floors: &[[Vec3; 3]],
) -> Option<WaterRegion> {
    // An empty group has no surface, no centroid and no footprint; the `?`
    // is what stops the fold below from returning -inf.
    indices.first()?;
    let kind = dominant_kind(liquid, indices);

    // The surface is the *top* of the region, not its mean: `waterup*` is one
    // flat plane in every level checked, but a region that welded a sloping
    // seam in should still report the height a body floats at.
    let surface_y = indices
        .iter()
        .flat_map(|index| liquid[*index].corners)
        .map(|corner| corner.y)
        .fold(f32::NEG_INFINITY, f32::max);

    let centroid = indices
        .iter()
        .map(|index| liquid[*index].centroid())
        .fold(Vec3::ZERO, |sum, point| sum + point)
        / indices.len() as f32;

    let floor_y = floor_below(floors, Vec3::new(centroid.x, surface_y, centroid.z));
    let triangles: Vec<[Vec3; 3]> = indices.iter().map(|index| liquid[*index].corners).collect();
    let footprint = triangles
        .iter()
        .map(|corners| [corners[0].xz(), corners[1].xz(), corners[2].xz()])
        .collect();

    let mut nodes: Vec<u32> = indices.iter().map(|index| liquid[*index].node).collect();
    nodes.sort_unstable();
    nodes.dedup();

    Some(WaterRegion {
        volume: WaterVolume {
            surface_y,
            // A floor the vehicle could stand on without diving is not a
            // floor for this purpose: the volume is shallow.
            floor_y: floor_y.filter(|floor| surface_y - floor >= SHALLOW_DEPTH),
            footprint,
            kind,
        },
        triangles,
        nodes,
    })
}

/// The kind most of a region's triangles are.
fn dominant_kind(liquid: &[LiquidTriangle], indices: &[usize]) -> LiquidKind {
    let count = |kind: LiquidKind| {
        indices
            .iter()
            .filter(|index| liquid[**index].kind == kind)
            .count()
    };

    [LiquidKind::Water, LiquidKind::Lava, LiquidKind::Quicksand]
        .into_iter()
        .max_by_key(|kind| count(*kind))
        .unwrap_or(LiquidKind::Water)
}

/// The highest altitude-surface triangle below a point, by ray cast down.
fn floor_below(floors: &[[Vec3; 3]], from: Vec3) -> Option<f32> {
    let mut best: Option<f32> = None;
    // Bounded, not open-ended: a puddle on a clifftop must not adopt the
    // valley floor 300 m down as its sea bed. m1's deep pools are 24 m, the
    // deepest docs/data/water.md records, so [`FLOOR_SEARCH`] is an order of magnitude of
    // slack rather than a tight fit.
    let floor_limit = from.y - FLOOR_SEARCH;

    for triangle in floors {
        let Some(y) = ray_down_hit(*triangle, from) else {
            continue;
        };
        // Below the surface, and the nearest such: a sea floor under a pool,
        // not the cave roof two levels down.
        if y < from.y && y >= floor_limit && best.is_none_or(|best| y > best) {
            best = Some(y);
        }
    }

    best
}

/// Where a straight-down ray from `from` crosses a triangle, if it does.
///
/// Barycentric, in x/z: the ray is vertical, so the triangle's projection
/// decides containment and the plane decides the height. A triangle seen
/// edge-on projects to zero area and is skipped — it is a wall, not a floor.
fn ray_down_hit(triangle: [Vec3; 3], from: Vec3) -> Option<f32> {
    let flat = [triangle[0].xz(), triangle[1].xz(), triangle[2].xz()];
    let barycentric = barycentric(from.xz(), &flat)?;

    Some(
        barycentric.x * triangle[0].y
            + barycentric.y * triangle[1].y
            + barycentric.z * triangle[2].y,
    )
}

/// A point's barycentric coordinates in an x/z triangle, `None` when outside
/// or when the triangle is degenerate.
fn barycentric(point: Vec2, triangle: &[Vec2; 3]) -> Option<Vec3> {
    let (v0, v1) = (triangle[1] - triangle[0], triangle[2] - triangle[0]);
    let area = v0.perp_dot(v1);

    if area.abs() < f32::EPSILON {
        return None;
    }

    let v2 = point - triangle[0];
    let b = v2.perp_dot(v1) / area;
    let c = v0.perp_dot(v2) / area;
    let a = 1.0 - b - c;

    // A small tolerance so a point exactly on a shared edge belongs to one
    // of the two triangles rather than to neither.
    const EDGE: f32 = -1e-5;

    (a >= EDGE && b >= EDGE && c >= EDGE).then_some(Vec3::new(a, b, c))
}

/// Whether an x/z point is inside a triangle.
fn point_in_triangle(point: Vec2, triangle: &[Vec2; 3]) -> bool {
    barycentric(point, triangle).is_some()
}

/// Cross-check the node names against the soil.
///
/// Names are *not* used to build the volume — they are a secondary signal
/// (docs/data/water.md) — but a node named `waterup03` that carries no water soil means
/// either the data or this reading has changed, and that is worth one
/// warning.
///
/// The *floor* names are excluded, and that is the whole subtlety: docs/data/water.md's
/// deep-water pattern is a liquid surface over a **`Soil::Default` floor**,
/// so `underwaterNN` and `oceanfloorN` are correctly not liquid. Measured
/// over the 13 levels, warning on them produces 14 false positives (m1 3,
/// m6 2, m11 6, …) and no true ones. With them excluded the only remaining
/// warnings are m4's `water_level`/`water_level2` and m6's `a2lava`, which
/// are genuinely named for liquid and carry none.
fn check_names(level: &LevelData, liquid: &[LiquidTriangle]) -> Vec<String> {
    let mut liquid_nodes: Vec<u32> = liquid.iter().map(|triangle| triangle.node).collect();
    liquid_nodes.sort_unstable();
    liquid_nodes.dedup();

    let mut mismatches = Vec::new();

    for (id, node) in level.nodes.iter().enumerate() {
        let name = node.common().name.as_str();
        let has_soil = liquid_nodes.binary_search(&(id as u32)).is_ok();

        // A named node with no model is the attachment point `water_miss`,
        // which every level has and which is not a surface; not a mismatch.
        if is_liquid_name(name)
            && !is_floor_name(name)
            && !has_soil
            && node.common().model_index.is_some()
        {
            mismatches.push(format!("{name} is named for liquid but has no liquid soil"));
        }
    }

    mismatches
}

/// Whether a name is a liquid *floor* — the solid surface under a volume,
/// which docs/data/water.md says carries `Soil::Default` and is not itself liquid.
pub fn is_floor_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();

    name.starts_with("underwater") || name.contains("floor") || name.contains("bottom")
}

/// Whether a node name is one of docs/data/water.md's liquid families.
pub fn is_liquid_name(name: &str) -> bool {
    // The families docs/data/nodes.md counted with the regex `water|ocean|sea`,
    // plus docs/data/water.md's lava and quicksand. Matched anywhere in the
    // name, not just at the start: `underwaterNN` and `clwaterN` are both water
    // nodes.
    const FAMILIES: [&str; 5] = ["water", "ocean", "lava", "quicksand", "sea"];

    // `water_miss` is the impact-effect anchor in every level, not a surface.
    if name == "water_miss" {
        return false;
    }

    let name = name.to_ascii_lowercase();
    FAMILIES.iter().any(|family| name.contains(family))
}

/// Spawn one entity per region: a [`WaterVolume`] with a sensor collider.
///
/// The collider is built in world space and the entity left at the identity,
/// because a region welds triangles from several nodes that have no common
/// parent below the level root. **That makes it correct only while the level
/// root is itself the identity**, which [`spawn_level`](crate::level::
/// spawn_level) guarantees — a root that moved would move the geometry and
/// the water apart, since one is in node-local space and the other already
/// composed. `WaterVolume`'s own `surface_y` and `footprint` are world-space
/// numbers for the same reason: [`water_at`] compares them against a world
/// position, with no transform in between.
pub fn spawn_water(commands: &mut Commands, root: Entity, regions: &WaterRegions) -> usize {
    for mismatch in &regions.name_mismatches {
        warn!("water name check: {mismatch}");
    }

    let mut spawned = 0;

    for region in &regions.regions {
        let Some(collider) = sensor_collider(&region.triangles) else {
            continue;
        };

        commands.spawn((
            Name::new(format!("{:?} volume", region.volume.kind)),
            region.volume.clone(),
            region.volume.kind,
            LevelStatic,
            Transform::IDENTITY,
            collider,
            RigidBody::Static,
            // A sensor reports overlap and never pushes: a vehicle drives
            // into water, it does not bounce off it.
            Sensor,
            layers::water(),
            ChildOf(root),
        ));
        spawned += 1;
    }

    info!(
        "water: {spawned} volume(s) from {} region(s), {} name warning(s)",
        regions.regions.len(),
        regions.name_mismatches.len()
    );

    spawned
}

/// A trimesh sensor over a region's triangles.
fn sensor_collider(triangles: &[[Vec3; 3]]) -> Option<Collider> {
    if triangles.is_empty() {
        return None;
    }

    let mut vertices = Vec::with_capacity(triangles.len() * 3);
    let mut indices = Vec::with_capacity(triangles.len());

    for triangle in triangles {
        let base = vertices.len() as u32;
        vertices.extend_from_slice(triangle);
        indices.push([base, base + 1, base + 2]);
    }

    Some(Collider::trimesh(vertices, indices))
}

/// Which node ids a region drew from, for the debug overlay.
pub fn region_nodes(region: &WaterRegion) -> impl Iterator<Item = NodeId> + '_ {
    region.nodes.iter().copied().map(NodeId)
}

/// The layer water volumes are on, re-exported so a caller need not reach
/// into [`crate::level`].
pub const WATER_LAYER: GameLayer = GameLayer::Water;

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::level::parse_gamez;
    use bft_assets::test_support::{WORLD_NODE, gamez_from};

    /// A level of two stacked boxes: a `Soil::Water` plane at y = 0 over a
    /// `Soil::Default` floor at y = -10, both nodes flagged as the data
    /// flags them — the deep-water pattern of docs/data/water.md in miniature.
    ///
    /// `floor_depth` moves the floor, so one fixture covers deep and shallow.
    fn two_box_level(floor_depth: f32) -> LevelData {
        let plane = |y: f32, material: u32| {
            format!(
                r#"{{
                  "model_type": "Default", "facade_mode": "CylindricalY",
                  "flags": {{ "lighting": true, "fog": true, "clouds": false,
                             "facade_centroid": false }},
                  "vertices": [
                    {{ "x": -5.0, "y": {y}, "z": -5.0 }},
                    {{ "x": -5.0, "y": {y}, "z":  5.0 }},
                    {{ "x":  5.0, "y": {y}, "z":  5.0 }},
                    {{ "x":  5.0, "y": {y}, "z": -5.0 }}
                  ],
                  "normals": [], "morphs": [], "lights": [],
                  "polygons": [ {{
                    "flags": {{ "show_backface": false, "tri_strip": false }},
                    "priority": 0, "zone_set": [],
                    "vertex_indices": [0, 1, 2, 3],
                    "normal_indices": null, "vertex_colors": [],
                    "materials": [ {{ "material_index": {material}, "uv_coords": null }} ]
                  }} ],
                  "texture_scroll": {{ "u": 0.0, "v": 0.0 }},
                  "bbox_mid": {{ "x": 0.0, "y": {y}, "z": 0.0 }}, "bbox_diag": 14.1
                }}"#
            )
        };

        // The surface is `waterup`-shaped: liquid soil, no collision flags.
        // The floor is `underwater`-shaped: default soil, both flags.
        let nodes = format!(
            r#"[{WORLD_NODE},
              {{ "name": "waterup01", "zone_id": 14, "model_index": 0,
                 "parent_indices": [0], "child_indices": [],
                 "flags": {{ "active": true, "tree_valid": true }},
                 "data": {{ "Object3d": {{ "transform": "Initial", "signs": 0 }} }} }},
              {{ "name": "underwater01", "zone_id": 14, "model_index": 1,
                 "parent_indices": [0], "child_indices": [],
                 "flags": {{ "active": true, "tree_valid": true,
                            "altitude_surface": true, "intersect_surface": true }},
                 "data": {{ "Object3d": {{ "transform": "Initial", "signs": 0 }} }} }}]"#
        );

        let gamez = gamez_from(
            r#"[{ "Colored": { "color": { "r": 200.0, "g": 200.0, "b": 200.0 }, "alpha": 255, "soil": "Default" } },
                { "Colored": { "color": { "r": 0.0, "g": 0.0, "b": 255.0 }, "alpha": 255, "soil": "Water" } }]"#,
            r#"[]"#,
            &format!("[{},{}]", plane(0.0, 1), plane(-floor_depth, 0)),
            &nodes,
        );

        parse_gamez(&gamez, "water.gamez.zip").expect("the fixture loads")
    }

    fn axes() -> AxisConvention {
        AxisConvention::GAMEZ_DEFAULT
    }

    #[test]
    fn a_water_plane_over_a_floor_is_one_deep_volume() {
        let regions = find_water(&two_box_level(10.0), &axes());

        assert_eq!(regions.regions.len(), 1, "one connected pool");

        let volume = &regions.regions[0].volume;
        assert_eq!(volume.kind, LiquidKind::Water);
        assert_eq!(volume.surface_y, 0.0);
        assert_eq!(volume.floor_y, Some(-10.0), "the underwater floor");
        assert!(volume.is_divable(), "24 m of water is a sub volume");
        assert_eq!(volume.footprint.len(), 2, "one quad, two triangles");
    }

    #[test]
    fn a_floor_within_a_couple_of_metres_makes_the_volume_shallow() {
        let regions = find_water(&two_box_level(1.0), &axes());
        let volume = &regions.regions[0].volume;

        assert_eq!(
            volume.floor_y, None,
            "a floor closer than SHALLOW_DEPTH is no room to dive"
        );
        assert!(!volume.is_divable());
    }

    #[test]
    fn the_footprint_answers_where_the_water_is() {
        let regions = find_water(&two_box_level(10.0), &axes());
        let volume = &regions.regions[0].volume;

        assert!(volume.contains_xz(Vec2::new(0.0, 0.0)), "the middle");
        assert!(volume.contains_xz(Vec2::new(4.9, -4.9)), "inside a corner");
        assert!(!volume.contains_xz(Vec2::new(6.0, 0.0)), "past the edge");
    }

    #[test]
    fn a_level_with_no_liquid_soil_has_no_volumes() {
        let level =
            parse_gamez(&bft_assets::test_support::minimal_gamez(), "m.gamez.zip").expect("loads");

        assert!(find_water(&level, &axes()).regions.is_empty());
    }

    #[test]
    fn the_liquid_name_families_match_the_data_notes() {
        for name in [
            "water02",
            "waterup01",
            "underwater01",
            "ocean1",
            "lavascroll2",
            "clwater1",
            "wateraz",
        ] {
            assert!(is_liquid_name(name), "{name} should read as liquid");
        }

        // The impact anchor every level has is not a surface, and ordinary
        // geometry must not trip the check.
        assert!(!is_liquid_name("water_miss"));
        assert!(!is_liquid_name("g0042"));
        assert!(!is_liquid_name("surf1"));
    }

    /// docs/data/water.md's deep-water floor is `Soil::Default` on purpose, so the name
    /// check must not report it. Without this the 13 levels produce 14
    /// warnings, every one of them wrong.
    #[test]
    fn the_floors_under_a_volume_are_not_reported_as_missing_liquid() {
        for name in ["underwater01", "oceanfloor3", "seabottom"] {
            assert!(is_liquid_name(name), "{name} is in a liquid family");
            assert!(is_floor_name(name), "{name} is a floor, not a surface");
        }

        for name in ["waterup01", "water02", "lavapool1"] {
            assert!(!is_floor_name(name), "{name} is a surface, not a floor");
        }
    }

    #[test]
    fn a_sample_below_the_surface_reports_the_volume_and_one_above_does_not() {
        let mut world = World::new();
        let volume = find_water(&two_box_level(10.0), &axes()).regions[0]
            .volume
            .clone();
        world.spawn(volume);

        let mut sample = |pos: Vec3| {
            world
                .run_system_cached_with(
                    |In(pos): In<Vec3>, volumes: Query<&WaterVolume>| water_at(&volumes, pos),
                    pos,
                )
                .expect("the system runs")
        };

        let below = sample(Vec3::new(0.0, -3.0, 0.0)).expect("in the water");
        assert_eq!(below.surface_y, 0.0);
        assert_eq!(below.floor_y, Some(-10.0));
        assert_eq!(below.kind, LiquidKind::Water);

        assert!(sample(Vec3::new(0.0, 5.0, 0.0)).is_none(), "above it");
        assert!(sample(Vec3::new(50.0, -3.0, 0.0)).is_none(), "beside it");
    }

    /// Water must be sensed, not stood on: a `Layer::Water` collider that
    /// answered a `Layer::Level` ground probe would float the sub on the sea
    /// surface it is supposed to dive through.
    #[test]
    fn water_volumes_are_sensors_on_their_own_layer() {
        let mut app = App::new();
        app.add_plugins(bevy::app::TaskPoolPlugin::default());

        let regions = find_water(&two_box_level(10.0), &axes());
        let root = app.world_mut().spawn(Transform::IDENTITY).id();

        app.world_mut()
            .run_system_cached_with(
                move |In((root, regions)): In<(Entity, WaterRegions)>, mut commands: Commands| {
                    spawn_water(&mut commands, root, &regions);
                },
                (root, regions),
            )
            .expect("the system runs");
        app.update();

        let mut query = app
            .world_mut()
            .query::<(&WaterVolume, &Sensor, &CollisionLayers)>();
        let (_, _, layers) = query.single(app.world()).expect("one volume");

        assert_eq!(*layers, layers::water());
    }
}
