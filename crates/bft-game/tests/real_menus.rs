//! The original menus against a real Recoil install: every page
//! draws from the real layouts, in the front end and in play, with the
//! controls the screens wire present by name, and every control and
//! background lands on screen at the form factors the HUD is checked at.
//!
//! Skips (not fails) when `BFT_DATA_DIR` is unset or the cache has not been
//! built. The art itself is not loaded — the manifest gives each bitmap's
//! size — and nothing from the files enters the repository.

use std::io::Read;

use bevy::ecs::system::RunSystemOnce;
use bevy::prelude::*;
use bft_assets::dialog::{briefing_layouts, dialog_set, transport_text};
use bft_assets::mech3ax::TextureManifest;
use bft_assets::messages::Messages;
use bft_assets::reader::parse_reader_data;
use bft_game::savegame::SaveListing;
use bft_game::ui::dialog::frame::{DIALOG_CANVAS, DialogFrame};
use bft_game::ui::dialog::render::{DialogControl, DialogRoot, DialogSlider, OnCanvas};
use bft_game::ui::dialog::screens::{
    CommandsView, Page, PageContext, Place, SaveBrowser, spawn_page,
};
use bft_game::ui::dialog::{MenuStyle, OriginalMenus};
use bft_game::ui::hud::original::HudArt;
use bft_game::ui::settings::Settings;
use bft_game::ui::settings_screen::RebindState;
use bft_game::ui::theme::{UiButton, UiFonts};

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

/// The real layouts, with every bitmap stood in for by an empty image of
/// its real size.
fn real_menus(paths: &bft_assets::DataPaths) -> Option<OriginalMenus> {
    let reader = parse_reader_data(&cached(paths, "reader/zrdr.rdr.zip")?)
        .expect("the global reader parses");
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

    let menus = OriginalMenus::assemble(
        dialog_set(&reader).expect("dialog.zrd parses"),
        briefing_layouts(&reader).expect("briefing.zrd parses"),
        transport_text(&reader).ok(),
        |name| {
            let info = manifest
                .texture_infos
                .iter()
                .find(|info| info.name == name)?;
            Some(HudArt {
                image: Handle::default(),
                size: Vec2::new(f32::from(info.width), f32::from(info.height)),
            })
        },
    );
    Some(menus)
}

fn world(menus: OriginalMenus) -> World {
    let mut world = World::new();
    world.insert_resource(MenuStyle::Original(Box::new(menus)));
    world.insert_resource(Settings::default());
    world.init_resource::<UiFonts>();
    world.init_resource::<Assets<Messages>>();
    world.init_resource::<SaveListing>();
    world.init_resource::<SaveBrowser>();
    world.init_resource::<CommandsView>();
    world.init_resource::<RebindState>();
    world.init_resource::<bft_game::savegame::SaveThumbnails>();
    world
}

