//! `MaterialDesc` -> `StandardMaterial`, drawn as a
//! [`ModelMaterial`](crate::cutaway::ModelMaterial), plus the two texture
//! animations.
//!
//! [`build_material`] is what the level spawner calls once per
//! (model, material) group. Identical requests share one handle through
//! [`MaterialCache`]; animated ones deliberately do not, because both
//! animations mutate the material they are attached to.
//!
//! # Where the animations come from
//!
//! Not from `gamez`: no material in any Recoil level has `CycleData` and every
//! model's `texture_scroll` is zero. Both come from the runtime interp script,
//! as [`bft_assets::interp::runtime_bindings`] extracts it
//! (docs/data/materials.md and docs/data/interp.md). The level spawner matches
//! its bindings to nodes by name and attaches
//! [`CycledTexture`]/[`TextureScroll`] to the entities it spawns;
//! `claim_animated_parts` hands them on to the mesh parts that draw them.
//!
//! The sprite effects are the other source: `effects.zrd` gives a cycle to
//! a *texture* — `fire101.tif` steps through `fire112.tif` wherever it is
//! drawn — and [`EffectTextureCycles`] is that table, which `attach_model`
//! consults for every node it draws, clones and anim copies included.

use std::hash::{BuildHasher, Hash, Hasher};

use bevy::platform::collections::HashMap;
use bevy::platform::hash::FixedHasher;
use bevy::prelude::*;
use bft_assets::materials::MaterialDesc;
use bft_assets::reader::ReaderData;
use bft_assets::reader::effects::{EffectCycles, parse_effect_cycles};
use bft_assets::textures::{TextureAlpha, TexturePool, TextureSet};

use crate::cutaway::{ModelMaterial, model_material};

/// Presentation-side materials: the cache, the animations and their systems.
pub struct MaterialsPlugin;

impl Plugin for MaterialsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MaterialCache>().add_systems(
            Update,
            // Presentation, so `Time` (real/virtual) is right; the 60 Hz
            // fixed step is for the sim and would make these judder.
            (
                read_effect_cycles,
                claim_animated_parts,
                (advance_cycled_textures, advance_texture_scroll),
            )
                .chain(),
        );
    }
}

/// `effects.zrd`'s sprite cycles, read from the global reader archive.
///
/// Without these every flame and smoke puff the anim scripts place —
/// `fire_hulk.flt` burning on a wreck for 80 s, `smoke_destroy.flt` rising
/// off it — sat on its first frame for its whole life, because no interp
/// binding names those nodes: the original cycled them by texture.
#[derive(Resource, Debug, Clone, Default)]
pub struct EffectTextureCycles(pub EffectCycles);

impl EffectTextureCycles {
    /// The cycle a material showing `texture` runs, resolved to images.
    ///
    /// `None` when the texture opens no cycle, or none of its frames can be
    /// found in any archive.
    pub fn cycle_for(
        &self,
        texture: &str,
        pool: &TexturePool,
        sets: &Assets<TextureSet>,
    ) -> Option<CycledTexture> {
        let cycle = self.0.for_texture(texture)?;
        let frames: Vec<Handle<Image>> = cycle
            .maps
            .iter()
            .filter_map(|frame| pool.image(frame, sets))
            .collect();

        (!frames.is_empty()).then(|| CycledTexture::new(frames, cycle.speed, cycle.looping))
    }
}

/// Build [`EffectTextureCycles`] once the global reader archive has landed.
///
/// Boot loads the globals before any level, so the table is in place by the
/// time the first model is drawn. An archive with no `effects.zrd` leaves an
/// empty table, and the sprites simply do not animate.
fn read_effect_cycles(
    mut commands: Commands,
    existing: Option<Res<EffectTextureCycles>>,
    globals: Option<Res<crate::globals::Globals>>,
    readers: Res<Assets<ReaderData>>,
) {
    if existing.is_some() {
        return;
    }
    let Some(reader) = globals
        .as_ref()
        .and_then(|globals| globals.reader.as_ref())
        .and_then(|handle| readers.get(handle))
    else {
        return;
    };

    let cycles = parse_effect_cycles(reader).unwrap_or_else(|| {
        warn!("the global reader has no effects.zrd; effect sprites will not animate");
        EffectCycles::default()
    });
    info!("{} effect sprite cycle(s)", cycles.len());
    commands.insert_resource(EffectTextureCycles(cycles));
}

