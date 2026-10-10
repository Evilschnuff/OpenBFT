//! Craters, drawn: a cratered node's meshes rebuilt from its cut triangles.
//!
//! `bft-sim`'s [`form_craters`](bft_sim::crater::form_craters) cuts a node's
//! triangles and hands the result to its collider; this draws the same
//! triangles, so the hole on screen is the hole the vehicle drives into. The
//! ground outside the rim keeps its own material, and each bowl is drawn with
//! the crater texture `declient.zrd` chose for the ground it replaced.
//!
//! Also where a mission's [`CraterConfig`] is loaded, on entering the level,
//! the way [`play`](crate::play) places the pickups, and where
//! `tuning/craters.ron` ([`CraterOverrides`]) adds ground the level's flags
//! leave out.

use bevy::prelude::*;
use bft_assets::level::LevelData;
use bft_assets::materials::MaterialDesc;
use bft_assets::reader::ReaderData;
use bft_assets::textures::{TexturePool, TextureSet};
use std::sync::Arc;

use avian3d::prelude::Collider;
use bft_assets::axes::AxisConvention;
use bft_sim::crater::{CraterConfig, CraterOverrides, Cratered, GroundSource, LevelSurfaces};
use bft_sim::level::{ModelRef, NodeIndex};
use bft_sim::states::AppState;

use crate::level_view::LoadedLevel;
use crate::materials::{MaterialCache, MaterialParams, build_material};
use crate::mesh_builder::{ModelPart, build_parts};

/// Loads the crater settings and redraws cratered nodes.
pub struct CraterViewPlugin;

impl Plugin for CraterViewPlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<CraterOverrides>()
            .register_asset_loader(bft_assets::RonAssetLoader::<CraterOverrides>::new(&[
                "craters.ron",
            ]))
            .add_systems(Startup, request_crater_overrides)
            .add_systems(OnEnter(AppState::InGame), load_crater_config)
            .add_systems(
                Update,
                (apply_crater_overrides, redraw_cratered_nodes).run_if(in_state(AppState::InGame)),
            );
    }
}

/// Where `assets/tuning/craters.ron` lives, as the asset server names it.
pub const CRATER_TUNING: &str = "tuning/craters.ron";

/// The loaded `craters.ron`.
#[derive(Resource, Debug, Clone)]
struct CraterOverridesHandle(Handle<CraterOverrides>);

fn request_crater_overrides(mut commands: Commands, server: Res<AssetServer>) {
    commands.insert_resource(CraterOverridesHandle(server.load(CRATER_TUNING)));
}

/// Make the nodes `craters.ron` names for this level deformable ground, once
/// the level has spawned and the file has loaded.
#[allow(clippy::too_many_arguments)]
fn apply_crater_overrides(
    mut commands: Commands,
    mut applied_to: Local<Option<AssetId<LevelData>>>,
    handle: Option<Res<CraterOverridesHandle>>,
    overrides: Res<Assets<CraterOverrides>>,
    loaded: Option<Res<LoadedLevel>>,
    levels: Res<Assets<LevelData>>,
    nodes: Option<Res<NodeIndex>>,
    axes: Option<Res<AxisConvention>>,
    candidates: Query<&ModelRef, (With<Collider>, Without<GroundSource>)>,
) {
    let (Some(handle), Some(loaded), Some(nodes)) = (handle, loaded, nodes) else {
        return;
    };
    if *applied_to == Some(loaded.data.id()) || !loaded.is_spawned() {
        return;
    }
    let (Some(overrides), Some(level)) = (overrides.get(&handle.0), levels.get(&loaded.data))
    else {
        return;
    };
    *applied_to = Some(loaded.data.id());

    let Some(names) = overrides.extra_ground.get(&loaded.id.dir_name()) else {
        return;
    };
    let axes = axes.map(|axes| *axes).unwrap_or_default();
    let surfaces = Arc::new(LevelSurfaces::from_materials(&level.materials));
    let mut added = 0;

    for name in names {
        let entities = nodes.by_name(name);
        if entities.is_empty() {
            warn!(
                "{CRATER_TUNING}: {} has no node `{name}`",
                loaded.id.dir_name()
            );
        }
        for &entity in entities {
            let Ok(ModelRef(model_index)) = candidates.get(entity) else {
                continue;
            };
            let Some(model) = level.models.get(*model_index) else {
                continue;
            };
            commands
                .entity(entity)
                .insert(GroundSource::for_model(model, &axes, surfaces.clone()));
            added += 1;
        }
    }
    info!("{CRATER_TUNING}: {added} extra node(s) take craters");
}

