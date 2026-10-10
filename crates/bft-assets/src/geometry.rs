//! Polygons → triangles. Pure math, no `bevy_render` (NFR-4).
//!
//! One triangulation feeds both consumers — `bft-game`'s mesh builder and
//! `bft-sim`'s trimesh colliders — so what you see is what you hit
//! (DESIGN §7.1). That is the whole reason this lives in `bft-assets` and
//! takes no renderer types: a dedicated server builds colliders from it
//! without a GPU.
//!
//! # Flat-shaded duplicates
//!
//! Every polygon corner becomes its own vertex. UVs and colours are authored
//! per polygon rather than per vertex, so sharing would need a weld keyed on
//! (position, normal, uv, colour) and would save little on this data. The
//! open question — weld or not — stays open; start duplicated and
//! measure (m1 is 10 092 polygons, so the cost is small).

use bevy::prelude::*;

use crate::axes::AxisConvention;
use crate::models::{ModelDesc, ModelType, PolygonDesc};

/// A model's polygons expanded into triangles, grouped by material.
///
/// The vertex attributes are parallel arrays over corners; `groups` index
/// into them. A group exists for each material index the model actually
/// uses, in first-use order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Triangulated {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub uvs: Vec<Vec2>,
    /// Linear RGBA. White unless the polygon authored vertex colours, which
    /// no Recoil polygon does.
    pub colors: Vec<[f32; 4]>,
    /// One per material index used, in first-use order.
    pub groups: Vec<TriangleGroup>,
    /// Polygons skipped because an index pointed outside the vertex table.
    /// Zero on all 13 Recoil levels; a loader logs it rather than failing.
    pub skipped_polygons: usize,
}

impl Triangulated {
    /// How many triangles across every group.
    pub fn triangle_count(&self) -> usize {
        self.groups
            .iter()
            .map(|group| group.indices.len() / 3)
            .sum()
    }

    /// Whether any corner carries a colour other than opaque white.
    ///
    /// The mesh builder adds `ATTRIBUTE_COLOR` only when this is true, which
    /// on Recoil data is never.
    pub fn has_vertex_colors(&self) -> bool {
        self.colors.iter().any(|color| *color != WHITE)
    }
}

/// The triangles sharing one material.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TriangleGroup {
    /// Index into `LevelData::materials`.
    pub material_index: u32,
    /// Corner indices into [`Triangulated`]'s attribute arrays, three per
    /// triangle.
    pub indices: Vec<u32>,
    /// True when **any** polygon in the group is double-sided. Culling is a
    /// material-level property in Bevy, and a group is one draw, so one
    /// double-sided polygon makes the group double-sided — over-drawing a
    /// backface is invisible, dropping a needed one is a hole.
    pub show_backface: bool,
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Expand a model's polygons into triangles in Bevy's coordinate system.
///
/// Winding: the source data is already counter-clockwise seen from outside
/// under a right-handed cross product (docs/data/models.md), which
/// is Bevy's front-face convention, so triangles keep their order — unless
/// `axes` mirrors, in which case each is reversed so front faces stay front.
pub fn triangulate(model: &ModelDesc, axes: &AxisConvention) -> Triangulated {
    let mut out = Triangulated::default();

    for polygon in &model.polygons {
        if polygon.vertex_indices.len() < 3 {
            // Not a surface. Points models are made of these.
            continue;
        }
        if polygon
            .vertex_indices
            .iter()
            .any(|index| *index as usize >= model.vertices.len())
        {
            out.skipped_polygons += 1;
            continue;
        }

        // A polygon with no material still needs to exist for the collider,
        // so it goes into group 0 rather than being dropped. Recoil always
        // has exactly one material; more than one is emitted once per
        // material as layered duplicates.
        if polygon.materials.is_empty() {
            emit(&mut out, model, polygon, None, axes);
        } else {
            for material in &polygon.materials {
                emit(&mut out, model, polygon, Some(material_ref(material)), axes);
            }
        }
    }

    out
}

