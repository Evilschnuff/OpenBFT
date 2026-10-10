//! The `hud.zrd` parser against a real Recoil install.
//!
//! Skips (not fails) when `BFT_DATA_DIR` is unset or the cache has not been
//! built. It asserts what the parser finds in the file, the way
//! `bft-game/tests/real_ui.rs` does: nothing from the file is copied into
//! the repository, and the HUD itself never reads these numbers from here.

use std::io::Read;
use std::path::PathBuf;

use bft_assets::DataPaths;
use bft_assets::TextureAlpha;
use bft_assets::hud::{HUD_ENTRY, hud_layout};
use bft_assets::mech3ax::TextureManifest;
use bft_assets::reader::{ReaderData, parse_reader_data};
use bft_assets::reader_value::ReaderValue;

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

fn cached(paths: &DataPaths, relative: &str) -> Option<Vec<u8>> {
    let path: PathBuf = paths.cache.path.join(relative);
    if !path.is_file() {
        eprintln!("{} is not in the cache; skipping", path.display());
        return None;
    }
    Some(std::fs::read(&path).expect("the cache is readable"))
}

fn global_reader(paths: &DataPaths) -> Option<ReaderData> {
    let bytes = cached(paths, "reader/zrdr.rdr.zip")?;
    Some(parse_reader_data(&bytes).expect("the global reader parses"))
}

fn image_manifest(paths: &DataPaths) -> Option<TextureManifest> {
    let bytes = cached(paths, "textures/image.tex.zip")?;
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("image.tex.zip is a zip");
    let mut json = String::new();
    archive
        .by_name("manifest.json")
        .expect("the archive has a manifest")
        .read_to_string(&mut json)
        .expect("the manifest reads");
    Some(serde_json::from_str(&json).expect("the manifest parses"))
}

/// The sections are the ones READER-SCHEMA §2.7a lists, in order, and the parser
/// takes all of them.
#[test]
fn hud_zrd_has_the_sections_the_hud_reads() {
    let Some(paths) = paths() else { return };
    let Some(reader) = global_reader(&paths) else {
        return;
    };

    let entry = reader
        .entry(HUD_ENTRY)
        .expect("the global archive has hud.zrd");
    let keys: Vec<&str> = ReaderValue::root(entry)
        .pairs()
        .into_iter()
        .map(|(key, _)| key)
        .collect();
    assert_eq!(
        keys,
        [
            "VERSION",
            "FONTS",
            "TYPEI",
            "TYPEII",
            "RETICULE",
            "SENSOR",
            "SHIELD",
            "NANITE",
            "OBJECTIVE",
            "STATS",
            "TARGET",
            "WEAPON",
            "MODES",
            "BRIEFING"
        ]
    );

    let layout = hud_layout(&reader).expect("the real hud.zrd parses");
    assert_eq!(layout.version, 8);
    // 640×480 art: the full view is the whole screen, the strip view a band.
    assert_eq!(
        (layout.viewport_full.width(), layout.viewport_full.height()),
        (640, 480)
    );
    assert_eq!(layout.viewport_strip.width(), 640);
    assert!(layout.viewport_strip.min.y > 0 && layout.viewport_strip.max.y < 480);
    assert_eq!(layout.strips.len(), 3);
    assert_eq!(layout.wide_strips.len(), 4);
    assert_eq!(layout.sensor.zoom, 4.0);
    assert_eq!(layout.sensor.frame.texture, "monitor");
    assert!(layout.sensor.reticle.centred);
    assert_eq!(layout.shield.frame.texture, "health");
    assert_eq!(layout.nanite.slots.len(), 3);
    assert_eq!(layout.objective.tab.texture, "objectab");
    assert_eq!(layout.objective.slide_time, 0.5);
    assert_eq!(layout.reticle.off, "ret_off");
    assert_eq!(layout.target.threats[0], "threat_l");
    assert_eq!(layout.modes[0].picked, "tank_pik");
    // Row 7 pairs the sonic wave's frames with the arc sabre's.
    assert_eq!(layout.weapons[6].frame(false, false), "ic_sonc1");
    assert_eq!(layout.weapons[6].frame(true, false), "ic_arc1");
    assert_eq!(layout.fonts.ammo.face, "Symbol");

    // The sensor's cross-hair is placed by its centre, and that centre is
    // the centre of the view it sits over.
    assert_eq!(
        layout.sensor.reticle.pos.as_vec2(),
        layout.sensor.view.as_rect().center()
    );
}

/// Every texture the layout names is in `image.zbd`, and the first flag of
/// a placed image is exactly "this texture has alpha".
#[test]
fn every_hud_texture_is_in_image_zbd() {
    let Some(paths) = paths() else { return };
    let (Some(reader), Some(manifest)) = (global_reader(&paths), image_manifest(&paths)) else {
        return;
    };
    let layout = hud_layout(&reader).expect("the real hud.zrd parses");

    let find = |name: &str| manifest.texture_infos.iter().find(|info| info.name == name);
    for name in layout.textures() {
        assert!(find(name).is_some(), "{name} is not in image.zbd");
    }

    let placed = layout
        .strips
        .iter()
        .chain([
            &layout.sensor.frame,
            &layout.sensor.reticle,
            &layout.shield.frame,
            &layout.objective.tab,
        ])
        .chain(&layout.nanite.slots);
    for image in placed {
        let info = find(&image.texture).expect("checked above");
        assert_eq!(
            image.transparent,
            info.alpha != TextureAlpha::None,
            "{}: flag {} but alpha {:?}",
            image.texture,
            image.transparent,
            info.alpha
        );
    }

    // The wide strips are the 640 ones plus 160 and 384: 800 and 1024
    // screens, with the side clusters unchanged.
    let width = |name: &str| u32::from(find(name).expect("present").width);
    let top = &layout.strips[0].texture;
    for wide in &layout.wide_strips {
        let base = if wide.starts_with("tops") {
            top.as_str()
        } else {
            layout.strips[2].texture.as_str()
        };
        let extra = width(wide) - width(base);
        assert!(extra == 160 || extra == 384, "{wide}: {extra} wider");
    }
}
