//! The inspector's first two tabs: the level's node tree, and its models and
//! materials.

use bevy::pbr::wireframe::WireframeConfig;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use bft_assets::catalog::LevelId;
use bft_assets::level::LevelData;
use bft_assets::textures::{TexturePool, TextureSet};
use bft_sim::ai::templates::Template;
use bft_sim::level::{ModelRef, NodeActive, NodeId, NodeName, NodeZone};
use bft_sim::physics::SurfaceInfo;

use super::inspect::flag_names;
use super::{DevPanel, SelectedNode, egui};
use crate::camera::FlyCamera;
use crate::level_view::{DrawCensus, LoadedLevel, level_draw_census};
use crate::materials::{MaterialCache, alpha_mode};
use crate::mesh_builder::{MeshCache, ModelPart};

/// How many search hits are listed. A one-letter search matches most of a
/// level, and egui lays out every row it is given.
const MAX_SEARCH_HITS: usize = 500;

/// The node tree, a name search, and the selected node's details.
#[derive(Default)]
pub struct LevelPanel {
    search: String,
    /// The selection the tree last opened itself to, so a selection made
    /// elsewhere (a click in the world) unfolds the tree once and then leaves
    /// the user's folding alone.
    revealed: Option<Entity>,
}

impl DevPanel for LevelPanel {
    fn title(&self) -> &str {
        "Level"
    }

    fn ui(&mut self, ui: &mut egui::Ui, world: &mut World) {
        let Some(root) = world.get_resource::<LoadedLevel>().and_then(|l| l.root) else {
            ui.label("No level loaded.");
            return;
        };
        let selected = world.resource::<SelectedNode>().0;

        ui.horizontal(|ui| {
            ui.label("Find");
            ui.text_edit_singleline(&mut self.search);
            if ui.button("Clear").clicked() {
                self.search.clear();
            }
        });
        ui.small("Ctrl+click in the world selects the node under the cursor.");

        // The ancestors of a new selection are opened once.
        let reveal: HashSet<Entity> = match selected {
            Some(entity) if self.revealed != selected => ancestors(world, entity),
            _ => HashSet::default(),
        };
        self.revealed = selected;

        let mut clicked = None;
        egui::ScrollArea::vertical()
            .id_salt("node_tree")
            .max_height(320.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let world: &World = world;
                if self.search.trim().is_empty() {
                    for child in children_of(world, root) {
                        tree_node(ui, world, child, selected, &reveal, &mut clicked);
                    }
                } else {
                    search_hits(ui, world, self.search.trim(), selected, &mut clicked);
                }
            });

        if let Some(entity) = clicked {
            world.resource_mut::<SelectedNode>().0 = Some(entity);
            // A click in the tree is already visible; do not unfold anything.
            self.revealed = Some(entity);
        }

        ui.separator();
        match world.resource::<SelectedNode>().0 {
            Some(entity) if world.get_entity(entity).is_ok() => details(ui, world, entity),
            _ => {
                ui.label("Select a node to see its details.");
            }
        }
    }
}

/// A node's label: its data name, else its `Name`, else the entity.
fn label_of(world: &World, entity: Entity) -> String {
    world
        .get::<NodeName>(entity)
        .map(|name| name.0.clone())
        .or_else(|| world.get::<Name>(entity).map(|name| name.to_string()))
        .unwrap_or_else(|| format!("{entity}"))
}

/// A node's children that are nodes, not its drawn mesh parts.
fn children_of(world: &World, entity: Entity) -> Vec<Entity> {
    world
        .get::<Children>(entity)
        .map(|children| {
            children
                .iter()
                .filter(|child| world.get::<ModelPart>(*child).is_none())
                .collect()
        })
        .unwrap_or_default()
}

/// Every ancestor of `entity`, for unfolding the tree down to it.
fn ancestors(world: &World, entity: Entity) -> HashSet<Entity> {
    let mut found = HashSet::default();
    let mut current = entity;

    while let Some(child_of) = world.get::<ChildOf>(current) {
        current = child_of.parent();
        found.insert(current);
    }

    found
}