/// The Bevy `AlphaMode` a texture's [`TextureAlpha`] calls for.
///
/// `Simple` is one-bit cutout, which `Mask(0.5)` reproduces. Only
/// `image.zbd`'s UI textures use it; level textures are `None` or `Full`
/// (docs/data/materials.md).
pub fn alpha_mode(alpha: TextureAlpha) -> AlphaMode {
    match alpha {
        TextureAlpha::None => AlphaMode::Opaque,
        TextureAlpha::Simple => AlphaMode::Mask(0.5),
        TextureAlpha::Full => AlphaMode::Blend,
    }
}

/// Everything that distinguishes one built material from another.
///
/// The flags come from the *model*, not the material, which is why they are
/// parameters rather than part of [`MaterialDesc`]: the same material on a
/// lit and an unlit model is two Bevy materials (DESIGN §6).
#[derive(Debug, Clone, Copy)]
pub struct MaterialParams<'a> {
    pub desc: &'a MaterialDesc,
    /// The model's `LIGHTING` flag. `false` makes the material `unlit`.
    pub lighting: bool,
    /// The model's `FOG` flag.
    pub fog: bool,
    /// The model's `SHOW_BACKFACE` flag: double-sided, no culling.
    pub show_backface: bool,
    /// UV units per second, from an interp `Object3DSetScroll` binding.
    /// `Some` makes the material unshareable, since the scroll mutates it.
    pub texture_scroll: Option<Vec2>,
}

/// Built materials, keyed by a hash of the [`MaterialParams`] that made them.
///
/// Levels reuse a handful of materials across thousands of polygons (m1: 512
/// materials, ~18k polygons), so sharing is worth the hashing.
#[derive(Resource, Debug, Default)]
pub struct MaterialCache {
    entries: HashMap<u64, Handle<ModelMaterial>>,
}

