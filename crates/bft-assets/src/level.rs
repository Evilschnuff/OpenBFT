//! A `gamez` ZIP → a [`LevelData`] asset, and a `zmap` JSON → a [`ZMap`].
//!
//! The ZIP holds exactly five documents — `metadata.json`, `textures.json`,
//! `materials.json`, `models.json`, `nodes.json` — and this module turns them
//! into one engine-owned description. Nothing here touches `bevy_render`:
//! `LevelData` is a pile of numbers and names, so a headless server loads the
//! same asset the client draws (NFR-4).
//!
//! # The shapes here are the *emitted* JSON, not `api-types`
//!
//! A node is flat, with its variant under `data`, and `parent_indices` —
//! **not** `child_indices` — is the hierarchy authority (docs/data/nodes.md).
//! [`crate::mech3ax`] mirrors that; this module converts it and validates what
//! the spawner will rely on.

use std::io::Read;

use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::prelude::*;

use crate::materials::{MaterialDesc, material_descs};
use crate::mech3ax;
use crate::models::{ModelDesc, model_descs};
use crate::textures::texture_key;

/// One level's whole content: what to draw, and how it is arranged.
#[derive(Asset, TypePath, Debug, Clone, Default)]
pub struct LevelData {
    /// Texture names this level refers to, already normalised by
    /// [`texture_key`] so they match archive stems (docs/data/materials.md).
    pub texture_names: Vec<String>,
    pub materials: Vec<MaterialDesc>,
    pub models: Vec<ModelDesc>,
    /// Every node, **in file order** — the order node ids index
    /// (docs/data/nodes.md).
    pub nodes: Vec<NodeDesc>,
    /// The world environment, from the single `World` node.
    pub world: WorldDesc,
    /// The `World` node's own id, so the spawner can tell it from the other
    /// root trees.
    pub world_node: u32,
    /// Nodes with no parent. **180–220 of them per level**, not one: the
    /// World, the scene-setup singletons, and the pickup, enemy and effect
    /// templates (docs/data/nodes.md).
    pub root_nodes: Vec<u32>,
    /// The sun, from the single `Light` node, when the level has one.
    pub sun: Option<LightDesc>,
}

impl LevelData {
    /// One node by id, or `None` if the id is out of range.
    pub fn node(&self, id: u32) -> Option<&NodeDesc> {
        self.nodes.get(id as usize)
    }

    /// How many nodes of each variant, for logs and tests.
    pub fn census(&self) -> NodeCensus {
        let mut census = NodeCensus::default();

        for node in &self.nodes {
            match node {
                NodeDesc::World(_) => census.world += 1,
                NodeDesc::Object3d(_) => census.object3d += 1,
                NodeDesc::Lod(_) => census.lod += 1,
                NodeDesc::Light(_) => census.light += 1,
                NodeDesc::Empty(_) => census.empty += 1,
                NodeDesc::Camera(_) => census.camera += 1,
                NodeDesc::Display(_) => census.display += 1,
                NodeDesc::Window(_) => census.window += 1,
            }
        }

        census
    }
}

/// How many nodes of each variant a level has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct NodeCensus {
    pub world: usize,
    pub object3d: usize,
    pub lod: usize,
    pub light: usize,
    pub empty: usize,
    pub camera: usize,
    pub display: usize,
    pub window: usize,
}

/// One node, by variant.
///
/// `Camera`, `Display` and `Window` are the original renderer's settings: kept
/// so the census is complete and so the viewer can compare its own camera
/// against the shipped field of view, but nothing is spawned from them.
#[derive(Debug, Clone, PartialEq)]
pub enum NodeDesc {
    World(WorldNodeDesc),
    Object3d(Object3dDesc),
    Lod(LodDesc),
    Light(LightDesc),
    Empty(EmptyDesc),
    Camera(CameraDesc),
    Display(DisplayDesc),
    Window(WindowDesc),
}

impl NodeDesc {
    /// Every variant carries these, because they are read from the node's
    /// flat top level rather than from `data` (docs/data/nodes.md).
    pub fn common(&self) -> &NodeCommon {
        match self {
            NodeDesc::World(n) => &n.common,
            NodeDesc::Object3d(n) => &n.common,
            NodeDesc::Lod(n) => &n.common,
            NodeDesc::Light(n) => &n.common,
            NodeDesc::Empty(n) => &n.common,
            NodeDesc::Camera(n) => &n.common,
            NodeDesc::Display(n) => &n.common,
            NodeDesc::Window(n) => &n.common,
        }
    }

    /// The node's name, which is how interp and anim refer to it.
    pub fn name(&self) -> &str {
        &self.common().name
    }

    /// The variant's name, for logs.
    pub fn kind(&self) -> &'static str {
        match self {
            NodeDesc::World(_) => "World",
            NodeDesc::Object3d(_) => "Object3d",
            NodeDesc::Lod(_) => "Lod",
            NodeDesc::Light(_) => "Light",
            NodeDesc::Empty(_) => "Empty",
            NodeDesc::Camera(_) => "Camera",
            NodeDesc::Display(_) => "Display",
            NodeDesc::Window(_) => "Window",
        }
    }
}

