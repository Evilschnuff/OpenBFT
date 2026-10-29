//! The one renderer every original dialog goes through.
//!
//! [`DialogPainter::open`] spawns a full-window root and the 640×480
//! canvas on it; the painter then draws the dialog's backgrounds and text,
//! and its controls as the screen wires them ([`Wiring`]). Every node on
//! the canvas carries its position in canvas pixels ([`OnCanvas`]) and is
//! placed by [`relayout`] whenever the window changes, so a screen never
//! thinks in screen pixels.
//!
//! # Controls
//!
//! A control's look follows its state: its `BITMAP` idle, its `ROLLOVER`
//! bitmap under the pointer, its `ACTIVATE` bitmap pressed or chosen (a
//! radio button's `_S`), its `DISABLE` bitmap when the screen says it
//! cannot be used. Checkboxes and cycles have no idle bitmap — the box is
//! painted into the background — and show only on hover. Moving onto a
//! control plays its rollover sound, pressing it its select sound.
//!
//! A control the screen gives an action to is a button in the simple menus'
//! sense ([`UiButton`]); one it gives none is drawn and inert.

use bevy::image::ImageFilterMode;
use bevy::picking::hover::{HoverMap, PickingInteraction};
use bevy::prelude::*;
use bevy::text::{EditableText, TextCursorStyle};
use bevy::window::PrimaryWindow;
use bft_assets::dialog::{Control, ControlKind, Dialog, DialogFont, SoundCue};
use bft_assets::messages::Messages;

use super::frame::DialogFrame;
use super::{MenuStyle, OriginalMenus};
use crate::audio::PlaySound;
use crate::ui::hud::original::hud_sampler;
use crate::ui::settings::Settings;
use crate::ui::settings_screen::{SettingControl, SettingKey};
use crate::ui::theme::{
    ArtButton, Disabled, Selected, UiAction, UiButton, UiClick, UiFonts, message_text,
};

/// The frame the dialogs are laid out in now.
#[derive(Resource, Debug, Clone, Copy, PartialEq)]
pub struct CurrentDialogFrame(pub DialogFrame);

/// A dialog's full-window root: the backdrop, and everything else under it.
#[derive(Component, Debug, Clone)]
pub struct DialogRoot {
    /// The dialog's name in `dialog.zrd`, or `CAMPAIGN<n>`.
    pub name: String,
}

/// The 640×480 canvas a dialog is drawn on.
#[derive(Component, Debug, Clone, Copy)]
pub struct DialogCanvas;

/// A node placed in canvas pixels, relative to its parent: the canvas, or
/// the control it belongs to. `size` is `None` for text, which sizes
/// itself; a wrapped text has a width and no height.
#[derive(Component, Debug, Clone, Copy, PartialEq)]
pub struct OnCanvas {
    pub pos: Vec2,
    pub size: Option<Vec2>,
    /// A width with no height: text that wraps.
    pub wrap: Option<f32>,
}

impl OnCanvas {
    pub fn at(pos: Vec2) -> OnCanvas {
        OnCanvas {
            pos,
            size: None,
            wrap: None,
        }
    }

    pub fn sized(pos: Vec2, size: Vec2) -> OnCanvas {
        OnCanvas {
            pos,
            size: Some(size),
            wrap: None,
        }
    }
}

/// Text whose size is a `FONTS` height in 480-line pixels.
#[derive(Component, Debug, Clone, Copy)]
pub struct CanvasText(pub f32);

/// A control, by its name in the layout.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct DialogControl {
    pub dialog: String,
    pub name: String,
}

/// A control's bitmaps by state.
#[derive(Component, Debug, Clone, Default)]
pub struct Looks {
    pub idle: Option<Handle<Image>>,
    pub hover: Option<Handle<Image>>,
    pub pressed: Option<Handle<Image>>,
    pub disabled: Option<Handle<Image>>,
}

impl Looks {
    /// The bitmap for a state; `None` draws nothing.
    pub fn pick(
        &self,
        interaction: PickingInteraction,
        selected: bool,
        disabled: bool,
    ) -> Option<&Handle<Image>> {
        if disabled {
            return self.disabled.as_ref().or(self.idle.as_ref());
        }
        match interaction {
            PickingInteraction::Pressed => self
                .pressed
                .as_ref()
                .or(self.hover.as_ref())
                .or(self.idle.as_ref()),
            _ if selected => self
                .pressed
                .as_ref()
                .or(self.hover.as_ref())
                .or(self.idle.as_ref()),
            PickingInteraction::Hovered => self.hover.as_ref().or(self.idle.as_ref()),
            PickingInteraction::None => self.idle.as_ref(),
        }
    }
}

