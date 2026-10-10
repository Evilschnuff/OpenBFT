//! Hand-written mirrors of the mech3ax JSON shapes this crate reads.
//!
//! DESIGN §3.1's mirroring rule: these structs are written from mech3ax's
//! `api-types` but are **not** that crate. They use `#[serde(default)]`
//! liberally, never `deny_unknown_fields`, and omit every field the engine
//! ignores (`ptr`, `unk*`, the `*_loaded` booleans). When mech3ax's output
//! changes, this module and its tests change and nothing else does.
//!
//! Verified against the pinned build (mech3ax `cbb838f`, docs/data/install.md) on
//! the retail Recoil data.

use serde::Deserialize;

/// `manifest.json` inside a `.tex.zip` (`TextureManifest`).
///
/// `global_palettes` is deliberately absent: every Recoil `rtexture*` and
/// `image` entry has `"palette": "None"` (docs/data/materials.md), so the PNGs are
/// already de-palettised and the palette table is dead weight.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TextureManifest {
    /// One entry per texture in the archive, in archive order.
    #[serde(default)]
    pub texture_infos: Vec<TextureInfo>,
}

/// One texture's metadata (`TextureInfo`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TextureInfo {
    /// The archive's own name for the texture, always a bare lowercase stem
    /// with no extension.
    pub name: String,
    /// mech3ax emits this only when it differs from `name`; Recoil never
    /// does, but the field is cheap to accept.
    #[serde(default)]
    pub rename: Option<String>,
    #[serde(default)]
    pub alpha: TextureAlpha,
    #[serde(default)]
    pub width: u16,
    #[serde(default)]
    pub height: u16,
    #[serde(default)]
    pub stretch: TextureStretch,
}

/// How a texture's alpha channel is meant (`TextureAlpha`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub enum TextureAlpha {
    /// Fully opaque.
    #[default]
    None,
    /// One-bit cutout. Only `image.zbd` uses it (docs/data/materials.md).
    Simple,
    /// A real alpha channel.
    Full,
}

/// The original hardware's texture-stretch flag (`TextureStretch`).
///
/// It does **not** describe the emitted PNG: mech3ax writes every texture at
/// its declared `width`×`height` whatever this says (verified on all 576 m1
/// entries), so the engine records it and applies nothing. The
/// Crimson-Skies-only `Unk4`/`Unk7`/`Unk8` variants are folded into `Other`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub enum TextureStretch {
    #[default]
    None,
    Vertical,
    Horizontal,
    Both,
    #[serde(other)]
    Other,
}

/// What a surface is made of, for physics and impact effects (`Soil`).
///
/// Copied verbatim from mech3ax as DESIGN §3.1 requires, all 14 variants,
/// even though Recoil only ever uses five (docs/data/materials.md and docs/data/water.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub enum Soil {
    #[default]
    Default,
    Water,
    Seafloor,
    Quicksand,
    Lava,
    Fire,
    Dirt,
    Mud,
    Grass,
    Concrete,
    Snow,
    Mech,
    Silt,
    NoSlip,
}

/// `materials.json` entry (`Material`), an externally tagged sum type.
#[derive(Debug, Clone, Deserialize)]
pub enum Material {
    Textured(TexturedMaterial),
    Colored(ColoredMaterial),
}

/// A material sampling a texture (`TexturedMaterial`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TexturedMaterial {
    /// Index into the same `gamez`'s `textures.json`, not a name.
    #[serde(default)]
    pub texture_index: u32,
    #[serde(default)]
    pub soil: Soil,
    /// Baked texture cycling. Null in every Recoil level (docs/data/materials.md);
    /// cycling comes from interp instead (docs/data/interp.md). Kept because the format has
    /// it and another game's data would use it.
    #[serde(default)]
    pub cycle: Option<CycleData>,
    /// Meaning unknown. `false` on all 4649 textured materials across the
    /// 13 levels, so there is nothing to learn from this data set.
    #[serde(default)]
    pub flag: bool,
}

/// A flat-coloured material (`ColoredMaterial`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ColoredMaterial {
    /// Components are **0–255**, not 0–1, despite being floats.
    #[serde(default)]
    pub color: Color,
    #[serde(default)]
    pub alpha: u8,
    #[serde(default)]
    pub soil: Soil,
}

/// An RGB triple as mech3ax writes it (`Color`), components 0–255.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct Color {
    #[serde(default)]
    pub r: f32,
    #[serde(default)]
    pub g: f32,
    #[serde(default)]
    pub b: f32,
}

/// Baked texture cycling (`CycleData`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CycleData {
    /// Indices into `textures.json`, one per frame.
    #[serde(default)]
    pub texture_indices: Vec<u32>,
    #[serde(default)]
    pub looping: bool,
    /// Frames per second.
    #[serde(default)]
    pub speed: f32,
}

/// `textures.json` entry: the source name a level refers to a texture by.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TextureRef {
    /// Carries the source extension (`dockp01k.tif`) that archive stems lack.
    pub name: String,
}

/// A 3-vector as mech3ax writes it (`Vec3`), an object rather than an array.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct Vec3Json {
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub z: f32,
}

/// A texture coordinate as mech3ax writes it (`UvCoord`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct UvCoord {
    #[serde(default)]
    pub u: f32,
    #[serde(default)]
    pub v: f32,
}

/// What a model is drawn as (`ModelType`).
///
/// Recoil uses only `Default` (9947 models) and `Facade` (3089); `Points`
/// never occurs (docs/data/models.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub enum ModelType {
    #[default]
    Default,
    Facade,
    Points,
}

/// Which axis a facade billboard rotates about (`FacadeMode`).
///
/// Every facade in every Recoil level is `CylindricalY` (docs/data/models.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub enum FacadeMode {
    #[default]
    CylindricalY,
    SphericalY,
    CylindricalX,
    CylindricalZ,
}

/// Per-model rendering flags (`ModelFlags`).
///
/// **Emitted as an object of booleans, not a bitfield**, and mech3ax skips
/// serialising the ones that are false — `texture_registered`, `morph` and
/// `texture_scroll` appear in no Recoil model, so every field defaults
/// (docs/data/models.md). `clouds` and `facade_centroid` are emitted
/// but false on all 13 036 models.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct ModelFlags {
    #[serde(default)]
    pub lighting: bool,
    #[serde(default)]
    pub fog: bool,
    #[serde(default)]
    pub texture_registered: bool,
    #[serde(default)]
    pub morph: bool,
    #[serde(default)]
    pub texture_scroll: bool,
    #[serde(default)]
    pub clouds: bool,
    #[serde(default)]
    pub facade_centroid: bool,
}