/// What [`emit`] needs from a [`crate::models::PolygonMaterialDesc`].
struct MaterialRef<'a> {
    index: u32,
    uvs: Option<&'a [Vec2]>,
}

fn material_ref(material: &crate::models::PolygonMaterialDesc) -> MaterialRef<'_> {
    MaterialRef {
        index: material.material_index,
        uvs: material.uv_coords.as_deref(),
    }
}

/// Emit one polygon's corners and triangles into one material's group.
fn emit(
    out: &mut Triangulated,
    model: &ModelDesc,
    polygon: &PolygonDesc,
    material: Option<MaterialRef<'_>>,
    axes: &AxisConvention,
) {
    let corners = polygon.vertex_indices.len();
    let base = out.positions.len() as u32;
    let flip = axes.flips_winding();

    // Corner attributes first, so the triangle indices below are just
    // arithmetic on `base`. The face normal is computed in source space and
    // converted with the rest, so it stays consistent with the positions.
    let face_normal = face_normal(model, polygon);

    for corner in 0..corners {
        let vertex = model.vertices[polygon.vertex_indices[corner] as usize];

        out.positions.push(axes.point(vertex));
        out.normals
            .push(axes.direction(corner_normal(model, polygon, corner, face_normal)));
        out.uvs.push(
            material
                .as_ref()
                .and_then(|material| material.uvs)
                .and_then(|uvs| uvs.get(corner).copied())
                // `None` exactly on `Colored` materials, which have nothing
                // to sample (docs/data/models.md).
                .unwrap_or(Vec2::ZERO),
        );
        out.colors.push(
            polygon
                .vertex_colors
                .get(corner)
                .map(|color| [color[0], color[1], color[2], 1.0])
                .unwrap_or(WHITE),
        );
    }

    let material_index = material
        .as_ref()
        .map(|material| material.index)
        .unwrap_or(0);
    let group = group_for(out, material_index, polygon.show_backface);

    if polygon.tri_strip {
        // Never fires on Recoil data; the format still means it.
        // Corners 0,1,2 then 1,3,2 then 2,3,4 … alternating winding so every
        // triangle faces the same way.
        for triangle in 0..corners - 2 {
            let (a, b, c) = if triangle % 2 == 0 {
                (triangle, triangle + 1, triangle + 2)
            } else {
                (triangle + 1, triangle, triangle + 2)
            };
            push_triangle(&mut out.groups[group].indices, base, a, b, c, flip);
        }
    } else {
        // Fan from corner 0. The n-gons are authored planar and convex —
        // 226 of 83 066 are not, and those are mildly concave quads and
        // pentagons whose fan is at worst a sliver (docs/data/models.md).
        for triangle in 1..corners - 1 {
            push_triangle(
                &mut out.groups[group].indices,
                base,
                0,
                triangle,
                triangle + 1,
                flip,
            );
        }
    }
}

fn push_triangle(indices: &mut Vec<u32>, base: u32, a: usize, b: usize, c: usize, flip: bool) {
    let (a, b, c) = (base + a as u32, base + b as u32, base + c as u32);

    if flip {
        indices.extend_from_slice(&[a, c, b]);
    } else {
        indices.extend_from_slice(&[a, b, c]);
    }
}

/// Find or create the group for a material index.
fn group_for(out: &mut Triangulated, material_index: u32, show_backface: bool) -> usize {
    if let Some(index) = out
        .groups
        .iter()
        .position(|group| group.material_index == material_index)
    {
        out.groups[index].show_backface |= show_backface;
        return index;
    }

    out.groups.push(TriangleGroup {
        material_index,
        indices: Vec::new(),
        show_backface,
    });

    out.groups.len() - 1
}

