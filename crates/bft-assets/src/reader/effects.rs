//! `effects.zrd`: the sprite effects' texture cycles.
//!
//! Thirteen records, each a model and the frames its texture steps through
//! (READER-SCHEMA §2.7):
//!
//! ```text
//! ["fire1.flt", "NAME", ["fire1"], "SPEED", [10.0], "LOOPING", ["ON"],
//!  "MAPS", ["fire101.tif", "fire102.tif", …, "fire112.tif"]]
//! ```
//!
//! The model first, then keywords each followed by a value. `LOOPING` is
//! present only on `fire1`; the rest play once and hold their last frame.
//!
//! # A cycle belongs to the texture, not the model
//!
//! Interp's `CycleTextureSet*` bindings name a node. These do not: a
//! model's material shows the first frame, and the cycle is what that
//! texture does wherever it is drawn. `fire_hulk.flt` and `burning_model`,
//! the burning wrecks, are not `fire1.flt` but show `fire101.tif`, and
//! `smoke_destroy.flt` shows `bsmok1.tif`, `smoke1`'s first frame; the
//! anim scripts rewind them with `ObjectCycleTexture` and expect them to
//! run. So [`EffectCycles::for_texture`] looks a cycle up by texture.

use crate::reader::ReaderData;
use crate::reader_value::ReaderValue;
use crate::textures::texture_key;

/// The reader entry this module reads.
pub const EFFECTS_ENTRY: &str = "effects.zrd";

/// Every sprite cycle `effects.zrd` defines.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectCycles {
    /// In file order.
    pub cycles: Vec<EffectCycle>,
}

/// One `EFFECTS` record.
#[derive(Debug, Clone, PartialEq)]
pub struct EffectCycle {
    /// `NAME`: what anim `Effect` events and weapon `IMPACT/EFFECT` keys call
    /// it (`"fire1"`, `"smoke1"`, `"exp_box"`).
    pub name: String,
    /// The model the record opens with (`"fire1.flt"`).
    pub model: String,
    /// `SPEED`: frames per second.
    pub speed: f32,
    /// `LOOPING ON`: wraps rather than holding the last frame.
    pub looping: bool,
    /// `MAPS`: the frames, as texture names.
    pub maps: Vec<String>,
}

impl EffectCycles {
    /// The cycle a material showing this texture runs, if any.
    ///
    /// Keyed by the **first** frame, the one a model's material is authored
    /// with. Two first frames repeat: `bftmbrst` and `exp_box` both open on
    /// `boxexp01.tif`, and the two splashes are the same list under two
    /// names. The first record wins, in file order.
    ///
    /// Both sides go through [`texture_key`], so `fire101.tif`, `FIRE101`
    /// and a material's already-normalised `fire101` all match.
    pub fn for_texture(&self, texture: &str) -> Option<&EffectCycle> {
        let wanted = texture_key(texture);
        self.cycles.iter().find(|cycle| {
            cycle
                .maps
                .first()
                .is_some_and(|first| texture_key(first) == wanted)
        })
    }

    /// A cycle by its `NAME`.
    pub fn by_name(&self, name: &str) -> Option<&EffectCycle> {
        self.cycles.iter().find(|cycle| cycle.name == name)
    }

    pub fn len(&self) -> usize {
        self.cycles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cycles.is_empty()
    }
}

/// Read `effects.zrd` from the global archive.
///
/// `None` when the archive has no such entry. A record with no `NAME` or no
/// `MAPS` is skipped: it names nothing a caller could look up.
pub fn parse_effect_cycles(reader: &ReaderData) -> Option<EffectCycles> {
    let root = reader.read(EFFECTS_ENTRY)?;
    let records = root.get("EFFECTS")?;

    let cycles = records.items().iter().filter_map(parse_record).collect();

    Some(EffectCycles { cycles })
}

