//! `ModelDesc` → Bevy `Mesh`es, one per material group.
//!
//! The triangulation itself is [`bft_assets::geometry`], which knows nothing
//! about `bevy_render`; this module only turns its output into `Mesh`
//! assets. `bft-sim` calls the same `triangulate` for colliders, so geometry
//! and collision can never drift apart (DESIGN §7.1).
//!
//! # What the level spawner calls
//!
//! [`build_model`] once per model a node references, then
//! [`bft_game::materials::build_material`](crate::materials::build_material)
//! once per returned part, spawning one child entity per part (DESIGN §6:
//! "a node with a multi-material model spawns one child entity per material
//! group").

use bevy::asset::{RenderAssetUsages, UntypedAssetId};
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::geometry::{Triangulated, triangulate};
use bft_assets::models::{FacadeMode, ModelDesc, ModelType};

/// One model's meshes, one per material group.
#[derive(Debug, Clone, Default)]
pub struct BuiltModel {
    /// A mesh and the material index it is drawn with, in the triangulation's
    /// first-use order.
    pub parts: Vec<BuiltPart>,
    /// `Some` for a [`ModelType::Facade`] model, which the spawner turns into
    /// a [`Facade`] component.
    pub facade: Option<FacadeMode>,
    /// The model's bounding-box centre, the pivot a centroid facade turns
    /// about.
    pub bbox_mid: Vec3,
}

/// One material group's mesh.
#[derive(Debug, Clone)]
pub struct BuiltPart {
    pub mesh: Handle<Mesh>,
    /// Which of the triangulation's groups it is.
    pub group: usize,
    /// Index into `LevelData::materials`, for `build_material`.
    pub material_index: u32,
    /// Whether this group needs backface culling off. It is a *material*
    /// property in Bevy, so the spawner passes it to
    /// [`MaterialParams`](crate::materials::MaterialParams) rather than
    /// setting it on the mesh.
    pub show_backface: bool,
}

/// Built meshes, keyed by level and model index.
///
/// A level instances its models heavily — m1 references 1350 models from 2170
/// nodes — so building each one once is the difference between 1350 meshes
/// and 2170.
///
/// The key's level part is an [`UntypedAssetId`] rather than
/// `AssetId<LevelData>` so that this module does not depend on the level
/// loader's type. The spawner passes
/// `level_handle.id().untyped()`.
#[derive(Resource, Debug, Default)]
pub struct MeshCache {
    entries: HashMap<(UntypedAssetId, usize), BuiltModel>,
}

impl MeshCache {
    /// How many models have been built.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Forget every entry, dropping the mesh handles with them. The level
    /// loader calls this when a level is unloaded, as it does `MaterialCache::clear`.
    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// Forget one level's entries, leaving any other level's alone.
    pub fn clear_level(&mut self, level: UntypedAssetId) {
        self.entries.retain(|(id, _), _| *id != level);
    }
}

/// A mesh child that draws part of its parent node's model, one per
/// material group: what a crater replaces when it recuts the node.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct ModelPart;

/// A billboard that turns to face the camera.
#[derive(Component, Debug, Clone, Copy)]
pub struct Facade {
    pub mode: FacadeMode,
    /// Turn about the model's `bbox_mid` rather than its origin.
    ///
    /// False on every Recoil model — `facade_centroid` is never set
    /// (docs/data/models.md) — so this path is untested against real data.
    pub centroid: bool,
}

/// The pivot a centroid facade turns about, in the entity's local space.
#[derive(Component, Debug, Clone, Copy)]
pub struct FacadePivot(pub Vec3);

/// Build (or reuse) one model's meshes.
///
/// `level` identifies the level the model belongs to, for the cache key.
pub fn build_model(
    level: UntypedAssetId,
    model_index: usize,
    model: &ModelDesc,
    axes: &AxisConvention,
    cache: &mut MeshCache,
    meshes: &mut Assets<Mesh>,
) -> BuiltModel {
    if let Some(built) = cache.entries.get(&(level, model_index)) {
        return built.clone();
    }

    let triangulated = triangulate(model, axes);

    if triangulated.skipped_polygons > 0 {
        warn!(
            "model {model_index}: {} polygons index past the vertex table and were dropped",
            triangulated.skipped_polygons
        );
    }

    let built = BuiltModel {
        parts: build_parts(&triangulated, meshes),
        facade: matches!(model.model_type, ModelType::Facade).then_some(model.facade_mode),
        bbox_mid: axes.point(model.bbox_mid),
    };

    cache.entries.insert((level, model_index), built.clone());

    built
}

