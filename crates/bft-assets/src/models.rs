//! Engine-owned model descriptions.
//!
//! A [`ModelDesc`] is what the `gamez` loader produces and what both
//! consumers read: [`crate::geometry::triangulate`] for meshes and colliders
//! alike. Polygons stay as index lists here — triangulation happens once, in
//! `geometry`, so what you see is what you hit (DESIGN §3.1).
//!
//! Everything in this module is renderer-free, so `bft-sim` can build a
//! collider from a `ModelDesc` without a `bevy_render` feature (NFR-4).

use bevy::prelude::*;

use crate::mech3ax;

pub use crate::mech3ax::{FacadeMode, ModelType};

/// One model as the engine describes it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelDesc {
    pub model_type: ModelType,
    /// Which axis a [`ModelType::Facade`] billboard turns about. Meaningless
    /// on other model types; mech3ax still emits a value.
    pub facade_mode: FacadeMode,
    pub flags: ModelFlags,
    pub vertices: Vec<Vec3>,
    /// A **separate table**, indexed only by [`PolygonDesc::normal_indices`]
    /// and never parallel to `vertices` (docs/data/models.md).
    pub normals: Vec<Vec3>,
    /// Morph targets. Parsed and kept; nothing consumes them.
    pub morphs: Vec<Vec3>,
    pub polygons: Vec<PolygonDesc>,
    /// Lights baked into the model. Parsed and kept; nothing consumes them.
    pub lights: Vec<ModelLightDesc>,
    /// UV units per second. Zero on every Recoil model; the real scroll comes
    /// from interp (docs/data/models.md and docs/data/interp.md).
    pub texture_scroll: Vec2,
    /// The model's own bounding-box centre, used as the facade pivot.
    pub bbox_mid: Vec3,
    pub bbox_diag: f32,
}

/// Per-model rendering flags.
///
/// `texture_registered`, `morph`, `texture_scroll`, `clouds` and
/// `facade_centroid` are false on all 13 036 Recoil models; they are kept so
/// another game's data round-trips and so the census can count them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct ModelFlags {
    /// `false` makes the material `unlit`.
    pub lighting: bool,
    pub fog: bool,
    pub texture_registered: bool,
    pub morph: bool,
    pub texture_scroll: bool,
    pub clouds: bool,
    /// Facade rotates about [`ModelDesc::bbox_mid`] rather than its origin.
    pub facade_centroid: bool,
}

/// One polygon: a 3- to 14-gon in Recoil, 73% of them quads.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PolygonDesc {
    /// Double-sided: no backface culling (`MaterialParams`).
    pub show_backface: bool,
    /// Never true in Recoil; [`crate::geometry`] handles it anyway.
    pub tri_strip: bool,
    /// Never emitted in Recoil. Meaning unknown; kept, does nothing.
    pub in_out: bool,
    /// Draw order, probably for transparency. Non-zero on 795 polygons.
    /// Kept, does nothing.
    pub priority: i32,
    /// Which zones the polygon belongs to; usually empty. Kept, does nothing.
    pub zone_set: Vec<i8>,
    /// Indices into [`ModelDesc::vertices`].
    pub vertex_indices: Vec<u32>,
    /// Indices into [`ModelDesc::normals`], when the polygon has authored
    /// normals. `None` on 98% of polygons, which use the face normal.
    pub normal_indices: Option<Vec<u32>>,
    /// Linear RGB per corner. Empty on every Recoil polygon.
    pub vertex_colors: Vec<[f32; 3]>,
    /// Exactly one in Recoil; the format allows more (emitted as layered
    /// duplicates).
    pub materials: Vec<PolygonMaterialDesc>,
}

/// One material reference on a polygon, with its UVs.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PolygonMaterialDesc {
    /// Index into `LevelData::materials`.
    pub material_index: u32,
    /// One coordinate per polygon corner. `None` exactly when the material is
    /// `Colored` — all 2610 occurrences, verified across the 13 levels — in
    /// which case triangulation emits zeroes.
    pub uv_coords: Option<Vec<Vec2>>,
}

