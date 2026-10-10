//! The original HUD: `hud.zrd`'s layout, `image.zbd`'s art.
//!
//! Nothing here is drawn from our own numbers. The layout is parsed from
//! the user's install ([`bft_assets::hud`]), the images are the install's,
//! and [`super::layout`] only decides where the 640×480 clusters go on a
//! larger screen. Without an install, or with any texture missing,
//! [`HudStyle`] is [`HudStyle::Fallback`] and the fallback HUD draws
//! instead.
//!
//! # What each part shows
//!
//! Checked against the manual (pp. 18–19, 40) and the art itself;
//! docs/data/hud.md has the details.
//!
//! - **Reticle**: the cursor itself ([`crate::ui::cursor`]), in the look
//!   [`reticle_texture`] picks: the bare circle when nothing is within the
//!   selected weapon's range, red triangles when a shot would hit something,
//!   green when that something can be damaged. The sim's aim ray is already
//!   cast to the weapon's range, so "a hit" is "in range".
//! - **Sensor**: a second camera, zoomed on the cursor ([`super::sensor`]).
//! - **Enemy bar**: the health of what the reticle is on, when it is an
//!   enemy.
//! - **Message tab**: fills when a transmission arrives; the objectives key
//!   grows a bar out of it with the current objective.
//! - **Weapons**: one icon per carried slot, green-bordered when selected,
//!   grey when out of ammo, with the other variant's mini icon when the key
//!   has two weapons; the ammo count under it.
//! - **Health**: the yellow bar and the percentage over it.
//! - **Forms**: off (not collected), on (collected), picked (current).
//! - **Score and clock** in the top strip's two slots.
//!
//! Not drawn yet, because the sim does not track them: the Nano-Canister
//! slots (`NANITE`; canisters heal on pickup rather than being held), the
//! lock-on marker and the threat arrows (`TARGET`).

use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use bft_assets::catalog::LevelCatalog;
use bft_assets::hud::{FontSpec, HudLayout, Placed, hud_layout, menu_pointer};
use bft_assets::messages::Messages;
use bft_assets::reader::ReaderData;
use bft_assets::textures::TextureSet;
use bft_sim::damage::{AimResult, Health};
use bft_sim::mission::{MissionClock, MissionScript, MissionState, ObjectiveState, Score};
use bft_sim::vehicle::{PlayerVehicle, Vehicle, VehicleForm};
use bft_sim::weapons::WeaponLoadout;
use bft_sim::weapons::firing::health_owner;

use super::layout::{Anchor, HudFrame, StripArt, strip_pieces};
use super::{
    HudClock, HudEnemyHealth, HudForms, HudHealth, HudMessages, HudObjectiveTab, HudRoot, HudScore,
    HudSensor, HudStyle, HudWeapon, MESSAGE_LINES, MessageLine, ObjectivesOpen,
};
use crate::globals::{self, Globals};
use crate::ui::cursor::ReticleState;
use crate::ui::settings::Settings;
use crate::ui::theme::{UiFonts, message_text};

/// The art the original HUD draws with, and its layout.
#[derive(Debug, Clone)]
pub struct OriginalHud {
    pub layout: HudLayout,
    /// Every texture the layout names, as images of our own (see
    /// [`copy_art`]) with their size in texels.
    pub art: HashMap<String, HudArt>,
    /// The menus' mouse pointer (`dialog.zrd`'s `CURSOR`), when the install
    /// has it. Not part of the HUD, but it comes from the same archive.
    pub pointer: Option<HudArt>,
}

/// One HUD image.
#[derive(Debug, Clone)]
pub struct HudArt {
    pub image: Handle<Image>,
    pub size: Vec2,
}

impl OriginalHud {
    /// The canvas the layout is drawn on: `TYPEI`, the whole screen.
    pub fn canvas(&self) -> Vec2 {
        self.layout.viewport_full.as_rect().size()
    }

    fn size(&self, texture: &str) -> Vec2 {
        self.art.get(texture).map_or(Vec2::ZERO, |art| art.size)
    }

    fn image(&self, texture: &str) -> Handle<Image> {
        self.art
            .get(texture)
            .map(|art| art.image.clone())
            .unwrap_or_default()
    }

    /// A placed image's rectangle on the canvas, or in its parent's frame.
    fn rect(&self, placed: &Placed) -> Rect {
        let size = self.size(&placed.texture);
        let pos = placed.pos.as_vec2();
        let min = if placed.centred {
            pos - size / 2.0
        } else {
            pos
        };
        Rect::from_corners(min, min + size)
    }
}

/// Which texture to draw, by name, for every texture the layout names.
///
/// `Err` names the first missing one, and the caller falls back.
pub fn assemble(
    layout: HudLayout,
    mut lookup: impl FnMut(&str) -> Option<HudArt>,
) -> Result<OriginalHud, String> {
    let mut art = HashMap::new();
    for name in layout.textures() {
        let found = lookup(name).ok_or_else(|| format!("image.zbd has no {name}"))?;
        art.insert(name.to_string(), found);
    }
    Ok(OriginalHud {
        layout,
        art,
        pointer: None,
    })
}

/// The loading state behind [`HudStyle::Pending`].
#[derive(Resource, Debug, Default)]
pub struct HudArtLoad {
    set: Option<Handle<TextureSet>>,
}

/// Settle [`HudStyle`] once the global reader and `image.zbd` have loaded.
// Eight parameters: the two sources, the stores they land in, and where to
// write the answer.
#[allow(clippy::too_many_arguments)]
pub fn resolve_style(
    mut style: ResMut<HudStyle>,
    mut load: ResMut<HudArtLoad>,
    globals: Option<Res<Globals>>,
    catalog: Option<Res<LevelCatalog>>,
    server: Res<AssetServer>,
    readers: Res<Assets<ReaderData>>,
    sets: Res<Assets<TextureSet>>,
    mut images: ResMut<Assets<Image>>,
) {
    if !matches!(*style, HudStyle::Pending) {
        return;
    }
    let (Some(globals), Some(catalog)) = (globals, catalog) else {
        return;
    };

    let fallback = |style: &mut HudStyle, why: &str| {
        warn!("the original HUD is unavailable ({why}); using the simple one");
        *style = HudStyle::Fallback;
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

    let layout = match hud_layout(reader) {
        Ok(layout) => layout,
        Err(e) => return fallback(&mut style, &e.to_string()),
    };
    let mut copy = |name: &str| {
        let source = set.image(name)?;
        let image = images.get(source)?.clone();
        let size = image.size().as_vec2();
        Some(HudArt {
            image: images.add(copy_art(image, ImageFilterMode::Linear)),
            size,
        })
    };
    let result = assemble(layout, &mut copy);
    match result {
        Ok(mut hud) => {
            hud.pointer = menu_pointer(reader).and_then(|name| copy(&name));
            info!(
                "original HUD: hud.zrd version {}, {} image(s)",
                hud.layout.version,
                hud.art.len()
            );
            *style = HudStyle::Original(Box::new(hud));
        }
        Err(why) => fallback(&mut style, &why),
    }
}

/// `image.zbd`'s archive, which the catalog puts in every level's texture
/// pool: the HUD's and the menus' art.
pub fn image_archive(catalog: &LevelCatalog) -> Option<bevy::asset::AssetPath<'static>> {
    catalog
        .texture_pool
        .iter()
        .find(|path| {
            path.path()
                .file_name()
                .is_some_and(|name| name == "image.tex.zip")
        })
        .cloned()
}