/// A control's sounds.
#[derive(Component, Debug, Clone, Default)]
pub struct Cues {
    pub rollover: Option<SoundCue>,
    pub activate: Option<SoundCue>,
}

/// A slider showing, and setting, a volume.
#[derive(Component, Debug, Clone, Copy)]
pub struct DialogSlider(pub SettingKey);

/// A slider's filled part.
#[derive(Component, Debug, Clone, Copy)]
struct SliderFill {
    key: SettingKey,
    /// The bar's width in canvas pixels, and its art's height.
    size: Vec2,
}

/// A checkbox's tick, shown while its setting is on.
#[derive(Component, Debug, Clone, Copy)]
struct SettingMark(SettingKey);

/// A cycle's text and picture, following a setting.
#[derive(Component, Debug, Clone)]
struct SettingCycle {
    key: SettingKey,
    /// Each state's picture, in `CYCLE` order.
    pictures: Vec<Option<Handle<Image>>>,
}

/// A text field a screen reads back.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogField {
    PlayerName,
    SaveName,
    CheatCode,
}

/// How a control is shown.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Show {
    #[default]
    Normal,
    /// Its `DISABLE` bitmap, and no clicks.
    Disabled,
    /// Not drawn at all.
    Hidden,
}

/// What a screen wants of one control. The default is inert: drawn, and
/// nothing happens.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Wiring {
    /// What a click does.
    pub action: Option<UiAction>,
    pub show: Show,
    /// Chosen: a radio button's `ACTIVATE` bitmap.
    pub selected: bool,
    /// A checkbox's tick, when not bound to a setting.
    pub checked: bool,
    /// The text a field, a text button or a cycle shows, over the layout's.
    pub text: Option<String>,
    /// Which `CYCLE` state to show.
    pub cycle: usize,
    /// A setting the checkbox, slider or cycle shows live.
    pub setting: Option<SettingKey>,
    /// A text field the player types in, and its longest text.
    pub field: Option<(DialogField, usize)>,
    /// A radio group's items, in order.
    pub items: Vec<Wiring>,
}

impl Wiring {
    /// A button that does this.
    pub fn action(action: UiAction) -> Wiring {
        Wiring {
            action: Some(action),
            ..default()
        }
    }

    /// Drawn and inert.
    pub fn inert() -> Wiring {
        Wiring::default()
    }

    /// Not drawn.
    pub fn hidden() -> Wiring {
        Wiring {
            show: Show::Hidden,
            ..default()
        }
    }

    /// The same, disabled when `disabled`.
    pub fn disabled_if(mut self, disabled: bool) -> Wiring {
        if disabled {
            self.show = Show::Disabled;
        }
        self
    }

    /// Bound to a setting.
    pub fn setting(key: SettingKey) -> Wiring {
        Wiring {
            action: Some(UiAction::Setting(SettingControl::Cycle(key))),
            setting: Some(key),
            ..default()
        }
    }
}

/// Draws one dialog onto its canvas.
pub struct DialogPainter<'a> {
    pub menus: &'a OriginalMenus,
    pub dialog: &'a Dialog,
    pub messages: Option<&'a Messages>,
    pub fonts: &'a UiFonts,
    /// The canvas entity everything is drawn under.
    pub canvas: Entity,
}

/// The backdrop behind a dialog.
#[derive(Debug, Clone, PartialEq)]
pub enum Backdrop {
    /// Black: the front end without a splash, the briefing.
    Black,
    /// In game with no capture of the view: the paused game, dimmed.
    Dimmed,
    /// Over another dialog: nothing.
    Clear,
    /// The splash, fitted to the window's height and centred on black.
    Splash(Handle<Image>),
    /// The game view as the menu opened, blurred, over the whole window.
    Blurred(Handle<Image>),
}

/// A 4:3 picture as big as fits under `parent`, centred vertically and
/// either centred across or against the left edge — where the dialogs'
/// canvas is ([`super::frame`]), so a dialog over it sits as it did on the
/// original's 4:3 screen.
pub fn spawn_fitted_picture(
    commands: &mut Commands,
    parent: Entity,
    image: Handle<Image>,
    left: bool,
) {
    let frame = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                justify_content: if left {
                    JustifyContent::FlexStart
                } else {
                    JustifyContent::Center
                },
                align_items: AlignItems::Center,
                ..default()
            },
            Pickable::IGNORE,
            ChildOf(parent),
        ))
        .id();
    commands.spawn((
        Node {
            height: percent(100),
            max_width: percent(100),
            aspect_ratio: Some(super::frame::DIALOG_CANVAS.x / super::frame::DIALOG_CANVAS.y),
            ..default()
        },
        ImageNode {
            image,
            image_mode: NodeImageMode::Stretch,
            ..default()
        },
        Pickable::IGNORE,
        ChildOf(frame),
    ));
}