/// What every node carries, whatever its variant.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NodeCommon {
    pub name: String,
    pub flags: NodeFlagsDesc,
    /// `None` for `zone_id == -1`. Pairs with anim's `Execution::ByZone`
    /// (docs/data/nodes.md); zones 14 and 32 are m1's water (docs/data/water.md).
    pub zone_id: Option<i32>,
    /// Index into [`LevelData::models`], or `None` for the third to half of
    /// nodes that are pure transform groups.
    pub model_index: Option<usize>,
    /// Which partition cell the node sits in, as **grid indices**
    /// (docs/data/nodes.md).
    pub area_partition: Option<PartitionKey>,
    /// The node's one parent, or `None` for a root. Out-of-range and extra
    /// parents are dropped at load with a warning (docs/data/nodes.md).
    pub parent: Option<u32>,
    /// The node's children in the original's order. **Incomplete by
    /// itself** — see [`NodeCommon::parent`] and docs/data/nodes.md; the
    /// spawner builds the hierarchy from parents and uses this only to order
    /// siblings.
    pub children: Vec<u32>,
    /// The node's own bounds, `(min, max)`.
    pub node_bbox: (Vec3, Vec3),
    /// The bounds of the node's model. Not trustworthy on `Lod` and `Empty`
    /// nodes (docs/data/quirks.md).
    pub model_bbox: (Vec3, Vec3),
    /// The union of the children's bounds, in this node's own frame. Unused
    /// at runtime; it is what pins down the `Matrix` reading (docs/data/nodes.md).
    pub child_bbox: (Vec3, Vec3),
}

/// Which partition cell a node belongs to: indices into
/// [`WorldDesc::partitions`], not world coordinates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct PartitionKey {
    pub x: i32,
    pub z: i32,
}

/// Per-node flags.
///
/// A named struct rather than a raw `u32`, because the JSON is an
/// object of booleans (docs/data/nodes.md). `altitude_surface` and
/// `intersect_surface` are the **collision authority** (docs/data/nodes.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub struct NodeFlagsDesc {
    pub active: bool,
    /// The vehicle rides on this surface.
    pub altitude_surface: bool,
    /// The vehicle collides with this surface.
    pub intersect_surface: bool,
    pub intersect_bbox: bool,
    pub proximity: bool,
    /// A navigation landmark (m1: 17 nodes).
    pub landmark: bool,
    pub bbox_node: bool,
    pub bbox_model: bool,
    pub bbox_child: bool,
    pub terrain: bool,
    pub can_modify: bool,
    pub clip_to: bool,
    pub tree_valid: bool,
    pub id_zone_check: bool,
}

impl NodeFlagsDesc {
    /// Whether this node's geometry takes part in collision at all.
    ///
    /// Either flag is enough: `altitude_surface` is what the vehicle rides on
    /// and `intersect_surface` what it bumps into, and a level's terrain sets
    /// both (docs/data/nodes.md and docs/data/water.md).
    pub fn collides(&self) -> bool {
        self.altitude_surface || self.intersect_surface
    }
}

/// An `Object3d` node: a transform, and usually a model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Object3dDesc {
    pub common: NodeCommon,
    pub transform: TransformDesc,
    /// mech3ax's `signs`. Meaning unknown; kept, unused.
    pub signs: u32,
}

/// A node's transform, in the three forms the data actually uses
/// (docs/data/nodes.md).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub enum TransformDesc {
    /// Identity: 2732 of m1's 4041 `Object3d` nodes.
    #[default]
    Initial,
    /// A 3×4 affine: rotation **and** translation together, which is why
    /// there is no separate translation-only form.
    Matrix { rotation: Mat3, translation: Vec3 },
    /// Euler radians, translation and scale. Two nodes per level.
    RotateTranslateScale {
        rotate: Vec3,
        translate: Vec3,
        scale: Vec3,
    },
}

/// A `Lod` node: a distance band over its children.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LodDesc {
    pub common: NodeCommon,
    /// mech3ax's `field00`. Splits 604/641 across the 13 levels; meaning
    /// still unknown (docs/data/nodes.md).
    pub level: bool,
    /// Near and far distance. A far value of ~1e7 means "no far limit".
    pub range: (f32, f32),
}

/// The single `Light` node: the level's **sun**, not a point light.
///
/// `color` is (0,0,0) in all 13 levels, so a renderer substitutes white; what
/// actually varies is the `diffuse`/`ambient` split and `orientation`
/// (docs/data/nodes.md).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LightDesc {
    pub common: NodeCommon,
    /// Euler angles in radians, to be put through
    /// [`crate::axes::AxisConvention::rotation`].
    pub orientation: Vec3,
    /// Directional share, 0–1. Sums with `ambient` to exactly 1.0.
    pub diffuse: f32,
    /// Ambient share, 0–1.
    pub ambient: f32,
    /// Linear RGB. Black in every level.
    pub color: [f32; 3],
    pub range: (f32, f32),
}

impl LightDesc {
    /// The sun's colour, with white substituted for the black every level
    /// ships, so a level is lit rather than pitch dark.
    pub fn effective_color(&self) -> [f32; 3] {
        if self.color == [0.0, 0.0, 0.0] {
            [1.0, 1.0, 1.0]
        } else {
            self.color
        }
    }
}

/// An `Empty` node: a named leaf with nothing on it. Only m6 has any (287).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmptyDesc {
    pub common: NodeCommon,
}

/// The `World` node's node-level data; the environment is [`WorldDesc`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorldNodeDesc {
    pub common: NodeCommon,
}

