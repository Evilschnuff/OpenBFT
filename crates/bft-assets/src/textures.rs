//! Texture archives (`.tex.zip`) as Bevy images.
//!
//! One `.tex.zip` holds a `manifest.json` and one `<name>.png` per texture at
//! the ZIP root. [`TextureSetLoader`] decodes every PNG into a labelled
//! sub-asset and returns a [`TextureSet`] keyed by the archive's own names.
//!
//! # Names
//!
//! Archives store bare lowercase stems (`dockp01k`); a level's
//! `textures.json` stores source names with an extension (`dockp01k.tif`).
//! [`texture_key`] is the one place that difference is resolved — call it on
//! anything coming out of `gamez` or an interp script before looking it up.
//!
//! # Why a pool
//!
//! A level references textures its own mission's archive does not have: 27 of
//! m1's 471 (effects and HUD — `ammolaser`, `lockon01`…) live in *other*
//! missions' archives (docs/data/materials.md). [`TexturePool`] is the fallback chain
//! that makes those resolve.

use std::io::{Cursor, Read};

use bevy::asset::{AssetLoader, LoadContext, RenderAssetUsages, io::Reader};
use bevy::image::{
    ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor, ImageType,
};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::mech3ax;

pub use crate::mech3ax::{TextureAlpha, TextureStretch};

/// The file inside a `.tex.zip` holding the [`mech3ax::TextureManifest`].
const MANIFEST_ENTRY: &str = "manifest.json";

/// One extracted texture archive.
///
/// `images` and `info` share their keys, which are the archive's own names:
/// bare, lowercase, no extension.
#[derive(Asset, TypePath, Debug, Default)]
pub struct TextureSet {
    /// The decoded PNGs, as labelled sub-assets of the archive.
    pub images: HashMap<String, Handle<Image>>,
    /// What the manifest said about each one.
    pub info: HashMap<String, TextureInfo>,
}

impl TextureSet {
    /// Look up an image by a name from `gamez` or an interp script.
    ///
    /// Normalises the name with [`texture_key`] first, so `"dockp01k.tif"`
    /// and `"dockp01k"` both work.
    pub fn image(&self, name: &str) -> Option<&Handle<Image>> {
        self.images.get(&texture_key(name))
    }

    /// Look up a texture's metadata by a name from `gamez` or interp.
    pub fn info(&self, name: &str) -> Option<&TextureInfo> {
        self.info.get(&texture_key(name))
    }
}

/// What the manifest records about one texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureInfo {
    pub width: u16,
    pub height: u16,
    /// Drives the material's `AlphaMode`; `bft_game::materials::alpha_mode`
    /// does the mapping, since `AlphaMode` is a renderer type.
    pub alpha: TextureAlpha,
    /// The original hardware's stretch flag. Recorded only: mech3ax emits
    /// every PNG at the declared size whatever this says, so applying it as
    /// a UV scale would double-count what the geometry's UVs already encode
    /// (docs/data/materials.md, verified on all 576 m1 entries).
    pub stretch: TextureStretch,
}

/// Normalise a texture name to the key a [`TextureSet`] uses.
///
/// Strips one trailing extension and lowercases. Recoil's data is already
/// lowercase everywhere (verified across all 13 missions), so the lowercasing
/// is belt-and-braces for a differently-cased re-extraction.
///
/// ```
/// # use bft_assets::textures::texture_key;
/// assert_eq!(texture_key("dockp01k.tif"), "dockp01k");
/// assert_eq!(texture_key("lflare1"), "lflare1");
/// ```
pub fn texture_key(name: &str) -> String {
    let stem = match name.rsplit_once('.') {
        Some((stem, _ext)) if !stem.is_empty() => stem,
        _ => name,
    };

    stem.to_ascii_lowercase()
}

/// How textures are filtered when magnified (FR-W3's settings toggle).
///
/// The original was point-sampled at low resolution; `Linear` looks better on
/// a modern display and is the default DESIGN §5.2 asks for.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct TextureFilter(pub ImageFilterMode);

impl Default for TextureFilter {
    fn default() -> Self {
        TextureFilter(ImageFilterMode::Linear)
    }
}