/// A light baked into a model.
///
/// Every field mech3ax still calls `unk*` is dropped per the mirroring rule,
/// which leaves position, colour and the flag word. Nothing in this crate
/// consumes them — but they are **not** decorative: every level has
/// exactly one `Light` node against 3288 of these on 203 `Object3d` nodes,
/// all on weapon and effect templates (docs/data/nodes.md);
/// `position` is model-local and needs the node transform applied.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelLightDesc {
    /// The light's position. mech3ax calls this a vertex list; 3224 of the
    /// 3272 lights have exactly one, so the first is taken and the rest kept
    /// in `extra_positions`.
    pub position: Vec3,
    /// Positions beyond the first, for the 48 lights that have them.
    pub extra_positions: Vec<Vec3>,
    /// Linear RGB, converted from mech3ax's 0–255.
    pub color: [f32; 3],
    /// Meaning unknown; 27 distinct values occur.
    pub flags: u16,
}

/// Convert one mech3ax model.
///
/// Infallible: a polygon index out of range is not rejected here, because a
/// single bad polygon should not cost a level its geometry.
/// [`crate::geometry::triangulate`] skips such polygons and counts them.
pub fn model_desc(model: &mech3ax::Model) -> ModelDesc {
    ModelDesc {
        model_type: model.model_type,
        facade_mode: model.facade_mode,
        flags: ModelFlags {
            lighting: model.flags.lighting,
            fog: model.flags.fog,
            texture_registered: model.flags.texture_registered,
            morph: model.flags.morph,
            texture_scroll: model.flags.texture_scroll,
            clouds: model.flags.clouds,
            facade_centroid: model.flags.facade_centroid,
        },
        vertices: model.vertices.iter().copied().map(vec3).collect(),
        normals: model.normals.iter().copied().map(vec3).collect(),
        morphs: model.morphs.iter().copied().map(vec3).collect(),
        polygons: model.polygons.iter().map(polygon_desc).collect(),
        lights: model.lights.iter().map(light_desc).collect(),
        texture_scroll: Vec2::new(model.texture_scroll.u, model.texture_scroll.v),
        bbox_mid: vec3(model.bbox_mid),
        bbox_diag: model.bbox_diag,
    }
}

/// Convert a whole level's model table, logging the census of unused features.
///
/// The counts go out at `info!` once per level so the recon has numbers for
/// the features this crate deliberately parses and ignores.
pub fn model_descs(models: &[mech3ax::Model]) -> Vec<ModelDesc> {
    let descs: Vec<ModelDesc> = models.iter().map(model_desc).collect();

    log_ignored_features(&descs);

    descs
}

/// Count the features that are parsed and do nothing.
fn log_ignored_features(models: &[ModelDesc]) {
    let mut morph_models = 0usize;
    let mut morph_points = 0usize;
    let mut light_models = 0usize;
    let mut lights = 0usize;
    let mut clouds = 0usize;
    let mut facade_centroid = 0usize;
    let mut facades = 0usize;
    let mut points = 0usize;
    let mut polygons = 0usize;
    let mut in_out = 0usize;
    let mut tri_strip = 0usize;
    let mut prioritised = 0usize;
    let mut zoned = 0usize;

    for model in models {
        if !model.morphs.is_empty() {
            morph_models += 1;
            morph_points += model.morphs.len();
        }
        if !model.lights.is_empty() {
            light_models += 1;
            lights += model.lights.len();
        }
        if model.flags.clouds {
            clouds += 1;
        }
        if model.flags.facade_centroid {
            facade_centroid += 1;
        }
        match model.model_type {
            ModelType::Facade => facades += 1,
            ModelType::Points => points += 1,
            ModelType::Default => {}
        }

        for polygon in &model.polygons {
            polygons += 1;
            if polygon.in_out {
                in_out += 1;
            }
            if polygon.tri_strip {
                tri_strip += 1;
            }
            if polygon.priority != 0 {
                prioritised += 1;
            }
            if !polygon.zone_set.is_empty() {
                zoned += 1;
            }
        }
    }

    info!(
        "{} models, {polygons} polygons: {facades} facades, {points} points models; \
         parsed-but-unused: {morph_models} models with {morph_points} morph points, \
         {light_models} models with {lights} point lights, {clouds} clouds, \
         {facade_centroid} facade-centroid, {tri_strip} tri-strip polygons, \
         {in_out} in-out, {prioritised} with a draw priority, {zoned} with a zone set",
        models.len(),
    );
}

