//! mech3ax JSON/PNG output -> Bevy assets.
//!
//! mech3ax's output formats are explicitly unstable ("please don't build tools
//! around the output yet"), so every assumption about their shape lives in this
//! crate and nowhere else.
//!
//! Extracted game data is never committed: the engine reads it from a
//! user-supplied Recoil install at runtime.
//!
//! # The path from an install to an asset
//!
//! 1. [`root::discover`] finds the install (FR-D1) and [`root::validate`]
//!    checks it (FR-D2).
//! 2. [`extract::plan`] works out what `unzbd` would have to run and what is
//!    already done; [`extract::run`] runs it (FR-D3).
//! 3. [`AssetsPlugin`] points Bevy's asset server at the cache, so every
//!    loader asks for `cache://gamez/m1-gamez.gamez.zip` and gets
//!    bytes (FR-D5).
//! 4. [`catalog::LevelCatalog`] says which of those paths is which level.

use std::path::PathBuf;

use bevy::asset::io::AssetSourceBuilder;
use bevy::asset::io::file::FileAssetReader;

use bevy::prelude::*;

pub mod anim;
pub mod axes;
pub mod cabinet;
pub mod cache;
pub mod catalog;
pub mod config;
pub mod dialog;
pub mod disc;
pub mod extract;
pub mod geometry;
pub mod hud;
pub mod interp;
pub mod level;
pub mod materials;
pub mod mech3ax;
pub mod messages;
pub mod models;
pub mod reader;
pub mod reader_value;
pub mod ron_asset;
pub mod root;
pub mod sounds;
pub mod source;
pub mod splash;
pub mod test_support;
pub mod textures;

pub use anim::{
    AnimActivation, AnimDefDesc, AnimError, AnimLoader, AnimRefDesc, AnimSet, CallAt, Condition,
    EventDataDesc, EventDesc, EventStart, Execution, OffsetFrom, PrereqDesc, ResetTime, SeqDesc,
    SiFrame, SiScriptDesc,
};
pub use axes::{Axis, AxisConvention, EulerOrder};
pub use cache::{CacheDir, CacheEntry, CacheManifest, ExtractKind};
pub use catalog::{CACHE_SOURCE, LevelCatalog, LevelEntry, LevelId, LevelKind, MultiplayerMode};
pub use config::Config;
pub use extract::{ExtractJob, ExtractPlan, ExtractProgress};
pub use geometry::{
    TriangleGroup, Triangulated, collider_indices, material_per_triangle, triangulate,
};
pub use interp::{
    InterpError, InterpScripts, InterpScriptsLoader, RuntimeBindings, TextureAnim,
    TextureAnimBinding, runtime_bindings,
};
pub use level::{
    LevelData, LevelDataError, LevelDataLoader, LightDesc, LodDesc, NodeCommon, NodeDesc,
    NodeFlagsDesc, Object3dDesc, PartitionDesc, PartitionKey, TransformDesc, WorldDesc, ZMap,
    ZMapFeature, ZMapLoader,
};
pub use materials::{CycleDesc, MaterialDesc, Soil, material_desc, material_descs};
pub use messages::{Messages, MessagesError, MessagesLoader};
pub use models::{
    FacadeMode, ModelDesc, ModelFlags, ModelLightDesc, ModelType, PolygonDesc, PolygonMaterialDesc,
    model_desc, model_descs,
};
pub use reader::mission::{MissionError, RawMission, RawObjective};
pub use reader::{PlayerStart, ReaderData, ReaderError, ReaderLoader};
pub use reader_value::ReaderValue;
pub use ron_asset::{RonAssetError, RonAssetLoader};
pub use root::{DataRoot, DataRootError, DataVersion};
pub use textures::{
    TextureAlpha, TextureFilter, TexturePool, TextureSet, TextureSetLoader, TextureStretch,
};

/// Where this run reads game data from.
#[derive(Resource, Debug, Clone)]
pub struct DataPaths {
    /// The validated Recoil install.
    pub root: DataRoot,
    /// The extraction cache, registered as the `cache://` asset source.
    pub cache: CacheDir,
    /// The `unzbd` binary, when one was found. `None` means the engine can
    /// only use a cache the user populated by hand (FR-D3).
    pub unzbd: Option<PathBuf>,
}

