//! `assets/tuning/ai.ron`: the knobs of *our* steering, and an escape hatch.
//!
//! The split this file draws is the one DESIGN §5.3 asks for. The game states
//! every parameter of *what* its AI does — activation and attack radii, dwell
//! times, pursuit standoff, return range, the strategy, the waypoint graph,
//! detection range, fire interval, ammunition — and all of it is read from
//! `ai.zrd`, `aiv*.zrd` and `net_NN.zrd`. What the game does not state is
//! anything about *how* our implementation gets there: how often a brain
//! re-perceives, how far it leads a moving target, how badly it aims, how far
//! its whiskers reach. Those are the knobs here, and each is marked
//! `// approximation` at its definition in [`super::tuning`].
//!
//! The per-net and per-glob sections at the bottom are an escape hatch, not a
//! tuning surface: they exist because a single hand-authored net or turret in
//! a 470-net, 130-entry corpus may need correcting after play, and the
//! alternative is editing game data we do not ship.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::table::{AiTable, NetDef, TurretDef};
use super::tuning::SteeringTuning;

/// The whole `ai.ron` file.
#[derive(Asset, TypePath, Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AiOverrides {
    /// How many ticks between perception passes. See
    /// [`SteeringTuning::reaction_ticks`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reaction_ticks: Option<u32>,
    /// How much of the computed lead to apply, 0..1+.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead_factor: Option<f32>,
    /// The standard deviation of the aim error, in degrees.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aim_error_deg: Option<f32>,
    /// Everything else our steering needs; see [`SteeringTuning`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steering: Option<SteeringOverride>,
    /// Per `vehicle.zrd` kind (`"scou"`, `"rumv_easy"`).
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub by_kind: HashMap<String, AiKindOverride>,
    /// Per `net_NN` index, for the handful of nets that need correcting.
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub nets: HashMap<usize, NetOverride>,
    /// Per `ai.zrd` glob, keyed exactly as the file writes it (`"tur_1**"`).
    #[serde(skip_serializing_if = "HashMap::is_empty")]
    pub turrets: HashMap<String, TurretOverride>,
}

impl AiOverrides {
    /// Apply the per-net and per-turret corrections to a freshly read table.
    ///
    /// The global knobs are not applied here: they live in
    /// [`SteeringTuning`], which is a resource rather than part of the table,
    /// because the table is per mission and the knobs are not.
    pub fn apply(&self, table: &mut AiTable) {
        for (index, over) in &self.nets {
            match table.nets.iter_mut().find(|net| net.index == *index) {
                Some(net) => over.apply(net),
                None => warn!("ai.ron: this mission has no net {index}; ignoring it"),
            }
        }

        for (glob, over) in &self.turrets {
            match table.turrets.iter_mut().find(|def| def.glob == *glob) {
                Some(def) => over.apply(def),
                None => warn!("ai.ron: this mission's ai.zrd has no `{glob}` entry; ignoring it"),
            }
        }
    }

    /// The steering tuning this file describes, over the compiled defaults.
    pub fn steering(&self) -> SteeringTuning {
        let mut tuning = SteeringTuning::default();

        if let Some(ticks) = self.reaction_ticks {
            // Zero would divide by zero in the cadence check and mean
            // "perceive every tick" anyway, which is what 1 says.
            tuning.reaction_ticks = ticks.max(1);
        }
        if let Some(factor) = self.lead_factor {
            tuning.lead_factor = factor;
        }
        if let Some(error) = self.aim_error_deg {
            tuning.aim_error_deg = error;
        }
        if let Some(over) = &self.steering {
            over.apply(&mut tuning);
        }

        tuning
    }

    /// Record the live mission-wide knobs, for the dev panel's save.
    ///
    /// Every field that differs from the compiled default is written and every
    /// one that matches is cleared, so the file only ever holds real
    /// corrections. The per-kind, per-net and per-turret sections are the
    /// file's own and are kept as they are.
    pub fn record_steering(&mut self, current: &SteeringTuning) {
        let default = SteeringTuning::default();

        macro_rules! differs {
            ($field:ident) => {
                (current.$field != default.$field).then_some(current.$field)
            };
        }

        self.reaction_ticks = differs!(reaction_ticks);
        self.lead_factor = differs!(lead_factor);
        self.aim_error_deg = differs!(aim_error_deg);

        let steering = SteeringOverride {
            los_lost_ticks: differs!(los_lost_ticks),
            arrive_radius: differs!(arrive_radius),
            whisker_length: differs!(whisker_length),
            whisker_angle_deg: differs!(whisker_angle_deg),
            whisker_height: differs!(whisker_height),
            steer_gain: differs!(steer_gain),
            strafe_period: differs!(strafe_period),
            fire_cone_deg: differs!(fire_cone_deg),
            turret_yaw_rate: differs!(turret_yaw_rate),
            turret_pitch_rate: differs!(turret_pitch_rate),
            reverse_angle_deg: differs!(reverse_angle_deg),
            reverse_max_distance: differs!(reverse_max_distance),
            min_engage_speed: differs!(min_engage_speed),
            activation_scale: differs!(activation_scale),
            roam_range: differs!(roam_range),
        };
        self.steering = (steering != SteeringOverride::default()).then_some(steering);
    }

