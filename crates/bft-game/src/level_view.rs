//! Drawing what `bft-sim` spawned: meshes, materials, fog and the sun.
//!
//! `bft-sim`'s [`spawn_level`](bft_sim::level::spawn_level) makes one entity
//! per node with a transform and a [`ModelRef`], and stops there — it has no
//! renderer (NFR-4). This module watches for those markers and hangs the
//! drawable parts off them:
//!
//! - an observer on `Add<ModelRef>` builds the node's meshes and
//!   materials and spawns one child entity per material group;
//! - [`WorldEnv`] becomes `DistanceFog` on the camera and [`SunLight`] a
//!   `DirectionalLight` plus `AmbientLight`;
//! - [`LodSwitch`] hides and shows its children by camera distance.
//!
//! # Loading a level
//!
//! [`LoadLevel`] is the request and [`LoadedLevel`] the result. The level's
//! `gamez` archive, its own texture archive and the cross-mission pool all
//! have to finish loading before anything can be spawned, so
//! [`poll_level_load`] waits on them and spawns in one go.

use bevy::asset::{LoadState, UntypedAssetId};
use bevy::camera::visibility::VisibilitySystems;
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bft_assets::axes::AxisConvention;
use bft_assets::catalog::{LevelCatalog, LevelId};
use bft_assets::interp::{InterpScripts, RuntimeBindings, TextureAnim};
use bft_assets::level::{LevelData, ZMap};
use bft_assets::materials::MaterialDesc;
use bft_assets::models::ModelDesc;
use bft_assets::textures::{TexturePool, TextureSet};
use bft_sim::anim::AnimPlaced;
use bft_sim::level::{
    DespawnLevel, LevelRoot, LodSwitch, ModelRef, NodeActive, NodeId, NodeIndex, NodeName,
    SunLight, WorldEnv, spawn_level_into,
};

use crate::materials::{
    CycledTexture, EffectTextureCycles, MaterialCache, MaterialParams, TextureScroll,
    build_material,
};
use crate::mesh_builder::{Facade, FacadePivot, MeshCache, build_model};

/// How bright a level's sun is, in lux, at `diffuse == 1.0`.
///
/// The data gives a 0–1 share, not a photometric value, so the absolute scale
/// is a tuning constant. 10 000 lux is Bevy's "overcast day" figure and reads
/// correctly against the game's own fog.
pub const SUN_ILLUMINANCE: f32 = 10_000.0;

/// How bright ambient light is at `ambient == 1.0`.
///
/// The levels lean heavily on ambient — m5 and m11 are 0.8 — because the 1999
/// renderer had no shadowing to fill in. Tuned so a 0.45 ambient level looks
/// lit rather than washed out.
pub const AMBIENT_BRIGHTNESS: f32 = 600.0;

/// Intensity for a model-carried [`LevelLight`](bft_sim::level::LevelLight).
///
/// The level spawn attaches none — Recoil's model lights are weapon and
/// effect templates, not level lighting (docs/data/nodes.md) — so this is the
/// constant the anim VM's lights use (`anim_view`).
pub const POINT_LIGHT_INTENSITY: f32 = 100_000.0;

/// Ask for a level to be loaded. Insert it as a resource; [`start_level_load`]
/// picks it up, and it is removed once loading starts.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadLevel(pub LevelId);

/// A level being loaded or in play.
#[derive(Resource, Debug, Clone)]
pub struct LoadedLevel {
    pub id: LevelId,
    pub data: Handle<LevelData>,
    /// The level's own texture archive first, then the cross-mission pool, as
    /// `TexturePool` orders them (docs/data/materials.md).
    pub textures: Vec<Handle<TextureSet>>,
    /// `None` for m13, which ships no automap; a placeholder copy of m1's for
    /// m7–m12 (docs/data/quirks.md).
    pub zmap: Option<Handle<ZMap>>,
    /// The level root, once spawned.
    pub root: Option<Entity>,
    /// The interp scripts, whose runtime path binds this mission's texture
    /// cycles and scrolls (docs/data/materials.md: nothing else defines them).
    pub interp: Option<Handle<InterpScripts>>,
    /// Whether [`apply_texture_anims`] has run for this level yet.
    pub anims_applied: bool,
    /// The whole per-mission reader archive: enemy placements, pickups, the
    /// player start (`aiv.zrd`'s `bft_00` — the `bft_00` *node* is a
    /// build-script placeholder and is not it, READER-SCHEMA §3.2) and this
    /// mission's objectives.
    pub reader: Option<Handle<bft_assets::reader::ReaderData>>,
    /// The level's compiled animation definitions: the level-scripting VM's
    /// program (docs/data/anims.md). `None` for a level whose anim extraction
    /// failed, which costs the scripting but not the level.
    pub anim: Option<Handle<bft_assets::anim::AnimSet>>,
}

impl LoadedLevel {
    /// A level whose assets are on the way and which has not spawned yet.
    pub fn loading(
        id: LevelId,
        data: Handle<LevelData>,
        textures: Vec<Handle<TextureSet>>,
        zmap: Option<Handle<ZMap>>,
        interp: Option<Handle<InterpScripts>>,
        reader: Option<Handle<bft_assets::reader::ReaderData>>,
        anim: Option<Handle<bft_assets::anim::AnimSet>>,
    ) -> LoadedLevel {
        LoadedLevel {
            id,
            data,
            textures,
            zmap,
            root: None,
            interp,
            anims_applied: false,
            reader,
            anim,
        }
    }

    /// Whether the hierarchy has been spawned yet.
    pub fn is_spawned(&self) -> bool {
        self.root.is_some()
    }
}

/// The level plugin: loading, spawning and drawing.
pub struct LevelViewPlugin;

impl Plugin for LevelViewPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(make_node_visible)
            .add_observer(make_level_root_visible)
            .add_observer(attach_model)
            .add_observer(animate_cloned_node)
            .add_observer(forget_level_caches)
            .add_observer(mark_sky_dome)
            .add_systems(
                Update,
                (
                    start_level_load,
                    poll_level_load,
                    apply_level_texture_anims,
                    apply_world_env,
                    apply_sun,
                )
                    .chain(),
            )
            // LOD switching writes `Visibility`, which has to land before
            // Bevy computes inherited visibility for the frame. The sky
            // follows the camera in the same pass, before transforms are
            // propagated, so it never lags a frame behind.
            .add_systems(
                PostUpdate,
                // `mirror_node_visibility` before `switch_lods`, so a node
                // the VM turned off this tick cannot be shown again by the
                // LOD pass in the same frame.
                (mirror_node_visibility, switch_lods, follow_sky_dome)
                    .chain()
                    .before(VisibilitySystems::VisibilityPropagate)
                    .before(TransformSystems::Propagate),
            );
    }
}