/// Per-polygon flags (`PolygonFlags`), an object of booleans like
/// [`ModelFlags`].
///
/// `tri_strip` is false on all 103 158 polygons and `in_out`/`unk3` are never
/// emitted at all (docs/data/models.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct PolygonFlags {
    #[serde(default)]
    pub show_backface: bool,
    #[serde(default)]
    pub tri_strip: bool,
    #[serde(default)]
    pub in_out: bool,
}

/// One material reference on a polygon (`PolygonMaterial`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PolygonMaterial {
    /// Index into the level's `materials.json`.
    #[serde(default)]
    pub material_index: u32,
    /// One coordinate per polygon corner, or null.
    ///
    /// Null exactly when the material is [`Material::Colored`] — verified on
    /// all 2610 occurrences across the 13 levels.
    #[serde(default)]
    pub uv_coords: Option<Vec<UvCoord>>,
}

/// One polygon (`Polygon`). Recoil's are 3- to 14-gons, 73% quads.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Polygon {
    #[serde(default)]
    pub flags: PolygonFlags,
    /// Draw order for transparency (inferred). Non-zero on 795 polygons.
    #[serde(default)]
    pub priority: i32,
    /// Which zones the polygon belongs to. Usually empty.
    #[serde(default)]
    pub zone_set: Vec<i8>,
    /// Indices into the model's `vertices`.
    #[serde(default)]
    pub vertex_indices: Vec<u32>,
    /// Indices into the model's `normals`, which is a **separate table** and
    /// not parallel to `vertices` (docs/data/models.md).
    #[serde(default)]
    pub normal_indices: Option<Vec<u32>>,
    /// Never non-empty in any Recoil level (docs/data/models.md).
    #[serde(default)]
    pub vertex_colors: Vec<Color>,
    /// Always exactly one in Recoil; the format allows more.
    #[serde(default)]
    pub materials: Vec<PolygonMaterial>,
}

/// A light baked into a model (`PointLight`).
///
/// Most fields are still `unk*` in mech3ax and are omitted per the mirroring
/// rule. `vertices` holds the light's position(s) — one in 3224 of the 3272
/// lights, more in the rest.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PointLight {
    #[serde(default)]
    pub vertices: Vec<Vec3Json>,
    /// Components are **0–255**, as everywhere else in mech3ax.
    #[serde(default)]
    pub color: Color,
    #[serde(default)]
    pub flags: u16,
}

/// `models.json` entry (`Model`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Model {
    #[serde(default)]
    pub model_type: ModelType,
    #[serde(default)]
    pub facade_mode: FacadeMode,
    #[serde(default)]
    pub flags: ModelFlags,
    #[serde(default)]
    pub vertices: Vec<Vec3Json>,
    /// Indexed only by `Polygon::normal_indices`; see [`Polygon`].
    #[serde(default)]
    pub normals: Vec<Vec3Json>,
    /// Morph targets. 189 models have them; nothing consumes them yet.
    #[serde(default)]
    pub morphs: Vec<Vec3Json>,
    #[serde(default)]
    pub lights: Vec<PointLight>,
    #[serde(default)]
    pub polygons: Vec<Polygon>,
    /// Zero on every Recoil model; scroll comes from interp (docs/data/models.md).
    #[serde(default)]
    pub texture_scroll: UvCoord,
    #[serde(default)]
    pub bbox_mid: Vec3Json,
    #[serde(default)]
    pub bbox_diag: f32,
}

/// `metadata.json` inside a `.gamez.zip` (`GameZMetadata`).
///
/// The original's fixed array allocations. Read so a corrupt archive fails at
/// the first document rather than halfway through the nodes; nothing uses the
/// values, since `node_last_free` only duplicates the node count.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GameZMetadata {
    #[serde(default)]
    pub material_array_size: i32,
    #[serde(default)]
    pub model_array_size: i32,
    #[serde(default)]
    pub node_array_size: i32,
    #[serde(default)]
    pub node_last_free: i32,
}

/// A min/max pair as mech3ax writes it (`Range`).
///
/// **Not necessarily ordered**: the World node's `fog_altitude` has
/// `min > max` in all 13 levels (docs/data/nodes.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct Range {
    #[serde(default)]
    pub min: f32,
    #[serde(default)]
    pub max: f32,
}

/// An axis-aligned box as mech3ax writes it (`BoundingBox`), corners named
/// `a` and `b` rather than min and max.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct BoundingBox {
    #[serde(default)]
    pub a: Vec3Json,
    #[serde(default)]
    pub b: Vec3Json,
}

/// Which partition cell a node belongs to (`AreaPartition`).
///
/// **Grid indices, not world coordinates**: this indexes
/// [`WorldNode::partitions`] as `partitions[z][x]` — verified exactly on all
/// 468 of m1's, 588 of m6's and 441 of m9's (docs/data/nodes.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct AreaPartition {
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub z: i32,
}

/// `nodes.json` entry.
///
/// **The emitted shape is flat, with the variant under `data`** — not the
/// per-variant structs `api-types/src/nodes/rc.rs` declares
/// (docs/data/nodes.md, docs/data/quirks.md). `name`, `flags`, `zone_id`,
/// `model_index` and `area_partition` are read from the top level; only
/// variant-specific fields come from `data`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Node {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub flags: NodeFlags,
    /// `-1` means "no zone". A small signed integer otherwise (docs/data/nodes.md).
    #[serde(default = "minus_one")]
    pub zone_id: i32,
    /// `-1` means "no model"; a third to a half of all nodes (docs/data/nodes.md).
    #[serde(default = "minus_one")]
    pub model_index: i32,
    #[serde(default)]
    pub area_partition: Option<AreaPartition>,
    /// Indices into the node array. **The hierarchy authority**: it never
    /// contradicts `child_indices` and is never missing an edge, whereas
    /// `child_indices` omits 83–874 per level (docs/data/nodes.md).
    #[serde(default)]
    pub parent_indices: Vec<i32>,
    /// Indices into the node array, in the original's draw order.
    #[serde(default)]
    pub child_indices: Vec<i32>,
    #[serde(default)]
    pub node_bbox: BoundingBox,
    #[serde(default)]
    pub model_bbox: BoundingBox,
    #[serde(default)]
    pub child_bbox: BoundingBox,
    /// The variant and its payload.
    #[serde(default)]
    pub data: NodeData,
}

fn minus_one() -> i32 {
    -1
}

