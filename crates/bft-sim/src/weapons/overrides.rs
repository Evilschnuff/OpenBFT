//! `assets/tuning/weapons.ron`: the override layer over `weapons.zrd`.
//!
//! Every leaf is `Option`, so an empty record is "use the game's own values"
//! and the shipped file is `(weapons: {})` (DESIGN §5.3). The tree mirrors
//! [`WeaponTable`]: a map of weapon id to per-weapon overrides.
//!
//! Only the *numbers* are overridable. The effect names — animations,
//! sounds, models — are references into the level's anim and template data
//! and re-pointing one from a tuning file would name something that is not
//! there; that is a data question, not a tuning one. [`Mover`] is likewise
//! derived and not overridable: changing `gravity` or `beam` changes it, and
//! a mover that disagreed with its own flags would be a projectile with no
//! model or a beam with a velocity.

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use serde::Deserialize;

use super::{Mover, WeaponDef, WeaponTable};

/// The whole `weapons.ron` file.
#[derive(Asset, TypePath, Deserialize, Debug, Clone, Default)]
#[serde(default, deny_unknown_fields)]
pub struct WeaponOverrides {
    /// Per `weapons.zrd` id (`"wep_2.1"`).
    pub weapons: HashMap<String, WeaponDefOverride>,
    /// `WeaponSetMaxTetherAltitude`, if the interp value is to be ignored.
    pub tether_max_altitude: Option<f32>,
}

impl WeaponOverrides {
    /// Apply every override in this file to a freshly parsed table.
    ///
    /// A weapon the file names but the data does not have is a warning, not
    /// an error: a tuning file outliving a data change should say so and keep
    /// playing.
    pub fn apply(&self, table: &mut WeaponTable) {
        for (id, over) in &self.weapons {
            match table.by_id.get(id).copied() {
                Some(index) => over.apply(&mut table.weapons[index]),
                None => warn!("weapons.ron: no weapon `{id}` in the reader data; ignoring it"),
            }
        }

        if let Some(altitude) = self.tether_max_altitude {
            table.tether_max_altitude = altitude;
        }
    }
}

/// One weapon's overrides: the reader's own numbers, nothing derived.
#[derive(Deserialize, Debug, Clone, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct WeaponDefOverride {
    pub damage: Option<f32>,
    /// Rounds. `None` leaves the data's limit, including its "unlimited".
    pub ammo_limit: Option<u32>,
    pub range: Option<f32>,
    pub velocity: Option<f32>,
    /// Changing this can change the derived [`Mover`], which is recomputed.
    pub gravity: Option<f32>,
    pub acceleration: Option<f32>,
    pub turn_rate: Option<f32>,
    pub pitch_rate: Option<f32>,
    /// Raw, in whichever unit this weapon's own `FIRE_RATE` uses.
    pub fire_rate: Option<f32>,
    pub splash_radius: Option<f32>,
    pub flyout_health: Option<u32>,
    pub lock_on: Option<f32>,
    pub catches_fire: Option<bool>,
    pub multi_target: Option<bool>,
    pub reload_one: Option<bool>,
    pub remote_detonate: Option<bool>,
    pub tether_guided: Option<bool>,
}