/// The original renderer's camera. Kept for reference; the engine spawns its
/// own.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CameraDesc {
    pub common: NodeCommon,
    pub clip: (f32, f32),
    /// Horizontal and vertical field of view in radians (m1: 60° × 45°).
    pub fov: (f32, f32),
    pub lod_multiplier: f32,
}

/// The original renderer's back buffer. Kept for reference.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DisplayDesc {
    pub common: NodeCommon,
    pub resolution: (u32, u32),
    /// Linear RGB.
    pub clear_color: [f32; 3],
}

/// The original renderer's viewport. Kept for reference.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WindowDesc {
    pub common: NodeCommon,
    pub resolution: (u32, u32),
}

/// The world environment, extracted from the single `World` node.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorldDesc {
    /// `(left, top, right, bottom)` in x/z. Negative in m6, m9 and m13.
    pub area: (i32, i32, i32, i32),
    /// Linear RGB.
    pub fog_color: [f32; 3],
    /// `(near, far)`: fog starts at `near` and is full at `far`.
    pub fog_range: (f32, f32),
    /// `(clear, full)` altitude — **`clear > full`** in every level, which is
    /// why this is not named min/max (docs/data/nodes.md).
    pub fog_altitude: (f32, f32),
    /// The partition grid, row-major in z then x.
    pub partitions: Vec<PartitionDesc>,
    /// How many x columns each z row has, so [`WorldDesc::partition`] can
    /// index the flattened grid.
    pub partition_columns: usize,
    /// The World node's own children, in the original's order.
    pub children: Vec<u32>,
}

impl WorldDesc {
    /// One cell by its grid indices, or `None` when out of range.
    pub fn partition(&self, key: PartitionKey) -> Option<&PartitionDesc> {
        let (x, z) = (usize::try_from(key.x).ok()?, usize::try_from(key.z).ok()?);

        if x >= self.partition_columns {
            return None;
        }

        self.partitions.get(z * self.partition_columns + x)
    }
}

/// One partition cell: a 256×256 column of the world with a height band.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PartitionDesc {
    /// The cell's world x corner, a multiple of 256.
    pub x: i32,
    /// The cell's world z corner.
    pub z: i32,
    /// The bottom of the cell's height band. `y`, not `z`: the world is y-up
    /// (docs/data/coordinates.md), hence `y_min`/`y_max` rather than `z_min`/`z_max`.
    pub y_min: f32,
    pub y_max: f32,
    /// The nodes in this cell.
    pub nodes: Vec<u32>,
}

/// What can go wrong reading a `gamez` ZIP.
///
/// Every variant names the document inside the archive, because "the level
/// failed to load" without it sends the reader to the wrong file (NFR-9).
#[derive(Debug, thiserror::Error)]
pub enum LevelDataError {
    #[error("cannot read the gamez archive: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a readable ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("{file} is missing from the gamez archive")]
    Missing { file: &'static str },
    #[error("{file} does not parse: {source}")]
    Parse {
        file: &'static str,
        #[source]
        source: serde_json::Error,
    },
    /// A material names a texture index the level's `textures.json` has no
    /// entry for. Never happens in the retail data.
    #[error("materials.json: {0}")]
    Material(#[from] crate::materials::MaterialError),
    /// The `World` node is what [`WorldDesc`] comes from, and the spawner
    /// needs exactly one.
    #[error("expected exactly one World node, found {found}")]
    WorldNodes { found: usize },
}

/// Loads `cache://gamez/<level>-gamez.gamez.zip`.
#[derive(TypePath, Debug, Default)]
pub struct LevelDataLoader;

impl AssetLoader for LevelDataLoader {
    type Asset = LevelData;
    type Settings = ();
    type Error = LevelDataError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        context: &mut LoadContext<'_>,
    ) -> Result<LevelData, LevelDataError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        let label = context.path().to_string();

        parse_gamez(&bytes, &label)
    }

    fn extensions(&self) -> &[&str] {
        // Bevy matches the longest known extension, so `gamez.zip` wins over
        // a bare `zip` — which is what keeps this loader off `.tex.zip`.
        &["gamez.zip"]
    }
}

/// Read the five documents out of a `gamez` ZIP and convert them.
///
/// `label` names the archive in log lines; it is not used for anything else,
/// so tests can pass any string.
pub fn parse_gamez(bytes: &[u8], label: &str) -> Result<LevelData, LevelDataError> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;

    // `metadata.json` is read and discarded: its four array sizes describe the
    // original's fixed allocations, and `node_last_free` duplicates the node
    // count. Parsing it anyway means a corrupt archive is caught here rather
    // than halfway through the nodes.
    let _metadata: mech3ax::GameZMetadata = read_json(&mut zip, "metadata.json")?;
    let textures: Vec<mech3ax::TextureRef> = read_json(&mut zip, "textures.json")?;
    let materials: Vec<mech3ax::Material> = read_json(&mut zip, "materials.json")?;
    let models: Vec<mech3ax::Model> = read_json(&mut zip, "models.json")?;
    let nodes: Vec<mech3ax::Node> = read_json(&mut zip, "nodes.json")?;

    let node_descs = node_descs(&nodes, models.len(), label);

    let (world_node, world) = world_desc(&nodes, &node_descs, label)?;
    let sun = sun_desc(&node_descs, label);

    let root_nodes = node_descs
        .iter()
        .enumerate()
        .filter(|(_, node)| node.common().parent.is_none())
        .map(|(id, _)| id as u32)
        .collect::<Vec<u32>>();

    let level = LevelData {
        texture_names: textures
            .iter()
            .map(|texture| texture_key(&texture.name))
            .collect(),
        materials: material_descs(&materials, &textures)?,
        models: model_descs(&models),
        nodes: node_descs,
        world,
        world_node,
        root_nodes,
        sun,
    };

    log_census(&level, label);

    Ok(level)
}

