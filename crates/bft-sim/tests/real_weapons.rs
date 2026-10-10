//! The weapons' checks that need a real Recoil install.
//!
//! They skip (not fail) when `BFT_DATA_DIR` is unset or the cache has not
//! been built, so `cargo test --workspace` stays green for a contributor
//! without the game. No game data enters the repository: only counts,
//! invariants and the handful of values docs/data/reader.md and READER-SCHEMA already
//! publish are asserted.
//!
//! Build the cache first with `open-bft extract`.

use bevy::math::Vec3Swizzles;
use bft_assets::ReaderData;
use bft_sim::pickups::{
    PickupEffect, TEMPLATE_ORDER, build_pickup_table, pickup_placements, template_for,
};
use bft_sim::vehicle::VehicleOverrides;
use bft_sim::vehicle::tuning::{Difficulty, VehicleKindId, build_vehicle_table};
use bft_sim::weapons::{
    DEFAULT_TETHER_MAX_ALTITUDE, KillVerb, Mover, Surface, WeaponOverrides, WeaponTable,
    build_weapon_table, launch_pitch, launch_velocity, loadout_from_vehicle,
};

/// The extraction cache, or `None` when there is no data to test against.
fn cache() -> Option<std::path::PathBuf> {
    let root = bft_assets::test_support::data_root()?;

    let paths = match bft_assets::DataPaths::discover(Some(&root), None, None) {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("cannot resolve the data paths ({e}); skipping");
            return None;
        }
    };

    if !paths.cache.path.join("reader").is_dir() {
        eprintln!(
            "no extraction cache at {}; run `open-bft extract`. Skipping.",
            paths.cache.path.display()
        );
        return None;
    }

    Some(paths.cache.path)
}

fn archive_at(path: std::path::PathBuf) -> Option<ReaderData> {
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) => {
            eprintln!("cannot read {}: {e}; skipping", path.display());
            return None;
        }
    };

    match bft_assets::reader::parse_reader_data(&bytes) {
        Ok(archive) => Some(archive),
        Err(e) => {
            eprintln!("cannot parse {}: {e}; skipping", path.display());
            None
        }
    }
}

/// The global reader archive: `weapons.zrd`, `pickup.zrd`, `vehicle.zrd`.
fn global_reader() -> Option<ReaderData> {
    archive_at(cache()?.join("reader/zrdr.rdr.zip"))
}

/// One mission's archive, which holds `puppies*.zrd`.
fn mission_reader(mission: &str) -> Option<ReaderData> {
    archive_at(cache()?.join(format!("reader/{mission}-zrdr.rdr.zip")))
}

fn table() -> Option<WeaponTable> {
    let reader = global_reader()?;

    match build_weapon_table(
        &reader,
        &WeaponOverrides::default(),
        DEFAULT_TETHER_MAX_ALTITUDE,
    ) {
        Ok(table) => Some(table),
        Err(e) => panic!("the shipped weapons.zrd must build a table: {e}"),
    }
}

/// The roster READER-SCHEMA §2.3 states: 19 weapons, slot 1–9 by variant
/// plus the enemy-only `wep_9.2`.
#[test]
fn the_shipped_data_holds_nineteen_weapons() {
    let Some(table) = table() else {
        return;
    };

    assert_eq!(table.weapons.len(), 19);

    for slot in 1..=9u8 {
        for strong in [false, true] {
            assert!(
                table.slot(slot, strong).is_some(),
                "wep_{slot}.{} is missing",
                u8::from(strong)
            );
        }
    }
    assert!(
        table.get("wep_9.2").is_some(),
        "the enemy-only small lock-on missile"
    );
    assert_eq!(table.get("wep_9.2").map(|w| w.variant), Some(2));
}

/// The four warning sounds, and the two that share a name.
#[test]
fn the_warning_sounds_are_the_ones_the_schema_names() {
    let Some(table) = table() else {
        return;
    };

    assert_eq!(table.warnings.lock_on, "snd_lo_warn");
    assert_eq!(table.warnings.no_ammo, "snd_ammo_out");
    assert_eq!(table.warnings.trigger_inactive, "snd_trigger_click");
    assert_eq!(
        table.warnings.weapon_inactive, table.warnings.trigger_inactive,
        "both inactive warnings are the same click"
    );
}