impl MaterialCache {
    /// How many distinct materials have been built.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Forget every entry. The level loader calls this when a level is unloaded, so
    /// the handles drop with it.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

/// Build (or reuse) the material for one material description.
///
/// `set` is the level's own texture archive; a name it does not have is
/// looked up in `pool`, because a level references textures held only in
/// *other* missions' archives (docs/data/materials.md). A name in neither is a
/// warning and an untextured material, not a failure — a missing texture
/// should not cost the level a wall.
pub fn build_material(
    params: MaterialParams,
    set: &TextureSet,
    pool: Option<(&TexturePool, &Assets<TextureSet>)>,
    cache: &mut MaterialCache,
    materials: &mut Assets<ModelMaterial>,
) -> Handle<ModelMaterial> {
    let shareable = is_shareable(&params);
    let key = cache_key(&params);

    if shareable && let Some(handle) = cache.entries.get(&key) {
        return handle.clone();
    }

    let handle = materials.add(model_material(standard_material(&params, set, pool)));

    if shareable {
        cache.entries.insert(key, handle.clone());
    }

    handle
}

/// Whether two identical requests may share one handle.
///
/// An animated material is mutated in place by its system, so sharing one
/// would animate every entity that asked for it.
fn is_shareable(params: &MaterialParams) -> bool {
    params.texture_scroll.is_none()
        && !matches!(params.desc, MaterialDesc::Textured { cycle: Some(_), .. })
}

/// Turn one description plus its model flags into a Bevy material.
fn standard_material(
    params: &MaterialParams,
    set: &TextureSet,
    pool: Option<(&TexturePool, &Assets<TextureSet>)>,
) -> StandardMaterial {
    let mut material = StandardMaterial {
        unlit: !params.lighting,
        fog_enabled: params.fog,
        double_sided: params.show_backface,
        cull_mode: if params.show_backface {
            None
        } else {
            Some(bevy::render::render_resource::Face::Back)
        },
        // The original is a 1999 rasteriser with no notion of either, and
        // PBR defaults make every surface look wet.
        perceptual_roughness: 1.0,
        metallic: 0.0,
        reflectance: 0.0,
        ..default()
    };

    match params.desc {
        MaterialDesc::Textured { texture, .. } => {
            match lookup(texture, set, pool) {
                Some((image, alpha)) => {
                    material.base_color_texture = Some(image);
                    material.alpha_mode = alpha_mode(alpha);
                }
                None => {
                    warn!("no texture named {texture} in the level's archive or the pool");
                    // Visible but not invisible: an untextured white surface
                    // reads as "missing texture" rather than as a hole.
                    material.alpha_mode = AlphaMode::Opaque;
                }
            }
        }
        MaterialDesc::Colored { color, alpha, .. } => {
            let alpha = f32::from(*alpha) / 255.0;
            material.base_color =
                Color::LinearRgba(LinearRgba::new(color[0], color[1], color[2], alpha));
            // Anything short of fully opaque blends.
            material.alpha_mode = if alpha < 1.0 {
                AlphaMode::Blend
            } else {
                AlphaMode::Opaque
            };
        }
    }

    // A scrolled material starts at the texture origin; `advance_texture_scroll`
    // writes `uv_transform.translation` from the `TextureScroll` component
    // every frame, so there is nothing to seed here.

    material
}

/// Find a texture in the level's own archive, then in the cross-mission pool.
fn lookup(
    name: &str,
    set: &TextureSet,
    pool: Option<(&TexturePool, &Assets<TextureSet>)>,
) -> Option<(Handle<Image>, TextureAlpha)> {
    if let (Some(image), Some(info)) = (set.image(name), set.info(name)) {
        return Some((image.clone(), info.alpha));
    }

    let (pool, sets) = pool?;
    let (image, info) = pool.resolve(name, sets)?;

    Some((image, info.alpha))
}

/// Hash the parameters into a cache key.
///
/// `f32`s are hashed by their bits: two materials whose colours differ only
/// by a NaN payload are not a case this data produces, and bit equality is
/// exactly the sharing rule we want.
fn cache_key(params: &MaterialParams) -> u64 {
    let mut hasher = FixedHasher.build_hasher();

    params.lighting.hash(&mut hasher);
    params.fog.hash(&mut hasher);
    params.show_backface.hash(&mut hasher);

    match params.desc {
        MaterialDesc::Textured {
            texture,
            soil,
            cycle,
            flag,
        } => {
            0u8.hash(&mut hasher);
            texture.hash(&mut hasher);
            soil.hash(&mut hasher);
            flag.hash(&mut hasher);
            // Cycled materials are never shared, so the frames need not be
            // in the key; whether there *is* a cycle still is.
            cycle.is_some().hash(&mut hasher);
        }
        MaterialDesc::Colored { color, alpha, soil } => {
            1u8.hash(&mut hasher);
            for component in color {
                component.to_bits().hash(&mut hasher);
            }
            alpha.hash(&mut hasher);
            soil.hash(&mut hasher);
        }
    }

    match params.texture_scroll {
        Some(scroll) => {
            1u8.hash(&mut hasher);
            scroll.x.to_bits().hash(&mut hasher);
            scroll.y.to_bits().hash(&mut hasher);
        }
        None => 0u8.hash(&mut hasher),
    }

    hasher.finish()
}

/// A material whose base colour texture cycles through a frame list.
///
/// Attach next to the `MeshMaterial3d` the frames belong to. The material is
/// mutated in place, which is why [`build_material`] never shares a cycled
/// one.
#[derive(Component, Debug, Clone)]
pub struct CycledTexture {
    pub frames: Vec<Handle<Image>>,
    /// Frames per second. `0.0` means the game advances [`Self::frame`]
    /// itself — the player's tank tracks do (docs/data/interp.md) — so the system
    /// leaves the timing alone but still writes the frame out.
    pub speed: f32,
    /// When false, the cycle stops on the last frame instead of wrapping.
    pub looping: bool,
    /// The frame currently shown.
    pub frame: usize,
    /// Seconds accumulated towards the next frame.
    pub acc: f32,
}

impl CycledTexture {
    /// A cycle sitting on its first frame.
    pub fn new(frames: Vec<Handle<Image>>, speed: f32, looping: bool) -> CycledTexture {
        CycledTexture {
            frames,
            speed,
            looping,
            frame: 0,
            acc: 0.0,
        }
    }