/// A HUD image of our own, cut loose from the level textures' sampler.
///
/// `image.zbd`'s images are shared with the level, which samples them with
/// `Repeat` and the user's texture filter. A HUD piece drawn from a
/// sub-rectangle must clamp — `Repeat` bleeds the opposite edge into a
/// linear-filtered border — and its filter follows the HUD's scale instead
/// ([`update_filter`]), so each gets a copy with its own sampler.
pub fn copy_art(mut image: Image, filter: ImageFilterMode) -> Image {
    image.sampler = hud_sampler(filter);
    image
}

pub(crate) fn hud_sampler(filter: ImageFilterMode) -> ImageSampler {
    ImageSampler::Descriptor(ImageSamplerDescriptor {
        label: None,
        address_mode_u: ImageAddressMode::ClampToEdge,
        address_mode_v: ImageAddressMode::ClampToEdge,
        address_mode_w: ImageAddressMode::ClampToEdge,
        mag_filter: filter,
        min_filter: filter,
        mipmap_filter: filter,
        ..default()
    })
}

// ---------------------------------------------------------------------------
// Layout components
// ---------------------------------------------------------------------------

/// The screen the HUD is laid out for right now.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct CurrentHudFrame(pub HudFrame);

/// A node placed on screen from a canvas rectangle.
#[derive(Component, Debug, Clone, Copy)]
pub(super) struct Anchored {
    rect: Rect,
    /// `None` to take the anchor from where `rect` sits.
    anchor: Option<Anchor>,
}

/// A node placed inside its parent, in canvas pixels from the parent's
/// top-left.
#[derive(Component, Debug, Clone, Copy)]
pub(super) struct InFrame(Rect);

/// Text whose size is a `FONTS` height in 480-line pixels.
#[derive(Component, Debug, Clone, Copy)]
pub(super) struct ScaledText(f32);

/// The container the strips are rebuilt into.
#[derive(Component, Debug, Clone, Copy)]
pub(super) struct StripLayer;

#[derive(Component, Debug, Clone, Copy)]
pub(super) struct EnemyFill;

#[derive(Component, Debug, Clone, Copy)]
pub(super) struct TabFill;

#[derive(Component, Debug, Clone, Copy)]
pub(super) struct ObjectivePanel;

#[derive(Component, Debug, Clone, Copy)]
pub(super) struct ObjectiveSummary;

#[derive(Component, Debug, Clone, Copy)]
pub(super) struct ObjectiveText;

#[derive(Component, Debug, Clone, Copy)]
pub(super) struct WeaponIcon(u8);

#[derive(Component, Debug, Clone, Copy)]
pub(super) struct WeaponMini(u8);

#[derive(Component, Debug, Clone, Copy)]
pub(super) struct WeaponAmmo(u8);

#[derive(Component, Debug, Clone, Copy)]
pub(super) struct ShieldFill;

#[derive(Component, Debug, Clone, Copy)]
pub(super) struct ShieldValue;

#[derive(Component, Debug, Clone, Copy)]
pub(super) struct FormIcon(usize);

/// The HUD's colours for what the layout draws as fills and text. The file
/// gives none but the objective bar's; these are the colours the manual and
/// the original's screenshots show (green digits, a yellow health bar, a
/// green enemy bar, a blue tab).
const DIGITS: Color = Color::srgb(0.25, 0.95, 0.35);
const SHIELD_YELLOW: Color = Color::srgb(0.95, 0.85, 0.15);
const ENEMY_GREEN: Color = Color::srgb(0.2, 0.85, 0.3);
const TAB_BLUE: Color = Color::srgb(0.2, 0.45, 1.0);
const MESSAGE_COLOUR: Color = Color::srgb(0.85, 0.95, 0.85);

/// The forms in `MODES` row order: tank, amphib, hover, sub (the textures'
/// own names say which is which).
const MODE_FORMS: [VehicleForm; 4] = [
    VehicleForm::Track,
    VehicleForm::Amphib,
    VehicleForm::Hover,
    VehicleForm::Sub,
];

// ---------------------------------------------------------------------------
// Spawning
// ---------------------------------------------------------------------------

