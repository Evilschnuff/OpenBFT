//! The `dialog.zrd` / `briefing.zrd` parsers against a real Recoil
//! install.
//!
//! Skips (not fails) when `BFT_DATA_DIR` is unset or the cache has not been
//! built. Like `real_hud.rs`, it asserts what the parser finds and that
//! every bitmap and string the layouts name resolves; nothing from the files
//! is copied into the repository.

use std::io::Read;
use std::path::PathBuf;

use bft_assets::DataPaths;
use bft_assets::dialog::{ControlKind, briefing_layouts, dialog_set, transport_text};
use bft_assets::mech3ax::TextureManifest;
use bft_assets::messages::{Messages, parse_messages};
use bft_assets::reader::{ReaderData, parse_reader_data};
use bft_assets::textures::texture_key;

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

fn image_names(paths: &DataPaths) -> Option<Vec<String>> {
    let bytes = cached(paths, "textures/image.tex.zip")?;
    let mut archive =
        zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("image.tex.zip is a zip");
    let mut json = String::new();
    archive
        .by_name("manifest.json")
        .expect("the archive has a manifest")
        .read_to_string(&mut json)
        .expect("the manifest reads");
    let manifest: TextureManifest = serde_json::from_str(&json).expect("the manifest parses");
    Some(
        manifest
            .texture_infos
            .into_iter()
            .map(|info| info.name)
            .collect(),
    )
}

fn messages(paths: &DataPaths) -> Option<Messages> {
    let bytes = cached(paths, "messages.json")?;
    Some(parse_messages(&bytes).expect("messages.json parses"))
}