/// The sampler every texture in an archive gets.
///
/// `Repeat` in both directions, not Bevy's `ClampToEdge` default: level
/// geometry tiles its textures and scrolled UVs (docs/data/interp.md) run off the
/// edge on purpose, both of which clamping turns into a smear.
fn sampler(filter: ImageFilterMode) -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        label: None,
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        address_mode_w: ImageAddressMode::Repeat,
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter: filter,
        ..default()
    })
}

/// Loads a `.tex.zip` into a [`TextureSet`].
///
/// The filter is fixed when the loader is registered rather than read from
/// [`TextureFilter`]: an `AssetLoader` has no access to the world. Changing
/// the setting at runtime therefore means re-registering and reloading, or
/// rewriting the samplers of the loaded images, as the settings screen does.
#[derive(TypePath)]
pub struct TextureSetLoader {
    filter: ImageFilterMode,
}

impl TextureSetLoader {
    /// A loader that samples with the given filter.
    pub fn new(filter: ImageFilterMode) -> TextureSetLoader {
        TextureSetLoader { filter }
    }
}

impl Default for TextureSetLoader {
    fn default() -> Self {
        TextureSetLoader::new(TextureFilter::default().0)
    }
}

/// Why a `.tex.zip` would not load.
#[derive(Debug, thiserror::Error)]
pub enum TextureSetError {
    #[error("cannot read the archive: {0}")]
    Read(#[from] std::io::Error),
    #[error("not a readable ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("no {MANIFEST_ENTRY} in the archive; is this a texture archive?")]
    NoManifest,
    #[error("{MANIFEST_ENTRY} will not parse: {0}")]
    Manifest(#[from] serde_json::Error),
}

impl AssetLoader for TextureSetLoader {
    type Asset = TextureSet;
    type Settings = ();
    type Error = TextureSetError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        load_context: &mut LoadContext<'_>,
    ) -> Result<TextureSet, TextureSetError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;

        let manifest: mech3ax::TextureManifest = {
            let mut entry = archive
                .by_name(MANIFEST_ENTRY)
                .map_err(|_| TextureSetError::NoManifest)?;
            let mut json = String::new();
            entry.read_to_string(&mut json)?;
            serde_json::from_str(&json)?
        };

        let mut set = TextureSet::default();
        let mut skipped = 0usize;

        for entry in &manifest.texture_infos {
            // `rename` is mech3ax's "the archive calls it X but the game
            // asks for Y"; Recoil never sets it, but honouring it costs a
            // line and keeps the key consistent with what a level asks for.
            let name = entry.rename.as_deref().unwrap_or(&entry.name);
            let key = texture_key(name);

            let Some(png) = read_entry(&mut archive, &format!("{}.png", entry.name)) else {
                // A manifest entry without its PNG is one missing texture,
                // not a broken level: warn and carry on.
                warn!(
                    "{}: manifest lists {} but the archive has no {}.png; skipping",
                    load_context.path(),
                    entry.name,
                    entry.name
                );
                skipped += 1;
                continue;
            };

            let image = match Image::from_buffer(
                &png,
                ImageType::Extension("png"),
                default(),
                // These are colour textures, so sRGB.
                true,
                sampler(self.filter),
                RenderAssetUsages::default(),
            ) {
                Ok(image) => image,
                Err(e) => {
                    warn!(
                        "{}: {}.png will not decode ({e}); skipping",
                        load_context.path(),
                        entry.name
                    );
                    skipped += 1;
                    continue;
                }
            };

            let handle = load_context.add_labeled_asset(key.clone(), image);
            set.images.insert(key.clone(), handle);
            set.info.insert(
                key,
                TextureInfo {
                    width: entry.width,
                    height: entry.height,
                    alpha: entry.alpha,
                    stretch: entry.stretch,
                },
            );
        }

        if skipped > 0 {
            warn!(
                "{}: loaded {} of {} textures, {skipped} skipped",
                load_context.path(),
                set.images.len(),
                manifest.texture_infos.len()
            );
        }

        Ok(set)
    }

    fn extensions(&self) -> &[&str] {
        // Bevy matches the longest known extension, so this claims
        // `foo.tex.zip` without claiming every `.zip`.
        &["tex.zip"]
    }
}

