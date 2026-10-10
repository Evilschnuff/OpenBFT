//! `dialog.zrd` and `briefing.zrd` → [`DialogSet`] and [`BriefingLayout`]:
//! the original menus' layouts.
//!
//! Like [`crate::hud`], this module only reads the files. Coordinates are
//! 640×480 pixels, the screen the art was drawn for; scaling and anchoring
//! are the game crate's. No layout number, bitmap name or string from the
//! files is repeated in code (the copyright rule in `docs/CONVENTIONS.md`).
//!
//! # The files
//!
//! `dialog.zrd` is `SHARED_IMAGE_PATH` and then one map per dialog
//! (`MAINMENU0`, `OPTIONSPANEL`, …); `briefing.zrd` is one map per level,
//! `CAMPAIGN1`…`CAMPAIGN13`, in the same shape (READER-SCHEMA §2.6, §2.7b).
//! A dialog map has:
//!
//! - `IMAGE_PATH` — the original authors' source directory, unused;
//! - `FONTS` — `[id, face, size, [r, g, b], flag, weight]` records;
//! - `BACKGROUND_IMAGES` — `[bitmap, x, y]`, drawn in order. The panel
//!   strips are 768 tall and sit at `y = −144`: drawn for 1024×768 and
//!   centred on 480 lines;
//! - `BACKGROUND_TEXT` — `[text, x, y, font]`, the text a `MSG_*` key;
//! - `BUTTONS` — named controls ([`Control`]), whatever their kind;
//! - `PRIMITIVES` — named places the game fills itself ([`Primitive`]):
//!   the save list's rows, a message box's text, the briefing's objective;
//! - `CURSOR` — `BITMAP`, the pointer;
//! - `BACKGROUND_VIDEOS` (once, the controls screen's pumps) — not read:
//!   the bitmaps under them are drawn and FMV is out of scope.
//!
//! Bitmap names are mixed case (`NewGame_H`); `image.zbd`'s are lower
//! case, so every name is looked up through
//! [`texture_key`](crate::textures::texture_key).

use std::collections::HashMap;

use bevy::math::{IRect, IVec2, UVec2};
use serde_json::Value;

use crate::hud::{DIALOG_ENTRY, HUD_ENTRY};
use crate::reader::ReaderData;
use crate::reader_value::ReaderValue;

/// The reader entry with the briefing and transport screens.
pub const BRIEFING_ENTRY: &str = "briefing.zrd";

/// Every dialog in `dialog.zrd`, by name.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DialogSet {
    /// `SHARED_IMAGE_PATH`: a source directory, kept for completeness.
    pub shared_image_path: Option<String>,
    pub dialogs: HashMap<String, Dialog>,
}

impl DialogSet {
    /// A dialog by name (`MAINMENU0`).
    pub fn get(&self, name: &str) -> Option<&Dialog> {
        self.dialogs.get(name)
    }
}

/// One dialog: what is drawn, and the controls on it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Dialog {
    pub name: String,
    pub fonts: Vec<DialogFont>,
    /// `BACKGROUND_IMAGES`, back to front.
    pub backgrounds: Vec<Placed>,
    /// `BACKGROUND_TEXT`.
    pub texts: Vec<PlacedText>,
    /// `BUTTONS`, in file order.
    pub controls: Vec<Control>,
    /// `PRIMITIVES`, in file order.
    pub primitives: Vec<Primitive>,
    /// `CURSOR`'s bitmap.
    pub cursor: Option<String>,
}

impl Dialog {
    /// A font by its `FONTS` id.
    pub fn font(&self, id: u32) -> Option<&DialogFont> {
        self.fonts.iter().find(|font| font.id == id)
    }

    /// A control by name.
    pub fn control(&self, name: &str) -> Option<&Control> {
        self.controls.iter().find(|control| control.name == name)
    }

    /// A primitive by name.
    pub fn primitive(&self, name: &str) -> Option<&Primitive> {
        self.primitives
            .iter()
            .find(|primitive| primitive.name == name)
    }

    /// Every bitmap the dialog names, as the file spells them, without
    /// repeats.
    pub fn bitmaps(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        names.extend(self.backgrounds.iter().map(|placed| placed.bitmap.as_str()));
        for control in &self.controls {
            control.collect_bitmaps(&mut names);
        }
        names.extend(
            self.primitives
                .iter()
                .filter_map(|primitive| primitive.bitmap.as_deref()),
        );
        names.extend(self.cursor.as_deref());
        names.sort_unstable();
        names.dedup();
        names
    }

    /// Every `MSG_*` key the dialog shows, without repeats.
    pub fn message_keys(&self) -> Vec<&str> {
        let mut texts: Vec<&PlacedText> = self.texts.iter().collect();
        for control in &self.controls {
            control.visit_texts(&mut |text| texts.push(text));
        }
        let mut keys: Vec<&str> = texts
            .into_iter()
            .filter(|text| text.is_message_key())
            .map(|text| text.text.as_str())
            .collect();
        keys.sort_unstable();
        keys.dedup();
        keys
    }
}

/// One `FONTS` record: `[id, face, size, [r, g, b], flag, weight]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogFont {
    pub id: u32,
    /// `Eurostile` everywhere but the multiplayer screen's `Courier New`.
    pub face: String,
    /// Height in 480-line pixels.
    pub size: u32,
    pub colour: [u8; 3],
    /// The fifth field, 0 or 1: 0 only on the dark, unshadowed input-field
    /// fonts. Unknown; read as "draw with a shadow" at most.
    pub flag: i64,
    /// 500 regular to 1000 heavy.
    pub weight: u32,
}