impl<'a> DialogPainter<'a> {
    /// Spawn the root and the canvas, and draw the dialog's backgrounds
    /// and text. Returns the root, which the caller gives a lifetime.
    pub fn open(
        commands: &mut Commands,
        menus: &'a OriginalMenus,
        dialog: &'a Dialog,
        messages: Option<&'a Messages>,
        fonts: &'a UiFonts,
        backdrop: Backdrop,
        z: i32,
    ) -> (Entity, DialogPainter<'a>) {
        let colour = match backdrop {
            Backdrop::Black | Backdrop::Splash(_) => Color::BLACK,
            Backdrop::Dimmed => Color::srgba(0.0, 0.0, 0.0, 0.6),
            Backdrop::Clear | Backdrop::Blurred(_) => Color::NONE,
        };
        let root = commands
            .spawn((
                DialogRoot {
                    name: dialog.name.clone(),
                },
                Name::new(format!("dialog {}", dialog.name)),
                Node {
                    position_type: PositionType::Absolute,
                    width: percent(100),
                    height: percent(100),
                    overflow: Overflow::clip(),
                    ..default()
                },
                BackgroundColor(colour),
                GlobalZIndex(z),
            ))
            .id();
        match &backdrop {
            Backdrop::Clear => {
                commands.entity(root).insert(Pickable::IGNORE);
            }
            Backdrop::Splash(image) => spawn_fitted_picture(commands, root, image.clone(), true),
            Backdrop::Blurred(image) => {
                commands.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        width: percent(100),
                        height: percent(100),
                        ..default()
                    },
                    ImageNode {
                        image: image.clone(),
                        image_mode: NodeImageMode::Stretch,
                        // A little darker, so the dialogs stand out.
                        color: Color::srgb(0.75, 0.75, 0.75),
                        ..default()
                    },
                    Pickable::IGNORE,
                    ChildOf(root),
                ));
            }
            _ => {}
        }
        let canvas = commands
            .spawn((
                DialogCanvas,
                Node {
                    position_type: PositionType::Absolute,
                    ..default()
                },
                Pickable::IGNORE,
                ChildOf(root),
            ))
            .id();
        let painter = DialogPainter {
            menus,
            dialog,
            messages,
            fonts,
            canvas,
        };
        painter.backgrounds(commands);
        (root, painter)
    }

    /// `BACKGROUND_IMAGES` and `BACKGROUND_TEXT`.
    fn backgrounds(&self, commands: &mut Commands) {
        for placed in &self.dialog.backgrounds {
            self.image(commands, &placed.bitmap, placed.pos.as_vec2());
        }
        for text in &self.dialog.texts {
            let words = self.resolve(&text.text);
            self.text(commands, text.pos.as_vec2(), text.font, words);
        }
    }

    /// A `MSG_*` key through the string table; literal text as it is.
    pub fn resolve(&self, text: &str) -> String {
        if text.starts_with("MSG_") {
            message_text(self.messages, text)
        } else {
            text.to_string()
        }
    }

    /// A bitmap at a canvas position, at its own size.
    pub fn image(&self, commands: &mut Commands, bitmap: &str, pos: Vec2) -> Option<Entity> {
        self.image_under(commands, self.canvas, bitmap, pos)
    }

    fn image_under(
        &self,
        commands: &mut Commands,
        parent: Entity,
        bitmap: &str,
        pos: Vec2,
    ) -> Option<Entity> {
        let art = self.menus.art(bitmap)?;
        Some(
            commands
                .spawn((
                    OnCanvas::sized(pos, art.size),
                    absolute(),
                    ImageNode {
                        image: art.image.clone(),
                        image_mode: NodeImageMode::Stretch,
                        ..default()
                    },
                    Pickable::IGNORE,
                    ChildOf(parent),
                ))
                .id(),
        )
    }

    /// The dialog's font `id`, or its first.
    pub fn font(&self, id: u32) -> Option<&DialogFont> {
        self.dialog.font(id).or(self.dialog.fonts.first())
    }

    /// Text in a `FONTS` style at a canvas position.
    pub fn text(
        &self,
        commands: &mut Commands,
        pos: Vec2,
        font: u32,
        text: impl Into<String>,
    ) -> Entity {
        self.text_under(commands, self.canvas, pos, font, text)
    }

    /// Text that wraps at `width` canvas pixels.
    pub fn wrapped(
        &self,
        commands: &mut Commands,
        pos: Vec2,
        width: f32,
        font: u32,
        text: impl Into<String>,
    ) -> Entity {
        let entity = self.text(commands, pos, font, text);
        commands.entity(entity).insert((
            OnCanvas {
                pos,
                size: None,
                wrap: Some(width),
            },
            TextLayout::default(),
        ));
        entity
    }

    fn text_under(
        &self,
        commands: &mut Commands,
        parent: Entity,
        pos: Vec2,
        font: u32,
        text: impl Into<String>,
    ) -> Entity {
        let bundle = self.text_bundle(font, text);
        commands
            .spawn((OnCanvas::at(pos), absolute(), bundle, ChildOf(parent)))
            .id()
    }

    fn text_bundle(
        &self,
        font: u32,
        text: impl Into<String>,
    ) -> (Text, TextFont, TextColor, CanvasText, TextLayout, Pickable) {
        let (size, colour, bold) = self
            .font(font)
            .map(|font| (font.size as f32, font_colour(font), font.weight >= 700))
            .unwrap_or((16.0, Color::WHITE, false));
        (
            Text::new(text),
            TextFont {
                font: if bold {
                    self.fonts.bold.clone().into()
                } else {
                    default()
                },
                ..TextFont::from_font_size(size)
            },
            TextColor(colour),
            CanvasText(size),
            TextLayout::no_wrap(),
            Pickable::IGNORE,
        )
    }

    /// Every control, wired by the screen. Returns the spawned control
    /// entities by name (a radio group's items as `NAME[i]`).
    pub fn controls(
        &self,
        commands: &mut Commands,
        mut wire: impl FnMut(&Control) -> Wiring,
    ) -> Vec<(String, Entity)> {
        let mut spawned = Vec::new();
        for control in &self.dialog.controls {
            let wiring = wire(control);
            self.control(commands, control, &wiring, &mut spawned);
        }
        spawned
    }

    fn control(
        &self,
        commands: &mut Commands,
        control: &Control,
        wiring: &Wiring,
        spawned: &mut Vec<(String, Entity)>,
    ) {
        if wiring.show == Show::Hidden {
            return;
        }
        if let ControlKind::Radio(items) = &control.kind {
            for (index, item) in items.iter().enumerate() {
                let item_wiring = wiring.items.get(index).cloned().unwrap_or_default();
                self.control(commands, item, &item_wiring, spawned);
            }
            return;
        }

        let art = |name: Option<&str>| {
            name.and_then(|name| self.menus.art(name))
                .map(|art| art.image.clone())
        };
        let looks = Looks {
            idle: art(control.bitmap.as_ref().map(|b| b.name.as_str())),
            hover: art(control.rollover.as_ref().and_then(|v| v.bitmap.as_deref())),
            pressed: art(control.activate.as_ref().and_then(|v| v.bitmap.as_deref())),
            disabled: art(control.disabled.as_deref()),
        };
        let offset = control
            .bitmap
            .as_ref()
            .map_or(Vec2::ZERO, |bitmap| bitmap.offset.as_vec2());
        let size = control
            .size(|name| self.menus.art(name).map(|art| art.size.as_uvec2()))
            .map(|size| size.as_vec2())
            .unwrap_or(Vec2::ZERO);
        let disabled = wiring.show == Show::Disabled;
        let shown = looks
            .pick(PickingInteraction::None, wiring.selected, disabled)
            .cloned();

        let mut node = commands.spawn((
            DialogControl {
                dialog: self.dialog.name.clone(),
                name: control.name.clone(),
            },
            Name::new(control.name.clone()),
            OnCanvas::sized(control.pos.as_vec2() + offset, size),
            absolute(),
            ImageNode {
                image: shown.clone().unwrap_or_default(),
                color: if shown.is_some() {
                    Color::WHITE
                } else {
                    Color::NONE
                },
                image_mode: NodeImageMode::Stretch,
                ..default()
            },
            ChildOf(self.canvas),
        ));
        let interactive =
            wiring.action.is_some() || wiring.field.is_some() || wiring.setting.is_some();
        if interactive {
            node.insert((
                Button,
                // Picking's state rather than `Interaction`, which only sees
                // a pointer over a window: the UI may draw into an image.
                PickingInteraction::default(),
                ArtButton,
                looks,
                Cues {
                    rollover: control.sounds.rollover.clone(),
                    activate: control.sounds.activate.clone(),
                },
            ));
            if let Some(action) = wiring.action {
                node.insert(UiButton(action));
            }
        } else {
            node.insert(Pickable::IGNORE);
        }
        // A field's text stops at its edge.
        if matches!(control.kind, ControlKind::Label) {
            node.insert(Node {
                position_type: PositionType::Absolute,
                overflow: Overflow::clip(),
                ..default()
            });
        }
        if disabled {
            node.insert(Disabled);
        }
        if wiring.selected {
            node.insert(Selected);
        }
        let entity = node.id();
        spawned.push((control.name.clone(), entity));

        // Text is placed relative to the control's position, which is the
        // node's less the bitmap's offset.
        let local = |pos: IVec2| pos.as_vec2() - offset;

        // Captions (a radio button's) and fields.
        match (&control.kind, wiring.field) {
            (ControlKind::Label, Some((field, max))) => {
                if let Some(label) = control.labels.first() {
                    let bundle = self.text_bundle(label.font, "");
                    let width = (size.x - 2.0 * label.pos.x as f32).max(8.0);
                    let initial = wiring.text.clone().unwrap_or_default();
                    commands.spawn((
                        field,
                        OnCanvas::sized(local(label.pos), Vec2::new(width, size.y)),
                        absolute(),
                        EditableText {
                            max_characters: Some(max),
                            allow_newlines: false,
                            ..EditableText::new(&initial)
                        },
                        bundle.1,
                        bundle.2,
                        bundle.3,
                        TextLayout::no_wrap(),
                        TextCursorStyle::default(),
                        ChildOf(entity),
                    ));
                }
            }
            (ControlKind::Label, None) => {
                if let Some(label) = control.labels.first() {
                    let words = wiring
                        .text
                        .clone()
                        .unwrap_or_else(|| self.resolve(&label.text));
                    self.text_under(commands, entity, local(label.pos), label.font, words);
                }
            }
            _ => {
                for label in &control.labels {
                    let words = self.resolve(&label.text);
                    self.text_under(commands, entity, local(label.pos), label.font, words);
                }
            }
        }

        match &control.kind {
            ControlKind::Checkbox(mark) => {
                let tick = self.text_under(
                    commands,
                    entity,
                    local(mark.pos),
                    mark.font,
                    self.resolve(&mark.text),
                );
                let on = wiring.checked;
                commands.entity(tick).insert(if on {
                    Visibility::Inherited
                } else {
                    Visibility::Hidden
                });
                if let Some(key) = wiring.setting {
                    commands.entity(tick).insert(SettingMark(key));
                }
            }
            ControlKind::Cycle(states) => {
                let Some(state) = states.get(wiring.cycle).or(states.first()) else {
                    return;
                };
                let words = wiring
                    .text
                    .clone()
                    .unwrap_or_else(|| self.resolve(&state.text.text));
                let text = self.text_under(
                    commands,
                    entity,
                    local(state.text.pos),
                    state.text.font,
                    words,
                );
                let picture = state.bitmap.as_ref().and_then(|bitmap| {
                    self.image_under(commands, entity, &bitmap.name, local(bitmap.offset))
                });
                if let Some(key) = wiring.setting {
                    let pictures: Vec<Option<Handle<Image>>> = states
                        .iter()
                        .map(|state| {
                            state
                                .bitmap
                                .as_ref()
                                .and_then(|bitmap| self.menus.art(&bitmap.name))
                                .map(|art| art.image.clone())
                        })
                        .collect();
                    let cycle = SettingCycle { key, pictures };
                    commands.entity(text).insert(cycle.clone());
                    if let Some(picture) = picture {
                        commands.entity(picture).insert(cycle);
                    }
                }
            }
            ControlKind::Slider { fill } => {
                if let (Some(key), Some(art)) = (wiring.setting, self.menus.art(fill)) {
                    commands.entity(entity).insert(DialogSlider(key));
                    commands.spawn((
                        SliderFill {
                            key,
                            size: Vec2::new(size.x, art.size.y),
                        },
                        OnCanvas::sized(Vec2::ZERO, Vec2::new(0.0, art.size.y)),
                        absolute(),
                        ImageNode {
                            image: art.image.clone(),
                            image_mode: NodeImageMode::Stretch,
                            rect: Some(Rect::new(0.0, 0.0, 0.0, art.size.y)),
                            ..default()
                        },
                        Pickable::IGNORE,
                        ChildOf(entity),
                    ));
                }
            }
            ControlKind::Text {
                font,
                offset: text_at,
            } => {
                let words = wiring.text.clone().unwrap_or_default();
                self.text_under(commands, entity, local(*text_at), *font, words);
            }
            _ => {}
        }
    }
}

