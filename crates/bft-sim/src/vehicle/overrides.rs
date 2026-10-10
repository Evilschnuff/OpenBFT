//! `assets/tuning/vehicle.ron`: the override layer over the reader values.
//!
//! Every leaf is `Option`, so an empty record is "use the game's own values"
//! and the shipped file is a comment block around `()` (DESIGN §5.3). The
//! tree mirrors [`VehicleTable`]: a map of kind key to per-kind overrides,
//! each with a map of form name to per-form overrides, plus one `player`
//! section for `player.zrd`'s constants.
//!
//! Overrides are applied **after** the derivation in
//! [`ModeTuning`](super::tuning::ModeTuning), so an override always wins over
//! a derived constant — which is the point: the derived ones are guesses
//! marked `// approximation`, and this file is where they get corrected
//! without touching code.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::VehicleForm;
use super::tuning::{
    CameraConstants, ModeTuning, PlayerConstants, SuspensionTuning, VehicleDef, VehicleKindId,
    VehicleTable,
};

/// The whole `vehicle.ron` file.
#[derive(Asset, TypePath, Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct VehicleOverrides {
    /// Per `vehicle.zrd` kind key (`"bft"`, `"scou"`, …).
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub kinds: HashMap<String, VehicleDefOverride>,
    /// `player.zrd`'s constants.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub player: Option<PlayerConstantsOverride>,
}

impl VehicleOverrides {
    /// Apply every override in this file to a freshly parsed table.
    ///
    /// A kind or form the file names but the reader does not have is a
    /// warning, not an error: a tuning file outliving a data change should
    /// say so and keep playing.
    pub fn apply(&self, table: &mut VehicleTable) {
        for (name, over) in &self.kinds {
            match table.kinds.get_mut(&VehicleKindId(name.clone())) {
                Some(def) => over.apply(name, def),
                None => warn!("vehicle.ron: no vehicle `{name}` in the reader data; ignoring it"),
            }
        }

        if let Some(player) = &self.player {
            player.apply(&mut table.player);
        }
    }
}

/// One kind's overrides.
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct VehicleDefOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shield: Option<f32>,
    /// Chase-camera offsets in hull space, all three modes.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camback: Option<[Vec3; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aim_y: Option<[f32; 2]>,
    /// Turret traverse and elevation rates, rad/s. No reader equivalent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turret_rate: Option<[f32; 2]>,
    /// Keyed by form name as the data spells it: `track`, `hover`, `amphib`,
    /// `sub`.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub modes: HashMap<String, ModeTuningOverride>,
}

impl VehicleDefOverride {
    fn apply(&self, name: &str, def: &mut VehicleDef) {
        if let Some(health) = self.health {
            def.health = health;
        }
        if let Some(shield) = self.shield {
            def.shield = Some(shield);
        }
        if let Some(camback) = self.camback {
            def.camback = camback;
        }
        if let Some(aim_y) = self.aim_y {
            def.aim_y = aim_y;
        }
        if let Some(turret_rate) = self.turret_rate {
            def.turret_rate = Some(turret_rate);
        }

        for (mode, over) in &self.modes {
            let Some(form) = VehicleForm::from_mode_key(mode) else {
                warn!("vehicle.ron: `{name}` names mode `{mode}`, which is not a form");
                continue;
            };
            match def.modes.get_mut(&form) {
                Some(tuning) => over.apply(tuning),
                None => warn!("vehicle.ron: `{name}` has no `{mode}` mode in the reader data"),
            }
        }
    }
}

/// One form's overrides: the reader values, then the derived constants.
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ModeTuningOverride {
    // Reader values.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode_alt: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rates: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turns: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turn_damping: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate_damping: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub friction: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mass: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub a_damping: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collision_d: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quicksand_slowdown: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lava_slowdown: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ok_to_amphib: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alt_control: Option<[f32; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gun_pitch: Option<[f32; 2]>,

    // Derived constants: the ones marked `// approximation` in `tuning.rs`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suspension: Option<SuspensionOverride>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub engine_force: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_speed: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reverse_speed: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lateral_grip: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub static_grip: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub air_control: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grip_force_scale: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub regrip_fraction: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stationary_speed: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upright_rate_air: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upright_rate_ground: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tilt: Option<f32>,
}

