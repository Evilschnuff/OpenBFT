//! `assets/tuning/sounds.ron`: the sound choices the data does not make.
//!
//! Most of the mapping from events to sounds is reader data and is read from
//! there: weapon fire, flight and impact sounds (`weapons.zrd`), pickup
//! sounds (`pickup.zrd`), engines, skids and the weapon-select click
//! (`vehicle.zrd`), and every anim `Sound`/`SoundNode` event, all resolved
//! through `sounds.zrd` (docs/data/sounds.md). What is left is here:
//!
//! - what a destruction sounds like, because the original plays that from
//!   the victim's kill animation;
//! - which transform sound a form change plays, because the transition
//!   animations in `anim.zbd` carry no sound events;
//! - the handful of engine constants no reader key covers.

use std::collections::HashMap;

use bevy::prelude::*;
use bft_sim::vehicle::VehicleForm;
use serde::Deserialize;

/// The whole `sounds.ron` file.
///
/// Every field has a default equal to the shipped file, so a missing or
/// still-loading file changes nothing audible.
#[derive(Asset, TypePath, Resource, Deserialize, Debug, Clone, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SoundTuning {
    /// Played where something small is destroyed; one is picked at random.
    pub destroyed_small: Vec<String>,
    /// Played where something large is destroyed.
    pub destroyed_large: Vec<String>,
    /// The collider half-extent, metres, from which a destruction counts as
    /// large.
    pub large_radius: f32,
    /// The transform sound, by the form being changed **to**.
    pub form_change: HashMap<VehicleForm, String>,
    /// How a positional sound fades between its `RANGE` distances.
    pub falloff: Falloff,
    /// `RANGE` for a positional sound whose record has none, metres.
    /// // approximation: 42 `3D` records carry no range; this is the
    /// commonest band among those that do.
    pub default_range: (f32, f32),
    /// The most sounds playing at once; further one-shots are dropped.
    pub max_voices: usize,
    /// Distance between the listener's ears, metres. Only the stereo
    /// panning depends on it.
    pub listener_gap: f32,
    /// How close an enemy has to be for its engine to be heard, metres.
    pub engine_range: f32,
    /// Engine playback rate at a standstill, 1 being the WAV's own. At top
    /// speed it is this plus the mode's `pitch_scale` × [`Self::engine_pitch_range`].
    /// // approximation: `vehicle.zrd` gives `pitch_scale` but not its base.
    pub engine_idle_pitch: f32,
    /// A factor on every mode's `pitch_scale`: how far the rate climbs from
    /// idle to top speed. 1 takes `pitch_scale` as the rise itself.
    /// // approximation: tuned by ear against the original, not measured.
    pub engine_pitch_range: f32,
    /// Engine loudness at a standstill, as a fraction of `volume_scale`.
    /// // approximation: no reader key.
    pub engine_idle_volume: f32,
    /// A gain on the player's own engine, on top of the record's `VOLUME`
    /// and the mode's `volume_scale`. Above 1 because in the original the
    /// tank's motor stands out over everything else.
    /// // approximation: tuned by ear, not measured.
    pub player_engine_volume: f32,
    /// The same for every other vehicle's engine. 1 leaves the data's own
    /// balance (`VOLUME` × `volume_scale`). Separate from the player's,
    /// because several enemy loops are denser than the BFT's and the player's
    /// boost made them louder than the player's own engine up close.
    /// // approximation: tuned by ear, not measured.
    pub enemy_engine_volume: f32,
    /// A gain on everything the game plays, effects and music alike, so the
    /// loudest sound stays under full scale and the balance between sounds
    /// is set by turning the others down, never by pushing one past 1. The
    /// system volume makes up the level. Ours, not the original's: it only
    /// keeps our mix from clipping.
    pub mix_headroom: f32,
    /// The menus' music: the first of these stems (any extension, any
    /// case) the music folder has loops in the menus instead of joining
    /// the shuffle.
    pub menu_tracks: Vec<String>,
}

