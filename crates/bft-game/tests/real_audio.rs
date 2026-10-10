//! The event-to-sound mapping against a real Recoil install.
//!
//! Every sound name the bridges can ask for — from `weapons.zrd`,
//! `pickup.zrd`, `vehicle.zrd` and `sounds.ron` — has to resolve through
//! `sounds.zrd` to a WAV in the bank, or that event is silent with one
//! warning. Skips (not fails) when `BFT_DATA_DIR` is unset or the cache has
//! not been built.

use std::collections::BTreeSet;

use bft_assets::reader::{ReaderData, parse_reader_data, sounds};
use bft_assets::sounds::read_sound_archive;
use bft_game::audio::SoundTuning;
use bft_sim::pickups::build_pickup_table;
use bft_sim::vehicle::VehicleOverrides;
use bft_sim::vehicle::tuning::{Difficulty, build_vehicle_table};
use bft_sim::weapons::{DEFAULT_TETHER_MAX_ALTITUDE, WeaponOverrides, build_weapon_table};

fn cached(relative: &str) -> Option<Vec<u8>> {
    let root = bft_assets::test_support::data_root()?;
    let paths = bft_assets::DataPaths::discover(Some(&root), None, None).ok()?;
    let path = paths.cache.path.join(relative);
    if !path.is_file() {
        eprintln!("{} is not in the cache; skipping", path.display());
        return None;
    }
    Some(std::fs::read(path).expect("the cache is readable"))
}

fn global_reader() -> Option<ReaderData> {
    Some(parse_reader_data(&cached("reader/zrdr.rdr.zip")?).expect("the global reader parses"))
}

#[test]
fn every_sound_the_bridges_name_is_in_the_bank() {
    let (Some(reader), Some(archive)) = (global_reader(), cached("sounds/soundsh.snd.zip")) else {
        return;
    };
    let table = sounds::parse(&reader).expect("sounds.zrd parses");
    let bank = read_sound_archive(&archive).expect("the archive reads");

    let mut names = BTreeSet::new();

    let weapons = build_weapon_table(
        &reader,
        &WeaponOverrides::default(),
        DEFAULT_TETHER_MAX_ALTITUDE,
    )
    .expect("weapons.zrd builds");
    for def in &weapons.weapons {
        names.extend(def.fire.sound.clone());
        names.extend(def.flyout.sound.clone());
        names.extend(def.bounce_sound.clone());
        for impact in def.impact.values() {
            names.extend(impact.sounds.iter().cloned());
        }
    }

    let pickups = build_pickup_table(&reader).expect("pickup.zrd builds");
    names.extend(pickups.kinds.values().filter_map(|def| def.sound.clone()));

    let vehicles = build_vehicle_table(&reader, Difficulty::Normal, &VehicleOverrides::default())
        .expect("vehicle.zrd builds");
    for def in vehicles.kinds.values() {
        names.extend(def.sounds.weapon_select.clone());
        // The swap, the sub's ping, and each form's knocks.
        names.extend(def.sounds.weapon_up.clone());
        names.extend(def.sounds.weapon_down.clone());
        names.extend(def.sounds.pinging.clone());
        for form in def.forms() {
            let Some(mode) = def.mode(form) else {
                continue;
            };
            names.extend(mode.sounds.engine.clone());
            names.extend(mode.sounds.skid.clone());
            names.extend(mode.sounds.collide.clone());
            names.extend(mode.sounds.land.clone());
        }
    }
    let (warning, _, alarm) = &vehicles.player.low_shield;
    names.extend([warning.clone(), alarm.clone()]);
    let warnings = &weapons.warnings;
    names.extend(
        [
            &warnings.lock_on,
            &warnings.no_ammo,
            &warnings.trigger_inactive,
            &warnings.weapon_inactive,
        ]
        .into_iter()
        .cloned(),
    );

    let tuning = SoundTuning::default();
    names.extend(tuning.destroyed_small.iter().cloned());
    names.extend(tuning.destroyed_large.iter().cloned());
    names.extend(tuning.form_change.values().cloned());

    // A floor, so an empty harvest cannot pass vacuously.
    assert!(names.len() > 40, "only {} names", names.len());

    let unresolved: Vec<_> = names
        .iter()
        .filter(|name| {
            table
                .get(name)
                .is_none_or(|def| !bank.wavs.contains_key(&def.file))
        })
        .collect();
    assert!(unresolved.is_empty(), "no WAV for {unresolved:?}");
}