/// One row of the tree, and its subtree under a fold when it has one.
fn tree_node(
    ui: &mut egui::Ui,
    world: &World,
    entity: Entity,
    selected: Option<Entity>,
    reveal: &HashSet<Entity>,
    clicked: &mut Option<Entity>,
) {
    let label = row_label(world, entity);
    let is_selected = selected == Some(entity);
    let children = children_of(world, entity);

    if children.is_empty() {
        let response = ui.selectable_label(is_selected, label);
        if response.clicked() {
            *clicked = Some(entity);
        }
        if is_selected && !reveal.is_empty() {
            response.scroll_to_me(Some(egui::Align::Center));
        }
        return;
    }

    let id = ui.make_persistent_id(entity);
    let mut state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    if reveal.contains(&entity) {
        state.set_open(true);
    }

    state
        .show_header(ui, |ui| {
            let response =
                ui.selectable_label(is_selected, format!("{label}  ({})", children.len()));
            if response.clicked() {
                *clicked = Some(entity);
            }
            if is_selected && !reveal.is_empty() {
                response.scroll_to_me(Some(egui::Align::Center));
            }
        })
        .body(|ui| {
            for child in children {
                tree_node(ui, world, child, selected, reveal, clicked);
            }
        });
}

/// The name plus the markers worth seeing without selecting: hidden,
/// prototype, has a model.
fn row_label(world: &World, entity: Entity) -> String {
    let mut label = label_of(world, entity);

    if world
        .get::<NodeActive>(entity)
        .is_some_and(|active| !active.0)
    {
        label.push_str("  [off]");
    }
    if world.get::<Template>(entity).is_some() {
        label.push_str("  [prototype]");
    }
    if let Some(model) = world.get::<ModelRef>(entity) {
        label.push_str(&format!("  #{}", model.0));
    }

    label
}

/// The flat list a search shows instead of the tree.
fn search_hits(
    ui: &mut egui::Ui,
    world: &World,
    search: &str,
    selected: Option<Entity>,
    clicked: &mut Option<Entity>,
) {
    let needle = search.to_lowercase();
    let mut hits: Vec<(Entity, &str)> = Vec::new();
    let mut total = 0usize;

    // `try_query` rather than `query`, because this has `&World`: the query
    // state for a component every level spawns always exists.
    let Some(mut query) = world.try_query::<(Entity, &NodeName)>() else {
        return;
    };
    for (entity, name) in query.iter(world) {
        if name.0.to_lowercase().contains(&needle) {
            total += 1;
            if hits.len() < MAX_SEARCH_HITS {
                hits.push((entity, name.0.as_str()));
            }
        }
    }
    hits.sort_by(|a, b| a.1.cmp(b.1));

    ui.label(format!("{total} match(es)"));
    for (entity, _) in hits {
        let path = path_of(world, entity);
        if ui
            .selectable_label(selected == Some(entity), path)
            .clicked()
        {
            *clicked = Some(entity);
        }
    }
    if total > MAX_SEARCH_HITS {
        ui.label(format!("… and {} more", total - MAX_SEARCH_HITS));
    }
}

/// `parent/child/node`, up to the level root, for telling repeated names
/// apart in a search.
fn path_of(world: &World, entity: Entity) -> String {
    let mut parts = vec![label_of(world, entity)];
    let mut current = entity;

    while let Some(child_of) = world.get::<ChildOf>(current) {
        current = child_of.parent();
        if world.get::<NodeName>(current).is_none() {
            break;
        }
        parts.push(label_of(world, current));
    }

    parts.reverse();
    parts.join("/")
}