impl DataPaths {
    /// Resolve every path from the CLI, the environment, the config file and
    /// the conventional locations.
    ///
    /// This is the whole of FR-D1 in one call, for `bft-bin` to use before it
    /// builds the `App`: the cache has to be known before [`AssetsPlugin`] is
    /// added, because Bevy builds asset sources when `AssetPlugin` does.
    pub fn discover(
        data_dir: Option<&std::path::Path>,
        cache_dir: Option<&std::path::Path>,
        unzbd: Option<&std::path::Path>,
    ) -> Result<DataPaths, DataRootError> {
        let config = Config::load().unwrap_or_default();
        let root = root::discover(data_dir)?;

        let cache = CacheDir::new(match (cache_dir, &config.cache_dir) {
            (Some(cli), _) => cli.to_path_buf(),
            (None, Some(configured)) => configured.clone(),
            (None, None) => cache::default_cache_dir(&root),
        });

        Ok(DataPaths {
            root,
            cache,
            unzbd: extract::find_unzbd(unzbd, config.unzbd.as_deref()),
        })
    }
}

/// Where this build's own `assets/` directory is.
///
/// Bevy resolves a relative [`AssetPlugin::file_path`] against
/// `BEVY_ASSET_ROOT`, else `CARGO_MANIFEST_DIR`, else the executable's
/// directory — and **none of those is right for this workspace**. Under
/// `cargo run` the manifest directory is `crates/bft-bin`, which has no
/// assets; from an installed binary it is the executable's directory, which
/// has none either. Both leave `tuning/vehicle.ron` unreadable, and the
/// vehicle table then waits for ever on an asset that never lands.
///
/// So the path is resolved here, absolutely, in this order:
///
/// 1. `BEVY_ASSET_ROOT`, joined with `assets`, for anyone who sets it;
/// 2. `<exe dir>/assets`, which is where a packaged build puts them;
/// 3. the workspace's own `assets`, found by walking up from the
///    `CARGO_MANIFEST_DIR` that `cargo run` sets, then from the current
///    directory, then from the manifest directory this crate was compiled
///    in.
///
/// A candidate from 2 or 3 counts only if it holds [`ASSET_MARKER`]. The
/// compile-time path came last and unchecked before, and a build reused from
/// another checkout (a git worktree sharing `target/`) baked in a path that
/// had since been deleted: the tuning files went missing and the game
/// entered a level with no vehicle table.
///
/// Returns an absolute path, which `AssetPlugin` uses as-is.
pub fn asset_directory() -> PathBuf {
    if let Some(root) = std::env::var_os("BEVY_ASSET_ROOT") {
        return PathBuf::from(root).join("assets");
    }

    let next_to_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("assets")));
    let roots = [
        std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from),
        std::env::current_dir().ok(),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
    ];

    find_asset_directory(next_to_exe, roots.into_iter().flatten())
        // Nothing found: hand back the conventional relative path so the
        // error Bevy logs names something a user can act on.
        .unwrap_or_else(|| PathBuf::from("assets"))
}

/// What marks a directory as this game's `assets/`: a tuning file every
/// build ships.
pub const ASSET_MARKER: &str = "tuning/vehicle.ron";

/// The first real `assets/` directory: `direct` itself, else one found by
/// walking up from each of `roots` in turn.
pub fn find_asset_directory(
    direct: Option<PathBuf>,
    roots: impl IntoIterator<Item = PathBuf>,
) -> Option<PathBuf> {
    let is_ours = |path: &std::path::Path| path.join(ASSET_MARKER).is_file();

    direct.filter(|path| is_ours(path)).or_else(|| {
        roots.into_iter().find_map(|root| {
            root.ancestors()
                .map(|directory| directory.join("assets"))
                .find(|candidate| is_ours(candidate))
        })
    })
}

/// The crate's single top-level plugin: the `cache://` asset source, the
/// resolved paths, and the level catalog.
///
/// **Add this before `DefaultPlugins`.** Bevy builds its asset sources while
/// `AssetPlugin` is built, so a source registered after it is ignored (with
/// an `error!`). `AssetsPlugin::new` takes the paths for the same reason:
/// they have to be known before the `App` starts.
///
/// The textures, level, reader, messages, anim and sound loaders register
/// here too, in
/// [`Plugin::finish`] rather than `build`: `register_asset_loader` reaches
/// for the `AssetServer`, which does not exist until `AssetPlugin` has been
/// built — and this plugin is added before it.
pub struct AssetsPlugin {
    paths: DataPaths,
    filter: TextureFilter,
}