/// Build the original HUD under a new [`HudRoot`], and return the root.
pub fn spawn(
    commands: &mut Commands,
    hud: &OriginalHud,
    fonts: &UiFonts,
    sensor: Handle<Image>,
) -> Entity {
    let layout = &hud.layout;
    let image = |texture: &str| ImageNode {
        image: hud.image(texture),
        image_mode: NodeImageMode::Stretch,
        ..default()
    };

    let mut root = commands.spawn((
        HudRoot,
        Name::new("hud (original)"),
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            ..default()
        },
        Pickable::IGNORE,
        DespawnOnExit(bft_sim::states::AppState::InGame),
    ));
    root.with_children(|root| {
        // The strips first, so everything else draws over them.
        root.spawn((StripLayer, full_screen(), Pickable::IGNORE));

        // Messages along the top, under the strip: "information about
        // pickups and messages from other players appear at the top of
        // the screen" (manual). The file places no message line; the
        // position is ours.
        let messages = &layout.fonts.messages;
        let top = layout
            .strips
            .iter()
            .find(|strip| strip.pos.y == 0)
            .map_or(0.0, |strip| hud.size(&strip.texture).y);
        let line = messages.height as f32;
        root.spawn((
            HudMessages,
            Anchored {
                rect: Rect::new(8.0, top + 4.0, 8.0, top + 4.0 + line * MESSAGE_LINES as f32),
                anchor: Some(Anchor {
                    column: super::layout::Column::Left,
                    row: super::layout::Row::Top,
                }),
            },
            Node {
                position_type: PositionType::Absolute,
                flex_direction: FlexDirection::Column,
                ..default()
            },
            Pickable::IGNORE,
        ))
        .with_children(|log| {
            for index in 0..MESSAGE_LINES {
                log.spawn((
                    MessageLine(index),
                    text("", messages, fonts, MESSAGE_COLOUR),
                    Node::default(),
                ));
            }
        });

        // The sensor window with the enemy bar and the message tab.
        let sensor_frame = hud.rect(&layout.sensor.frame);
        root.spawn((
            HudSensor,
            Anchored {
                rect: sensor_frame,
                anchor: None,
            },
            absolute(),
            Pickable::IGNORE,
        ))
        .with_children(|frame| {
            frame.spawn((
                InFrame(layout.sensor.view.as_rect()),
                absolute(),
                ImageNode {
                    image: sensor,
                    image_mode: NodeImageMode::Stretch,
                    ..default()
                },
                BackgroundColor(Color::BLACK),
                Pickable::IGNORE,
            ));
            frame.spawn((
                InFrame(Rect::from_corners(Vec2::ZERO, sensor_frame.size())),
                absolute(),
                image(&layout.sensor.frame.texture),
                Pickable::IGNORE,
            ));
            frame.spawn((
                InFrame(hud.rect(&layout.sensor.reticle)),
                absolute(),
                image(&layout.sensor.reticle.texture),
                Pickable::IGNORE,
            ));
            frame
                .spawn((
                    HudEnemyHealth,
                    InFrame(layout.sensor.enemy_bar.as_rect()),
                    absolute(),
                    Pickable::IGNORE,
                ))
                .with_children(|bar| {
                    bar.spawn((EnemyFill, bottom_fill(), BackgroundColor(ENEMY_GREEN)));
                });

            // The tab, and the bar that grows out of its right edge.
            let objective = &layout.objective;
            let tab = hud.rect(&objective.tab);
            frame
                .spawn((
                    HudObjectiveTab,
                    InFrame(tab),
                    absolute(),
                    image(&objective.tab.texture),
                    Pickable::IGNORE,
                ))
                .with_children(|tab_node| {
                    tab_node
                        .spawn((
                            InFrame(objective.tab_fill.as_rect()),
                            absolute(),
                            Pickable::IGNORE,
                        ))
                        .with_children(|slot| {
                            slot.spawn((TabFill, bottom_fill(), BackgroundColor(TAB_BLUE)));
                        });
                });
            let panel = objective.panel.as_rect();
            let [r, g, b] = objective.colour;
            frame
                .spawn((
                    ObjectivePanel,
                    InFrame(Rect::from_corners(
                        Vec2::new(tab.max.x, tab.min.y) + panel.min,
                        Vec2::new(tab.max.x, tab.min.y) + panel.max,
                    )),
                    Node {
                        position_type: PositionType::Absolute,
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    BackgroundColor(Color::srgb_u8(r, g, b)),
                    Visibility::Hidden,
                    Pickable::IGNORE,
                ))
                .with_children(|bar| {
                    let wrap = objective.text_box.y as f32;
                    let at = |pos: UVec2, height: u32| {
                        Rect::from_corners(
                            pos.as_vec2(),
                            pos.as_vec2() + Vec2::new(wrap, height as f32),
                        )
                    };
                    bar.spawn((
                        ObjectiveSummary,
                        InFrame(at(objective.summary_at, layout.fonts.obj_summary.height)),
                        absolute(),
                        text("", &layout.fonts.obj_summary, fonts, DIGITS),
                    ));
                    bar.spawn((
                        ObjectiveText,
                        InFrame(Rect::from_corners(
                            objective.description_at.as_vec2(),
                            Vec2::new(objective.description_at.x as f32 + wrap, panel.height()),
                        )),
                        absolute(),
                        text("", &layout.fonts.obj_description, fonts, MESSAGE_COLOUR),
                    ));
                });
        });

        // The weapon icons, placed on the icon strip whether or not the
        // strip itself is drawn.
        if let Some(icon_strip) = icon_strip(hud) {
            let strip_rect = hud.rect(icon_strip);
            root.spawn((
                HudWeapon,
                Anchored {
                    rect: strip_rect,
                    anchor: None,
                },
                absolute(),
                Pickable::IGNORE,
            ))
            .with_children(|strip| {
                for (index, row) in layout.weapons.iter().enumerate() {
                    let slot = index as u8 + 1;
                    let icon_size = hud.size(&row.frames[0]);
                    let icon =
                        Rect::from_corners(row.icon.as_vec2(), row.icon.as_vec2() + icon_size);
                    strip.spawn((
                        WeaponIcon(slot),
                        InFrame(icon),
                        absolute(),
                        image(&row.frames[0]),
                        Visibility::Hidden,
                        Pickable::IGNORE,
                    ));
                    // "A small icon … near the lower-right corner of the
                    // larger icon" (manual).
                    let mini = hud.size(&row.minis[0]);
                    strip.spawn((
                        WeaponMini(slot),
                        InFrame(Rect::from_corners(icon.max - mini, icon.max)),
                        absolute(),
                        image(&row.minis[0]),
                        Visibility::Hidden,
                        Pickable::IGNORE,
                    ));
                    let ammo = &layout.fonts.ammo;
                    strip.spawn((
                        WeaponAmmo(slot),
                        InFrame(Rect::from_corners(
                            row.ammo.as_vec2(),
                            row.ammo.as_vec2() + Vec2::new(icon_size.x, ammo.height as f32),
                        )),
                        absolute(),
                        text("", ammo, fonts, DIGITS),
                    ));
                }
            });
        }

        // Health: the frame, the bar, the percentage.
        let shield = &layout.shield;
        let shield_frame = hud.rect(&shield.frame);
        root.spawn((
            HudHealth,
            Anchored {
                rect: shield_frame,
                anchor: None,
            },
            absolute(),
            Pickable::IGNORE,
        ))
        .with_children(|frame| {
            frame.spawn((
                InFrame(Rect::from_corners(Vec2::ZERO, shield_frame.size())),
                absolute(),
                image(&shield.frame.texture),
                Pickable::IGNORE,
            ));
            frame
                .spawn((InFrame(shield.fill.as_rect()), absolute(), Pickable::IGNORE))
                .with_children(|bar| {
                    bar.spawn((ShieldFill, bottom_fill(), BackgroundColor(SHIELD_YELLOW)));
                });
            let strings = &layout.fonts.strings;
            let readout = shield.readout;
            let width = if readout.centred {
                shield_frame.width()
            } else {
                (readout.digits * strings.width) as f32
            };
            let mut value = text("", strings, fonts, DIGITS);
            if readout.centred {
                value.3 = TextLayout::justify(Justify::Center);
            }
            frame.spawn((
                ShieldValue,
                InFrame(Rect::from_corners(
                    readout.pos.as_vec2(),
                    readout.pos.as_vec2() + Vec2::new(width, strings.height as f32),
                )),
                absolute(),
                value,
            ));
        });

        // The forms, as one cluster.
        let bounds = layout.modes.iter().fold(None::<Rect>, |bounds, mode| {
            let rect =
                Rect::from_corners(mode.pos.as_vec2(), mode.pos.as_vec2() + hud.size(&mode.off));
            Some(bounds.map_or(rect, |bounds| bounds.union(rect)))
        });
        if let Some(bounds) = bounds {
            root.spawn((
                HudForms,
                Anchored {
                    rect: bounds,
                    anchor: None,
                },
                absolute(),
                Pickable::IGNORE,
            ))
            .with_children(|forms| {
                for (index, mode) in layout.modes.iter().enumerate() {
                    forms.spawn((
                        FormIcon(index),
                        InFrame(Rect::from_corners(
                            mode.pos.as_vec2() - bounds.min,
                            mode.pos.as_vec2() - bounds.min + hud.size(&mode.off),
                        )),
                        absolute(),
                        image(&mode.off),
                        Pickable::IGNORE,
                    ));
                }
            });
        }

        // Score and clock. The score reads right to left from its
        // point, which leaves the slot's right half to the canisters
        // (`NANITE` starts where the score ends); the clock reads on
        // from its point. Both inferred from the art.
        let strings = &layout.fonts.strings;
        let height = strings.height as f32;
        let score_width = (strings.width * 8) as f32;
        let score = layout.stats.score.as_vec2();
        let score_rect = Rect::new(score.x - score_width, score.y, score.x, score.y + height);
        let mut score_text = text("0", strings, fonts, DIGITS);
        score_text.3 = TextLayout::justify(Justify::Right);
        // Anchored by its point, not by the box drawn left of it.
        let canvas = HudFrame::new(hud.canvas(), hud.canvas(), 1.0);
        root.spawn((
            HudScore,
            Anchored {
                rect: score_rect,
                anchor: Some(canvas.anchor_of(Rect::from_corners(score, score))),
            },
            absolute(),
            score_text,
        ));
        let clock = layout.stats.clock.as_vec2();
        root.spawn((
            HudClock,
            Anchored {
                rect: Rect::new(
                    clock.x,
                    clock.y,
                    clock.x + (strings.width * 8) as f32,
                    clock.y + height,
                ),
                anchor: None,
            },
            absolute(),
            text("", strings, fonts, DIGITS),
        ));
    });
    root.id()
}