/// One positional record: the model, then keyword/value pairs.
fn parse_record(record: &ReaderValue<'_>) -> Option<EffectCycle> {
    let items = record.items();
    let (model, rest) = items.split_first()?;

    let mut name = None;
    let mut speed = 0.0;
    let mut looping = false;
    let mut maps = Vec::new();

    for pair in rest.chunks(2) {
        let [key, value] = pair else {
            continue;
        };
        match key.string() {
            Some("NAME") => name = value.string().map(str::to_string),
            Some("SPEED") => speed = value.float().unwrap_or(0.0),
            Some("LOOPING") => looping = value.string() == Some("ON"),
            Some("MAPS") => maps = value.strings().into_iter().map(str::to_string).collect(),
            _ => {}
        }
    }

    if maps.is_empty() {
        return None;
    }

    Some(EffectCycle {
        name: name?,
        model: model.string().unwrap_or_default().to_string(),
        speed,
        looping,
        maps,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde_json::json;

    use super::*;

    fn archive(effects: serde_json::Value) -> ReaderData {
        ReaderData::from_entries(BTreeMap::from([(EFFECTS_ENTRY.to_string(), effects)]))
    }

    /// The shape the shipped file has: wrapped twice, records positional.
    fn shipped_shape() -> ReaderData {
        archive(json!([[
            "EFFECTS",
            [
                [
                    "smoke1.flt",
                    "NAME",
                    ["smoke1"],
                    "SPEED",
                    [3.0],
                    "MAPS",
                    ["bsmok1.tif", "bsmok2.tif", "bsmok3.tif"]
                ],
                [
                    "fire1.flt",
                    "NAME",
                    ["fire1"],
                    "SPEED",
                    [10.0],
                    "LOOPING",
                    ["ON"],
                    "MAPS",
                    ["fire101.tif", "fire102.tif"]
                ],
                [
                    "bftmbrst.flt",
                    "NAME",
                    ["bftmbrst"],
                    "SPEED",
                    [15.0],
                    "MAPS",
                    ["boxexp01.tif", "wsmok1.tif"]
                ],
                [
                    "boxexp.flt",
                    "NAME",
                    ["exp_box"],
                    "SPEED",
                    [7.8],
                    "MAPS",
                    ["boxexp01.tif", "boxexp02.tif"]
                ]
            ]
        ]]))
    }

    #[test]
    fn every_record_reads_with_its_keywords() {
        let cycles = parse_effect_cycles(&shipped_shape()).expect("the entry parses");

        assert_eq!(cycles.len(), 4);
        let smoke = cycles.by_name("smoke1").expect("smoke1");
        assert_eq!(smoke.model, "smoke1.flt");
        assert_eq!(smoke.speed, 3.0);
        assert!(!smoke.looping);
        assert_eq!(smoke.maps, ["bsmok1.tif", "bsmok2.tif", "bsmok3.tif"]);

        let fire = cycles.by_name("fire1").expect("fire1");
        assert!(fire.looping, "fire1 is the one with LOOPING ON");
        assert_eq!(fire.speed, 10.0);
    }

    #[test]
    fn a_texture_finds_the_cycle_it_opens() {
        let cycles = parse_effect_cycles(&shipped_shape()).expect("the entry parses");

        assert_eq!(
            cycles.for_texture("fire101.tif").map(|c| c.name.as_str()),
            Some("fire1")
        );
        assert_eq!(
            cycles.for_texture("BSMOK1.TIF").map(|c| c.name.as_str()),
            Some("smoke1"),
            "texture names are matched without case"
        );
        assert_eq!(
            cycles.for_texture("bsmok1").map(|c| c.name.as_str()),
            Some("smoke1"),
            "or extension, the way a material names its texture"
        );
        assert!(
            cycles.for_texture("bsmok2.tif").is_none(),
            "a later frame is not where a material starts"
        );
        assert_eq!(
            cycles.for_texture("boxexp01.tif").map(|c| c.name.as_str()),
            Some("bftmbrst"),
            "a repeated first frame goes to the first record"
        );
    }

    #[test]
    fn a_missing_entry_is_none() {
        assert!(parse_effect_cycles(&ReaderData::default()).is_none());
    }
}