impl WeaponDefOverride {
    /// Overwrite the fields this record names, then re-derive the mover.
    fn apply(&self, def: &mut WeaponDef) {
        macro_rules! set {
            ($($field:ident),* $(,)?) => {
                $(if let Some(value) = self.$field { def.$field = value; })*
            };
        }

        set!(
            damage,
            ammo_limit,
            range,
            gravity,
            fire_rate,
            splash_radius,
            catches_fire,
            multi_target,
            reload_one,
            remote_detonate,
            tether_guided,
        );

        macro_rules! set_some {
            ($($field:ident),* $(,)?) => {
                $(if let Some(value) = self.$field { def.$field = Some(value); })*
            };
        }

        set_some!(
            velocity,
            acceleration,
            turn_rate,
            pitch_rate,
            flyout_health,
            lock_on
        );

        // The mover is derived, never stored, so an override that changes one
        // of its inputs has to change it too.
        let homing = def.turn_rate.is_some() || def.lock_on.is_some_and(|value| value > 0.0);
        def.mover = Mover::derive(
            def.beam.is_some(),
            def.mine,
            def.tether_guided,
            homing,
            def.gravity,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weapons::{
        DEFAULT_TETHER_MAX_ALTITUDE, WEAPONS_ENTRY, WeaponOverrides, build_weapon_table,
    };

    /// The shipped file has to parse into "change nothing".
    #[test]
    fn the_shipped_file_is_an_empty_override() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/tuning/weapons.ron");
        let text = std::fs::read_to_string(&path).expect("the shipped file is in the repo");

        let overrides: WeaponOverrides =
            ron::from_str(&text).expect("the shipped weapons.ron parses");

        assert!(overrides.weapons.is_empty());
        assert_eq!(overrides.tether_max_altitude, None);
    }

    fn table() -> WeaponTable {
        let json: serde_json::Value = serde_json::from_str(
            r#"[["BALLISTICS", [
                 "wep_1.0", ["NAME", ["RFPG"], "AMMO_LIMIT", [999], "DAMAGE", [1.2],
                             "FIRE_RATE", [12.0], "RANGE", [700.0], "VELOCITY", [120.0],
                             "IMPACT_PROXIMITY", [0.0]]]]]"#,
        )
        .expect("valid test JSON");
        let archive = bft_assets::ReaderData::from_entries(
            [(WEAPONS_ENTRY.to_string(), json)].into_iter().collect(),
        );

        build_weapon_table(
            &archive,
            &WeaponOverrides::default(),
            DEFAULT_TETHER_MAX_ALTITUDE,
        )
        .expect("builds")
    }

    #[test]
    fn an_override_replaces_the_readers_value() {
        let mut table = table();
        let overrides: WeaponOverrides =
            ron::from_str(r#"(weapons: { "wep_1.0": (damage: Some(5.0), range: Some(900.0)) })"#)
                .expect("valid RON");

        overrides.apply(&mut table);

        assert_eq!(table.weapons[0].damage, 5.0);
        assert_eq!(table.weapons[0].range, 900.0);
        // Everything it did not name is the data's.
        assert_eq!(table.weapons[0].velocity, Some(120.0));
    }

    /// The mover is derived, so an override of one of its inputs has to move
    /// it too — otherwise a weapon given gravity would still fly flat.
    #[test]
    fn overriding_gravity_rederives_the_mover() {
        let mut table = table();
        assert_eq!(table.weapons[0].mover, Mover::Straight);

        let overrides: WeaponOverrides =
            ron::from_str(r#"(weapons: { "wep_1.0": (gravity: Some(9.8)) })"#).expect("valid RON");
        overrides.apply(&mut table);

        assert_eq!(table.weapons[0].mover, Mover::Ballistic);
    }

    #[test]
    fn overriding_turn_rate_makes_a_gun_home() {
        let mut table = table();

        let overrides: WeaponOverrides =
            ron::from_str(r#"(weapons: { "wep_1.0": (turn_rate: Some(1.5)) })"#)
                .expect("valid RON");
        overrides.apply(&mut table);

        assert_eq!(table.weapons[0].mover, Mover::Homing);
        assert_eq!(table.weapons[0].turn_rate, Some(1.5));
    }

    /// A tuning file that outlives a data change warns rather than failing.
    #[test]
    fn an_unknown_weapon_id_is_ignored() {
        let mut table = table();
        let overrides: WeaponOverrides =
            ron::from_str(r#"(weapons: { "wep_9.9": (damage: Some(1.0)) })"#).expect("valid RON");

        overrides.apply(&mut table);

        assert_eq!(table.weapons[0].damage, 1.2, "untouched");
    }

    #[test]
    fn the_tether_altitude_can_be_overridden() {
        let mut table = table();
        let overrides: WeaponOverrides =
            ron::from_str("(tether_max_altitude: Some(45.0))").expect("valid RON");

        overrides.apply(&mut table);

        assert_eq!(table.tether_max_altitude, 45.0);
    }
}