/// Read and parse one document, naming it in either error.
fn read_json<T: serde::de::DeserializeOwned>(
    zip: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    file: &'static str,
) -> Result<T, LevelDataError> {
    let mut json = String::new();

    zip.by_name(file)
        .map_err(|_| LevelDataError::Missing { file })?
        .read_to_string(&mut json)?;

    // `serde_json`'s error carries the line, column and field path, which is
    // what the requirement "names the file inside the ZIP and the serde path"
    // asks for; the file name is this function's contribution.
    serde_json::from_str(&json).map_err(|source| LevelDataError::Parse { file, source })
}

/// Convert every node, resolving and validating the cross-references.
fn node_descs(nodes: &[mech3ax::Node], model_count: usize, label: &str) -> Vec<NodeDesc> {
    let count = nodes.len();
    let mut bad_parents = 0usize;
    let mut extra_parents = 0usize;
    let mut bad_children = 0usize;
    let mut bad_models = 0usize;

    let descs = nodes
        .iter()
        .enumerate()
        .map(|(id, node)| {
            // One parent, the first in range. m6 has one out-of-range parent
            // index and m9 one node with eight parents; both are isolated
            // data defects that must not cost the level (docs/data/nodes.md).
            let mut parents = node
                .parent_indices
                .iter()
                .copied()
                .filter(|index| in_range(*index, count));
            let parent = parents.next().map(|index| index as u32);

            bad_parents += node
                .parent_indices
                .iter()
                .filter(|index| !in_range(**index, count))
                .count();
            extra_parents += parents.count();

            let children: Vec<u32> = node
                .child_indices
                .iter()
                .copied()
                .filter(|index| {
                    let ok = in_range(*index, count);
                    if !ok {
                        bad_children += 1;
                    }
                    ok
                })
                .map(|index| index as u32)
                .collect();

            let model_index = match node.model_index {
                // `-1` is "no model" and is not an error.
                index if index < 0 => None,
                index if (index as usize) < model_count => Some(index as usize),
                _ => {
                    bad_models += 1;
                    None
                }
            };

            let common = NodeCommon {
                name: node.name.clone(),
                flags: node_flags(&node.flags),
                zone_id: (node.zone_id != -1).then_some(node.zone_id),
                model_index,
                area_partition: node
                    .area_partition
                    .map(|ap| PartitionKey { x: ap.x, z: ap.z }),
                parent,
                children,
                node_bbox: bbox(&node.node_bbox),
                model_bbox: bbox(&node.model_bbox),
                child_bbox: bbox(&node.child_bbox),
            };

            let _ = id;

            node_desc(common, &node.data)
        })
        .collect();

    if bad_parents > 0 {
        warn!(
            "{label}: {bad_parents} parent index(es) point outside the {count}-node array; \
             those nodes become roots"
        );
    }
    if extra_parents > 0 {
        warn!(
            "{label}: {extra_parents} extra parent(s) dropped — a node can have only one \
             parent in an entity hierarchy"
        );
    }
    if bad_children > 0 {
        warn!("{label}: {bad_children} child index(es) point outside the node array");
    }
    if bad_models > 0 {
        warn!("{label}: {bad_models} model index(es) point past the {model_count}-model table");
    }

    descs
}

fn in_range(index: i32, count: usize) -> bool {
    index >= 0 && (index as usize) < count
}

/// Build one node's variant from its `data`.
fn node_desc(common: NodeCommon, data: &mech3ax::NodeData) -> NodeDesc {
    match data {
        mech3ax::NodeData::World(_) => NodeDesc::World(WorldNodeDesc { common }),
        mech3ax::NodeData::Object3d(object) => NodeDesc::Object3d(Object3dDesc {
            common,
            transform: transform_desc(&object.transform),
            signs: object.signs,
        }),
        mech3ax::NodeData::Lod(lod) => NodeDesc::Lod(LodDesc {
            common,
            level: lod.field00 != 0,
            range: (lod.range.min, lod.range.max),
        }),
        mech3ax::NodeData::Light(light) => NodeDesc::Light(LightDesc {
            common,
            orientation: vec3(light.orientation),
            diffuse: light.diffuse,
            ambient: light.ambient,
            // The `Light` node's colour is 0-1 already, unlike the 0-255
            // colours elsewhere in mech3ax — and is black in every level.
            color: [light.color.r, light.color.g, light.color.b],
            range: (light.range.min, light.range.max),
        }),
        mech3ax::NodeData::Empty => NodeDesc::Empty(EmptyDesc { common }),
        mech3ax::NodeData::Camera(camera) => NodeDesc::Camera(CameraDesc {
            common,
            clip: (camera.clip_near, camera.clip_far),
            fov: (camera.fov_h_base, camera.fov_v_base),
            lod_multiplier: camera.lod_multiplier,
        }),
        mech3ax::NodeData::Display(display) => NodeDesc::Display(DisplayDesc {
            common,
            resolution: (display.resolution_x, display.resolution_y),
            clear_color: [
                display.clear_color.r,
                display.clear_color.g,
                display.clear_color.b,
            ],
        }),
        mech3ax::NodeData::Window(window) => NodeDesc::Window(WindowDesc {
            common,
            resolution: (window.resolution_x, window.resolution_y),
        }),
    }
}