/// Is any weapon hitscan? `BEAM` is on exactly three
/// weapons, and they are the ones docs/data/reader.md names.
#[test]
fn exactly_three_weapons_are_hitscan() {
    let Some(table) = table() else {
        return;
    };

    let beams: Vec<&str> = table
        .weapons
        .iter()
        .filter(|weapon| weapon.mover == Mover::Beam)
        .map(|weapon| weapon.id.as_str())
        .collect();

    assert_eq!(beams, vec!["wep_6.0", "wep_6.1", "wep_7.1"]);

    // And a hitscan weapon has no muzzle velocity, which is what makes it
    // hitscan rather than a very fast projectile.
    for id in beams {
        assert_eq!(
            table.get(id).and_then(|weapon| weapon.velocity),
            None,
            "{id} is a beam and must not carry a VELOCITY"
        );
    }
}

/// Every weapon gets a mover, and the counts are the ones the flags imply.
#[test]
fn every_weapon_has_a_mover_and_the_mix_is_what_the_flags_say() {
    let Some(table) = table() else {
        return;
    };

    let count = |wanted: Mover| {
        table
            .weapons
            .iter()
            .filter(|weapon| weapon.mover == wanted)
            .count()
    };

    assert_eq!(count(Mover::Beam), 3, "the two lasers and the arc sabre");
    assert_eq!(count(Mover::Mine), 4, "P-HE, P-MD, R-HE, R-MD");
    // The five with a TURN_RATE, less the two TETHER_GUIDED ones: the
    // lock-on missile and nuke and wep_9.2, whose LOCK_ON is 2.0.
    assert_eq!(count(Mover::Homing), 3);
    assert_eq!(count(Mover::Tethered), 2, "wep_8.1 and wep_9.1");
    // The two mortars: the mines carry GRAVITY too but are mines first.
    assert_eq!(count(Mover::Ballistic), 2);

    assert_eq!(
        count(Mover::Straight)
            + count(Mover::Ballistic)
            + count(Mover::Homing)
            + count(Mover::Tethered)
            + count(Mover::Mine)
            + count(Mover::Beam),
        19,
        "every weapon has exactly one mover"
    );
}

/// The player's pulse gun is the only unlimited weapon, and its count does
/// not survive an f32 — the same trap as in `vehicle.zrd`.
#[test]
fn only_the_pulse_gun_is_unlimited() {
    let Some(table) = table() else {
        return;
    };

    let unlimited: Vec<&str> = table
        .weapons
        .iter()
        .filter(|weapon| weapon.is_unlimited())
        .map(|weapon| weapon.id.as_str())
        .collect();

    assert_eq!(unlimited, vec!["wep_1.0"]);
    assert_eq!(table.get("wep_1.0").map(|w| w.ammo_limit), Some(u32::MAX));
    // Every other limit is a small, exact number.
    for weapon in &table.weapons {
        if !weapon.is_unlimited() {
            assert!(
                weapon.ammo_limit <= 999,
                "{} has an odd limit of {}",
                weapon.id,
                weapon.ammo_limit
            );
        }
    }
}

/// `KILL_VERB` is on 18 of 19, and exactly one is a literal word rather than
/// a message key.
#[test]
fn one_kill_verb_is_a_literal_and_one_weapon_has_none() {
    let Some(table) = table() else {
        return;
    };

    let literals: Vec<(&str, &str)> = table
        .weapons
        .iter()
        .filter_map(|weapon| match &weapon.kill_verb {
            KillVerb::Literal(text) if !text.is_empty() => {
                Some((weapon.id.as_str(), text.as_str()))
            }
            _ => None,
        })
        .collect();

    assert_eq!(
        literals,
        vec![("wep_6.1", "executed")],
        "the laser sabre spells its verb out"
    );

    let missing: Vec<&str> = table
        .weapons
        .iter()
        .filter(|weapon| matches!(&weapon.kill_verb, KillVerb::Literal(text) if text.is_empty()))
        .map(|weapon| weapon.id.as_str())
        .collect();
    assert_eq!(missing, vec!["wep_9.2"], "the enemy missile has no verb");
}

/// Every weapon has a `default` impact block; not every one has all three
/// surfaces (`wep_7.0`, the sonic cannon, has only `default`).
#[test]
fn every_weapon_has_a_default_impact_and_most_have_three() {
    let Some(table) = table() else {
        return;
    };

    let mut all_three = 0;

    for weapon in &table.weapons {
        assert!(
            weapon.impact.contains_key(&Surface::Default),
            "{} has no default impact block",
            weapon.id
        );
        if weapon.impact.len() == 3 {
            all_three += 1;
        }
        assert!(
            weapon.impact.len() <= 3,
            "{} has {} surfaces; only three exist",
            weapon.id,
            weapon.impact.len()
        );
    }

    assert_eq!(all_three, 18, "all but the sonic cannon carry all three");
}