/// The strip the weapon icons sit on: `WEAPON`'s positions are relative
/// to it. It is the one the original widened for wider screens without
/// spanning the whole canvas — the top strip spans it.
fn icon_strip(hud: &OriginalHud) -> Option<&Placed> {
    let canvas = hud.canvas();
    let partial = |strip: &&Placed| hud.size(&strip.texture).x < canvas.x;
    hud.layout
        .strips
        .iter()
        .filter(partial)
        .find(|strip| {
            let size = hud.size(&strip.texture);
            hud.layout.wide_strips.iter().any(|wide| {
                let wide = hud.size(wide);
                (wide.y - size.y).abs() < 0.5 && wide.x > size.x
            })
        })
        // A layout without wide art: the last strip that is not the top one.
        .or_else(|| hud.layout.strips.iter().rev().find(partial))
}

fn full_screen() -> Node {
    Node {
        position_type: PositionType::Absolute,
        width: percent(100),
        height: percent(100),
        ..default()
    }
}

fn absolute() -> Node {
    Node {
        position_type: PositionType::Absolute,
        ..default()
    }
}

/// A fill that grows up from the bottom of its parent.
fn bottom_fill() -> (Node, Pickable) {
    (
        Node {
            position_type: PositionType::Absolute,
            left: px(0),
            right: px(0),
            bottom: px(0),
            height: percent(0),
            ..default()
        },
        Pickable::IGNORE,
    )
}

/// A HUD label in a `FONTS` style. Eurostile stands in for the Windows
/// faces the file names (`Arial`, `Symbol`), which are not shipped; the
/// bold cut stands in for weights from 600.
fn text(
    value: &str,
    spec: &FontSpec,
    fonts: &UiFonts,
    colour: Color,
) -> (Text, TextFont, TextColor, TextLayout, ScaledText, Pickable) {
    let font = if spec.weight >= 600 {
        fonts.bold.clone().into()
    } else {
        default()
    };
    (
        Text::new(value),
        TextFont {
            font,
            ..TextFont::from_font_size(spec.height as f32)
        },
        TextColor(colour),
        TextLayout::default(),
        ScaledText(spec.height as f32),
        Pickable::IGNORE,
    )
}

// ---------------------------------------------------------------------------
// Layout systems
// ---------------------------------------------------------------------------

/// Track the window and the scale setting.
pub(super) fn update_frame(
    style: Res<HudStyle>,
    settings: Res<Settings>,
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    current: Option<Res<CurrentHudFrame>>,
    mut commands: Commands,
) {
    let (HudStyle::Original(hud), Some(window)) = (&*style, window) else {
        return;
    };
    let frame = HudFrame::new(
        Vec2::new(window.width(), window.height()),
        hud.canvas(),
        settings.hud.scale,
    );
    if current.is_none_or(|current| current.0 != frame) {
        commands.insert_resource(CurrentHudFrame(frame));
    }
}

/// Place every anchored and local node, and size the text, when the frame
/// changes or new nodes appear.
#[allow(clippy::type_complexity)]
pub(super) fn relayout(
    frame: Option<Res<CurrentHudFrame>>,
    mut anchored: Query<(Ref<Anchored>, &mut Node), Without<InFrame>>,
    mut locals: Query<(Ref<InFrame>, &mut Node), Without<Anchored>>,
    mut texts: Query<(Ref<ScaledText>, &mut TextFont)>,
) {
    let Some(frame) = frame else {
        return;
    };
    let changed = frame.is_changed();
    let frame = frame.0;
    let s = frame.scale;

    for (placed, mut node) in &mut anchored {
        if !changed && !placed.is_added() {
            continue;
        }
        let rect = match placed.anchor {
            Some(anchor) => frame.place_as(placed.rect, anchor),
            None => frame.place(placed.rect),
        };
        node.left = px(rect.min.x);
        node.top = px(rect.min.y);
        node.width = px(rect.width());
        node.height = px(rect.height());
    }
    for (local, mut node) in &mut locals {
        if !changed && !local.is_added() {
            continue;
        }
        node.left = px(local.0.min.x * s);
        node.top = px(local.0.min.y * s);
        node.width = px(local.0.width() * s);
        node.height = px(local.0.height() * s);
    }
    for (scaled, mut font) in &mut texts {
        if !changed && !scaled.is_added() {
            continue;
        }
        font.font_size = FontSize::Px(scaled.0 * s);
    }
}

