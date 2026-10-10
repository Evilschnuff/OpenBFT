//! The level catalog: what the cache holds, as asset paths.
//!
//! Built from the cache manifest at startup so every loader asks the
//! asset server for `cache://gamez/m1-gamez.gamez.zip` and never computes a
//! path itself.
//!
//! Which level is which comes from docs/data/install.md: `m1`–`m6` are the six
//! campaign missions and `m7`–`m13` the multiplayer maps, with `m9` the race
//! map (it is the only one with a `race.zrd`) and `m13` the teleporter arena.

use std::collections::BTreeMap;
use std::path::Path;

use bevy::asset::AssetPath;
use bevy::prelude::*;

use crate::cache::{CacheDir, CacheManifest, ExtractKind};

/// The asset source the cache is registered under.
pub const CACHE_SOURCE: &str = "cache";

/// A mission number: `1`–`13` on the retail release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LevelId(pub u16);

impl LevelId {
    /// The directory name this level's archives live under, e.g. `m1`.
    pub fn dir_name(self) -> String {
        format!("m{}", self.0)
    }
}

impl std::fmt::Display for LevelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "m{}", self.0)
    }
}

impl std::str::FromStr for LevelId {
    type Err = String;

    /// Parse `m1` or a bare `1`, so `open-bft view m1` and `view 1` both work.
    fn from_str(s: &str) -> Result<LevelId, String> {
        let digits = s
            .strip_prefix('m')
            .or_else(|| s.strip_prefix('M'))
            .unwrap_or(s);

        digits
            .parse()
            .map(LevelId)
            .map_err(|_| format!("`{s}` is not a level name; try m1 … m13"))
    }
}

/// What a level is for (docs/data/install.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LevelKind {
    /// One of the six campaign missions, one stage each.
    Campaign {
        /// The campaign number, `1..=6`.
        campaign: u8,
    },
    /// One of the seven multiplayer maps.
    Multiplayer {
        /// Which mode this map is built for.
        mode: MultiplayerMode,
    },
    /// A level from an install we do not recognise.
    Unknown,
}

/// The multiplayer modes the shipped maps implement (docs/data/install.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiplayerMode {
    /// Eight spawn points in `net.zrd`; m7, m8, m10, m11, m12.
    Deathmatch,
    /// Start gates and laps; m9 only, the map with a `race.zrd`.
    Race,
    /// The teleporter arena; m13, the map with no automap.
    Teleport,
}

/// One level and everything the loaders need to open it.
#[derive(Debug, Clone)]
pub struct LevelEntry {
    /// The mission number.
    pub id: LevelId,
    /// The directory name, e.g. `m1`. Stable; use it for CLI and logs.
    pub name: String,
    /// The `messages.json` key holding the level's display title.
    ///
    /// `MSG_CAMPAIGN<N>_NAME` for **every** level, not only the campaigns
    /// (all 13 keys exist; m9 is "Beach Rally", m13 "Transporter Trauma").
    pub title_key: String,
    /// The level's nodes, models, materials and textures.
    pub gamez: AssetPath<'static>,
    /// The level's animation definitions, absent if extraction failed.
    pub anim: Option<AssetPath<'static>>,
    /// The per-mission reader archive.
    pub reader: AssetPath<'static>,
    /// The automap. `None` for m13, which ships none.
    pub zmap: Option<AssetPath<'static>>,
    /// Whether [`Self::zmap`] is a placeholder copy of m1's rather than this
    /// level's own map (true for m7–m12, docs/data/quirks.md).
    pub zmap_is_placeholder: bool,
    /// Texture archives to search, this mission's own first, then the
    /// cross-mission pool (docs/data/materials.md).
    pub textures: Vec<AssetPath<'static>>,
    /// What the level is for.
    pub kind: LevelKind,
}

impl LevelEntry {
    /// Whether this level has an automap worth showing.
    pub fn has_real_zmap(&self) -> bool {
        self.zmap.is_some() && !self.zmap_is_placeholder
    }
}