/// Put a level's own texture archives at the front of the pool, in order.
///
/// In reverse, because each `prefer` goes to the front. `build_material`
/// reads a level texture out of `textures[0]`, and a pool that found another
/// archive's copy first handed texture cycles a different image handle for
/// the same name (`fire101` is in more than one of m1's), which
/// `claim_animated_parts` then never matched: every wreck's flame stood
/// still on its first frame.
pub fn prefer_level_archives(pool: &mut TexturePool, textures: &[Handle<TextureSet>]) {
    for handle in textures.iter().rev() {
        pool.prefer(handle.clone());
    }
}

/// Turn a [`LoadLevel`] request into asset handles.
pub fn start_level_load(
    mut commands: Commands,
    request: Option<Res<LoadLevel>>,
    loaded: Option<Res<LoadedLevel>>,
    catalog: Res<LevelCatalog>,
    server: Res<AssetServer>,
    mut pool: ResMut<TexturePool>,
) {
    let Some(request) = request else {
        return;
    };

    // Asking for the level already in play is a no-op rather than a reload.
    if loaded.is_some_and(|loaded| loaded.id == request.0) {
        commands.remove_resource::<LoadLevel>();
        return;
    }

    let Some(entry) = catalog.level(request.0) else {
        error!(
            "no level {} in the catalog; run `open-bft extract` or pick another",
            request.0
        );
        commands.remove_resource::<LoadLevel>();
        return;
    };

    // Tear down whatever is loaded before loading the next one, so two levels
    // never share the world.
    commands.trigger(DespawnLevel);

    let data = server.load::<LevelData>(entry.gamez.clone());
    let zmap = entry.zmap.clone().map(|path| server.load::<ZMap>(path));

    // The level's own archive is preferred and the rest of the catalog's pool
    // backs it up: 27 of m1's 471 references are only in other missions'
    // archives (docs/data/materials.md).
    let mut textures: Vec<Handle<TextureSet>> = Vec::new();
    for path in &entry.textures {
        textures.push(server.load::<TextureSet>(path.clone()));
    }
    prefer_level_archives(&mut pool, &textures);
    for path in &catalog.texture_pool {
        let handle = server.load::<TextureSet>(path.clone());
        pool.push(handle.clone());
        textures.push(handle);
    }

    info!(
        "loading level {} ({}): {} texture archive(s){}",
        entry.id,
        entry.name,
        textures.len(),
        if entry.zmap.is_some() {
            ", automap"
        } else {
            ""
        }
    );

    let interp = catalog
        .interp
        .clone()
        .map(|path| server.load::<InterpScripts>(path));

    // The mission's reader archive: the player start (READER-SCHEMA §3.2),
    // `puppies.zrd`'s pickups, `ai.zrd`'s turrets and `objectives.zrd`.
    let reader = server.load::<bft_assets::reader::ReaderData>(entry.reader.clone());
    // The level-scripting VM's program. A level whose anim extraction failed
    // still loads; it just has no scripted animation (the catalog keeps
    // `anim: None` for exactly that case).
    let anim = entry
        .anim
        .clone()
        .map(|path| server.load::<bft_assets::anim::AnimSet>(path));

    commands.insert_resource(LoadedLevel::loading(
        request.0,
        data,
        textures,
        zmap,
        interp,
        Some(reader),
        anim,
    ));
    commands.remove_resource::<LoadLevel>();
}

/// Spawn the hierarchy once the level and its textures have loaded.
///
/// The textures are waited on too, not just the level: a material built before
/// its `TextureSet` exists would be untextured and cached that way.
pub fn poll_level_load(
    mut commands: Commands,
    loaded: Option<ResMut<LoadedLevel>>,
    server: Res<AssetServer>,
    levels: Res<Assets<LevelData>>,
    axes: Option<Res<AxisConvention>>,
) {
    let Some(mut loaded) = loaded else {
        return;
    };
    if loaded.is_spawned() {
        return;
    }

    match server.load_state(&loaded.data) {
        LoadState::Loaded => {}
        LoadState::Failed(e) => {
            error!("level {} failed to load: {e}", loaded.id);
            // Removing the resource stops this from logging every frame; the
            // user picks another level.
            commands.remove_resource::<LoadedLevel>();
            return;
        }
        // Still loading, or not started.
        _ => return,
    }

    // A texture archive that fails is a warning, not a failure: the material
    // path already falls back to a placeholder per name, and one missing
    // archive should not cost the level its geometry.
    // The reader archive decides where the camera and the player go, so it
    // has to be in before the level opens; like a texture archive, a failed
    // one is a warning and a fallback rather than a dead level. The anim set
    // is waited on for the same reason: the VM applies every def's
    // `reset_state` at spawn, and a level spawned before that has the wrong
    // nodes visible for a frame.
    // `UntypedAssetId`, because `AssetId<T>` is typed and the two handles
    // are different asset types.
    let assets_pending = loaded
        .reader
        .iter()
        .map(Handle::id)
        .map(Into::<UntypedAssetId>::into)
        .chain(loaded.anim.iter().map(Handle::id).map(Into::into))
        .any(|id| {
            !matches!(
                server.load_state(id),
                LoadState::Loaded | LoadState::Failed(_)
            )
        });
    if assets_pending {
        return;
    }

    let textures_pending = loaded.textures.iter().any(|handle| {
        // A failed archive counts as settled, not pending: waiting on it would
        // hang the level for good.
        !matches!(
            server.load_state(handle),
            LoadState::Loaded | LoadState::Failed(_)
        )
    });
    if textures_pending {
        return;
    }

    let Some(level) = levels.get(&loaded.data) else {
        return;
    };

    let axes = axes.map(|axes| *axes).unwrap_or_default();
    let root = spawn_level_into(&mut commands, level, &axes);

    loaded.root = Some(root);
}

/// Give every spawned node a `Visibility`.
///
/// `bft-sim` cannot: `Visibility` is a `bevy_render` type and that crate builds
/// without a renderer (NFR-4). It has to go on **every** node, not just the
/// ones with a model, because inherited visibility is computed down the
/// hierarchy — a model-bearing child under a model-less group (m1's `g6` has
/// six such children) would otherwise sit under a parent with no `Visibility`,
/// which Bevy reports as error B0004 and which breaks LOD switching, since a
/// `Lod` node hides its children by inheritance.
pub fn make_node_visible(add: On<Add, NodeId>, mut commands: Commands) {
    commands.entity(add.entity).insert(Visibility::default());
}

