//! The envelope: versions, unknown fields, round trips.

use super::format::*;
use super::*;

fn minimal() -> SaveFile {
    SaveFile {
        format: FORMAT_TAG.to_string(),
        version: CURRENT_VERSION,
        header: SaveHeader {
            written_by: "open-bft test".to_string(),
            created: 1_790_000_000,
            label: "Quick save".to_string(),
            level: 1,
            level_name: "m1".to_string(),
            title_key: "MSG_CAMPAIGN1_NAME".to_string(),
            difficulty: DifficultyRecord::Hard,
            play_time: 61.5,
            fingerprint: Fingerprint {
                nodes: 3,
                node_hash: 42,
                anims: 2,
                anim_hash: 7,
            },
        },
        campaign: CampaignRecord {
            completed: vec![],
            unlocked: vec![Form::Track],
            mounted: vec!["wep_1.0".to_string()],
        },
        world: WorldRecord {
            clock: 61.5,
            ai_tick: 3690,
            ..default()
        },
        nodes: vec![NodeRecord {
            active: Some(false),
            t: Some(Transform::from_xyz(1.0, 2.0, 3.0).into()),
            ..NodeRecord::new(SaveRef::Node(12))
        }],
        craters: Vec::new(),
        player: None,
        enemies: Vec::new(),
        turrets: Vec::new(),
        pickups: Vec::new(),
        projectiles: Vec::new(),
        anims: AnimsRecord::default(),
    }
}

#[test]
fn a_save_round_trips_through_ron() {
    let save = minimal();
    let text = save_to_ron(&save).unwrap();

    assert_eq!(read_save(&text).unwrap(), save);
}

/// Empty sections and `None`s are not written: the file carries only what
/// happened.
#[test]
fn empty_sections_are_left_out() {
    let text = save_to_ron(&minimal()).unwrap();

    assert!(!text.contains("projectiles"), "{text}");
    assert!(!text.contains("opacity"), "{text}");
}

#[test]
fn the_probe_reads_the_header_alone() {
    let text = save_to_ron(&minimal()).unwrap();
    let probe = probe_save(&text).unwrap();

    assert_eq!(probe.version, CURRENT_VERSION);
    assert_eq!(probe.header.unwrap().label, "Quick save");
}

#[test]
fn a_file_that_is_not_a_save_is_refused() {
    let text = save_to_ron(&minimal())
        .unwrap()
        .replace(FORMAT_TAG, "something-else");

    assert!(matches!(read_save(&text), Err(SaveFileError::NotASave(_))));
}

/// SAVEGAME §4: a newer minor adds fields; this build ignores them.
#[test]
fn a_newer_minor_with_unknown_fields_loads() {
    let text = save_to_ron(&minimal()).unwrap();
    let text = text.replacen(
        &format!("minor: {}", CURRENT_VERSION.minor),
        &format!("minor: {}", CURRENT_VERSION.minor + 1),
        1,
    );
    // A field and a section nobody has invented yet.
    let text = text.replacen("label:", "weather: \"rain\",\n  label:", 1);
    let text = text.replacen("world:", "achievements: [1, 2, 3],\n  world:", 1);

    let loaded = read_save(&text).unwrap();

    assert_eq!(loaded.header.label, "Quick save");
    assert_eq!(
        loaded.version, CURRENT_VERSION,
        "re-saving writes this build's"
    );
}

/// SAVEGAME §4: fields an older writer did not know take their defaults.
#[test]
fn missing_sections_default() {
    let text = r#"(
        format: "open-bft-save",
        version: (major: 1, minor: 0),
        header: (level: 2, label: "old"),
    )"#;

    let loaded = read_save(text).unwrap();

    assert_eq!(loaded.header.level, 2);
    assert!(loaded.nodes.is_empty());
    assert_eq!(loaded.world.clock, 0.0);
}

#[test]
fn a_newer_major_is_refused_but_still_listed() {
    let text = save_to_ron(&minimal()).unwrap().replacen(
        &format!("major: {}", CURRENT_VERSION.major),
        &format!("major: {}", CURRENT_VERSION.major + 1),
        1,
    );

    assert!(matches!(
        read_save(&text),
        Err(SaveFileError::TooNew { .. })
    ));
    let probe = probe_save(&text).unwrap();
    assert_eq!(probe.header.unwrap().level, 1);
}