/// One `Mesh` per non-empty triangle group.
pub fn build_parts(triangulated: &Triangulated, meshes: &mut Assets<Mesh>) -> Vec<BuiltPart> {
    // Vertex colours are per corner and shared across groups, so this is a
    // whole-model decision: no Recoil polygon has any, so no mesh gets the
    // attribute.
    let colored = triangulated.has_vertex_colors();

    triangulated
        .groups
        .iter()
        .enumerate()
        .filter(|(_, group)| !group.indices.is_empty())
        .map(|(index, group)| {
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                // The sim reads geometry from `ModelDesc`, never from a
                // `Mesh`, so the CPU copy would only be dead memory.
                RenderAssetUsages::RENDER_WORLD,
            );

            mesh.insert_attribute(
                Mesh::ATTRIBUTE_POSITION,
                triangulated
                    .positions
                    .iter()
                    .map(Vec3::to_array)
                    .collect::<Vec<_>>(),
            );
            mesh.insert_attribute(
                Mesh::ATTRIBUTE_NORMAL,
                triangulated
                    .normals
                    .iter()
                    .map(Vec3::to_array)
                    .collect::<Vec<_>>(),
            );
            mesh.insert_attribute(
                Mesh::ATTRIBUTE_UV_0,
                triangulated
                    .uvs
                    .iter()
                    .map(Vec2::to_array)
                    .collect::<Vec<_>>(),
            );
            if colored {
                mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, triangulated.colors.clone());
            }

            mesh.insert_indices(Indices::U32(group.indices.clone()));

            BuiltPart {
                mesh: meshes.add(mesh),
                group: index,
                material_index: group.material_index,
                show_backface: group.show_backface,
            }
        })
        .collect()
}

/// The facade system and the mesh cache.
pub struct MeshBuilderPlugin;

impl Plugin for MeshBuilderPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MeshCache>()
            // `PostUpdate` before transform propagation, so a facade's new
            // rotation reaches its global transform in the same frame rather
            // than a frame late.
            .add_systems(
                PostUpdate,
                face_the_camera.before(TransformSystems::Propagate),
            );
    }
}

/// Turn every facade toward the active camera.
///
/// Every Recoil facade is `CylindricalY` — a tree or a smoke sprite standing
/// upright and spinning about y to keep its face to the viewer. The other
/// three modes are implemented from the format's semantics; no Recoil level
/// exercises them.
pub fn face_the_camera(
    camera: Query<&GlobalTransform, (Without<Facade>, crate::ui::hud::sensor::MainCamera3d)>,
    mut facades: Query<(
        &Facade,
        &GlobalTransform,
        &mut Transform,
        Option<&FacadePivot>,
    )>,
) {
    // The active camera: the first enabled 3d one. A viewer with a split
    // screen would need to pick per view, which billboards cannot express in
    // a transform anyway.
    let Some(eye) = camera.iter().next().map(|t| t.translation()) else {
        return;
    };

    for (facade, global, mut transform, pivot) in &mut facades {
        // Work in the parent's space: `transform` is relative to the parent,
        // and `global` is where the entity actually is.
        let to_eye = eye - global.translation();

        let Some(rotation) = facing_rotation(facade.mode, to_eye) else {
            continue;
        };

        // The parent's rotation has to come out, or a rotated parent turns
        // the billboard with it.
        let parent_rotation = global.rotation() * transform.rotation.inverse();
        let local = parent_rotation.inverse() * rotation;

        if facade.centroid
            && let Some(FacadePivot(pivot)) = pivot
        {
            // Turning about the centroid rather than the origin moves the
            // entity too: the pivot must stay where it was.
            let before = transform.rotation * *pivot;
            let after = local * *pivot;
            transform.translation += before - after;
        }

        transform.rotation = local;
    }
}