/// Every level in the cache, plus the archives that are not per level.
#[derive(Resource, Debug, Clone, Default)]
pub struct LevelCatalog {
    /// The levels, ordered by mission number.
    pub levels: Vec<LevelEntry>,
    /// The global reader archive: vehicles, weapons, player, pickups.
    pub global_reader: Option<AssetPath<'static>>,
    /// The interp scripts, kept for their texture animation bindings.
    pub interp: Option<AssetPath<'static>>,
    /// The localised string table.
    pub messages: Option<AssetPath<'static>>,
    /// Every `rtexture4` archive plus `image.zbd`, the fallback pool a level
    /// needs for the ~27 effect and HUD textures held in other missions'
    /// archives (docs/data/materials.md).
    pub texture_pool: Vec<AssetPath<'static>>,
    /// The high-fidelity sound archive (docs/data/sounds.md).
    pub sounds: Option<AssetPath<'static>>,
}

impl LevelCatalog {
    /// Find a level by its mission number.
    pub fn level(&self, id: LevelId) -> Option<&LevelEntry> {
        self.levels.iter().find(|l| l.id == id)
    }

    /// Find a level by its directory name, e.g. `m1`.
    pub fn level_by_name(&self, name: &str) -> Option<&LevelEntry> {
        self.levels.iter().find(|l| l.name == name)
    }

    /// The campaign missions, in play order.
    pub fn campaign(&self) -> impl Iterator<Item = &LevelEntry> {
        self.levels
            .iter()
            .filter(|l| matches!(l.kind, LevelKind::Campaign { .. }))
    }

    /// The multiplayer maps.
    pub fn multiplayer(&self) -> impl Iterator<Item = &LevelEntry> {
        self.levels
            .iter()
            .filter(|l| matches!(l.kind, LevelKind::Multiplayer { .. }))
    }

    /// Build the catalog from a cache manifest.
    ///
    /// Only what the manifest records is offered: a level whose gamez job
    /// failed does not appear at all, and one whose anim job failed appears
    /// with `anim: None`, because a level with no geometry cannot be loaded
    /// while one with no animation can.
    ///
    /// `cache` is needed to tell a race map from a deathmatch one: the mode
    /// is implied by which files the extracted reader ZIP holds, not declared
    /// anywhere (docs/data/install.md).
    pub fn from_manifest(manifest: &CacheManifest, cache: &CacheDir) -> LevelCatalog {
        // One output per (kind, mission) for the kinds a mission has exactly
        // one of; textures are the exception and get their own list, because
        // a mission ships both `rtexture4` and `rtexture2`.
        let mut by_kind: BTreeMap<(ExtractKind, String), &Path> = BTreeMap::new();
        let mut mission_textures: BTreeMap<String, Vec<&Path>> = BTreeMap::new();
        let mut globals: BTreeMap<ExtractKind, &Path> = BTreeMap::new();
        let mut global_textures: Vec<&Path> = Vec::new();

        for entry in &manifest.entries {
            let output = entry.output.as_path();
            match (entry.kind, mission_of(output)) {
                (ExtractKind::Textures, Some(mission)) => {
                    mission_textures.entry(mission).or_default().push(output);
                }
                (kind, Some(mission)) => {
                    by_kind.insert((kind, mission), output);
                }
                // `image.zbd` belongs to no mission but is part of the pool.
                (ExtractKind::Textures, None) => global_textures.push(output),
                (kind, None) => {
                    globals.insert(kind, output);
                }
            }
        }

        // The cross-mission pool: every mission's full-size hardware archive
        // plus `image.zbd`. Half-size `rtexture2` holds the same names at a
        // lower resolution, so it is a tier, not a fallback source
        // (docs/data/materials.md). Mission order keeps a lookup deterministic.
        let mut texture_pool: Vec<AssetPath<'static>> = Vec::new();
        for outputs in mission_textures.values() {
            for output in outputs {
                if is_full_size_textures(output) {
                    texture_pool.push(asset_path(output));
                }
            }
        }
        texture_pool.extend(global_textures.iter().map(|o| asset_path(o)));

        let mut levels = Vec::new();
        for id in level_ids(&by_kind) {
            let mission = id.dir_name();
            let (Some(gamez), Some(reader)) = (
                by_kind.get(&(ExtractKind::Gamez, mission.clone())),
                by_kind.get(&(ExtractKind::Reader, mission.clone())),
            ) else {
                // No geometry or no reader data: nothing the level loader
                // could do with it.
                continue;
            };

            let zmap = by_kind.get(&(ExtractKind::ZMap, mission.clone()));

            // This mission's own archives first — full size before half — so
            // a lookup hits the level's own texture before a same-named one
            // from another mission. The pool follows, which is what resolves
            // the ~27 effect and HUD textures m1 borrows (docs/data/materials.md).
            let mut own: Vec<&&Path> = mission_textures
                .get(&mission)
                .map(|v| v.iter().collect())
                .unwrap_or_default();
            own.sort_by_key(|o| !is_full_size_textures(o));

            let mut textures: Vec<AssetPath<'static>> =
                own.into_iter().map(|o| asset_path(o)).collect();
            for pooled in &texture_pool {
                if !textures.contains(pooled) {
                    textures.push(pooled.clone());
                }
            }

            let kind = classify(id, cache, reader);

            levels.push(LevelEntry {
                id,
                name: mission.clone(),
                // All 13 levels have a title under this key, campaign or not.
                title_key: format!("MSG_CAMPAIGN{}_NAME", id.0),
                gamez: asset_path(gamez),
                anim: by_kind
                    .get(&(ExtractKind::Anim, mission.clone()))
                    .map(|o| asset_path(o)),
                reader: asset_path(reader),
                zmap: zmap.map(|o| asset_path(o)),
                // docs/data/quirks.md: m7-m12 ship byte-identical copies of m1's.
                zmap_is_placeholder: zmap.is_some() && (7..=12).contains(&id.0),
                textures,
                kind,
            });
        }

        LevelCatalog {
            levels,
            global_reader: globals.get(&ExtractKind::Reader).map(|o| asset_path(o)),
            interp: globals.get(&ExtractKind::Interp).map(|o| asset_path(o)),
            messages: globals.get(&ExtractKind::Messages).map(|o| asset_path(o)),
            texture_pool,
            sounds: globals.get(&ExtractKind::Sounds).map(|o| asset_path(o)),
        }
    }
}

