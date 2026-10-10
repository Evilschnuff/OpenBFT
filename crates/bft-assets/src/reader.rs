//! `zrdr.zbd` → [`ReaderData`]: the game's data tables.
//!
//! Reader archives are where Recoil keeps everything that is not geometry:
//! vehicle tuning, weapon ballistics, pickup kinds, sound names, AI path
//! networks, enemy placements, mission objectives. There are two kinds —
//! one global archive (127 entries) and one per level (14–150) — and
//! [`ReaderData`] is either of them (READER-SCHEMA §1).
//!
//! # What this module does and does not know
//!
//! It parses the archive and hands out [`ReaderValue`] views. What a key
//! *means* lives with the consumer: `bft-sim`'s vehicle module reads
//! `vehicle.zrd`, weapons `weapons.zrd` and `pickup.zrd`, AI `ai.zrd` and
//! `net_NN.zrd`. The typed views here are [`mission`], and [`sounds`],
//! because the sound table has no home in `bft-sim`.
//!
//! # The format, in one paragraph
//!
//! A file is a list alternating `"KEY", <value>`, wrapped in one extra
//! single-element list, nesting the same way. A key with no value is followed
//! by `null`. Keys repeat and their order matters. Some values are positional
//! records rather than maps. See [`ReaderValue`](crate::reader_value) for the
//! view that handles all of it.

use std::collections::BTreeMap;

use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::prelude::*;
use serde_json::Value;

use crate::reader_value::ReaderValue;

pub mod effects;
pub mod mission;
pub mod sounds;

/// The key holding the player's start in `aiv.zrd` (READER-SCHEMA §3.2).
pub const PLAYER_KEY: &str = "bft_00";

/// The `aiv.zrd` entry, where enemy placements and the player start live.
pub const AIV_ENTRY: &str = "aiv.zrd";

/// Every entry of one reader archive, parsed but not interpreted.
///
/// Entries are keyed **without** the `.json` the extraction adds, so a caller
/// names `vehicle.zrd` the way READER-SCHEMA does.
#[derive(Asset, TypePath, Debug, Clone, Default)]
pub struct ReaderData {
    entries: BTreeMap<String, Value>,
}

impl ReaderData {
    /// Build from entry name to parsed JSON, for tests and for a caller that
    /// already has the values.
    pub fn from_entries(entries: BTreeMap<String, Value>) -> ReaderData {
        ReaderData { entries }
    }

    /// One entry's parsed JSON, named as READER-SCHEMA does (`vehicle.zrd`).
    pub fn entry(&self, name: &str) -> Option<&Value> {
        self.entries.get(name)
    }

    /// One entry as a reader value, with the outer wrapper already unwrapped.
    pub fn read(&self, name: &str) -> Option<ReaderValue<'_>> {
        self.entry(name).map(ReaderValue::root)
    }

    /// Every entry name, in sorted order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// How many entries the archive holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the archive holds nothing, which a valid one never does.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Where the player starts in this level, from `aiv.zrd`'s `bft_00`.
    ///
    /// `None` when the entry or the key is missing, which no shipped level
    /// does — every one of the 13 has a `bft_00` record.
    pub fn player_start(&self) -> Option<PlayerStart> {
        player_start_from_aiv(self.entry(AIV_ENTRY)?)
    }
}

/// Where the player starts, and which way they face.
///
/// From `aiv.zrd`'s positional record `[net_index, [x, y, z], heading_deg]`.
/// The heading is **degrees** — reader angles are degrees even though gamez
/// stores radians (READER-SCHEMA §0.4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerStart {
    /// World position, in metres.
    pub position: Vec3,
    /// Yaw, in degrees, as the reader states it.
    pub heading_deg: f32,
}

impl PlayerStart {
    /// The heading as a Bevy yaw rotation.
    pub fn rotation(&self) -> Quat {
        Quat::from_rotation_y(self.heading_deg.to_radians())
    }

    /// A unit vector pointing the way the player faces.
    pub fn facing(&self) -> Vec3 {
        self.rotation() * Vec3::NEG_Z
    }
}

/// Find `bft_00` in a parsed `aiv.zrd`.
///
/// The file is a flat key/value list wrapped in one extra single-element
/// list (READER-SCHEMA §0.1). The player's value is the positional record
/// `[net_index, [x, y, z], heading_deg]`; `net_index` is 0 for the player and
/// is not read here.
pub fn player_start_from_aiv(json: &Value) -> Option<PlayerStart> {
    let record = ReaderValue::root(json).get(PLAYER_KEY)?;

    let position = record.nth(1)?.float_array::<3>()?;
    let heading_deg = record.nth(2)?.float()?;

    Some(PlayerStart {
        position: Vec3::from_array(position),
        heading_deg,
    })
}

/// What can go wrong reading a reader archive.
#[derive(Debug, thiserror::Error)]
pub enum ReaderError {
    #[error("cannot read the reader archive: {0}")]
    Io(#[from] std::io::Error),
    #[error("the reader archive is not a readable ZIP: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("{0} is not the JSON the reader loader expects: {1}")]
    Json(String, serde_json::Error),
}

/// Loads `cache://reader/<level>-zrdr.rdr.zip` and `cache://reader/zrdr.rdr.zip`.
///
/// **Registered for no extension.** The asset server resolves a loader by the
/// requested *type* before it looks at extensions, so claiming `.rdr.zip`
/// here would only create a conflict the moment a second loader wants the
/// same file.
#[derive(TypePath, Debug, Default)]
pub struct ReaderLoader;

impl AssetLoader for ReaderLoader {
    type Asset = ReaderData;
    type Settings = ();
    type Error = ReaderError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _context: &mut LoadContext<'_>,
    ) -> Result<ReaderData, ReaderError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        parse_reader_data(&bytes)
    }

    fn extensions(&self) -> &[&str] {
        &[]
    }
}

