//! The sound table and sound archive against a real Recoil install.
//!
//! Skips (not fails) when `BFT_DATA_DIR` is unset or the cache has not been
//! built. Only counts and the handful of names docs/data/sounds.md and
//! READER-SCHEMA §2.5 already publish are asserted; no game data enters the
//! repository.

use std::collections::{BTreeSet, HashMap};
use std::path::PathBuf;

use bft_assets::DataPaths;
use bft_assets::anim::{EventDataDesc, parse_anim_set};
use bft_assets::reader::{ReaderData, parse_reader_data, sounds};
use bft_assets::sounds::{read_sound_archive, wav_info};

fn paths() -> Option<DataPaths> {
    let root = bft_assets::test_support::data_root()?;
    match DataPaths::discover(Some(&root), None, None) {
        Ok(paths) if paths.cache.path.is_dir() => Some(paths),
        Ok(paths) => {
            eprintln!(
                "no extraction cache at {}; run `open-bft extract`. Skipping.",
                paths.cache.path.display()
            );
            None
        }
        Err(e) => {
            eprintln!("cannot resolve the data paths ({e}); skipping");
            None
        }
    }
}

fn cached(paths: &DataPaths, relative: &str) -> Option<Vec<u8>> {
    let path: PathBuf = paths.cache.path.join(relative);
    if !path.is_file() {
        eprintln!("{} is not in the cache; skipping", path.display());
        return None;
    }
    Some(std::fs::read(&path).expect("the cache is readable"))
}

fn global_reader(paths: &DataPaths) -> Option<ReaderData> {
    let bytes = cached(paths, "reader/zrdr.rdr.zip")?;
    Some(parse_reader_data(&bytes).expect("the global reader parses"))
}

/// 280 records in 16 sets, six names repeated across mission sets with the
/// same file each time (eight extra records) — so 272 distinct logical names.
#[test]
fn the_table_has_every_record_and_its_flags() {
    let Some(paths) = paths() else { return };
    let Some(reader) = global_reader(&paths) else {
        return;
    };

    let table = sounds::parse(&reader).expect("sounds.zrd parses");
    assert_eq!(table.len(), 272);

    // READER-SCHEMA §2.5's two examples.
    let chain = table.get("snd_chainbreak").expect("snd_chainbreak");
    assert_eq!(chain.file, "chainbreak");
    assert!(chain.positional);
    assert_eq!(chain.range, Some((60.0, 180.0)));
    assert_eq!(table.get("snd_m1obj1").expect("snd_m1obj1").file, "m1obj1");

    // Counted over the shipped file: 181 records carry `3D`, 139 `RANGE`,
    // 51 `LOOPED`, 40 `VOICE`. The distinct-name counts are a little lower
    // where a repeated name carries the flag.
    let positional = table.sounds.values().filter(|d| d.positional).count();
    let voice = table.sounds.values().filter(|d| d.voice).count();
    let looped = table.sounds.values().filter(|d| d.looped).count();
    assert!((170..=181).contains(&positional), "3D: {positional}");
    assert_eq!(voice, 40);
    assert!((45..=51).contains(&looped), "LOOPED: {looped}");

    // The one reversed range in the file, `[50, 20]`, comes out in order.
    let gunside = table.get("snd_gunside").expect("snd_gunside");
    assert_eq!(gunside.range, Some((20.0, 50.0)));

    // Every range is a sane attenuation band.
    for def in table.sounds.values() {
        if let Some((min, max)) = def.range {
            assert!(0.0 < min && min < max, "{}: {min}..{max}", def.name);
        }
        assert!(def.volume > 0.0 && def.volume <= 1.0, "{}", def.name);
    }
}

/// Every WAV is playable PCM, every record's file is in the archive, and the
/// only files no record names are the fifteen `-1`/`-2` variants.
#[test]
fn every_named_file_is_a_playable_wav_in_the_high_archive() {
    let Some(paths) = paths() else { return };
    let (Some(reader), Some(bytes)) = (
        global_reader(&paths),
        cached(&paths, "sounds/soundsh.snd.zip"),
    ) else {
        return;
    };

    let table = sounds::parse(&reader).expect("sounds.zrd parses");
    let archive = read_sound_archive(&bytes).expect("the archive reads");

    assert!(archive.rejected.is_empty(), "{:?}", archive.rejected);
    assert_eq!(archive.wavs.len(), 280);

    for wav in archive.wavs.values() {
        let info = wav_info(wav).expect("already checked");
        assert_eq!(info.channels, 1, "every sound is mono");
    }

    let named: BTreeSet<&str> = table.sounds.values().map(|d| d.file.as_str()).collect();
    for file in &named {
        assert!(
            archive.wavs.contains_key(*file),
            "{file} is not in the archive"
        );
    }

    let unnamed: Vec<&str> = archive
        .wavs
        .keys()
        .map(String::as_str)
        .filter(|stem| !named.contains(stem))
        .collect();
    assert_eq!(unnamed.len(), 15, "{unnamed:?}");
    assert!(
        unnamed
            .iter()
            .all(|stem| stem.ends_with("-1") || stem.ends_with("-2")),
        "{unnamed:?}"
    );
}

/// Every `Sound` and `SoundNode` event in all 13 levels names a sound the
/// table has. A name that failed to resolve would be a silent event.
#[test]
fn every_anim_sound_resolves_through_the_table() {
    let Some(paths) = paths() else { return };
    let Some(reader) = global_reader(&paths) else {
        return;
    };
    let table = sounds::parse(&reader).expect("sounds.zrd parses");

    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    for level in 1..=13u16 {
        let Some(bytes) = cached(&paths, &format!("anim/m{level}-anim.anim.zip")) else {
            return;
        };
        let set = parse_anim_set(&bytes).expect("a real anim archive parses");

        for def in &set.defs {
            for event in def.sequences.iter().flat_map(|seq| seq.events.iter()) {
                let EventDataDesc::Presentation { kind, value } = &event.data else {
                    continue;
                };
                if !matches!(*kind, "Sound" | "SoundNode") {
                    continue;
                }
                *counts.entry(kind).or_default() += 1;
                let name = value["name"].as_str().expect("a sound event has a name");
                assert!(table.get(name).is_some(), "m{level}: {name}");
            }
        }
    }

    // docs/data/sounds.md's addendum: 3093 `Sound` and 149 `SoundNode` events.
    assert_eq!(counts.get("Sound"), Some(&3093));
    assert_eq!(counts.get("SoundNode"), Some(&149));
}