/// A bitmap at a position: `[bitmap, x, y]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    pub bitmap: String,
    /// Top-left corner. May be negative: the 768-tall strips start above
    /// the screen.
    pub pos: IVec2,
}

/// Text at a position: `[text, x, y, font]`.
///
/// Relative to its control when it belongs to one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlacedText {
    /// A `MSG_*` key, or literal text (`"X"`, `"640x480"`, `""`).
    pub text: String,
    pub pos: IVec2,
    /// A `FONTS` id.
    pub font: u32,
}

impl PlacedText {
    /// Whether the text is a string-table key rather than literal.
    pub fn is_message_key(&self) -> bool {
        self.text.starts_with("MSG_")
    }
}

/// A bitmap drawn with a control, offset from its position:
/// `["comselect", -7, -3]`, or a bare name at no offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub name: String,
    pub offset: IVec2,
}

/// A look a control takes: `ROLLOVER`'s hover bitmap, `ACTIVATE`'s pressed
/// or selected one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Visual {
    pub bitmap: Option<String>,
}

/// A sound a control makes: `[snd_name, volume]`, the volume 1 when the
/// file gives none.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundCue {
    /// A logical `sounds.zrd` name (`snd_rollover`, in the `DIALOG` set).
    pub name: String,
    pub volume: f32,
}

/// `ROLLOVER`'s and `ACTIVATE`'s sounds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ControlSounds {
    pub rollover: Option<SoundCue>,
    pub activate: Option<SoundCue>,
}

/// One entry of `BUTTONS`.
#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    /// What the game calls it (`NEWGAME`, `SOUND_VOLUME`): the name says
    /// what it does, the art says nothing a program can read.
    pub name: String,
    /// Top-left, on the canvas. A radio group has none of its own.
    pub pos: IVec2,
    pub kind: ControlKind,
    /// `BITMAP`: the idle look. Checkboxes and cycles have none; they are
    /// painted into the background and only light up on hover.
    pub bitmap: Option<Bitmap>,
    /// `ROLLOVER`'s bitmap: under the pointer.
    pub rollover: Option<Visual>,
    /// `ACTIVATE`'s bitmap: a pressed message-box button, a chosen radio
    /// button.
    pub activate: Option<Visual>,
    /// `DISABLE`'s bitmap: what shows when the control cannot be used
    /// (`LoadGame_no` reads "can not load while moving").
    pub disabled: Option<String>,
    /// `LABEL`: text drawn over the control. Empty text on a field the game
    /// fills (a name, a save's title, a number).
    pub labels: Vec<PlacedText>,
    pub sounds: ControlSounds,
}

/// What kind of widget a control is, from the keys it has.
#[derive(Debug, Clone, PartialEq)]
pub enum ControlKind {
    /// A bitmap that is clicked.
    Button,
    /// `CHECKED`: a box whose tick is text (`"X"`) drawn when on.
    Checkbox(PlacedText),
    /// `CYCLE`: each click moves to the next state.
    Cycle(Vec<CycleState>),
    /// `RADIO`: one of several buttons, each a control of its own.
    Radio(Vec<Control>),
    /// `FILLBITMAP`: a bar; `bitmap` is empty, this full, drawn to the level.
    Slider { fill: String },
    /// `LISTSIZE`…: a scrolling list the game fills.
    List(ListStyle),
    /// `FONT` and `TEXTOFFSET`: a button with text the game sets.
    Text { font: u32, offset: IVec2 },
    /// `LABEL` on its own: a field showing text — typed, or set by the
    /// game.
    Label,
    /// `SCROLLING_TEXT`: the credits roll.
    Scroller(Scroller),
}

/// One state of a `CYCLE`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleState {
    pub text: PlacedText,
    /// A picture for the state (a resolution's preview, a map's thumbnail).
    pub bitmap: Option<Bitmap>,
}

/// A list's look.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListStyle {
    pub list_font: u32,
    pub selected_font: u32,
    /// Pixels between rows.
    pub spacing: u32,
    /// `LISTSIZE`: two numbers, read as visible rows and columns.
    pub size: [u32; 2],
    /// `LIST_OFFSET`: unknown offsets, kept as read.
    pub offsets: Vec<IVec2>,
}

/// The credits roll.
#[derive(Debug, Clone, PartialEq)]
pub struct Scroller {
    /// Where the text shows, relative to the control: `RECT [[x0, y0],
    /// [x1, y1]]`, corners.
    pub rect: IRect,
    /// `SCROLL_RATE`: seconds per pixel.
    pub rate: f32,
    pub lines: Vec<ScrollLine>,
}

/// One line of the roll: `[text, x, gap, font]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrollLine {
    pub text: String,
    /// Indent from the rect's left.
    pub x: i32,
    /// Extra space above the line, in pixels.
    pub gap: i32,
    pub font: u32,
}

/// A named place the game draws into itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Primitive {
    pub name: String,
    pub pos: IVec2,
    pub font: Option<u32>,
    /// `WORDWRAP [width, height]`: the box text wraps in.
    pub wordwrap: Option<UVec2>,
    pub bitmap: Option<String>,
}

