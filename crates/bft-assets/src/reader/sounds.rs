//! `sounds.zrd`: the table from logical sound names to WAV files.
//!
//! Everything in the game names a sound logically — `snd_ricsizzle1` in
//! `weapons.zrd`, `snd_exp_hit1` in an anim `Sound` event — and this table
//! turns the name into a file in the sound archive plus how to play it
//! (READER-SCHEMA §2.5, docs/data/sounds.md).
//!
//! # The record, as the shipped file writes it
//!
//! `SETS` is a map of 16 set names (`COMMON`, `DIALOG`, `BRIEFING1`…,
//! `M1`…`M6`, `M9`, `M10`) to lists of positional records:
//!
//! ```text
//! ["snd_glassbreak", "glassbreak.wav", "3D", "PURGEABLE", "RANGE", [60.0, 180.0],
//!  "HIGH", [22050, 16, 1], "MED", [22050, 8, 1], "LOW", [11025, 8, 1]]
//! ```
//!
//! The logical name, the file, then **keywords, some followed by a value**:
//! `RANGE [min, max]`, `VOLUME f` and each of `HIGH`/`MED`/`LOW` with its own
//! format triple. READER-SCHEMA §2.5 printed the tiers as three words
//! followed by three triples; the file interleaves them, which is why this
//! parser walks keywords rather than counting positions. The bare keywords
//! are `3D`, `LOOPED`, `PURGEABLE`, `VOICE`, `FREQUENCY` and `HARDWARE`, in
//! no fixed order.

use std::collections::HashMap;

use crate::reader::ReaderData;
use crate::reader_value::ReaderValue;

/// The reader entry this module reads.
pub const SOUNDS_ENTRY: &str = "sounds.zrd";

/// Every logical sound the game names.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SoundTable {
    /// Keyed by logical name (`"snd_glassbreak"`).
    pub sounds: HashMap<String, SoundDef>,
}

impl SoundTable {
    /// One sound's definition.
    pub fn get(&self, name: &str) -> Option<&SoundDef> {
        self.sounds.get(name)
    }

    /// How many logical names the table holds.
    pub fn len(&self) -> usize {
        self.sounds.len()
    }

    /// Whether the table is empty, which a parsed shipped file never is.
    pub fn is_empty(&self) -> bool {
        self.sounds.is_empty()
    }
}

/// One `SETS` record.
#[derive(Debug, Clone, PartialEq)]
pub struct SoundDef {
    /// The logical name.
    pub name: String,
    /// The WAV's stem in the sound archive, lower-cased (`"glassbreak"`) —
    /// the key [`SoundBank`](crate::sounds) uses.
    pub file: String,
    /// The first set that lists it. Six names appear in several mission
    /// sets, always with the same file.
    pub set: String,
    /// `3D`: positional. The rest play at the listener.
    pub positional: bool,
    /// `LOOPED`: plays until stopped.
    pub looped: bool,
    /// `RANGE [min, max]`: full volume inside `min`, silent beyond `max`.
    /// Always `min < max`: the one record written the other way round
    /// (`snd_gunside`, `[50, 20]`) is put in order.
    pub range: Option<(f32, f32)>,
    /// `VOLUME f`: a per-sound gain, 1 where absent.
    pub volume: f32,
    /// `FREQUENCY`: the engine varies this sound's playback rate. Every one
    /// is a looped machine noise (`snd_ls_on`, the engines).
    pub frequency: bool,
    /// `VOICE`: speech — the 40 mission lines.
    pub voice: bool,
    /// `PURGEABLE`: the original could unload it after use.
    pub purgeable: bool,
    /// `HARDWARE`: asked for a hardware voice.
    pub hardware: bool,
}

/// Why `sounds.zrd` would not parse.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SoundTableError {
    #[error("the global reader has no {SOUNDS_ENTRY}")]
    NoEntry,
    #[error("{SOUNDS_ENTRY}: no SETS key")]
    NoSets,
    #[error("{SOUNDS_ENTRY}: SETS/{set} record {index} has no name and file")]
    NoName { set: String, index: usize },
    #[error("{SOUNDS_ENTRY}: SETS/{set}/{name}: {keyword} has no value")]
    MissingValue {
        set: String,
        name: String,
        keyword: String,
    },
    #[error("{SOUNDS_ENTRY}: SETS/{set}/{name}: unknown keyword {keyword:?}")]
    UnknownKeyword {
        set: String,
        name: String,
        keyword: String,
    },
}

