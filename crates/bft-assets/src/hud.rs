//! `hud.zrd` → [`HudLayout`]: where the original HUD puts everything.
//!
//! The layout is in 640×480 pixels, the resolution the HUD art was drawn
//! for. Nothing here scales or anchors it; that is the game crate's job. This module only reads the file, and every value the HUD
//! draws with comes out of it: no layout number from the file is repeated
//! in code (the copyright rule in `docs/CONVENTIONS.md`).
//!
//! # The file, section by section
//!
//! `VERSION 8`, then map-shaped sections (READER-SCHEMA §2.7a). Most are
//! positional records, and three shapes recur:
//!
//! - a **placed image** `[texture, x, y, transparent, centred]`, the two
//!   flags spelt `"TRUE"`/`"FALSE"`. The first is set exactly on the
//!   textures `image.zbd` stores with an alpha channel; the second only on
//!   `targ_ret`, whose position is the centre of the sensor view — so it
//!   places the image by its centre;
//! - a **rect** `[x0, y0, x1, y1]`, corners and not size: `SHIELD`'s fill
//!   rect is exactly the inner window of the `health` image;
//! - a **point** `[x, y]`.
//!
//! Positions inside `SENSOR`, `SHIELD` and `OBJECTIVE` are relative to that
//! section's frame image; the rest are screen positions.
//!
//! See [`parse_hud`] for which record means what, and docs/data/hud.md for
//! how each was checked.

use bevy::math::{URect, UVec2};
use serde_json::Value;

use crate::reader::ReaderData;
use crate::reader_value::ReaderValue;

/// The reader entry this module reads, in the global `zrdr.zbd`.
pub const HUD_ENTRY: &str = "hud.zrd";

/// The whole HUD layout, in 640×480 pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct HudLayout {
    /// `VERSION`; 8 in the retail data.
    pub version: u32,
    pub fonts: HudFonts,
    /// `TYPEI`: the 3D view's rectangle with no strips — the whole screen.
    pub viewport_full: URect,
    /// `TYPEII`: the 3D view letterboxed between the top and bottom strips.
    pub viewport_strip: URect,
    /// `TYPEII`'s strip images, drawn only in that mode.
    pub strips: Vec<Placed>,
    /// `TYPEII`'s wider variants of the strips (`tops800`, `iback1024`…),
    /// for screens wider than 640.
    pub wide_strips: Vec<String>,
    /// `RETICULE`: the reticle's three looks.
    pub reticle: Reticle,
    pub sensor: SensorLayout,
    pub shield: ShieldLayout,
    pub nanite: NaniteLayout,
    pub objective: ObjectiveLayout,
    pub stats: StatsLayout,
    pub target: TargetLayout,
    /// `WEAPON`: one row per slot, 1–9.
    pub weapons: [WeaponIcons; 9],
    /// `MODES`: tank, amphib, hover, sub, in file order.
    pub modes: [ModeIcons; 4],
}

impl HudLayout {
    /// Every texture the layout names, for checking the art is all there.
    pub fn textures(&self) -> Vec<&str> {
        let mut names: Vec<&str> = Vec::new();
        names.extend(self.strips.iter().map(|placed| placed.texture.as_str()));
        names.extend(self.wide_strips.iter().map(String::as_str));
        names.extend([
            self.reticle.range.as_str(),
            self.reticle.off.as_str(),
            self.reticle.on.as_str(),
        ]);
        names.push(&self.sensor.frame.texture);
        names.push(&self.sensor.reticle.texture);
        names.push(&self.shield.frame.texture);
        names.extend(
            self.nanite
                .slots
                .iter()
                .map(|placed| placed.texture.as_str()),
        );
        names.push(&self.objective.tab.texture);
        names.push(&self.target.lock_on);
        names.extend(self.target.threats.iter().map(String::as_str));
        for row in &self.weapons {
            names.extend(row.frames.iter().map(String::as_str));
            names.extend(row.minis.iter().map(String::as_str));
        }
        for mode in &self.modes {
            names.extend([mode.off.as_str(), mode.on.as_str(), mode.picked.as_str()]);
        }
        names.sort_unstable();
        names.dedup();
        names
    }
}