/// Everything known about one node.
fn details(ui: &mut egui::Ui, world: &mut World, entity: Entity) {
    let level = world
        .get_resource::<LoadedLevel>()
        .and_then(|loaded| world.resource::<Assets<LevelData>>().get(&loaded.data))
        .cloned();
    let node = world
        .get::<NodeId>(entity)
        .and_then(|id| level.as_ref()?.node(id.0).cloned());

    ui.heading(label_of(world, entity));

    egui::Grid::new("node_details")
        .num_columns(2)
        .striped(true)
        .show(ui, |ui| {
            let row = |ui: &mut egui::Ui, key: &str, value: String| {
                ui.label(key);
                ui.label(value);
                ui.end_row();
            };

            row(ui, "entity", format!("{entity}"));
            if let Some(id) = world.get::<NodeId>(entity) {
                let kind = node.as_ref().map_or("?", |node| node.kind());
                row(ui, "node", format!("{} ({kind})", id.0));
            }
            if let Some(active) = world.get::<NodeActive>(entity) {
                row(ui, "active", active.0.to_string());
            }
            if let Some(zone) = world.get::<NodeZone>(entity) {
                row(ui, "zone", zone.0.to_string());
            }
            if world.get::<Template>(entity).is_some() {
                row(ui, "prototype", "yes: hidden, cloned by AI and anim".into());
            }
            if let Some(transform) = world.get::<Transform>(entity) {
                let t = transform.translation;
                let (y, x, z) = transform.rotation.to_euler(EulerRot::YXZ);
                row(ui, "local", format!("{:.2}, {:.2}, {:.2}", t.x, t.y, t.z));
                row(
                    ui,
                    "rotation",
                    format!(
                        "yaw {:.1}°  pitch {:.1}°  roll {:.1}°",
                        y.to_degrees(),
                        x.to_degrees(),
                        z.to_degrees()
                    ),
                );
                let s = transform.scale;
                if s != Vec3::ONE {
                    row(ui, "scale", format!("{:.3}, {:.3}, {:.3}", s.x, s.y, s.z));
                }
            }
            if let Some(global) = world.get::<GlobalTransform>(entity) {
                let t = global.translation();
                row(ui, "world", format!("{:.2}, {:.2}, {:.2}", t.x, t.y, t.z));
            }
            if let Some(node) = &node {
                row(ui, "flags", flag_names(&node.common().flags).join(" "));
            }
            if let Some(surface) = world.get::<SurfaceInfo>(entity) {
                row(
                    ui,
                    "soil",
                    format!(
                        "{:?} ({} triangles)",
                        surface.dominant,
                        surface.soil_by_triangle.len()
                    ),
                );
            }
            row(ui, "children", children_of(world, entity).len().to_string());
        });

    if let (Some(model_ref), Some(level)) = (world.get::<ModelRef>(entity).copied(), &level) {
        model_details(ui, world, level, model_ref.0);
    }

    ui.horizontal(|ui| {
        if world.query::<&FlyCamera>().iter(world).next().is_some() && ui.button("Fly to").clicked()
        {
            fly_to(world, entity);
        }
        if ui.button("Deselect").clicked() {
            world.resource_mut::<SelectedNode>().0 = None;
        }
    });
}

/// The model a node draws: its size and the materials it uses.
fn model_details(ui: &mut egui::Ui, world: &World, level: &LevelData, index: usize) {
    let Some(model) = level.models.get(index) else {
        ui.label(format!("model #{index} is past the model table"));
        return;
    };

    ui.label(format!(
        "model #{index}: {:?}, {} vertices, {} polygons",
        model.model_type,
        model.vertices.len(),
        model.polygons.len()
    ));

    let mut used: Vec<u32> = model
        .polygons
        .iter()
        .flat_map(|polygon| polygon.materials.iter().map(|m| m.material_index))
        .collect();
    used.sort_unstable();
    used.dedup();

    for material in used {
        ui.label(format!(
            "  material {material}: {}",
            material_summary(world, level, material as usize)
        ));
    }
}

/// A material's texture, soil and the alpha mode its texture gives it.
fn material_summary(world: &World, level: &LevelData, index: usize) -> String {
    let Some(desc) = level.materials.get(index) else {
        return "missing".into();
    };

    let texture = desc.texture();
    let alpha = texture.and_then(|name| {
        let pool = world.get_resource::<TexturePool>()?;
        let sets = world.get_resource::<Assets<TextureSet>>()?;
        pool.resolve(name, sets).map(|(_, info)| info.alpha)
    });

    format!(
        "{}  {}  soil {:?}",
        texture.unwrap_or("(colour)"),
        alpha.map_or("-".to_string(), |alpha| format!("{:?}", alpha_mode(alpha))),
        desc.soil()
    )
}