/// Read one ZIP entry whole, or `None` when it is absent or unreadable.
fn read_entry<R: std::io::Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
    name: &str,
) -> Option<Vec<u8>> {
    let mut entry = archive.by_name(name).ok()?;
    let mut bytes = Vec::with_capacity(entry.size() as usize);
    entry.read_to_end(&mut bytes).ok()?;

    Some(bytes)
}

/// The archives a lookup searches, in order.
///
/// A level's own `rtexture4` first, then every other mission's, then
/// `image.zbd`'s UI set. docs/data/materials.md: 27 of m1's references and 6 of the
/// interp cycle frames are only findable this way, so a single-archive
/// lookup is not enough.
#[derive(Resource, Debug, Default)]
pub struct TexturePool {
    sets: Vec<Handle<TextureSet>>,
}

impl TexturePool {
    /// A pool searching the given archives in order.
    pub fn new(sets: impl IntoIterator<Item = Handle<TextureSet>>) -> TexturePool {
        TexturePool {
            sets: sets.into_iter().collect(),
        }
    }

    /// Put an archive at the front of the search order.
    ///
    /// The level loader calls this with the level's own archive on level load, so
    /// a mission that has its own copy of a shared name wins.
    pub fn prefer(&mut self, set: Handle<TextureSet>) {
        self.sets.retain(|existing| *existing != set);
        self.sets.insert(0, set);
    }

    /// Add an archive at the end of the search order.
    pub fn push(&mut self, set: Handle<TextureSet>) {
        if !self.sets.contains(&set) {
            self.sets.push(set);
        }
    }

    /// The archives, in search order.
    pub fn sets(&self) -> &[Handle<TextureSet>] {
        &self.sets
    }

    /// Find a texture by a `gamez` or interp name, searching every archive.
    pub fn image(&self, name: &str, sets: &Assets<TextureSet>) -> Option<Handle<Image>> {
        self.resolve(name, sets).map(|(handle, _)| handle)
    }

    /// Find a texture and its metadata by a `gamez` or interp name.
    pub fn resolve(
        &self,
        name: &str,
        sets: &Assets<TextureSet>,
    ) -> Option<(Handle<Image>, TextureInfo)> {
        let key = texture_key(name);

        self.sets.iter().find_map(|handle| {
            let set = sets.get(handle)?;
            let image = set.images.get(&key)?;
            let info = set.info.get(&key)?;

            Some((image.clone(), *info))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_extension_is_stripped_and_the_name_lowercased() {
        assert_eq!(texture_key("dockp01k.tif"), "dockp01k");
        assert_eq!(texture_key("DOCKP01K.TIF"), "dockp01k");
        // The four lens-flare names have no extension at all.
        assert_eq!(texture_key("lflare1"), "lflare1");
        // Only the last extension goes, and a leading dot is not one.
        assert_eq!(texture_key("ash_dirt.tif"), "ash_dirt");
        assert_eq!(texture_key(".hidden"), ".hidden");
    }

    #[test]
    fn the_default_filter_is_linear() {
        assert_eq!(TextureFilter::default().0, ImageFilterMode::Linear);
    }

    #[test]
    fn textures_repeat_rather_than_clamping() {
        // Scrolled UVs leave [0,1] by design; clamping would smear them.
        let ImageSampler::Descriptor(descriptor) = sampler(ImageFilterMode::Nearest) else {
            panic!("a descriptor sampler");
        };

        assert_eq!(descriptor.address_mode_u, ImageAddressMode::Repeat);
        assert_eq!(descriptor.address_mode_v, ImageAddressMode::Repeat);
        assert_eq!(descriptor.mag_filter, ImageFilterMode::Nearest);
    }

    #[test]
    fn the_pool_searches_in_order_and_prefer_moves_to_the_front() {
        // Two distinct handles without an `Assets<TextureSet>` to make them.
        let uuid = |n: u128| {
            Handle::<TextureSet>::Uuid(bevy::asset::uuid::Uuid::from_u128(n), Default::default())
        };
        let mut pool = TexturePool::default();
        let first = uuid(1);
        let second = uuid(2);

        pool.push(first.clone());
        pool.push(second.clone());
        pool.push(first.clone());

        assert_eq!(pool.sets().len(), 2, "push must not duplicate");

        pool.prefer(second.clone());

        assert_eq!(pool.sets()[0], second);
    }
}