/// An image at a position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placed {
    /// An `image.zbd` texture name.
    pub texture: String,
    /// Top-left corner, or the centre when [`Self::centred`].
    pub pos: UVec2,
    /// The first flag: the texture has see-through pixels. It matches which
    /// textures `image.zbd` stores with alpha, one for one.
    pub transparent: bool,
    /// The second flag: `pos` is the image's centre.
    pub centred: bool,
}

/// `FONTS`: the five text styles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HudFonts {
    pub obj_summary: FontSpec,
    pub obj_description: FontSpec,
    pub messages: FontSpec,
    pub ammo: FontSpec,
    pub strings: FontSpec,
}

/// One `FONTS` entry: `[face, height, width, weight, flag]`, a Windows
/// `CreateFont` call's worth.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontSpec {
    /// A Windows system font (`Arial`, `Symbol`), not shipped.
    pub face: String,
    /// Cell height in 480-line pixels.
    pub height: u32,
    /// Average character width.
    pub width: u32,
    /// 400 regular, 700 bold.
    pub weight: u32,
    /// The fifth field, 0 or 1; unknown (antialiasing is the likeliest).
    pub flag: bool,
}

/// `RETICULE`'s three images.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reticle {
    /// First in the file: red triangles, "the reticle weapons will hit an
    /// object if fired" (manual p. 19) — something is in range.
    pub range: String,
    /// Second: the bare circle, nothing within range.
    pub off: String,
    /// Third: green triangles, the target can be damaged.
    pub on: String,
}

/// `SENSOR`: the zoomed target window, bottom left.
#[derive(Debug, Clone, PartialEq)]
pub struct SensorLayout {
    /// `monitor`, the frame. Everything below is relative to it.
    pub frame: Placed,
    /// How much closer than the main view the sensor looks.
    pub zoom: f32,
    /// Where the view shows through the frame (offset and size in the file).
    pub view: URect,
    /// The fifth field, 400; a range in metres by its size (unused).
    pub range: f32,
    /// `targ_ret`, the cross-hair over the view's centre.
    pub reticle: Placed,
    /// The targeted enemy's health bar.
    pub enemy_bar: URect,
}

/// `SHIELD`: the player's health, bottom right.
#[derive(Debug, Clone, PartialEq)]
pub struct ShieldLayout {
    /// `health`, the frame. Everything below is relative to it.
    pub frame: Placed,
    /// `[x, y, digits, centred]`: the percentage above the bar.
    pub readout: Readout,
    /// The yellow bar.
    pub fill: URect,
}

/// Where a number goes: `SHIELD`'s `[x, y, digits, flag]` record.
///
/// Read as x, y, how many digits, centred across the frame — the number
/// sits in the small window at the top of `health`, which spans the frame's
/// width. Inferred from the art; nothing else in the file has this shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Readout {
    pub pos: UVec2,
    pub digits: u32,
    pub centred: bool,
}

/// `NANITE`: the Nano-Canisters held (manual p. 40: up to three).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NaniteLayout {
    /// The rectangle the slots cover.
    pub rect: URect,
    /// One `hnanite` per canister, in file order (right to left).
    pub slots: Vec<Placed>,
}

/// `OBJECTIVE`: the message tab and the bar that grows out of it.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectiveLayout {
    /// Seconds for the bar to open or close.
    pub slide_time: f32,
    /// `objectab`, relative to the sensor frame (it sits in the monitor's
    /// dark top-right corner).
    pub tab: Placed,
    /// The bar, relative to the tab.
    pub panel: URect,
    /// The bar's colour, 8-bit RGB.
    pub colour: [u8; 3],
    /// Where the objective's summary line starts, in the bar.
    pub summary_at: UVec2,
    /// Where its description starts.
    pub description_at: UVec2,
    /// The slot inside `objectab` that fills when a transmission arrives.
    pub tab_fill: URect,
    /// The last record, two numbers: unknown. Read as the text's left margin
    /// and wrap width, which fits the bar.
    pub text_box: UVec2,
}