fn font_colour(font: &DialogFont) -> Color {
    let [r, g, b] = font.colour;
    Color::srgb_u8(r, g, b)
}

fn absolute() -> Node {
    Node {
        position_type: PositionType::Absolute,
        ..default()
    }
}

/// The dialogs' systems.
pub fn plugin(app: &mut App) {
    app.init_resource::<LastRollover>()
        .add_message::<PlaySound>()
        .add_systems(
            Update,
            (
                update_frame,
                relayout,
                update_filter,
                apply_looks,
                play_cues,
                drag_sliders.before(crate::ui::theme::dispatch_clicks),
                show_settings,
            )
                .chain(),
        );
}

/// Track the window.
fn update_frame(
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    current: Option<Res<CurrentDialogFrame>>,
    mut commands: Commands,
) {
    let Some(window) = window else {
        return;
    };
    let frame = DialogFrame::new(Vec2::new(window.width(), window.height()));
    if current.is_none_or(|current| current.0 != frame) {
        commands.insert_resource(CurrentDialogFrame(frame));
    }
}

/// Place the canvases and every node on them, and size their text, when
/// the frame changes or new nodes appear.
#[allow(clippy::type_complexity)]
pub fn relayout(
    frame: Option<Res<CurrentDialogFrame>>,
    mut canvases: Query<(Ref<DialogCanvas>, &mut Node), Without<OnCanvas>>,
    mut nodes: Query<(Ref<OnCanvas>, &mut Node), Without<DialogCanvas>>,
    mut texts: Query<(Ref<CanvasText>, &mut TextFont)>,
) {
    let Some(frame) = frame else {
        return;
    };
    let changed = frame.is_changed();
    let frame = frame.0;
    let s = frame.scale;

    for (canvas, mut node) in &mut canvases {
        if !changed && !canvas.is_added() {
            continue;
        }
        let rect = frame.canvas();
        node.left = px(rect.min.x);
        node.top = px(rect.min.y);
        node.width = px(rect.width());
        node.height = px(rect.height());
    }
    for (placed, mut node) in &mut nodes {
        if !changed && !placed.is_changed() {
            continue;
        }
        node.left = px(placed.pos.x * s);
        node.top = px(placed.pos.y * s);
        match (placed.size, placed.wrap) {
            (Some(size), _) => {
                node.width = px(size.x * s);
                node.height = px(size.y * s);
            }
            (None, Some(width)) => {
                node.width = px(width * s);
            }
            (None, None) => {}
        }
    }
    for (scaled, mut font) in &mut texts {
        if !changed && !scaled.is_added() {
            continue;
        }
        font.font_size = FontSize::Px(scaled.0 * s);
    }
}