/// Every page draws from the real file in both places, as the dialog it
/// should be, with the controls its screen wires found by name.
#[test]
fn every_page_draws_from_the_real_layouts() {
    let Some(paths) = paths() else { return };
    let Some(menus) = real_menus(&paths) else {
        return;
    };

    // (page, place, dialog, controls that must be wired to an action)
    let pages: [(Page, Place, &str, &[&str]); 12] = [
        (
            Page::Main,
            Place::FrontEnd,
            "MAINMENU0",
            &[
                "NEWGAME", "LOADGAME", "OPTIONS", "CONTROLS", "CREDITS", "QUIT",
            ],
        ),
        (
            Page::Main,
            Place::InGame,
            "MAINMENU1",
            &["BACK", "SAVEGAME", "LOADGAME", "OPTIONS", "QUIT"],
        ),
        (
            Page::NewGame,
            Place::FrontEnd,
            "NEWGAMEPANEL",
            &["BACK", "START", "INTENSITY[0]", "INTENSITY[2]"],
        ),
        (
            Page::Options,
            Place::InGame,
            "OPTIONSPANEL",
            &[
                "BACK",
                "MUSIC_ENABLE",
                "SOUND_ACTIVE",
                "FULLHUD",
                "RESOLUTION_CYCLE",
            ],
        ),
        (
            Page::Controls,
            Place::FrontEnd,
            "CONTROLS_DIALOG",
            &["RESUME", "COMMANDS_BTN"],
        ),
        (
            Page::Commands,
            Place::FrontEnd,
            "COMMANDS_DIALOG",
            &[
                "CMD_RESUME_BTN",
                "CMD_SET_BTN",
                "CMD_NEXT_CMD_BTN",
                "CMD_RESET_BTN",
            ],
        ),
        (Page::Credits, Place::FrontEnd, "CREDITSPANEL", &["BACK"]),
        (
            Page::ConfirmQuit,
            Place::InGame,
            "CONFIRM_QUIT",
            &["OK_TO_QUIT", "CANCEL_QUIT"],
        ),
        (
            Page::Saves,
            Place::InGame,
            "SAVE_GAME_DIALOG",
            &["BACK", "SAVE", "PREV_GAME_BTN", "NEXT_GAME_BTN"],
        ),
        (
            Page::Loads,
            Place::FrontEnd,
            "LOAD_GAME_DIALOG",
            &["BACK", "PREV_GAME_BTN", "NEXT_GAME_BTN"],
        ),
        (Page::Cheat, Place::InGame, "CHEAT_CODE_DIALOG", &["GO"]),
        (Page::Options, Place::FrontEnd, "OPTIONSPANEL", &["BACK"]),
    ];

    for (page, place, dialog, wired) in pages {
        let mut world = world(menus.clone());
        let root = world
            .run_system_once(move |mut commands: Commands, ctx: PageContext| {
                spawn_page(&mut commands, &ctx, page, place, false)
            })
            .expect("the page system runs")
            .unwrap_or_else(|| panic!("{dialog} draws"));
        world.flush();

        assert_eq!(
            world.get::<DialogRoot>(root).map(|root| root.name.as_str()),
            Some(dialog)
        );
        let mut controls = world.query::<(&DialogControl, Option<&UiButton>)>();
        for name in wired {
            let found = controls
                .iter(&world)
                .find(|(control, _)| control.name == *name);
            assert!(
                found.is_some_and(|(_, button)| button.is_some()),
                "{dialog}: {name} is drawn and wired"
            );
        }
        if dialog == "OPTIONSPANEL" {
            let sliders = world.query::<&DialogSlider>().iter(&world).count();
            assert_eq!(sliders, 2, "the music and sound volumes");
        }
    }
}

/// On every form factor, every control and background of every dialog
/// lands inside the window's width, and the canvas it hangs from is whole
/// on screen: nothing to click is ever off the edge.
#[test]
fn every_dialog_fits_every_screen() {
    let Some(paths) = paths() else { return };
    let Some(menus) = real_menus(&paths) else {
        return;
    };
    let screens = [
        Vec2::new(640.0, 480.0),
        Vec2::new(1920.0, 1080.0),
        Vec2::new(3440.0, 1440.0),
        Vec2::new(3840.0, 2160.0),
        Vec2::new(1280.0, 800.0),
    ];

    for page in [
        Page::Main,
        Page::NewGame,
        Page::Options,
        Page::Controls,
        Page::Commands,
        Page::Credits,
        Page::Saves,
    ] {
        let mut world = world(menus.clone());
        world
            .run_system_once(move |mut commands: Commands, ctx: PageContext| {
                spawn_page(&mut commands, &ctx, page, Place::InGame, false)
            })
            .expect("the page system runs");
        world.flush();

        let mut placed = world.query_filtered::<&OnCanvas, With<DialogControl>>();
        let rects: Vec<Rect> = placed
            .iter(&world)
            .filter_map(|on| Some(Rect::from_corners(on.pos, on.pos + on.size?)))
            .collect();
        assert!(!rects.is_empty(), "{page:?} has controls");
        for rect in &rects {
            assert!(
                rect.min.x >= 0.0 && rect.max.x <= DIALOG_CANVAS.x,
                "{page:?}: a control at {rect:?} is off the canvas"
            );
        }
        for screen in screens {
            let frame = DialogFrame::new(screen);
            for rect in &rects {
                let on_screen = frame.place(*rect);
                assert!(
                    on_screen.min.x >= 0.0 && on_screen.max.x <= screen.x + 0.5,
                    "{page:?} at {screen}: {on_screen:?}"
                );
                assert!(on_screen.max.y <= screen.y + 0.5);
            }
            // Readable: the dialogs keep their CRT share of the height,
            // whatever the width.
            assert!(frame.canvas().height() >= screen.y.min(screen.x * 0.75) - 0.5);
        }
    }
}