    /// Advance by `delta` seconds and say whether the frame changed.
    ///
    /// Split out from the system so the arithmetic is testable without an
    /// `App`. Handles a `delta` spanning several frames (a stalled frame
    /// must not desynchronise a looping animation) and stops cleanly on the
    /// last frame when not looping.
    pub fn advance(&mut self, delta: f32) -> bool {
        if self.frames.is_empty() || self.speed <= 0.0 {
            return false;
        }
        if !self.looping && self.frame + 1 >= self.frames.len() {
            return false;
        }

        self.acc += delta;

        let step = 1.0 / self.speed;
        let steps = (self.acc / step).floor();
        if steps < 1.0 {
            return false;
        }
        self.acc -= steps * step;

        let before = self.frame;
        let advanced = self.frame + steps as usize;

        self.frame = if self.looping {
            advanced % self.frames.len()
        } else {
            // Stop on the last frame and stay there.
            let last = self.frames.len() - 1;
            self.acc = 0.0;
            advanced.min(last)
        };

        self.frame != before
    }

    /// The handle the material should be showing.
    pub fn current(&self) -> Option<&Handle<Image>> {
        self.frames.get(self.frame)
    }
}

/// A material whose UVs scroll, from an interp `Object3DSetScroll` binding.
#[derive(Component, Debug, Clone, Copy)]
pub struct TextureScroll {
    /// UV units per second.
    pub rate: Vec2,
    /// Accumulated offset, kept in `[0,1)` so it never loses precision in a
    /// long session. Textures sample with `Repeat`, so wrapping is invisible.
    pub offset: Vec2,
}

impl TextureScroll {
    /// A scroll starting at the texture's origin.
    pub fn new(rate: Vec2) -> TextureScroll {
        TextureScroll {
            rate,
            offset: Vec2::ZERO,
        }
    }

    /// Advance by `delta` seconds and return the new offset.
    pub fn advance(&mut self, delta: f32) -> Vec2 {
        self.offset = (self.offset + self.rate * delta).fract_gl();

        self.offset
    }

    /// Set the offset to where a steady scroll is after `seconds`, and
    /// return it: every part on the same clock and rate agrees.
    pub fn at(&mut self, seconds: f64) -> Vec2 {
        self.offset = (self.rate.as_dvec2() * seconds).fract_gl().as_vec2();

        self.offset
    }
}

/// A part whose [`TextureScroll`] shares its material with others scrolling
/// the same way, and so takes its offset from the clock
/// ([`TextureScroll::at`]) rather than adding up its own.
#[derive(Component, Debug, Clone, Copy)]
pub struct ClockScroll;

/// A model part that has been given its own copy of its node's material, so
/// the node's texture animation can mutate it.
///
/// The frame the part's material last showed, for a cycle; `None` for a
/// scroll, or before the first write.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct AnimatedPart {
    shown: Option<usize>,
}