/// Cut the strips for this screen: on a new frame, or when the strips are
/// switched on or off.
pub(super) fn rebuild_strips(
    mut commands: Commands,
    style: Res<HudStyle>,
    settings: Res<Settings>,
    frame: Option<Res<CurrentHudFrame>>,
    layers: Query<(Entity, Ref<StripLayer>)>,
    mut last: Local<Option<(HudFrame, bool)>>,
) {
    let (HudStyle::Original(hud), Some(frame)) = (&*style, frame) else {
        return;
    };
    let wanted = (frame.0, settings.hud.strips);
    let fresh = layers.iter().any(|(_, layer)| layer.is_added());
    if !fresh && *last == Some(wanted) {
        return;
    }
    *last = Some(wanted);

    let variants: Vec<StripArt> = hud
        .layout
        .wide_strips
        .iter()
        .map(|name| StripArt {
            texture: name.clone(),
            size: hud.size(name),
        })
        .collect();

    for (layer, _) in &layers {
        commands.entity(layer).despawn_related::<Children>();
        if !settings.hud.strips {
            continue;
        }
        commands.entity(layer).with_children(|layer| {
            for strip in &hud.layout.strips {
                let base = StripArt {
                    texture: strip.texture.clone(),
                    size: hud.size(&strip.texture),
                };
                for piece in strip_pieces(&frame.0, &base, strip.pos.as_vec2(), &variants) {
                    layer.spawn((
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(piece.screen.min.x),
                            top: px(piece.screen.min.y),
                            width: px(piece.screen.width()),
                            height: px(piece.screen.height()),
                            ..default()
                        },
                        ImageNode {
                            image: hud.image(&piece.texture),
                            rect: Some(piece.source),
                            image_mode: NodeImageMode::Stretch,
                            ..default()
                        },
                        Pickable::IGNORE,
                    ));
                }
            }
        });
    }
}

/// Nearest sampling at whole-number scales, linear otherwise: at 2× or 3×
/// the art's pixels stay crisp squares, and at 2.25× nearest would draw
/// some texel columns twice and some three times, which shimmers.
pub(super) fn update_filter(
    style: Res<HudStyle>,
    frame: Option<Res<CurrentHudFrame>>,
    mut images: ResMut<Assets<Image>>,
    mut last: Local<Option<bool>>,
) {
    let (HudStyle::Original(hud), Some(frame)) = (&*style, frame) else {
        return;
    };
    let nearest = frame.0.is_integer_scale();
    if *last == Some(nearest) {
        return;
    }
    *last = Some(nearest);
    let filter = if nearest {
        ImageFilterMode::Nearest
    } else {
        ImageFilterMode::Linear
    };
    for art in hud.art.values() {
        if let Some(mut image) = images.get_mut(&art.image) {
            image.sampler = hud_sampler(filter);
        }
    }
}

// ---------------------------------------------------------------------------
// Content systems
// ---------------------------------------------------------------------------

/// The reticle's look: `ret_off` with nothing in range, `ret_range` with
/// something in range, `ret_on` on something damageable.
pub fn reticle_texture<'a>(layout: &'a HudLayout, aim: &AimResult) -> &'a str {
    match ReticleState::of(aim) {
        ReticleState::Off => &layout.reticle.off,
        ReticleState::Range => &layout.reticle.range,
        ReticleState::On => &layout.reticle.on,
    }
}

/// The health of what the reticle is on, when it is an enemy; `None`
/// otherwise, which empties the bar.
pub fn enemy_fraction(
    aim: Option<&AimResult>,
    healths: &Query<&Health>,
    parents: &Query<&ChildOf>,
) -> Option<f32> {
    let aim = aim?;
    if !aim.damageable {
        return None;
    }
    let owner = health_owner(aim.hit?, |entity| healths.contains(entity), parents)?;
    healths.get(owner).ok().map(Health::fraction)
}

/// Fill the enemy bar.
pub(super) fn update_enemy_bar(
    players: Query<&AimResult, With<PlayerVehicle>>,
    healths: Query<&Health>,
    parents: Query<&ChildOf>,
    mut fills: Query<&mut Node, With<EnemyFill>>,
) {
    let fraction = enemy_fraction(players.iter().next(), &healths, &parents).unwrap_or(0.0);
    for mut node in &mut fills {
        let height = percent(fraction.clamp(0.0, 1.0) * 100.0);
        if node.height != height {
            node.height = height;
        }
    }
}

/// The message tab: whether a transmission is waiting, and how far the bar
/// has slid out.
#[derive(Resource, Debug, Clone, Default, PartialEq)]
pub struct ObjectiveTab {
    /// A transmission has arrived and not been read: the tab's slot fills.
    pub pending: bool,
    /// 0 closed, 1 fully out.
    pub progress: f32,
    /// The latest transmission, `(summary, text)` as message keys, which
    /// the bar shows until the player has read it.
    pub latest: Option<(String, String)>,
}

impl ObjectiveTab {
    /// A transmission arrived.
    pub fn notify(&mut self, summary: String, text: String) {
        self.pending = true;
        self.latest = Some((summary, text));
    }

    /// Slide toward open or closed over `slide_time` seconds; opening reads
    /// the waiting transmission.
    pub fn step(&mut self, open: bool, dt: f32, slide_time: f32) {
        if open {
            self.pending = false;
        }
        let target = if open { 1.0 } else { 0.0 };
        let rate = if slide_time > 0.0 {
            dt / slide_time
        } else {
            1.0
        };
        self.progress = if self.progress < target {
            (self.progress + rate).min(target)
        } else {
            (self.progress - rate).max(target)
        };
    }
}

/// The briefing is the first transmission: the tab starts full when a
/// mission starts with something to do. The HUD may be built before the
/// mission's script arrives, so this watches the script.
pub fn brief_on_mission_start(script: Option<Res<MissionScript>>, mut tab: ResMut<ObjectiveTab>) {
    let Some(script) = script else {
        return;
    };
    if script.is_added() {
        *tab = ObjectiveTab {
            pending: script.objectives.iter().any(|o| o.initially_active),
            ..default()
        };
    }
}