fn transform_desc(transform: &mech3ax::Transform3d) -> TransformDesc {
    match transform {
        mech3ax::Transform3d::Initial => TransformDesc::Initial,
        mech3ax::Transform3d::Matrix(m) => TransformDesc::Matrix {
            // Direct3D's row-vector convention, `v' = v * M + t`: row `i`
            // (`ri0`–`ri2`) is where local axis `i` lands, just as row 3 is
            // the translation. Bevy applies `M * v`, so each stored row is one
            // of its columns. mech3ax builds the matrix this way ("and then
            // transposed", with scale applied per row), and every parent's
            // `child_bbox` in all 13 levels matches its children only under
            // this reading (docs/data/nodes.md).
            rotation: Mat3::from_cols(
                Vec3::new(m.r00, m.r01, m.r02),
                Vec3::new(m.r10, m.r11, m.r12),
                Vec3::new(m.r20, m.r21, m.r22),
            ),
            translation: Vec3::new(m.r30, m.r31, m.r32),
        },
        mech3ax::Transform3d::RotateTranslateScale(rts) => TransformDesc::RotateTranslateScale {
            rotate: vec3(rts.rotate),
            translate: vec3(rts.translate),
            scale: vec3(rts.scale),
        },
    }
}

fn node_flags(flags: &mech3ax::NodeFlags) -> NodeFlagsDesc {
    NodeFlagsDesc {
        active: flags.active,
        altitude_surface: flags.altitude_surface,
        intersect_surface: flags.intersect_surface,
        intersect_bbox: flags.intersect_bbox,
        proximity: flags.proximity,
        landmark: flags.landmark,
        bbox_node: flags.bbox_node,
        bbox_model: flags.bbox_model,
        bbox_child: flags.bbox_child,
        terrain: flags.terrain,
        can_modify: flags.can_modify,
        clip_to: flags.clip_to,
        tree_valid: flags.tree_valid,
        id_zone_check: flags.id_zone_check,
    }
}

/// Find the one `World` node and build [`WorldDesc`] from it.
fn world_desc(
    nodes: &[mech3ax::Node],
    descs: &[NodeDesc],
    label: &str,
) -> Result<(u32, WorldDesc), LevelDataError> {
    let worlds: Vec<usize> = nodes
        .iter()
        .enumerate()
        .filter(|(_, node)| matches!(node.data, mech3ax::NodeData::World(_)))
        .map(|(id, _)| id)
        .collect();

    let [id] = worlds[..] else {
        return Err(LevelDataError::WorldNodes {
            found: worlds.len(),
        });
    };

    let mech3ax::NodeData::World(world) = &nodes[id].data else {
        unreachable!("filtered on the variant just above");
    };

    let columns = world
        .partitions
        .iter()
        .map(|row| row.len())
        .max()
        .unwrap_or(0);

    if world.partitions.iter().any(|row| row.len() != columns) {
        warn!(
            "{label}: the partition grid is ragged ({} rows, widest {columns}); \
             short rows are padded with empty cells",
            world.partitions.len()
        );
    }

    let node_count = nodes.len();
    let mut dropped = 0usize;
    let mut partitions = Vec::with_capacity(world.partitions.len() * columns);

    for row in &world.partitions {
        for column in 0..columns {
            partitions.push(match row.get(column) {
                Some(cell) => PartitionDesc {
                    x: cell.x,
                    z: cell.z,
                    y_min: cell.min.y,
                    y_max: cell.max.y,
                    nodes: cell
                        .node_indices
                        .iter()
                        .copied()
                        .filter(|index| {
                            let ok = in_range(*index, node_count);
                            if !ok {
                                dropped += 1;
                            }
                            ok
                        })
                        .map(|index| index as u32)
                        .collect(),
                },
                None => PartitionDesc::default(),
            });
        }
    }

    if dropped > 0 {
        warn!("{label}: {dropped} partition node index(es) point outside the node array");
    }

    Ok((
        id as u32,
        WorldDesc {
            area: (
                world.area.left,
                world.area.top,
                world.area.right,
                world.area.bottom,
            ),
            // The World's fog colour is 0-1 already, unlike material colours.
            fog_color: [
                world.fog.fog_color.r,
                world.fog.fog_color.g,
                world.fog.fog_color.b,
            ],
            fog_range: (world.fog.fog_range.min, world.fog.fog_range.max),
            fog_altitude: (world.fog.fog_altitude.min, world.fog.fog_altitude.max),
            partitions,
            partition_columns: columns,
            children: descs[id].common().children.clone(),
        },
    ))
}

/// The level's sun: the one `Light` node, if there is one.
fn sun_desc(descs: &[NodeDesc], label: &str) -> Option<LightDesc> {
    let mut lights = descs.iter().filter_map(|node| match node {
        NodeDesc::Light(light) => Some(light),
        _ => None,
    });

    let sun = lights.next()?;
    let extra = lights.count();

    if extra > 0 {
        // Every shipped level has exactly one, so this is worth saying.
        warn!(
            "{label}: {} Light nodes; using the first ({})",
            extra + 1,
            sun.common.name
        );
    }

    Some(sun.clone())
}