/// Hand a node's texture animation to the model parts that draw it.
///
/// The interp bindings name *nodes*, and the level spawner attaches
/// [`CycledTexture`]/[`TextureScroll`] to the node's entity. But a node's
/// meshes are its [`ModelPart`](crate::mesh_builder::ModelPart) children, one
/// per material group, and their materials come from the shared
/// [`MaterialCache`]. So until this ran, the two advance systems — which want
/// the animation and the `MeshMaterial3d` on one entity — matched nothing,
/// and no interp texture animation in the game played: not the water, not
/// the lava, not the player's tracks.
///
/// Each part gets a copy of the animation and a copy of the material, so
/// animating one node does not animate every surface sharing its material;
/// parts that only scroll, steadily and alike, share one copy between them
/// ([`ClockScroll`]). A cycle goes only on the parts showing one of its frames: the
/// bindings match by name (`rtracks` is also a node of every enemy tank,
/// with a different texture), and a node can mix a cycled texture with
/// fixed ones.
///
/// Polled rather than run once, because a node's parts arrive from the
/// `attach_model` observer and a clone's arrive later still.
#[allow(clippy::type_complexity)]
fn claim_animated_parts(
    mut commands: Commands,
    mut shared: Local<HashMap<(AssetId<ModelMaterial>, [u32; 2]), Handle<ModelMaterial>>>,
    mut materials: ResMut<Assets<ModelMaterial>>,
    nodes: Query<
        (Option<&CycledTexture>, Option<&TextureScroll>, &Children),
        (
            Or<(With<CycledTexture>, With<TextureScroll>)>,
            Without<crate::mesh_builder::ModelPart>,
        ),
    >,
    parts: Query<
        &MeshMaterial3d<ModelMaterial>,
        (With<crate::mesh_builder::ModelPart>, Without<AnimatedPart>),
    >,
) {
    for (cycle, scroll, children) in &nodes {
        for child in children.iter() {
            let Ok(material) = parts.get(child) else {
                continue;
            };
            let Some(source) = materials.get(&material.0).cloned() else {
                continue;
            };
            let cycle = cycle.filter(|cycle| {
                source
                    .base
                    .base_color_texture
                    .as_ref()
                    .is_some_and(|texture| cycle.frames.contains(texture))
            });
            if cycle.is_none() && scroll.is_none() {
                // Not this binding's surface; never look at it again.
                commands.entity(child).insert(AnimatedPart::default());
                continue;
            }

            // A steady scroll alone (water, lava) shares one copy with every
            // part scrolling the same material at the same rate, and takes
            // its offset from the clock so they all agree: each copy is
            // prepared for the GPU again whenever it moves, which is every
            // frame. A scroll that starts still is the player rig's, which
            // changes its rate as the tracks run, so it keeps its own.
            let steady = scroll.filter(|scroll| cycle.is_none() && scroll.rate != Vec2::ZERO);
            let copy = match steady {
                Some(scroll) => shared
                    .entry((material.0.id(), scroll.rate.to_array().map(f32::to_bits)))
                    .or_insert_with(|| materials.add(source))
                    .clone(),
                None => materials.add(source),
            };
            let mut part = commands.entity(child);
            part.insert((AnimatedPart::default(), MeshMaterial3d(copy)));
            if let Some(cycle) = cycle {
                part.insert(cycle.clone());
            }
            if let Some(scroll) = scroll {
                part.insert(*scroll);
            }
            if steady.is_some() {
                part.insert(ClockScroll);
            }
        }
    }
}

/// Swap each cycled material's `base_color_texture` as its frame changes.
///
/// Writes whenever the frame differs from what the material shows, not only
/// when the cycle's own clock turns it: a zero-speed cycle is advanced by the
/// game (the tracks) or rewound by the anim VM (`ObjectCycleTexture`), and
/// both set [`CycledTexture::frame`] directly.
fn advance_cycled_textures(
    time: Res<Time>,
    mut materials: ResMut<Assets<ModelMaterial>>,
    mut cycled: Query<(
        &mut CycledTexture,
        &MeshMaterial3d<ModelMaterial>,
        Option<&mut AnimatedPart>,
    )>,
) {
    let delta = time.delta_secs();

    for (mut cycle, material, part) in &mut cycled {
        let turned = cycle.advance(delta);
        let stale = part
            .as_ref()
            .is_some_and(|part| part.shown != Some(cycle.frame));
        if !turned && !stale {
            continue;
        }
        let Some(frame) = cycle.current().cloned() else {
            continue;
        };
        let Some(mut material) = materials.get_mut(&material.0) else {
            continue;
        };

        material.base.base_color_texture = Some(frame);
        if let Some(mut part) = part {
            part.shown = Some(cycle.frame);
        }
    }
}