/// `STATS`: the numbers in the top strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatsLayout {
    /// The score.
    pub score: UVec2,
    /// The mission clock.
    pub clock: UVec2,
    /// The six records after those two, unread: by their positions, the
    /// multiplayer scoreboard.
    pub rest: Vec<Vec<i64>>,
}

/// `TARGET`: lock-on and threat markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetLayout {
    pub lock_on: String,
    /// Left, right, up, down.
    pub threats: [String; 4],
}

/// One `WEAPON` row.
///
/// The five frames, as the art shows them (and the manual's "a green border
/// around an icon shows that the weapon is selected"): the first variant,
/// the second, greyed out, the first selected, the second selected. The
/// minis are the small icons "near the lower-right corner" saying the key
/// has another weapon: the first variant's, then the second's. Row 7 spells
/// it out — `ic_sonc*` frames for the sonic wave, `ic_arc*` for the arc
/// sabre, and `icm_g2` (orange, sonic) before `icm_g1` (blue, arc).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WeaponIcons {
    pub frames: [String; 5],
    pub minis: [String; 2],
    /// The icon's top-left, relative to the icon strip.
    pub icon: UVec2,
    /// Where the ammo count goes, relative to the icon strip.
    pub ammo: UVec2,
}

impl WeaponIcons {
    /// The frame for a variant (`strong` = the second), selected or not.
    pub fn frame(&self, strong: bool, selected: bool) -> &str {
        let index = match (strong, selected) {
            (false, false) => 0,
            (true, false) => 1,
            (false, true) => 3,
            (true, true) => 4,
        };
        &self.frames[index]
    }

    /// The greyed-out frame: carried, but nothing to fire.
    pub fn empty_frame(&self) -> &str {
        &self.frames[2]
    }

    /// The mini icon for a variant.
    pub fn mini(&self, strong: bool) -> &str {
        &self.minis[usize::from(strong)]
    }
}

/// One `MODES` row: a form's three looks and where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModeIcons {
    /// Not collected: an empty panel.
    pub off: String,
    /// Collected: the form's picture, a dim light.
    pub on: String,
    /// The form the BFT is in: the light lit.
    pub picked: String,
    /// Top-left, on screen.
    pub pos: UVec2,
}

/// What went wrong reading `hud.zrd`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum HudParseError {
    #[error("{HUD_ENTRY}: no {0} section")]
    Missing(&'static str),
    #[error("{HUD_ENTRY}: {section}: {detail}")]
    Malformed {
        section: &'static str,
        detail: String,
    },
}

/// Read `hud.zrd` out of the global reader archive.
pub fn hud_layout(reader: &ReaderData) -> Result<HudLayout, HudParseError> {
    let value = reader
        .entry(HUD_ENTRY)
        .ok_or(HudParseError::Missing("hud.zrd entry"))?;
    parse_hud(value)
}

/// The reader entry that lays out the menus, and names their pointer.
pub const DIALOG_ENTRY: &str = "dialog.zrd";

/// The menus' mouse pointer: `dialog.zrd`'s `CURSOR` → `BITMAP` name,
/// from the first dialog that gives one (every shipped dialog names the
/// same image), as a texture key. `None` when the entry or the key is
/// missing.
pub fn menu_pointer(reader: &ReaderData) -> Option<String> {
    let dialogs = reader.read(DIALOG_ENTRY)?;
    dialogs.pairs().into_iter().find_map(|(_, dialog)| {
        let name = dialog.get("CURSOR")?.get("BITMAP")?.string()?;
        Some(crate::textures::texture_key(name))
    })
}

