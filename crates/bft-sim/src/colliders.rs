//! `ModelDesc` → avian3d trimesh colliders, without a renderer.
//!
//! The collider is built from the *same* [`bft_assets::geometry::triangulate`]
//! output the mesh builder draws, so collision can never drift from what the
//! player sees (DESIGN §7.1). It deliberately does not go through
//! `Collider::trimesh_from_mesh`: that needs a `Mesh`, which needs
//! `bevy_render`, which a dedicated server does not have (NFR-4).
//!
//! # What this module does not decide
//!
//! *Which* nodes get a collider is the level spawner's call, from the
//! `altitude_surface` / `intersect_surface` node flags (docs/data/nodes.md) — not
//! from the model. This module answers only "given this model, what is its
//! collision shape", and returns `None` when the model has no surface.

use avian3d::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::geometry::{Triangulated, collider_indices, triangulate};
use bft_assets::models::{ModelDesc, ModelType};

/// Build a static trimesh collider for one model.
///
/// `None` when the model contributes no collision geometry:
///
/// - [`ModelType::Points`] and [`ModelType::Facade`] models by rule (a
///   billboard's orientation changes every frame, so a baked collider would
///   be wrong as soon as the camera moved);
/// - models whose polygons all degenerate, which no Recoil level has.
pub fn build_collider(model: &ModelDesc, axes: &AxisConvention) -> Option<Collider> {
    collider_from(model.model_type, &triangulate(model, axes))
}

/// The same, when the caller already has a triangulation (the viewer builds
/// meshes and colliders from one pass).
pub fn collider_from(model_type: ModelType, triangulated: &Triangulated) -> Option<Collider> {
    let (vertices, indices) = collider_indices(model_type, triangulated);

    if indices.is_empty() {
        return None;
    }

    Some(Collider::trimesh(vertices, indices))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::*;
    use bft_assets::models::{PolygonDesc, PolygonMaterialDesc};

    /// A unit cube, so the collider has a volume worth asserting on.
    fn cube() -> ModelDesc {
        let corners = [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(1.0, 0.0, 1.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(0.0, 1.0, 1.0),
        ];
        // Each face wound counter-clockwise seen from outside, as the real
        // data is (docs/data/models.md).
        let faces = [
            [0, 3, 2, 1],
            [4, 5, 6, 7],
            [0, 1, 5, 4],
            [2, 3, 7, 6],
            [1, 2, 6, 5],
            [0, 4, 7, 3],
        ];

        ModelDesc {
            vertices: corners.to_vec(),
            polygons: faces
                .iter()
                .map(|face| PolygonDesc {
                    vertex_indices: face.to_vec(),
                    materials: vec![PolygonMaterialDesc {
                        material_index: 0,
                        uv_coords: None,
                    }],
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_closed_model_becomes_a_trimesh_of_the_same_triangles() {
        let model = cube();
        let axes = AxisConvention::GAMEZ_DEFAULT;

        let collider = build_collider(&model, &axes).expect("a collider");
        let triangulated = triangulate(&model, &axes);

        assert_eq!(
            collider
                .shape()
                .as_trimesh()
                .expect("a trimesh")
                .num_triangles(),
            triangulated.triangle_count(),
            "the collider gets every triangle the mesh does"
        );
        assert_eq!(triangulated.triangle_count(), 12, "six quads, two each");

        // And it is where the model is: the cube's AABB is its unit extent.
        let aabb = collider.aabb(Vec3::ZERO, Quat::IDENTITY);
        assert!((aabb.min - Vec3::ZERO).length() < 1e-5, "{:?}", aabb.min);
        assert!((aabb.max - Vec3::ONE).length() < 1e-5, "{:?}", aabb.max);
    }

    #[test]
    fn facades_and_points_models_get_no_collider() {
        for model_type in [ModelType::Facade, ModelType::Points] {
            let model = ModelDesc {
                model_type,
                ..cube()
            };

            assert!(
                build_collider(&model, &AxisConvention::GAMEZ_DEFAULT).is_none(),
                "{model_type:?} should not collide"
            );
        }
    }

    #[test]
    fn a_model_with_no_surface_gets_no_collider_rather_than_an_empty_one() {
        // An empty trimesh is a shape avian would accept and then never hit;
        // `None` lets the spawner skip the component entirely.
        assert!(build_collider(&ModelDesc::default(), &AxisConvention::GAMEZ_DEFAULT).is_none());
    }

    #[test]
    fn the_axis_convention_moves_the_collider_with_the_geometry() {
        // The point of sharing one `AxisConvention`: a collider built under a
        // different convention than the mesh would sit somewhere else.
        let mirrored = AxisConvention::parse("x,y,-z").expect("a permutation");

        let collider = build_collider(&cube(), &mirrored).expect("a collider");
        let aabb = collider.aabb(Vec3::ZERO, Quat::IDENTITY);

        assert!(
            (aabb.min - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-5,
            "{:?}",
            aabb.min
        );
        assert!(
            (aabb.max - Vec3::new(1.0, 1.0, 0.0)).length() < 1e-5,
            "{:?}",
            aabb.max
        );
    }
}