/// Advance each scrolling material's `uv_transform`.
///
/// Writes only an offset that moved: a write makes Bevy prepare the material
/// for the GPU again, and a scroll standing still (the tracks at rest, rate
/// zero) would otherwise cost that every frame.
fn advance_texture_scroll(
    time: Res<Time>,
    mut materials: ResMut<Assets<ModelMaterial>>,
    mut scrolling: Query<(
        &mut TextureScroll,
        &MeshMaterial3d<ModelMaterial>,
        Has<ClockScroll>,
    )>,
) {
    let delta = time.delta_secs();

    for (mut scroll, material, clock) in &mut scrolling {
        let offset = if clock {
            scroll.at(time.elapsed_secs_f64())
        } else {
            scroll.advance(delta)
        };
        if materials
            .get(&material.0)
            .is_none_or(|current| current.base.uv_transform.translation == offset)
        {
            continue;
        }
        if let Some(mut material) = materials.get_mut(&material.0) {
            material.base.uv_transform.translation = offset;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bft_assets::materials::Soil;

    fn textured(name: &str) -> MaterialDesc {
        MaterialDesc::Textured {
            texture: name.to_string(),
            soil: Soil::Default,
            cycle: None,
            flag: false,
        }
    }

    fn params(desc: &MaterialDesc) -> MaterialParams<'_> {
        MaterialParams {
            desc,
            lighting: true,
            fog: true,
            show_backface: false,
            texture_scroll: None,
        }
    }

    #[test]
    fn alpha_maps_to_the_three_bevy_modes() {
        assert_eq!(alpha_mode(TextureAlpha::None), AlphaMode::Opaque);
        assert_eq!(alpha_mode(TextureAlpha::Simple), AlphaMode::Mask(0.5));
        assert_eq!(alpha_mode(TextureAlpha::Full), AlphaMode::Blend);
    }

    #[test]
    fn identical_requests_share_a_handle_and_different_ones_do_not() {
        let mut cache = MaterialCache::default();
        let mut materials = Assets::<ModelMaterial>::default();
        let set = TextureSet::default();
        let desc = textured("wall");

        let first = build_material(params(&desc), &set, None, &mut cache, &mut materials);
        let second = build_material(params(&desc), &set, None, &mut cache, &mut materials);
        assert_eq!(first, second);
        assert_eq!(cache.len(), 1);

        // The model flags are part of the identity, not just the material.
        let unlit = MaterialParams {
            lighting: false,
            ..params(&desc)
        };
        let third = build_material(unlit, &set, None, &mut cache, &mut materials);
        assert_ne!(first, third);
        assert_eq!(cache.len(), 2);
    }

    #[test]
    fn animated_materials_are_never_shared() {
        let mut cache = MaterialCache::default();
        let mut materials = Assets::<ModelMaterial>::default();
        let set = TextureSet::default();

        let cycled = MaterialDesc::Textured {
            texture: "surf01".to_string(),
            soil: Soil::Default,
            cycle: Some(bft_assets::materials::CycleDesc {
                textures: vec!["surf01".to_string(), "surf02".to_string()],
                looping: true,
                speed: 6.0,
            }),
            flag: false,
        };

        let first = build_material(params(&cycled), &set, None, &mut cache, &mut materials);
        let second = build_material(params(&cycled), &set, None, &mut cache, &mut materials);
        assert_ne!(first, second, "a cycled material is mutated in place");

        // Same for a scrolled one.
        let plain = textured("hscroll");
        let scrolled = MaterialParams {
            texture_scroll: Some(Vec2::new(0.0, 3.5)),
            ..params(&plain)
        };
        let third = build_material(scrolled, &set, None, &mut cache, &mut materials);
        let fourth = build_material(scrolled, &set, None, &mut cache, &mut materials);
        assert_ne!(third, fourth);

        assert!(cache.is_empty(), "nothing animated enters the cache");
    }

    #[test]
    fn model_flags_reach_the_material() {
        let desc = textured("wall");
        let material = standard_material(
            &MaterialParams {
                lighting: false,
                fog: false,
                show_backface: true,
                ..params(&desc)
            },
            &TextureSet::default(),
            None,
        );

        assert!(material.unlit);
        assert!(!material.fog_enabled);
        assert!(material.double_sided);
        assert_eq!(material.cull_mode, None);
    }

    #[test]
    fn a_translucent_colour_blends_and_an_opaque_one_does_not() {
        let translucent = MaterialDesc::Colored {
            color: [1.0, 0.0, 0.0],
            alpha: 127,
            soil: Soil::Default,
        };
        let opaque = MaterialDesc::Colored {
            color: [1.0, 0.0, 0.0],
            alpha: 255,
            soil: Soil::Default,
        };
        let set = TextureSet::default();

        let built = standard_material(&params(&translucent), &set, None);
        assert_eq!(built.alpha_mode, AlphaMode::Blend);
        assert!((built.base_color.alpha() - 127.0 / 255.0).abs() < 1e-6);

        assert_eq!(
            standard_material(&params(&opaque), &set, None).alpha_mode,
            AlphaMode::Opaque
        );
    }

    #[test]
    fn a_missing_texture_yields_a_material_rather_than_nothing() {
        // The pool cannot help either: neither has the name.
        let desc = textured("does_not_exist");

        let material = standard_material(&params(&desc), &TextureSet::default(), None);

        assert!(material.base_color_texture.is_none());
        assert_eq!(material.alpha_mode, AlphaMode::Opaque);
    }

    #[test]
    fn a_cycle_advances_one_frame_per_period() {
        let frames = vec![Handle::default(), Handle::default(), Handle::default()];
        // 6 fps: a frame every 1/6 s.
        let mut cycle = CycledTexture::new(frames, 6.0, true);

        assert!(!cycle.advance(0.1), "not a whole frame yet");
        assert_eq!(cycle.frame, 0);

        assert!(cycle.advance(0.1), "0.2s is past 1/6s");
        assert_eq!(cycle.frame, 1);

        // The remainder carries: 0.2 - 1/6 is already banked.
        assert!(cycle.advance(0.134));
        assert_eq!(cycle.frame, 2);
    }

    #[test]
    fn a_looping_cycle_wraps_and_a_long_delta_does_not_desynchronise_it() {
        let frames = vec![Handle::default(), Handle::default(), Handle::default()];
        let mut cycle = CycledTexture::new(frames, 10.0, true);

        // A one-second stall at 10 fps is 10 frames: 10 % 3 == 1.
        assert!(cycle.advance(1.0));
        assert_eq!(cycle.frame, 1);
    }

    #[test]
    fn a_non_looping_cycle_stops_on_its_last_frame() {
        let frames = vec![Handle::default(), Handle::default(), Handle::default()];
        let mut cycle = CycledTexture::new(frames, 10.0, false);

        assert!(cycle.advance(1.0), "overshoots to the end");
        assert_eq!(cycle.frame, 2);

        assert!(!cycle.advance(10.0), "and stays there");
        assert_eq!(cycle.frame, 2);
    }

    #[test]
    fn a_zero_speed_cycle_never_advances_itself() {
        // The tank tracks: the vehicle sets `frame`, not the clock.
        let frames = vec![Handle::default(), Handle::default()];
        let mut cycle = CycledTexture::new(frames, 0.0, false);

        assert!(!cycle.advance(100.0));
        assert_eq!(cycle.frame, 0);
    }

    #[test]
    fn an_empty_cycle_is_harmless() {
        let mut cycle = CycledTexture::new(Vec::new(), 10.0, true);

        assert!(!cycle.advance(1.0));
        assert!(cycle.current().is_none());
    }

    #[test]
    fn scroll_accumulates_and_wraps_into_the_unit_square() {
        let mut scroll = TextureScroll::new(Vec2::new(0.0, 12.0));

        assert_eq!(
            scroll.advance(0.5),
            Vec2::new(0.0, 0.0),
            "12 * 0.5 = 6.0, wraps to 0"
        );

        let mut slow = TextureScroll::new(Vec2::new(0.5, 0.0));
        assert_eq!(slow.advance(1.0), Vec2::new(0.5, 0.0));
        assert_eq!(slow.advance(1.0), Vec2::new(0.0, 0.0));
    }

    #[test]
    fn clock_scrolls_agree_however_late_they_start() {
        let mut early = TextureScroll::new(Vec2::new(0.25, 0.5));
        let mut late = TextureScroll::new(Vec2::new(0.25, 0.5));
        early.advance(3.0);
        assert_eq!(early.at(10.0), late.at(10.0));
        assert_eq!(late.at(10.0), Vec2::new(0.5, 0.0));
    }
}