/// One `info!` per level with the numbers the recon and the tests care about.
fn log_census(level: &LevelData, label: &str) {
    let census = level.census();
    let with_model = level
        .nodes
        .iter()
        .filter(|node| node.common().model_index.is_some())
        .count();
    let collides = level
        .nodes
        .iter()
        .filter(|node| node.common().model_index.is_some() && node.common().flags.collides())
        .count();
    let partitioned = level
        .nodes
        .iter()
        .filter(|node| node.common().area_partition.is_some())
        .count();
    let cells = level.world.partitions.len();
    let in_cells: usize = level
        .world
        .partitions
        .iter()
        .map(|cell| cell.nodes.len())
        .sum();

    info!(
        "{label}: {} nodes ({} Object3d, {} Lod, {} Empty, {} World/Camera/Display/Window/Light), \
         {} roots, {with_model} with a model of which {collides} collide, \
         {partitioned} in a partition, {cells} cells holding {in_cells} node refs, \
         {} materials, {} models, {} texture names",
        level.nodes.len(),
        census.object3d,
        census.lod,
        census.empty,
        census.world + census.camera + census.display + census.window + census.light,
        level.root_nodes.len(),
        level.materials.len(),
        level.models.len(),
        level.texture_names.len(),
    );

    if let Some(sun) = &level.sun {
        info!(
            "{label}: sun `{}` diffuse {:.2} ambient {:.2}, orientation {:?}, fog {:?} at {:?}",
            sun.common.name,
            sun.diffuse,
            sun.ambient,
            sun.orientation,
            level.world.fog_color,
            level.world.fog_range,
        );
    } else {
        warn!("{label}: no Light node, so the level has no sun");
    }
}

/// The automap: a handful of coloured polylines over the level's x/z extent.
#[derive(Asset, TypePath, Debug, Clone, Default)]
pub struct ZMap {
    /// The map's extent. `y` is zero in both in every mission: the automap is
    /// an x/z projection (docs/data/coordinates.md).
    pub min: Vec3,
    pub max: Vec3,
    pub features: Vec<ZMapFeature>,
}

/// One automap polyline.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ZMapFeature {
    /// sRGB 0–255, as the file stores it.
    pub color: [u8; 3],
    pub vertices: Vec<Vec3>,
    /// The objective this feature marks, or `None`.
    pub objective: Option<i32>,
}

/// What can go wrong reading a `zmap` JSON.
#[derive(Debug, thiserror::Error)]
pub enum ZMapError {
    #[error("cannot read the zmap: {0}")]
    Io(#[from] std::io::Error),
    #[error("the zmap does not parse: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Loads `cache://zmap/<level>.zmap.json`.
#[derive(TypePath, Debug, Default)]
pub struct ZMapLoader;

impl AssetLoader for ZMapLoader {
    type Asset = ZMap;
    type Settings = ();
    type Error = ZMapError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _context: &mut LoadContext<'_>,
    ) -> Result<ZMap, ZMapError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        parse_zmap(&bytes)
    }

    fn extensions(&self) -> &[&str] {
        &["zmap.json"]
    }
}

/// Convert a `zmap` JSON document.
pub fn parse_zmap(bytes: &[u8]) -> Result<ZMap, ZMapError> {
    let file: mech3ax::ZMapFile = serde_json::from_slice(bytes)?;

    Ok(ZMap {
        min: vec3(file.min),
        max: vec3(file.max),
        features: file
            .features
            .iter()
            .map(|feature| ZMapFeature {
                color: [feature.color.r, feature.color.g, feature.color.b],
                vertices: feature.vertices.iter().copied().map(vec3).collect(),
                objective: (feature.objective >= 0).then_some(feature.objective),
            })
            .collect(),
    })
}

fn vec3(v: mech3ax::Vec3Json) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