/// The special `IMPACT` children are read as weapon-level blocks, not as a
/// fourth surface, as the module docs record.
#[test]
fn the_special_impact_blocks_land_on_the_weapons_the_data_puts_them_on() {
    let Some(table) = table() else {
        return;
    };

    let with = |pick: fn(&bft_sim::weapons::WeaponDef) -> bool| -> Vec<&str> {
        table
            .weapons
            .iter()
            .filter(|weapon| pick(weapon))
            .map(|weapon| weapon.id.as_str())
            .collect()
    };

    assert_eq!(
        with(|w| w.designate.is_some()),
        vec!["wep_6.0"],
        "designator"
    );
    assert_eq!(with(|w| w.heat.is_some()), vec!["wep_6.1"], "laser sabre");
    assert_eq!(
        with(|w| w.freeze.is_some()),
        vec!["wep_3.0"],
        "the freon cannon, which the contract does not list"
    );
    assert_eq!(
        with(|w| w.impact_always),
        vec!["wep_9.0", "wep_9.1"],
        "both nukes"
    );
}

/// `PITCH_RATE` is real and is on the two tether-guided weapons.
#[test]
fn pitch_rate_is_on_the_tether_guided_pair() {
    let Some(table) = table() else {
        return;
    };

    let with_pitch: Vec<&str> = table
        .weapons
        .iter()
        .filter(|weapon| weapon.pitch_rate.is_some())
        .map(|weapon| weapon.id.as_str())
        .collect();
    let tethered: Vec<&str> = table
        .weapons
        .iter()
        .filter(|weapon| weapon.tether_guided)
        .map(|weapon| weapon.id.as_str())
        .collect();

    assert_eq!(with_pitch, vec!["wep_8.1", "wep_9.1"]);
    assert_eq!(with_pitch, tethered, "the same two weapons");
}

/// `LOCK_ON` is a float, not a 0/1/2 mode that would fit a `u8`.
#[test]
fn lock_on_is_a_float_with_a_fractional_value() {
    let Some(table) = table() else {
        return;
    };

    // The tether-guided pair carries 0.1, which no `u8` can hold.
    assert_eq!(table.get("wep_8.1").and_then(|w| w.lock_on), Some(0.1));
    assert_eq!(table.get("wep_9.1").and_then(|w| w.lock_on), Some(0.1));
    // And the arc sabre's 0.0 must not make it a homing missile.
    assert_eq!(table.get("wep_7.1").and_then(|w| w.lock_on), Some(0.0));
    assert_eq!(table.get("wep_7.1").map(|w| w.mover), Some(Mover::Beam));
}

/// The `FIRE_RATE` unit split of READER-SCHEMA §5.2, checked
/// against the player's own `interval` column for the same weapon.
#[test]
fn the_fire_rate_split_agrees_with_the_players_intervals() {
    let Some(reader) = global_reader() else {
        return;
    };
    let Some(table) = table() else {
        return;
    };
    let vehicles = build_vehicle_table(&reader, Difficulty::Normal, &VehicleOverrides::default())
        .expect("the shipped vehicle.zrd builds");
    let player = vehicles
        .kind(&VehicleKindId::player())
        .expect("the player's kind");

    for row in &player.weapons {
        let Some(weapon) = table.get(&row.weapon) else {
            continue;
        };
        let derived = weapon.shot_interval();

        // The two readings must land in the same order of magnitude as the
        // vehicle's own interval; a weapon read in the wrong unit is off by
        // 12x or more and this catches it.
        assert!(
            derived > 0.0 && derived < 10.0,
            "{}: derived interval {derived} is not a plausible rate",
            weapon.id
        );
        assert!(
            (derived / row.interval).clamp(0.1, 10.0) == derived / row.interval,
            "{}: derived {derived} s against the vehicle's {} s is a unit mismatch",
            weapon.id,
            row.interval
        );
    }
}