/// Parse `hud.zrd`'s JSON, as the extraction writes it.
pub fn parse_hud(zrd: &Value) -> Result<HudLayout, HudParseError> {
    let root = ReaderValue::root(zrd);
    let section = |name: &'static str| root.get(name).ok_or(HudParseError::Missing(name));

    let version = section("VERSION")?
        .int()
        .and_then(|v| u32::try_from(v).ok())
        .ok_or_else(|| malformed("VERSION", "not a number"))?;

    let fonts = section("FONTS")?;
    let font = |name: &'static str| -> Result<FontSpec, HudParseError> {
        let spec = fonts.get(name).ok_or_else(|| malformed("FONTS", name))?;
        let number = |index: usize| -> Result<u32, HudParseError> {
            spec.nth(index)
                .and_then(|v| v.int())
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| malformed("FONTS", format!("{name}: field {index}")))
        };
        Ok(FontSpec {
            face: spec
                .nth(0)
                .and_then(|v| v.string())
                .ok_or_else(|| malformed("FONTS", format!("{name}: no face")))?
                .to_string(),
            height: number(1)?,
            width: number(2)?,
            weight: number(3)?,
            flag: spec.nth(4).and_then(|v| v.flag()).unwrap_or(false),
        })
    };
    let fonts = HudFonts {
        obj_summary: font("OBJ_SUMMARY")?,
        obj_description: font("OBJ_DESCRIPTION")?,
        messages: font("MESSAGES")?,
        ammo: font("AMMO")?,
        strings: font("STRINGS")?,
    };

    // `TYPEI` holds its one rect wrapped in a list: `[[0, 0, 640, 480]]`.
    let full = section("TYPEI")?;
    let viewport_full = full
        .nth(0)
        .filter(|first| first.len() == 4)
        .map_or_else(|| rect(&full, "TYPEI"), |first| rect(&first, "TYPEI"))?;

    let strip = section("TYPEII")?.items();
    let viewport_strip = rect(
        strip.first().ok_or_else(|| malformed("TYPEII", "empty"))?,
        "TYPEII",
    )?;
    let mut strips = Vec::new();
    let mut wide_strips = Vec::new();
    for item in &strip[1..] {
        // A placed image has a number second; the variants list is all
        // names.
        if item.nth(1).and_then(|v| v.int()).is_some() {
            strips.push(placed(item, "TYPEII")?);
        } else {
            wide_strips.extend(item.strings().into_iter().map(str::to_string));
        }
    }

    let reticle = names::<3>(section("RETICULE")?, "RETICULE")?;
    let [range, off, on] = reticle;
    let reticle = Reticle { range, off, on };

    let sensor = section("SENSOR")?.items();
    let offset = point(at(&sensor, 2, "SENSOR")?, "SENSOR")?;
    let size = point(at(&sensor, 3, "SENSOR")?, "SENSOR")?;
    let sensor = SensorLayout {
        frame: placed(&at(&sensor, 0, "SENSOR")?, "SENSOR")?,
        zoom: number(at(&sensor, 1, "SENSOR")?, "SENSOR")?,
        view: URect::from_corners(offset, offset + size),
        range: number(at(&sensor, 4, "SENSOR")?, "SENSOR")?,
        reticle: placed(&at(&sensor, 5, "SENSOR")?, "SENSOR")?,
        enemy_bar: rect(&at(&sensor, 6, "SENSOR")?, "SENSOR")?,
    };

    let shield = section("SHIELD")?.items();
    let readout = at(&shield, 1, "SHIELD")?;
    let readout_numbers = readout
        .items()
        .iter()
        .take(3)
        .map(|v| v.int().and_then(|v| u32::try_from(v).ok()))
        .collect::<Option<Vec<u32>>>()
        .filter(|numbers| numbers.len() == 3)
        .ok_or_else(|| malformed("SHIELD", "readout is not [x, y, digits, flag]"))?;
    let shield = ShieldLayout {
        frame: placed(&at(&shield, 0, "SHIELD")?, "SHIELD")?,
        readout: Readout {
            pos: UVec2::new(readout_numbers[0], readout_numbers[1]),
            digits: readout_numbers[2],
            centred: readout.nth(3).is_some_and(|v| truth(&v)),
        },
        fill: rect(&at(&shield, 2, "SHIELD")?, "SHIELD")?,
    };

    let nanite = section("NANITE")?.items();
    let nanite = NaniteLayout {
        rect: rect(&at(&nanite, 0, "NANITE")?, "NANITE")?,
        slots: nanite[1..]
            .iter()
            .map(|item| placed(item, "NANITE"))
            .collect::<Result<_, _>>()?,
    };

    let objective = section("OBJECTIVE")?.items();
    let colour = at(&objective, 3, "OBJECTIVE")?
        .items()
        .iter()
        .map(|v| v.int().and_then(|v| u8::try_from(v).ok()))
        .collect::<Option<Vec<u8>>>()
        .and_then(|rgb| <[u8; 3]>::try_from(rgb).ok())
        .ok_or_else(|| malformed("OBJECTIVE", "colour is not [r, g, b]"))?;
    let objective = ObjectiveLayout {
        slide_time: number(at(&objective, 0, "OBJECTIVE")?, "OBJECTIVE")?,
        tab: placed(&at(&objective, 1, "OBJECTIVE")?, "OBJECTIVE")?,
        panel: rect(&at(&objective, 2, "OBJECTIVE")?, "OBJECTIVE")?,
        colour,
        summary_at: point(at(&objective, 4, "OBJECTIVE")?, "OBJECTIVE")?,
        description_at: point(at(&objective, 5, "OBJECTIVE")?, "OBJECTIVE")?,
        tab_fill: rect(&at(&objective, 6, "OBJECTIVE")?, "OBJECTIVE")?,
        text_box: point(at(&objective, 7, "OBJECTIVE")?, "OBJECTIVE")?,
    };

    let stats = section("STATS")?.items();
    let stats = StatsLayout {
        score: point(at(&stats, 0, "STATS")?, "STATS")?,
        clock: point(at(&stats, 1, "STATS")?, "STATS")?,
        rest: stats[2..]
            .iter()
            .map(|item| item.items().iter().filter_map(|v| v.int()).collect())
            .collect(),
    };

    let [lock_on, left, right, up, down] = names::<5>(section("TARGET")?, "TARGET")?;
    let target = TargetLayout {
        lock_on,
        threats: [left, right, up, down],
    };

    let weapon_rows = section("WEAPON")?.items();
    if weapon_rows.len() != 9 {
        return Err(malformed(
            "WEAPON",
            format!("{} rows, not 9", weapon_rows.len()),
        ));
    }
    let mut weapons = Vec::with_capacity(9);
    for row in &weapon_rows {
        let items = row.items();
        if items.len() != 11 {
            return Err(malformed("WEAPON", format!("a row of {}", items.len())));
        }
        let name = |index: usize| -> Result<String, HudParseError> {
            items[index]
                .string()
                .map(str::to_string)
                .ok_or_else(|| malformed("WEAPON", format!("field {index} is not a name")))
        };
        let coordinate = |index: usize| -> Result<u32, HudParseError> {
            items[index]
                .int()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| malformed("WEAPON", format!("field {index} is not a position")))
        };
        weapons.push(WeaponIcons {
            frames: [name(0)?, name(1)?, name(2)?, name(3)?, name(4)?],
            minis: [name(5)?, name(6)?],
            icon: UVec2::new(coordinate(7)?, coordinate(8)?),
            ammo: UVec2::new(coordinate(9)?, coordinate(10)?),
        });
    }
    let weapons =
        <[WeaponIcons; 9]>::try_from(weapons).map_err(|_| malformed("WEAPON", "not nine rows"))?;

    let mode_rows = section("MODES")?.items();
    let modes = mode_rows
        .iter()
        .map(|row| {
            let [off, on, picked] = names::<3>(*row, "MODES")?;
            let x = row.nth(3).and_then(|v| v.int());
            let y = row.nth(4).and_then(|v| v.int());
            let (Some(x), Some(y)) = (x, y) else {
                return Err(malformed("MODES", "a row without a position"));
            };
            Ok(ModeIcons {
                off,
                on,
                picked,
                pos: UVec2::new(unsigned(x, "MODES")?, unsigned(y, "MODES")?),
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let modes = <[ModeIcons; 4]>::try_from(modes)
        .map_err(|rows| malformed("MODES", format!("{} rows, not 4", rows.len())))?;

    Ok(HudLayout {
        version,
        fonts,
        viewport_full,
        viewport_strip,
        strips,
        wide_strips,
        reticle,
        sensor,
        shield,
        nanite,
        objective,
        stats,
        target,
        weapons,
        modes,
    })
}

fn malformed(section: &'static str, detail: impl Into<String>) -> HudParseError {
    HudParseError::Malformed {
        section,
        detail: detail.into(),
    }
}

/// The `index`th record of a section.
fn at<'a>(
    items: &[ReaderValue<'a>],
    index: usize,
    section: &'static str,
) -> Result<ReaderValue<'a>, HudParseError> {
    items
        .get(index)
        .copied()
        .ok_or_else(|| malformed(section, format!("no record {index}")))
}

fn unsigned(value: i64, section: &'static str) -> Result<u32, HudParseError> {
    u32::try_from(value).map_err(|_| malformed(section, format!("{value} is negative")))
}

/// `"TRUE"`/`"FALSE"`, or a number.
fn truth(value: &ReaderValue<'_>) -> bool {
    match value.string() {
        Some(text) => text.eq_ignore_ascii_case("TRUE"),
        None => value.flag().unwrap_or(false),
    }
}

fn number(value: ReaderValue<'_>, section: &'static str) -> Result<f32, HudParseError> {
    value
        .float()
        .ok_or_else(|| malformed(section, "expected a number"))
}

fn point(value: ReaderValue<'_>, section: &'static str) -> Result<UVec2, HudParseError> {
    match value.items().as_slice() {
        [x, y] => {
            let (Some(x), Some(y)) = (x.int(), y.int()) else {
                return Err(malformed(section, "a point that is not two integers"));
            };
            Ok(UVec2::new(unsigned(x, section)?, unsigned(y, section)?))
        }
        _ => Err(malformed(section, "expected [x, y]")),
    }
}

fn rect(value: &ReaderValue<'_>, section: &'static str) -> Result<URect, HudParseError> {
    let numbers = value
        .items()
        .iter()
        .map(|v| v.int())
        .collect::<Option<Vec<i64>>>()
        .filter(|numbers| numbers.len() == 4)
        .ok_or_else(|| malformed(section, "expected [x0, y0, x1, y1]"))?;
    let [x0, y0, x1, y1] = [numbers[0], numbers[1], numbers[2], numbers[3]];
    Ok(URect::new(
        unsigned(x0, section)?,
        unsigned(y0, section)?,
        unsigned(x1, section)?,
        unsigned(y1, section)?,
    ))
}

fn placed(value: &ReaderValue<'_>, section: &'static str) -> Result<Placed, HudParseError> {
    let texture = value
        .nth(0)
        .and_then(|v| v.string())
        .ok_or_else(|| malformed(section, "a placed image without a texture"))?;
    let coordinate = |index: usize| {
        value
            .nth(index)
            .and_then(|v| v.int())
            .ok_or_else(|| malformed(section, format!("{texture}: no position")))
            .and_then(|v| unsigned(v, section))
    };
    Ok(Placed {
        texture: texture.to_string(),
        pos: UVec2::new(coordinate(1)?, coordinate(2)?),
        transparent: value.nth(3).is_some_and(|v| truth(&v)),
        centred: value.nth(4).is_some_and(|v| truth(&v)),
    })
}

/// The first `N` names of a list.
fn names<const N: usize>(
    value: ReaderValue<'_>,
    section: &'static str,
) -> Result<[String; N], HudParseError> {
    let names: Vec<String> = value
        .items()
        .iter()
        .take(N)
        .filter_map(|v| v.string().map(str::to_string))
        .collect();
    <[String; N]>::try_from(names)
        .map_err(|found| malformed(section, format!("{} names, not {N}", found.len())))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A layout of the file's shape with made-up numbers: every value here
    /// is invented, so the test says nothing about the real file (which
    /// `tests/real_hud.rs` checks against an install).
    fn synthetic() -> Value {
        let row = |stem: &str, x: u32| {
            serde_json::json!([
                format!("{stem}1"),
                format!("{stem}2"),
                format!("{stem}3"),
                format!("{stem}4"),
                format!("{stem}5"),
                format!("{stem}m1"),
                format!("{stem}m2"),
                x,
                30,
                x + 2,
                60
            ])
        };
        let weapons: Vec<Value> = (0..9).map(|i| row(&format!("w{i}_"), i * 40)).collect();
        serde_json::json!([[
            "VERSION",
            [8],
            "FONTS",
            [
                "OBJ_SUMMARY",
                ["Face", 12, 8, 700, 1],
                "OBJ_DESCRIPTION",
                ["Face", 10, 7, 400, 0],
                "MESSAGES",
                ["Face", 14, 7, 600, 1],
                "AMMO",
                ["Other", 12, 6, 500, 1],
                "STRINGS",
                ["Face", 11, 6, 500, 0]
            ],
            "TYPEI",
            [[0, 0, 640, 480]],
            "TYPEII",
            [
                [0, 20, 640, 400],
                ["top", 0, 0, "FALSE", "FALSE"],
                ["mid", 100, 400, "TRUE", "FALSE"],
                ["top_wide", "mid_wide"]
            ],
            "RETICULE",
            ["r_range", "r_off", "r_on"],
            "SENSOR",
            [
                ["frame", 0, 350, "TRUE", "FALSE"],
                3.0,
                [8, 6],
                [90, 80],
                300,
                ["cross", 50, 45, "TRUE", "TRUE"],
                [110, 60, 115, 100]
            ],
            "SHIELD",
            [
                ["bar", 500, 400, "FALSE", "FALSE"],
                [1, 6, 3, "TRUE"],
                [8, 20, 26, 70]
            ],
            "NANITE",
            [
                [300, 12, 340, 22],
                ["can", 330, 12, "TRUE", "FALSE"],
                ["can", 315, 12, "TRUE", "FALSE"]
            ],
            "OBJECTIVE",
            [
                0.25,
                ["tab", 110, 0, "FALSE", "FALSE"],
                [0, 0, 300, 50],
                [10, 20, 30],
                [2, 1],
                [2, 15],
                [4, 5, 9, 40],
                [2, 300]
            ],
            "STATS",
            [[300, 4], [500, 4], [1, 2, 3], [4, 5]],
            "TARGET",
            ["lock", "t_l", "t_r", "t_u", "t_d"],
            "WEAPON",
            weapons,
            "MODES",
            [
                ["a_off", "a_on", "a_pik", 540, 400],
                ["b_off", "b_on", "b_pik", 580, 400],
                ["c_off", "c_on", "c_pik", 540, 440],
                ["d_off", "d_on", "d_pik", 580, 440]
            ],
            "BRIEFING",
            ["NUMBER", [1, 2]]
        ]])
    }

    #[test]
    fn a_synthetic_layout_parses_section_by_section() {
        let layout = parse_hud(&synthetic()).expect("the synthetic layout parses");

        assert_eq!(layout.version, 8);
        assert_eq!(layout.fonts.ammo.face, "Other");
        assert_eq!(layout.fonts.messages.height, 14);
        assert!(layout.fonts.obj_summary.flag);
        assert_eq!(layout.viewport_full, URect::new(0, 0, 640, 480));
        assert_eq!(layout.viewport_strip, URect::new(0, 20, 640, 400));
        assert_eq!(layout.strips.len(), 2);
        assert_eq!(layout.strips[1].pos, UVec2::new(100, 400));
        assert!(layout.strips[1].transparent);
        assert_eq!(layout.wide_strips, vec!["top_wide", "mid_wide"]);
        assert_eq!(layout.reticle.range, "r_range");
        assert_eq!(layout.reticle.on, "r_on");

        assert_eq!(layout.sensor.zoom, 3.0);
        assert_eq!(layout.sensor.view, URect::new(8, 6, 98, 86));
        assert!(layout.sensor.reticle.centred);
        assert_eq!(layout.sensor.enemy_bar, URect::new(110, 60, 115, 100));

        assert_eq!(layout.shield.readout.pos, UVec2::new(1, 6));
        assert_eq!(layout.shield.readout.digits, 3);
        assert!(layout.shield.readout.centred);
        assert_eq!(layout.shield.fill, URect::new(8, 20, 26, 70));

        assert_eq!(layout.nanite.slots.len(), 2);
        assert_eq!(layout.objective.slide_time, 0.25);
        assert_eq!(layout.objective.colour, [10, 20, 30]);
        assert_eq!(layout.objective.tab_fill, URect::new(4, 5, 9, 40));
        assert_eq!(layout.stats.clock, UVec2::new(500, 4));
        assert_eq!(layout.stats.rest, vec![vec![1, 2, 3], vec![4, 5]]);
        assert_eq!(layout.target.threats[3], "t_d");

        assert_eq!(layout.weapons[3].icon, UVec2::new(120, 30));
        assert_eq!(layout.weapons[3].frame(true, true), "w3_5");
        assert_eq!(layout.weapons[3].frame(false, false), "w3_1");
        assert_eq!(layout.weapons[3].empty_frame(), "w3_3");
        assert_eq!(layout.weapons[3].mini(true), "w3_m2");
        assert_eq!(layout.modes[2].picked, "c_pik");
        assert_eq!(layout.modes[3].pos, UVec2::new(580, 440));

        let textures = layout.textures();
        assert!(textures.contains(&"frame") && textures.contains(&"mid_wide"));
        assert_eq!(
            textures.iter().filter(|name| **name == "can").count(),
            1,
            "deduplicated"
        );
    }

    #[test]
    fn the_menu_pointer_is_read_from_the_dialogs() {
        let dialog = serde_json::json!([[
            "FIRST",
            ["IMAGE_PATH", ["x"]],
            "SECOND",
            ["BUTTONS", [], "CURSOR", ["BITMAP", ["Arrow"]]],
            "THIRD",
            ["CURSOR", ["BITMAP", ["Other"]]]
        ]]);
        let reader =
            ReaderData::from_entries([(DIALOG_ENTRY.to_string(), dialog)].into_iter().collect());
        assert_eq!(menu_pointer(&reader).as_deref(), Some("arrow"));
        assert_eq!(menu_pointer(&ReaderData::default()), None);
    }

    #[test]
    fn a_missing_section_is_named() {
        let mut value = synthetic();
        let list = value[0].as_array_mut().expect("a list");
        let at = list
            .iter()
            .position(|v| v == "SENSOR")
            .expect("the section is there");
        list.drain(at..at + 2);

        assert_eq!(parse_hud(&value), Err(HudParseError::Missing("SENSOR")));
    }

    #[test]
    fn a_short_weapon_table_is_refused() {
        let mut value = synthetic();
        let list = value[0].as_array_mut().expect("a list");
        let at = list
            .iter()
            .position(|v| v == "WEAPON")
            .expect("the section is there");
        list[at + 1].as_array_mut().expect("rows").pop();

        assert!(matches!(
            parse_hud(&value),
            Err(HudParseError::Malformed {
                section: "WEAPON",
                ..
            })
        ));
    }
}