impl Control {
    fn collect_bitmaps<'a>(&'a self, names: &mut Vec<&'a str>) {
        names.extend(self.bitmap.as_ref().map(|bitmap| bitmap.name.as_str()));
        for visual in [&self.rollover, &self.activate].into_iter().flatten() {
            names.extend(visual.bitmap.as_deref());
        }
        names.extend(self.disabled.as_deref());
        match &self.kind {
            ControlKind::Cycle(states) => names.extend(
                states
                    .iter()
                    .filter_map(|state| state.bitmap.as_ref())
                    .map(|bitmap| bitmap.name.as_str()),
            ),
            ControlKind::Radio(items) => {
                for item in items {
                    item.collect_bitmaps(names);
                }
            }
            ControlKind::Slider { fill } => names.push(fill),
            _ => {}
        }
    }

    fn visit_texts<'a>(&'a self, take: &mut impl FnMut(&'a PlacedText)) {
        self.labels.iter().for_each(&mut *take);
        match &self.kind {
            ControlKind::Checkbox(mark) => take(mark),
            ControlKind::Cycle(states) => states.iter().for_each(|state| take(&state.text)),
            ControlKind::Radio(items) => {
                for item in items {
                    item.visit_texts(take);
                }
            }
            _ => {}
        }
    }

    /// The control's size on the canvas: its bitmap's, else its hover
    /// bitmap's — the only thing that says how big a checkbox is. `size`
    /// looks a bitmap's size up by name.
    pub fn size(&self, size: impl Fn(&str) -> Option<UVec2>) -> Option<UVec2> {
        self.bitmap
            .as_ref()
            .map(|bitmap| bitmap.name.as_str())
            .or_else(|| self.rollover.as_ref()?.bitmap.as_deref())
            .or(self.disabled.as_deref())
            .and_then(size)
    }
}

/// One `CAMPAIGN<n>` layout from `briefing.zrd`: the briefing, which is
/// also the loading ("transport") screen.
#[derive(Debug, Clone, PartialEq)]
pub struct BriefingLayout {
    /// The `<n>` of `CAMPAIGN<n>`: the level number, m1–m13.
    pub campaign: u32,
    pub dialog: Dialog,
}

impl BriefingLayout {
    /// `LOCATOR1`…: map positions, in order. Only m1–m6 have them.
    pub fn locators(&self) -> Vec<IVec2> {
        let mut found: Vec<(u32, IVec2)> = self
            .dialog
            .primitives
            .iter()
            .filter_map(|primitive| {
                let index = primitive.name.strip_prefix("LOCATOR")?.parse().ok()?;
                Some((index, primitive.pos))
            })
            .collect();
        found.sort_by_key(|(index, _)| *index);
        found.into_iter().map(|(_, pos)| pos).collect()
    }

    /// `TRANSPORT_PROGRESS`, the loading bar.
    pub fn transport(&self) -> Option<&Control> {
        self.dialog.control("TRANSPORT_PROGRESS")
    }
}

/// `hud.zrd`'s `BRIEFING`: what the transport bar says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportText {
    /// `LOADING_TEXT`'s words while the level loads.
    pub loading: String,
    pub loading_colour: [u8; 3],
    /// `READY_TEXT`'s, once it has.
    pub ready: String,
    pub ready_colour: [u8; 3],
    /// Text height in 480-line pixels (the second field of `FONT`).
    pub size: u32,
    /// `STAGES`: how many frames the loading words pulse through.
    pub stages: u32,
}

/// What went wrong reading a layout.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum DialogParseError {
    #[error("{0}: missing")]
    Missing(&'static str),
    #[error("{file}: {dialog}: {detail}")]
    Malformed {
        file: &'static str,
        dialog: String,
        detail: String,
    },
}

/// Read `dialog.zrd` out of the global reader archive.
pub fn dialog_set(reader: &ReaderData) -> Result<DialogSet, DialogParseError> {
    parse_dialogs(
        reader
            .entry(DIALOG_ENTRY)
            .ok_or(DialogParseError::Missing(DIALOG_ENTRY))?,
    )
}

/// Read `briefing.zrd` out of the global reader archive.
pub fn briefing_layouts(reader: &ReaderData) -> Result<Vec<BriefingLayout>, DialogParseError> {
    parse_briefings(
        reader
            .entry(BRIEFING_ENTRY)
            .ok_or(DialogParseError::Missing(BRIEFING_ENTRY))?,
    )
}

/// Read `hud.zrd`'s `BRIEFING` out of the global reader archive.
pub fn transport_text(reader: &ReaderData) -> Result<TransportText, DialogParseError> {
    parse_transport(
        reader
            .entry(HUD_ENTRY)
            .ok_or(DialogParseError::Missing(HUD_ENTRY))?,
    )
}

/// Parse `dialog.zrd`'s JSON, as the extraction writes it.
pub fn parse_dialogs(zrd: &Value) -> Result<DialogSet, DialogParseError> {
    let root = ReaderValue::root(zrd);
    let mut set = DialogSet::default();
    for (name, body) in root.pairs() {
        if name == "SHARED_IMAGE_PATH" {
            set.shared_image_path = body.string().map(str::to_string);
            continue;
        }
        let dialog = parse_dialog(DIALOG_ENTRY, name, body)?;
        set.dialogs.insert(name.to_string(), dialog);
    }
    if set.dialogs.is_empty() {
        return Err(DialogParseError::Missing("dialog.zrd's dialogs"));
    }
    Ok(set)
}