    /// The file's text: `existing`'s leading comment block, then this.
    ///
    /// The comment header is the file's documentation and is kept; anything
    /// after it is rewritten, as `vehicle.ron`'s save does.
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

    /// One kind's overrides, or the empty record.
    pub fn for_kind(&self, kind: &str) -> AiKindOverride {
        self.by_kind.get(kind).cloned().unwrap_or_default()
    }
}

/// The rest of [`SteeringTuning`], all optional.
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SteeringOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub los_lost_ticks: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub arrive_radius: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whisker_length: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whisker_angle_deg: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub whisker_height: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub steer_gain: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub strafe_period: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fire_cone_deg: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turret_yaw_rate: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub turret_pitch_rate: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reverse_angle_deg: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reverse_max_distance: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_engage_speed: Option<f32>,
    pub activation_scale: Option<f32>,
    pub roam_range: Option<f32>,
}

impl SteeringOverride {
    fn apply(&self, tuning: &mut SteeringTuning) {
        macro_rules! set {
            ($($field:ident),* $(,)?) => {
                $(if let Some(value) = self.$field { tuning.$field = value; })*
            };
        }

        set!(
            los_lost_ticks,
            arrive_radius,
            whisker_length,
            whisker_angle_deg,
            whisker_height,
            steer_gain,
            strafe_period,
            fire_cone_deg,
            turret_yaw_rate,
            turret_pitch_rate,
            reverse_angle_deg,
            reverse_max_distance,
            min_engage_speed,
            activation_scale,
            roam_range,
        );

        tuning.los_lost_ticks = tuning.los_lost_ticks.max(1);
    }
}

/// Per-kind steering, for the kinds that need different handling.
///
/// The physics of a kind is `vehicle.ron`'s; this is only the
/// steering. A drone hovering at 20 m standoff and a trencher lobbing mortars
/// from 275 m want different lead and different whiskers, and neither number
/// is in the data.
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AiKindOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lead_factor: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aim_error_deg: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reaction_ticks: Option<u32>,
    /// Ignore the whiskers entirely, for a kind that flies or swims and has
    /// nothing to bump into at hull height.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub no_whiskers: Option<bool>,
}

impl AiKindOverride {
    /// This kind's steering, over the mission-wide values.
    pub fn apply(&self, tuning: &mut SteeringTuning) {
        if let Some(factor) = self.lead_factor {
            tuning.lead_factor = factor;
        }
        if let Some(error) = self.aim_error_deg {
            tuning.aim_error_deg = error;
        }
        if let Some(ticks) = self.reaction_ticks {
            tuning.reaction_ticks = ticks.max(1);
        }
        if self.no_whiskers == Some(true) {
            tuning.whisker_length = 0.0;
        }
    }
}

/// One net's reader values, overridden.
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct NetOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activate_rad: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attack_rad: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attack_dwell: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_pursuit_dwell: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub return_range: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pursuit: Option<(f32, f32)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path_width: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hide_times: Option<(f32, f32)>,
}

impl NetOverride {
    fn apply(&self, net: &mut NetDef) {
        macro_rules! set {
            ($($field:ident),* $(,)?) => {
                $(if let Some(value) = self.$field { net.$field = value; })*
            };
        }

        set!(
            activate_rad,
            attack_rad,
            attack_dwell,
            not_pursuit_dwell,
            return_range,
            path_width,
        );

        if let Some((min, max)) = self.pursuit {
            net.pursuit = [min, max];
        }
        if let Some((min, max)) = self.hide_times {
            net.hide_times = [min, max];
        }
    }
}

/// One `ai.zrd` entry's values, overridden.
#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TurretOverride {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detection_range: Option<f32>,
    /// Seconds between shots, in the same unit the data uses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fire_interval: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ammo: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fire_limits: Option<(f32, f32)>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fire_dwell: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub damage_modifier: Option<f32>,
}