/// Per-node flags.
///
/// Unlike [`ModelFlags`], mech3ax emits **every** field including the false
/// ones, but they still default so a future addition cannot break the parse.
/// `altitude_surface` and `intersect_surface` are the collision authority
/// (docs/data/nodes.md): they mark, respectively, what the vehicle rides on and
/// what it collides with.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct NodeFlags {
    #[serde(default)]
    pub active: bool,
    /// The surfaces the vehicle rides on; interp's `SetAltitudeSurface`.
    #[serde(default)]
    pub altitude_surface: bool,
    /// The surfaces the vehicle collides with; interp's `SetIntersectSurface`.
    #[serde(default)]
    pub intersect_surface: bool,
    #[serde(default)]
    pub intersect_bbox: bool,
    #[serde(default)]
    pub proximity: bool,
    /// A navigation landmark; interp's `SetLandmark`. Rare (m1: 17).
    #[serde(default)]
    pub landmark: bool,
    #[serde(default)]
    pub bbox_node: bool,
    #[serde(default)]
    pub bbox_model: bool,
    #[serde(default)]
    pub bbox_child: bool,
    #[serde(default)]
    pub terrain: bool,
    #[serde(default)]
    pub can_modify: bool,
    #[serde(default)]
    pub clip_to: bool,
    #[serde(default)]
    pub tree_valid: bool,
    #[serde(default)]
    pub r#override: bool,
    #[serde(default)]
    pub id_zone_check: bool,
    #[serde(default)]
    pub unk25: bool,
    #[serde(default)]
    pub unk28: bool,
}

/// A node's variant and its payload, as it sits under `data`.
///
/// Externally tagged, except `Empty`, which mech3ax emits as the bare string
/// `"Empty"` — a unit variant, which serde reads from exactly that
/// (docs/data/nodes.md). Only m6 has any (287).
#[derive(Debug, Clone, Default, Deserialize)]
pub enum NodeData {
    World(WorldNode),
    Object3d(Object3dNode),
    Lod(LodNode),
    Light(LightNode),
    /// The default too: a node whose `data` will not parse is an empty one
    /// rather than a failed level.
    #[default]
    Empty,
    Camera(CameraNode),
    Display(DisplayNode),
    Window(WindowNode),
}

impl NodeData {
    /// The variant's name, for logs and censuses.
    pub fn kind(&self) -> &'static str {
        match self {
            NodeData::World(_) => "World",
            NodeData::Object3d(_) => "Object3d",
            NodeData::Lod(_) => "Lod",
            NodeData::Light(_) => "Light",
            NodeData::Empty => "Empty",
            NodeData::Camera(_) => "Camera",
            NodeData::Display(_) => "Display",
            NodeData::Window(_) => "Window",
        }
    }
}

/// The single `World` node: fog, extent, the partition grid and the sun.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorldNode {
    #[serde(default)]
    pub fog: WorldFog,
    #[serde(default)]
    pub area: Area,
    /// **`[z_row][x_col]`**, indexed by [`AreaPartition`] as
    /// `partitions[z][x]` (docs/data/nodes.md).
    #[serde(default)]
    pub partitions: Vec<Vec<PartitionCell>>,
    /// The `Light` node(s) lighting this world. Exactly one in every level.
    #[serde(default)]
    pub light_indices: Vec<i32>,
    /// Empty in every level.
    #[serde(default)]
    pub sound_indices: Vec<i32>,
}

/// The world's fog, as a nested object rather than flat fields.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WorldFog {
    /// `"Linear"` in all 13 levels.
    #[serde(default)]
    pub fog_type: FogType,
    /// Components are **0–1** here, unlike mech3ax's other colours.
    #[serde(default)]
    pub fog_color: Color,
    #[serde(default)]
    pub fog_range: Range,
    /// **`min > max`** in every level: full fog at `max`, clear at `min`
    /// (docs/data/nodes.md).
    #[serde(default)]
    pub fog_altitude: Range,
    /// `0.0` in all 13 levels; only `Exponential` fog would use it.
    #[serde(default)]
    pub fog_density: f32,
}

/// How fog falls off with distance (`FogType`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub enum FogType {
    Off,
    #[default]
    Linear,
    Exponential,
}

/// The world's extent in the x/z ground plane (`Area`).
///
/// `left`/`right` are x and `top`/`bottom` are z. **Not anchored at the
/// origin**: negative in m6, m9 and m13 (docs/data/nodes.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct Area {
    #[serde(default)]
    pub left: i32,
    #[serde(default)]
    pub top: i32,
    #[serde(default)]
    pub right: i32,
    #[serde(default)]
    pub bottom: i32,
}

/// One cell of the world's partition grid (`PartitionPg`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PartitionCell {
    /// The cell's world x corner, a multiple of 256.
    #[serde(default)]
    pub x: i32,
    /// The cell's world z corner, a multiple of 256.
    #[serde(default)]
    pub z: i32,
    /// The cell's corner, whose `y` carries its height band.
    #[serde(default)]
    pub min: Vec3Json,
    #[serde(default)]
    pub max: Vec3Json,
    /// The nodes in this cell. Their union is exactly the set of nodes the
    /// World's `child_indices` omits (docs/data/nodes.md).
    #[serde(default)]
    pub node_indices: Vec<i32>,
}

/// An `Object3d` node's own data: just the transform, plus unused fields.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Object3dNode {
    #[serde(default)]
    pub transform: Transform3d,
    /// Meaning unknown (m1's first `Object3d` has 128). Kept, unused.
    #[serde(default)]
    pub signs: u32,
}

/// A node's transform, in one of three forms (docs/data/nodes.md).
#[derive(Debug, Clone, Default, Deserialize)]
pub enum Transform3d {
    /// Identity. Emitted as the bare string `"Initial"`; 2732 of m1's 4041.
    #[default]
    Initial,
    /// A **3×4 affine**: row-major 3×3 rotation in `r00`–`r22` and the
    /// translation in `r30`–`r32`. 1307 of m1's 4041.
    Matrix(TransformMatrix),
    /// Euler rotation (radians), translation and scale. 2 per level.
    RotateTranslateScale(TransformRts),
}

/// The `Matrix` transform's twelve components.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
#[allow(missing_docs)]
pub struct TransformMatrix {
    #[serde(default)]
    pub r00: f32,
    #[serde(default)]
    pub r01: f32,
    #[serde(default)]
    pub r02: f32,
    #[serde(default)]
    pub r10: f32,
    #[serde(default)]
    pub r11: f32,
    #[serde(default)]
    pub r12: f32,
    #[serde(default)]
    pub r20: f32,
    #[serde(default)]
    pub r21: f32,
    #[serde(default)]
    pub r22: f32,
    /// The translation's x, despite the row-major naming.
    #[serde(default)]
    pub r30: f32,
    #[serde(default)]
    pub r31: f32,
    #[serde(default)]
    pub r32: f32,
}