impl AssetsPlugin {
    /// Point the engine at a discovered install and its cache.
    pub fn new(paths: DataPaths) -> AssetsPlugin {
        AssetsPlugin {
            paths,
            filter: TextureFilter::default(),
        }
    }

    /// Sample textures with this filter (FR-W3's toggle, DESIGN §5.2).
    ///
    /// It is fixed here rather than read from the [`TextureFilter`] resource
    /// because an `AssetLoader` has no access to the world; the resource is
    /// inserted too, so a settings screen can see the current value.
    pub fn with_texture_filter(mut self, filter: TextureFilter) -> AssetsPlugin {
        self.filter = filter;
        self
    }

    /// The cache directory this plugin will serve `cache://` from.
    pub fn cache_dir(&self) -> &std::path::Path {
        &self.paths.cache.path
    }
}

impl Plugin for AssetsPlugin {
    fn build(&self, app: &mut App) {
        let cache = self.paths.cache.path.clone();

        // `FileAssetReader::new` joins onto the executable's directory, which
        // an absolute path overrides — and `default_cache_dir` always
        // produces one. `create_root` is false: an empty cache should surface
        // as "run `open-bft extract`", not as a silently created directory.
        app.register_asset_source(
            CACHE_SOURCE,
            AssetSourceBuilder::new(move || Box::new(FileAssetReader::new(cache.clone()))),
        );

        app.insert_resource(self.paths.clone());
        app.insert_resource(self.filter);
        app.init_resource::<TexturePool>();
        // The catalog is built in `finish` rather than here so its warnings
        // reach a subscriber: this plugin has to be added before
        // `DefaultPlugins`, which is where `LogPlugin` lives, so anything
        // logged during `build` would go nowhere (NFR-9).
        app.init_resource::<LevelCatalog>();
    }

    fn finish(&self, app: &mut App) {
        let catalog = level_catalog(&self.paths.cache);
        app.insert_resource(catalog);

        // `Image` too: the texture loader hands out `Handle<Image>`s, and
        // Bevy panics on an uninitialised asset type. `bft-assets` is added
        // before `DefaultPlugins`, so it cannot rely on `ImagePlugin` having
        // done this — and a headless `bft-sim` host never adds one at all.
        //
        // **Only when nothing else has.** `init_asset` is not idempotent: it
        // registers a *fresh* `Assets<A>` every time, so calling it after
        // `ImagePlugin` did replaces the populated storage with an empty one
        // while the asset server keeps handing out indices into the old —
        // which panics with "the len is 0 but the index is N" the moment a
        // texture finishes loading. This `finish` runs after `ImagePlugin`'s
        // `build`, so the guard is what makes a windowed client work at all.
        if !app.world().contains_resource::<Assets<Image>>() {
            app.init_asset::<Image>();
        }
        app.init_asset::<TextureSet>();
        app.register_asset_loader(TextureSetLoader::new(self.filter.0));

        // The interp scripts, whose runtime path is the only
        // source of texture cycles and scrolls (docs/data/materials.md).
        app.init_asset::<InterpScripts>();
        app.register_asset_loader(InterpScriptsLoader);

        // Reader, messages and anim. `ReaderLoader` and `MessagesLoader` claim **no
        // extension**: the asset server resolves a loader by requested type
        // before it looks at extensions, and `messages.json` shares `.json`
        // with `interp.json`, and any second reader-backed loader would
        // contest `.rdr.zip`. Two loaders on one extension makes
        // Bevy pick by extension alone and warn, so neither claims one.
        app.init_asset::<ReaderData>();
        app.register_asset_loader(ReaderLoader);
        app.init_asset::<Messages>();
        app.register_asset_loader(MessagesLoader);
        app.init_asset::<AnimSet>();
        app.register_asset_loader(AnimLoader);

        // The sound bank. `AudioSource` is `bevy_audio`'s, and only
        // exists when its plugin was added — which a test app or a headless
        // host may not do — so the asset type is initialised here under the
        // same guard as `Image` above.
        #[cfg(feature = "audio")]
        {
            if !app
                .world()
                .contains_resource::<Assets<bevy::audio::AudioSource>>()
            {
                app.init_asset::<bevy::audio::AudioSource>();
            }
            app.init_asset::<sounds::SoundBank>();
            app.register_asset_loader(sounds::SoundBankLoader);
        }

        // A level's content, and its automap.
        app.init_asset::<LevelData>();
        app.register_asset_loader(LevelDataLoader);
        app.init_asset::<ZMap>();
        app.register_asset_loader(ZMapLoader);
    }
}