impl TurretOverride {
    fn apply(&self, def: &mut TurretDef) {
        macro_rules! set {
            ($($field:ident),* $(,)?) => {
                $(if let Some(value) = self.$field { def.$field = value; })*
            };
        }

        set!(health, detection_range, fire_interval);

        if let Some(ammo) = self.ammo {
            def.ammo = Some(ammo);
        }
        if let Some((min, max)) = self.fire_limits {
            def.fire_limits = Some([min, max]);
        }
        if let Some(dwell) = self.fire_dwell {
            def.fire_dwell = Some(dwell);
        }
        if let Some(modifier) = self.damage_modifier {
            def.damage_modifier = Some(modifier);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> AiOverrides {
        ron::from_str(text).expect("the RON parses")
    }

    /// The shipped file: every leaf absent, and the defaults survive.
    #[test]
    fn an_empty_file_changes_nothing() {
        let overrides = parse("()");

        assert_eq!(overrides, AiOverrides::default());
        assert_eq!(overrides.steering(), SteeringTuning::default());
    }

    #[test]
    fn a_typo_is_a_parse_error_rather_than_a_silent_no_op() {
        assert!(ron::from_str::<AiOverrides>("(lead_facter: Some(1.0))").is_err());
    }

    #[test]
    fn the_global_knobs_land_on_the_steering_tuning() {
        let overrides = parse(
            r#"(
                reaction_ticks: Some(3),
                lead_factor: Some(0.5),
                aim_error_deg: Some(4.0),
                steering: Some((whisker_length: Some(30.0), steer_gain: Some(2.0))),
            )"#,
        );

        let tuning = overrides.steering();

        assert_eq!(tuning.reaction_ticks, 3);
        assert_eq!(tuning.lead_factor, 0.5);
        assert_eq!(tuning.aim_error_deg, 4.0);
        assert_eq!(tuning.whisker_length, 30.0);
        assert_eq!(tuning.steer_gain, 2.0);
        // Untouched fields keep the compiled default.
        assert_eq!(
            tuning.arrive_radius,
            SteeringTuning::default().arrive_radius
        );
    }

    /// A cadence of zero would divide by zero in the perception check.
    /// The dev panel's save: only what differs from the defaults is written,
    /// the escape-hatch sections survive, and the result reads back as the
    /// same tuning.
    #[test]
    fn recording_the_steering_round_trips_and_keeps_the_rest_of_the_file() {
        let existing = "// header\n// more\n\n(nets: { 3: (activate_rad: Some(50.0)) })\n";
        let mut file = parse(existing);

        let tuning = SteeringTuning {
            roam_range: 250.0,
            activation_scale: 1.5,
            aim_error_deg: 3.0,
            ..SteeringTuning::default()
        };
        file.record_steering(&tuning);

        let text = file.to_ron_file(existing).expect("it serialises");
        assert!(text.starts_with("// header\n// more\n"), "{text}");
        assert!(
            !text.contains("whisker_length"),
            "defaults are not written: {text}"
        );

        let back = parse(&text);
        assert_eq!(back.steering(), tuning);
        assert_eq!(back.nets[&3].activate_rad, Some(50.0));

        // And recording the defaults clears it all again.
        let mut cleared = back.clone();
        cleared.record_steering(&SteeringTuning::default());
        assert_eq!(cleared.steering, None);
        assert_eq!(cleared.aim_error_deg, None);
    }

    #[test]
    fn a_zero_cadence_is_clamped_to_every_tick() {
        assert_eq!(
            parse("(reaction_ticks: Some(0))").steering().reaction_ticks,
            1
        );
        assert_eq!(
            parse("(steering: Some((los_lost_ticks: Some(0))))")
                .steering()
                .los_lost_ticks,
            1
        );
    }

    #[test]
    fn a_kind_override_layers_over_the_mission_wide_values() {
        let overrides = parse(
            r#"(
                lead_factor: Some(0.8),
                by_kind: { "dron": (lead_factor: Some(1.0), no_whiskers: Some(true)) },
            )"#,
        );

        let mut tuning = overrides.steering();
        assert_eq!(tuning.lead_factor, 0.8);

        overrides.for_kind("dron").apply(&mut tuning);
        assert_eq!(tuning.lead_factor, 1.0);
        assert_eq!(tuning.whisker_length, 0.0, "a drone has nothing to bump");

        // A kind with no entry leaves the mission values alone.
        let mut tuning = overrides.steering();
        overrides.for_kind("scou").apply(&mut tuning);
        assert_eq!(tuning.lead_factor, 0.8);
    }

    #[test]
    fn a_net_and_a_turret_override_reach_the_table() {
        let overrides = parse(
            r#"(
                nets: { 1: (activate_rad: Some(50.0), pursuit: Some((10.0, 20.0))) },
                turrets: { "tur_1**": (health: Some(99.0), fire_interval: Some(0.5)) },
            )"#,
        );

        let mut table = AiTable {
            nets: vec![NetDef {
                index: 1,
                activate_rad: 200.0,
                pursuit: [0.0, 0.0],
                ..super::super::table::tests_support::net_def()
            }],
            turrets: vec![TurretDef {
                glob: "tur_1**".to_string(),
                health: 6.0,
                fire_interval: 0.1,
                ..super::super::table::tests_support::turret_def()
            }],
            ..default()
        };

        overrides.apply(&mut table);

        assert_eq!(table.nets[0].activate_rad, 50.0);
        assert_eq!(table.nets[0].pursuit, [10.0, 20.0]);
        assert_eq!(table.turrets[0].health, 99.0);
        assert_eq!(table.turrets[0].fire_interval, 0.5);
        // Untouched values survive.
        assert_eq!(table.turrets[0].detection_range, 300.0);
    }

    /// A file naming something this mission does not have must warn and keep
    /// playing: one `ai.ron` serves all 13 levels.
    #[test]
    fn naming_an_absent_net_or_glob_is_not_an_error() {
        let overrides = parse(
            r#"(nets: { 99: (activate_rad: Some(1.0)) },
                turrets: { "nope**": (health: Some(1.0)) })"#,
        );

        let mut table = AiTable::default();
        overrides.apply(&mut table);

        assert!(table.nets.is_empty());
    }
}