/// Parse `briefing.zrd`'s JSON: one layout per `CAMPAIGN<n>`, by `n`.
pub fn parse_briefings(zrd: &Value) -> Result<Vec<BriefingLayout>, DialogParseError> {
    let root = ReaderValue::root(zrd);
    let mut layouts = Vec::new();
    for (name, body) in root.pairs() {
        let Some(campaign) = name
            .strip_prefix("CAMPAIGN")
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        layouts.push(BriefingLayout {
            campaign,
            dialog: parse_dialog(BRIEFING_ENTRY, name, body)?,
        });
    }
    if layouts.is_empty() {
        return Err(DialogParseError::Missing("briefing.zrd's CAMPAIGN layouts"));
    }
    layouts.sort_by_key(|layout| layout.campaign);
    Ok(layouts)
}

/// Parse `hud.zrd`'s `BRIEFING` section.
///
/// `LOADING_TEXT ["Transporting", "FONT", [face, height, width, weight],
/// "COLOR", [r, g, b], "STAGES", [n]]`, and `READY_TEXT` alike.
pub fn parse_transport(zrd: &Value) -> Result<TransportText, DialogParseError> {
    let root = ReaderValue::root(zrd);
    let briefing = root
        .get("BRIEFING")
        .ok_or(DialogParseError::Missing("hud.zrd's BRIEFING"))?;
    let bad = |detail: &str| DialogParseError::Malformed {
        file: HUD_ENTRY,
        dialog: "BRIEFING".into(),
        detail: detail.into(),
    };
    let line = |key: &str| -> Result<(String, [u8; 3], u32, u32), DialogParseError> {
        let record = briefing.get(key).ok_or_else(|| bad(key))?;
        let words = record
            .nth(0)
            .and_then(|v| v.string())
            .ok_or_else(|| bad(&format!("{key}: no text")))?;
        // After the text the record is a map: "FONT", [...], "COLOR", [...].
        let pairs: Vec<(&str, ReaderValue)> = record
            .items()
            .windows(2)
            .filter_map(|pair| Some((pair[0].string()?, pair[1])))
            .collect();
        let find = |name: &str| pairs.iter().find(|(key, _)| *key == name).map(|(_, v)| *v);
        let colour = find("COLOR")
            .and_then(|v| rgb(&v))
            .ok_or_else(|| bad(&format!("{key}: no COLOR")))?;
        let size = find("FONT")
            .and_then(|v| v.nth(1))
            .and_then(|v| v.int())
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| bad(&format!("{key}: no FONT height")))?;
        let stages = find("STAGES")
            .and_then(|v| v.int())
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(1);
        Ok((words.to_string(), colour, size, stages))
    };
    let (loading, loading_colour, size, stages) = line("LOADING_TEXT")?;
    let (ready, ready_colour, _, _) = line("READY_TEXT")?;
    Ok(TransportText {
        loading,
        loading_colour,
        ready,
        ready_colour,
        size,
        stages,
    })
}

/// One dialog map.
fn parse_dialog(
    file: &'static str,
    name: &str,
    body: ReaderValue<'_>,
) -> Result<Dialog, DialogParseError> {
    let bad = |detail: String| DialogParseError::Malformed {
        file,
        dialog: name.to_string(),
        detail,
    };

    let mut dialog = Dialog {
        name: name.to_string(),
        ..Dialog::default()
    };

    if let Some(fonts) = body.get("FONTS") {
        for record in records(fonts) {
            let items = record.items();
            let number = |index: usize| items.get(index).and_then(|v| v.int());
            let (Some(id), Some(face), Some(size), Some(colour), Some(weight)) = (
                number(0).and_then(|v| u32::try_from(v).ok()),
                items.get(1).and_then(|v| v.string()),
                number(2).and_then(|v| u32::try_from(v).ok()),
                items.get(3).and_then(rgb),
                number(5).and_then(|v| u32::try_from(v).ok()),
            ) else {
                return Err(bad(format!(
                    "FONTS: a record is not [id, face, size, rgb, flag, weight]: {}",
                    record.raw()
                )));
            };
            dialog.fonts.push(DialogFont {
                id,
                face: face.to_string(),
                size,
                colour,
                flag: number(4).unwrap_or(0),
                weight,
            });
        }
    }

    if let Some(backgrounds) = body.get("BACKGROUND_IMAGES") {
        for record in records(backgrounds) {
            dialog.backgrounds.push(
                placed(&record)
                    .ok_or_else(|| bad(format!("BACKGROUND_IMAGES: {}", record.raw())))?,
            );
        }
    }

    if let Some(texts) = body.get("BACKGROUND_TEXT") {
        for record in records(texts) {
            dialog.texts.push(
                placed_text(&record)
                    .ok_or_else(|| bad(format!("BACKGROUND_TEXT: {}", record.raw())))?,
            );
        }
    }

    if let Some(buttons) = body.get("BUTTONS") {
        for (control_name, control_body) in buttons.pairs() {
            dialog.controls.push(
                parse_control(control_name, control_body)
                    .map_err(|detail| bad(format!("{control_name}: {detail}")))?,
            );
        }
    }

    if let Some(primitives) = body.get("PRIMITIVES") {
        for (primitive_name, primitive) in primitives.pairs() {
            let pos = primitive
                .get("POSITION")
                .and_then(|v| ivec2(&v))
                .ok_or_else(|| bad(format!("{primitive_name}: no POSITION")))?;
            dialog.primitives.push(Primitive {
                name: primitive_name.to_string(),
                pos,
                font: primitive
                    .get("FONT")
                    .and_then(|v| v.int())
                    .and_then(|v| u32::try_from(v).ok()),
                wordwrap: primitive
                    .get("WORDWRAP")
                    .and_then(|v| ivec2(&v))
                    .map(|size| size.max(IVec2::ZERO).as_uvec2()),
                bitmap: primitive
                    .get("BITMAP")
                    .and_then(|v| v.string())
                    .map(str::to_string),
            });
        }
    }

    dialog.cursor = body
        .get("CURSOR")
        .and_then(|cursor| cursor.get("BITMAP"))
        .and_then(|v| v.string())
        .map(str::to_string);

    Ok(dialog)
}

