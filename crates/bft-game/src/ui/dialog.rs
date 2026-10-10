//! The original menus: `dialog.zrd`'s and `briefing.zrd`'s
//! layouts drawn with `image.zbd`'s art.
//!
//! Our own simple menus are the fallback ([`MenuStyle::Fallback`]): they show
//! without an install, when a layout or any bitmap a dialog needs is
//! missing, or when the player picks "Menus: Simple" in the settings.
//!
//! # How a screen is built
//!
//! One generic renderer ([`render`]) draws any dialog: its backgrounds, its
//! text, and each control with its hover, pressed and disabled bitmaps and
//! its rollover and select sounds. A screen ([`screens`], [`briefing`])
//! only says what each control *does*, by name — `NEWGAME` opens the new
//! game panel, `SOUND_VOLUME` is the effects volume — so the click flow is
//! the simple menus' own: a control carries a [`UiButton`](super::theme::UiButton),
//! and handlers read [`UiClick`](super::theme::UiClick). A control this game
//! has no use for is drawn and does nothing.
//!
//! # Where a dialog goes on screen
//!
//! [`frame`]: 640×480 designs, scaled by height, anchored left — the panel
//! strips down the left edge are the dialogs' spine, and the rest hangs off
//! them. Behind the front-end dialogs is the game's splash, the red RECOIL
//! wallpaper ([`Splash`], from `Recoil.exe`); behind the in-game ones the
//! game, blurred ([`GameBackdrop`]), as the original did (`fmv.zrd`'s
//! `BLUR`).

use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use bft_assets::catalog::LevelCatalog;
use bft_assets::dialog::{
    BriefingLayout, Dialog, DialogSet, TransportText, briefing_layouts, dialog_set, transport_text,
};
use bft_assets::reader::ReaderData;
use bft_assets::textures::{TextureSet, texture_key};
use bft_sim::states::AppState;

use super::hud::original::{HudArt, copy_art, image_archive};
use super::settings::Settings;
use crate::globals::Globals;

pub mod briefing;
pub mod frame;
pub mod render;
pub mod screens;

/// Which menus this install can draw.
///
/// `Pending` until the global reader and `image.zbd` have loaded; then
/// `Original` with the parsed layouts and the art, or `Fallback` when
/// either is missing.
#[derive(Resource, Debug, Default)]
pub enum MenuStyle {
    #[default]
    Pending,
    Original(Box<OriginalMenus>),
    Fallback,
}

/// The original menus' layouts and art.
#[derive(Debug, Clone, Default)]
pub struct OriginalMenus {
    pub dialogs: DialogSet,
    /// `briefing.zrd`'s `CAMPAIGN<n>` layouts; empty when the file is
    /// missing or unreadable, which costs the briefing screens only.
    pub briefings: Vec<BriefingLayout>,
    /// `hud.zrd`'s transport words.
    pub transport: Option<TransportText>,
    /// Every bitmap any layout names that the install has, by texture key.
    pub art: HashMap<String, HudArt>,
    /// The dialogs and briefings whose every bitmap is here: only those
    /// are drawn, so a dialog is never half there.
    complete: HashSet<String>,
}

impl OriginalMenus {
    /// Collect the art for every layout, noting which are complete.
    pub fn assemble(
        dialogs: DialogSet,
        briefings: Vec<BriefingLayout>,
        transport: Option<TransportText>,
        mut lookup: impl FnMut(&str) -> Option<HudArt>,
    ) -> OriginalMenus {
        let mut art = HashMap::new();
        let mut complete = HashSet::new();
        let mut missing: Vec<String> = Vec::new();

        let layouts = dialogs
            .dialogs
            .values()
            .map(|dialog| (dialog.name.clone(), dialog))
            .chain(
                briefings
                    .iter()
                    .map(|layout| (briefing_key(layout.campaign), &layout.dialog)),
            );
        for (key, dialog) in layouts {
            let mut whole = true;
            for name in dialog.bitmaps() {
                let texture = texture_key(name);
                if art.contains_key(&texture) {
                    continue;
                }
                match lookup(&texture) {
                    Some(found) => {
                        art.insert(texture, found);
                    }
                    None => {
                        whole = false;
                        missing.push(format!("{key}: {name}"));
                    }
                }
            }
            if whole {
                complete.insert(key);
            }
        }
        if !missing.is_empty() {
            missing.sort();
            warn!(
                "image.zbd lacks {} menu bitmap(s); those dialogs use the simple menus: {}",
                missing.len(),
                missing.join(", ")
            );
        }

        OriginalMenus {
            dialogs,
            briefings,
            transport,
            art,
            complete,
        }
    }

    /// A dialog, if all of its art is here.
    pub fn dialog(&self, name: &str) -> Option<&Dialog> {
        self.dialogs
            .get(name)
            .filter(|_| self.complete.contains(name))
    }