/// The `RotateTranslateScale` transform.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct TransformRts {
    /// Euler angles in radians, order inferred XYZ (docs/data/coordinates.md).
    #[serde(default)]
    pub rotate: Vec3Json,
    #[serde(default)]
    pub translate: Vec3Json,
    #[serde(default)]
    pub scale: Vec3Json,
}

/// A `Lod` node: a distance band and the children it switches.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LodNode {
    /// Read as `level: bool`. 0 or 1, splitting 604/641 across the 13
    /// levels (docs/data/nodes.md).
    #[serde(default)]
    pub field00: i32,
    /// Near and far distance. Clean bands; `1e7` means "no far limit".
    #[serde(default)]
    pub range: Range,
}

/// The single `Light` node: a **directional sun**, not a point light
/// (docs/data/nodes.md).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct LightNode {
    #[serde(default)]
    pub flags: LightFlags,
    /// The sun's direction as Euler angles in radians.
    #[serde(default)]
    pub orientation: Vec3Json,
    #[serde(default)]
    pub translate: Vec3Json,
    /// Directional share of the lighting. Sums with `ambient` to exactly 1.0
    /// in all 13 levels.
    #[serde(default)]
    pub diffuse: f32,
    /// Ambient share.
    #[serde(default)]
    pub ambient: f32,
    /// **(0,0,0) in every level**, so a renderer must substitute white.
    #[serde(default)]
    pub color: Color,
    #[serde(default)]
    pub range: Range,
}

/// The `Light` node's flags. `directed_source` is set and `directional`
/// clear in all 13 levels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
#[allow(missing_docs)]
pub struct LightFlags {
    #[serde(default)]
    pub recalc: bool,
    #[serde(default)]
    pub unk1: bool,
    #[serde(default)]
    pub directional: bool,
    #[serde(default)]
    pub directed_source: bool,
    #[serde(default)]
    pub point_source: bool,
    #[serde(default)]
    pub saturated: bool,
    #[serde(default)]
    pub subdivide: bool,
    #[serde(default)]
    pub r#static: bool,
    #[serde(default)]
    pub color: bool,
    #[serde(default)]
    pub unk9: bool,
    #[serde(default)]
    pub light_map: bool,
    #[serde(default)]
    pub bicolored: bool,
}

/// The single `Camera` node: the original's projection settings.
///
/// Kept and counted only; the engine has its own camera.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CameraNode {
    #[serde(default)]
    pub clip_near: f32,
    #[serde(default)]
    pub clip_far: f32,
    /// Horizontal field of view in radians (m1: 1.047 = 60°).
    #[serde(default)]
    pub fov_h_base: f32,
    #[serde(default)]
    pub fov_v_base: f32,
    /// LOD distance multiplier. 1.0 in every level.
    #[serde(default)]
    pub lod_multiplier: f32,
}

/// The single `Display` node: the original's back-buffer size and clear
/// colour. Kept and counted only.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DisplayNode {
    #[serde(default)]
    pub resolution_x: u32,
    #[serde(default)]
    pub resolution_y: u32,
    /// Components are **0–1** here, as in [`WorldFog`].
    #[serde(default)]
    pub clear_color: Color,
}

/// The single `Window` node: the original's viewport. Kept and counted only.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WindowNode {
    #[serde(default)]
    pub resolution_x: u32,
    #[serde(default)]
    pub resolution_y: u32,
}

/// `<mission>.zmap.json`, the automap (`Zmap`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ZMapFile {
    /// The map's extent. `y` is 0 in both in every mission: the automap is an
    /// x/z projection (docs/data/coordinates.md).
    #[serde(default)]
    pub min: Vec3Json,
    #[serde(default)]
    pub max: Vec3Json,
    #[serde(default)]
    pub features: Vec<ZMapFeatureFile>,
}

/// One automap polyline (`ZmapItem`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ZMapFeatureFile {
    /// **0–255 integers** here, not mech3ax's usual float colour.
    #[serde(default)]
    pub color: ColorU8,
    #[serde(default)]
    pub vertices: Vec<Vec3Json>,
    /// Which objective this feature marks, or `-1` for none (docs/data/anims.md).
    #[serde(default = "minus_one")]
    pub objective: i32,
}

/// An RGB triple with integer components, as the zmap writes it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub struct ColorU8 {
    #[serde(default)]
    pub r: u8,
    #[serde(default)]
    pub g: u8,
    #[serde(default)]
    pub b: u8,
}

/// One interp script as `interp.json` holds it (`Script`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Script {
    /// The original DOS path, backslash-separated (`support\tex_fx.gw`).
    pub name: String,
    /// The script's lines, verbatim. There is no interpreter; docs/data/interp.md
    /// explains which handful of verbs are read.
    #[serde(default)]
    pub lines: Vec<String>,
}

// ---------------------------------------------------------------------------
// `anim.zip` — the level-scripting VM's compiled definitions (docs/data/anims.md)
// ---------------------------------------------------------------------------

/// `metadata.json` inside an `anim.zip` (`AnimMetadata`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AnimMetadata {
    /// Which mission this archive is for, as mech3ax spells it (`RcM01`).
    #[serde(default)]
    pub mission: String,
    /// The gravity the anim VM's `ObjectMotion` events fall under. −9.8 in
    /// every level, and **not** `player.zrd`'s 40 m/s² player gravity.
    #[serde(default)]
    pub gravity: f32,
    /// Every def's file stem, in archive order.
    #[serde(default)]
    pub anim_def_names: Vec<String>,
}

