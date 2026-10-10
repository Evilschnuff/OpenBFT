//! Upgrading saves written in an older major version of the format.
//!
//! # How a major bump is done
//!
//! RON's generic `Value` cannot hold enums, and the format is full of them,
//! so a migration does not edit a value tree. Instead, when the format goes
//! from major N to N+1:
//!
//! 1. copy `format.rs` as it was to `format_vN.rs`, a frozen module nobody
//!    edits again, and bump [`CURRENT_VERSION`](super::CURRENT_VERSION);
//! 2. write `fn from_vN(old: format_vN::SaveFile) -> format_vN1::SaveFile`,
//!    which is ordinary, type-checked Rust;
//! 3. add the arm to [`upgrade`] that parses the text as `format_vN` and
//!    runs every step from there to the current major.
//!
//! Steps are never deleted: a save from 1.0 must load in every later build.
//! Minor versions need none of this — their changes are additive and serde's
//! defaults absorb them (SAVEGAME §4).
//!
//! 1.0 is the first format, so there is nothing to upgrade yet.

use super::{SaveFile, SaveFileError, SaveVersion};

/// Parse `text`, written in the older major `from`, as the current format.
pub fn upgrade(text: &str, from: SaveVersion) -> Result<SaveFile, SaveFileError> {
    // The arms go here, oldest first, each chaining into the next:
    //
    //     1 => from_v2(from_v1(ron::from_str::<format_v1::SaveFile>(text)?)),
    let _ = text;
    Err(SaveFileError::Migration {
        from,
        reason: "no open-bft release wrote this version".to_string(),
    })
}