/// Put the viewer's camera where it can see the node.
fn fly_to(world: &mut World, entity: Entity) {
    let target = world
        .get::<GlobalTransform>(entity)
        .map(GlobalTransform::translation)
        .unwrap_or_default();
    let eye = target + Vec3::new(0.0, 20.0, 40.0);

    let mut cameras = world.query::<(&mut FlyCamera, &mut Transform)>();
    if let Some((mut fly, mut transform)) = cameras.iter_mut(world).next() {
        fly.place(&mut transform, eye, target);
    }
}

/// Counts for the level's models, meshes and materials, the material table,
/// and the wireframe switch.
#[derive(Default)]
pub struct ModelsPanel {
    filter: String,
    /// `level_draw_census` triangulates every model, so it runs on request
    /// and is kept per level.
    census: Option<(LevelId, DrawCensus)>,
}

impl DevPanel for ModelsPanel {
    fn title(&self) -> &str {
        "Models"
    }

    fn ui(&mut self, ui: &mut egui::Ui, world: &mut World) {
        if let Some(mut config) = world.get_resource_mut::<WireframeConfig>() {
            let mut global = config.global;
            ui.checkbox(&mut global, "Wireframe");
            if global != config.global {
                config.global = global;
            }
        }

        ui.label(format!(
            "built: {} models, {} materials   assets: {} meshes, {} materials, {} images",
            world.get_resource::<MeshCache>().map_or(0, MeshCache::len),
            world
                .get_resource::<MaterialCache>()
                .map_or(0, MaterialCache::len),
            world.resource::<Assets<Mesh>>().len(),
            world.resource::<Assets<StandardMaterial>>().len()
                + world
                    .get_resource::<Assets<crate::cutaway::ModelMaterial>>()
                    .map_or(0, Assets::len),
            world.resource::<Assets<Image>>().len(),
        ));

        let Some(loaded) = world.get_resource::<LoadedLevel>() else {
            ui.label("No level loaded.");
            return;
        };
        let id = loaded.id;
        let Some(level) = world.resource::<Assets<LevelData>>().get(&loaded.data) else {
            ui.label("The level's data has not loaded.");
            return;
        };

        let census = level.census();
        ui.label(format!(
            "{} nodes ({} object3d, {} lod, {} empty, {} other), {} models, {} materials, {} texture names",
            level.nodes.len(),
            census.object3d,
            census.lod,
            census.empty,
            census.world + census.light + census.camera + census.display + census.window,
            level.models.len(),
            level.materials.len(),
            level.texture_names.len(),
        ));

        match self.census {
            Some((census_id, census)) if census_id == id => {
                ui.label(format!(
                    "draws {} model references to {} distinct models, {} triangles; {} empty",
                    census.model_refs,
                    census.distinct_models,
                    census.triangles,
                    census.empty_models
                ));
            }
            _ => {
                if ui.button("Count triangles").clicked() {
                    let axes = world
                        .get_resource::<bft_assets::axes::AxisConvention>()
                        .copied()
                        .unwrap_or_default();
                    self.census = Some((id, level_draw_census(level, &axes)));
                }
            }
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.text_edit_singleline(&mut self.filter);
        });

        let needle = self.filter.trim().to_lowercase();
        let rows: Vec<(usize, String)> = level
            .materials
            .iter()
            .enumerate()
            .map(|(i, _)| (i, material_summary(world, level, i)))
            .filter(|(_, summary)| needle.is_empty() || summary.to_lowercase().contains(&needle))
            .collect();

        let row_height = ui.text_style_height(&egui::TextStyle::Body);
        egui::ScrollArea::vertical()
            .id_salt("materials")
            .max_height(320.0)
            .auto_shrink([false, true])
            .show_rows(ui, row_height, rows.len(), |ui, range| {
                for (index, summary) in &rows[range] {
                    ui.label(format!("{index:4}  {summary}"));
                }
            });
    }
}