/// The player's starting loadout: the two pulse guns and nothing else.
#[test]
fn the_player_starts_with_the_two_pulse_guns() {
    let Some(reader) = global_reader() else {
        return;
    };
    let Some(table) = table() else {
        return;
    };
    let vehicles = build_vehicle_table(&reader, Difficulty::Normal, &VehicleOverrides::default())
        .expect("the shipped vehicle.zrd builds");
    let player = vehicles
        .kind(&VehicleKindId::player())
        .expect("the player's kind");

    let loadout = loadout_from_vehicle(player, &table);

    assert!(loadout.is_selectable(1), "slot 1 holds the pulse guns");
    assert_eq!(loadout.selected, 1);
    // Both variants of slot 1, and only slot 1.
    let slot1 = loadout.slot(1).expect("slot 1");
    assert!(slot1.weak.is_some() && slot1.strong.is_some());
    for slot in 2..=9u8 {
        assert!(
            !loadout.is_selectable(slot),
            "slot {slot} should start empty; the rest are pickups"
        );
    }

    // The weak one is the unlimited RFPG.
    assert_eq!(slot1.weak.map(|state| state.ammo), Some(u32::MAX));
    assert_eq!(slot1.strong.map(|state| state.ammo), Some(300));
}

/// Every enemy kind carries a weapon this table can resolve.
#[test]
fn every_enemy_weapon_row_names_a_weapon_in_the_table() {
    let Some(reader) = global_reader() else {
        return;
    };
    let Some(table) = table() else {
        return;
    };
    let vehicles = build_vehicle_table(&reader, Difficulty::Normal, &VehicleOverrides::default())
        .expect("the shipped vehicle.zrd builds");

    for (kind, def) in &vehicles.kinds {
        for row in &def.weapons {
            assert!(
                table.get(&row.weapon).is_some(),
                "{}'s row names `{}`, which weapons.zrd has no entry for",
                kind.as_str(),
                row.weapon
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pickups
// ---------------------------------------------------------------------------

/// `pickup.zrd` holds **40** kinds, not the 43 READER-SCHEMA §2.4 states,
/// and every one maps to a `puNNN` template.
#[test]
fn the_shipped_data_holds_forty_pickup_kinds() {
    let Some(reader) = global_reader() else {
        return;
    };

    let table = build_pickup_table(&reader).expect("the shipped pickup.zrd builds");

    assert_eq!(table.kinds.len(), 40);

    for kind in table.kinds.keys() {
        assert!(
            template_for(kind).is_some(),
            "`{kind}` has no `puNNN` template; the order list is out of date"
        );
    }
    // And the mapping is onto: every template is claimed by a kind.
    for name in TEMPLATE_ORDER {
        assert!(
            table.get(name).is_some(),
            "the order list names `{name}`, which pickup.zrd does not have"
        );
    }
}

/// Only the 17 gun kinds carry a HUD icon, and the icons go past `gun_12`.
#[test]
fn the_gun_pickups_carry_the_hud_icons() {
    let Some(reader) = global_reader() else {
        return;
    };
    let table = build_pickup_table(&reader).expect("it builds");

    let with_image: Vec<&str> = table
        .kinds
        .values()
        .filter(|def| def.image.is_some())
        .map(|def| def.kind.as_str())
        .collect();

    assert_eq!(with_image.len(), 17, "one per gun");
    for kind in &with_image {
        assert!(kind.ends_with("_WEAPON"), "`{kind}` is not a gun");
        assert!(matches!(
            table.get(kind).map(|def| def.effect),
            Some(PickupEffect::Weapon { .. })
        ));
    }

    // READER-SCHEMA §2.4 says `gun_01`…`gun_12`; the data goes to `gun_13`.
    assert_eq!(
        table
            .get("GUIDED_NUKE_WEAPON")
            .and_then(|def| def.image.clone())
            .as_deref(),
        Some("gun_13")
    );
}

/// Every placement in every mission names a kind the table has.
#[test]
fn every_placement_in_every_mission_resolves() {
    let Some(reader) = global_reader() else {
        return;
    };
    let table = build_pickup_table(&reader).expect("it builds");

    let mut total = 0;

    for index in 1..=13 {
        let mission = format!("m{index}");
        let Some(archive) = mission_reader(&mission) else {
            return;
        };

        for difficulty in [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard] {
            let placements = pickup_placements(&archive, difficulty)
                .unwrap_or_else(|e| panic!("{mission} {difficulty:?}: {e}"));

            assert!(
                !placements.is_empty(),
                "{mission} has no pickups at {difficulty:?}"
            );

            for placement in &placements {
                assert!(
                    table.get(&placement.kind).is_some(),
                    "{mission}: `{}` is not a pickup.zrd kind",
                    placement.kind
                );
            }
            total += placements.len();
        }
    }

    // 1225 records over 13 missions × 3 difficulties.
    assert!(total > 1000, "only {total} placements across the game");
}

/// Campaign maps never respawn their pickups; multiplayer maps do
/// (READER-SCHEMA §3.5).
#[test]
fn campaign_pickups_do_not_respawn_and_multiplayer_ones_do() {
    // m1 is a campaign map, m9 a multiplayer one.
    let Some(campaign) = mission_reader("m1") else {
        return;
    };
    let Some(multiplayer) = mission_reader("m9") else {
        return;
    };

    let campaign = pickup_placements(&campaign, Difficulty::Normal).expect("m1 reads");
    let multiplayer = pickup_placements(&multiplayer, Difficulty::Normal).expect("m9 reads");

    assert!(
        campaign.iter().all(|placement| placement.respawn.is_none()),
        "a campaign pickup is taken once"
    );
    assert!(
        multiplayer
            .iter()
            .all(|placement| placement.respawn.is_some()),
        "every deathmatch pickup comes back"
    );
    // 30, 60 or 180 seconds, and nothing else.
    for placement in &multiplayer {
        let seconds = placement.respawn.expect("checked above");
        assert!(
            [30.0, 60.0, 180.0].contains(&seconds),
            "unexpected respawn of {seconds} s"
        );
    }
}

/// The instance names objectives reference actually get generated: m13
/// places the sub chip, whose first instance is `pu03900`.
#[test]
fn a_placed_chip_gets_the_instance_name_objectives_use() {
    let Some(archive) = mission_reader("m13") else {
        return;
    };

    let placements = pickup_placements(&archive, Difficulty::Normal).expect("m13 reads");
    let named = bft_sim::pickups::name_placements(&placements);

    let chips: Vec<&str> = named
        .iter()
        .filter(|(placement, _)| placement.kind == "PUP_SUB")
        .map(|(_, name)| name.as_str())
        .collect();

    assert!(!chips.is_empty(), "m13 places a sub chip");
    assert_eq!(chips[0], "pu03900", "the first one objectives can name");
}

/// The ballistic solver against the mortars' *shipped* numbers.
///
/// `weapons/ballistics.rs`'s own tests use representative constants; this one
/// uses whatever `weapons.zrd` actually states, so a data change that puts a
/// mortar's velocity somewhere the solver cannot cope with is caught here.
#[test]
fn the_solver_hits_with_the_real_mortars() {
    let Some(table) = table() else {
        return;
    };

    let mortars: Vec<_> = table
        .weapons
        .iter()
        .filter(|weapon| weapon.mover == Mover::Ballistic)
        .collect();
    assert_eq!(mortars.len(), 2, "HEM and MDM");

    for weapon in mortars {
        let speed = weapon.velocity.expect("a ballistic weapon has a VELOCITY");
        assert!(weapon.gravity > 0.0, "{} must fall", weapon.id);

        // Well inside the weapon's own stated RANGE, so this asserts about a
        // shot the weapon is meant to be able to take.
        let distance = weapon.range * 0.5;
        let delta = bevy::prelude::Vec3::new(0.0, 0.0, -distance);

        let pitch = launch_pitch(delta, speed, weapon.gravity).unwrap_or_else(|| {
            panic!(
                "{} cannot reach {distance} m, half its own {} m range",
                weapon.id, weapon.range
            )
        });
        assert!(
            pitch > 0.0,
            "{} needs to elevate to reach {distance} m, got {pitch} rad",
            weapon.id
        );

        // Fly it through the simulation's integrator and see where it lands.
        let velocity = launch_velocity(delta, speed, weapon.gravity).expect("solved above");
        let dt = 1.0 / 60.0;
        let mut position = bevy::prelude::Vec3::ZERO;
        let mut velocity = velocity;
        let mut landed = f32::NAN;
        for _ in 0..6000 {
            let previous = position;
            velocity.y -= weapon.gravity * dt;
            position += velocity * dt;
            if position.xz().length() >= distance {
                let before = previous.xz().length();
                let after = position.xz().length();
                let t = ((distance - before) / (after - before)).clamp(0.0, 1.0);
                landed = previous.y + (position.y - previous.y) * t;
                break;
            }
        }

        // A metre over a range of hundreds: the closed form and the discrete
        // integrator disagree by about this much and the gap is what the
        // tolerance is for.
        assert!(
            landed.abs() < 1.0,
            "{} aimed flat at {distance} m landed at y = {landed}",
            weapon.id
        );
    }
}
