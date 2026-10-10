//! The UI's checks against a real Recoil install: the automap draws every
//! campaign zmap, its objective ids name real objectives, and every string the
//! briefing and HUD ask for is in the table.
//!
//! They skip (not fail) when `BFT_DATA_DIR` is unset or the cache has not been
//! built. Only counts and invariants are asserted; no game text enters the
//! repository.

use bft_assets::level::parse_zmap;
use bft_assets::messages::parse_messages;
use bft_assets::reader::parse_reader_data;
use bft_game::ui::automap::{MapProjection, draw_base};
use bft_game::ui::menus::briefing_lines;
use bft_sim::mission::build_mission_script;

fn paths() -> Option<bft_assets::DataPaths> {
    let root = bft_assets::test_support::data_root()?;
    match bft_assets::DataPaths::discover(Some(&root), None, None) {
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

fn cached(paths: &bft_assets::DataPaths, relative: &str) -> Option<Vec<u8>> {
    let path = paths.cache.path.join(relative);
    if !path.is_file() {
        eprintln!("{} is not in the cache; skipping", path.display());
        return None;
    }
    Some(std::fs::read(&path).expect("the cache is readable"))
}

/// Each campaign map rasterises with ink on it, the extent fits the image,
/// and every objective id on it is an objective the mission has (0-based,
/// READER-SCHEMA §3.6).
#[test]
fn every_campaign_automap_draws_and_names_real_objectives() {
    let Some(paths) = paths() else { return };

    for level in 1..=6u16 {
        let Some(bytes) = cached(&paths, &format!("zmap/m{level}.zmap.json")) else {
            continue;
        };
        let zmap = parse_zmap(&bytes).expect("a real zmap parses");
        let projection = MapProjection::fit(zmap.min, zmap.max, 512);
        assert_eq!(projection.size.max_element(), 512, "m{level}");

        let raster = draw_base(&zmap, &projection);
        let inked = raster.pixels.iter().filter(|pixel| pixel[3] > 0).count();
        assert!(inked > 500, "m{level}: only {inked} texels drawn");

        let Some(reader) = cached(&paths, &format!("reader/m{level}-zrdr.rdr.zip")) else {
            continue;
        };
        let script = build_mission_script(&parse_reader_data(&reader).expect("parses"))
            .expect("a campaign mission has a script");
        for feature in &zmap.features {
            if let Some(objective) = feature.objective {
                assert!(
                    (objective as usize) < script.objectives.len(),
                    "m{level}: feature names objective {objective} of {}",
                    script.objectives.len()
                );
            }
        }
    }
}

/// The briefing resolves every line through the table: no raw `MSG_` key
/// reaches the screen, and every mission starts with something to do.
#[test]
fn every_briefing_resolves_its_text() {
    let Some(paths) = paths() else { return };
    let Some(bytes) = cached(&paths, "messages.json") else {
        return;
    };
    let messages = parse_messages(&bytes).expect("the table parses");

    for key in [
        "MSG_GAME_OVER",
        "MSG_CAMPAIGN_DONE",
        "MSG_LOAD_NEXT_MISSION",
    ] {
        assert!(messages.get(key).is_some(), "{key} is missing");
    }
    for level in 1..=13u16 {
        assert!(
            messages.get(&format!("MSG_CAMPAIGN{level}_NAME")).is_some(),
            "m{level} has no title"
        );
    }

    for level in 1..=6u16 {
        let Some(reader) = cached(&paths, &format!("reader/m{level}-zrdr.rdr.zip")) else {
            continue;
        };
        let script = build_mission_script(&parse_reader_data(&reader).expect("parses"));
        let lines = briefing_lines(script.as_ref(), Some(&messages));

        assert!(!lines.is_empty(), "m{level} briefs nothing");
        for (summary, text) in &lines {
            assert!(!summary.starts_with("MSG_"), "m{level}: {summary}");
            assert!(!text.starts_with("MSG_"), "m{level}: {text}");
        }
    }
}

/// The real `hud.zrd` laid out on five form factors (640×480, 1920×1080,
/// 3440×1440, 3840×2160, 1280×800). Clusters stay apart and on screen, the
/// sensor window keeps at least a fifth of the height, and the strips run edge to edge with the
/// bottom row closed between its clusters. Only the geometry is asserted.
#[test]
fn the_original_hud_fits_every_form_factor() {
    use bevy::math::{Rect, Vec2};
    use bft_assets::hud::hud_layout;
    use bft_game::ui::hud::layout::{HudFrame, StripArt, strip_pieces};
    use std::io::Read;

    let Some(paths) = paths() else { return };
    let (Some(reader), Some(images)) = (
        cached(&paths, "reader/zrdr.rdr.zip"),
        cached(&paths, "textures/image.tex.zip"),
    ) else {
        return;
    };
    let layout = hud_layout(&parse_reader_data(&reader).expect("parses")).expect("hud.zrd parses");
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(images)).expect("a zip");
    let mut json = String::new();
    archive
        .by_name("manifest.json")
        .expect("a manifest")
        .read_to_string(&mut json)
        .expect("readable");
    let manifest: bft_assets::mech3ax::TextureManifest =
        serde_json::from_str(&json).expect("the manifest parses");
    let size = |name: &str| {
        let info = manifest
            .texture_infos
            .iter()
            .find(|info| info.name == name)
            .unwrap_or_else(|| panic!("{name} is in image.zbd"));
        Vec2::new(f32::from(info.width), f32::from(info.height))
    };
    let at = |name: &str, pos: bevy::math::UVec2| {
        Rect::from_corners(pos.as_vec2(), pos.as_vec2() + size(name))
    };

    let canvas = layout.viewport_full.as_rect().size();
    let monitor = at(&layout.sensor.frame.texture, layout.sensor.frame.pos);
    let health = at(&layout.shield.frame.texture, layout.shield.frame.pos);
    let forms = layout
        .modes
        .iter()
        .map(|mode| at(&mode.off, mode.pos))
        .reduce(|a, b| a.union(b))
        .expect("four forms");
    let icon_strip = &layout.strips[2];
    let icons = at(&icon_strip.texture, icon_strip.pos);
    let view = Rect::from_corners(
        monitor.min + layout.sensor.view.as_rect().min,
        monitor.min + layout.sensor.view.as_rect().max,
    );
    let clusters = [monitor, icons, health, forms];

    for (width, height) in [
        (640.0, 480.0),
        (1920.0, 1080.0),
        (3440.0, 1440.0),
        (3840.0, 2160.0),
        (1280.0, 800.0),
    ] {
        let frame = HudFrame::new(Vec2::new(width, height), canvas, 1.0);
        let placed: Vec<Rect> = clusters.iter().map(|rect| frame.place(*rect)).collect();
        for (i, a) in placed.iter().enumerate() {
            assert!(
                a.min.x > -0.01 && a.max.x < width + 0.01,
                "{width}x{height}: {i} off screen"
            );
            for b in &placed[i + 1..] {
                let overlap = a.intersect(*b);
                assert!(
                    overlap.width() < 0.01 || overlap.height() < 0.01,
                    "{width}x{height}: clusters overlap"
                );
            }
        }
        assert!(
            frame.place(view).height() >= 0.2 * height,
            "{width}x{height}: the sensor is too small"
        );

        let variants: Vec<StripArt> = layout
            .wide_strips
            .iter()
            .map(|name| StripArt {
                texture: name.clone(),
                size: size(name),
            })
            .collect();
        let strip = |placed: &bft_assets::hud::Placed| {
            strip_pieces(
                &frame,
                &StripArt {
                    texture: placed.texture.clone(),
                    size: size(&placed.texture),
                },
                placed.pos.as_vec2(),
                &variants,
            )
        };
        let top = strip(&layout.strips[0]);
        assert!(top[0].screen.min.x.abs() < 0.01);
        assert!((top.last().expect("pieces").screen.max.x - width).abs() < 0.01);
        let bottom = strip(icon_strip);
        assert!(
            (bottom[0].screen.min.x - placed[0].max.x).abs() < 0.01,
            "{width}x{height}"
        );
        assert!(
            (bottom.last().expect("pieces").screen.max.x - placed[2].min.x).abs() < 0.01,
            "{width}x{height}"
        );
        for pieces in [&top, &bottom] {
            for pair in pieces.windows(2) {
                assert!((pair[0].screen.max.x - pair[1].screen.min.x).abs() < 0.01);
            }
        }
    }
}

/// The two HUD strings the original HUD reads by key.
#[test]
fn the_hud_strings_are_in_the_table() {
    let Some(paths) = paths() else { return };
    let Some(bytes) = cached(&paths, "messages.json") else {
        return;
    };
    let messages = parse_messages(&bytes).expect("the table parses");
    for key in [bft_game::ui::hud::INCOMING_KEY, "MSG_SCORE"] {
        assert!(messages.get(key).is_some(), "{key} is missing");
    }
}