/// This mission's `declient.zrd`, or `CraterConfig`'s defaults when it has none.
fn load_crater_config(
    mut commands: Commands,
    loaded: Option<Res<LoadedLevel>>,
    archives: Res<Assets<ReaderData>>,
) {
    let config = loaded
        .as_ref()
        .and_then(|loaded| loaded.reader.as_ref())
        .and_then(|handle| archives.get(handle))
        .and_then(CraterConfig::from_reader);

    match config {
        Some(config) => commands.insert_resource(config),
        None => {
            warn!("this mission has no `declient.zrd` crater block; craters use the defaults");
            commands.insert_resource(CraterConfig::default());
        }
    }
}

/// Swap a cratered node's mesh children for ones built from its cut.
#[allow(clippy::too_many_arguments)]
fn redraw_cratered_nodes(
    mut commands: Commands,
    cratered: Query<(Entity, &Cratered, &ModelRef, Option<&Children>), Changed<Cratered>>,
    parts: Query<(), With<ModelPart>>,
    loaded: Option<Res<LoadedLevel>>,
    levels: Res<Assets<LevelData>>,
    sets: Res<Assets<TextureSet>>,
    pool: Res<TexturePool>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut material_cache: ResMut<MaterialCache>,
) {
    if cratered.is_empty() {
        return;
    }
    let Some(loaded) = loaded else {
        return;
    };
    let Some(level) = levels.get(&loaded.data) else {
        return;
    };
    let empty_set = TextureSet::default();
    let level_set = loaded
        .textures
        .first()
        .and_then(|handle| sets.get(handle))
        .unwrap_or(&empty_set);

    for (entity, cratered, ModelRef(model_index), children) in &cratered {
        let Some(model) = level.models.get(*model_index) else {
            continue;
        };

        for child in children.into_iter().flat_map(|children| children.iter()) {
            if parts.contains(child) {
                commands.entity(child).despawn();
            }
        }

        let cut = &cratered.cut;
        for part in build_parts(&cut.triangulated, &mut meshes) {
            let Some(source) = level.materials.get(part.material_index as usize) else {
                continue;
            };
            // A bowl is drawn with its crater's texture, on the soil of the
            // ground it was cut from; a crater with no texture keeps the
            // ground's own.
            let crater_texture = cut
                .crater_of_group
                .get(part.group)
                .copied()
                .flatten()
                .and_then(|crater| cratered.craters.get(crater))
                .and_then(|crater| crater.texture.clone());
            let bowl = crater_texture.map(|texture| MaterialDesc::Textured {
                texture,
                soil: source.soil(),
                cycle: None,
                flag: false,
            });

            let material = build_material(
                MaterialParams {
                    desc: bowl.as_ref().unwrap_or(source),
                    lighting: model.flags.lighting,
                    fog: model.flags.fog,
                    show_backface: part.show_backface,
                    texture_scroll: None,
                },
                level_set,
                Some((&pool, &sets)),
                &mut material_cache,
                &mut materials,
            );

            commands.spawn((
                Mesh3d(part.mesh),
                MeshMaterial3d(material),
                ModelPart,
                ChildOf(entity),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped file parses, and names m1's pier road.
    #[test]
    fn the_shipped_crater_overrides_parse() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets")
            .join(CRATER_TUNING);
        let text = std::fs::read_to_string(&path).expect("the file is in the repo");
        let parsed: CraterOverrides = ron::de::from_str(&text).expect("it parses");

        assert_eq!(parsed.extra_ground["m1"], vec!["g943".to_string()]);
    }
}