/// One compiled animation definition (`AnimDef`), as a `<stem>.json` holds it.
///
/// The `ptr`/`pad`/`affine` fields mech3ax round-trips are omitted: they are
/// the original executable's addresses and the bytes around them, and nothing
/// the engine does needs them (DESIGN §3.1's mirroring rule).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AnimDef {
    /// The **anim root object**: the node the def hangs off.
    pub name: String,
    /// What `CallAnimation` and the reader files refer to. **Not unique**
    /// within a level: 5–12 effect templates per level share one
    /// (`smxplsn.flt` ×2, `spark_blue.flt` ×3).
    pub anim_name: String,
    /// The node under [`Self::name`] that scopes name lookups inside this
    /// def, which is how the 186 `healthy` nodes in m4 stay distinguishable.
    #[serde(default)]
    pub anim_root_name: String,
    #[serde(default)]
    pub has_callbacks: bool,
    #[serde(default)]
    pub auto_reset_node_states: bool,
    #[serde(default)]
    pub local_nodes_only: bool,
    #[serde(default)]
    pub proximity_damage: bool,
    /// Whether the def starts enabled at all. False on none of the 4798.
    #[serde(default = "default_true")]
    pub active: bool,
    #[serde(default)]
    pub low_priority: bool,
    #[serde(default)]
    pub activation: AnimActivation,
    #[serde(default)]
    pub execution: AnimExecution,
    /// Null on most defs, so `Option` rather than `bool`.
    #[serde(default)]
    pub network_log: Option<bool>,
    #[serde(default)]
    pub save_log: Option<bool>,
    /// `-1` never, `0` immediate, `t > 0` after `t` seconds, `null` n/a,
    /// `-99` on three defs and unknown (docs/data/anims.md).
    #[serde(default)]
    pub reset_time: Option<f32>,
    /// Damage threshold for the hit activations; 0 means "any hit".
    #[serde(default)]
    pub health: f32,
    #[serde(default)]
    pub activ_prereq_min_to_satisfy: u8,
    #[serde(default)]
    pub objects: Option<Vec<AnimNamed>>,
    #[serde(default)]
    pub nodes: Option<Vec<AnimNamed>>,
    #[serde(default)]
    pub lights: Option<Vec<AnimNamed>>,
    #[serde(default)]
    pub puffers: Option<Vec<AnimNamed>>,
    #[serde(default)]
    pub dynamic_sounds: Option<Vec<AnimNamed>>,
    #[serde(default)]
    pub static_sounds: Option<Vec<AnimNamed>>,
    #[serde(default)]
    pub effects: Option<Vec<AnimNamed>>,
    #[serde(default)]
    pub activ_prereqs: Option<Vec<ActivationPrereq>>,
    #[serde(default)]
    pub anim_refs: Option<Vec<AnimRef>>,
    /// The def's initial node states, applied at load and on reset. A
    /// **wrapper object** around the event list, not the list itself.
    #[serde(default)]
    pub reset_state: Option<SeqDef>,
    #[serde(default)]
    pub sequences: Option<Vec<SeqDef>>,
}

fn default_true() -> bool {
    true
}

/// A name in one of an [`AnimDef`]'s resource lists.
///
/// Every list element is an object carrying `name` plus pointers, padding and
/// (for `objects`) a base64 `affine` blob. The name is read, and in
/// `objects`/`nodes` the `ptr`: the node the original engine bound the name
/// to (see `bft_assets::anim::node_id_from_ptr`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AnimNamed {
    pub name: String,
    #[serde(default)]
    pub ptr: u32,
}

/// When a def arms (`AnimActivation`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub enum AnimActivation {
    /// Armed when the level starts (121 defs).
    OnStartup,
    /// Only through `CallAnimation`, `startanims.zrd` or the engine (2897).
    #[default]
    OnCall,
    /// Accumulated weapon damage on one of its nodes reaches `health` (1339).
    WeaponHit,
    /// A collision impulse does (110).
    CollideHit,
    /// Either (331).
    WeaponOrCollideHit,
}

/// The polled condition layered on top of activation (`Execution`).
///
/// **`ByRange` holds squared distances.** All 90 `max` values in the game are
/// exact squares of round numbers (30.25 = 5.5², 160000 = 400²) and every
/// `min` is 0, so the field names say so and the VM compares against
/// `distance_squared`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub enum AnimExecution {
    /// No extra condition (4663 defs).
    #[default]
    None,
    /// Also fires while the player is within `sqrt(max)` metres (90).
    ByRange(ByRange),
    /// Fires while the player is in the root object's `zone_id` (45).
    ByZone,
}

/// The squared near and far bounds of an [`AnimExecution::ByRange`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct ByRange {
    /// Squared metres. 0 in all 90 shipped cases.
    #[serde(default, rename = "min")]
    pub min_sq: f32,
    /// Squared metres.
    #[serde(default, rename = "max")]
    pub max_sq: f32,
}

/// A gate on arming (`ActivationPrereq`).
#[derive(Debug, Clone, Deserialize)]
pub enum ActivationPrereq {
    /// That animation must have started (334).
    Animation(PrereqAnimation),
    /// That node must be in the given active state (30).
    Object(PrereqObject),
    /// The root's parent must be (27).
    Parent(PrereqObject),
}

/// An [`ActivationPrereq::Animation`]'s fields.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PrereqAnimation {
    pub name: String,
    /// A required prereq must hold; the rest count toward
    /// `activ_prereq_min_to_satisfy`.
    #[serde(default)]
    pub required: bool,
}

/// An [`ActivationPrereq::Object`] or [`ActivationPrereq::Parent`]'s fields.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PrereqObject {
    pub name: String,
    #[serde(default)]
    pub required: bool,
    /// The active state the node must be in.
    #[serde(default)]
    pub active: bool,
}

/// A sub-animation the def instantiates by name (`AnimRef`).
#[derive(Debug, Clone, Deserialize)]
pub enum AnimRef {
    /// A connector between two nodes (boat wake, exhaust trail): 112 cases.
    CallObjectConnector(AnimRefConnector),
    /// A plain animation reference: 14 cases.
    CallAnimation(AnimRefAnimation),
}

/// A [`AnimRef::CallObjectConnector`]'s names.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AnimRefConnector {
    pub name: String,
    #[serde(default)]
    pub local_name: Option<String>,
}

/// A [`AnimRef::CallAnimation`]'s name.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AnimRefAnimation {
    pub name: String,
}

/// One sequence of a def, and the shape `reset_state` reuses (`SeqDef`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SeqDef {
    /// Empty on a `reset_state`, which has no name of its own.
    #[serde(default)]
    pub name: String,
    /// `Initial` sequences start when the def activates; `OnCall` ones only
    /// through `CallSequence`. Absent on a `reset_state`.
    #[serde(default)]
    pub seq_state: Option<SeqState>,
    #[serde(default)]
    pub events: Vec<AnimEvent>,
}

/// When a sequence runs (`SeqState`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub enum SeqState {
    /// Runs on activation (12 430).
    #[default]
    Initial,
    /// Runs only via `CallSequence` (3252).
    OnCall,
}

/// One event: when it starts, and what it does (`Event`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AnimEvent {
    /// `null` — 41 171 of 53 167 — means "at the same time as the previous
    /// event", which is how a sequence flips ten node states in one tick.
    #[serde(default)]
    pub start: Option<EventStart>,
    pub data: EventData,
}

/// A non-`null` [`AnimEvent::start`] (`StartOffset`).
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct EventStart {
    pub offset: OffsetFrom,
    /// Seconds after whatever `offset` names.
    #[serde(default)]
    pub time: f32,
}

/// What an [`EventStart::time`] counts from (`StartOffset`'s tag).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Deserialize)]
pub enum OffsetFrom {
    /// The animation's activation (2459).
    #[default]
    Animation,
    /// The previous event's *start* (2262).
    Event,
    /// The sequence's start (649).
    Sequence,
}

