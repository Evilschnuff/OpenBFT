//! The texture and interp loaders against a real Recoil install.
//!
//! These are the texture loader's checks that need game data. They skip
//! (not fail) when `BFT_DATA_DIR` is unset or the cache has not been built,
//! so a contributor without the game still runs `cargo test --workspace`
//! green. No game data enters the repository: everything is read from the
//! cache and only counts are asserted.
//!
//! Build the cache first with `open-bft extract`.

use std::path::PathBuf;

use bevy::asset::AssetPlugin;
use bevy::asset::io::AssetSourceId;
use bevy::prelude::*;
use bft_assets::interp::{TextureAnim, runtime_bindings};
use bft_assets::materials::{MaterialDesc, material_desc};
use bft_assets::mech3ax::{Material, Script, TextureRef};
use bft_assets::textures::{TextureSet, texture_key};
use bft_assets::{AssetsPlugin, CACHE_SOURCE, DataPaths};

/// The resolved paths for a real install, or `None` when there is no data.
fn paths() -> Option<DataPaths> {
    let root = bft_assets::test_support::data_root()?;

    match DataPaths::discover(Some(&root), None, None) {
        Ok(paths) if paths.cache.path.is_dir() => Some(paths),
        Ok(paths) => {
            eprintln!(
                "no extraction cache at {}; run `open-bft extract`. Skipping.",
                paths.cache.path.display()
            );
            None
        }
        Err(e) => {
            eprintln!("cannot resolve the data paths ({e}); skipping");
            None
        }
    }
}

/// Read one JSON member of a cached ZIP.
fn zip_json<T: serde::de::DeserializeOwned>(path: &PathBuf, entry: &str) -> T {
    use std::io::Read;

    let file = std::fs::File::open(path).expect("the cache holds this archive");
    let mut archive = zip::ZipArchive::new(std::io::BufReader::new(file)).expect("a readable ZIP");
    let mut json = String::new();
    archive
        .by_name(entry)
        .expect("the archive holds this entry")
        .read_to_string(&mut json)
        .expect("readable");

    serde_json::from_str(&json).expect("parses into the mirrored types")
}

/// An app with the cache source and the texture loader, ready to load.
fn app_for(paths: DataPaths) -> App {
    let mut app = App::new();
    app.add_plugins(AssetsPlugin::new(paths));
    app.add_plugins((MinimalPlugins, AssetPlugin::default()));
    app.finish();

    app
}

/// Drive the asset server until the handle resolves or the budget runs out.
fn load_blocking<'a>(app: &'a mut App, path: &'static str) -> Option<&'a TextureSet> {
    let handle: Handle<TextureSet> = app.world().resource::<AssetServer>().load(path);

    for _ in 0..2000 {
        app.update();

        match app.world().resource::<AssetServer>().load_state(&handle) {
            bevy::asset::LoadState::Loaded => {
                return app.world().resource::<Assets<TextureSet>>().get(&handle);
            }
            bevy::asset::LoadState::Failed(e) => panic!("{path} failed to load: {e}"),
            _ => {}
        }
    }

    panic!("{path} did not finish loading");
}

#[test]
fn a_texture_archive_loads_every_manifest_entry() {
    let Some(paths) = paths() else { return };

    // The manifest's own count, read directly, is the number to match.
    let archive = paths.cache.path.join("textures/m1-rtexture4.tex.zip");
    if !archive.is_file() {
        eprintln!("no m1 texture archive in the cache; skipping");
        return;
    }
    let manifest: bft_assets::mech3ax::TextureManifest = zip_json(&archive, "manifest.json");
    let expected = manifest.texture_infos.len();

    let mut app = app_for(paths);
    let set = load_blocking(&mut app, "cache://textures/m1-rtexture4.tex.zip")
        .expect("the set is in Assets once loaded");

    assert_eq!(
        set.images.len(),
        expected,
        "every manifest entry should have decoded"
    );
    assert_eq!(set.info.len(), expected);

    // The archive stores bare stems, and `info` must agree with the manifest.
    let first = &manifest.texture_infos[0];
    let info = set.info(&first.name).expect("the first texture is present");
    assert_eq!(info.width, first.width);
    assert_eq!(info.height, first.height);
}

#[test]
fn the_cache_source_is_what_the_loader_reads_through() {
    let Some(paths) = paths() else { return };
    let app = app_for(paths);

    assert!(
        app.world()
            .resource::<AssetServer>()
            .get_source(AssetSourceId::from(CACHE_SOURCE))
            .is_ok()
    );
}