impl Default for SoundTuning {
    fn default() -> SoundTuning {
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect();
        SoundTuning {
            destroyed_small: names(&["snd_expl01", "snd_expl02", "snd_expl03", "snd_expl04"]),
            destroyed_large: names(&["snd_expl05", "snd_expl06", "snd_expl07", "snd_expl08"]),
            large_radius: 4.0,
            form_change: HashMap::from([
                (VehicleForm::Track, "snd_morphdown".to_string()),
                (VehicleForm::Hover, "snd_morphup".to_string()),
                (VehicleForm::Amphib, "snd_vehamp_morph".to_string()),
                (VehicleForm::Sub, "snd_vehsub_morph".to_string()),
            ]),
            falloff: Falloff::Inverse,
            default_range: (50.0, 400.0),
            max_voices: 48,
            listener_gap: 4.0,
            engine_range: 250.0,
            engine_idle_pitch: 0.58,
            engine_pitch_range: 1.0,
            engine_idle_volume: 0.96,
            player_engine_volume: 1.95,
            enemy_engine_volume: 1.0,
            mix_headroom: 0.5,
            menu_tracks: names(&["menu", "track02"]),
        }
    }
}

/// How loud a positional sound is between the two distances of its
/// `RANGE [min, max]`. Under either, it is at full volume inside `min` and
/// silent past `max`.
///
/// // approximation: `sounds.zrd` gives the two distances but not the curve
/// between them, and the curve is chosen by ear until an observation settles
/// it: [`Falloff::Inverse`] sounds closer to the original. Past `max` both are
/// silent, as a one-shot that far away is not started at all.
#[derive(Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Falloff {
    /// Straight down from 1 at `min` to 0 at `max`: the reading that makes
    /// both distances mean something.
    Linear,
    /// `min / distance`, halving with every doubling of distance:
    /// DirectSound 3D's rule, which the records' `3D`, `HARDWARE` and
    /// `FREQUENCY` flags suggest the original played through. DirectSound
    /// itself holds a sound at its level at `max` from there on; that would
    /// leave every fire loop in a level audible everywhere, so it is cut.
    #[default]
    Inverse,
}

impl SoundTuning {
    /// The gain on an engine: [`Self::player_engine_volume`] for the
    /// player's, [`Self::enemy_engine_volume`] for anyone else's.
    pub fn engine_volume(&self, is_player: bool) -> f32 {
        if is_player {
            self.player_engine_volume
        } else {
            self.enemy_engine_volume
        }
    }