fn polygon_desc(polygon: &mech3ax::Polygon) -> PolygonDesc {
    PolygonDesc {
        show_backface: polygon.flags.show_backface,
        tri_strip: polygon.flags.tri_strip,
        in_out: polygon.flags.in_out,
        priority: polygon.priority,
        zone_set: polygon.zone_set.clone(),
        vertex_indices: polygon.vertex_indices.clone(),
        normal_indices: polygon.normal_indices.clone(),
        vertex_colors: polygon
            .vertex_colors
            .iter()
            .map(|color| {
                [
                    color_byte_to_linear(color.r),
                    color_byte_to_linear(color.g),
                    color_byte_to_linear(color.b),
                ]
            })
            .collect(),
        materials: polygon
            .materials
            .iter()
            .map(|material| PolygonMaterialDesc {
                material_index: material.material_index,
                uv_coords: material
                    .uv_coords
                    .as_ref()
                    .map(|coords| coords.iter().map(|uv| Vec2::new(uv.u, uv.v)).collect()),
            })
            .collect(),
    }
}

fn light_desc(light: &mech3ax::PointLight) -> ModelLightDesc {
    let mut positions = light.vertices.iter().copied().map(vec3);

    ModelLightDesc {
        position: positions.next().unwrap_or(Vec3::ZERO),
        extra_positions: positions.collect(),
        color: [
            color_byte_to_linear(light.color.r),
            color_byte_to_linear(light.color.g),
            color_byte_to_linear(light.color.b),
        ],
        flags: light.flags,
    }
}

fn vec3(v: mech3ax::Vec3Json) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