#[test]
fn every_level_material_converts_and_resolves_to_a_texture_name() {
    let Some(paths) = paths() else { return };

    for mission in 1..=13u16 {
        let gamez = paths
            .cache
            .path
            .join(format!("gamez/m{mission}-gamez.gamez.zip"));
        if !gamez.is_file() {
            continue;
        }

        let materials: Vec<Material> = zip_json(&gamez, "materials.json");
        let textures: Vec<TextureRef> = zip_json(&gamez, "textures.json");

        for (index, material) in materials.iter().enumerate() {
            let desc = material_desc(material, &textures, index)
                .unwrap_or_else(|e| panic!("m{mission} material {index}: {e}"));

            if let MaterialDesc::Textured { texture, .. } = &desc {
                assert!(
                    !texture.is_empty(),
                    "m{mission} material {index} resolved to an empty name"
                );
                assert_eq!(
                    *texture,
                    texture_key(texture),
                    "names out of the converter are already normalised"
                );
            }
        }
    }
}

#[test]
fn no_level_bakes_a_cycle_or_sets_the_unknown_flag() {
    // docs/data/materials.md's central claim, and the reason cycling is read from
    // interp instead. If this ever fails, the interp path is no longer the
    // only source and the texture-animation design (docs/data/materials.md) has changed.
    let Some(paths) = paths() else { return };
    let mut checked = 0;

    for mission in 1..=13u16 {
        let gamez = paths
            .cache
            .path
            .join(format!("gamez/m{mission}-gamez.gamez.zip"));
        if !gamez.is_file() {
            continue;
        }

        let materials: Vec<Material> = zip_json(&gamez, "materials.json");
        for material in &materials {
            if let Material::Textured(textured) = material {
                assert!(textured.cycle.is_none(), "m{mission} bakes a cycle");
                assert!(!textured.flag, "m{mission} sets the unknown flag");
                checked += 1;
            }
        }
    }

    assert!(checked > 0, "at least one level should have been read");
}

#[test]
fn the_interp_runtime_path_yields_the_animations_data_notes_counts() {
    let Some(paths) = paths() else { return };

    let interp = paths.cache.path.join("interp.json");
    if !interp.is_file() {
        eprintln!("no interp.json in the cache; skipping");
        return;
    }
    let scripts: Vec<Script> =
        serde_json::from_str(&std::fs::read_to_string(&interp).expect("readable"))
            .expect("parses into the mirrored Script type");

    let mut cycles = 0;
    let mut scrolls = 0;

    for mission in 1..=13u16 {
        let bindings = runtime_bindings(&scripts, mission);

        assert_eq!(
            bindings.max_tether_altitude,
            Some(30.0),
            "m{mission}: docs/data/interp.md says 30.0 in all 13 missions"
        );
        assert_eq!(
            bindings.name("playerName"),
            Some("bft_00"),
            "m{mission}: the player node name"
        );
        assert_eq!(bindings.name("worldName"), Some("world1"));

        for binding in &bindings.texture_anims {
            assert!(
                !binding.node.contains('%'),
                "m{mission}: {} still has an unexpanded variable",
                binding.node
            );

            match &binding.anim {
                TextureAnim::Cycle { frames, .. } => {
                    assert!(!frames.is_empty(), "a cycle with no frames was emitted");
                    cycles += 1;
                }
                TextureAnim::Scroll { u, v } => {
                    assert!(
                        *u != 0.0 || *v != 0.0,
                        "m{mission}: a scroll binding with no rate is pointless"
                    );
                    scrolls += 1;
                }
            }
        }
    }

    // docs/data/interp.md's counts across all 13 missions, which the scripts
    // reach by `source`. Exact, because a parser that quietly stops
    // following `source` would still produce a plausible-looking subset.
    assert_eq!(cycles, 215, "cycle bindings across the 13 missions");
    assert_eq!(scrolls, 157, "scroll bindings across the 13 missions");
}