/// The polygon's geometric normal, from its first three corners.
///
/// `Vec3::ZERO` for a degenerate polygon; none occur in Recoil, and a zero
/// normal is better than a NaN one.
fn face_normal(model: &ModelDesc, polygon: &PolygonDesc) -> Vec3 {
    let corner = |i: usize| model.vertices[polygon.vertex_indices[i] as usize];
    let normal = (corner(1) - corner(0)).cross(corner(2) - corner(0));

    normal.try_normalize().unwrap_or(Vec3::ZERO)
}

/// One corner's normal: authored when the polygon has `normal_indices`, the
/// face normal otherwise.
///
/// `normals` is its own table and is **not** parallel to `vertices`
/// (docs/data/models.md), so it is only ever indexed through
/// `normal_indices`, and an out-of-range entry falls back to the face normal
/// rather than panicking.
fn corner_normal(
    model: &ModelDesc,
    polygon: &PolygonDesc,
    corner: usize,
    face_normal: Vec3,
) -> Vec3 {
    polygon
        .normal_indices
        .as_ref()
        .and_then(|indices| indices.get(corner))
        .and_then(|index| model.normals.get(*index as usize))
        .copied()
        .and_then(|normal| normal.try_normalize())
        .unwrap_or(face_normal)
}

/// Merge every group into one vertex/index pair for a trimesh collider.
///
/// [`ModelType::Points`] and [`ModelType::Facade`] models produce nothing:
/// points have no surface, and a facade is a billboard whose
/// orientation changes every frame, so a static collider built from it would
/// be wrong as soon as the camera moved.
///
/// The positions are returned whole rather than compacted: the indices come
/// straight from [`Triangulated`], and avian3d tolerates unreferenced
/// vertices.
pub fn collider_indices(
    model_type: ModelType,
    triangulated: &Triangulated,
) -> (Vec<Vec3>, Vec<[u32; 3]>) {
    if matches!(model_type, ModelType::Points | ModelType::Facade) {
        return (Vec::new(), Vec::new());
    }

    let mut triangles = Vec::with_capacity(triangulated.triangle_count());

    for group in &triangulated.groups {
        // `as_chunks` gives `&[u32; 3]` directly; the remainder is always
        // empty, since every triangle pushes exactly three indices.
        let (chunks, _) = group.indices.as_chunks::<3>();
        triangles.extend_from_slice(chunks);
    }

    (triangulated.positions.clone(), triangles)
}