/// mech3ax's 0–255 sRGB float to linear 0–1.
///
/// The same transfer `materials::srgb_byte_to_linear` applies; duplicated
/// rather than shared because the two modules are otherwise independent and
/// the function is four lines.
fn color_byte_to_linear(component: f32) -> f32 {
    let value = (component / 255.0).clamp(0.0, 1.0);

    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_model_converts_with_its_polygons_lights_and_flags() {
        // Shape copied by hand from a real `models.json`, values invented.
        let json = r#"{
          "model_type": "Facade",
          "facade_mode": "CylindricalY",
          "flags": { "lighting": true, "fog": true, "clouds": false, "facade_centroid": false },
          "parent_count": 1,
          "vertices": [ { "x": 1.0, "y": 2.0, "z": 3.0 } ],
          "normals": [ { "x": 0.0, "y": 1.0, "z": 0.0 } ],
          "morphs": [],
          "lights": [ {
            "unk00": 0, "unk04": 0, "unk08": 0.0,
            "vertices": [ { "x": 0.0, "y": 13.0, "z": 0.0 } ],
            "unk24": 0, "color": { "r": 255.0, "g": 0.0, "b": 0.0 },
            "flags": 363, "vertices_ptr": 0,
            "unk48": 0.0, "unk52": 0.0, "unk56": 0.0,
            "unk60": 0, "unk64": 30.0, "unk68": 400.0, "unk72": 600.0
          } ],
          "polygons": [ {
            "flags": { "show_backface": true, "tri_strip": false },
            "priority": 2,
            "zone_set": [ -1 ],
            "vertex_indices": [ 0, 1, 2, 3 ],
            "normal_indices": null,
            "vertex_colors": [],
            "materials": [ { "material_index": 7, "uv_coords": [ { "u": 1.0, "v": 0.0 } ] } ],
            "vertex_indices_ptr": 1, "normal_indices_ptr": 0, "uvs_ptr": 2,
            "vertex_colors_ptr": 0, "matl_refs_ptr": 0, "materials_ptr": 0
          } ],
          "texture_scroll": { "u": 0.0, "v": 0.0 },
          "bbox_mid": { "x": 0.5, "y": 1.0, "z": 0.0 },
          "bbox_diag": 17.6,
          "polygons_ptr": 0, "vertices_ptr": 0, "normals_ptr": 0,
          "lights_ptr": 0, "morphs_ptr": 0, "material_refs_ptr": 0
        }"#;

        let model: mech3ax::Model = serde_json::from_str(json).expect("parses");
        let desc = model_desc(&model);

        assert_eq!(desc.model_type, ModelType::Facade);
        assert_eq!(desc.facade_mode, FacadeMode::CylindricalY);
        assert!(desc.flags.lighting && desc.flags.fog);
        assert_eq!(desc.vertices, vec![Vec3::new(1.0, 2.0, 3.0)]);
        assert_eq!(desc.normals, vec![Vec3::Y]);
        assert_eq!(desc.bbox_mid, Vec3::new(0.5, 1.0, 0.0));
        assert_eq!(desc.bbox_diag, 17.6);

        let polygon = &desc.polygons[0];
        assert!(polygon.show_backface);
        assert!(!polygon.tri_strip);
        assert_eq!(polygon.priority, 2);
        assert_eq!(polygon.vertex_indices, vec![0, 1, 2, 3]);
        assert_eq!(polygon.materials[0].material_index, 7);
        assert_eq!(
            polygon.materials[0].uv_coords.as_deref(),
            Some(&[Vec2::new(1.0, 0.0)][..])
        );

        let light = &desc.lights[0];
        assert_eq!(light.position, Vec3::new(0.0, 13.0, 0.0));
        assert!(light.extra_positions.is_empty());
        assert_eq!(light.color[0], 1.0, "255 is full brightness");
        assert_eq!(light.flags, 363);
    }

    #[test]
    fn the_flags_mech3ax_omits_when_false_default_to_false() {
        // The real emitted shape: only the four non-skipped flags, and a
        // polygon whose `in_out` is simply absent.
        let json = r#"{
          "model_type": "Default",
          "facade_mode": "CylindricalY",
          "flags": { "lighting": false, "fog": true, "clouds": false, "facade_centroid": false },
          "vertices": [], "normals": [], "morphs": [], "lights": [],
          "polygons": [ {
            "flags": { "show_backface": false, "tri_strip": false },
            "priority": 0, "zone_set": [], "vertex_indices": [],
            "normal_indices": null, "vertex_colors": [], "materials": []
          } ],
          "texture_scroll": { "u": 0.0, "v": 0.0 },
          "bbox_mid": { "x": 0.0, "y": 0.0, "z": 0.0 }, "bbox_diag": 0.0
        }"#;

        let model: mech3ax::Model = serde_json::from_str(json).expect("parses");
        let desc = model_desc(&model);

        assert!(!desc.flags.texture_registered);
        assert!(!desc.flags.morph);
        assert!(!desc.flags.texture_scroll);
        assert!(desc.flags.fog);
        assert!(!desc.polygons[0].in_out);
    }

    #[test]
    fn a_light_with_several_positions_keeps_all_of_them() {
        let json = r#"{
          "vertices": [
            { "x": 1.0, "y": 0.0, "z": 0.0 },
            { "x": 2.0, "y": 0.0, "z": 0.0 }
          ],
          "color": { "r": 0.0, "g": 0.0, "b": 0.0 },
          "flags": 7
        }"#;

        let light: mech3ax::PointLight = serde_json::from_str(json).expect("parses");
        let desc = light_desc(&light);

        assert_eq!(desc.position, Vec3::X);
        assert_eq!(desc.extra_positions, vec![Vec3::new(2.0, 0.0, 0.0)]);
    }

    #[test]
    fn a_light_with_no_position_lands_at_the_origin_rather_than_panicking() {
        let light = mech3ax::PointLight::default();

        assert_eq!(light_desc(&light).position, Vec3::ZERO);
    }

    #[test]
    fn unknown_fields_and_a_missing_table_are_both_tolerated() {
        // The mirroring rule again: mech3ax adding or dropping a field must
        // not break the loader.
        let json = r#"{ "model_type": "Points", "brand_new_field": 7 }"#;

        let model: mech3ax::Model = serde_json::from_str(json).expect("parses");
        let desc = model_desc(&model);

        assert_eq!(desc.model_type, ModelType::Points);
        assert!(desc.vertices.is_empty());
        assert!(desc.polygons.is_empty());
        assert_eq!(desc.texture_scroll, Vec2::ZERO);
    }

    #[test]
    fn the_batch_converter_keeps_order() {
        let a = mech3ax::Model {
            bbox_diag: 1.0,
            ..Default::default()
        };
        let b = mech3ax::Model {
            bbox_diag: 2.0,
            ..Default::default()
        };

        let descs = model_descs(&[a, b]);

        assert_eq!(descs.len(), 2);
        assert_eq!(descs[0].bbox_diag, 1.0);
        assert_eq!(descs[1].bbox_diag, 2.0);
    }
}