#[test]
fn the_cross_mission_pool_resolves_all_but_one_known_dangling_reference() {
    // The reason `TexturePool` exists: 27 of m1's 471 references are in no
    // m1 archive (docs/data/materials.md). The pool finds all of them -- and every
    // other level's -- with exactly one exception, m9's `surf00.tif`, which
    // is in no archive at all: the surf cycle ships as `surf01`..`surf13`,
    // so `surf00` is a dangling reference in the retail data. It is the case
    // the loader's "warn and carry on" path exists for.
    let Some(paths) = paths() else { return };

    let mut pool = std::collections::HashSet::new();
    let mut archives = 0;
    for mission in 1..=13u16 {
        let archive = paths
            .cache
            .path
            .join(format!("textures/m{mission}-rtexture4.tex.zip"));
        if !archive.is_file() {
            continue;
        }
        archives += 1;
        let manifest: bft_assets::mech3ax::TextureManifest = zip_json(&archive, "manifest.json");
        for info in manifest.texture_infos {
            pool.insert(texture_key(&info.name));
        }
    }
    let ui = paths.cache.path.join("textures/image.tex.zip");
    if ui.is_file() {
        let manifest: bft_assets::mech3ax::TextureManifest = zip_json(&ui, "manifest.json");
        for info in manifest.texture_infos {
            pool.insert(texture_key(&info.name));
        }
    }
    if archives == 0 {
        eprintln!("no texture archives in the cache; skipping");
        return;
    }

    let mut unresolved: Vec<(u16, String)> = Vec::new();
    for mission in 1..=13u16 {
        let gamez = paths
            .cache
            .path
            .join(format!("gamez/m{mission}-gamez.gamez.zip"));
        if !gamez.is_file() {
            continue;
        }

        let textures: Vec<TextureRef> = zip_json(&gamez, "textures.json");
        for texture in &textures {
            let key = texture_key(&texture.name);
            if !pool.contains(&key) {
                unresolved.push((mission, key));
            }
        }
    }

    assert_eq!(
        unresolved,
        vec![(9, "surf00".to_string())],
        "the pool should resolve everything but the one known dangling reference"
    );
}

/// `effects.zrd`'s 13 sprite cycles read as READER-SCHEMA §2.7 describes,
/// every frame is in some texture archive, and the textures the burning
/// wrecks and their smoke are drawn with open a cycle.
///
/// That last part is why the table matters at runtime: no interp binding
/// names `fire_hulk.flt`'s or `smoke_destroy.flt`'s nodes, so without it
/// the flame on every wreck showed `fire101` for its whole 80 s.
#[test]
fn every_effect_cycle_resolves_and_the_wreck_fires_open_one() {
    use bft_assets::reader::effects::parse_effect_cycles;
    use bft_assets::reader::parse_reader_data;

    let Some(paths) = paths() else { return };
    let global = paths.cache.path.join("reader/zrdr.rdr.zip");
    if !global.is_file() {
        eprintln!("no global reader in the cache; skipping");
        return;
    }
    let reader = parse_reader_data(&std::fs::read(&global).expect("readable"))
        .expect("the global reader parses");
    let cycles = parse_effect_cycles(&reader).expect("effects.zrd is in the global reader");

    assert_eq!(cycles.len(), 13);
    let looping: Vec<&str> = cycles
        .cycles
        .iter()
        .filter(|cycle| cycle.looping)
        .map(|cycle| cycle.name.as_str())
        .collect();
    assert_eq!(looping, ["fire1"], "fire1 is the one with LOOPING ON");
    for cycle in &cycles.cycles {
        assert!(cycle.speed > 0.0, "{} has no speed", cycle.name);
    }

    let fire = cycles
        .for_texture("fire101")
        .expect("the flame's first frame");
    assert_eq!((fire.name.as_str(), fire.maps.len()), ("fire1", 12));
    let smoke = cycles
        .for_texture("bsmok1")
        .expect("the smoke's first frame");
    assert_eq!(smoke.name, "smoke1");

    let mut pool = std::collections::HashSet::new();
    for archive in std::fs::read_dir(paths.cache.path.join("textures"))
        .expect("a texture cache")
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.to_string_lossy().ends_with(".tex.zip"))
    {
        let manifest: bft_assets::mech3ax::TextureManifest = zip_json(&archive, "manifest.json");
        pool.extend(
            manifest
                .texture_infos
                .iter()
                .map(|info| texture_key(&info.name)),
        );
    }
    let missing: Vec<&str> = cycles
        .cycles
        .iter()
        .flat_map(|cycle| cycle.maps.iter())
        .filter(|frame| !pool.contains(&texture_key(frame)))
        .map(String::as_str)
        .collect();
    assert!(missing.is_empty(), "frames in no archive: {missing:?}");
}