impl ModeTuningOverride {
    fn apply(&self, tuning: &mut ModeTuning) {
        macro_rules! set {
            ($field:ident) => {
                if let Some(value) = self.$field {
                    tuning.$field = value;
                }
            };
            (opt $field:ident) => {
                if let Some(value) = self.$field {
                    tuning.$field = Some(value);
                }
            };
        }

        set!(mode_alt);
        set!(rates);
        set!(turns);
        set!(turn_damping);
        set!(rate_damping);
        set!(opt friction);
        set!(opt mass);
        set!(opt a_damping);
        set!(collision_d);
        set!(opt quicksand_slowdown);
        set!(opt lava_slowdown);
        set!(ok_to_amphib);
        set!(opt alt_control);
        set!(opt gun_pitch);

        set!(engine_force);
        set!(max_speed);
        set!(reverse_speed);
        set!(lateral_grip);
        set!(static_grip);
        set!(air_control);
        set!(grip_force_scale);
        set!(regrip_fraction);
        set!(stationary_speed);
        set!(upright_rate_air);
        set!(upright_rate_ground);
        set!(max_tilt);

        if let Some(suspension) = &self.suspension {
            suspension.apply(&mut tuning.suspension);
        }
    }
}

/// The suspension's own overrides.
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SuspensionOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rays: Option<Vec<Vec3>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rest: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stiffness: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damping: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_len: Option<f32>,
}

impl SuspensionOverride {
    fn apply(&self, suspension: &mut SuspensionTuning) {
        if let Some(rays) = &self.rays {
            suspension.rays = rays.clone();
        }
        if let Some(rest) = self.rest {
            suspension.rest = rest;
        }
        if let Some(stiffness) = self.stiffness {
            suspension.stiffness = stiffness;
        }
        if let Some(damping) = self.damping {
            suspension.damping = damping;
        }
        if let Some(max_len) = self.max_len {
            suspension.max_len = max_len;
        }
    }
}

/// `player.zrd`'s constants, overridden.
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PlayerConstantsOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gravity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub water_gravity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quicksand_gravity: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quicksand_sink: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lava_sink: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_slope_cos: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<CameraConstantsOverride>,
}

impl PlayerConstantsOverride {
    fn apply(&self, player: &mut PlayerConstants) {
        macro_rules! set {
            ($field:ident) => {
                if let Some(value) = self.$field {
                    player.$field = value;
                }
            };
        }

        set!(gravity);
        set!(water_gravity);
        set!(quicksand_gravity);
        set!(quicksand_sink);
        set!(lava_sink);
        set!(max_slope_cos);

        if let Some(camera) = &self.camera {
            camera.apply(&mut player.camera);
        }
    }
}

/// `player.zrd`'s camera keys, overridden.
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct CameraConstantsOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zone: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_yaw_rate: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mouse_push: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elastic: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_tether_angle_deg: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fp_el_rate: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fp_el_lim: Option<[f32; 2]>,
}

impl CameraConstantsOverride {
    fn apply(&self, camera: &mut CameraConstants) {
        macro_rules! set {
            ($field:ident) => {
                if let Some(value) = self.$field {
                    camera.$field = value;
                }
            };
        }

        set!(zone);
        set!(max_yaw_rate);
        set!(mouse_push);
        set!(elastic);
        set!(max_tether_angle_deg);
        set!(fp_el_rate);
        set!(fp_el_lim);
    }
}

// ---------------------------------------------------------------------------
// Writing the file back
// ---------------------------------------------------------------------------

impl VehicleOverrides {
    /// Record one form's live-tuned values as overrides: every field the dev
    /// panel edits is set where `current` differs from `pristine` (the table
    /// built with no overrides at all) and cleared where it does not, so the
    /// file only ever holds real corrections. Fields the panel does not edit
    /// keep whatever the file already said.
    pub fn record_mode(
        &mut self,
        kind: &str,
        form: VehicleForm,
        current: &ModeTuning,
        pristine: &ModeTuning,
    ) {
        let def = self.kinds.entry(kind.to_string()).or_default();
        let over = def.modes.entry(form.mode_key().to_string()).or_default();

        macro_rules! record {
            ($field:ident) => {
                over.$field = (current.$field != pristine.$field).then_some(current.$field);
            };
            (opt $field:ident) => {
                over.$field = if current.$field != pristine.$field {
                    current.$field
                } else {
                    None
                };
            };
        }

        record!(max_speed);
        record!(reverse_speed);
        record!(engine_force);
        record!(rate_damping);
        record!(turns);
        record!(turn_damping);
        record!(stationary_speed);
        record!(upright_rate_air);
        record!(upright_rate_ground);
        record!(max_tilt);
        record!(static_grip);
        record!(lateral_grip);
        record!(grip_force_scale);
        record!(regrip_fraction);
        record!(air_control);
        record!(opt mass);
        record!(opt a_damping);

        let (now, was) = (&current.suspension, &pristine.suspension);
        let mut suspension = over.suspension.take().unwrap_or_default();
        suspension.rest = (now.rest != was.rest).then_some(now.rest);
        suspension.stiffness = (now.stiffness != was.stiffness).then_some(now.stiffness);
        suspension.damping = (now.damping != was.damping).then_some(now.damping);
        suspension.max_len = (now.max_len != was.max_len).then_some(now.max_len);
        over.suspension = (suspension != SuspensionOverride::default()).then_some(suspension);

        self.prune();
    }