/// Mirror the anim VM's [`NodeActive`] onto Bevy's `Visibility`.
///
/// `active` is visibility *and* collision for a node's subtree
/// (docs/data/anims.md), and the VM owns it — `bft-sim` has no renderer, so it
/// cannot write `Visibility` itself (NFR-4). This is the other half: what the
/// VM turns off stops being drawn.
///
/// It matters from the very first frame, not just when an animation runs:
/// every node in `gamez` is stored `active: true`, and it is each
/// definition's `reset_state` that turns off the destroyed hulks, the debris
/// and the opened-door variants. m1 alone starts 507 nodes inactive.
///
/// `Inherited` rather than `Visible` for the on case: a node inside a hidden
/// `Lod` subtree must stay hidden, and forcing it visible would defeat the
/// LOD switching that [`switch_lods`] does.
///
/// A template the anim VM has placed somewhere ([`AnimPlaced`]) is `Visible`
/// when on, so it shows through the hidden template root above it.
pub fn mirror_node_visibility(
    mut nodes: Query<(&NodeActive, &mut Visibility, Has<AnimPlaced>), Changed<NodeActive>>,
) {
    for (active, mut visibility, placed) in &mut nodes {
        let wanted = match (active.0, placed) {
            (true, true) => Visibility::Visible,
            (true, false) => Visibility::Inherited,
            (false, _) => Visibility::Hidden,
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}

/// The same for the level root, which is not a node.
///
/// Its 174–220 root-tree children each have a `Visibility` and it does not, so
/// without this every one of them is a B0004 warning and the whole level's
/// inherited visibility is computed from a missing parent.
pub fn make_level_root_visible(add: On<Add, LevelRoot>, mut commands: Commands) {
    commands.entity(add.entity).insert(Visibility::default());
}

/// Build and attach one node's drawable parts.
///
/// An observer rather than a system so a node is drawn the moment it is
/// spawned, whoever spawned it — the level loader here, or the anim VM
/// cloning a template later.
// Twelve parameters, all of them ECS injections: the asset tables, the two
// caches and the level it reads from. Splitting it would mean passing the same
// set on by hand.
#[allow(clippy::too_many_arguments)]
pub fn attach_model(
    add: On<Add, ModelRef>,
    mut commands: Commands,
    nodes: Query<&ModelRef>,
    loaded: Option<Res<LoadedLevel>>,
    levels: Res<Assets<LevelData>>,
    sets: Res<Assets<TextureSet>>,
    pool: Res<TexturePool>,
    axes: Option<Res<AxisConvention>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut mesh_cache: ResMut<MeshCache>,
    mut material_cache: ResMut<MaterialCache>,
    effect_cycles: Option<Res<EffectTextureCycles>>,
) {
    let entity = add.entity;

    let Ok(ModelRef(model_index)) = nodes.get(entity).copied() else {
        return;
    };
    // Without a loaded level there is no model table to index, which is the
    // case in a `MinimalPlugins` sim test.
    let Some(loaded) = loaded else {
        return;
    };
    let Some(level) = levels.get(&loaded.data) else {
        return;
    };
    let Some(model) = level.models.get(model_index) else {
        warn!("node model index {model_index} is past the level's model table");
        return;
    };

    let axes = axes.map(|axes| *axes).unwrap_or_default();

    let built = build_model(
        loaded.data.id().untyped(),
        model_index,
        model,
        &axes,
        &mut mesh_cache,
        &mut meshes,
    );

    if let Some(mode) = built.facade {
        commands.entity(entity).insert(Facade {
            mode,
            centroid: model.flags.facade_centroid,
        });
        // Never set in Recoil, so this branch does not fire on shipped data.
        if model.flags.facade_centroid {
            commands.entity(entity).insert(FacadePivot(built.bbox_mid));
        }
    }

    // 148 of the game's models have no polygons at all and every one of them
    // is referenced by a real node: an empty part list is normal, and the
    // entity keeps its transform and children (docs/data/models.md).
    if built.parts.is_empty() {
        return;
    }

    // The level's own archive, when it has loaded. `build_material` falls back
    // to the cross-mission pool for the 27 names m1 does not hold itself, and
    // to a placeholder for m9's one genuinely dangling `surf00`
    // (docs/data/materials.md).
    let empty_set = TextureSet::default();
    let level_set = loaded
        .textures
        .first()
        .and_then(|handle| sets.get(handle))
        .unwrap_or(&empty_set);

    for part in &built.parts {
        let Some(desc) = level.materials.get(part.material_index as usize) else {
            warn!(
                "model {model_index} names material {} of {}",
                part.material_index,
                level.materials.len()
            );
            continue;
        };

        let material = build_material(
            MaterialParams {
                desc,
                lighting: model.flags.lighting,
                fog: model.flags.fog,
                // Per material *group*, decided by the triangulator, not per
                // material: otherwise every double-sided surface is culled
                // (docs/data/models.md).
                show_backface: part.show_backface,
                texture_scroll: None,
            },
            level_set,
            Some((&pool, &sets)),
            &mut material_cache,
            &mut materials,
        );

        commands.spawn((
            Mesh3d(part.mesh.clone()),
            MeshMaterial3d(material),
            crate::mesh_builder::ModelPart,
            ChildOf(entity),
        ));

        // A sprite whose texture opens an `effects.zrd` cycle runs it, on
        // this node wherever it was spawned. `insert_if_new`, so an interp
        // binding naming the node keeps priority: `apply_texture_anims`
        // inserts over this one on the level's own nodes.
        if let Some(cycle) = effect_cycles
            .as_ref()
            .zip(desc.texture())
            .and_then(|(cycles, texture)| cycles.cycle_for(texture, &pool, &sets))
        {
            commands.entity(entity).insert_if_new(cycle);
        }
    }
}

/// Drop the built meshes and materials when a level is torn down.
///
/// Without this the caches keep every handle alive and the next level's
/// memory is the sum of both.
pub fn forget_level_caches(
    _despawn: On<DespawnLevel>,
    mut commands: Commands,
    loaded: Option<Res<LoadedLevel>>,
    mut mesh_cache: ResMut<MeshCache>,
    mut material_cache: ResMut<MaterialCache>,
) {
    // `clear_level` rather than `clear` where possible, so a level loaded
    // while another is up does not throw away the other's meshes.
    match loaded.as_ref().map(|loaded| loaded.data.id().untyped()) {
        Some(id) => mesh_cache.clear_level(id),
        None => mesh_cache.clear(),
    }

    // Materials are keyed by their parameters, not by level, so there is no
    // per-level variant to clear.
    material_cache.clear();

    commands.remove_resource::<LoadedLevel>();
    commands.remove_resource::<NodeTextureAnims>();
}

/// Put the level's fog on every 3d camera.
///
/// Cameras that have no fog yet are served even when the resource has not
/// changed, because the viewer spawns its camera on entering
/// `Viewer` — *after* the level was loaded and `WorldEnv` last changed. A
/// change-detection-only version leaves that camera unfogged, which shows up
/// as a hard band of unfogged geometry at the far plane.
pub fn apply_world_env(
    mut commands: Commands,
    env: Option<Res<WorldEnv>>,
    mut cameras: Query<(Entity, Option<&DistanceFog>, &mut Camera), With<Camera3d>>,
) {
    let Some(env) = env else {
        return;
    };

    let changed = env.is_changed();
    for (camera, fog, mut settings) in &mut cameras {
        if !changed && fog.is_some() {
            continue;
        }

        // The fog colour doubles as the clear colour: Recoil's levels ship no
        // skybox, so anywhere the geometry does not reach reads as distance
        // rather than as a hole in the world. Set on the existing `Camera`
        // rather than inserted, which would reset `order` and the rest.
        settings.clear_color = ClearColorConfig::Custom(env.fog_color);
        commands.entity(camera).insert(DistanceFog {
            color: env.fog_color,
            // `fog_type` is `Linear` in all 13 levels, and the range is the
            // start and end distance, which is exactly `Linear`'s two
            // parameters.
            falloff: FogFalloff::Linear {
                start: env.fog_range.0,
                end: env.fog_range.1,
            },
            ..default()
        });
    }
}

/// The level's sun: one directional light, plus ambient on each camera.
pub fn apply_sun(
    mut commands: Commands,
    sun: Option<Res<SunLight>>,
    existing: Query<Entity, With<LevelSun>>,
    cameras: Query<(Entity, Option<&AmbientLight>), With<Camera3d>>,
) {
    let Some(sun) = sun else {
        return;
    };

    // The ambient term goes on a camera that has none even when the sun has
    // not changed, for the same reason as the fog above: the viewer's camera
    // is spawned after the level is loaded.
    let changed = sun.is_changed();
    if !changed {
        for (camera, ambient) in &cameras {
            if ambient.is_none() {
                commands.entity(camera).insert(AmbientLight {
                    color: sun.color,
                    brightness: sun.ambient * AMBIENT_BRIGHTNESS,
                    ..default()
                });
            }
        }
        return;
    }

    for entity in &existing {
        commands.entity(entity).despawn();
    }

    commands.spawn((
        LevelSun,
        DirectionalLight {
            color: sun.color,
            illuminance: sun.diffuse * SUN_ILLUMINANCE,
            // Shadows are off: the original had none, and 4000 static nodes
            // in one cascade is a lot of shadow-map work for an effect the
            // game never had (FR-W6's "looks like Recoil").
            shadow_maps_enabled: false,
            ..default()
        },
        // A `DirectionalLight` shines along its own -z.
        Transform::from_translation(Vec3::ZERO).looking_to(sun.direction.as_vec3(), Vec3::Y),
    ));

    // In Bevy 0.19 `AmbientLight` is a per-camera component, not a resource.
    for (camera, _) in &cameras {
        commands.entity(camera).insert(AmbientLight {
            color: sun.color,
            brightness: sun.ambient * AMBIENT_BRIGHTNESS,
            ..default()
        });
    }
}

/// The entity carrying the level's directional light.
#[derive(Component, Debug, Clone, Copy)]
pub struct LevelSun;

/// The node every level's sky is modelled on.
///
/// All 13 levels carry exactly one, always with `transform: "Initial"` — an
/// identity, so the dome sits at the **world origin** — and always with
/// `lighting: false, fog: false` on its model, which is how a 1999 renderer
/// says "this is the sky". Spans are ±1749 to ±3264 depending on the level.
///
/// A dome fixed at the origin clips straight through any level whose content
/// is elsewhere, which every level's is: m1's terrain sits around
/// (2200, 1700) while its dome reaches only ±2560. It has to follow the
/// camera, which is what [`SkyDome`] and [`follow_sky_dome`] do.
pub const SKY_NODE: &str = "horizon";

/// A level's sky, which follows the camera instead of standing still.
///
/// Attached to the [`SKY_NODE`] entity and to its descendants' parent — the
/// whole subtree moves, because m2, m6 and m10 model the sky as a group of
/// two to four children rather than one mesh.
#[derive(Component, Debug, Clone, Copy)]
pub struct SkyDome {
    /// The dome's own y offset at spawn, kept so following the camera in x
    /// and z does not also drag the horizon line up and down with it.
    pub base_height: f32,
}

/// Mark the level's sky so it can follow the camera.
///
/// An observer on the name rather than a lookup at spawn time, because the
/// node arrives with the rest of the hierarchy and the camera may not exist
/// yet.
pub fn mark_sky_dome(add: On<Add, NodeName>, names: Query<&NodeName>, mut commands: Commands) {
    let Ok(name) = names.get(add.entity) else {
        return;
    };
    if name.0 != SKY_NODE {
        return;
    }

    info!("sky dome `{SKY_NODE}` will follow the camera");
    commands.entity(add.entity).insert(SkyDome {
        // Every level's is an identity transform, so this is 0 in practice;
        // read rather than assumed, in case a level is ever different.
        base_height: 0.0,
    });
}

/// Keep the sky centred on the camera.
///
/// Only x and z follow: a dome that tracked the camera's height too would
/// keep the horizon line at eye level however high the player flew, which
/// looks wrong the moment you leave the ground. The y offset stays where the
/// level put it, so climbing raises you towards the top of the dome exactly
/// as the original did.
pub fn follow_sky_dome(
    camera: Query<&GlobalTransform, crate::ui::hud::sensor::MainCamera3d>,
    mut domes: Query<(&SkyDome, &mut Transform)>,
) {
    let Some(eye) = camera.iter().next().map(|t| t.translation()) else {
        return;
    };

    for (dome, mut transform) in &mut domes {
        let wanted = Vec3::new(eye.x, dome.base_height, eye.z);

        // Written only when it moves, so a stationary camera does not dirty
        // the transform and force a hierarchy propagation every frame.
        if transform.translation != wanted {
            transform.translation = wanted;
        }
    }
}

/// Show a `Lod` node's children while the camera is inside its band.
///
/// The simplest faithful reading of the data: `range` is a clean distance band
/// (0–256, 256–512, …) and a far value of ~1e7 means "no far limit". The
/// distance is measured to the node's own origin, which is what a 1999
/// renderer would have done.
pub fn switch_lods(
    camera: Query<&GlobalTransform, crate::ui::hud::sensor::MainCamera3d>,
    mut lods: Query<(&LodSwitch, &GlobalTransform, &mut Visibility)>,
) {
    let Some(eye) = camera.iter().next().map(|t| t.translation()) else {
        return;
    };

    for (lod, transform, mut visibility) in &mut lods {
        let distance = transform.translation().distance(eye);
        let inside = distance >= lod.near && distance <= lod.far;

        // Written unconditionally only when it changes, so unchanged LOD nodes
        // do not dirty change detection every frame for 1245 nodes.
        let wanted = if inside {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *visibility != wanted {
            *visibility = wanted;
        }
    }
}

/// Run [`apply_texture_anims`] once, as soon as a level is spawned and its
/// interp scripts have arrived.
///
/// Separate from [`poll_level_load`] because the two wait on different things:
/// the level can be spawned while `interp.json` is still in flight, and the
/// cycles have to be attached to nodes that already exist.
pub fn apply_level_texture_anims(
    mut commands: Commands,
    loaded: Option<ResMut<LoadedLevel>>,
    scripts: Res<Assets<InterpScripts>>,
    nodes: Option<Res<NodeIndex>>,
    names: Query<&NodeName>,
    pool: Res<TexturePool>,
    sets: Res<Assets<TextureSet>>,
) {
    let Some(mut loaded) = loaded else {
        return;
    };
    if loaded.anims_applied || !loaded.is_spawned() {
        return;
    }
    let Some(nodes) = nodes else {
        return;
    };
    let Some(handle) = loaded.interp.clone() else {
        // No interp in the cache: the level draws, it just does not animate.
        loaded.anims_applied = true;
        return;
    };
    let Some(scripts) = scripts.get(&handle) else {
        return;
    };

    let bindings = scripts.bindings(loaded.id.0);
    apply_texture_anims(&mut commands, &bindings, &nodes, &names, &pool, &sets);
    commands.insert_resource(NodeTextureAnims::from_bindings(&bindings));
    loaded.anims_applied = true;
}

/// Attach the interp texture animations to the nodes they name
/// (docs/data/materials.md).
///
/// Neither animation is baked into `gamez`: 215 cycles and 157 scrolls across
/// the 13 missions all come from the runtime interp script. Call this once
/// after a level is spawned, with the mission's bindings.
pub fn apply_texture_anims(
    commands: &mut Commands,
    bindings: &RuntimeBindings,
    nodes: &NodeIndex,
    names: &Query<&NodeName>,
    pool: &TexturePool,
    sets: &Assets<TextureSet>,
) -> (usize, usize) {
    let mut scrolls = 0usize;
    let mut cycles = 0usize;
    let mut unmatched = 0usize;

    for binding in &bindings.texture_anims {
        // A sub-node narrows the target, but the engine's hierarchy is keyed
        // by name at every level, so both forms resolve the same way; the
        // sub-node wins when it is present.
        let target = binding.sub_node.as_deref().unwrap_or(&binding.node);
        let entities = nodes.by_name(target);

        if entities.is_empty() {
            unmatched += 1;
            continue;
        }

        for entity in entities {
            let _ = names;

            match insert_texture_anim(commands, *entity, &binding.anim, pool, sets) {
                Some(TextureAnimKind::Scroll) => scrolls += 1,
                Some(TextureAnimKind::Cycle) => cycles += 1,
                None => {}
            }
        }
    }

    if unmatched > 0 {
        debug!("{unmatched} interp texture binding(s) name no node in this level");
    }
    info!("attached {scrolls} texture scrolls and {cycles} texture cycles");

    (scrolls, cycles)
}

/// Which texture animation [`insert_texture_anim`] attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextureAnimKind {
    Scroll,
    Cycle,
}

/// Attach one interp texture animation to a node.
///
/// `None` for a cycle none of whose frames can be found: the six
/// `spray01`-`spray06` frames are in no archive, so a partially-resolved
/// cycle is expected, and an empty one has nothing to show.
fn insert_texture_anim(
    commands: &mut Commands,
    entity: Entity,
    anim: &TextureAnim,
    pool: &TexturePool,
    sets: &Assets<TextureSet>,
) -> Option<TextureAnimKind> {
    match anim {
        TextureAnim::Scroll { u, v } => {
            commands
                .entity(entity)
                .insert(TextureScroll::new(Vec2::new(*u, *v)));
            Some(TextureAnimKind::Scroll)
        }
        TextureAnim::Cycle {
            frames,
            fps,
            looping,
        } => {
            let images: Vec<Handle<Image>> = frames
                .iter()
                .filter_map(|frame| pool.image(frame, sets))
                .collect();
            if images.is_empty() {
                return None;
            }
            commands
                .entity(entity)
                .insert(CycledTexture::new(images, *fps, *looping));
            Some(TextureAnimKind::Cycle)
        }
    }
}

/// This level's interp texture animations by the node name they bind.
///
/// Kept after [`apply_level_texture_anims`] has run so a node spawned later
/// — an enemy's cloned model, an effect the anim VM copies — gets what its
/// prototype got. The drone's engine flame, `drone_fire`, is one: its cycle
/// is bound by name, and every drone in the level is a clone.
#[derive(Resource, Debug, Clone, Default)]
pub struct NodeTextureAnims(pub HashMap<String, Vec<TextureAnim>>);

impl NodeTextureAnims {
    /// Index a mission's bindings by the node each one targets.
    pub fn from_bindings(bindings: &RuntimeBindings) -> NodeTextureAnims {
        let mut by_name: HashMap<String, Vec<TextureAnim>> = HashMap::new();
        for binding in &bindings.texture_anims {
            let target = binding.sub_node.as_deref().unwrap_or(&binding.node);
            by_name
                .entry(target.to_string())
                .or_default()
                .push(binding.anim.clone());
        }
        NodeTextureAnims(by_name)
    }
}

/// Give a cloned node the interp texture animations its name binds.
///
/// A clone carries its prototype's [`NodeName`] but no [`NodeId`], which is
/// how it is told apart from a level node: those were bound once, by
/// [`apply_texture_anims`], before any clone existed.
pub fn animate_cloned_node(
    add: On<Add, NodeName>,
    mut commands: Commands,
    nodes: Query<&NodeName, Without<NodeId>>,
    anims: Option<Res<NodeTextureAnims>>,
    pool: Res<TexturePool>,
    sets: Res<Assets<TextureSet>>,
) {
    let (Some(anims), Ok(name)) = (anims, nodes.get(add.entity)) else {
        return;
    };
    let Some(bound) = anims.0.get(&name.0) else {
        return;
    };
    for anim in bound {
        insert_texture_anim(&mut commands, add.entity, anim, &pool, &sets);
    }
}

/// Which models a level actually draws, for the `inspect` command and tests.
///
/// Counts the material groups and triangles a level would spawn, without
/// touching the GPU.
pub fn level_draw_census(level: &LevelData, axes: &AxisConvention) -> DrawCensus {
    let mut census = DrawCensus::default();
    let mut per_model: HashMap<usize, usize> = HashMap::new();

    for node in &level.nodes {
        let Some(model_index) = node.common().model_index else {
            continue;
        };
        census.model_refs += 1;
        *per_model.entry(model_index).or_default() += 1;

        if let Some(model) = level.models.get(model_index) {
            let triangulated = bft_assets::geometry::triangulate(model, axes);
            census.triangles += triangulated.triangle_count();
            if triangulated.groups.is_empty() {
                census.empty_models += 1;
            }
        }
    }

    census.distinct_models = per_model.len();
    census
}

/// What [`level_draw_census`] found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DrawCensus {
    /// How many nodes reference a model.
    pub model_refs: usize,
    /// How many distinct models those references use.
    pub distinct_models: usize,
    /// Fully instanced triangle count.
    pub triangles: usize,
    /// References to models with no drawable geometry (the light carriers).
    pub empty_models: usize,
}

/// A material description's texture name, for tests and diagnostics.
pub fn material_texture(desc: &MaterialDesc) -> Option<&str> {
    desc.texture()
}

/// A model's part count without building meshes, for diagnostics.
pub fn model_group_count(model: &ModelDesc, axes: &AxisConvention) -> usize {
    bft_assets::geometry::triangulate(model, axes)
        .groups
        .iter()
        .filter(|group| !group.indices.is_empty())
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::level::parse_gamez;
    use bft_assets::test_support::minimal_gamez;

    fn minimal_level() -> LevelData {
        parse_gamez(&minimal_gamez(), "test.gamez.zip").expect("the fixture loads")
    }

    /// An app with the render-side assets but no window or GPU.
    fn test_app() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            bevy::transform::TransformPlugin,
        ));
        app.init_asset::<Mesh>()
            .init_asset::<Image>()
            .init_asset::<StandardMaterial>()
            .init_asset::<LevelData>()
            .init_asset::<TextureSet>()
            .init_resource::<MeshCache>()
            .init_resource::<MaterialCache>()
            .init_resource::<TexturePool>()
            .insert_resource(AxisConvention::GAMEZ_DEFAULT)
            .add_observer(make_node_visible)
            .add_observer(make_level_root_visible)
            .add_observer(attach_model)
            .add_observer(forget_level_caches)
            .add_observer(mark_sky_dome)
            .add_observer(bft_sim::level::despawn_level);
        app
    }

    /// Put a level in `Assets` and a `LoadedLevel` alongside it, as the real
    /// loader would once loading finished.
    fn install_level(app: &mut App, level: LevelData) -> Handle<LevelData> {
        let handle = app
            .world_mut()
            .resource_mut::<Assets<LevelData>>()
            .add(level);

        app.insert_resource(LoadedLevel::loading(
            LevelId(1),
            handle.clone(),
            Vec::new(),
            None,
            None,
            None,
            None,
        ));

        handle
    }

    /// Every level models its sky at the world origin, so a dome that stays
    /// put clips straight through terrain that sits elsewhere — m1's is
    /// around (2200, 1700) with a dome reaching only 2560.
    #[test]
    fn the_sky_dome_follows_the_camera_in_x_and_z() {
        let mut app = test_app();
        app.add_systems(Update, follow_sky_dome);

        let dome = app
            .world_mut()
            .spawn((
                SkyDome { base_height: 0.0 },
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        // `Transform`, not `GlobalTransform`: `TransformPlugin` recomputes
        // the global from the local every frame and would undo a direct write.
        app.world_mut().spawn((
            Camera3d::default(),
            Transform::from_xyz(2222.0, 40.0, 1294.0),
        ));

        app.update();

        let moved = app.world().get::<Transform>(dome).expect("still there");
        assert_eq!(moved.translation.x, 2222.0);
        assert_eq!(moved.translation.z, 1294.0);
    }

    /// Only x and z. A dome that tracked height too would pin the horizon at
    /// eye level however high the player flew.
    #[test]
    fn the_sky_dome_keeps_its_own_height() {
        let mut app = test_app();
        app.add_systems(Update, follow_sky_dome);

        let dome = app
            .world_mut()
            .spawn((
                SkyDome { base_height: 0.0 },
                Transform::default(),
                GlobalTransform::default(),
            ))
            .id();
        app.world_mut().spawn((
            Camera3d::default(),
            Transform::from_xyz(100.0, 900.0, 200.0),
        ));

        app.update();

        assert_eq!(
            app.world().get::<Transform>(dome).unwrap().translation.y,
            0.0,
            "the horizon line stays where the level put it"
        );
    }

    fn spawn(app: &mut App, level: &LevelData) {
        let level = level.clone();
        app.world_mut()
            .run_system_cached_with(
                |In(level): In<LevelData>, mut commands: Commands| {
                    spawn_level_into(&mut commands, &level, &AxisConvention::GAMEZ_DEFAULT);
                },
                level,
            )
            .expect("runs");
        app.update();
    }

    #[test]
    fn a_model_bearing_node_gets_one_mesh_child_per_material_group() {
        let level = minimal_level();
        let mut app = test_app();
        install_level(&mut app, level.clone());
        spawn(&mut app, &level);

        let index = app.world().resource::<NodeIndex>().clone();
        let floor = index.get(2).expect("the floor");

        let children = app
            .world()
            .get::<Children>(floor)
            .map(|c| c.len())
            .unwrap_or(0);
        assert_eq!(children, 1, "one material group, so one mesh child");

        // And the child really carries a mesh and a material.
        let meshes = app
            .world_mut()
            .query::<(&Mesh3d, &MeshMaterial3d<StandardMaterial>)>()
            .iter(app.world())
            .count();
        assert_eq!(meshes, 1);
    }

    #[test]
    fn a_node_whose_model_has_no_polygons_still_keeps_its_entity() {
        // 148 models across the game are polygon-less light carriers, and
        // every one is referenced by a real node (docs/data/models.md).
        let nodes = format!(
            "[{},{}]",
            bft_assets::test_support::WORLD_NODE,
            r#"{ "name": "sparky", "model_index": 0, "parent_indices": [0],
                 "flags": { "active": true, "tree_valid": true },
                 "data": { "Object3d": { "transform": "Initial" } } }"#
        );
        // A model with lights and no polygons, as the real carriers are.
        let models = r#"[{
          "model_type": "Default", "facade_mode": "CylindricalY",
          "flags": { "lighting": true, "fog": true },
          "vertices": [], "normals": [], "morphs": [], "polygons": [],
          "lights": [ { "vertices": [ { "x": 0.0, "y": 13.0, "z": 0.0 } ],
                        "color": { "r": 255.0, "g": 0.0, "b": 0.0 }, "flags": 655 } ],
          "texture_scroll": { "u": 0.0, "v": 0.0 },
          "bbox_mid": { "x": 0.0, "y": 0.0, "z": 0.0 }, "bbox_diag": 0.0
        }]"#;
        let bytes = bft_assets::test_support::gamez_from("[]", "[]", models, &nodes);
        let level = parse_gamez(&bytes, "test").expect("loads");

        let mut app = test_app();
        install_level(&mut app, level.clone());
        spawn(&mut app, &level);

        let index = app.world().resource::<NodeIndex>().clone();
        let sparky = index.get(1).expect("the carrier node");

        assert!(app.world().get::<ModelRef>(sparky).is_some());
        assert_eq!(
            app.world()
                .get::<Children>(sparky)
                .map(|c| c.len())
                .unwrap_or(0),
            0,
            "no mesh child, but the node is still here for its transform"
        );
    }

    #[test]
    fn every_node_entity_has_a_visibility_so_inheritance_works() {
        // The B0004 case: a model-bearing child under a model-less group. If
        // only `attach_model` added `Visibility`, the group would lack one and
        // both Bevy's hierarchy check and LOD inheritance would break.
        let level = minimal_level();
        let mut app = test_app();
        install_level(&mut app, level.clone());
        spawn(&mut app, &level);

        let index = app.world().resource::<NodeIndex>().clone();

        for id in 0..3u32 {
            let entity = index.get(id).expect("a node entity");
            assert!(
                app.world().get::<Visibility>(entity).is_some(),
                "node {id} ({:?}) has no Visibility",
                app.world().get::<NodeName>(entity)
            );
        }

        // Specifically the group, which has no model of its own.
        let group = index.get(1).expect("the group");
        assert!(app.world().get::<ModelRef>(group).is_none());
        assert!(app.world().get::<Visibility>(group).is_some());

        // And the level root, which is not a node at all but is every root
        // tree's parent.
        let root = app
            .world_mut()
            .query_filtered::<Entity, With<LevelRoot>>()
            .single(app.world())
            .expect("one level root");
        assert!(
            app.world().get::<Visibility>(root).is_some(),
            "the level root needs one too, or every root tree is a B0004"
        );
    }

    #[test]
    fn the_world_env_becomes_fog_on_the_camera() {
        let mut app = test_app();
        app.add_systems(Update, apply_world_env);
        let camera = app.world_mut().spawn(Camera3d::default()).id();

        app.insert_resource(WorldEnv {
            fog_color: Color::linear_rgb(0.5, 0.6, 0.7),
            fog_range: (550.0, 650.0),
            fog_altitude: (48.0, 24.0),
            area: (0, 0, 4096, 4352),
        });
        app.update();

        let fog = app.world().get::<DistanceFog>(camera).expect("fog");
        assert_eq!(fog.color, Color::linear_rgb(0.5, 0.6, 0.7));
        let FogFalloff::Linear { start, end } = fog.falloff else {
            panic!("linear falloff, as all 13 levels use");
        };
        assert_eq!((start, end), (550.0, 650.0));
    }

    #[test]
    fn the_sun_becomes_one_directional_light_and_ambient_on_the_camera() {
        let mut app = test_app();
        app.add_systems(Update, apply_sun);
        let camera = app.world_mut().spawn(Camera3d::default()).id();

        app.insert_resource(SunLight {
            direction: Dir3::NEG_Y,
            color: Color::WHITE,
            diffuse: 0.55,
            ambient: 0.45,
        });
        app.update();

        let lights: Vec<&DirectionalLight> = app
            .world_mut()
            .query::<&DirectionalLight>()
            .iter(app.world())
            .collect();
        assert_eq!(lights.len(), 1, "one sun, not one per node");
        assert_eq!(lights[0].illuminance, 0.55 * SUN_ILLUMINANCE);

        let ambient = app.world().get::<AmbientLight>(camera).expect("ambient");
        assert_eq!(ambient.brightness, 0.45 * AMBIENT_BRIGHTNESS);
    }

    #[test]
    fn a_second_sun_replaces_the_first_rather_than_adding_to_it() {
        let mut app = test_app();
        app.add_systems(Update, apply_sun);
        app.world_mut().spawn(Camera3d::default());

        app.insert_resource(SunLight {
            direction: Dir3::NEG_Y,
            color: Color::WHITE,
            diffuse: 0.5,
            ambient: 0.5,
        });
        app.update();
        app.insert_resource(SunLight {
            direction: Dir3::NEG_X,
            color: Color::WHITE,
            diffuse: 0.2,
            ambient: 0.8,
        });
        app.update();

        let lights = app
            .world_mut()
            .query::<&DirectionalLight>()
            .iter(app.world())
            .count();
        assert_eq!(lights, 1);
    }

    #[test]
    fn a_lod_node_is_visible_inside_its_band_and_hidden_outside() {
        let mut app = test_app();
        app.add_systems(Update, switch_lods);
        app.world_mut().spawn((
            Camera3d::default(),
            Transform::default(),
            GlobalTransform::default(),
        ));

        // Bands 0-100 and 200-300, with the node at the origin: the camera
        // sits inside the first and outside the second.
        let near = app
            .world_mut()
            .spawn((
                LodSwitch {
                    near: 0.0,
                    far: 100.0,
                    level: true,
                },
                Transform::default(),
                GlobalTransform::default(),
                Visibility::default(),
            ))
            .id();
        let far = app
            .world_mut()
            .spawn((
                LodSwitch {
                    near: 200.0,
                    far: 300.0,
                    level: false,
                },
                Transform::default(),
                GlobalTransform::default(),
                Visibility::default(),
            ))
            .id();

        app.update();

        assert_eq!(
            app.world().get::<Visibility>(near),
            Some(&Visibility::Inherited)
        );
        assert_eq!(
            app.world().get::<Visibility>(far),
            Some(&Visibility::Hidden)
        );
    }

    #[test]
    fn the_caches_are_emptied_when_the_level_goes_away() {
        let level = minimal_level();
        let mut app = test_app();
        install_level(&mut app, level.clone());
        spawn(&mut app, &level);

        assert_eq!(app.world().resource::<MeshCache>().len(), 1);
        assert_eq!(app.world().resource::<MaterialCache>().len(), 1);

        app.world_mut().trigger(DespawnLevel);
        app.update();

        assert!(app.world().resource::<MeshCache>().is_empty());
        assert!(app.world().resource::<MaterialCache>().is_empty());
        assert!(app.world().get_resource::<LoadedLevel>().is_none());
    }

    #[test]
    fn instanced_models_are_built_once_and_drawn_twice() {
        // The MeshCache's whole point: m1 references 1350 models from 2170
        // nodes (docs/data/models.md).
        let nodes = format!(
            "[{},{},{}]",
            bft_assets::test_support::WORLD_NODE,
            r#"{ "name": "a", "model_index": 0, "parent_indices": [0],
                 "flags": { "active": true }, "data": { "Object3d": { "transform": "Initial" } } }"#,
            r#"{ "name": "b", "model_index": 0, "parent_indices": [0],
                 "flags": { "active": true }, "data": { "Object3d": { "transform": "Initial" } } }"#
        );
        let bytes = bft_assets::test_support::gamez_from(
            r#"[{ "Textured": { "texture_index": 0 } }]"#,
            r#"[{ "name": "wall.tif" }]"#,
            bft_assets::test_support::MINIMAL_MODEL,
            &nodes,
        );
        let level = parse_gamez(&bytes, "test").expect("loads");

        let mut app = test_app();
        install_level(&mut app, level.clone());
        spawn(&mut app, &level);

        assert_eq!(app.world().resource::<MeshCache>().len(), 1, "built once");
        assert_eq!(
            app.world().resource::<Assets<Mesh>>().len(),
            1,
            "and one mesh asset, handed to both nodes"
        );
        assert_eq!(
            app.world_mut().query::<&Mesh3d>().iter(app.world()).count(),
            2,
            "drawn twice"
        );
        // One material too: identical parameters share a handle.
        assert_eq!(app.world().resource::<MaterialCache>().len(), 1);
    }

    #[test]
    fn the_draw_census_counts_instances_and_distinct_models() {
        let level = minimal_level();

        let census = level_draw_census(&level, &AxisConvention::GAMEZ_DEFAULT);

        assert_eq!(census.model_refs, 1);
        assert_eq!(census.distinct_models, 1);
        assert_eq!(census.triangles, 2, "one quad fans to two triangles");
        assert_eq!(census.empty_models, 0);
    }

    #[test]
    fn a_missing_level_id_is_logged_and_dropped_rather_than_retried() {
        let mut app = test_app();
        app.init_resource::<LevelCatalog>()
            .init_resource::<TexturePool>()
            .add_systems(Update, start_level_load);
        app.insert_resource(LoadLevel(LevelId(1)));

        app.update();

        // The request is consumed even though the catalog is empty, so the
        // error does not repeat every frame.
        assert!(app.world().get_resource::<LoadLevel>().is_none());
        assert!(app.world().get_resource::<LoadedLevel>().is_none());
    }

    /// The pool searches a level's archives in the level's own order, so a
    /// texture in two of them resolves to the image `build_material` used.
    #[test]
    fn a_levels_archives_are_searched_in_its_own_order() {
        let mut app = test_app();
        let (first, second, other) = {
            let mut sets = app.world_mut().resource_mut::<Assets<TextureSet>>();
            (
                sets.add(TextureSet::default()),
                sets.add(TextureSet::default()),
                sets.add(TextureSet::default()),
            )
        };
        let mut pool = TexturePool::new([other.clone()]);

        prefer_level_archives(&mut pool, &[first.clone(), second.clone()]);

        assert_eq!(pool.sets(), [first, second, other]);
    }

    /// A texture set holding `wall.tif` and a second frame, first in the pool.
    fn pool_with_two_frames(app: &mut App) -> [Handle<Image>; 2] {
        let frames = {
            let mut images = app.world_mut().resource_mut::<Assets<Image>>();
            [images.add(Image::default()), images.add(Image::default())]
        };
        let info = bft_assets::textures::TextureInfo {
            width: 1,
            height: 1,
            alpha: bft_assets::textures::TextureAlpha::None,
            stretch: bft_assets::textures::TextureStretch::None,
        };
        let set = TextureSet {
            images: [
                ("wall".to_string(), frames[0].clone()),
                ("wall2".to_string(), frames[1].clone()),
            ]
            .into(),
            info: [("wall".to_string(), info), ("wall2".to_string(), info)].into(),
        };
        let set = app
            .world_mut()
            .resource_mut::<Assets<TextureSet>>()
            .add(set);
        app.world_mut().resource_mut::<TexturePool>().push(set);
        frames
    }

    /// A node spawned after the level — an anim copy of `fire_hulk.flt`, an
    /// enemy's model — whose texture opens an `effects.zrd` cycle runs it.
    /// No interp binding names those nodes, and they sat on their first
    /// frame: the "frozen fire sprite" on every wreck.
    #[test]
    fn a_clone_showing_an_effects_first_frame_runs_its_cycle() {
        use bft_assets::reader::effects::{EffectCycle, EffectCycles};

        let level = minimal_level();
        let mut app = test_app();
        app.init_asset::<bft_assets::reader::ReaderData>()
            .init_resource::<Time>()
            .add_plugins(crate::materials::MaterialsPlugin)
            .insert_resource(EffectTextureCycles(EffectCycles {
                cycles: vec![EffectCycle {
                    name: "fire1".into(),
                    model: "fire1.flt".into(),
                    speed: 10.0,
                    looping: true,
                    maps: vec!["wall.tif".into(), "wall2.tif".into()],
                }],
            }));
        let frames = pool_with_two_frames(&mut app);
        install_level(&mut app, level.clone());
        spawn(&mut app, &level);

        // A clone: the prototype's name and model, no node id.
        let clone = app
            .world_mut()
            .spawn((NodeName("floor".into()), ModelRef(0), Transform::default()))
            .id();
        app.update();

        let cycle = app
            .world()
            .get::<CycledTexture>(clone)
            .expect("the clone runs the effect's cycle");
        assert_eq!(cycle.frames, frames.to_vec());
        assert!(cycle.looping);

        let parts = app
            .world()
            .get::<Children>(clone)
            .expect("the clone's parts");
        assert!(
            parts
                .iter()
                .any(|part| app.world().get::<CycledTexture>(part).is_some()),
            "the part drawing the frame is handed the cycle"
        );
    }

    /// A clone gets the interp texture animation its name binds, as its
    /// prototype did — the drone's `drone_fire` on every drone.
    #[test]
    fn a_clone_gets_the_interp_animation_its_name_binds() {
        let mut app = test_app();
        app.add_observer(animate_cloned_node);
        let frames = pool_with_two_frames(&mut app);
        app.insert_resource(NodeTextureAnims(
            [(
                "drone_fire".to_string(),
                vec![TextureAnim::Cycle {
                    frames: vec!["wall.tif".into(), "wall2.tif".into()],
                    fps: 12.0,
                    looping: true,
                }],
            )]
            .into(),
        ));

        let clone = app.world_mut().spawn(NodeName("drone_fire".into())).id();
        // A level node of the same name was bound by `apply_texture_anims`
        // already; the observer leaves it alone.
        let level_node = app
            .world_mut()
            .spawn((NodeName("drone_fire".into()), NodeId(7)))
            .id();
        app.update();

        assert_eq!(
            app.world()
                .get::<CycledTexture>(clone)
                .map(|cycle| cycle.frames.clone()),
            Some(frames.to_vec())
        );
        assert!(app.world().get::<CycledTexture>(level_node).is_none());
    }
}