/// The current objective's `(summary, text)` keys: the first active one.
pub fn current_objective(script: &MissionScript, state: &MissionState) -> Option<(String, String)> {
    script
        .objectives
        .iter()
        .zip(&state.objectives)
        .find(|(_, progress)| **progress == ObjectiveState::Active)
        .map(|(objective, _)| (objective.summary_key.clone(), objective.text_key.clone()))
}

/// Slide the bar, fill the tab, and write the objective into the bar.
// Nine parameters: the tab's state, the key, the clock, the mission, the
// strings and the four nodes it moves.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn update_objective_tab(
    style: Res<HudStyle>,
    time: Res<Time<Real>>,
    open: Res<ObjectivesOpen>,
    mut tab: ResMut<ObjectiveTab>,
    script: Option<Res<MissionScript>>,
    state: Option<Res<MissionState>>,
    globals: Option<Res<Globals>>,
    tables: Res<Assets<Messages>>,
    mut fills: Query<&mut Node, (With<TabFill>, Without<ObjectivePanel>)>,
    mut panels: Query<(&InFrame, &mut Node, &mut Visibility), With<ObjectivePanel>>,
    mut summaries: Query<&mut Text, (With<ObjectiveSummary>, Without<ObjectiveText>)>,
    mut texts: Query<&mut Text, (With<ObjectiveText>, Without<ObjectiveSummary>)>,
    frame: Option<Res<CurrentHudFrame>>,
) {
    let (HudStyle::Original(hud), Some(frame)) = (&*style, frame) else {
        return;
    };
    // Stepped through a comparison so a closed, quiet tab is not marked
    // changed every frame.
    let mut next = tab.clone();
    next.step(open.0, time.delta_secs(), hud.layout.objective.slide_time);
    if *tab != next {
        *tab = next;
    }

    for mut node in &mut fills {
        let height = percent(if tab.pending { 100.0 } else { 0.0 });
        if node.height != height {
            node.height = height;
        }
    }

    for (local, mut node, mut visibility) in &mut panels {
        let width = local.0.width() * tab.progress * frame.0.scale;
        node.width = px(width);
        visibility.set_if_neq(if tab.progress > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }

    if tab.progress > 0.0 {
        let keys = tab.latest.clone().or_else(|| {
            script
                .as_deref()
                .zip(state.as_deref())
                .and_then(|(script, state)| current_objective(script, state))
        });
        let messages = globals::messages(globals.as_deref(), &tables);
        let (summary, text) = keys
            .map(|(summary, text)| {
                (
                    message_text(messages, &summary),
                    message_text(messages, &text),
                )
            })
            .unwrap_or_default();
        for mut line in &mut summaries {
            line.set_if_neq(Text::new(summary.clone()));
        }
        for mut line in &mut texts {
            line.set_if_neq(Text::new(text.clone()));
        }
    }
    // Closing forgets the transmission: the bar then shows the current
    // objective, which is what "Open the Current Mission Objective Message"
    // means.
    if !open.0 && tab.progress == 0.0 && !tab.pending {
        tab.latest = None;
    }
}

/// The ammo count of a weapon that never runs out. The original set `AMMO`
/// in the Symbol face, which is where its ∞ came from; Eurostile, which
/// stands in for it, has the glyph too.
pub const UNLIMITED: &str = "\u{221E}";

/// What one weapon slot shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotView {
    /// The icon texture; `None` hides the slot (nothing carried).
    pub icon: Option<String>,
    /// The other variant's mini icon, when the key has two weapons.
    pub mini: Option<String>,
    /// The ammo count; [`UNLIMITED`] for the pulse gun.
    pub ammo: String,
}

/// What slot `slot` (1–9) shows for a loadout.
pub fn slot_view(layout: &HudLayout, loadout: &WeaponLoadout, slot: u8) -> SlotView {
    let hidden = SlotView {
        icon: None,
        mini: None,
        ammo: String::new(),
    };
    let (Some(row), Some(state)) = (
        layout.weapons.get(usize::from(slot).saturating_sub(1)),
        loadout.slot(slot),
    ) else {
        return hidden;
    };
    let (weak, strong) = (state.is_mounted(false), state.is_mounted(true));
    if !weak && !strong {
        return hidden;
    }

    let selected = loadout.selected == slot;
    let shown_strong = if selected { loadout.strong } else { !weak };
    let ammo = state.variant(shown_strong);
    let empty = ammo.is_some_and(|ammo| ammo.ammo == 0);
    let icon = if empty {
        row.empty_frame()
    } else {
        row.frame(shown_strong, selected)
    };
    let other = if shown_strong { weak } else { strong };

    SlotView {
        icon: Some(icon.to_string()),
        mini: other.then(|| row.mini(!shown_strong).to_string()),
        ammo: ammo
            .map(|ammo| {
                if ammo.ammo == u32::MAX {
                    UNLIMITED.to_string()
                } else {
                    ammo.ammo.to_string()
                }
            })
            .unwrap_or_default(),
    }
}