/// The world-space rotation that points a facade's +z at the viewer.
///
/// `None` when the direction is degenerate (the camera sits exactly on the
/// billboard), in which case the previous rotation is kept.
fn facing_rotation(mode: FacadeMode, to_eye: Vec3) -> Option<Quat> {
    match mode {
        // Yaw only: the billboard stays upright.
        FacadeMode::CylindricalY => {
            let flat = Vec3::new(to_eye.x, 0.0, to_eye.z).try_normalize()?;
            Some(Quat::from_rotation_arc(Vec3::Z, flat))
        }
        // Full look-at, up-vector chosen to avoid a singularity overhead.
        FacadeMode::SphericalY => {
            let direction = to_eye.try_normalize()?;
            let up = if direction.y.abs() > 0.999 {
                Vec3::Z
            } else {
                Vec3::Y
            };
            Some(Quat::from_mat3(&Mat3::from_cols(
                up.cross(direction).normalize(),
                direction.cross(up.cross(direction).normalize()),
                direction,
            )))
        }
        // Spin about x or z instead, the same idea with the free axis moved.
        FacadeMode::CylindricalX => {
            let flat = Vec3::new(0.0, to_eye.y, to_eye.z).try_normalize()?;
            Some(Quat::from_rotation_arc(Vec3::Z, flat))
        }
        FacadeMode::CylindricalZ => {
            let flat = Vec3::new(to_eye.x, to_eye.y, 0.0).try_normalize()?;
            Some(Quat::from_rotation_arc(Vec3::Y, flat))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::models::{PolygonDesc, PolygonMaterialDesc};

    fn quad_model(material_index: u32) -> ModelDesc {
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
                    material_index,
                    uv_coords: None,
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn level_id() -> UntypedAssetId {
        AssetId::<Mesh>::invalid().untyped()
    }

    #[test]
    fn a_model_builds_one_mesh_per_material_group() {
        let mut model = quad_model(3);
        let base = model.vertices.len() as u32;
        model.vertices.extend([
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
        ]);
        model.polygons.push(PolygonDesc {
            vertex_indices: vec![base, base + 1, base + 2],
            show_backface: true,
            materials: vec![PolygonMaterialDesc {
                material_index: 8,
                uv_coords: None,
            }],
            ..Default::default()
        });

        let mut meshes = Assets::<Mesh>::default();
        let mut cache = MeshCache::default();

        let built = build_model(
            level_id(),
            0,
            &model,
            &AxisConvention::GAMEZ_DEFAULT,
            &mut cache,
            &mut meshes,
        );

        assert_eq!(built.parts.len(), 2);
        assert_eq!(built.parts[0].material_index, 3);
        assert!(!built.parts[0].show_backface);
        assert_eq!(built.parts[1].material_index, 8);
        assert!(built.parts[1].show_backface);
        assert!(built.facade.is_none());

        let mesh = meshes.get(&built.parts[0].mesh).expect("a mesh");
        assert_eq!(mesh.primitive_topology(), PrimitiveTopology::TriangleList);
        assert_eq!(
            mesh.indices().expect("indexed").len(),
            6,
            "a quad fans to 2"
        );
        // Every group carries the whole model's attribute arrays and indexes
        // into them, so the counts match the triangulation, not the group.
        assert_eq!(mesh.count_vertices(), 7);
        assert!(mesh.attribute(Mesh::ATTRIBUTE_NORMAL).is_some());
        assert!(mesh.attribute(Mesh::ATTRIBUTE_UV_0).is_some());
    }

    #[test]
    fn the_colour_attribute_is_added_only_when_a_colour_is_not_white() {
        let mut meshes = Assets::<Mesh>::default();
        let mut cache = MeshCache::default();

        let plain = build_model(
            level_id(),
            0,
            &quad_model(0),
            &AxisConvention::GAMEZ_DEFAULT,
            &mut cache,
            &mut meshes,
        );
        assert!(
            meshes
                .get(&plain.parts[0].mesh)
                .expect("a mesh")
                .attribute(Mesh::ATTRIBUTE_COLOR)
                .is_none(),
            "no Recoil polygon has vertex colours, so no mesh should carry the attribute"
        );

        let mut coloured = quad_model(0);
        coloured.polygons[0].vertex_colors = vec![[1.0, 0.0, 0.0]; 4];
        let built = build_model(
            level_id(),
            1,
            &coloured,
            &AxisConvention::GAMEZ_DEFAULT,
            &mut cache,
            &mut meshes,
        );
        assert!(
            meshes
                .get(&built.parts[0].mesh)
                .expect("a mesh")
                .attribute(Mesh::ATTRIBUTE_COLOR)
                .is_some()
        );
    }

    #[test]
    fn a_built_mesh_keeps_its_indices_readable_on_the_cpu() {
        // `RenderAssetUsages::RENDER_WORLD` means the data is not *retained*
        // in the main world after upload, but it is present on the asset
        // until then — which is what lets a tool (the `model_viewer`
        // example's `--unwound`) inspect or rewrite the winding. If this ever
        // stops holding, that flag silently becomes a no-op.
        let mut meshes = Assets::<Mesh>::default();
        let mut cache = MeshCache::default();

        let built = build_model(
            level_id(),
            0,
            &quad_model(0),
            &AxisConvention::GAMEZ_DEFAULT,
            &mut cache,
            &mut meshes,
        );

        let mesh = meshes.get(&built.parts[0].mesh).expect("a mesh");
        let indices: Vec<u32> = match mesh.indices().expect("indexed") {
            bevy::mesh::Indices::U32(indices) => indices.clone(),
            bevy::mesh::Indices::U16(indices) => indices.iter().map(|i| u32::from(*i)).collect(),
        };

        assert_eq!(indices, vec![0, 1, 2, 0, 2, 3]);
    }

    #[test]
    fn a_second_request_for_the_same_model_reuses_the_meshes() {
        let mut meshes = Assets::<Mesh>::default();
        let mut cache = MeshCache::default();
        let model = quad_model(0);

        let first = build_model(
            level_id(),
            7,
            &model,
            &AxisConvention::GAMEZ_DEFAULT,
            &mut cache,
            &mut meshes,
        );
        let again = build_model(
            level_id(),
            7,
            &model,
            &AxisConvention::GAMEZ_DEFAULT,
            &mut cache,
            &mut meshes,
        );

        assert_eq!(first.parts[0].mesh, again.parts[0].mesh);
        assert_eq!(meshes.len(), 1, "built once, handed out twice");
        assert_eq!(cache.len(), 1);

        // A different model index in the same level is a different entry.
        build_model(
            level_id(),
            8,
            &model,
            &AxisConvention::GAMEZ_DEFAULT,
            &mut cache,
            &mut meshes,
        );
        assert_eq!(cache.len(), 2);
        assert_eq!(meshes.len(), 2);

        cache.clear();
        assert!(cache.is_empty());
    }

    #[test]
    fn a_facade_model_reports_its_mode() {
        let mut model = quad_model(0);
        model.model_type = ModelType::Facade;
        model.facade_mode = FacadeMode::CylindricalY;
        model.bbox_mid = Vec3::new(1.0, 2.0, 3.0);

        let built = build_model(
            level_id(),
            0,
            &model,
            &AxisConvention::GAMEZ_DEFAULT,
            &mut cache_and_meshes().0,
            &mut cache_and_meshes().1,
        );

        assert_eq!(built.facade, Some(FacadeMode::CylindricalY));
        assert_eq!(built.bbox_mid, Vec3::new(1.0, 2.0, 3.0));
    }

    fn cache_and_meshes() -> (MeshCache, Assets<Mesh>) {
        (MeshCache::default(), Assets::<Mesh>::default())
    }

    #[test]
    fn a_model_with_no_polygons_builds_no_parts_rather_than_an_empty_mesh() {
        let mut meshes = Assets::<Mesh>::default();
        let mut cache = MeshCache::default();

        let built = build_model(
            level_id(),
            0,
            &ModelDesc::default(),
            &AxisConvention::GAMEZ_DEFAULT,
            &mut cache,
            &mut meshes,
        );

        assert!(built.parts.is_empty());
        assert_eq!(meshes.len(), 0);
    }

    #[test]
    fn a_cylindrical_y_facade_yaws_toward_the_camera_and_stays_upright() {
        // Camera off to the +x side and well above: the billboard should turn
        // to face +x and not tip back.
        let rotation = facing_rotation(FacadeMode::CylindricalY, Vec3::new(10.0, 50.0, 0.0))
            .expect("a direction");

        let facing = rotation * Vec3::Z;
        assert!((facing - Vec3::X).length() < 1e-5, "{facing:?}");
        assert!(
            (rotation * Vec3::Y - Vec3::Y).length() < 1e-5,
            "stays upright"
        );
    }

    #[test]
    fn a_spherical_facade_tips_to_face_a_camera_overhead() {
        let rotation = facing_rotation(FacadeMode::SphericalY, Vec3::new(0.0, 10.0, 0.0))
            .expect("a direction");

        let facing = rotation * Vec3::Z;
        assert!((facing - Vec3::Y).length() < 1e-4, "{facing:?}");
    }

    #[test]
    fn a_camera_exactly_on_the_billboard_keeps_the_previous_rotation() {
        assert!(facing_rotation(FacadeMode::CylindricalY, Vec3::ZERO).is_none());
        assert!(facing_rotation(FacadeMode::SphericalY, Vec3::ZERO).is_none());
        // And a camera directly overhead is degenerate for a Y cylinder,
        // which has no yaw that faces it.
        assert!(facing_rotation(FacadeMode::CylindricalY, Vec3::Y).is_none());
    }

    #[test]
    fn the_facade_system_turns_a_billboard_toward_the_camera() {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::transform::TransformPlugin,
        ));
        app.add_systems(
            PostUpdate,
            face_the_camera.before(TransformSystems::Propagate),
        );

        app.world_mut().spawn((
            Camera3d::default(),
            Transform::from_xyz(10.0, 0.0, 0.0),
            GlobalTransform::from_xyz(10.0, 0.0, 0.0),
        ));
        let facade = app
            .world_mut()
            .spawn((
                Facade {
                    mode: FacadeMode::CylindricalY,
                    centroid: false,
                },
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();

        app.update();

        let transform = app.world().get::<Transform>(facade).expect("a transform");
        let facing = transform.rotation * Vec3::Z;
        assert!((facing - Vec3::X).length() < 1e-5, "{facing:?}");
    }
}