/// Which material each merged collider triangle came from, parallel to
/// [`collider_indices`]'s triangle list.
///
/// `bft_sim`'s `SurfaceInfo { soil_by_triangle }` is
/// `material_per_triangle(..).map(|index| materials[index as usize].soil())`.
/// It exists so the sim never has to re-derive the merge order: triangles
/// are emitted group by group, in `groups` order, and a caller that assumed
/// polygon order instead would attribute the wrong soil to every triangle
/// after the first material change — which is the difference between
/// driving on water and driving on rock.
pub fn material_per_triangle(model_type: ModelType, triangulated: &Triangulated) -> Vec<u32> {
    if matches!(model_type, ModelType::Points | ModelType::Facade) {
        return Vec::new();
    }

    triangulated
        .groups
        .iter()
        .flat_map(|group| std::iter::repeat_n(group.material_index, group.indices.len() / 3))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axes::{Axis, AxisConvention};
    use crate::models::{ModelDesc, PolygonDesc, PolygonMaterialDesc};

    fn quad_model() -> ModelDesc {
        ModelDesc {
            vertices: vec![
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(1.0, 0.0, 1.0),
                Vec3::new(0.0, 0.0, 1.0),
            ],
            polygons: vec![PolygonDesc {
                vertex_indices: vec![0, 1, 2, 3],
                materials: vec![PolygonMaterialDesc {
                    material_index: 3,
                    uv_coords: Some(vec![
                        Vec2::new(0.0, 0.0),
                        Vec2::new(1.0, 0.0),
                        Vec2::new(1.0, 1.0),
                        Vec2::new(0.0, 1.0),
                    ]),
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    #[test]
    fn a_quad_fans_into_two_triangles_sharing_the_first_corner() {
        let t = triangulate(&quad_model(), &AxisConvention::GAMEZ_DEFAULT);

        assert_eq!(t.positions.len(), 4, "one vertex per corner");
        assert_eq!(t.groups.len(), 1);
        assert_eq!(t.groups[0].material_index, 3);
        assert_eq!(t.groups[0].indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(t.triangle_count(), 2);
        assert_eq!(t.skipped_polygons, 0);
    }

    #[test]
    fn a_strip_expands_with_alternating_winding() {
        // Five corners -> three triangles. The odd one swaps its first two
        // corners so every triangle faces the same way.
        let mut model = quad_model();
        model.vertices.push(Vec3::new(2.0, 0.0, 0.0));
        model.polygons[0].tri_strip = true;
        model.polygons[0].vertex_indices = vec![0, 1, 2, 3, 4];
        model.polygons[0].materials[0].uv_coords = None;

        let t = triangulate(&model, &AxisConvention::GAMEZ_DEFAULT);

        assert_eq!(
            t.groups[0].indices,
            vec![
                0, 1, 2, // even
                2, 1, 3, // odd: first two swapped
                2, 3, 4, // even
            ]
        );
        assert_eq!(t.triangle_count(), 3);
    }

    #[test]
    fn a_strip_and_a_fan_of_the_same_quad_wind_the_same_way() {
        // The point of the alternation: every triangle of a strip must face
        // the way the fan's do, or half a surface turns inside out.
        //
        // The two orders are different by definition. A fan lists the quad's
        // corners around its perimeter (0,1,2,3); the same quad as a strip is
        // the zigzag 0,1,3,2. Feeding perimeter order to the strip path would
        // produce a bowtie, which is a property of the input, not of the
        // expansion.
        let mut strip = quad_model();
        strip.polygons[0].tri_strip = true;
        strip.polygons[0].vertex_indices = vec![0, 1, 3, 2];

        let fanned = triangulate(&quad_model(), &AxisConvention::GAMEZ_DEFAULT);
        let stripped = triangulate(&strip, &AxisConvention::GAMEZ_DEFAULT);

        assert_eq!(fanned.triangle_count(), stripped.triangle_count());

        for t in [&fanned, &stripped] {
            let (triangles, _) = t.groups[0].indices.as_chunks::<3>();
            for triangle in triangles {
                let normal = (t.positions[triangle[1] as usize]
                    - t.positions[triangle[0] as usize])
                    .cross(t.positions[triangle[2] as usize] - t.positions[triangle[0] as usize]);
                assert!(
                    normal.normalize().dot(Vec3::NEG_Y) > 0.99,
                    "every triangle faces the same way: {normal:?}"
                );
            }
        }
    }

    #[test]
    fn polygons_split_into_one_group_per_material_in_first_use_order() {
        let mut model = quad_model();
        // A second quad on material 1, then a third back on material 3.
        for offset in [1.0f32, 2.0] {
            let base = model.vertices.len() as u32;
            model.vertices.extend([
                Vec3::new(0.0, offset, 0.0),
                Vec3::new(1.0, offset, 0.0),
                Vec3::new(1.0, offset, 1.0),
                Vec3::new(0.0, offset, 1.0),
            ]);
            model.polygons.push(PolygonDesc {
                vertex_indices: vec![base, base + 1, base + 2, base + 3],
                materials: vec![PolygonMaterialDesc {
                    material_index: if offset == 1.0 { 1 } else { 3 },
                    uv_coords: None,
                }],
                ..Default::default()
            });
        }

        let t = triangulate(&model, &AxisConvention::GAMEZ_DEFAULT);

        assert_eq!(t.groups.len(), 2, "two distinct materials, three polygons");
        assert_eq!(t.groups[0].material_index, 3, "first used, first group");
        assert_eq!(t.groups[1].material_index, 1);
        assert_eq!(t.groups[0].indices.len(), 12, "two quads on material 3");
        assert_eq!(t.groups[1].indices.len(), 6);
        assert_eq!(t.positions.len(), 12, "corners are never shared");
    }

    #[test]
    fn a_multi_material_polygon_is_emitted_once_per_material() {
        // Never happens in Recoil (all 103 158 polygons have exactly one
        // material), but the format allows it, as layered duplicates.
        let mut model = quad_model();
        model.polygons[0].materials.push(PolygonMaterialDesc {
            material_index: 9,
            uv_coords: None,
        });

        let t = triangulate(&model, &AxisConvention::GAMEZ_DEFAULT);

        assert_eq!(t.groups.len(), 2);
        assert_eq!(t.triangle_count(), 4, "the quad twice");
        assert_eq!(t.positions.len(), 8, "its corners twice");
    }

    #[test]
    fn a_polygon_with_no_uvs_gets_zeroes_rather_than_being_dropped() {
        // The `Colored` material case: 2610 of them across the 13 levels.
        let mut model = quad_model();
        model.polygons[0].materials[0].uv_coords = None;

        let t = triangulate(&model, &AxisConvention::GAMEZ_DEFAULT);

        assert_eq!(t.uvs, vec![Vec2::ZERO; 4]);
        assert_eq!(t.triangle_count(), 2);
    }

    #[test]
    fn authored_normals_are_used_and_the_face_normal_fills_in_otherwise() {
        let mut model = quad_model();
        model.normals = vec![Vec3::X, Vec3::Z];
        model.polygons[0].normal_indices = Some(vec![0, 1, 0, 1]);

        let t = triangulate(&model, &AxisConvention::GAMEZ_DEFAULT);
        assert_eq!(t.normals, vec![Vec3::X, Vec3::Z, Vec3::X, Vec3::Z]);

        // Without `normal_indices`, every corner gets the face normal. The
        // quad lies in the y=0 plane wound so its normal points at -y.
        let t = triangulate(&quad_model(), &AxisConvention::GAMEZ_DEFAULT);
        assert_eq!(t.normals, vec![Vec3::NEG_Y; 4]);
    }

    #[test]
    fn an_out_of_range_normal_index_falls_back_to_the_face_normal() {
        // `normals` is its own table and is shorter than `vertices` on 400
        // of the 456 models that have one, so this has to be safe.
        let mut model = quad_model();
        model.normals = vec![Vec3::X];
        model.polygons[0].normal_indices = Some(vec![0, 7, 0, 0]);

        let t = triangulate(&model, &AxisConvention::GAMEZ_DEFAULT);

        assert_eq!(t.normals[1], Vec3::NEG_Y, "corner 1 falls back");
        assert_eq!(t.normals[0], Vec3::X);
    }

    #[test]
    fn a_polygon_indexing_past_the_vertex_table_is_counted_not_a_panic() {
        let mut model = quad_model();
        model.polygons[0].vertex_indices = vec![0, 1, 99];

        let t = triangulate(&model, &AxisConvention::GAMEZ_DEFAULT);

        assert_eq!(t.skipped_polygons, 1);
        assert_eq!(t.triangle_count(), 0);
        assert!(t.groups.is_empty());
    }

    #[test]
    fn a_polygon_with_fewer_than_three_corners_is_skipped_silently() {
        let mut model = quad_model();
        model.polygons[0].vertex_indices = vec![0, 1];

        let t = triangulate(&model, &AxisConvention::GAMEZ_DEFAULT);

        assert_eq!(t.triangle_count(), 0);
        assert_eq!(t.skipped_polygons, 0, "not a defect, just not a surface");
    }

    #[test]
    fn a_mirroring_convention_reverses_every_triangle() {
        let mirrored = AxisConvention {
            z: Axis::NegZ,
            ..AxisConvention::GAMEZ_DEFAULT
        };

        let straight = triangulate(&quad_model(), &AxisConvention::GAMEZ_DEFAULT);
        let flipped = triangulate(&quad_model(), &mirrored);

        assert_eq!(straight.groups[0].indices, vec![0, 1, 2, 0, 2, 3]);
        assert_eq!(flipped.groups[0].indices, vec![0, 2, 1, 0, 3, 2]);
    }

    #[test]
    fn one_double_sided_polygon_makes_its_whole_group_double_sided() {
        let mut model = quad_model();
        let mut second = model.polygons[0].clone();
        second.show_backface = true;
        model.polygons.push(second);

        let t = triangulate(&model, &AxisConvention::GAMEZ_DEFAULT);

        assert_eq!(t.groups.len(), 1);
        assert!(t.groups[0].show_backface);
    }

    #[test]
    fn colours_default_to_white_and_are_reported_as_absent() {
        let t = triangulate(&quad_model(), &AxisConvention::GAMEZ_DEFAULT);

        assert_eq!(t.colors, vec![[1.0, 1.0, 1.0, 1.0]; 4]);
        assert!(!t.has_vertex_colors());

        let mut coloured = quad_model();
        coloured.polygons[0].vertex_colors = vec![[1.0, 0.0, 0.0]; 4];
        assert!(triangulate(&coloured, &AxisConvention::GAMEZ_DEFAULT).has_vertex_colors());
    }

    #[test]
    fn the_collider_merges_every_group_and_counts_the_same_triangles() {
        let mut model = quad_model();
        let base = model.vertices.len() as u32;
        model.vertices.extend([
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
        ]);
        model.polygons.push(PolygonDesc {
            vertex_indices: vec![base, base + 1, base + 2],
            materials: vec![PolygonMaterialDesc {
                material_index: 8,
                uv_coords: None,
            }],
            ..Default::default()
        });

        let t = triangulate(&model, &AxisConvention::GAMEZ_DEFAULT);
        let (vertices, triangles) = collider_indices(ModelType::Default, &t);

        assert_eq!(t.groups.len(), 2);
        assert_eq!(
            triangles.len(),
            t.triangle_count(),
            "the collider gets every triangle the mesh does"
        );
        assert_eq!(triangles.len(), 3);
        assert_eq!(vertices.len(), t.positions.len());
        // And they index the same corners in the same order.
        assert_eq!(triangles[0], [0, 1, 2]);
        assert_eq!(triangles[2], [4, 5, 6]);
    }

    #[test]
    fn every_collider_triangle_knows_which_material_it_came_from() {
        // The alignment the per-triangle soil lookup depends on.
        let mut model = quad_model(); // material 3, one quad -> 2 triangles
        let base = model.vertices.len() as u32;
        model.vertices.extend([
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
        ]);
        model.polygons.push(PolygonDesc {
            vertex_indices: vec![base, base + 1, base + 2],
            materials: vec![PolygonMaterialDesc {
                material_index: 8,
                uv_coords: None,
            }],
            ..Default::default()
        });

        let t = triangulate(&model, &AxisConvention::GAMEZ_DEFAULT);
        let (_, triangles) = collider_indices(ModelType::Default, &t);
        let materials = material_per_triangle(ModelType::Default, &t);

        assert_eq!(materials.len(), triangles.len(), "one entry per triangle");
        assert_eq!(materials, vec![3, 3, 8], "group order, not polygon order");

        assert!(material_per_triangle(ModelType::Facade, &t).is_empty());
    }

    #[test]
    fn facades_and_points_models_produce_no_collider() {
        let t = triangulate(&quad_model(), &AxisConvention::GAMEZ_DEFAULT);

        for model_type in [ModelType::Facade, ModelType::Points] {
            let (vertices, triangles) = collider_indices(model_type, &t);
            assert!(vertices.is_empty(), "{model_type:?}");
            assert!(triangles.is_empty(), "{model_type:?}");
        }

        assert!(!collider_indices(ModelType::Default, &t).1.is_empty());
    }
}