    /// The engine and mix knobs, by their names in `sounds.ron` and as RON:
    /// what the dev panel edits and saves.
    pub fn mix_knobs(&self) -> [(&'static str, String); 8] {
        let number = |value: f32| format!("{value:?}");
        [
            ("falloff", format!("{:?}", self.falloff)),
            ("engine_range", number(self.engine_range)),
            ("engine_idle_pitch", number(self.engine_idle_pitch)),
            ("engine_pitch_range", number(self.engine_pitch_range)),
            ("engine_idle_volume", number(self.engine_idle_volume)),
            ("player_engine_volume", number(self.player_engine_volume)),
            ("enemy_engine_volume", number(self.enemy_engine_volume)),
            ("mix_headroom", number(self.mix_headroom)),
        ]
    }
}

/// `sounds.ron`'s text with each named value's line rewritten. Values are
/// RON already (`0.58`, `Inverse`).
///
/// Comments, order and every other line are kept, so a save from the dev
/// panel changes only the values. Each name must already have its own
/// `name: value,` line; one that has none is an error, not an append.
pub fn rewrite_values(existing: &str, values: &[(&str, String)]) -> Result<String, String> {
    let mut found = vec![false; values.len()];
    let mut out = String::with_capacity(existing.len());

    for line in existing.split_inclusive('\n') {
        let body = line.trim_start();
        let indent = &line[..line.len() - body.len()];
        let ending = &line[line.trim_end_matches(['\r', '\n']).len()..];
        let hit = values.iter().position(|(name, _)| {
            body.strip_prefix(name)
                .is_some_and(|rest| rest.trim_start().starts_with(':'))
        });
        let Some(index) = hit else {
            out.push_str(line);
            continue;
        };

        found[index] = true;
        let (name, value) = &values[index];
        let comment = body
            .find("//")
            .map(|at| format!(" {}", body[at..].trim_end()))
            .unwrap_or_default();
        out.push_str(&format!("{indent}{name}: {value},{comment}{ending}"));
    }

    let missing: Vec<&str> = values
        .iter()
        .zip(&found)
        .filter(|(_, found)| !**found)
        .map(|((name, _), _)| *name)
        .collect();
    if missing.is_empty() {
        Ok(out)
    } else {
        Err(format!("no line for {}", missing.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/tuning/sounds.ron"),
        )
        .expect("the shipped sounds.ron is in the repo")
    }

    /// The file and the defaults say the same thing, so a slow load changes
    /// nothing and the file documents every value the code falls back to.
    #[test]
    fn the_shipped_file_parses_to_the_defaults() {
        let parsed: SoundTuning = ron::de::from_str(&shipped()).expect("it parses");
        assert_eq!(parsed, SoundTuning::default());
    }

    #[test]
    fn every_form_has_a_transform_sound() {
        let parsed: SoundTuning = ron::de::from_str(&shipped()).expect("it parses");
        for form in VehicleForm::ALL {
            assert!(parsed.form_change.contains_key(&form), "{form:?}");
        }
    }

    #[test]
    fn a_partial_file_keeps_the_other_defaults() {
        let parsed: SoundTuning = ron::de::from_str("(max_voices: 8)").expect("it parses");
        assert_eq!(parsed.max_voices, 8);
        assert_eq!(parsed.engine_range, SoundTuning::default().engine_range);
    }

    #[test]
    fn saving_the_defaults_leaves_the_file_as_it_is() {
        let text = shipped();
        let knobs = SoundTuning::default().mix_knobs();
        assert_eq!(
            rewrite_values(&text, &knobs).expect("every knob has a line"),
            text
        );
    }

    #[test]
    fn saving_changes_only_the_values() {
        let text = shipped();
        let tuned = SoundTuning {
            engine_idle_pitch: 0.65,
            engine_pitch_range: 1.8,
            player_engine_volume: 0.75,
            enemy_engine_volume: 0.6,
            mix_headroom: 0.4,
            falloff: Falloff::Linear,
            ..SoundTuning::default()
        };

        let saved = rewrite_values(&text, &tuned.mix_knobs()).expect("rewrites");
        let parsed: SoundTuning = ron::de::from_str(&saved).expect("still parses");
        assert_eq!(parsed, tuned);

        // Every comment survives, in place.
        let comments = |s: &str| -> Vec<String> {
            s.lines()
                .filter(|line| line.trim_start().starts_with("//"))
                .map(str::to_string)
                .collect()
        };
        assert_eq!(comments(&saved), comments(&text));
        assert_eq!(saved.lines().count(), text.lines().count());
    }

    #[test]
    fn a_knob_with_no_line_is_refused() {
        let one = |name, value: &str| [(name, value.to_string())];
        let result = rewrite_values(
            "(\n    engine_range: 1.0,\n)\n",
            &one("player_engine_volume", "1.0"),
        );
        assert!(result.is_err());
        // A longer name sharing the prefix is not a match.
        let result = rewrite_values(
            "(\n    engine_range_x: 1.0,\n)\n",
            &one("engine_range", "2.0"),
        );
        assert!(result.is_err());
    }

    #[test]
    fn a_typo_is_a_parse_error_rather_than_a_silent_no_op() {
        let result: Result<SoundTuning, _> = ron::de::from_str("(max_voice: 8)");
        assert!(result.is_err());
    }
}