    /// A level's briefing layout, if all of its art is here.
    pub fn briefing(&self, campaign: u32) -> Option<&BriefingLayout> {
        self.briefings
            .iter()
            .find(|layout| layout.campaign == campaign)
            .filter(|_| self.complete.contains(&briefing_key(campaign)))
    }

    /// A bitmap by the name a layout spells it with.
    pub fn art(&self, name: &str) -> Option<&HudArt> {
        self.art.get(&texture_key(name))
    }
}

fn briefing_key(campaign: u32) -> String {
    format!("CAMPAIGN{campaign}")
}

/// Whether the original menus are the ones to draw: available and wanted.
pub fn uses_original_menus(style: &MenuStyle, settings: &Settings) -> bool {
    matches!(style, MenuStyle::Original(_)) && settings.menus.original
}

/// The original menus, when they are the ones to draw.
pub fn original_menus<'a>(
    style: Option<&'a MenuStyle>,
    settings: Option<&Settings>,
) -> Option<&'a OriginalMenus> {
    match style? {
        MenuStyle::Original(menus) if settings.is_none_or(|s| s.menus.original) => Some(menus),
        _ => None,
    }
}

/// Run condition: draw the original menus.
pub fn original_style(style: Option<Res<MenuStyle>>, settings: Option<Res<Settings>>) -> bool {
    original_menus(style.as_deref(), settings.as_deref()).is_some()
}

/// Run condition: draw the simple menus. Not while the style is still
/// being worked out, so the front end does not flash the simple menus
/// before the original ones; an app without the resource (a test) is
/// settled on the fallback.
pub fn fallback_style(style: Option<Res<MenuStyle>>, settings: Option<Res<Settings>>) -> bool {
    match style.as_deref() {
        None => true,
        Some(MenuStyle::Pending) => false,
        Some(style) => original_menus(Some(style), settings.as_deref()).is_none(),
    }
}

/// The game's splash, `Recoil.exe`'s `GAMEBMP` (docs/data/menus.md): the red
/// RECOIL wallpaper behind the boot screen and the front-end dialogs.
#[derive(Resource, Debug, Clone)]
pub struct Splash(pub Handle<Image>);

/// Read the splash from the install, when there is one. Called from
/// `UiPlugin::build`, so the boot screen can show it from its first frame.
pub fn install_splash(app: &mut App) {
    let Some(root) = app
        .world()
        .get_resource::<bft_assets::DataPaths>()
        .map(|paths| paths.root.path.clone())
    else {
        return;
    };
    let Some(picture) = bft_assets::splash::read_splash(&root) else {
        warn!("no splash in {}'s Recoil.exe", root.display());
        return;
    };
    let Some(mut images) = app.world_mut().get_resource_mut::<Assets<Image>>() else {
        return;
    };
    let image = Image::new(
        bevy::render::render_resource::Extent3d {
            width: picture.width,
            height: picture.height,
            depth_or_array_layers: 1,
        },
        bevy::render::render_resource::TextureDimension::D2,
        picture.rgba,
        bevy::render::render_resource::TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    );
    let handle = images.add(image);
    app.insert_resource(Splash(handle));
}

/// The game view behind the in-game menus: the capture taken as the pause
/// menu opened, blurred, as the original showed its menus in play.
#[derive(Resource, Debug, Clone)]
pub struct GameBackdrop(pub Handle<Image>);

/// The loading state behind [`MenuStyle::Pending`].
#[derive(Resource, Debug, Default)]
pub struct MenuArtLoad {
    /// `image.zbd`'s archive, kept for the pictures no layout names: the
    /// briefing's objective pictures, named by each mission's objectives.
    pub(crate) set: Option<Handle<TextureSet>>,
}

/// The menus' systems.
pub fn plugin(app: &mut App) {
    app.init_resource::<MenuStyle>()
        .init_resource::<MenuArtLoad>()
        .add_systems(Update, resolve_menu_style);
    render::plugin(app);
    screens::plugin(app);
    briefing::plugin(app);
}