/// The eighteen dialogs `dialog.zrd` holds, each with the pieces the menus
/// draw.
#[test]
fn dialog_zrd_has_every_dialog_the_menus_map() {
    let Some(paths) = paths() else { return };
    let Some(reader) = global_reader(&paths) else {
        return;
    };
    let set = dialog_set(&reader).expect("the real dialog.zrd parses");

    let mut names: Vec<&str> = set.dialogs.keys().map(String::as_str).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "CHEAT_CODE_DIALOG",
            "COMMANDS_DIALOG",
            "CONFIRM_QUIT",
            "CONTROLS_DIALOG",
            "CREDITSPANEL",
            "LOAD_GAME_DIALOG",
            "MAINMENU0",
            "MAINMENU1",
            "MAINMENU2",
            "MAINMENU3",
            "MESSAGEBOX",
            "MPEXIT",
            "MP_NEW_GAME",
            "NETEXIT",
            "NEWGAMEPANEL",
            "OPTIONSPANEL",
            "SAVE_GAME_DIALOG",
            "WOL_NEW_GAME",
        ]
    );

    for dialog in set.dialogs.values() {
        assert!(!dialog.fonts.is_empty(), "{} has fonts", dialog.name);
        assert!(!dialog.backgrounds.is_empty(), "{} has art", dialog.name);
        assert!(!dialog.controls.is_empty(), "{} has controls", dialog.name);
        assert!(dialog.cursor.is_some(), "{} names a pointer", dialog.name);
        // Every text names a font the dialog defines.
        for text in &dialog.texts {
            assert!(dialog.font(text.font).is_some(), "{}", dialog.name);
        }
    }

    // The controls the screens map, with the kinds the renderer expects.
    let kind = |dialog: &str, control: &str| {
        set.get(dialog)
            .and_then(|d| d.control(control))
            .map(|c| c.kind.clone())
            .unwrap_or_else(|| panic!("{dialog} has no {control}"))
    };
    for control in [
        "NEWGAME", "LOADGAME", "OPTIONS", "CONTROLS", "CREDITS", "QUIT",
    ] {
        assert_eq!(kind("MAINMENU0", control), ControlKind::Button);
    }
    assert_eq!(kind("MAINMENU1", "BACK"), ControlKind::Button);
    assert_eq!(kind("MAINMENU1", "SAVEGAME"), ControlKind::Button);
    assert!(
        matches!(kind("NEWGAMEPANEL", "INTENSITY"), ControlKind::Radio(items) if items.len() == 3)
    );
    assert_eq!(kind("NEWGAMEPANEL", "NAME"), ControlKind::Label);
    assert_eq!(kind("SAVE_GAME_DIALOG", "GAMENAME"), ControlKind::Label);
    assert!(matches!(
        kind("OPTIONSPANEL", "SOUND_VOLUME"),
        ControlKind::Slider { .. }
    ));
    assert!(matches!(
        kind("OPTIONSPANEL", "FULLHUD"),
        ControlKind::Checkbox(_)
    ));
    assert!(
        matches!(kind("OPTIONSPANEL", "RESOLUTION_CYCLE"), ControlKind::Cycle(states) if states.len() == 6)
    );
    assert!(matches!(
        kind("COMMANDS_DIALOG", "CMD_KEYA_BTN"),
        ControlKind::List(_)
    ));
    assert!(
        matches!(kind("CREDITSPANEL", "CREDITS_SCREEN"), ControlKind::Scroller(roll) if !roll.lines.is_empty())
    );

    // The save and load lists: nine rows around the name bar.
    for dialog in ["SAVE_GAME_DIALOG", "LOAD_GAME_DIALOG"] {
        let rows = set
            .get(dialog)
            .map(|d| {
                d.primitives
                    .iter()
                    .filter(|p| p.name.starts_with("LIST_"))
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(rows, 9, "{dialog}");
    }
    assert!(
        set.get("MESSAGEBOX")
            .and_then(|d| d.primitive("MESSAGE"))
            .and_then(|p| p.wordwrap)
            .is_some()
    );

    // Every rollover and select sound is one of the `DIALOG` set's two.
    for dialog in set.dialogs.values() {
        for control in &dialog.controls {
            for cue in [&control.sounds.rollover, &control.sounds.activate]
                .into_iter()
                .flatten()
            {
                assert!(
                    cue.name == "snd_rollover" || cue.name == "snd_select",
                    "{}.{}: {}",
                    dialog.name,
                    control.name,
                    cue.name
                );
            }
        }
    }
}

/// Every bitmap either file names is in `image.zbd`, once lower-cased, and
/// every `MSG_*` key is in the string table.
#[test]
fn every_layout_bitmap_and_string_resolves() {
    let Some(paths) = paths() else { return };
    let (Some(reader), Some(images), Some(messages)) =
        (global_reader(&paths), image_names(&paths), messages(&paths))
    else {
        return;
    };
    let set = dialog_set(&reader).expect("the real dialog.zrd parses");
    let briefings = briefing_layouts(&reader).expect("the real briefing.zrd parses");

    let dialogs = set
        .dialogs
        .values()
        .chain(briefings.iter().map(|layout| &layout.dialog));
    let mut checked = 0;
    for dialog in dialogs {
        for bitmap in dialog.bitmaps() {
            let key = texture_key(bitmap);
            assert!(
                images.contains(&key),
                "{}: {bitmap} is not in image.zbd",
                dialog.name
            );
            checked += 1;
        }
        for key in dialog.message_keys() {
            assert!(
                messages.get(key).is_some(),
                "{}: {key} is not in messages.json",
                dialog.name
            );
        }
    }
    assert!(checked > 200, "{checked} bitmaps checked");
}

/// Thirteen briefing layouts; the campaign's with an objective panel and
/// map locators, every one with a transport bar. The transport words come
/// from `hud.zrd`.
#[test]
fn briefing_zrd_lays_out_every_level() {
    let Some(paths) = paths() else { return };
    let Some(reader) = global_reader(&paths) else {
        return;
    };
    let layouts = briefing_layouts(&reader).expect("the real briefing.zrd parses");

    assert_eq!(
        layouts.iter().map(|l| l.campaign).collect::<Vec<_>>(),
        (1..=13).collect::<Vec<_>>()
    );
    for layout in &layouts {
        assert!(
            matches!(
                layout.transport().map(|c| &c.kind),
                Some(ControlKind::Slider { .. })
            ),
            "CAMPAIGN{} has a transport bar",
            layout.campaign
        );
        let campaign = layout.campaign <= 6;
        for primitive in ["OBJECTIVE_PICT", "OBJECTIVE_SUMMARY", "OBJECTIVE_DESC"] {
            assert_eq!(
                layout.dialog.primitive(primitive).is_some(),
                campaign,
                "CAMPAIGN{}: {primitive}",
                layout.campaign
            );
        }
        let locators = layout.locators().len();
        assert_eq!(locators > 0, campaign, "CAMPAIGN{}", layout.campaign);
        assert!(locators <= 6);
    }

    let transport = transport_text(&reader).expect("hud.zrd has BRIEFING");
    assert!(!transport.loading.is_empty() && !transport.ready.is_empty());
    assert!(transport.stages >= 1);
}

/// The splash is `Recoil.exe`'s bitmap resource: a 640×480 picture.
#[test]
fn the_splash_is_read_from_the_executable() {
    let Some(root) = bft_assets::test_support::data_root() else {
        return;
    };
    let Some(picture) = bft_assets::splash::read_splash(&root) else {
        panic!("{} has a Recoil.exe with a bitmap", root.display());
    };
    assert_eq!((picture.width, picture.height), (640, 480));
    assert_eq!(picture.rgba.len(), 640 * 480 * 4);
    // Not blank: the wallpaper is mostly red.
    let red = picture
        .rgba
        .chunks(4)
        .filter(|p| p[0] > p[1] && p[0] > p[2])
        .count();
    assert!(red > 640 * 480 / 2, "{red} red pixels");
}