    /// Record a kind's turret rate, the same way as [`Self::record_mode`].
    pub fn record_turret_rate(&mut self, kind: &str, current: &VehicleDef, pristine: &VehicleDef) {
        let def = self.kinds.entry(kind.to_string()).or_default();
        def.turret_rate = if current.turret_rate != pristine.turret_rate {
            current.turret_rate
        } else {
            None
        };
        self.prune();
    }

    /// Drop form and kind entries left with nothing in them.
    fn prune(&mut self) {
        for def in self.kinds.values_mut() {
            def.modes
                .retain(|_, mode| *mode != ModeTuningOverride::default());
        }
        self.kinds
            .retain(|_, def| *def != VehicleDefOverride::default());
    }

    /// This file as RON, under the comment header of `existing` (the file's
    /// current text): every leading comment and blank line is kept, and
    /// everything after it is replaced.
    pub fn to_ron_file(&self, existing: &str) -> Result<String, ron::Error> {
        let header: String = existing
            .lines()
            .take_while(|line| {
                let line = line.trim();
                line.is_empty() || line.starts_with("//")
            })
            .map(|line| format!("{line}\n"))
            .collect();
        let body = ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::default())?;

        Ok(format!("{header}{body}\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vehicle::tuning::{Difficulty, build_vehicle_table};
    use serde_json::json;

    fn parse(text: &str) -> VehicleOverrides {
        ron::de::from_str(text).expect("the RON parses")
    }

    fn table(overrides: &VehicleOverrides) -> VehicleTable {
        let mut entries = std::collections::BTreeMap::new();
        entries.insert(
            "vehicle.zrd".to_string(),
            json!([[
                "bft",
                [
                    "common_mode",
                    [
                        "health",
                        [140.0, 60.0],
                        "camback",
                        [[0.0, 4.0, 12.75], [0.0, 3.5, 2.25], [0.0, 3.5, 2.25]]
                    ],
                    "track_mode",
                    [
                        "mode",
                        "track",
                        "rates",
                        [48.0, 65.0],
                        "turns",
                        [2.5, 3.5],
                        "turn_damping",
                        12.0,
                        "rate_damping",
                        [4.0, 1.25],
                        "friction",
                        [150.0, 80.0, 0.02],
                        "mass",
                        2.2,
                        "collision_d",
                        [0.35, 0.0]
                    ]
                ]
            ]]),
        );
        entries.insert("player.zrd".to_string(), json!([["nom_gravity", 40.0]]));

        build_vehicle_table(
            &bft_assets::ReaderData::from_entries(entries),
            Difficulty::Normal,
            overrides,
        )
        .expect("the table builds")
    }

    /// The shipped file: an empty record is the correct default, and has to
    /// leave every reader value alone.
    #[test]
    fn an_empty_file_changes_nothing() {
        let empty = parse("(kinds: {}, player: None)");
        let table = table(&empty);
        let track = table
            .player_def()
            .and_then(|def| def.mode(VehicleForm::Track))
            .expect("track");

        assert_eq!(track.max_speed, 48.0);
        assert_eq!(table.player.gravity, 40.0);
    }

    #[test]
    fn an_override_wins_over_the_derived_constant_it_replaces() {
        let overrides = parse(
            r#"(
                kinds: {
                    "bft": (
                        modes: {
                            "track": (
                                max_speed: Some(30.0),
                                lateral_grip: Some(2.5),
                                suspension: Some((stiffness: Some(12345.0))),
                            ),
                        },
                    ),
                },
            )"#,
        );

        let table = table(&overrides);
        let track = table
            .player_def()
            .and_then(|def| def.mode(VehicleForm::Track))
            .expect("track");

        assert_eq!(track.max_speed, 30.0);
        assert_eq!(track.lateral_grip, 2.5);
        assert_eq!(track.suspension.stiffness, 12345.0);
        // Untouched leaves keep the derived values.
        assert_eq!(track.reverse_speed, 65.0);
        assert_eq!(track.suspension.rays.len(), 4);
    }

    #[test]
    fn a_reader_value_can_be_overridden_too() {
        let overrides = parse(
            r#"(kinds: {"bft": (health: Some(200.0), modes: {"track": (rates: Some((12.0, 20.0)))})})"#,
        );

        let table = table(&overrides);
        let def = table.player_def().expect("player");

        assert_eq!(def.health, 200.0);
        assert_eq!(
            def.mode(VehicleForm::Track).expect("track").rates,
            [12.0, 20.0]
        );
    }

    /// Overriding `rates` does **not** silently re-derive `max_speed`: the
    /// derivation ran before the overrides, so a file that wants a new top
    /// speed sets `max_speed`. Worth pinning, because the other answer is
    /// defensible and this is the one the code does.
    #[test]
    fn overriding_a_reader_value_does_not_redrive_what_was_derived_from_it() {
        let overrides =
            parse(r#"(kinds: {"bft": (modes: {"track": (rates: Some((12.0, 20.0)))})})"#);

        let track_speed = table(&overrides)
            .player_def()
            .and_then(|def| def.mode(VehicleForm::Track))
            .expect("track")
            .max_speed;

        assert_eq!(track_speed, 48.0, "still the derived value");
    }

    #[test]
    fn the_player_constants_can_be_overridden() {
        let overrides =
            parse(r#"(player: Some((gravity: Some(9.81), camera: Some((zone: Some(0.1))))))"#);

        let table = table(&overrides);

        assert_eq!(table.player.gravity, 9.81);
        assert_eq!(table.player.camera.zone, 0.1);
        // Not named, so still the reader's.
        assert_eq!(table.player.water_gravity, 9.0);
    }

    /// A tuning file that outlives a data change must say so rather than
    /// failing the load.
    #[test]
    fn a_kind_the_data_does_not_have_is_ignored_rather_than_fatal() {
        let overrides = parse(r#"(kinds: {"nosuchvehicle": (health: Some(1.0))})"#);

        let table = table(&overrides);

        assert!(table.player_def().is_some());
    }

    /// The file the repo actually ships has to parse, or the table waits for
    /// an asset that never arrives and the game starts with no vehicles. It
    /// holds real tuning now (the dev panel writes it), so only parsing and
    /// applying are checked, not its contents.
    #[test]
    fn the_shipped_file_parses_and_applies() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../assets/tuning/vehicle.ron"),
        )
        .expect("the shipped vehicle.ron is in the repo");

        let parsed: VehicleOverrides = ron::de::from_str(&text).expect("it parses");

        assert!(table(&parsed).player_def().is_some());
    }

    /// A typo in a hand-edited file should fail loudly at parse time, not
    /// silently do nothing.
    #[test]
    fn an_unknown_field_is_a_parse_error() {
        let result: Result<VehicleOverrides, _> =
            ron::de::from_str(r#"(kinds: {"bft": (helth: Some(200.0))})"#);

        assert!(result.is_err(), "a typo must not parse");
    }

    /// What the dev panel's save does: only changed fields are written, the
    /// header survives, and the result reads back to the same table.
    #[test]
    fn recorded_changes_round_trip_through_the_file() {
        let pristine = table(&VehicleOverrides::default());
        let mut live = pristine.clone();
        let def = live.kinds.get_mut(&VehicleKindId::player()).expect("bft");
        let track = def.modes.get_mut(&VehicleForm::Track).expect("track");
        track.max_speed = 55.0;
        track.grip_force_scale = 4.0;
        track.suspension.stiffness = 20000.0;

        let mut file = VehicleOverrides::default();
        file.record_mode(
            "bft",
            VehicleForm::Track,
            live.player_def().unwrap().mode(VehicleForm::Track).unwrap(),
            pristine
                .player_def()
                .unwrap()
                .mode(VehicleForm::Track)
                .unwrap(),
        );
        let text = file
            .to_ron_file("// header\n\n(kinds: {})\n")
            .expect("serialises");

        assert!(text.starts_with("// header\n\n("));
        assert!(
            !text.contains("None"),
            "unchanged fields are left out: {text}"
        );

        let reread = table(&parse(&text));
        let track = reread
            .player_def()
            .and_then(|def| def.mode(VehicleForm::Track))
            .expect("track");
        assert_eq!(track.max_speed, 55.0);
        assert_eq!(track.grip_force_scale, 4.0);
        assert_eq!(track.suspension.stiffness, 20000.0);
        assert_eq!(track.turns, [2.5, 3.5]);

        // Putting a value back to the game's own clears it from the file.
        let mut file = parse(&text);
        file.record_mode(
            "bft",
            VehicleForm::Track,
            pristine
                .player_def()
                .unwrap()
                .mode(VehicleForm::Track)
                .unwrap(),
            pristine
                .player_def()
                .unwrap()
                .mode(VehicleForm::Track)
                .unwrap(),
        );
        assert_eq!(file, VehicleOverrides::default());
    }
}
