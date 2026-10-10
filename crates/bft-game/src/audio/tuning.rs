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
    /// Engine playback rate at a standstill. At top speed it is this plus
    /// the mode's `pitch_scale`.
    /// // approximation: `vehicle.zrd` gives `pitch_scale` but not its base.
    pub engine_idle_pitch: f32,
    /// Engine loudness at a standstill, as a fraction of `volume_scale`.
    /// // approximation: no reader key.
    pub engine_idle_volume: f32,
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
            default_range: (50.0, 400.0),
            max_voices: 48,
            listener_gap: 4.0,
            engine_range: 250.0,
            engine_idle_pitch: 1.0,
            engine_idle_volume: 0.6,
            menu_tracks: names(&["menu", "track02"]),
        }
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
    fn a_typo_is_a_parse_error_rather_than_a_silent_no_op() {
        let result: Result<SoundTuning, _> = ron::de::from_str("(max_voice: 8)");
        assert!(result.is_err());
    }
}