fn bbox(b: &mech3ax::BoundingBox) -> (Vec3, Vec3) {
    (vec3(b.a), vec3(b.b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::minimal_gamez;

    #[test]
    fn the_minimal_archive_loads_into_a_level() {
        let bytes = minimal_gamez();

        let level = parse_gamez(&bytes, "test.gamez.zip").expect("loads");

        assert_eq!(level.nodes.len(), 3, "a world, a group and an object");
        assert_eq!(level.models.len(), 1);
        assert_eq!(level.materials.len(), 1);
        // The extension is stripped, so the name matches an archive stem.
        assert_eq!(level.texture_names, vec!["wall".to_string()]);

        let census = level.census();
        assert_eq!(census.world, 1);
        assert_eq!(census.object3d, 2);

        assert_eq!(level.world_node, 0);
        assert_eq!(level.root_nodes, vec![0], "only the world has no parent");
    }

    #[test]
    fn the_world_node_becomes_the_world_environment() {
        let level = parse_gamez(&minimal_gamez(), "test").expect("loads");

        assert_eq!(level.world.area, (0, 0, 512, 512));
        assert_eq!(level.world.fog_range, (100.0, 200.0));
        // Deliberately inverted in the fixture, as the real data is.
        assert_eq!(level.world.fog_altitude, (48.0, 24.0));
        assert_eq!(level.world.fog_color, [0.5, 0.6, 0.7]);
        assert_eq!(level.world.children, vec![1]);
    }

    #[test]
    fn the_light_node_becomes_the_sun_with_white_substituted_for_black() {
        let bytes = crate::test_support::gamez_from(
            r#"[{"Textured":{"texture_index":0}}]"#,
            r#"[{"name":"wall.tif"}]"#,
            r#"[]"#,
            &format!(
                "[{},{}]",
                crate::test_support::WORLD_NODE,
                r#"{ "name": "sunlight", "parent_indices": [0], "data": { "Light": {
                     "orientation": { "x": -0.5, "y": -2.0, "z": 0.0 },
                     "diffuse": 0.55, "ambient": 0.45,
                     "color": { "r": 0.0, "g": 0.0, "b": 0.0 },
                     "range": { "min": 480.0, "max": 540.0 } } } }"#
            ),
        );

        let level = parse_gamez(&bytes, "test").expect("loads");
        let sun = level.sun.expect("a sun");

        assert_eq!(sun.diffuse, 0.55);
        assert_eq!(sun.ambient, 0.45);
        assert_eq!(sun.orientation, Vec3::new(-0.5, -2.0, 0.0));
        assert_eq!(sun.color, [0.0, 0.0, 0.0], "as the data has it");
        assert_eq!(
            sun.effective_color(),
            [1.0, 1.0, 1.0],
            "but a black sun is substituted with white"
        );
    }

    #[test]
    fn a_matrix_transform_becomes_a_rotation_and_a_translation() {
        // The JSON is Direct3D row-vector: row 0 is where +x lands. So
        // `r02 = -1` sends +x to -z and row 2 sends +z to +x: a +90° turn
        // about y, m1's `dockcrane` exactly.
        let nodes = format!(
            "[{},{}]",
            crate::test_support::WORLD_NODE,
            r#"{ "name": "turned", "parent_indices": [0], "data": { "Object3d": {
                 "transform": { "Matrix": {
                   "r00": 0.0, "r01": 0.0, "r02": -1.0,
                   "r10": 0.0, "r11": 1.0, "r12": 0.0,
                   "r20": 1.0, "r21": 0.0, "r22": 0.0,
                   "r30": 10.0, "r31": 2.0, "r32": 30.0 } },
                 "signs": 128 } } }"#
        );
        let bytes = crate::test_support::gamez_from(
            r#"[{"Textured":{"texture_index":0}}]"#,
            r#"[{"name":"wall.tif"}]"#,
            "[]",
            &nodes,
        );

        let level = parse_gamez(&bytes, "test").expect("loads");
        let NodeDesc::Object3d(object) = &level.nodes[1] else {
            panic!("an Object3d");
        };
        let TransformDesc::Matrix {
            rotation,
            translation,
        } = object.transform
        else {
            panic!("a Matrix transform, got {:?}", object.transform);
        };

        assert_eq!(translation, Vec3::new(10.0, 2.0, 30.0));
        // Row 0 becomes column 0: +x goes to (r00, r01, r02).
        let turned = rotation * Vec3::X;
        assert!((turned - Vec3::NEG_Z).length() < 1e-6, "{turned:?}");
        // And the other way round, so a transposed reading would fail here.
        let back = rotation * Vec3::Z;
        assert!((back - Vec3::X).length() < 1e-6, "{back:?}");
        assert!(rotation.abs_diff_eq(Mat3::from_rotation_y(std::f32::consts::FRAC_PI_2), 1e-6));
        assert_eq!(object.signs, 128);
    }

    #[test]
    fn a_node_whose_parent_is_out_of_range_becomes_a_root_rather_than_a_failure() {
        // m6 ships exactly this: node 4787 names parent 4955 in a 4955-node
        // array (docs/data/nodes.md).
        let nodes = format!(
            "[{},{}]",
            crate::test_support::WORLD_NODE,
            r#"{ "name": "db", "parent_indices": [99], "data": "Empty" }"#
        );
        let bytes = crate::test_support::gamez_from(
            r#"[{"Textured":{"texture_index":0}}]"#,
            r#"[{"name":"wall.tif"}]"#,
            "[]",
            &nodes,
        );

        let level = parse_gamez(&bytes, "test").expect("loads despite the bad index");

        assert_eq!(level.nodes[1].common().parent, None);
        assert_eq!(level.root_nodes, vec![0, 1]);
        assert!(matches!(level.nodes[1], NodeDesc::Empty(_)));
    }

    #[test]
    fn a_node_with_several_parents_keeps_the_first() {
        // m9's node 37 hangs off all eight race gates.
        let nodes = format!(
            "[{},{},{}]",
            crate::test_support::WORLD_NODE,
            r#"{ "name": "gate2", "parent_indices": [0], "data": "Empty" }"#,
            r#"{ "name": "shared", "parent_indices": [1, 0], "data": "Empty" }"#
        );
        let bytes = crate::test_support::gamez_from(
            r#"[{"Textured":{"texture_index":0}}]"#,
            r#"[{"name":"wall.tif"}]"#,
            "[]",
            &nodes,
        );

        let level = parse_gamez(&bytes, "test").expect("loads");

        assert_eq!(level.nodes[2].common().parent, Some(1));
    }

    #[test]
    fn a_model_index_past_the_table_is_dropped_not_trusted() {
        let nodes = format!(
            "[{},{}]",
            crate::test_support::WORLD_NODE,
            r#"{ "name": "ghost", "model_index": 42, "parent_indices": [0],
                 "data": { "Object3d": { "transform": "Initial" } } }"#
        );
        let bytes = crate::test_support::gamez_from(
            r#"[{"Textured":{"texture_index":0}}]"#,
            r#"[{"name":"wall.tif"}]"#,
            "[]",
            &nodes,
        );

        let level = parse_gamez(&bytes, "test").expect("loads");

        assert_eq!(level.nodes[1].common().model_index, None);
    }

    #[test]
    fn a_level_with_no_world_node_is_an_error_that_says_so() {
        let bytes = crate::test_support::gamez_from(
            "[]",
            "[]",
            "[]",
            r#"[{ "name": "lonely", "data": "Empty" }]"#,
        );

        let error = parse_gamez(&bytes, "test").expect_err("no world");

        assert!(
            matches!(error, LevelDataError::WorldNodes { found: 0 }),
            "{error}"
        );
    }

    #[test]
    fn two_world_nodes_are_an_error_too() {
        let nodes = format!(
            "[{},{}]",
            crate::test_support::WORLD_NODE,
            crate::test_support::WORLD_NODE
        );
        let bytes = crate::test_support::gamez_from("[]", "[]", "[]", &nodes);

        let error = parse_gamez(&bytes, "test").expect_err("two worlds");

        assert!(
            matches!(error, LevelDataError::WorldNodes { found: 2 }),
            "{error}"
        );
    }

    #[test]
    fn a_missing_document_names_itself() {
        // Everything but `nodes.json`.
        let mut buffer = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            for (name, body) in [
                ("metadata.json", "{}"),
                ("textures.json", "[]"),
                ("materials.json", "[]"),
                ("models.json", "[]"),
            ] {
                zip.start_file(name, options).unwrap();
                std::io::Write::write_all(&mut zip, body.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }

        let error = parse_gamez(&buffer, "test").expect_err("no nodes.json");

        assert!(
            matches!(error, LevelDataError::Missing { file: "nodes.json" }),
            "{error}"
        );
    }

    #[test]
    fn a_parse_error_names_the_document_and_keeps_the_serde_path() {
        let bytes = crate::test_support::gamez_from("[]", "[]", "[]", r#"{"not":"a list"}"#);

        let error = parse_gamez(&bytes, "test").expect_err("nodes.json is an object");

        let LevelDataError::Parse { file, source } = &error else {
            panic!("a parse error, got {error}");
        };
        assert_eq!(*file, "nodes.json");
        // serde_json's message carries the position, which is the "serde path"
        // half of the requirement.
        assert!(source.line() > 0 || source.column() > 0, "{source}");
        assert!(format!("{error}").contains("nodes.json"), "{error}");
    }

    #[test]
    fn a_partition_cell_is_found_by_its_grid_indices() {
        let world = WorldDesc {
            partition_columns: 3,
            partitions: (0..6)
                .map(|i| PartitionDesc {
                    x: (i % 3) * 256,
                    z: (i / 3) * 256,
                    nodes: vec![i as u32],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };

        // Row-major in z then x: {x:2, z:1} is the sixth cell.
        let cell = world
            .partition(PartitionKey { x: 2, z: 1 })
            .expect("in range");
        assert_eq!((cell.x, cell.z), (512, 256));
        assert_eq!(cell.nodes, vec![5]);

        // Out of range in either axis, and negative, are all `None` rather
        // than a wrapped index into the wrong row.
        assert!(world.partition(PartitionKey { x: 3, z: 0 }).is_none());
        assert!(world.partition(PartitionKey { x: 0, z: 2 }).is_none());
        assert!(world.partition(PartitionKey { x: -1, z: 0 }).is_none());
    }

    #[test]
    fn collision_follows_the_two_surface_flags_and_nothing_else() {
        let mut flags = NodeFlagsDesc {
            active: true,
            ..Default::default()
        };
        assert!(!flags.collides(), "active alone is not collision");

        flags.altitude_surface = true;
        assert!(flags.collides(), "what the vehicle rides on");

        flags.altitude_surface = false;
        flags.intersect_surface = true;
        assert!(flags.collides(), "what the vehicle bumps into");
    }

    #[test]
    fn a_zmap_parses_into_features() {
        let json = br#"{
          "unk04": 31,
          "min": { "x": 0.0, "y": 0.0, "z": 0.0 },
          "max": { "x": 2768.0, "y": 0.0, "z": 3423.0 },
          "features": [
            { "color": { "r": 255, "g": 0, "b": 0 },
              "vertices": [ { "x": 1.0, "y": 2.0, "z": 3.0 }, { "x": 4.0, "y": 5.0, "z": 6.0 } ],
              "objective": 2 },
            { "color": { "r": 0, "g": 0, "b": 255 }, "vertices": [], "objective": -1 }
          ]
        }"#;

        let zmap = parse_zmap(json).expect("parses");

        assert_eq!(zmap.max, Vec3::new(2768.0, 0.0, 3423.0));
        assert_eq!(zmap.features.len(), 2);
        assert_eq!(zmap.features[0].color, [255, 0, 0]);
        assert_eq!(zmap.features[0].vertices.len(), 2);
        assert_eq!(zmap.features[0].objective, Some(2));
        // -1 is "no objective", not objective number -1.
        assert_eq!(zmap.features[1].objective, None);
    }

    #[test]
    fn a_malformed_zmap_is_an_error_rather_than_an_empty_map() {
        assert!(parse_zmap(b"not json").is_err());
    }
}