/// Every mission number the manifest mentions, ascending.
fn level_ids(by_kind: &BTreeMap<(ExtractKind, String), &Path>) -> Vec<LevelId> {
    let mut ids: Vec<LevelId> = by_kind
        .keys()
        .filter_map(|(_, mission)| mission.strip_prefix('m')?.parse().ok().map(LevelId))
        .collect();
    ids.sort_unstable();
    ids.dedup();

    ids
}

/// Decide what a level is for (docs/data/install.md).
///
/// `m1`–`m6` are the campaigns. Among the multiplayer maps the mode is
/// implied by files rather than declared: m9 is the only one carrying a
/// `race.zrd`, and m13 — the only map with no automap — is the teleporter
/// arena. Everything else is deathmatch.
fn classify(id: LevelId, cache: &CacheDir, reader: &Path) -> LevelKind {
    match id.0 {
        1..=6 => LevelKind::Campaign {
            campaign: id.0 as u8,
        },
        7..=13 => {
            let mode = if reader_has(&cache.path.join(reader), "race.zrd.json") {
                MultiplayerMode::Race
            } else if id.0 == 13 {
                MultiplayerMode::Teleport
            } else {
                MultiplayerMode::Deathmatch
            };

            LevelKind::Multiplayer { mode }
        }
        _ => LevelKind::Unknown,
    }
}

/// Whether an extracted reader ZIP holds a named entry.
///
/// Reads the central directory only, so it is cheap enough to do for the
/// seven multiplayer maps at startup. A ZIP that will not open is not an
/// error here: the mode falls back to deathmatch and the loader that
/// actually needs the file reports the problem with its own context.
fn reader_has(zip_path: &Path, entry: &str) -> bool {
    let Ok(file) = std::fs::File::open(zip_path) else {
        return false;
    };
    let Ok(archive) = zip::ZipArchive::new(std::io::BufReader::new(file)) else {
        return false;
    };

    archive.file_names().any(|name| name == entry)
}

/// The mission directory a cache-relative output belongs to, if any.
///
/// Outputs are named `<mission>-<archive>.<suffix>` inside a per-kind
/// directory, so the mission is the file name's first `m<digits>` segment.
fn mission_of(output: &Path) -> Option<String> {
    let stem = output.file_name()?.to_str()?;
    let head = stem.split(['-', '.']).next()?;

    let digits = head.strip_prefix('m')?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }

    Some(head.to_string())
}

/// Whether a texture output is the full-size hardware archive.
///
/// docs/data/materials.md: `rtexture4` is full size and `rtexture2` the same names at
/// half size, so only `rtexture4` belongs in the cross-mission pool.
fn is_full_size_textures(output: &Path) -> bool {
    output
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.contains("rtexture4"))
}