/// Build the catalog from whatever the cache currently holds.
///
/// A cache that has never been built, or whose manifest will not parse,
/// yields an empty catalog and a warning: the `Boot` state is where
/// the user is told to run `open-bft extract`, and failing at startup would
/// take the window down with it (NFR-9).
fn level_catalog(cache: &CacheDir) -> LevelCatalog {
    match cache::load_manifest(cache) {
        Ok(Some(manifest)) => LevelCatalog::from_manifest(&manifest, cache),
        Ok(None) => {
            warn!(
                "no extraction cache at {}; run `open-bft extract`",
                cache.path.display()
            );
            LevelCatalog::default()
        }
        Err(e) => {
            error!("cannot read the extraction cache: {e}");
            LevelCatalog::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{FakeRoot, TempDir};

    fn paths_for(fake: &FakeRoot, cache: &TempDir) -> DataPaths {
        DataPaths {
            root: fake.data_root(),
            cache: CacheDir::new(cache.path()),
            unzbd: None,
        }
    }

    fn workspace_with_assets(dir: &TempDir) -> std::path::PathBuf {
        let assets = dir.path().join("assets");
        std::fs::create_dir_all(assets.join("tuning")).expect("mkdir");
        std::fs::write(assets.join(ASSET_MARKER), "()").expect("write");
        std::fs::create_dir_all(dir.path().join("crates/bft-bin")).expect("mkdir");
        assets
    }

    /// `cargo run`'s manifest directory is `crates/bft-bin`: the walk up
    /// finds the workspace's `assets`.
    #[test]
    fn the_asset_directory_is_found_above_the_binary_crate() {
        let dir = TempDir::new();
        let assets = workspace_with_assets(&dir);

        let found = find_asset_directory(None, [dir.path().join("crates/bft-bin")]);
        assert_eq!(found, Some(assets));
    }

    /// A compile-time path from a deleted checkout, or an `assets` folder
    /// without our tuning files, is passed over for the next candidate.
    #[test]
    fn a_stale_or_foreign_assets_directory_is_skipped() {
        let dir = TempDir::new();
        let assets = workspace_with_assets(&dir);
        let foreign = TempDir::new();
        std::fs::create_dir_all(foreign.path().join("assets")).expect("mkdir");

        let found = find_asset_directory(
            Some(foreign.path().join("assets")),
            [
                std::path::PathBuf::from("/nonexistent/worktree/crates/bft-assets"),
                foreign.path().to_path_buf(),
                dir.path().join("crates/bft-bin"),
            ],
        );
        assert_eq!(found, Some(assets));
        assert_eq!(
            find_asset_directory(None, [foreign.path().to_path_buf()]),
            None
        );
    }

    #[test]
    fn the_plugin_registers_the_cache_source_and_the_resources() {
        let fake = FakeRoot::retail();
        let cache = TempDir::new();
        let mut app = App::new();

        // The order a real binary uses: our source first, then Bevy's asset
        // machinery, which is what builds it.
        app.add_plugins(AssetsPlugin::new(paths_for(&fake, &cache)));
        app.add_plugins((bevy::MinimalPlugins, bevy::asset::AssetPlugin::default()));

        assert!(app.world().get_resource::<DataPaths>().is_some());
        assert!(app.world().get_resource::<LevelCatalog>().is_some());

        let server = app.world().resource::<AssetServer>();
        assert!(
            server
                .get_source(bevy::asset::io::AssetSourceId::from(CACHE_SOURCE))
                .is_ok(),
            "cache:// should be a registered asset source"
        );
    }

    #[test]
    fn image_is_not_reinitialised_when_bevy_already_did_it() {
        // `init_asset` registers a *fresh* `Assets<A>`, so a second call after
        // `ImagePlugin` replaces the populated storage with an empty one and
        // the asset server then indexes past its end ("the len is 0 but the
        // index is 5") the moment a texture finishes loading. That took the
        // windowed client down, so the guard in `finish` is tested here.
        let fake = FakeRoot::retail();
        let cache = TempDir::new();
        let mut app = App::new();

        app.add_plugins(AssetsPlugin::new(paths_for(&fake, &cache)));
        app.add_plugins((bevy::MinimalPlugins, bevy::asset::AssetPlugin::default()));
        // Stand in for `ImagePlugin`, which a real client adds and which owns
        // `Image` first.
        app.init_asset::<Image>();
        let marker = app
            .world_mut()
            .resource_mut::<Assets<Image>>()
            .add(Image::default());

        app.finish();

        assert!(
            app.world()
                .resource::<Assets<Image>>()
                .get(&marker)
                .is_some(),
            "`finish` must not replace an `Assets<Image>` somebody else populated"
        );
    }

    #[test]
    fn an_unbuilt_cache_gives_an_empty_catalog_not_a_panic() {
        let fake = FakeRoot::retail();
        let cache = TempDir::new();
        let mut app = App::new();

        app.add_plugins(AssetsPlugin::new(paths_for(&fake, &cache)));
        app.add_plugins((bevy::MinimalPlugins, bevy::asset::AssetPlugin::default()));
        // The catalog is built in `finish`, which the app runs after every
        // plugin is built.
        app.finish();

        assert!(app.world().resource::<LevelCatalog>().levels.is_empty());
    }

    #[test]
    fn a_built_cache_is_catalogued_when_the_app_finishes() {
        let fake = FakeRoot::retail();
        let cache = TempDir::new();
        let cache_dir = CacheDir::new(cache.path());
        cache::save_manifest(
            &cache_dir,
            &CacheManifest {
                mech3ax_version: "unzbd v0.0.0-test".to_string(),
                mech3ax_commit: None,
                created: "2000-01-01T00:00:00Z".to_string(),
                entries: vec![
                    CacheEntry {
                        kind: ExtractKind::Gamez,
                        source: fake.path().join("zbd/m1/gamez.zbd"),
                        output: std::path::PathBuf::from("gamez/m1-gamez.gamez.zip"),
                        source_len: 1,
                        source_mtime: 0,
                    },
                    CacheEntry {
                        kind: ExtractKind::Reader,
                        source: fake.path().join("zbd/m1/zrdr.zbd"),
                        output: std::path::PathBuf::from("reader/m1-zrdr.rdr.zip"),
                        source_len: 1,
                        source_mtime: 0,
                    },
                ],
            },
        )
        .unwrap();

        let mut app = App::new();
        app.add_plugins(AssetsPlugin::new(DataPaths {
            root: fake.data_root(),
            cache: cache_dir,
            unzbd: None,
        }));
        app.add_plugins((bevy::MinimalPlugins, bevy::asset::AssetPlugin::default()));
        app.finish();

        let catalog = app.world().resource::<LevelCatalog>();
        assert_eq!(catalog.levels.len(), 1);
        assert_eq!(catalog.levels[0].id, LevelId(1));
    }

    #[test]
    fn the_cache_dir_falls_back_from_cli_to_config_to_default() {
        let fake = FakeRoot::retail();
        let explicit = TempDir::new();

        let paths = DataPaths::discover(Some(fake.path()), Some(explicit.path()), None).unwrap();
        assert_eq!(paths.cache.path, explicit.path());

        let paths = DataPaths::discover(Some(fake.path()), None, None).unwrap();
        assert_eq!(
            paths.cache.path,
            cache::default_cache_dir(&fake.data_root()),
        );
    }

    #[test]
    fn discovery_reports_where_it_looked_when_there_is_nothing() {
        let nothing = TempDir::new();

        let err = DataPaths::discover(Some(nothing.path()), None, None).unwrap_err();

        assert!(err.looked_in.contains(&nothing.path().to_path_buf()));
        assert!(!err.missing.is_empty());
    }
}
