//! A typed view over one reader (`zrdr.zbd`) entry's JSON.
//!
//! **[`crate::reader`] owns the reader as a whole.** READER-SCHEMA describes
//! 127 global and 14–150 per-mission entries; this module reads *values*, not
//! the archive, and knows nothing about what any particular key means. It
//! exists because `bft-sim` reads `vehicle.zrd`, `player.zrd` and others
//! itself, and hand-rolling `serde_json::Value`
//! walks in `bft-sim` would put format knowledge in the wrong crate (the
//! whole point of `bft-assets`).
//!
//! # The format, in one paragraph
//!
//! A file is one JSON value: a list alternating `"KEY", <value>`, wrapped in
//! one extra single-element list, nesting the same way (READER-SCHEMA §0.1).
//! A key with no value is followed by `null`, because mech3ax collapses a
//! one-element list into its element and an empty list into `null` (§0.2).
//! Some values are *positional* records mixing scalars and lists — `aiv.zrd`
//! is `[net_index, [x,y,z], heading_deg]` — so a caller needs both
//! [`ReaderValue::get`] and [`ReaderValue::nth`] over the same value (§0.3).
//! Keys may repeat and their order matters, so the map view is a `Vec` of
//! pairs and never a `HashMap` (§0.1).
//!
//! # The one-element-list trap
//!
//! `["health", [140, 60]]` and `["mass", [2.2]]` are the same shape in the
//! format, but mech3ax writes the second as `2.2`, not `[2.2]`. So "the
//! float under `mass`" and "the first float under `health`" have to be the
//! same call: [`ReaderValue::float`] reads a bare number *or* a one-element
//! list, and [`ReaderValue::floats`] reads a list *or* a bare number as a
//! list of one. Every `[f]`-shaped key in READER-SCHEMA arrives collapsed.

use serde_json::Value;

/// A borrowed view over a reader value.
///
/// Cheap to copy: it is a reference to a `serde_json::Value` plus the path
/// that reached it, which is what error messages need (DESIGN §9: a data
/// error names the file and the field).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReaderValue<'a> {
    value: &'a Value,
}