#[test]
fn saves_are_written_atomically_and_listed_newest_first() {
    let dir = std::env::temp_dir().join(format!("bft-saves-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let mut older = minimal();
    older.header.created = 100;
    older.header.label = "older".to_string();
    let mut newer = minimal();
    newer.header.created = 200;
    newer.header.label = "newer".to_string();

    write_save(&older, &new_save_path(&dir, 100)).unwrap();
    write_save(&newer, &new_save_path(&dir, 200)).unwrap();
    std::fs::write(dir.join("broken.ron"), "(not a save").unwrap();

    let list = list_saves(&dir);
    let labels: Vec<Option<&str>> = list
        .iter()
        .map(|entry| entry.header.as_ref().map(|header| header.label.as_str()))
        .collect();

    assert_eq!(labels, vec![Some("newer"), Some("older"), None]);
    assert!(
        !list[2].is_loadable(),
        "the broken one is listed, not loadable"
    );
    assert!(
        !dir.join("save-200.ron.tmp").exists(),
        "no temporary file is left behind"
    );
    assert_eq!(load_save(&list[0].path).unwrap().header.label, "newer");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The hash is pinned: a save's fingerprint must mean the same thing in
/// every build.
#[test]
fn the_fingerprint_hash_is_fnv1a() {
    let mut hash = Fnv64::default();
    hash.write(b"a");

    assert_eq!(hash.finish(), 0xaf63_dc4c_8601_ec8c);
}

/// The probe reads only the head, and strings full of brackets, commas and
/// the word `header` do not move the cut.
#[test]
fn the_probe_cuts_after_the_header() {
    let mut save = minimal();
    save.header.label = "a \"quoted\" ), header: [ ( label".to_string();
    let text = save_to_ron(&save).unwrap();

    let head = save_head(&text).expect("the writer's layout has a head");
    assert!(head.len() < text.len());
    assert!(!head.contains("campaign"), "{head}");
    assert_eq!(
        probe_save(&text).unwrap().header.unwrap().label,
        save.header.label
    );
}

/// A file laid out differently still probes, the slow way.
#[test]
fn a_file_without_the_usual_head_is_probed_whole() {
    let text = r#"(version: (major: 1, minor: 0), format: "open-bft-save")"#;

    assert!(save_head(text).is_none());
    let probe = probe_save(text).unwrap();
    assert_eq!(probe.version, SaveVersion { major: 1, minor: 0 });
    assert!(probe.header.is_none());
}

/// A nested field called `header` is not the top-level one.
#[test]
fn only_the_top_level_header_ends_the_head() {
    let text = r#"(format: "open-bft-save", world: (header: 1, x: 2), version: (major: 1, minor: 0), header: (level: 3), nodes: [])"#;

    let head = save_head(text).unwrap();
    assert!(head.ends_with("header: (level: 3))"), "{head}");
    assert_eq!(probe_save(text).unwrap().header.unwrap().level, 3);
}

/// 1.1 added the HUD's clock and score to `world.mission`; a 1.0 file has
/// neither and still reads, with the clock left for the reader to fill in.
#[test]
fn the_hud_clock_and_score_ride_in_the_mission_record() {
    let mut save = minimal();
    save.world.mission = Some(MissionRecord {
        elapsed: Some(83.5),
        score: 1200,
        ..Default::default()
    });
    let text = save_to_ron(&save).unwrap();
    let loaded = read_save(&text).unwrap();
    let mission = loaded.world.mission.expect("the record survives");
    assert_eq!(mission.elapsed, Some(83.5));
    assert_eq!(mission.score, 1200);

    let old = r#"(
        format: "open-bft-save",
        version: (major: 1, minor: 0),
        header: (level: 1),
        world: (clock: 12.0, mission: Some((objectives: [], started: true, dying: false))),
    )"#;
    let loaded = read_save(old).unwrap();
    let mission = loaded.world.mission.expect("a 1.0 mission record reads");
    assert_eq!(mission.elapsed, None);
    assert_eq!(mission.score, 0);
}