/// Nearest sampling at whole-number scales, linear otherwise (the original
/// HUD's rule, for the same reasons).
fn update_filter(
    style: Res<MenuStyle>,
    frame: Option<Res<CurrentDialogFrame>>,
    mut images: ResMut<Assets<Image>>,
    mut last: Local<Option<bool>>,
) {
    let (MenuStyle::Original(menus), Some(frame)) = (&*style, frame) else {
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
    for art in menus.art.values() {
        if let Some(mut image) = images.get_mut(&art.image) {
            image.sampler = hud_sampler(filter);
        }
    }
}

/// Show each control's bitmap for its state.
#[allow(clippy::type_complexity)]
fn apply_looks(
    mut controls: Query<(
        &Looks,
        &PickingInteraction,
        &mut ImageNode,
        Has<Selected>,
        Has<Disabled>,
    )>,
) {
    for (looks, interaction, mut image, selected, disabled) in &mut controls {
        let wanted = looks.pick(*interaction, selected, disabled);
        let (handle, colour) = match wanted {
            Some(handle) => (handle.clone(), Color::WHITE),
            None => (image.image.clone(), Color::NONE),
        };
        if image.image != handle {
            image.image = handle;
        }
        if image.color != colour {
            image.color = colour;
        }
    }
}

/// The control the pointer was last over, so a dialog rebuilt under a
/// still pointer does not play the rollover again.
#[derive(Resource, Debug, Clone, Default, PartialEq, Eq)]
struct LastRollover(Option<(String, String)>);

/// Rollover and select sounds.
fn play_cues(
    controls: Query<
        (&PickingInteraction, &Cues, &DialogControl, Has<Disabled>),
        Changed<PickingInteraction>,
    >,
    all: Query<(&PickingInteraction, &DialogControl)>,
    mut last: ResMut<LastRollover>,
    mut sounds: MessageWriter<PlaySound>,
) {
    for (interaction, cues, control, disabled) in &controls {
        let key = Some((control.dialog.clone(), control.name.clone()));
        match interaction {
            PickingInteraction::Hovered if last.0 != key => {
                last.0 = key;
                if let Some(cue) = &cues.rollover {
                    sounds.write(cue_sound(cue));
                }
            }
            PickingInteraction::Pressed if !disabled => {
                if let Some(cue) = &cues.activate {
                    sounds.write(cue_sound(cue));
                }
            }
            _ => {}
        }
    }
    // Off every control: the next one entered is a new rollover.
    if last.0.is_some()
        && all
            .iter()
            .all(|(interaction, _)| *interaction == PickingInteraction::None)
    {
        last.0 = None;
    }
}

fn cue_sound(cue: &SoundCue) -> PlaySound {
    PlaySound {
        volume: cue.volume,
        ..PlaySound::ui(cue.name.clone())
    }
}

/// A slider held down sets its volume from where the pointer is along it.
fn drag_sliders(
    sliders: Query<(Entity, &DialogSlider, &PickingInteraction)>,
    // Optional so a headless test app without picking still builds.
    hovers: Option<Res<HoverMap>>,
    settings: Res<Settings>,
    mut clicks: MessageWriter<UiClick>,
) {
    let Some(hovers) = hovers else {
        return;
    };
    for (entity, slider, interaction) in &sliders {
        if *interaction != PickingInteraction::Pressed {
            continue;
        }
        // Across the slider from −0.5 to 0.5, from whichever pointer is on
        // it.
        let Some(along) = hovers.values().find_map(|hits| hits.get(&entity)?.position) else {
            continue;
        };
        let percent = ((along.x + 0.5).clamp(0.0, 1.0) * 100.0).round() as u8;
        let current = (setting_level(&settings, slider.0) * 100.0).round() as u8;
        // Tenths, as the settings keep them.
        if percent.abs_diff(current) >= 5 {
            clicks.write(UiClick(UiAction::Setting(SettingControl::Set(
                slider.0, percent,
            ))));
        }
    }
}

/// A volume setting's level, 0–1.
pub fn setting_level(settings: &Settings, key: SettingKey) -> f32 {
    match key {
        SettingKey::Master => settings.audio.master,
        SettingKey::Effects => settings.audio.effects,
        SettingKey::Music => settings.audio.music,
        _ => 0.0,
    }
}

/// Whether a switch is on, as the original's checkboxes show it.
pub fn setting_on(settings: &Settings, key: SettingKey) -> bool {
    match key {
        SettingKey::Mute => !settings.audio.muted,
        SettingKey::MusicOn => settings.audio.music > 0.0,
        SettingKey::HudStrips => settings.hud.strips,
        SettingKey::HudClock => settings.hud.clock,
        SettingKey::VSync => settings.video.vsync,
        SettingKey::TextureFilter => settings.texture_filter_linear,
        SettingKey::HudOriginal => settings.hud.original,
        SettingKey::MenusOriginal => settings.menus.original,
        SettingKey::InvertPitch => settings.input.invert_pitch,
        _ => false,
    }
}

/// Which of a cycle's pictures stands for a setting's value: for the
/// resolution, the preview whose aspect is closest to the window's.
pub fn cycle_picture(
    settings: &Settings,
    key: SettingKey,
    aspects: &[Option<f32>],
) -> Option<usize> {
    match key {
        SettingKey::Resolution => {
            let (width, height) = settings.video.resolution;
            let aspect = width as f32 / height.max(1) as f32;
            aspects
                .iter()
                .enumerate()
                .filter_map(|(index, candidate)| {
                    Some((index, (candidate.as_ref()? - aspect).abs()))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(index, _)| index)
        }
        _ => None,
    }
}

/// Keep ticks, slider fills and cycles in step with the settings.
#[allow(clippy::type_complexity)]
fn show_settings(
    settings: Res<Settings>,
    images: Res<Assets<Image>>,
    mut marks: Query<(Ref<SettingMark>, &mut Visibility)>,
    mut fills: Query<(Ref<SliderFill>, &mut OnCanvas, &mut ImageNode), Without<SettingCycle>>,
    mut cycle_texts: Query<(Ref<SettingCycle>, &mut Text)>,
    mut cycle_pictures: Query<(Ref<SettingCycle>, &mut ImageNode), Without<SliderFill>>,
) {
    let all = settings.is_changed();
    for (mark, mut visibility) in &mut marks {
        if all || mark.is_added() {
            visibility.set_if_neq(if setting_on(&settings, mark.0) {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            });
        }
    }
    for (fill, mut placed, mut image) in &mut fills {
        if !(all || fill.is_added()) {
            continue;
        }
        let level = setting_level(&settings, fill.key).clamp(0.0, 1.0);
        let width = fill.size.x * level;
        let texels = images
            .get(&image.image)
            .map_or(fill.size.x, |image| image.width() as f32);
        placed.set_if_neq(OnCanvas::sized(Vec2::ZERO, Vec2::new(width, fill.size.y)));
        image.rect = Some(Rect::new(0.0, 0.0, texels * level, fill.size.y));
    }
    for (cycle, mut text) in &mut cycle_texts {
        if all || cycle.is_added() {
            text.set_if_neq(Text::new(cycle_text(&settings, cycle.key)));
        }
    }
    for (cycle, mut image) in &mut cycle_pictures {
        if !(all || cycle.is_added()) {
            continue;
        }
        let aspects: Vec<Option<f32>> = cycle
            .pictures
            .iter()
            .map(|picture| {
                let image = images.get(picture.as_ref()?)?;
                Some(image.width() as f32 / image.height().max(1) as f32)
            })
            .collect();
        if let Some(index) = cycle_picture(&settings, cycle.key, &aspects)
            && let Some(Some(picture)) = cycle.pictures.get(index)
            && image.image != *picture
        {
            image.image = picture.clone();
        }
    }
}

/// What a cycle bound to a setting reads.
pub fn cycle_text(settings: &Settings, key: SettingKey) -> String {
    match key {
        SettingKey::Resolution => {
            let (width, height) = settings.video.resolution;
            format!("{width}x{height}")
        }
        other => crate::ui::settings_screen::value_label(settings, other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(n: u128) -> Option<Handle<Image>> {
        Some(Handle::Uuid(
            bevy::asset::uuid::Uuid::from_u128(n),
            default(),
        ))
    }

    #[test]
    fn a_control_shows_the_bitmap_for_its_state() {
        let looks = Looks {
            idle: handle(1),
            hover: handle(2),
            pressed: handle(3),
            disabled: handle(4),
        };
        assert_eq!(
            looks.pick(PickingInteraction::None, false, false),
            handle(1).as_ref()
        );
        assert_eq!(
            looks.pick(PickingInteraction::Hovered, false, false),
            handle(2).as_ref()
        );
        assert_eq!(
            looks.pick(PickingInteraction::Pressed, false, false),
            handle(3).as_ref()
        );
        assert_eq!(
            looks.pick(PickingInteraction::None, true, false),
            handle(3).as_ref()
        );
        assert_eq!(
            looks.pick(PickingInteraction::Hovered, false, true),
            handle(4).as_ref()
        );

        // A checkbox: nothing until hovered.
        let checkbox = Looks {
            hover: handle(2),
            ..default()
        };
        assert_eq!(checkbox.pick(PickingInteraction::None, false, false), None);
        assert_eq!(
            checkbox.pick(PickingInteraction::Hovered, false, false),
            handle(2).as_ref()
        );
    }

    #[test]
    fn the_resolution_preview_is_the_closest_aspect() {
        let mut settings = Settings::default();
        let aspects = [Some(4.0 / 3.0), Some(16.0 / 10.0), None];
        settings.video.resolution = (1280, 800);
        assert_eq!(
            cycle_picture(&settings, SettingKey::Resolution, &aspects),
            Some(1)
        );
        settings.video.resolution = (1024, 768);
        assert_eq!(
            cycle_picture(&settings, SettingKey::Resolution, &aspects),
            Some(0)
        );
        assert_eq!(cycle_text(&settings, SettingKey::Resolution), "1024x768");
    }

    #[test]
    fn checkboxes_read_the_original_way_round() {
        let mut settings = Settings::default();
        // `SOUND_ACTIVE` is ticked while sound is not muted.
        assert!(setting_on(&settings, SettingKey::Mute));
        settings.audio.muted = true;
        assert!(!setting_on(&settings, SettingKey::Mute));
        settings.audio.music = 0.0;
        assert!(!setting_on(&settings, SettingKey::MusicOn));
    }
}