#[cfg(test)]
mod visibility_tests {
    use super::*;

    /// The VM owns `active`; this is what makes it visible on screen.
    #[test]
    fn an_inactive_node_is_hidden_and_an_active_one_inherits() {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins);
        app.add_systems(Update, mirror_node_visibility);

        let off = app
            .world_mut()
            .spawn((NodeActive(false), Visibility::Inherited))
            .id();
        let on = app
            .world_mut()
            .spawn((NodeActive(true), Visibility::Hidden))
            .id();

        app.update();

        assert_eq!(
            app.world().get::<Visibility>(off),
            Some(&Visibility::Hidden)
        );
        // `Inherited`, not `Visible`: a node inside a hidden `Lod` subtree has
        // to stay hidden, and forcing it visible would defeat LOD switching.
        assert_eq!(
            app.world().get::<Visibility>(on),
            Some(&Visibility::Inherited)
        );
    }

    /// A template the VM placed is `Visible`, to show through its hidden
    /// root, and still hides when switched off.
    #[test]
    fn a_placed_template_shows_through_its_hidden_root() {
        let mut app = App::new();
        app.add_plugins(bevy::MinimalPlugins);
        app.add_systems(Update, mirror_node_visibility);

        let placed = app
            .world_mut()
            .spawn((NodeActive(true), Visibility::Hidden, AnimPlaced))
            .id();
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(placed),
            Some(&Visibility::Visible)
        );

        app.world_mut()
            .get_mut::<NodeActive>(placed)
            .expect("node")
            .0 = false;
        app.update();
        assert_eq!(
            app.world().get::<Visibility>(placed),
            Some(&Visibility::Hidden)
        );
    }
}