/// One control, from its map of keys.
fn parse_control(name: &str, body: ReaderValue<'_>) -> Result<Control, String> {
    let rollover = body.get("ROLLOVER");
    let activate = body.get("ACTIVATE");
    let sounds = ControlSounds {
        rollover: rollover.and_then(|v| v.get("SOUND")).and_then(sound),
        activate: activate.and_then(|v| v.get("SOUND")).and_then(sound),
    };
    let visual = |value: Option<ReaderValue<'_>>| {
        value.map(|value| Visual {
            bitmap: value
                .get("BITMAP")
                .and_then(|v| v.string())
                .map(str::to_string),
        })
    };

    let labels = match body.get("LABEL") {
        None => Vec::new(),
        Some(label) => records(label)
            .iter()
            .map(|record| placed_text(record).ok_or_else(|| format!("LABEL: {}", record.raw())))
            .collect::<Result<_, _>>()?,
    };

    let kind = if let Some(items) = body.get("RADIO") {
        ControlKind::Radio(
            items
                .items()
                .iter()
                .enumerate()
                .map(|(index, item)| parse_control(&format!("{name}[{index}]"), *item))
                .collect::<Result<_, _>>()?,
        )
    } else if let Some(checked) = body.get("CHECKED") {
        let mark = checked
            .get("TEXT")
            .and_then(|v| placed_text(&v))
            .ok_or("CHECKED without TEXT")?;
        ControlKind::Checkbox(mark)
    } else if let Some(states) = body.get("CYCLE") {
        ControlKind::Cycle(
            states
                .items()
                .iter()
                .map(|state| {
                    let text = state
                        .get("TEXT")
                        .and_then(|v| placed_text(&v))
                        .ok_or_else(|| format!("CYCLE: a state without TEXT: {}", state.raw()))?;
                    Ok(CycleState {
                        text,
                        bitmap: state.get("BITMAP").and_then(|v| bitmap(&v)),
                    })
                })
                .collect::<Result<_, String>>()?,
        )
    } else if let Some(fill) = body.get("FILLBITMAP") {
        ControlKind::Slider {
            fill: fill.string().ok_or("FILLBITMAP is not a name")?.to_string(),
        }
    } else if body.has("LISTSIZE") {
        let int = |key: &str| {
            body.get(key)
                .and_then(|v| v.int())
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| format!("{key} is not a number"))
        };
        let size = body
            .get("LISTSIZE")
            .and_then(|v| ivec2(&v))
            .ok_or("LISTSIZE is not [a, b]")?
            .max(IVec2::ZERO)
            .as_uvec2();
        ControlKind::List(ListStyle {
            list_font: int("LIST_FONT")?,
            selected_font: int("SELECTED_FONT")?,
            spacing: int("SPACING")?,
            size: [size.x, size.y],
            offsets: body
                .get("LIST_OFFSET")
                .map(|v| v.items().iter().filter_map(ivec2).collect())
                .unwrap_or_default(),
        })
    } else if let Some(lines) = body.get("SCROLLING_TEXT") {
        let rect = body.get("RECT").ok_or("SCROLLING_TEXT without RECT")?;
        let corners: Vec<IVec2> = rect.items().iter().filter_map(ivec2).collect();
        let [min, max] = corners[..] else {
            return Err("RECT is not [[x0, y0], [x1, y1]]".into());
        };
        ControlKind::Scroller(Scroller {
            rect: IRect::from_corners(min, max),
            rate: body
                .get("SCROLL_RATE")
                .and_then(|v| v.float())
                .ok_or("no SCROLL_RATE")?,
            lines: lines
                .items()
                .iter()
                .filter_map(|line| {
                    // Each line is wrapped once more: `[[text, x, gap, font]]`.
                    let record = records(*line).into_iter().next()?;
                    let text = placed_text(&record)?;
                    Some(ScrollLine {
                        text: text.text,
                        x: text.pos.x,
                        gap: text.pos.y,
                        font: text.font,
                    })
                })
                .collect(),
        })
    } else if let Some(font) = body.get("FONT") {
        ControlKind::Text {
            font: font
                .int()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or("FONT is not a number")?,
            offset: body
                .get("TEXTOFFSET")
                .and_then(|v| ivec2(&v))
                .unwrap_or(IVec2::ZERO),
        }
    } else if !labels.is_empty() && !is_radio_item(name) {
        ControlKind::Label
    } else {
        ControlKind::Button
    };

    let pos = match body.get("POSITION") {
        Some(pos) => ivec2(&pos).ok_or("POSITION is not [x, y]")?,
        None if matches!(kind, ControlKind::Radio(_)) => IVec2::ZERO,
        None => return Err("no POSITION".into()),
    };

    Ok(Control {
        name: name.to_string(),
        pos,
        kind,
        bitmap: body.get("BITMAP").and_then(|v| bitmap(&v)),
        rollover: visual(rollover),
        activate: visual(activate),
        disabled: body
            .get("DISABLE")
            .and_then(|v| v.get("BITMAP"))
            .and_then(|v| v.string())
            .map(str::to_string),
        labels,
        sounds,
    })
}