/// The 37 event kinds Recoil's compiled anims use (`EventData`).
///
/// Every variant mech3ax emits for Recoil is named so an unknown one is a
/// loud parse error rather than a silently dropped behaviour; the payloads of
/// the presentation events are deliberately not modelled
/// (`serde_json::Value`), because mirroring 30 motion and light structures
/// the loader never reads would be churn with no consumer.
#[derive(Debug, Clone, Deserialize)]
pub enum EventData {
    // --- the VM's own control flow and the mission-visible eight ---
    /// Toggle a node's `active` flag: visibility and collision for its
    /// subtree. The workhorse, 8937 events, and what objectives watch.
    ObjectActiveState(ObjectActiveState),
    /// Start another def by `anim_name` (5333).
    CallAnimation(CallAnimationEvent),
    /// Stop one (383).
    StopAnimation(NamedEvent),
    /// Disarm one permanently (1285).
    InvalidateAnimation(NamedEvent),
    /// Re-arm one (16).
    ResetAnimation(NamedEvent),
    /// Start a sequence of this def by name (2983).
    CallSequence(NamedEvent),
    /// Stop one (1715).
    StopSequence(NamedEvent),
    /// An engine hook. 99 ends the mission (docs/data/anims.md).
    Callback(CallbackEvent),
    /// Re-run the sequence from its first event; `Count` −1 loops for ever.
    Loop(LoopEvent),
    /// Begin a conditional block (1471).
    If(ConditionEvent),
    /// An alternative branch (797).
    Elseif(ConditionEvent),
    /// The else branch (843).
    Else(EmptyEvent),
    /// Close the block (1471).
    Endif(EmptyEvent),

    // --- presentation events, carried but not modelled ---
    ObjectMotion(serde_json::Value),
    ObjectMotionFromTo(serde_json::Value),
    ObjectMotionSiScript(ObjectMotionSiScript),
    ObjectTranslateState(serde_json::Value),
    ObjectScaleState(serde_json::Value),
    ObjectRotateState(serde_json::Value),
    ObjectOpacityState(serde_json::Value),
    ObjectOpacityFromTo(serde_json::Value),
    ObjectAddChild(serde_json::Value),
    ObjectDeleteChild(serde_json::Value),
    ObjectCycleTexture(serde_json::Value),
    ObjectConnector(serde_json::Value),
    CallObjectConnector(serde_json::Value),
    Sound(serde_json::Value),
    SoundNode(serde_json::Value),
    Effect(serde_json::Value),
    LightState(serde_json::Value),
    LightAnimation(serde_json::Value),
    FogState(serde_json::Value),
    FbfxColorFromTo(serde_json::Value),
    FbfxCsinwaveFromTo(serde_json::Value),
    CameraState(serde_json::Value),
    CameraFromTo(serde_json::Value),
    /// A debug toggle; one event in the whole game.
    AnimVerbose(serde_json::Value),
}

impl Default for EventData {
    fn default() -> EventData {
        EventData::Endif(EmptyEvent {})
    }
}

/// `ObjectActiveState`'s fields.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ObjectActiveState {
    pub node: String,
    #[serde(default)]
    pub state: bool,
}

/// An event whose whole payload is one name.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NamedEvent {
    pub name: String,
}

/// `CallAnimation`'s fields.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CallAnimationEvent {
    /// The `anim_name` to start.
    pub name: String,
    /// Run the called animation on this node instead of its own root.
    #[serde(default)]
    pub operand_node: Option<String>,
    /// Non-null on a call the sequence waits for before continuing.
    #[serde(default)]
    pub wait_for_completion: Option<u16>,
    /// Where the called animation plays: 3365 calls name a node to play it
    /// at, 13 a node to play it with, the rest nothing.
    #[serde(default)]
    pub parameters: Option<CallParameters>,
}

/// `CallAnimation`'s `parameters`.
#[derive(Debug, Clone, Deserialize)]
pub enum CallParameters {
    /// Play the called animation at this node: its root object moves there.
    AtNode(CallAtNode),
    /// Play it with this node as its input, without moving it.
    WithNode(CallAtNode),
}

/// The node a [`CallParameters`] names, and an offset in that node's frame.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct CallAtNode {
    pub node: String,
    #[serde(default)]
    pub position: Option<Vec3Json>,
}

/// `Callback`'s value.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct CallbackEvent {
    pub value: i32,
}

/// `Loop`'s count: −1 loops for ever (911 of 1254).
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct LoopEvent {
    #[serde(rename = "Count")]
    pub count: i32,
}

/// `If`'s and `Elseif`'s condition.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct ConditionEvent {
    pub condition: AnimCondition,
}

/// `Else` and `Endif`, which carry nothing.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct EmptyEvent {}

/// `ObjectMotionSiScript`'s reference into the archive's `.zan` scripts.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ObjectMotionSiScript {
    pub name: String,
    /// Index into [`AnimSet::si_scripts`](crate::anim::AnimSet::si_scripts).
    #[serde(default)]
    pub index: i32,
}

/// The four conditions `If`/`Elseif` can test (`Condition`).
///
/// **`PlayerRange` is squared**, like [`ByRange`]: all 31 distinct values in
/// the game are exact squares of round numbers.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
pub enum AnimCondition {
    /// The current LOD level (1204). Presentation; the VM treats it as true.
    AnimationLod(u32),
    /// Squared metres from the player to the def's root (666).
    PlayerRange(f32),
    /// Take this branch with the given probability (342).
    RandomWeight(f32),
    /// Whether the named node is under cover (56).
    NodeUndercover(NodeUndercover),
}

impl Default for AnimCondition {
    fn default() -> AnimCondition {
        AnimCondition::AnimationLod(0)
    }
}

/// [`AnimCondition::NodeUndercover`]'s payload.
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize)]
pub struct NodeUndercover {
    /// mech3ax emits an index, not a name.
    #[serde(default)]
    pub node_index: u32,
}