impl<'a> ReaderValue<'a> {
    /// View a parsed reader entry, unwrapping the outer single-element list.
    ///
    /// Every file is wrapped once (§0.1); some are wrapped twice when their
    /// single top-level key's value is itself a list, so this unwraps while
    /// the value is a one-element list *of a list*, which is exactly what
    /// `tools/recon/zrd.py` does.
    pub fn root(value: &'a Value) -> ReaderValue<'a> {
        let mut value = value;

        while let Some([only]) = value.as_array().map(|array| array.as_slice())
            && only.is_array()
        {
            value = only;
        }

        ReaderValue { value }
    }

    /// View a value directly, without unwrapping anything.
    pub fn new(value: &'a Value) -> ReaderValue<'a> {
        ReaderValue { value }
    }

    /// The underlying JSON, for a caller that needs something this view does
    /// not model.
    pub fn raw(&self) -> &'a Value {
        self.value
    }

    /// Whether this value is `null` — a key that is present and carries
    /// nothing (§0.2).
    pub fn is_null(&self) -> bool {
        self.value.is_null()
    }

    /// The key/value pairs of a map-shaped value, in file order.
    ///
    /// Empty for anything that is not a list alternating string and value,
    /// which is how a positional record reads: `[0, [x,y,z], 166.5]` has no
    /// pairs, and the caller uses [`Self::nth`] instead.
    pub fn pairs(&self) -> Vec<(&'a str, ReaderValue<'a>)> {
        let Some(array) = self.value.as_array() else {
            return Vec::new();
        };

        // `chunks(2)` and not `windows(2)`: a map's pairs do not overlap, and
        // a trailing odd element is a malformed file rather than a key.
        array
            .chunks(2)
            .filter_map(|chunk| match chunk {
                [key, value] => Some((key.as_str()?, ReaderValue { value })),
                _ => None,
            })
            .collect()
    }

    /// The first value under `key`, or `None`.
    ///
    /// First rather than last because reader keys that repeat (anim
    /// `SEQUENCE_DEFINITION`) are read through [`Self::all`]; every key this
    /// crate reads by name is unique within its block.
    pub fn get(&self, key: &str) -> Option<ReaderValue<'a>> {
        self.pairs()
            .into_iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value)
    }

    /// Every value under `key`, in file order, for the keys that repeat.
    pub fn all(&self, key: &str) -> Vec<ReaderValue<'a>> {
        self.pairs()
            .into_iter()
            .filter(|(name, _)| *name == key)
            .map(|(_, value)| value)
            .collect()
    }

    /// Whether a key is present, whatever its value — including `null`.
    pub fn has(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// The `index`th element of a positional record or list value.
    pub fn nth(&self, index: usize) -> Option<ReaderValue<'a>> {
        self.value
            .as_array()?
            .get(index)
            .map(|value| ReaderValue { value })
    }

    /// Every element of a list value, or an empty vector for anything else.
    pub fn items(&self) -> Vec<ReaderValue<'a>> {
        self.value
            .as_array()
            .map(|array| array.iter().map(|value| ReaderValue { value }).collect())
            .unwrap_or_default()
    }

    /// How many elements a list value has; 0 for a scalar or `null`.
    pub fn len(&self) -> usize {
        self.value.as_array().map(|array| array.len()).unwrap_or(0)
    }

    /// Whether a list value is empty. A scalar is *not* empty.
    pub fn is_empty(&self) -> bool {
        match self.value.as_array() {
            Some(array) => array.is_empty(),
            None => self.value.is_null(),
        }
    }

    /// This value as a float, reading a one-element list as its element.
    ///
    /// The collapse is mech3ax's, not ours: every `[f]` key in
    /// READER-SCHEMA (`mass`, `mode_alt`, `turn_damping`, …) arrives as a
    /// bare number, while `[f, f]` keys arrive as a list.
    pub fn float(&self) -> Option<f32> {
        match self.value {
            Value::Number(number) => number.as_f64().map(|value| value as f32),
            Value::Array(array) => match array.as_slice() {
                [only] => ReaderValue { value: only }.float(),
                _ => None,
            },
            _ => None,
        }
    }

    /// This value as a list of floats, reading a bare number as a list of one.
    pub fn floats(&self) -> Vec<f32> {
        match self.value {
            Value::Number(_) => self.float().into_iter().collect(),
            Value::Array(array) => array
                .iter()
                .filter_map(|value| ReaderValue { value }.float())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// This value as an integer, with the same one-element-list collapse as
    /// [`Self::float`].
    pub fn int(&self) -> Option<i64> {
        match self.value {
            Value::Number(number) => number.as_i64(),
            Value::Array(array) => match array.as_slice() {
                [only] => ReaderValue { value: only }.int(),
                _ => None,
            },
            _ => None,
        }
    }

    /// This value as a `1`/`0` flag, which is how the reader spells booleans
    /// (`ok_to_amphib`). Any non-zero integer is true.
    pub fn flag(&self) -> Option<bool> {
        self.int().map(|value| value != 0)
    }

    /// This value as a string, with the same one-element-list collapse.
    pub fn string(&self) -> Option<&'a str> {
        match self.value {
            Value::String(text) => Some(text),
            Value::Array(array) => match array.as_slice() {
                [only] => ReaderValue { value: only }.string(),
                _ => None,
            },
            _ => None,
        }
    }

    /// This value as a list of strings, reading a bare string as one.
    pub fn strings(&self) -> Vec<&'a str> {
        match self.value {
            Value::String(_) => self.string().into_iter().collect(),
            Value::Array(array) => array
                .iter()
                .filter_map(|value| ReaderValue { value }.string())
                .collect(),
            _ => Vec::new(),
        }
    }

    /// Exactly `N` floats, or `None`.
    ///
    /// The shape check every fixed-width key wants: `rates` is `[f, f]` and a
    /// file that gives three is not a file this code understands.
    pub fn float_array<const N: usize>(&self) -> Option<[f32; N]> {
        let floats = self.floats();

        (floats.len() == N).then(|| {
            let mut out = [0.0; N];
            out.copy_from_slice(&floats);
            out
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Value {
        serde_json::from_str(text).expect("valid test JSON")
    }

    #[test]
    fn the_outer_wrapper_is_unwrapped_but_a_real_single_key_is_not() {
        let json = parse(r#"[["health", [140, 60]]]"#);
        let root = ReaderValue::root(&json);

        assert_eq!(
            root.get("health").map(|v| v.floats()),
            Some(vec![140.0, 60.0])
        );
    }

    /// The collapse mech3ax does: `[2.2]` is written `2.2`, so "the float
    /// under this key" cannot assume a list.
    #[test]
    fn a_one_element_list_reads_as_its_element() {
        let json = parse(r#"[["mass", 2.2, "rates", [48, 65]]]"#);
        let root = ReaderValue::root(&json);

        assert_eq!(root.get("mass").and_then(|v| v.float()), Some(2.2));
        assert_eq!(root.get("rates").and_then(|v| v.float()), None);
        assert_eq!(
            root.get("rates").and_then(|v| v.float_array::<2>()),
            Some([48.0, 65.0])
        );
        // And a collapsed scalar still reads as a list of one.
        assert_eq!(root.get("mass").map(|v| v.floats()), Some(vec![2.2]));
    }

    #[test]
    fn a_key_with_no_value_is_present_and_null() {
        let json = parse(r#"[["ANIMATION", null, "SOUND", null]]"#);
        let root = ReaderValue::root(&json);

        assert!(root.has("ANIMATION"));
        assert!(root.get("ANIMATION").expect("present").is_null());
        assert_eq!(root.get("MISSING"), None.map(|v: ReaderValue| v));
    }

    /// `aiv.zrd`'s `[net_index, [x,y,z], heading]`: no keys, only positions.
    #[test]
    fn a_positional_record_reads_by_index_and_has_no_pairs() {
        let json = parse(r#"[0, [2222.1, 3.9, 1294.2], 166.5]"#);
        let record = ReaderValue::new(&json);

        assert!(record.pairs().is_empty());
        assert_eq!(record.nth(0).and_then(|v| v.int()), Some(0));
        assert_eq!(
            record.nth(1).and_then(|v| v.float_array::<3>()),
            Some([2222.1, 3.9, 1294.2])
        );
        assert_eq!(record.nth(2).and_then(|v| v.float()), Some(166.5));
    }

    #[test]
    fn repeated_keys_keep_their_order_and_are_all_reachable() {
        let json = parse(r#"[["W", ["a"], "W", ["b"], "W", ["c"]]]"#);
        let root = ReaderValue::root(&json);

        let all: Vec<_> = root.all("W").iter().filter_map(|v| v.string()).collect();
        assert_eq!(all, vec!["a", "b", "c"]);
        // `get` is the first, which is what a unique key wants.
        assert_eq!(root.get("W").and_then(|v| v.string()), Some("a"));
    }

    #[test]
    fn a_flag_is_the_readers_one_or_zero() {
        let json = parse(r#"[["ok_to_amphib", 1, "off", 0]]"#);
        let root = ReaderValue::root(&json);

        assert_eq!(root.get("ok_to_amphib").and_then(|v| v.flag()), Some(true));
        assert_eq!(root.get("off").and_then(|v| v.flag()), Some(false));
    }

    #[test]
    fn a_wrong_width_array_is_none_rather_than_a_truncated_answer() {
        let json = parse(r#"[["rates", [48, 65, 12]]]"#);
        let root = ReaderValue::root(&json);

        assert_eq!(root.get("rates").and_then(|v| v.float_array::<2>()), None);
        assert_eq!(
            root.get("rates").and_then(|v| v.float_array::<3>()),
            Some([48.0, 65.0, 12.0])
        );
    }

    /// Nested blocks: `vehicle.zrd` is three levels deep.
    #[test]
    fn nesting_repeats_the_same_convention() {
        let json = parse(r#"[["bft", ["track_mode", ["mode", "track", "rates", [48, 65]]]]]"#);
        let root = ReaderValue::root(&json);

        let mode = root
            .get("bft")
            .and_then(|v| v.get("track_mode"))
            .expect("the mode block");

        assert_eq!(mode.get("mode").and_then(|v| v.string()), Some("track"));
        assert_eq!(
            mode.get("rates").and_then(|v| v.float_array::<2>()),
            Some([48.0, 65.0])
        );
    }

    #[test]
    fn a_list_of_records_is_read_by_items() {
        let json = parse(r#"[["weapons", [["wep_1.0", 1, 1, 123456789, 0.15, 5, 100, 1, 2]]]]"#);
        let root = ReaderValue::root(&json);

        let rows = root.get("weapons").expect("weapons").items();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].nth(0).and_then(|v| v.string()), Some("wep_1.0"));
        assert_eq!(rows[0].nth(4).and_then(|v| v.float()), Some(0.15));
    }
}