/// The suffix the extraction puts on every reader entry.
const ZRD_SUFFIX: &str = ".json";

/// The ZIP's own index, which is **not** a reader entry.
///
/// It holds the original file names, flags and timestamps mech3ax needs to
/// rebuild the archive byte-for-byte. Storing it as an entry named `manifest`
/// is what made the global archive report 128 entries where READER-SCHEMA §1
/// counts 127.
const MANIFEST_ENTRY: &str = "manifest.json";

/// Parse every entry of an extracted reader ZIP.
///
/// An entry that will not parse is logged and skipped rather than failing the
/// archive: the 101 anim-definition sources are not read by this engine at
/// all (docs/data/reader.md), and one of them being odd must not cost the mission.
pub fn parse_reader_data(bytes: &[u8]) -> Result<ReaderData, ReaderError> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes))?;
    let mut entries = BTreeMap::new();

    let names: Vec<String> = zip.file_names().map(str::to_string).collect();

    for name in names {
        if name == MANIFEST_ENTRY {
            continue;
        }
        let Some(stem) = name.strip_suffix(ZRD_SUFFIX) else {
            continue;
        };

        let entry = zip.by_name(&name)?;
        match serde_json::from_reader::<_, Value>(entry) {
            Ok(value) => {
                entries.insert(stem.to_string(), value);
            }
            Err(e) => warn!("reader entry {name} will not parse, skipping it: {e}"),
        }
    }

    Ok(ReaderData { entries })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape the real files have, down to the extra outer list.
    fn aiv(body: &str) -> Value {
        serde_json::from_str(&format!("[[{body}]]")).expect("valid test JSON")
    }

    #[test]
    fn the_player_record_is_position_then_heading() {
        // m1's real values (READER-SCHEMA §3.2).
        let json = aiv(r#""bft_00", [0, [2222.1, 3.9, 1294.2], 166.5]"#);

        let start = player_start_from_aiv(&json).expect("bft_00 is there");

        assert_eq!(start.position, Vec3::new(2222.1, 3.9, 1294.2));
        assert_eq!(start.heading_deg, 166.5);
    }

    /// The player is one key among 84 in m1, and never the first in general.
    #[test]
    fn the_player_is_found_among_the_enemy_placements() {
        let json = aiv(r#""rumv_easy_01", [1, [2529.4, 16.6, 1597.6], 80.0],
               "bft_00", [0, [2222.1, 3.9, 1294.2], 166.5],
               "scou_easy_01", [5, [2337.6, 7.1, 1615.5], 60.0]"#);

        let start = player_start_from_aiv(&json).expect("found");

        assert_eq!(start.position, Vec3::new(2222.1, 3.9, 1294.2));
    }

    #[test]
    fn an_aiv_without_a_player_is_none_rather_than_a_wrong_answer() {
        let json = aiv(r#""scou_03", [12, [2672.7, 25.4, 1667.9], 20.0]"#);

        assert_eq!(player_start_from_aiv(&json), None);
    }

    /// A truncated record must not be read as a position from the next key.
    #[test]
    fn a_malformed_record_is_none_rather_than_a_misread() {
        let json = aiv(r#""bft_00", [0], "scou_03", [12, [1.0, 2.0, 3.0], 0.0]"#);

        assert_eq!(player_start_from_aiv(&json), None);
    }

    /// Reader headings are degrees, gamez rotations are radians
    /// (READER-SCHEMA §0.4), so the conversion has to happen exactly once.
    #[test]
    fn the_heading_becomes_a_yaw_in_radians() {
        let start = PlayerStart {
            position: Vec3::ZERO,
            heading_deg: 90.0,
        };

        let facing = start.facing();

        // 90 degrees of yaw turns -z into -x in Bevy's right-handed, y-up
        // frame, which is the convention the levels already use.
        assert!((facing.x - -1.0).abs() < 1e-5, "got {facing:?}");
        assert!(facing.z.abs() < 1e-5, "got {facing:?}");
    }

    /// The ZIP's own index is not a reader entry; counting it puts the
    /// global archive at 128 where READER-SCHEMA §1 says 127.
    #[test]
    fn the_zip_manifest_is_not_stored_as_an_entry() {
        let mut bytes = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            for (name, body) in [
                (MANIFEST_ENTRY, r#"[{"name": "aiv.zrd"}]"#),
                ("aiv.zrd.json", r#"[["bft_00", [0, [1.0, 2.0, 3.0], 0.0]]]"#),
            ] {
                use std::io::Write;
                zip.start_file(name, options).expect("writes");
                zip.write_all(body.as_bytes()).expect("writes");
            }
            zip.finish().expect("finishes");
        }

        let data = parse_reader_data(&bytes).expect("parses");

        assert_eq!(data.len(), 1);
        assert_eq!(data.names().collect::<Vec<_>>(), vec![AIV_ENTRY]);
        assert!(data.player_start().is_some());
    }

    #[test]
    fn the_player_start_is_reachable_through_the_archive() {
        let data = ReaderData::from_entries(
            [(
                AIV_ENTRY.to_string(),
                aiv(r#""bft_00", [0, [1560.0, -4.0, 614.0], 0.0]"#),
            )]
            .into_iter()
            .collect(),
        );

        assert_eq!(
            data.player_start().map(|start| start.position),
            Some(Vec3::new(1560.0, -4.0, 614.0))
        );
        assert_eq!(ReaderData::default().player_start(), None);
    }
}