/// A radio group's items are named `GROUP[i]` while parsing; their labels
/// are captions, not fields.
fn is_radio_item(name: &str) -> bool {
    name.ends_with(']')
}

/// A value that is either one record or a list of them.
///
/// The file writes `[[a, b], [c, d]]` for several records and, where one
/// would do, sometimes `[a, b]` alone: a list whose first element is a list
/// is a list of records.
fn records(value: ReaderValue<'_>) -> Vec<ReaderValue<'_>> {
    match value.nth(0) {
        Some(first) if first.raw().is_array() => value.items(),
        Some(_) => vec![value],
        None => Vec::new(),
    }
}

fn ivec2(value: &ReaderValue<'_>) -> Option<IVec2> {
    match value.items().as_slice() {
        [x, y] => Some(IVec2::new(
            i32::try_from(x.int()?).ok()?,
            i32::try_from(y.int()?).ok()?,
        )),
        _ => None,
    }
}

fn rgb(value: &ReaderValue<'_>) -> Option<[u8; 3]> {
    let channels = value
        .items()
        .iter()
        .map(|v| v.int().and_then(|v| u8::try_from(v).ok()))
        .collect::<Option<Vec<u8>>>()?;
    <[u8; 3]>::try_from(channels).ok()
}

/// `[bitmap, x, y]`.
fn placed(value: &ReaderValue<'_>) -> Option<Placed> {
    Some(Placed {
        bitmap: value.nth(0)?.string()?.to_string(),
        pos: IVec2::new(
            i32::try_from(value.nth(1)?.int()?).ok()?,
            i32::try_from(value.nth(2)?.int()?).ok()?,
        ),
    })
}

/// `[text, x, y, font]`.
fn placed_text(value: &ReaderValue<'_>) -> Option<PlacedText> {
    Some(PlacedText {
        text: value.nth(0)?.string()?.to_string(),
        pos: IVec2::new(
            i32::try_from(value.nth(1)?.int()?).ok()?,
            i32::try_from(value.nth(2)?.int()?).ok()?,
        ),
        font: u32::try_from(value.nth(3)?.int()?).ok()?,
    })
}

/// `["name"]`, or `["name", dx, dy]`.
fn bitmap(value: &ReaderValue<'_>) -> Option<Bitmap> {
    if let Some(name) = value.string() {
        return Some(Bitmap {
            name: name.to_string(),
            offset: IVec2::ZERO,
        });
    }
    let name = value.nth(0)?.string()?.to_string();
    let offset = match (value.nth(1), value.nth(2)) {
        (Some(x), Some(y)) => {
            IVec2::new(i32::try_from(x.int()?).ok()?, i32::try_from(y.int()?).ok()?)
        }
        _ => IVec2::ZERO,
    };
    Some(Bitmap { name, offset })
}

/// `["snd_name"]` or `["snd_name", volume]`.
fn sound(value: ReaderValue<'_>) -> Option<SoundCue> {
    if let Some(name) = value.string() {
        return Some(SoundCue {
            name: name.to_string(),
            volume: 1.0,
        });
    }
    Some(SoundCue {
        name: value.nth(0)?.string()?.to_string(),
        volume: value.nth(1).and_then(|v| v.float()).unwrap_or(1.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A made-up file in the real one's shape: every kind of control once.
    fn file() -> Value {
        json!([[
            "SHARED_IMAGE_PATH",
            ["shared"],
            "FRONT",
            [
                "IMAGE_PATH",
                ["front"],
                "FONTS",
                [
                    [0, "Face", 20, [1, 2, 3], 1, 500],
                    [1, "Face", 16, [4, 5, 6], 0, 700]
                ],
                "BACKGROUND_IMAGES",
                [["Strip", 0, -144], ["Bolt", 83, 96]],
                "BACKGROUND_TEXT",
                [["MSG_HELLO", 320, 420, 1]],
                "BUTTONS",
                [
                    "GO",
                    [
                        "POSITION",
                        [269, 86],
                        "BITMAP",
                        ["Go"],
                        "ROLLOVER",
                        ["BITMAP", ["Go_H"], "SOUND", ["snd_over", 0.5]],
                        "ACTIVATE",
                        ["SOUND", ["snd_press"]],
                        "DISABLE",
                        ["BITMAP", ["Go_no"]]
                    ],
                    "TICK",
                    [
                        "POSITION",
                        [10, 20],
                        "ROLLOVER",
                        ["BITMAP", ["box_H"]],
                        "CHECKED",
                        ["TEXT", ["X", 9, 2, 1]]
                    ],
                    "LEVEL",
                    [
                        "POSITION",
                        [30, 40],
                        "ROLLOVER",
                        ["BITMAP", ["lvl_H"]],
                        "CYCLE",
                        [
                            ["TEXT", ["MSG_LOW", 1, 2, 0]],
                            ["TEXT", ["High", 3, 4, 0], "BITMAP", ["pic", 5, -6]]
                        ]
                    ],
                    "VOLUME",
                    [
                        "POSITION",
                        [50, 60],
                        "BITMAP",
                        ["Empty"],
                        "FILLBITMAP",
                        ["Full"]
                    ],
                    "MODE",
                    [
                        "RADIO",
                        [
                            [
                                "POSITION",
                                [1, 2],
                                "BITMAP",
                                ["Radio"],
                                "LABEL",
                                [["MSG_A", 2, 5, 1], ["MSG_B", 2, 25, 1]],
                                "ACTIVATE",
                                ["BITMAP", ["Radio_S"]]
                            ],
                            ["POSITION", [3, 4], "BITMAP", ["Radio"]]
                        ]
                    ],
                    "NAME",
                    [
                        "POSITION",
                        [180, 83],
                        "BITMAP",
                        ["Field"],
                        "LABEL",
                        ["", 14, 4, 1]
                    ],
                    "KEYS",
                    [
                        "POSITION",
                        [259, 141],
                        "BITMAP",
                        ["sel", -7, -3],
                        "SELECTED_FONT",
                        0,
                        "LIST_FONT",
                        1,
                        "SPACING",
                        15,
                        "LISTSIZE",
                        [7, 2],
                        "LIST_OFFSET",
                        [[0, -25], [0, 18]]
                    ],
                    "SET",
                    [
                        "POSITION",
                        [156, 45],
                        "BITMAP",
                        ["pick"],
                        "FONT",
                        0,
                        "TEXTOFFSET",
                        [10, 6]
                    ],
                    "ROLL",
                    [
                        "POSITION",
                        [145, 100],
                        "BITMAP",
                        ["bak"],
                        "RECT",
                        [[50, 10], [312, 215]],
                        "SCROLL_RATE",
                        0.02,
                        "SCROLLING_TEXT",
                        [[["TITLE", 20, 0, 0]], [["Name", 30, 200, 1]]]
                    ]
                ],
                "PRIMITIVES",
                [
                    "LIST_0",
                    ["POSITION", [230, 91], "FONT", 1],
                    "MESSAGE",
                    ["POSITION", [393, 230], "FONT", 1, "WORDWRAP", [500, 100]]
                ],
                "CURSOR",
                ["BITMAP", ["Cursor"]]
            ],
            "BOX",
            [
                "FONTS",
                [[0, "Face", 12, [7, 8, 9], 1, 500]],
                "BACKGROUND_IMAGES",
                [["msgbox", 380, 220]],
                "BUTTONS",
                ["OK", ["POSITION", [387, 248], "BITMAP", ["ok"]]]
            ]
        ]])
    }

    #[test]
    fn every_part_of_a_dialog_is_read() {
        let set = parse_dialogs(&file()).expect("the file parses");
        assert_eq!(set.shared_image_path.as_deref(), Some("shared"));
        assert_eq!(set.dialogs.len(), 2);

        let front = set.get("FRONT").expect("FRONT");
        assert_eq!(front.fonts.len(), 2);
        assert_eq!(
            front.font(1),
            Some(&DialogFont {
                id: 1,
                face: "Face".into(),
                size: 16,
                colour: [4, 5, 6],
                flag: 0,
                weight: 700,
            })
        );
        assert_eq!(front.backgrounds[0].pos, IVec2::new(0, -144));
        assert_eq!(front.texts[0].text, "MSG_HELLO");
        assert_eq!(front.cursor.as_deref(), Some("Cursor"));

        let go = front.control("GO").expect("GO");
        assert_eq!(go.kind, ControlKind::Button);
        assert_eq!(go.pos, IVec2::new(269, 86));
        assert_eq!(go.bitmap.as_ref().map(|b| b.name.as_str()), Some("Go"));
        assert_eq!(
            go.rollover.as_ref().and_then(|v| v.bitmap.as_deref()),
            Some("Go_H")
        );
        assert_eq!(go.disabled.as_deref(), Some("Go_no"));
        assert_eq!(
            go.sounds.rollover,
            Some(SoundCue {
                name: "snd_over".into(),
                volume: 0.5
            })
        );
        assert_eq!(
            go.sounds.activate.as_ref().map(|cue| cue.volume),
            Some(1.0),
            "a sound without a volume plays at full volume"
        );

        let tick = front.control("TICK").expect("TICK");
        assert!(matches!(&tick.kind, ControlKind::Checkbox(mark) if mark.text == "X"));

        let ControlKind::Cycle(states) = &front.control("LEVEL").expect("LEVEL").kind else {
            panic!("LEVEL is a cycle");
        };
        assert_eq!(states.len(), 2);
        assert_eq!(
            states[1].bitmap,
            Some(Bitmap {
                name: "pic".into(),
                offset: IVec2::new(5, -6)
            })
        );

        assert_eq!(
            front.control("VOLUME").map(|c| &c.kind),
            Some(&ControlKind::Slider {
                fill: "Full".into()
            })
        );

        let ControlKind::Radio(items) = &front.control("MODE").expect("MODE").kind else {
            panic!("MODE is a radio group");
        };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].labels.len(), 2, "a radio item's captions");
        assert_eq!(items[0].kind, ControlKind::Button);
        assert_eq!(
            items[0].activate.as_ref().and_then(|v| v.bitmap.as_deref()),
            Some("Radio_S")
        );

        let name = front.control("NAME").expect("NAME");
        assert_eq!(name.kind, ControlKind::Label);
        assert_eq!(name.labels[0].text, "");

        let keys = front.control("KEYS").expect("KEYS");
        assert!(matches!(&keys.kind, ControlKind::List(style)
            if style.size == [7, 2] && style.spacing == 15 && style.offsets.len() == 2));
        assert_eq!(
            keys.bitmap.as_ref().map(|b| b.offset),
            Some(IVec2::new(-7, -3))
        );

        assert_eq!(
            front.control("SET").map(|c| &c.kind),
            Some(&ControlKind::Text {
                font: 0,
                offset: IVec2::new(10, 6)
            })
        );

        let ControlKind::Scroller(roll) = &front.control("ROLL").expect("ROLL").kind else {
            panic!("ROLL scrolls");
        };
        assert_eq!(roll.rect, IRect::new(50, 10, 312, 215));
        assert_eq!(roll.lines.len(), 2);
        assert_eq!(roll.lines[1].gap, 200);

        assert_eq!(
            front.primitive("MESSAGE").and_then(|p| p.wordwrap),
            Some(UVec2::new(500, 100))
        );

        // One record where a list of records would be: still read.
        let box_dialog = set.get("BOX").expect("BOX");
        assert_eq!(box_dialog.backgrounds.len(), 1);
        assert_eq!(box_dialog.fonts.len(), 1);
    }

    #[test]
    fn the_bitmaps_and_keys_a_dialog_needs_are_listed_once() {
        let set = parse_dialogs(&file()).expect("the file parses");
        let front = set.get("FRONT").expect("FRONT");

        let bitmaps = front.bitmaps();
        for name in [
            "Strip", "Go", "Go_H", "Go_no", "box_H", "pic", "Empty", "Full", "Radio", "Radio_S",
            "sel", "Cursor",
        ] {
            assert!(bitmaps.contains(&name), "{name} is listed");
        }
        assert_eq!(bitmaps.iter().filter(|name| **name == "Radio").count(), 1);

        assert_eq!(
            front.message_keys(),
            vec!["MSG_A", "MSG_B", "MSG_HELLO", "MSG_LOW"]
        );
    }

    #[test]
    fn a_broken_control_names_itself() {
        let broken = json!([["D", ["BUTTONS", ["GO", ["BITMAP", ["x"]]]]]]);
        let error = parse_dialogs(&broken).expect_err("GO has no position");
        assert!(error.to_string().contains("GO"), "{error}");

        assert_eq!(
            parse_dialogs(&json!([[]])),
            Err(DialogParseError::Missing("dialog.zrd's dialogs"))
        );
    }

    #[test]
    fn briefings_are_ordered_by_level_and_find_their_parts() {
        let file = json!([[
            "CAMPAIGN10",
            [
                "BUTTONS",
                [
                    "TRANSPORT_PROGRESS",
                    [
                        "POSITION",
                        [133, 401],
                        "BITMAP",
                        ["Empty"],
                        "FILLBITMAP",
                        ["Full"]
                    ]
                ]
            ],
            "CAMPAIGN2",
            [
                "PRIMITIVES",
                [
                    "LOCATOR2",
                    ["POSITION", [3, 4]],
                    "LOCATOR1",
                    ["POSITION", [1, 2]],
                    "OBJECTIVE_PICT",
                    ["POSITION", [180, 70]]
                ]
            ]
        ]]);
        let layouts = parse_briefings(&file).expect("the file parses");
        assert_eq!(
            layouts.iter().map(|l| l.campaign).collect::<Vec<_>>(),
            vec![2, 10]
        );
        assert_eq!(
            layouts[0].locators(),
            vec![IVec2::new(1, 2), IVec2::new(3, 4)]
        );
        assert!(layouts[0].transport().is_none());
        assert!(matches!(
            layouts[1].transport().map(|c| &c.kind),
            Some(ControlKind::Slider { .. })
        ));
    }

    #[test]
    fn the_transport_words_come_from_the_hud_file() {
        let hud = json!([[
            "VERSION",
            8,
            "BRIEFING",
            [
                "NUMBER",
                [5, 29],
                "LOADING_TEXT",
                [
                    "Going",
                    "FONT",
                    ["Face", 18, 10, 500],
                    "COLOR",
                    [1, 2, 3],
                    "STAGES",
                    [4]
                ],
                "READY_TEXT",
                [
                    "Done",
                    "FONT",
                    ["Face", 18, 10, 500],
                    "COLOR",
                    [4, 5, 6],
                    "STAGES",
                    [4]
                ]
            ]
        ]]);
        assert_eq!(
            parse_transport(&hud),
            Ok(TransportText {
                loading: "Going".into(),
                loading_colour: [1, 2, 3],
                ready: "Done".into(),
                ready_colour: [4, 5, 6],
                size: 18,
                stages: 4,
            })
        );
        assert!(parse_transport(&json!([["VERSION", 8]])).is_err());
    }
}