/// Settle [`MenuStyle`] once the global reader and `image.zbd` have loaded.
// Nine parameters: the two sources, the stores they land in, the state
// that says whether to keep waiting, and where to write the answer.
#[allow(clippy::too_many_arguments)]
pub fn resolve_menu_style(
    mut style: ResMut<MenuStyle>,
    mut load: ResMut<MenuArtLoad>,
    globals: Option<Res<Globals>>,
    catalog: Option<Res<LevelCatalog>>,
    state: Option<Res<State<AppState>>>,
    server: Res<AssetServer>,
    readers: Res<Assets<ReaderData>>,
    sets: Res<Assets<TextureSet>>,
    mut images: ResMut<Assets<Image>>,
) {
    if !matches!(*style, MenuStyle::Pending) {
        return;
    }
    let fallback = |style: &mut MenuStyle, why: &str| {
        warn!("the original menus are unavailable ({why}); using the simple ones");
        *style = MenuStyle::Fallback;
    };
    let (Some(globals), Some(catalog)) = (globals, catalog) else {
        // Past boot without the globals: nothing is coming.
        if state.is_some_and(|state| *state.get() != AppState::Boot) {
            fallback(&mut style, "no game data loaded");
        }
        return;
    };

    let Some(reader) = globals.reader.clone() else {
        return fallback(&mut style, "no global reader archive");
    };
    if load.set.is_none() {
        let Some(path) = image_archive(&catalog) else {
            return fallback(&mut style, "no image.zbd in the cache");
        };
        load.set = Some(server.load::<TextureSet>(path));
    }
    let Some(set_handle) = load.set.clone() else {
        return;
    };

    let settled = |id: bevy::asset::UntypedAssetId| {
        use bevy::asset::LoadState;
        match server.load_state(id) {
            LoadState::Loaded => Some(true),
            LoadState::Failed(_) => Some(false),
            _ => None,
        }
    };
    let (Some(reader_ok), Some(set_ok)) = (
        settled(reader.id().untyped()),
        settled(set_handle.id().untyped()),
    ) else {
        return;
    };
    let (true, true) = (reader_ok, set_ok) else {
        return fallback(&mut style, "the reader or image archive failed to load");
    };
    let (Some(reader), Some(set)) = (readers.get(&reader), sets.get(&set_handle)) else {
        return;
    };

    let dialogs = match dialog_set(reader) {
        Ok(dialogs) => dialogs,
        Err(e) => return fallback(&mut style, &e.to_string()),
    };
    let briefings = briefing_layouts(reader).unwrap_or_else(|e| {
        warn!("no original briefings ({e}); loading shows the simple screen");
        Vec::new()
    });
    let transport = transport_text(reader)
        .inspect_err(|e| warn!("no transport words ({e})"))
        .ok();

    let copy = |name: &str| {
        let source = set.image(name)?;
        let image = images.get(source)?.clone();
        let size = image.size().as_vec2();
        Some(HudArt {
            image: images.add(copy_art(image, bevy::image::ImageFilterMode::Linear)),
            size,
        })
    };
    let menus = OriginalMenus::assemble(dialogs, briefings, transport, copy);
    info!(
        "original menus: {} dialog(s), {} briefing(s), {} image(s)",
        menus.dialogs.dialogs.len(),
        menus.briefings.len(),
        menus.art.len()
    );
    *style = MenuStyle::Original(Box::new(menus));
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::IVec2;
    use bft_assets::dialog::{Control, ControlKind, ControlSounds, Placed};

    fn dialog(name: &str, bitmaps: &[&str]) -> Dialog {
        Dialog {
            name: name.into(),
            backgrounds: bitmaps
                .iter()
                .map(|bitmap| Placed {
                    bitmap: bitmap.to_string(),
                    pos: IVec2::ZERO,
                })
                .collect(),
            controls: vec![Control {
                name: "GO".into(),
                pos: IVec2::ZERO,
                kind: ControlKind::Button,
                bitmap: None,
                rollover: None,
                activate: None,
                disabled: None,
                labels: Vec::new(),
                sounds: ControlSounds::default(),
            }],
            ..Dialog::default()
        }
    }

    /// A dialog missing one bitmap is not drawn; the others are, and names
    /// are matched without regard to case.
    #[test]
    fn a_dialog_with_missing_art_is_left_to_the_fallback() {
        let mut set = DialogSet::default();
        set.dialogs
            .insert("WHOLE".into(), dialog("WHOLE", &["Strip", "NewGame_H"]));
        set.dialogs
            .insert("BROKEN".into(), dialog("BROKEN", &["Strip", "Gone"]));

        let menus = OriginalMenus::assemble(set, Vec::new(), None, |name| {
            (name != "gone").then(|| HudArt {
                image: Handle::default(),
                size: Vec2::ONE,
            })
        });

        assert!(menus.dialog("WHOLE").is_some());
        assert!(menus.dialog("BROKEN").is_none());
        assert!(menus.art("NEWGAME_h").is_some());
    }

    #[test]
    fn the_settings_can_turn_the_original_menus_off() {
        let style = MenuStyle::Original(Box::default());
        let mut settings = Settings::default();
        assert!(uses_original_menus(&style, &settings));
        settings.menus.original = false;
        assert!(!uses_original_menus(&style, &settings));
        assert!(!uses_original_menus(
            &MenuStyle::Fallback,
            &Settings::default()
        ));
    }
}