/// One `.zan.json`: keyframed motion for `ObjectMotionSiScript` (`SiScript`).
///
/// The frames themselves are the handler's; the loader carries them so a
/// handler can be written without touching the loader.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SiScript {
    #[serde(default)]
    pub script_name: String,
    #[serde(default)]
    pub object_name: String,
    #[serde(default)]
    pub frames: Vec<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_manifest_entry_parses_with_the_fields_mech3ax_emits() {
        // Shape copied by hand from a real manifest, values invented.
        let json = r#"{
          "texture_infos": [
            {
              "name": "example",
              "alpha": "Full",
              "width": 64,
              "height": 32,
              "stretch": "Horizontal",
              "image_loaded": false,
              "alpha_loaded": false,
              "palette_loaded": false,
              "palette": "None"
            }
          ],
          "global_palettes": []
        }"#;

        let manifest: TextureManifest = serde_json::from_str(json).expect("parses");
        let info = &manifest.texture_infos[0];

        assert_eq!(info.name, "example");
        assert_eq!(info.alpha, TextureAlpha::Full);
        assert_eq!(info.stretch, TextureStretch::Horizontal);
        assert_eq!((info.width, info.height), (64, 32));
        assert_eq!(info.rename, None);
    }

    #[test]
    fn unknown_fields_and_missing_ones_are_both_tolerated() {
        // The mirroring rule's whole point: mech3ax adding a field must not
        // break us, and dropping one must fall back to a default.
        let json = r#"{
          "texture_infos": [{ "name": "sparse", "brand_new_field": 7 }],
          "some_future_table": {}
        }"#;

        let manifest: TextureManifest = serde_json::from_str(json).expect("parses");
        let info = &manifest.texture_infos[0];

        assert_eq!(info.alpha, TextureAlpha::None);
        assert_eq!(info.stretch, TextureStretch::None);
        assert_eq!((info.width, info.height), (0, 0));
    }

    #[test]
    fn crimson_skies_stretch_variants_do_not_fail_the_parse() {
        let json = r#"{"texture_infos":[{"name":"cs","stretch":"Unk7"}]}"#;

        let manifest: TextureManifest = serde_json::from_str(json).expect("parses");

        assert_eq!(manifest.texture_infos[0].stretch, TextureStretch::Other);
    }

    #[test]
    fn materials_parse_as_an_externally_tagged_sum() {
        let json = r#"[
          { "Textured": { "texture_index": 12, "soil": "Water", "cycle": null, "flag": false } },
          { "Colored": { "color": { "r": 205.0, "g": 205.0, "b": 255.0 }, "alpha": 127, "soil": "Default" } }
        ]"#;

        let materials: Vec<Material> = serde_json::from_str(json).expect("parses");

        let Material::Textured(textured) = &materials[0] else {
            panic!("the first material is textured");
        };
        assert_eq!(textured.texture_index, 12);
        assert_eq!(textured.soil, Soil::Water);
        assert!(textured.cycle.is_none());

        let Material::Colored(colored) = &materials[1] else {
            panic!("the second material is coloured");
        };
        // 0-255, not 0-1.
        assert_eq!(colored.color.b, 255.0);
        assert_eq!(colored.alpha, 127);
    }

    #[test]
    fn a_node_parses_from_the_flat_shape_with_the_variant_under_data() {
        // Shape copied by hand from m1's `nodes.json`, values invented.
        let json = r#"{
          "name": "water02",
          "flags": {
            "active": true, "altitude_surface": true, "intersect_surface": true,
            "intersect_bbox": false, "proximity": false, "landmark": false,
            "bbox_node": true, "bbox_model": false, "bbox_child": true,
            "terrain": false, "can_modify": false, "clip_to": true,
            "tree_valid": true, "override": false, "id_zone_check": true,
            "unk25": true, "unk28": false
          },
          "update_flags": 4,
          "zone_id": 32,
          "model_index": 412,
          "area_partition": { "x": 9, "z": 12 },
          "virtual_partition": null,
          "parent_indices": [0],
          "child_indices": [17, 18],
          "active_bbox": "Node",
          "node_bbox": { "a": { "x": -1.0, "y": 0.0, "z": -2.0 }, "b": { "x": 1.0, "y": 0.0, "z": 2.0 } },
          "model_bbox": { "a": { "x": 0.0, "y": 0.0, "z": 0.0 }, "b": { "x": 0.0, "y": 0.0, "z": 0.0 } },
          "child_bbox": { "a": { "x": 0.0, "y": 0.0, "z": 0.0 }, "b": { "x": 0.0, "y": 0.0, "z": 0.0 } },
          "field192": 0, "field196": 0, "field200": 0, "field204": 0,
          "data": { "Object3d": {
            "opacity": null, "color": null, "unk": 0.0,
            "transform": "Initial", "signs": 128
          } },
          "data_ptr": 25417344, "parent_array_ptr": 0, "child_array_ptr": 0,
          "index": 0
        }"#;

        let node: Node = serde_json::from_str(json).expect("parses");

        assert_eq!(node.name, "water02");
        assert_eq!(node.zone_id, 32);
        assert_eq!(node.model_index, 412);
        assert_eq!(node.area_partition, Some(AreaPartition { x: 9, z: 12 }));
        assert_eq!(node.parent_indices, vec![0]);
        assert_eq!(node.child_indices, vec![17, 18]);
        assert!(node.flags.altitude_surface && node.flags.intersect_surface);
        assert!(node.flags.clip_to);
        assert!(!node.flags.landmark);
        assert_eq!(node.node_bbox.a.z, -2.0);

        let NodeData::Object3d(object) = &node.data else {
            panic!("an Object3d, got {}", node.data.kind());
        };
        assert!(matches!(object.transform, Transform3d::Initial));
        assert_eq!(object.signs, 128);
    }

    #[test]
    fn the_three_transform_forms_all_parse() {
        // "Initial" is a bare string, the other two are externally tagged.
        let initial: Transform3d = serde_json::from_str(r#""Initial""#).expect("parses");
        assert!(matches!(initial, Transform3d::Initial));

        let matrix: Transform3d = serde_json::from_str(
            r#"{ "Matrix": {
                 "r00": 1.0, "r01": 0.0, "r02": 0.0,
                 "r10": 0.0, "r11": 1.0, "r12": 0.0,
                 "r20": 0.0, "r21": 0.0, "r22": 1.0,
                 "r30": 0.6, "r31": 0.5, "r32": 0.3 } }"#,
        )
        .expect("parses");
        let Transform3d::Matrix(m) = matrix else {
            panic!("a Matrix");
        };
        assert_eq!((m.r00, m.r11, m.r22), (1.0, 1.0, 1.0));
        // r30-r32 are the translation, not a fourth matrix row.
        assert_eq!((m.r30, m.r31, m.r32), (0.6, 0.5, 0.3));

        let rts: Transform3d = serde_json::from_str(
            r#"{ "RotateTranslateScale": {
                 "rotate": { "x": 0.0, "y": -1.2217305, "z": 0.0 },
                 "translate": { "x": 2417.0, "y": 0.0, "z": 1117.0 },
                 "scale": { "x": 1.0, "y": 1.0, "z": 1.0 },
                 "original": null } }"#,
        )
        .expect("parses");
        let Transform3d::RotateTranslateScale(rts) = rts else {
            panic!("an RTS");
        };
        assert_eq!(rts.rotate.y, -1.2217305);
        assert_eq!(rts.translate.x, 2417.0);
        assert_eq!(rts.scale.x, 1.0);
    }

    #[test]
    fn an_empty_node_parses_from_the_bare_string_m6_emits() {
        // m6's 287 Empty nodes: `data` is the string "Empty", not an object.
        let json = r#"{
          "name": "street", "zone_id": -1, "model_index": -1,
          "area_partition": null, "parent_indices": [72], "child_indices": [],
          "data": "Empty"
        }"#;

        let node: Node = serde_json::from_str(json).expect("parses");

        assert!(matches!(node.data, NodeData::Empty));
        assert_eq!(node.data.kind(), "Empty");
    }

    #[test]
    fn the_world_node_parses_its_fog_area_and_partition_grid() {
        let json = r#"{ "World": {
          "fog": {
            "fog_type": "Linear",
            "fog_color": { "r": 0.66, "g": 0.76, "b": 0.64 },
            "fog_range": { "min": 550.0, "max": 650.0 },
            "fog_altitude": { "min": 48.0, "max": 24.0 },
            "fog_density": 0.0
          },
          "area": { "left": 0, "top": 0, "right": 4096, "bottom": 4352 },
          "virtual_partition": false,
          "partition_max_dec_feature_count": 16,
          "light_indices": [4198],
          "sound_indices": [],
          "partitions": [
            [ { "x": 0, "z": 4352,
                "min": { "x": 0.0, "y": -12.0, "z": 4096.0 },
                "max": { "x": 256.0, "y": 8.0, "z": 4352.0 },
                "node_indices": [2466], "values": [], "nodes_ptr": 0 } ]
          ]
        } }"#;

        let data: NodeData = serde_json::from_str(json).expect("parses");
        let NodeData::World(world) = data else {
            panic!("a World");
        };

        assert_eq!(world.fog.fog_type, FogType::Linear);
        assert_eq!(
            world.fog.fog_range,
            Range {
                min: 550.0,
                max: 650.0
            }
        );
        // Inverted on purpose: min is the clear altitude, max the foggy one.
        assert!(world.fog.fog_altitude.min > world.fog.fog_altitude.max);
        assert_eq!(world.area.bottom, 4352);
        assert_eq!(world.light_indices, vec![4198]);
        // One z row of one x column, and the cell's y range is its height band.
        assert_eq!(world.partitions.len(), 1);
        assert_eq!(world.partitions[0][0].node_indices, vec![2466]);
        assert_eq!(world.partitions[0][0].min.y, -12.0);
    }

    #[test]
    fn the_light_node_parses_as_the_sun_it_is() {
        let json = r#"{ "Light": {
          "flags": {
            "recalc": true, "unk1": true, "directional": false,
            "directed_source": true, "point_source": false, "saturated": true,
            "subdivide": false, "static": false, "color": false, "unk9": true,
            "light_map": false, "bicolored": false
          },
          "orientation": { "x": -0.5235988, "y": -2.0071287, "z": 0.0 },
          "translate": { "x": 0.0, "y": 0.0, "z": 0.0 },
          "diffuse": 0.55, "ambient": 0.45,
          "color": { "r": 0.0, "g": 0.0, "b": 0.0 },
          "color_ambient": { "r": 0.0, "g": 0.0, "b": 0.0 },
          "range": { "min": 480.0, "max": 540.0 },
          "parent_indices": [0], "parent_ptr": 0
        } }"#;

        let data: NodeData = serde_json::from_str(json).expect("parses");
        let NodeData::Light(light) = data else {
            panic!("a Light");
        };

        assert!(light.flags.directed_source);
        assert!(
            !light.flags.directional,
            "it is not a `directional` light despite being the sun"
        );
        assert_eq!(light.diffuse + light.ambient, 1.0);
        assert_eq!(light.orientation.y, -2.0071287);
        // Black, in every level: a renderer must substitute white.
        assert_eq!(light.color, Color::default());
    }

    #[test]
    fn a_lod_node_parses_its_band_and_its_level_flag() {
        let json = r#"{ "Lod": {
          "field00": 1, "range": { "min": 0.0, "max": 128.0 },
          "field16": 0.0, "field68": 1, "field76": 0.0
        } }"#;

        let data: NodeData = serde_json::from_str(json).expect("parses");
        let NodeData::Lod(lod) = data else {
            panic!("a Lod");
        };

        assert_eq!(lod.field00, 1);
        assert_eq!(
            lod.range,
            Range {
                min: 0.0,
                max: 128.0
            }
        );
    }

    #[test]
    fn a_node_missing_every_optional_field_still_parses_with_no_model_or_zone() {
        // The mirroring rule: `-1` is the "absent" value for both, so the
        // defaults have to be -1 and not 0, which is a valid index.
        let node: Node =
            serde_json::from_str(r#"{ "name": "sparse", "data": "Empty" }"#).expect("parses");

        assert_eq!(node.model_index, -1);
        assert_eq!(node.zone_id, -1);
        assert!(node.parent_indices.is_empty());
        assert_eq!(node.area_partition, None);
    }

    #[test]
    fn a_zmap_parses_with_its_integer_colours() {
        let json = r#"{
          "unk04": 31,
          "min": { "x": 0.0, "y": 0.0, "z": 0.0 },
          "max": { "x": 2768.7866, "y": 0.0, "z": 3423.5288 },
          "features": [ {
            "color": { "r": 255, "g": 255, "b": 255 },
            "vertices": [ { "x": 1684.78, "y": 7.08, "z": 2181.29 } ],
            "objective": 2
          } ]
        }"#;

        let zmap: ZMapFile = serde_json::from_str(json).expect("parses");

        assert_eq!(zmap.max.x, 2768.7866);
        assert_eq!(zmap.min.y, 0.0, "the automap is an x/z projection");
        assert_eq!(
            zmap.features[0].color,
            ColorU8 {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(zmap.features[0].objective, 2);
        assert_eq!(zmap.features[0].vertices.len(), 1);
    }

    #[test]
    fn a_zmap_feature_with_no_objective_defaults_to_minus_one() {
        let feature: ZMapFeatureFile =
            serde_json::from_str(r#"{ "vertices": [] }"#).expect("parses");

        assert_eq!(feature.objective, -1);
    }

    #[test]
    fn baked_cycle_data_parses_even_though_recoil_never_has_any() {
        let json = r#"{
          "texture_indices": [3, 4, 5],
          "looping": true,
          "speed": 6.0,
          "current_frame": 0,
          "cycle_ptr": 0,
          "tex_map_ptr": 0
        }"#;

        let cycle: CycleData = serde_json::from_str(json).expect("parses");

        assert_eq!(cycle.texture_indices, vec![3, 4, 5]);
        assert!(cycle.looping);
        assert_eq!(cycle.speed, 6.0);
    }
}
