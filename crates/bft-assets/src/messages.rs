//! `messages.json` → [`Messages`]: every string the game displays.
//!
//! `messages.dll` is a Windows message table, so the 749 entries use
//! `%1!s!`-style insert placeholders rather than anything Rust can format
//! directly (docs/data/messages.md). [`Messages::format`] translates them.
//!
//! This is the single source of user-facing text (FR-U4): mission titles
//! (`MSG_CAMPAIGN<N>_NAME`), the 32 objective strings
//! (`MSG_M<n>_O<k>_{SUMMARY,OBJ,COMP}`), weapon names, HUD lines and every
//! menu label. Nothing in this engine hard-codes a display string that the
//! table already carries.

use std::collections::HashMap;
use std::fmt::Display;

use bevy::asset::io::Reader;
use bevy::asset::{Asset, AssetLoader, LoadContext};
use bevy::prelude::*;

use serde::Deserialize;

/// The game's string table.
#[derive(Asset, TypePath, Debug, Clone, Default)]
pub struct Messages {
    /// The Windows LCID the table was built for. 1033 (US English) in the
    /// retail release; kept so a localised install is recognisable.
    pub language_id: u32,
    /// `MSG_*` key → text, with the placeholders still in it.
    pub entries: HashMap<String, String>,
}

impl Messages {
    /// The raw text for a key, placeholders and all.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(String::as_str)
    }

    /// The text for a key with its inserts substituted.
    ///
    /// Message-table placeholders are 1-based (`%1` is `args[0]`), may carry
    /// a printf-ish suffix (`%1!s!`, `%1!d!`) and `%n` is a newline. An
    /// argument that is not supplied leaves its placeholder in place rather
    /// than eating the surrounding text, so a missing insert shows up in the
    /// UI instead of silently corrupting a sentence.
    ///
    /// Returns `None` only when the key is absent, which is the case worth
    /// distinguishing from "the string is empty".
    pub fn format(&self, key: &str, args: &[&dyn Display]) -> Option<String> {
        Some(substitute(self.get(key)?, args))
    }

    /// The text for a key, or the key itself.
    ///
    /// What a UI wants: a missing string should show `MSG_M1_O1_OBJ` on
    /// screen, which is diagnosable, rather than a blank label.
    pub fn text(&self, key: &str) -> String {
        self.get(key).unwrap_or(key).to_string()
    }

    /// How many strings the table holds. 749 in the retail release.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the table is empty, which a real one never is.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Replace the Windows message-table placeholders in one string.
fn substitute(text: &str, args: &[&dyn Display]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.char_indices().peekable();

    while let Some((_, c)) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }

        match chars.peek().map(|(_, c)| *c) {
            // `%n` is a hard newline; 17 strings use it.
            Some('n') => {
                chars.next();
                out.push('\n');
            }
            // `%%` is a literal percent. Not in the shipped table, but the
            // format defines it and dropping the escape would eat a digit.
            Some('%') => {
                chars.next();
                out.push('%');
            }
            Some(digit) if digit.is_ascii_digit() => {
                let mut index = 0usize;
                while let Some((_, c)) = chars.peek() {
                    let Some(value) = c.to_digit(10) else { break };
                    index = index * 10 + value as usize;
                    chars.next();
                }

                // The optional `!s!`/`!d!` type suffix. It says how the
                // original formatted the insert; `Display` covers both, so it
                // is consumed and discarded.
                if chars.peek().map(|(_, c)| *c) == Some('!') {
                    chars.next();
                    for (_, c) in chars.by_ref() {
                        if c == '!' {
                            break;
                        }
                    }
                }

                // 1-based, and an absent argument keeps its placeholder.
                match index.checked_sub(1).and_then(|i| args.get(i)) {
                    Some(arg) => out.push_str(&arg.to_string()),
                    None => {
                        out.push('%');
                        out.push_str(&index.to_string());
                    }
                }
            }
            // A bare `%d` with no index — two strings have one. There is no
            // insert number to key on, so it is left alone.
            _ => out.push('%'),
        }
    }

    out
}

/// `messages.json` as mech3ax writes it.
#[derive(Debug, Clone, Default, Deserialize)]
struct MessagesFile {
    #[serde(default)]
    language_id: u32,
    #[serde(default)]
    entries: Vec<MessageEntry>,
}

/// One entry. `id` is the message-table ordinal and nothing reads it.
#[derive(Debug, Clone, Default, Deserialize)]
struct MessageEntry {
    key: String,
    #[serde(default)]
    value: String,
}