/// Build the table from the global reader archive.
pub fn parse(global: &ReaderData) -> Result<SoundTable, SoundTableError> {
    let root = global.read(SOUNDS_ENTRY).ok_or(SoundTableError::NoEntry)?;
    let sets = root.get("SETS").ok_or(SoundTableError::NoSets)?;

    let mut table = SoundTable::default();
    for (set, records) in sets.pairs() {
        for (index, record) in set_records(records).into_iter().enumerate() {
            let def = parse_record(set, index, record)?;
            // First wins. The six repeats name the same file in every set
            // (asserted against the real file), so which one is kept only
            // changes `set`.
            table.sounds.entry(def.name.clone()).or_insert(def);
        }
    }

    Ok(table)
}

/// A set's records, allowing for mech3ax's one-element-list collapse.
///
/// A set holding a single record could arrive as that record rather than a
/// list of one. The shipped file wraps even the one-record sets, but a record
/// is recognisable — it starts with a string — so both are accepted.
fn set_records(value: ReaderValue<'_>) -> Vec<ReaderValue<'_>> {
    if value.nth(0).is_some_and(|first| first.raw().is_string()) {
        vec![value]
    } else {
        value.items()
    }
}

fn parse_record(
    set: &str,
    index: usize,
    record: ReaderValue<'_>,
) -> Result<SoundDef, SoundTableError> {
    let (Some(name), Some(file)) = (
        record.nth(0).and_then(|v| v.raw().as_str()),
        record.nth(1).and_then(|v| v.raw().as_str()),
    ) else {
        return Err(SoundTableError::NoName {
            set: set.to_string(),
            index,
        });
    };

    let mut def = SoundDef {
        name: name.to_string(),
        file: wav_stem(file),
        set: set.to_string(),
        positional: false,
        looped: false,
        range: None,
        volume: 1.0,
        frequency: false,
        voice: false,
        purgeable: false,
        hardware: false,
    };

    let missing = |keyword: &str| SoundTableError::MissingValue {
        set: set.to_string(),
        name: name.to_string(),
        keyword: keyword.to_string(),
    };

    let items = record.items();
    let mut rest = items.iter().skip(2);
    while let Some(item) = rest.next() {
        let Some(keyword) = item.raw().as_str() else {
            // A value with no keyword before it: nothing in the shipped file
            // does this, and skipping it keeps the keyword walk aligned.
            continue;
        };
        match keyword {
            "3D" => def.positional = true,
            "LOOPED" => def.looped = true,
            "FREQUENCY" => def.frequency = true,
            "VOICE" => def.voice = true,
            "PURGEABLE" => def.purgeable = true,
            "HARDWARE" => def.hardware = true,
            "RANGE" => {
                let [a, b] = rest
                    .next()
                    .and_then(|v| v.float_array::<2>())
                    .ok_or_else(|| missing(keyword))?;
                // One of the 139 ranges in the shipped file is reversed;
                // read as an authoring slip rather than an inverted falloff.
                def.range = Some((a.min(b), a.max(b)));
            }
            "VOLUME" => {
                def.volume = rest
                    .next()
                    .and_then(|v| v.float())
                    .ok_or_else(|| missing(keyword))?;
            }
            // The format triple of each quality tier. Only the high tier is
            // extracted (docs/data/sounds.md), and its WAV header is the truth
            // about its format, so the triples are consumed and not kept.
            "HIGH" | "MED" | "LOW" => {
                rest.next().ok_or_else(|| missing(keyword))?;
            }
            other => {
                return Err(SoundTableError::UnknownKeyword {
                    set: set.to_string(),
                    name: name.to_string(),
                    keyword: other.to_string(),
                });
            }
        }
    }

    Ok(def)
}

/// A WAV file name as the sound bank keys it: lower-cased, no `.wav`.
pub fn wav_stem(file: &str) -> String {
    let lower = file.to_ascii_lowercase();
    match lower.strip_suffix(".wav") {
        Some(stem) => stem.to_string(),
        None => lower,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn table(json: &str) -> Result<SoundTable, SoundTableError> {
        let mut entries = BTreeMap::new();
        entries.insert(
            SOUNDS_ENTRY.to_string(),
            serde_json::from_str(json).expect("the fixture is JSON"),
        );
        parse(&ReaderData::from_entries(entries))
    }

    /// The shapes the real file uses, values invented.
    const FIXTURE: &str = r#"[["SYNTAX", [2], "SOUND_PATH", ["..\\sounds"], "SETS", [
        "COMMON", [
            ["snd_a", "Alpha.WAV", "3D", "PURGEABLE", "RANGE", [60.0, 180.0],
             "HIGH", [22050, 16, 1], "MED", [22050, 8, 1], "LOW", [11025, 8, 1]],
            ["snd_b", "b.wav", "VOLUME", 0.75, "LOOPED", "FREQUENCY", "3D",
             "HIGH", [22050, 16, 1], "MED", [22050, 8, 1], "LOW", [11025, 8, 1]]
        ],
        "M1", [["snd_a", "alpha.wav", "VOICE", "HIGH", [22050, 16, 1]],
               ["snd_r", "r.wav", "3D", "RANGE", [50.0, 20.0]]],
        "M9", ["snd_c", "c.wav", "HARDWARE", "HIGH", [22050, 16, 1]]
    ]]]"#;

    #[test]
    fn a_record_is_read_by_keyword_not_position() {
        let table = table(FIXTURE).expect("parses");

        let a = table.get("snd_a").expect("snd_a");
        assert_eq!(a.file, "alpha");
        assert!(a.positional && a.purgeable && !a.looped);
        assert_eq!(a.range, Some((60.0, 180.0)));
        assert_eq!(a.volume, 1.0);

        // `VOLUME` before the flags, as three of the real records have it.
        let b = table.get("snd_b").expect("snd_b");
        assert_eq!(b.volume, 0.75);
        assert!(b.looped && b.frequency && b.positional);
        assert_eq!(b.range, None);
    }

    #[test]
    fn the_first_set_to_name_a_sound_wins() {
        let table = table(FIXTURE).expect("parses");
        let a = table.get("snd_a").expect("snd_a");
        assert_eq!(a.set, "COMMON");
        assert!(!a.voice, "the M1 duplicate did not overwrite it");
    }

    #[test]
    fn a_set_collapsed_to_its_one_record_still_reads() {
        let table = table(FIXTURE).expect("parses");
        let c = table.get("snd_c").expect("snd_c");
        assert_eq!(c.set, "M9");
        assert!(c.hardware);
        assert_eq!(table.len(), 4);
    }

    #[test]
    fn a_reversed_range_is_put_in_order() {
        let table = table(FIXTURE).expect("parses");
        assert_eq!(table.get("snd_r").expect("snd_r").range, Some((20.0, 50.0)));
    }

    #[test]
    fn an_unknown_keyword_is_an_error_naming_the_record() {
        let error = table(r#"[["SETS", ["COMMON", [["snd_x", "x.wav", "STEREO"]]]]]"#)
            .expect_err("rejects it");
        assert_eq!(
            error,
            SoundTableError::UnknownKeyword {
                set: "COMMON".into(),
                name: "snd_x".into(),
                keyword: "STEREO".into(),
            }
        );
    }

    #[test]
    fn a_range_without_its_pair_is_an_error() {
        let error = table(r#"[["SETS", ["COMMON", [["snd_x", "x.wav", "RANGE"]]]]]"#)
            .expect_err("rejects it");
        assert!(matches!(error, SoundTableError::MissingValue { .. }));
    }

    #[test]
    fn no_entry_is_its_own_error() {
        assert_eq!(parse(&ReaderData::default()), Err(SoundTableError::NoEntry));
    }

    #[test]
    fn stems_are_lower_case_without_the_extension() {
        assert_eq!(wav_stem("Ocean_Gull.WAV"), "ocean_gull");
        assert_eq!(wav_stem("arc01-1.wav"), "arc01-1");
        assert_eq!(wav_stem("noext"), "noext");
    }
}