/// The weapon row.
#[allow(clippy::type_complexity)]
pub(super) fn update_weapons(
    style: Res<HudStyle>,
    players: Query<&WeaponLoadout, (With<PlayerVehicle>, Changed<WeaponLoadout>)>,
    mut icons: Query<(&WeaponIcon, &mut ImageNode, &mut Visibility), Without<WeaponMini>>,
    mut minis: Query<(&WeaponMini, &mut ImageNode, &mut Visibility), Without<WeaponIcon>>,
    mut ammo: Query<(&WeaponAmmo, &mut Text)>,
    added: Query<(), Added<WeaponIcon>>,
    all_players: Query<&WeaponLoadout, With<PlayerVehicle>>,
) {
    let HudStyle::Original(hud) = &*style else {
        return;
    };
    // On a change, or when the row was just built.
    let loadout = players.iter().next().or_else(|| {
        (!added.is_empty())
            .then(|| all_players.iter().next())
            .flatten()
    });
    let Some(loadout) = loadout else {
        return;
    };
    let views: Vec<SlotView> = (1..=9)
        .map(|slot| slot_view(&hud.layout, loadout, slot))
        .collect();
    let view = |slot: u8| &views[usize::from(slot) - 1];

    for (icon, mut image, mut visibility) in &mut icons {
        match &view(icon.0).icon {
            Some(texture) => {
                image.image = hud.image(texture);
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    for (mini, mut image, mut visibility) in &mut minis {
        match &view(mini.0).mini {
            Some(texture) => {
                image.image = hud.image(texture);
                visibility.set_if_neq(Visibility::Inherited);
            }
            None => {
                visibility.set_if_neq(Visibility::Hidden);
            }
        }
    }
    for (slot, mut text) in &mut ammo {
        text.set_if_neq(Text::new(view(slot.0).ammo.clone()));
    }
}

/// The yellow bar and the percentage.
pub(super) fn update_health(
    players: Query<&Health, With<PlayerVehicle>>,
    mut fills: Query<&mut Node, With<ShieldFill>>,
    mut values: Query<&mut Text, With<ShieldValue>>,
) {
    let Some(health) = players.iter().next() else {
        return;
    };
    let fraction = health.fraction().clamp(0.0, 1.0);
    for mut node in &mut fills {
        let height = percent(fraction * 100.0);
        if node.height != height {
            node.height = height;
        }
    }
    // "The number is a percentage of the BFT's maximum health" (manual).
    let value = format!("{:.0}", (fraction * 100.0).ceil());
    for mut text in &mut values {
        text.set_if_neq(Text::new(value.clone()));
    }
}

/// The four form lights.
pub(super) fn update_forms(
    style: Res<HudStyle>,
    players: Query<&Vehicle, With<PlayerVehicle>>,
    mut icons: Query<(&FormIcon, &mut ImageNode)>,
) {
    let (HudStyle::Original(hud), Some(vehicle)) = (&*style, players.iter().next()) else {
        return;
    };
    for (icon, mut image) in &mut icons {
        let (Some(mode), Some(form)) = (hud.layout.modes.get(icon.0), MODE_FORMS.get(icon.0))
        else {
            continue;
        };
        let texture = if vehicle.form == *form {
            &mode.picked
        } else if vehicle.unlocked.has(*form) {
            &mode.on
        } else {
            &mode.off
        };
        let wanted = hud.image(texture);
        if image.image != wanted {
            image.image = wanted;
        }
    }
}

/// `hh:mm:ss`.
pub fn clock_text(seconds: f32) -> String {
    let total = seconds.max(0.0) as u64;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total / 60) % 60,
        total % 60
    )
}

/// The score and the clock.
#[allow(clippy::type_complexity)]
pub(super) fn update_stats(
    settings: Res<Settings>,
    score: Option<Res<Score>>,
    clock: Option<Res<MissionClock>>,
    mut scores: Query<&mut Text, (With<HudScore>, Without<HudClock>)>,
    mut clocks: Query<(&mut Text, &mut Visibility), (With<HudClock>, Without<HudScore>)>,
) {
    let points = score.map_or(0, |score| score.0);
    for mut text in &mut scores {
        text.set_if_neq(Text::new(points.to_string()));
    }
    let time = clock_text(clock.map_or(0.0, |clock| clock.0));
    for (mut text, mut visibility) in &mut clocks {
        text.set_if_neq(Text::new(time.clone()));
        visibility.set_if_neq(if settings.hud.clock {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use bft_sim::weapons::{AmmoState, SlotState};

    /// A layout whose texture names are invented; only the frame order is
    /// the file's.
    pub(crate) fn layout() -> HudLayout {
        let value = serde_json::json!([[
            "VERSION",
            [8],
            "FONTS",
            [
                "OBJ_SUMMARY",
                ["F", 11, 8, 700, 1],
                "OBJ_DESCRIPTION",
                ["F", 9, 7, 400, 0],
                "MESSAGES",
                ["F", 13, 7, 600, 1],
                "AMMO",
                ["G", 11, 6, 500, 1],
                "STRINGS",
                ["F", 10, 6, 500, 0]
            ],
            "TYPEI",
            [[0, 0, 600, 450]],
            "TYPEII",
            [
                [0, 20, 600, 380],
                ["top", 0, 0, "FALSE", "FALSE"],
                ["icons", 130, 380, "FALSE", "FALSE"],
                ["top_w", "icons_w"]
            ],
            "RETICULE",
            ["rr", "ro", "rn"],
            "SENSOR",
            [
                ["mon", 0, 340, "TRUE", "FALSE"],
                3.0,
                [9, 7],
                [90, 90],
                300,
                ["tr", 50, 50, "TRUE", "TRUE"],
                [110, 60, 115, 100]
            ],
            "SHIELD",
            [
                ["hp", 500, 380, "FALSE", "FALSE"],
                [0, 6, 3, "TRUE"],
                [8, 20, 26, 70]
            ],
            "NANITE",
            [[300, 9, 340, 19]],
            "OBJECTIVE",
            [
                0.4,
                ["tab", 110, 0, "FALSE", "FALSE"],
                [0, 0, 300, 50],
                [1, 2, 3],
                [2, 2],
                [2, 15],
                [4, 5, 9, 45],
                [2, 300]
            ],
            "STATS",
            [[300, 4], [500, 4]],
            "TARGET",
            ["l", "a", "b", "c", "d"],
            "WEAPON",
            (0..9)
                .map(|i| serde_json::json!([
                    format!("s{i}a"),
                    format!("s{i}b"),
                    format!("s{i}x"),
                    format!("s{i}A"),
                    format!("s{i}B"),
                    format!("m{i}a"),
                    format!("m{i}b"),
                    i * 30,
                    30,
                    i * 30,
                    60
                ]))
                .collect::<Vec<_>>(),
            "MODES",
            [
                ["t0", "t1", "t2", 520, 380],
                ["a0", "a1", "a2", 560, 380],
                ["h0", "h1", "h2", 520, 415],
                ["s0", "s1", "s2", 560, 415]
            ]
        ]]);
        bft_assets::hud::parse_hud(&value).expect("the test layout parses")
    }

    fn ammo(weapon: usize, rounds: u32) -> Option<AmmoState> {
        Some(AmmoState {
            weapon,
            ammo: rounds,
            interval: 1.0,
            mounted: true,
        })
    }

    #[test]
    fn a_missing_texture_falls_back() {
        let layout = layout();
        let complete = assemble(layout.clone(), |_| {
            Some(HudArt {
                image: Handle::default(),
                size: Vec2::ONE,
            })
        });
        assert!(complete.is_ok());

        let missing = assemble(layout, |name| {
            (name != "tab").then(|| HudArt {
                image: Handle::default(),
                size: Vec2::ONE,
            })
        });
        assert_eq!(missing.err().as_deref(), Some("image.zbd has no tab"));
    }

    fn spawned(style: HudStyle, original: bool) -> World {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let mut settings = Settings::default();
        settings.hud.original = original;
        world.insert_resource(settings);
        world.insert_resource(style);
        world.insert_resource(UiFonts::default());
        world.init_resource::<ObjectiveTab>();
        world
            .run_system_once(super::super::spawn_hud)
            .expect("spawn_hud runs");
        world
    }

    fn count<C: Component>(world: &mut World) -> usize {
        world.query_filtered::<(), With<C>>().iter(world).count()
    }

    /// Without the art, or with it turned off, the fallback HUD is
    /// what gets built; with it, the original.
    #[test]
    fn the_fallback_hud_is_built_when_the_original_is_unavailable_or_off() {
        let complete = || {
            assemble(layout(), |_| {
                Some(HudArt {
                    image: Handle::default(),
                    size: Vec2::splat(10.0),
                })
            })
            .expect("every texture is there")
        };

        let mut fallback = spawned(HudStyle::Fallback, true);
        assert_eq!(count::<HudRoot>(&mut fallback), 1);
        assert_eq!(count::<HudSensor>(&mut fallback), 0);
        assert_eq!(count::<super::super::HudHealth>(&mut fallback), 1);

        let mut turned_off = spawned(HudStyle::Original(Box::new(complete())), false);
        assert_eq!(count::<HudSensor>(&mut turned_off), 0);
        assert_eq!(count::<HudRoot>(&mut turned_off), 1);

        let mut original = spawned(HudStyle::Original(Box::new(complete())), true);
        assert_eq!(count::<HudRoot>(&mut original), 1);
        assert_eq!(count::<HudSensor>(&mut original), 1);
        assert_eq!(count::<WeaponIcon>(&mut original), 9);
        assert_eq!(count::<FormIcon>(&mut original), 4);

        // Still loading: nothing yet, rather than the wrong HUD.
        let mut pending = spawned(HudStyle::Pending, true);
        assert_eq!(count::<HudRoot>(&mut pending), 0);
    }

    /// The bar bills the entity that owns the health, found up the
    /// hierarchy from the collider the reticle is on, and only for an enemy.
    #[test]
    fn the_enemy_bar_shows_the_targets_health() {
        use bevy::ecs::system::RunSystemOnce;
        let mut world = World::new();
        let mut health = Health::new(80.0);
        health.current = 20.0;
        let enemy = world.spawn(health).id();
        let part = world.spawn(ChildOf(enemy)).id();

        let fraction = |world: &mut World, aim: AimResult| {
            world
                .run_system_once(move |healths: Query<&Health>, parents: Query<&ChildOf>| {
                    enemy_fraction(Some(&aim), &healths, &parents)
                })
                .expect("runs")
        };
        let on = AimResult {
            hit: Some(part),
            point: Vec3::ONE,
            damageable: true,
        };
        assert_eq!(fraction(&mut world, on), Some(0.25));
        assert_eq!(
            fraction(
                &mut world,
                AimResult {
                    damageable: false,
                    ..on
                }
            ),
            None,
            "not an enemy: the bar is empty"
        );
        assert_eq!(fraction(&mut world, AimResult::default()), None);
    }

    #[test]
    fn the_reticle_says_nothing_in_range_in_range_or_damageable() {
        let layout = layout();
        let mut aim = AimResult::default();
        assert_eq!(reticle_texture(&layout, &aim), "ro");

        aim.hit = Some(Entity::from_raw_u32(7).expect("a valid index"));
        assert_eq!(reticle_texture(&layout, &aim), "rr");

        aim.damageable = true;
        assert_eq!(reticle_texture(&layout, &aim), "rn");
    }

    #[test]
    fn weapon_slots_show_what_is_carried_selected_and_empty() {
        let layout = layout();
        let mut loadout = WeaponLoadout::default();
        loadout.slots[0] = SlotState {
            weak: ammo(0, u32::MAX),
            strong: ammo(1, 40),
        };
        loadout.slots[1] = SlotState {
            weak: ammo(2, 0),
            strong: None,
        };
        loadout.slots[2] = SlotState {
            weak: None,
            strong: ammo(5, 7),
        };

        // Slot 1, selected on its weak variant, with a strong one behind the
        // same key.
        let one = slot_view(&layout, &loadout, 1);
        assert_eq!(one.icon.as_deref(), Some("s0A"));
        assert_eq!(one.mini.as_deref(), Some("m0b"));
        assert_eq!(one.ammo, UNLIMITED);

        // Slot 2: carried, out of ammo.
        let two = slot_view(&layout, &loadout, 2);
        assert_eq!(two.icon.as_deref(), Some("s1x"));
        assert_eq!(two.ammo, "0");
        assert!(two.mini.is_none());

        // Slot 3: only the strong variant, not selected.
        let three = slot_view(&layout, &loadout, 3);
        assert_eq!(three.icon.as_deref(), Some("s2b"));
        assert_eq!(three.ammo, "7");

        // Slot 4: nothing.
        assert!(slot_view(&layout, &loadout, 4).icon.is_none());

        loadout.selected = 1;
        loadout.strong = true;
        let strong = slot_view(&layout, &loadout, 1);
        assert_eq!(strong.icon.as_deref(), Some("s0B"));
        assert_eq!(strong.mini.as_deref(), Some("m0a"));
        assert_eq!(strong.ammo, "40");
    }

    #[test]
    fn the_tab_slides_over_its_slide_time_and_opening_reads_it() {
        let mut tab = ObjectiveTab::default();
        tab.notify("MSG_S".into(), "MSG_T".into());
        assert!(tab.pending);

        tab.step(false, 0.1, 0.4);
        assert_eq!(tab.progress, 0.0);
        assert!(tab.pending, "closed, it stays unread");

        tab.step(true, 0.1, 0.4);
        assert!(!tab.pending);
        assert!((tab.progress - 0.25).abs() < 1e-6);
        for _ in 0..3 {
            tab.step(true, 0.1, 0.4);
        }
        assert!((tab.progress - 1.0).abs() < 1e-6);
        tab.step(true, 0.1, 0.4);
        assert_eq!(tab.progress, 1.0, "it stops fully out");

        tab.step(false, 0.3, 0.4);
        assert!((tab.progress - 0.25).abs() < 1e-6);
        tab.step(false, 0.3, 0.4);
        assert_eq!(tab.progress, 0.0);
    }

    #[test]
    fn the_clock_reads_hours_minutes_seconds() {
        assert_eq!(clock_text(0.0), "00:00:00");
        assert_eq!(clock_text(61.9), "00:01:01");
        assert_eq!(clock_text(3.0 * 3600.0 + 25.0 * 60.0 + 7.0), "03:25:07");
    }
}