/// What can go wrong reading `messages.json`.
#[derive(Debug, thiserror::Error)]
pub enum MessagesError {
    #[error("cannot read messages.json: {0}")]
    Io(#[from] std::io::Error),
    #[error("messages.json is not the message table this loader expects: {0}")]
    Json(#[from] serde_json::Error),
}

/// Loads `cache://messages.json`.
///
/// **Registered for no extension**, like `ReaderLoader`: `interp.json` is the
/// other `.json` in the cache root, and two loaders claiming one extension
/// makes Bevy pick by extension alone and warn. With none registered the
/// asset server resolves this one by the requested type.
#[derive(TypePath, Debug, Default)]
pub struct MessagesLoader;

impl AssetLoader for MessagesLoader {
    type Asset = Messages;
    type Settings = ();
    type Error = MessagesError;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _context: &mut LoadContext<'_>,
    ) -> Result<Messages, MessagesError> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;

        parse_messages(&bytes)
    }

    fn extensions(&self) -> &[&str] {
        &[]
    }
}

/// Parse an extracted `messages.json`.
pub fn parse_messages(bytes: &[u8]) -> Result<Messages, MessagesError> {
    let file: MessagesFile = serde_json::from_slice(bytes)?;

    Ok(Messages {
        language_id: file.language_id,
        entries: file
            .entries
            .into_iter()
            .map(|entry| (entry.key, entry.value))
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(pairs: &[(&str, &str)]) -> Messages {
        Messages {
            language_id: 1033,
            entries: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn the_file_shape_mech3ax_writes_parses() {
        let json = br#"{ "language_id": 1033, "entries": [
            { "key": "MSG_BACK", "id": 1, "value": "Back" },
            { "key": "MSG_SCORE", "id": 2, "value": "Score" }
        ] }"#;

        let messages = parse_messages(json).expect("parses");

        assert_eq!(messages.language_id, 1033);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages.get("MSG_BACK"), Some("Back"));
    }

    /// The real placeholder spellings, all four of them.
    #[test]
    fn the_windows_placeholders_are_translated() {
        let messages = table(&[
            ("PLAIN", "Island Storm"),
            ("INDEXED", "RECOIL %1"),
            ("TYPED_D", "   %1!d! Players"),
            ("TYPED_S", "%1!s! not operative in Sub Mode"),
            ("NEWLINE", "Line one%nLine two"),
            ("TWO", "%1 killed %2"),
        ]);

        assert_eq!(
            messages.format("PLAIN", &[]).as_deref(),
            Some("Island Storm")
        );
        assert_eq!(
            messages.format("INDEXED", &[&"1.0"]).as_deref(),
            Some("RECOIL 1.0")
        );
        assert_eq!(
            messages.format("TYPED_D", &[&4]).as_deref(),
            Some("   4 Players")
        );
        assert_eq!(
            messages.format("TYPED_S", &[&"Mortar"]).as_deref(),
            Some("Mortar not operative in Sub Mode")
        );
        assert_eq!(
            messages.format("NEWLINE", &[]).as_deref(),
            Some("Line one\nLine two")
        );
        assert_eq!(
            messages.format("TWO", &[&"A", &"B"]).as_deref(),
            Some("A killed B")
        );
    }

    /// Inserts are 1-based. Reading `%1` as `args[1]` shifts every string in
    /// the game by one argument, which is silent and wrong everywhere.
    #[test]
    fn inserts_are_one_based() {
        let messages = table(&[("K", "first=%1 second=%2")]);

        assert_eq!(
            messages.format("K", &[&"a", &"b"]).as_deref(),
            Some("first=a second=b")
        );
    }

    /// A caller that forgets an argument should see the gap, not a sentence
    /// missing a word.
    #[test]
    fn a_missing_argument_leaves_its_placeholder_visible() {
        let messages = table(&[("K", "%1 needs %2")]);

        assert_eq!(
            messages.format("K", &[&"this"]).as_deref(),
            Some("this needs %2")
        );
    }

    /// Two shipped strings carry a bare `%d` with no insert number; eating
    /// the `d` would corrupt them.
    #[test]
    fn a_bare_percent_without_an_index_is_left_alone() {
        let messages = table(&[("K", "%d rounds"), ("ESC", "100%% done")]);

        assert_eq!(messages.format("K", &[&5]).as_deref(), Some("%d rounds"));
        assert_eq!(messages.format("ESC", &[]).as_deref(), Some("100% done"));
    }

    #[test]
    fn an_unknown_key_is_none_from_format_and_itself_from_text() {
        let messages = table(&[]);

        assert_eq!(messages.format("MSG_NOPE", &[]), None);
        assert_eq!(messages.text("MSG_NOPE"), "MSG_NOPE");
    }
}