/// A cache-relative output as a `cache://` asset path.
fn asset_path(output: &Path) -> AssetPath<'static> {
    // Asset paths are always `/`-separated, whatever the host filesystem uses.
    let relative = output.to_string_lossy().replace('\\', "/");

    AssetPath::from(format!("{CACHE_SOURCE}://{relative}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cache::CacheEntry;
    use crate::test_support::TempDir;
    use std::path::PathBuf;

    /// A manifest for a full retail extraction, as `extract::plan` would
    /// produce it.
    fn retail_manifest() -> CacheManifest {
        let mut entries = vec![
            entry(ExtractKind::Interp, "interp.json"),
            entry(ExtractKind::Messages, "messages.json"),
            entry(ExtractKind::Reader, "reader/zrdr.rdr.zip"),
            entry(ExtractKind::Textures, "textures/image.tex.zip"),
            entry(ExtractKind::Sounds, "sounds/soundsh.snd.zip"),
        ];

        for n in 1..=13 {
            entries.push(entry(
                ExtractKind::Gamez,
                &format!("gamez/m{n}-gamez.gamez.zip"),
            ));
            entries.push(entry(
                ExtractKind::Anim,
                &format!("anim/m{n}-anim.anim.zip"),
            ));
            entries.push(entry(
                ExtractKind::Reader,
                &format!("reader/m{n}-zrdr.rdr.zip"),
            ));
            entries.push(entry(
                ExtractKind::Textures,
                &format!("textures/m{n}-rtexture4.tex.zip"),
            ));
            entries.push(entry(
                ExtractKind::Textures,
                &format!("textures/m{n}-rtexture2.tex.zip"),
            ));
            if n != 13 {
                entries.push(entry(ExtractKind::ZMap, &format!("zmap/m{n}.zmap.json")));
            }
        }

        CacheManifest {
            mech3ax_version: "unzbd v0.0.0-test".to_string(),
            mech3ax_commit: None,
            created: "2000-01-01T00:00:00Z".to_string(),
            entries,
        }
    }

    fn entry(kind: ExtractKind, output: &str) -> CacheEntry {
        CacheEntry {
            kind,
            source: PathBuf::from("/games/recoil").join(output),
            output: PathBuf::from(output),
            source_len: 1,
            source_mtime: 0,
        }
    }

    /// A cache with a reader ZIP for m9 holding `race.zrd.json`.
    fn cache_with_race_map() -> TempDir {
        let dir = TempDir::new();
        let reader = dir.path().join("reader");
        std::fs::create_dir_all(&reader).unwrap();

        for n in 7..=13 {
            let path = reader.join(format!("m{n}-zrdr.rdr.zip"));
            let file = std::fs::File::create(&path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            zip.start_file("net.zrd.json", options).unwrap();
            if n == 9 {
                zip.start_file("race.zrd.json", options).unwrap();
            }
            zip.finish().unwrap();
        }

        dir
    }

    #[test]
    fn a_retail_manifest_yields_thirteen_levels() {
        let cache = cache_with_race_map();

        let catalog = LevelCatalog::from_manifest(&retail_manifest(), &CacheDir::new(cache.path()));

        assert_eq!(catalog.levels.len(), 13);
        assert_eq!(catalog.campaign().count(), 6);
        assert_eq!(catalog.multiplayer().count(), 7);
        assert_eq!(catalog.levels[0].id, LevelId(1));
        assert_eq!(catalog.levels[12].id, LevelId(13));
    }

    #[test]
    fn the_globals_are_catalog_level_not_per_level() {
        let cache = cache_with_race_map();

        let catalog = LevelCatalog::from_manifest(&retail_manifest(), &CacheDir::new(cache.path()));

        assert_eq!(
            catalog.global_reader.as_ref().map(|p| p.to_string()),
            Some("cache://reader/zrdr.rdr.zip".to_string())
        );
        assert!(catalog.interp.is_some());
        assert!(catalog.messages.is_some());
        assert_eq!(
            catalog.sounds.as_ref().map(|p| p.to_string()),
            Some("cache://sounds/soundsh.snd.zip".to_string())
        );
    }

    #[test]
    fn the_texture_pool_is_every_full_size_archive_plus_image() {
        let cache = cache_with_race_map();

        let catalog = LevelCatalog::from_manifest(&retail_manifest(), &CacheDir::new(cache.path()));
        let pool: Vec<String> = catalog.texture_pool.iter().map(|p| p.to_string()).collect();

        // docs/data/materials.md: 13 rtexture4 archives plus image.zbd.
        assert_eq!(pool.len(), 14);
        assert!(pool.contains(&"cache://textures/image.tex.zip".to_string()));
        assert!(pool.contains(&"cache://textures/m13-rtexture4.tex.zip".to_string()));
        assert!(
            !pool.iter().any(|p| p.contains("rtexture2")),
            "the half-size tier is not a fallback source"
        );
    }

    #[test]
    fn a_levels_own_archive_comes_before_the_pool() {
        let cache = cache_with_race_map();

        let catalog = LevelCatalog::from_manifest(&retail_manifest(), &CacheDir::new(cache.path()));
        let m1 = catalog.level(LevelId(1)).unwrap();

        assert_eq!(
            m1.textures[0].to_string(),
            "cache://textures/m1-rtexture4.tex.zip"
        );
        // And the pool follows, so the 27 cross-mission effect textures
        // resolve (docs/data/materials.md).
        assert!(m1.textures.len() > 1);
        assert!(
            m1.textures
                .iter()
                .any(|p| p.to_string().contains("m2-rtexture4"))
        );
    }

    #[test]
    fn m9_is_the_race_map_and_m13_the_teleporter_arena() {
        let cache = cache_with_race_map();

        let catalog = LevelCatalog::from_manifest(&retail_manifest(), &CacheDir::new(cache.path()));

        assert_eq!(
            catalog.level(LevelId(9)).unwrap().kind,
            LevelKind::Multiplayer {
                mode: MultiplayerMode::Race
            }
        );
        assert_eq!(
            catalog.level(LevelId(13)).unwrap().kind,
            LevelKind::Multiplayer {
                mode: MultiplayerMode::Teleport
            }
        );
        assert_eq!(
            catalog.level(LevelId(7)).unwrap().kind,
            LevelKind::Multiplayer {
                mode: MultiplayerMode::Deathmatch
            }
        );
        assert_eq!(
            catalog.level(LevelId(3)).unwrap().kind,
            LevelKind::Campaign { campaign: 3 }
        );
    }

    #[test]
    fn m13_has_no_zmap_and_m7_to_m12_have_placeholders() {
        let cache = cache_with_race_map();

        let catalog = LevelCatalog::from_manifest(&retail_manifest(), &CacheDir::new(cache.path()));

        // docs/data/install.md and docs/data/quirks.md.
        assert!(catalog.level(LevelId(13)).unwrap().zmap.is_none());
        assert!(catalog.level(LevelId(1)).unwrap().has_real_zmap());
        let m8 = catalog.level(LevelId(8)).unwrap();
        assert!(m8.zmap.is_some());
        assert!(m8.zmap_is_placeholder);
        assert!(!m8.has_real_zmap());
    }

    #[test]
    fn every_level_has_a_title_key_including_the_multiplayer_maps() {
        let cache = cache_with_race_map();

        let catalog = LevelCatalog::from_manifest(&retail_manifest(), &CacheDir::new(cache.path()));

        // All 13 MSG_CAMPAIGN<N>_NAME keys exist in messages.dll.
        assert_eq!(
            catalog.level(LevelId(1)).unwrap().title_key,
            "MSG_CAMPAIGN1_NAME"
        );
        assert_eq!(
            catalog.level(LevelId(13)).unwrap().title_key,
            "MSG_CAMPAIGN13_NAME"
        );
    }

    #[test]
    fn a_level_whose_anim_failed_still_appears() {
        let cache = cache_with_race_map();
        let mut manifest = retail_manifest();
        manifest.entries.retain(|e| {
            !(e.kind == ExtractKind::Anim && e.output.to_string_lossy().contains("m4-"))
        });

        let catalog = LevelCatalog::from_manifest(&manifest, &CacheDir::new(cache.path()));

        let m4 = catalog.level(LevelId(4)).unwrap();
        assert!(m4.anim.is_none(), "an anim failure is not fatal");
        assert!(!m4.gamez.to_string().is_empty());
    }

    #[test]
    fn a_level_whose_gamez_failed_is_not_offered() {
        let cache = cache_with_race_map();
        let mut manifest = retail_manifest();
        manifest.entries.retain(|e| {
            !(e.kind == ExtractKind::Gamez && e.output.to_string_lossy().contains("m5-"))
        });

        let catalog = LevelCatalog::from_manifest(&manifest, &CacheDir::new(cache.path()));

        assert!(catalog.level(LevelId(5)).is_none());
        assert_eq!(catalog.levels.len(), 12);
    }

    #[test]
    fn an_empty_manifest_is_an_empty_catalog_not_a_panic() {
        let cache = TempDir::new();
        let manifest = CacheManifest {
            mech3ax_version: String::new(),
            mech3ax_commit: None,
            created: String::new(),
            entries: Vec::new(),
        };

        let catalog = LevelCatalog::from_manifest(&manifest, &CacheDir::new(cache.path()));

        assert!(catalog.levels.is_empty());
        assert!(catalog.global_reader.is_none());
    }

    #[test]
    fn outputs_map_back_to_their_mission() {
        assert_eq!(
            mission_of(Path::new("gamez/m1-gamez.gamez.zip")),
            Some("m1".to_string())
        );
        assert_eq!(
            mission_of(Path::new("zmap/m12.zmap.json")),
            Some("m12".to_string())
        );
        assert_eq!(mission_of(Path::new("textures/image.tex.zip")), None);
        assert_eq!(mission_of(Path::new("reader/zrdr.rdr.zip")), None);
        assert_eq!(mission_of(Path::new("messages.json")), None);
    }

    #[test]
    fn level_ids_parse_from_both_spellings() {
        assert_eq!("m1".parse(), Ok(LevelId(1)));
        assert_eq!("13".parse(), Ok(LevelId(13)));
        assert!("banana".parse::<LevelId>().is_err());
        assert_eq!(LevelId(7).to_string(), "m7");
    }

    /// The full round trip on real data: plan, then catalog what a completed
    /// run would have recorded.
    ///
    /// Needs both an install (`BFT_DATA_DIR`) and a cache that has actually
    /// been extracted into (`BFT_CACHE_DIR`, as `open-bft extract` would
    /// leave it), because the multiplayer mode is read out of the extracted
    /// reader ZIPs.
    #[test]
    fn the_real_cache_catalogs_thirteen_levels() {
        let (Some(root), Some(cache)) = (
            crate::test_support::data_root(),
            std::env::var_os("BFT_CACHE_DIR").map(PathBuf::from),
        ) else {
            return;
        };
        let cache = CacheDir::new(cache);
        let Some(manifest) = crate::cache::load_manifest(&cache).expect("a readable manifest")
        else {
            return;
        };
        let _ = root;

        let catalog = LevelCatalog::from_manifest(&manifest, &cache);

        // docs/data/install.md's split, read back out of the real data.
        assert_eq!(catalog.levels.len(), 13);
        assert_eq!(catalog.campaign().count(), 6);
        assert_eq!(catalog.multiplayer().count(), 7);
        assert_eq!(
            catalog.level(LevelId(9)).unwrap().kind,
            LevelKind::Multiplayer {
                mode: MultiplayerMode::Race
            },
            "m9 is the only map with a race.zrd"
        );
        assert_eq!(
            catalog.level(LevelId(13)).unwrap().kind,
            LevelKind::Multiplayer {
                mode: MultiplayerMode::Teleport
            }
        );

        // install.md: m13 ships no automap; quirks.md: m7-m12's are placeholders.
        assert!(catalog.level(LevelId(13)).unwrap().zmap.is_none());
        assert!(catalog.level(LevelId(1)).unwrap().has_real_zmap());
        assert!(!catalog.level(LevelId(8)).unwrap().has_real_zmap());

        // materials.md: the cross-mission pool is 13 rtexture4 archives plus image.
        assert_eq!(catalog.texture_pool.len(), 14);

        // Every level has geometry, animation and a reader archive.
        for level in &catalog.levels {
            assert!(level.anim.is_some(), "{} has no anim", level.name);
            assert!(
                level.gamez.to_string().ends_with(".gamez.zip"),
                "{}",
                level.gamez
            );
        }
    }

    #[test]
    fn asset_paths_use_the_cache_source() {
        let path = asset_path(Path::new("gamez/m1-gamez.gamez.zip"));

        assert_eq!(path.to_string(), "cache://gamez/m1-gamez.gamez.zip");
        assert_eq!(
            path.source(),
            &bevy::asset::io::AssetSourceId::from(CACHE_SOURCE)
        );
    }
}
