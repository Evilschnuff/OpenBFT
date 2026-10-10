//! The AI's checks that need a real Recoil install.
//!
//! They skip (not fail) when `BFT_DATA_DIR` is unset or the cache has not
//! been built, so `cargo test --workspace` stays green for a contributor
//! without the game. No game data enters the repository: only counts,
//! invariants and the handful of values docs/data/nodes.md and READER-SCHEMA already
//! publish are asserted.
//!
//! Build the cache first with `open-bft extract`.

use bevy::prelude::*;
use bft_assets::ReaderData;
use bft_sim::ai::table::{AI_ENTRY, aiv_entry, kind_of};
use bft_sim::ai::templates::LevelTemplates;
use bft_sim::ai::{AiTable, AttackStrategy, NetType, build_ai_table};
use bft_sim::vehicle::VehicleOverrides;
use bft_sim::vehicle::tuning::{Difficulty, VehicleKindId, VehicleTable, build_vehicle_table};
use bft_sim::weapons::{
    DEFAULT_TETHER_MAX_ALTITUDE, WeaponOverrides, WeaponTable, build_weapon_table,
};

/// The six campaign missions, which are the ones with enemies and nets.
const CAMPAIGN: [&str; 6] = ["m1", "m2", "m3", "m4", "m5", "m6"];

/// The seven multiplayer maps, which have neither.
const MULTIPLAYER: [&str; 7] = ["m7", "m8", "m9", "m10", "m11", "m12", "m13"];

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

/// The global archive: `vehicle.zrd`, `weapons.zrd`.
fn global_reader() -> Option<ReaderData> {
    archive_at(cache()?.join("reader/zrdr.rdr.zip"))
}

/// One mission's archive: `ai.zrd`, `aiv*.zrd`, `net_NN.zrd`, `net.zrd`.
fn mission_reader(mission: &str) -> Option<ReaderData> {
    archive_at(cache()?.join(format!("reader/{mission}-zrdr.rdr.zip")))
}

fn table_for(mission: &str, difficulty: Difficulty) -> Option<AiTable> {
    let reader = mission_reader(mission)?;

    match build_ai_table(&reader, difficulty, &Default::default()) {
        Ok(table) => Some(table),
        Err(e) => panic!("{mission}: {e}"),
    }
}

fn vehicles() -> Option<VehicleTable> {
    let reader = global_reader()?;

    match build_vehicle_table(&reader, Difficulty::Normal, &VehicleOverrides::default()) {
        Ok(table) => Some(table),
        Err(e) => panic!("cannot build the vehicle table: {e}"),
    }
}

fn weapons() -> Option<WeaponTable> {
    let reader = global_reader()?;

    match build_weapon_table(
        &reader,
        &WeaponOverrides::default(),
        DEFAULT_TETHER_MAX_ALTITUDE,
    ) {
        Ok(table) => Some(table),
        Err(e) => panic!("cannot build the weapon table: {e}"),
    }
}

// ---------------------------------------------------------------------------
// Counts, against the figures READER-SCHEMA publishes
// ---------------------------------------------------------------------------

/// Every mission's table builds, and the campaign/multiplayer split is the
/// one READER-SCHEMA §1 and §3.3 describe.
#[test]
fn every_mission_builds_a_table() {
    let Some(_) = cache() else { return };

    for mission in CAMPAIGN {
        let table = table_for(mission, Difficulty::Normal).expect("the cache has it");

        assert!(
            !table.nets.is_empty(),
            "{mission} is a campaign mission and must have nets"
        );
        assert!(
            table.enemies().count() > 50,
            "{mission} has only {} enemies",
            table.enemies().count()
        );
        assert!(
            table.player_start().is_some(),
            "{mission} must have a bft_00"
        );
    }

    for mission in MULTIPLAYER {
        let table = table_for(mission, Difficulty::Normal).expect("the cache has it");

        assert!(table.nets.is_empty(), "{mission} is multiplayer: no nets");
        assert_eq!(
            table.enemies().count(),
            0,
            "{mission} is multiplayer: no aiv enemies"
        );
        assert!(table.player_start().is_some());
    }
}

/// m1's figures, which READER-SCHEMA §3.2 and §3.3 state outright: 84 units
/// (83 enemies plus the player) and 91 nets.
#[test]
fn m1_matches_the_published_counts() {
    let Some(table) = table_for("m1", Difficulty::Normal) else {
        return;
    };

    assert_eq!(table.units.len(), 84, "READER-SCHEMA §3.2 says 84");
    assert_eq!(table.enemies().count(), 83);
    assert_eq!(table.nets.len(), 91, "net_01 … net_91");
}

/// "`aiv_hard` adds 3 units, `aiv_easy` removes 4" (READER-SCHEMA §3.2).
#[test]
fn difficulty_picks_a_whole_file_rather_than_scaling() {
    let Some(normal) = table_for("m1", Difficulty::Normal) else {
        return;
    };
    let easy = table_for("m1", Difficulty::Easy).expect("the cache has it");
    let hard = table_for("m1", Difficulty::Hard).expect("the cache has it");

    assert_eq!(normal.units.len(), 84);
    assert_eq!(easy.units.len(), 80, "aiv_easy removes 4");
    assert_eq!(hard.units.len(), 87, "aiv_hard adds 3");
}

/// The multiplayer maps ship `aiv.zrd` alone, so a hard game there has to
/// fall back rather than failing to load.
#[test]
fn a_multiplayer_map_has_no_difficulty_variants_and_falls_back() {
    let Some(reader) = mission_reader("m7") else {
        return;
    };

    assert!(
        reader.entry(aiv_entry(Difficulty::Hard)).is_none(),
        "m7 ships no aiv_hard.zrd"
    );

    let table = build_ai_table(&reader, Difficulty::Hard, &Default::default())
        .expect("it falls back to aiv.zrd");

    assert_eq!(table.units.len(), 1, "only bft_00");
}

/// "`net.zrd` … has 8 entries on every map" (READER-SCHEMA §3.4).
#[test]
fn every_map_has_eight_deathmatch_spawns() {
    let Some(_) = cache() else { return };

    for mission in CAMPAIGN.iter().chain(MULTIPLAYER.iter()) {
        let table = table_for(mission, Difficulty::Normal).expect("the cache has it");

        assert_eq!(
            table.deathmatch_spawns.len(),
            8,
            "{mission} has {} deathmatch spawns",
            table.deathmatch_spawns.len()
        );
    }
}

// ---------------------------------------------------------------------------
// Cross-references: the tables have to agree with each other
// ---------------------------------------------------------------------------

/// Every kind `aiv*.zrd` places must exist in `vehicle.zrd`, or the enemy
/// spawns with no physics and no weapons.
///
/// This is the check that would have caught the `rumv_easy_01` → `rumv`
/// mistake: stripping one suffix too many produces a kind the table has, so
/// the enemy spawns and is silently the wrong vehicle.
#[test]
fn every_placed_kind_exists_in_the_vehicle_table() {
    let Some(vehicles) = vehicles() else { return };

    for mission in CAMPAIGN.iter().chain(MULTIPLAYER.iter()) {
        for difficulty in [Difficulty::Easy, Difficulty::Normal, Difficulty::Hard] {
            let table = table_for(mission, difficulty).expect("the cache has it");

            for unit in &table.units {
                assert!(
                    vehicles.kind(&unit.kind).is_some(),
                    "{mission} ({difficulty:?}) places `{}`, whose kind `{}` \
                     vehicle.zrd has no entry for",
                    unit.instance,
                    unit.kind.as_str()
                );
            }
        }
    }
}

/// And every weapon `ai.zrd` names must exist in `weapons.zrd`, or a turret
/// spawns unable to fire.
#[test]
fn every_turret_weapon_exists_in_the_weapon_table() {
    let Some(weapons) = weapons() else { return };

    for mission in CAMPAIGN.iter().chain(MULTIPLAYER.iter()) {
        let table = table_for(mission, Difficulty::Normal).expect("the cache has it");

        for def in &table.turrets {
            assert!(
                weapons.index(&def.weapon).is_some(),
                "{mission}'s `{}` names weapon `{}`, which weapons.zrd lacks",
                def.glob,
                def.weapon
            );
        }
    }
}

/// The suffix rule: `rumv_easy` is a kind of its own, not a difficulty
/// variant of `rumv` (READER-SCHEMA §1).
#[test]
fn the_easy_variants_are_separate_kinds_not_difficulty_switches() {
    let Some(vehicles) = vehicles() else { return };
    let Some(table) = table_for("m1", Difficulty::Normal) else {
        return;
    };

    let kinds: Vec<&str> = table.enemies().map(|unit| unit.kind.as_str()).collect();

    assert!(
        kinds.contains(&"rumv_easy"),
        "m1 places rumv_easy units: {kinds:?}"
    );
    assert!(kinds.contains(&"rumv"), "and plain rumv ones");

    // Both are real, distinct entries with different health.
    let plain = vehicles
        .kind(&VehicleKindId("rumv".to_string()))
        .expect("rumv");
    let easy = vehicles
        .kind(&VehicleKindId("rumv_easy".to_string()))
        .expect("rumv_easy");

    assert_ne!(
        plain.health, easy.health,
        "they are different vehicles, not the same one twice"
    );

    assert_eq!(kind_of("rumv_easy_01"), "rumv_easy");
}

// ---------------------------------------------------------------------------
// The turret globs
// ---------------------------------------------------------------------------

/// The overlap the data actually has: m6, m10 and m11 carry both `a2tur**`
/// and `a2tur_**`, and the more specific entry has to win or its
/// hand-authored health and parts are unreachable.
///
/// `a2tur**` describes `sontrig1.flt` (a `turret` part, 1e9 health) and
/// `a2tur_**` describes `a2gun.flt` (no `turret` part, 30 health) — different
/// models, so first-match-wins cannot be what the original did.
#[test]
fn the_overlapping_turret_globs_resolve_to_the_specific_one() {
    let Some(table) = table_for("m6", Difficulty::Normal) else {
        return;
    };

    let (_, specific) = table
        .turret_for("a2tur_01")
        .expect("a2tur_01 matches something");
    assert_eq!(specific.glob, "a2tur_**");
    assert_eq!(specific.parts.len(), 2, "a2gun.flt has no yaw part");
    assert_eq!(specific.yaw_part(), None);

    let (_, general) = table
        .turret_for("a2tur00")
        .expect("a2tur00 matches something");
    assert_eq!(general.glob, "a2tur**");
    assert_eq!(general.parts.len(), 3, "sontrig1.flt has a turret part");
    assert!(
        !general.is_destructible(),
        "its 1e9 health cannot be shot down"
    );
}

/// Every mission defines turrets, `tur_1**`, `tur_2**`, `tur_3**`, `sam_**`
/// and the three `hel_*N` globs among them (READER-SCHEMA §3.1).
#[test]
fn every_mission_defines_the_common_turret_globs() {
    let Some(_) = cache() else { return };

    for mission in CAMPAIGN.iter().chain(MULTIPLAYER.iter()) {
        let table = table_for(mission, Difficulty::Normal).expect("the cache has it");

        assert!(
            table.turrets.len() >= 8,
            "{mission} defines only {} turret entries",
            table.turrets.len()
        );

        for glob in ["tur_1**", "sam_**", "hel_*1", "hel_*2", "hel_*3"] {
            assert!(
                table.turrets.iter().any(|def| def.glob == glob),
                "{mission} has no `{glob}` entry"
            );
        }
    }
}

/// A SAM site's `TARGETS: ["vtol"]` is what keeps it off the player, and
/// every mission's has it.
#[test]
fn every_sam_site_targets_the_vtol_rather_than_the_player() {
    let Some(_) = cache() else { return };

    for mission in CAMPAIGN.iter().chain(MULTIPLAYER.iter()) {
        let table = table_for(mission, Difficulty::Normal).expect("the cache has it");

        let sam = table
            .turrets
            .iter()
            .find(|def| def.glob == "sam_**")
            .unwrap_or_else(|| panic!("{mission} has a sam_** entry"));

        assert_eq!(sam.targets.as_deref(), Some("vtol"), "{mission}");
        assert!(!sam.targets_player(), "{mission}");
    }
}

/// The `PARTS` shapes, which decide how the yaw, pitch and muzzle resolve.
///
/// 0, 2, 3 or 4 names — never 1 — and a 3-or-more list always ends in at
/// least one `firepoint`-like muzzle.
#[test]
fn every_parts_list_is_a_shape_the_resolver_handles() {
    let Some(_) = cache() else { return };

    for mission in CAMPAIGN.iter().chain(MULTIPLAYER.iter()) {
        let table = table_for(mission, Difficulty::Normal).expect("the cache has it");

        for def in &table.turrets {
            assert!(
                matches!(def.parts.len(), 0 | 2 | 3 | 4),
                "{mission}'s `{}` has {} parts: {:?}",
                def.glob,
                def.parts.len(),
                def.parts
            );

            if def.parts.len() >= 2 {
                assert!(
                    def.pitch_part().is_some(),
                    "{mission}'s `{}` has parts but no pitch part",
                    def.glob
                );
                assert!(
                    !def.muzzle_parts().is_empty(),
                    "{mission}'s `{}` has parts but nothing to fire from",
                    def.glob
                );
            }
        }
    }
}

/// `FIRE_RATE` in `ai.zrd` is seconds between shots throughout, unlike
/// `weapons.zrd`'s bimodal column (docs/data/reader.md).
///
/// The evidence is the absence of a gap: the values run 0.06 to 30 with no
/// empty band to split on, and the SAM sites' 20–30 with 6–12 rounds of
/// `wep_9.2` is a missile every half minute, not 30 a second. This test pins
/// the range so a future mission's data that *does* have a gap is noticed.
#[test]
fn the_turret_fire_rate_range_has_no_gap_to_split_on() {
    let Some(_) = cache() else { return };

    let mut rates: Vec<f32> = Vec::new();

    for mission in CAMPAIGN.iter().chain(MULTIPLAYER.iter()) {
        let table = table_for(mission, Difficulty::Normal).expect("the cache has it");
        rates.extend(
            table
                .turrets
                .iter()
                .map(|def| def.fire_interval)
                .filter(|rate| *rate > 0.0),
        );
    }

    rates.sort_by(f32::total_cmp);
    assert!(!rates.is_empty());

    let (low, high) = (rates[0], rates[rates.len() - 1]);
    assert!(
        (0.05..=0.2f32).contains(&low) && (25.0..=35.0f32).contains(&high),
        "the range moved: {low} … {high}"
    );

    // `weapons.zrd`'s split is at 3.0 because nothing lies between 2.0 and
    // 12.0 there. Here the band is populated, so the same rule cannot apply.
    let in_the_band = rates
        .iter()
        .filter(|rate| (2.0..=12.0f32).contains(rate))
        .count();
    assert!(
        in_the_band > 0,
        "nothing in 2–12 would mean ai.zrd's column is bimodal after all"
    );
}

// ---------------------------------------------------------------------------
// The nets
// ---------------------------------------------------------------------------

/// The vocabulary READER-SCHEMA §3.3 states, and the one typo in it.
#[test]
fn the_net_vocabulary_is_the_published_one() {
    let Some(_) = cache() else { return };

    let mut strategies = [0usize; 5];
    let mut types = [0usize; 4];
    let mut total = 0usize;

    for mission in CAMPAIGN {
        let table = table_for(mission, Difficulty::Normal).expect("the cache has it");

        for net in &table.nets {
            total += 1;

            match net.net_type {
                NetType::Standard => types[0] += 1,
                NetType::Fixed => types[1] += 1,
                NetType::Defilade => types[2] += 1,
                NetType::Hide => types[3] += 1,
            }

            match net.strategy {
                Some(AttackStrategy::FollowNet) => strategies[0] += 1,
                Some(AttackStrategy::HeadOn) => strategies[1] += 1,
                Some(AttackStrategy::BackForth) => strategies[2] += 1,
                Some(AttackStrategy::Circle) => strategies[3] += 1,
                Some(AttackStrategy::Sit) => strategies[4] += 1,
                None => {}
            }
        }
    }

    assert_eq!(total, 470, "91 + 83 + 92 + 69 + 55 + 80");
    // §3.3's counts. `standard` is 430 *plus* m3's net_63, whose `type` is
    // the strategy name `follow_net` and which reads as standard.
    assert_eq!(types[0], 431, "standard, including the one typo");
    assert_eq!(types[1], 27, "fixed");
    assert_eq!(types[2], 11, "defilade");
    assert_eq!(types[3], 1, "hide");

    assert_eq!(strategies[0], 163, "follow_net");
    assert_eq!(strategies[1], 150, "head_on");
    assert_eq!(strategies[2], 98, "back_forth");
    assert_eq!(strategies[3], 27, "circle");
    assert_eq!(strategies[4], 19, "sit");
}

/// Every waypoint graph is walkable, and the dangling links in the data are
/// exactly the ones already found.
///
/// **Three nets carry links that point past the end of their own graph**:
/// m2's `net_23` (6 links, 7 nodes), m3's `net_47` (5, 7) and m6's `net_62`
/// (1, 3) — 12 links out of 11 151. They read as nets that were edited down
/// without their neighbour lists being fixed. `NetDef::neighbours` drops
/// them, which is why it filters rather than indexing, and this test pins the
/// count so a *new* dangling link — the kind that would mean the parser is
/// misreading the record — is not hidden by the known ones.
///
/// m1's `net_82` is a one-node net with no links at all; `advance` holds
/// position on it rather than looping.
#[test]
fn the_only_dangling_net_links_are_the_known_ones() {
    let Some(_) = cache() else { return };

    let mut dangling: Vec<String> = Vec::new();
    let mut nodes = 0usize;

    for mission in CAMPAIGN {
        let table = table_for(mission, Difficulty::Normal).expect("the cache has it");

        for net in &table.nets {
            assert!(
                !net.nodes.is_empty(),
                "{mission}'s {} has no waypoints",
                net.name
            );
            nodes += net.nodes.len();

            for (index, node) in net.nodes.iter().enumerate() {
                for link in node.links {
                    if link != -1 && (link < 0 || link as usize >= net.nodes.len()) {
                        dangling.push(format!(
                            "{mission}/{} node {index} → {link} of {}",
                            net.name,
                            net.nodes.len()
                        ));
                    }
                }

                // Every node carries the same flag, which is why nothing
                // reads it. A mission that ever ships another value shows up
                // here.
                assert_eq!(
                    node.flag, 12,
                    "{mission}'s {} node {index} has flag {}",
                    net.name, node.flag
                );
            }

            // And whatever the links say, the walk never leaves the graph.
            for index in 0..net.nodes.len() {
                for neighbour in net.neighbours(index) {
                    assert!(neighbour < net.nodes.len());
                }
            }
        }
    }

    assert_eq!(nodes, 3717, "the corpus moved");
    assert_eq!(
        dangling.len(),
        12,
        "the dangling links changed: {dangling:#?}"
    );
}

/// An `aiv.zrd` record's net index must name a net the mission actually has,
/// or the unit has no graph to walk and no radii to wake on.
#[test]
fn every_placed_unit_names_a_net_the_mission_has() {
    let Some(_) = cache() else { return };

    for mission in CAMPAIGN {
        let table = table_for(mission, Difficulty::Normal).expect("the cache has it");

        for unit in table.enemies() {
            let Some(index) = unit.net else {
                continue;
            };

            assert!(
                table.net(index).is_some(),
                "{mission}'s `{}` walks net {index}, which has no net_{index:02}.zrd",
                unit.instance
            );
        }
    }
}

/// "The instance's position equals the chosen net's `node_00`"
/// (READER-SCHEMA §3.2), which is what makes `nearest_node` the right way to
/// join a graph.
#[test]
fn a_units_start_is_its_nets_first_node() {
    let Some(table) = table_for("m1", Difficulty::Normal) else {
        return;
    };

    let mut checked = 0usize;
    let mut matched = 0usize;

    for unit in table.enemies() {
        let Some(net) = unit.net.and_then(|index| table.net(index)) else {
            continue;
        };
        let Some(first) = net.nodes.first() else {
            continue;
        };

        checked += 1;
        if unit.pos.distance(first.pos) < 1.0 {
            matched += 1;
        }
    }

    assert!(checked > 50, "only {checked} units to check");
    // Not every unit: several share a net, and only one of them can sit on
    // node 0. A clear majority is what §3.2 describes.
    assert!(
        matched * 2 > checked,
        "only {matched} of {checked} units start on their net's node_00"
    );
}

/// The player's start is `aiv.zrd/bft_00` and is not the `bft_00` *node*'s
/// gamez transform, which is a build-script placeholder at (2417, 0, 1117).
#[test]
fn the_player_start_is_the_reader_record_not_the_node() {
    let Some(table) = table_for("m1", Difficulty::Normal) else {
        return;
    };

    let start = table.player_start().expect("m1 has a bft_00");

    // §3.2 publishes m1's: (2222.1, 3.9, 1294.2) at 166.5 degrees.
    assert!(
        (start.pos.x - 2222.1).abs() < 0.5,
        "the start moved: {:?}",
        start.pos
    );
    assert!((start.heading - 166.5_f32.to_radians()).abs() < 1e-4);
    assert_eq!(start.net, None, "the player walks no net");

    // And it is nowhere near the placeholder node.
    assert!(
        start
            .pos
            .distance(bevy::math::Vec3::new(2417.0, 0.0, 1117.0))
            > 100.0,
        "that is the build script's placeholder, not the start"
    );
}

/// `attack_buddy` is 0 in every shipped net, which is why nothing implements
/// it. If a mission's data ever sets it, this test is the notice.
#[test]
fn attack_buddy_is_unused_in_the_shipped_data() {
    let Some(_) = cache() else { return };

    for mission in CAMPAIGN {
        let table = table_for(mission, Difficulty::Normal).expect("the cache has it");

        assert!(
            table.nets.iter().all(|net| !net.attack_buddy),
            "{mission} sets attack_buddy; the enemy AI deliberately does not \
             implement it because no shipped net exercised it"
        );
    }

    // `activate_buddy`, which *is* implemented, is set on 50 nets.
    let m1 = table_for("m1", Difficulty::Normal).expect("the cache has it");
    assert!(
        m1.nets.iter().any(|net| net.activate_buddy),
        "m1 has nets that wake as a group"
    );
}

/// `ai.zrd`'s top-level `DESTROY_ANIM` is inherited by every entry that does
/// not override it, which is what a turret's death plays.
#[test]
fn every_turret_has_a_destroy_anim() {
    let Some(_) = cache() else { return };

    for mission in CAMPAIGN.iter().chain(MULTIPLAYER.iter()) {
        let reader = mission_reader(mission).expect("the cache has it");
        assert!(reader.entry(AI_ENTRY).is_some(), "{mission} has an ai.zrd");

        let table = table_for(mission, Difficulty::Normal).expect("the cache has it");

        for def in &table.turrets {
            assert!(
                !def.destroy_anim.is_empty(),
                "{mission}'s `{}` has no destroy anim, not even the file's",
                def.glob
            );
        }
    }
}

// ---------------------------------------------------------------------------
// The template subtrees
// ---------------------------------------------------------------------------

/// Parse one level's `gamez` archive.
fn level(name: &str) -> Option<bft_assets::level::LevelData> {
    let path = cache()?.join(format!("gamez/{name}-gamez.gamez.zip"));
    let bytes = std::fs::read(&path).ok()?;

    match bft_assets::level::parse_gamez(&bytes, name) {
        Ok(level) => Some(level),
        Err(e) => panic!("{name} failed to parse: {e}"),
    }
}

/// The prototype count is every root but the World's, which is what
/// docs/data/nodes.md describes and what `spawn_level` marks `Template`.
#[test]
fn every_root_but_the_world_is_a_template() {
    let Some(_) = cache() else { return };

    for mission in CAMPAIGN.iter().chain(MULTIPLAYER.iter()) {
        let level = level(mission).expect("the cache has it");
        let templates = LevelTemplates::from_level(&level);
        let marked = bft_sim::ai::templates::template_roots(&level);

        assert_eq!(
            marked.len(),
            level.root_nodes.len() - 1,
            "{mission}: every root but the World is a prototype"
        );
        // Exactly one name repeats, on every level: `rfpg_mzl.flt`, the
        // pulse gun's muzzle flash, which is two roots with *different*
        // models rather than a duplicate. So the map is always one shorter
        // than the root list.
        assert_eq!(
            templates.len(),
            marked.len() - 1,
            "{mission}: {} roots collapsed to {} names",
            marked.len(),
            templates.len()
        );
        assert_eq!(
            LevelTemplates::all(&level, "rfpg_mzl.flt").len(),
            2,
            "{mission}: the muzzle flash is the one repeated root name"
        );
        assert!(
            (173..=219).contains(&marked.len()),
            "{mission} has {} prototypes, outside the 173–219 the note found",
            marked.len()
        );
    }
}

/// Every kind a mission places has a template to clone, or its enemies spawn
/// invisible.
///
/// This is the cross-reference that matters for the models: `aiv.zrd` and the
/// level's prototype set are built by different parts of the original's
/// toolchain and nothing guarantees they agree.
#[test]
fn every_placed_kind_has_a_template_in_its_level() {
    let Some(_) = cache() else { return };

    for mission in CAMPAIGN {
        let level = level(mission).expect("the cache has it");
        let templates = LevelTemplates::from_level(&level);
        let table = table_for(mission, Difficulty::Normal).expect("the cache has it");

        for unit in table.enemies() {
            assert!(
                templates.has(unit.kind.as_str()),
                "{mission} places `{}` but has no `{}` template subtree",
                unit.instance,
                unit.kind.as_str()
            );
        }
    }
}

/// m1's prototype set, as docs/data/nodes.md records it.
#[test]
fn m1_carries_the_prototypes_it_places_and_not_the_others() {
    let Some(level) = level("m1") else { return };
    let templates = LevelTemplates::from_level(&level);

    for present in [
        "scou",
        "scou_easy",
        "rumv",
        "rumv_easy",
        "ntank",
        "tren",
        "dron",
        "minelay",
        "lock",
        "bft",
    ] {
        assert!(templates.has(present), "m1 has a `{present}` template");
    }

    // Kinds m1 does not place, and so does not carry.
    for absent in ["htank", "subt", "arc", "stank", "ldestank"] {
        assert!(!templates.has(absent), "m1 has no `{absent}` template");
    }

    // And the pickup prototypes, cloned from the same set.
    assert!(templates.has("pu000"));
}

/// What the clone test hands its one-shot system.
type CloneArgs = (
    bft_assets::level::LevelData,
    bft_assets::axes::AxisConvention,
    u32,
);

/// Cloning a real prototype produces the subtree the enemy's model needs.
///
/// The fixture in `ai/templates.rs` can only check the shape of the result;
/// this checks it against a real 25–49 node vehicle subtree, including that
/// the clone carries no `NodeId` — two entities claiming one node id would
/// make the index ambiguous for anim and interp.
#[test]
fn cloning_a_real_enemy_prototype_reproduces_its_subtree() {
    let Some(level) = level("m1") else { return };
    let templates = LevelTemplates::from_level(&level);
    let root = templates.root("scou").expect("m1 has a scout prototype");

    let mut app = App::new();
    app.add_plugins(bevy::app::TaskPoolPlugin::default());

    let axes = bft_assets::axes::AxisConvention::GAMEZ_DEFAULT;
    let cloned = app
        .world_mut()
        .run_system_cached_with(
            move |In((level, axes, root)): In<CloneArgs>, mut commands: Commands| {
                bft_sim::ai::clone_template(
                    &mut commands,
                    &level,
                    &axes,
                    root,
                    Transform::IDENTITY,
                    "scou_01 model",
                )
            },
            (level, axes, root),
        )
        .expect("the clone system runs");

    assert!(
        cloned.len() >= 25,
        "a vehicle prototype is 25–49 nodes; this one cloned {}",
        cloned.len()
    );
    assert!(
        cloned.find("healthy").is_some(),
        "the `healthy` subtree is what is drawn"
    );
    // The scout's own turret parts, which differ per kind (docs/data/nodes.md).
    assert!(cloned.find("turret").is_some());
    assert!(cloned.find("fpnt_c").is_some(), "the scout's firepoint");

    app.update();

    let world = app.world();
    assert!(
        world.get::<bft_sim::level::NodeId>(cloned.root).is_none(),
        "a clone must not claim the prototype's node id"
    );
    assert_eq!(
        world
            .get::<bft_sim::ai::ClonedFrom>(cloned.root)
            .map(|from| from.0.clone()),
        Some("scou".to_string()),
        "and must say what it came from"
    );
}

/// An enemy's model turns its turret where the sim aims it.
///
/// `aim_turret` slewed every enemy's `Turret` from the start, and the shots
/// went where it pointed, but nothing tied the angle to the cloned model:
/// a tank could shoot sideways while driving with its gun fixed forward.
/// Every kind m6 places — it places nearly all of them — whose prototype has
/// a `turret` now turns it, and its muzzle goes round with the barrel.
#[test]
fn an_enemys_model_turret_follows_its_aim() {
    use bft_sim::ai::{EnemyTables, EnemyTemplates, SteeringTuning, spawn_enemy};
    use bft_sim::vehicle::{AimJoints, Turret};
    use bft_sim::weapons::Muzzle;

    let Some(level) = level("m6") else { return };
    let (Some(table), Some(vehicles), Some(weapons)) =
        (table_for("m6", Difficulty::Normal), vehicles(), weapons())
    else {
        return;
    };

    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bft_sim::SimPlugin);
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(
        std::time::Duration::from_secs_f64(1.0 / bft_sim::schedule::SIM_HZ),
    ));
    app.insert_resource(State::new(bft_sim::states::AppState::InGame));
    app.finish();
    app.update();

    // One of each kind, far apart and far from anything to shoot at, so no
    // brain has a target of its own to turn to.
    let mut kinds: Vec<_> = table.enemies().collect();
    kinds.sort_by(|a, b| a.kind.as_str().cmp(b.kind.as_str()));
    kinds.dedup_by(|a, b| a.kind == b.kind);
    let placements: Vec<_> = kinds.into_iter().cloned().collect();

    let hulls: Vec<(String, Entity)> = app
        .world_mut()
        .run_system_cached_with(
            move |In((level, placements, table, vehicles, weapons)): In<(
                bft_assets::level::LevelData,
                Vec<bft_sim::ai::UnitPlacement>,
                AiTable,
                VehicleTable,
                WeaponTable,
            )>,
                  mut commands: Commands| {
                let axes = bft_assets::axes::AxisConvention::GAMEZ_DEFAULT;
                let templates = LevelTemplates::from_level(&level);
                let enemy_templates = EnemyTemplates {
                    level: &level,
                    axes: &axes,
                    templates: &templates,
                };
                placements
                    .iter()
                    .enumerate()
                    .map(|(index, placement)| {
                        let mut placement = placement.clone();
                        placement.pos = Vec3::new(index as f32 * 100.0, 500.0, 0.0);
                        let hull = spawn_enemy(
                            &mut commands,
                            &placement,
                            EnemyTables {
                                vehicles: &vehicles,
                                weapons: Some(&weapons),
                                ai: &table,
                            },
                            Some(&enemy_templates),
                            SteeringTuning::default(),
                            index as u32,
                        );
                        (placement.kind.as_str().to_string(), hull)
                    })
                    .collect::<Vec<_>>()
            },
            (level, placements, table, vehicles, weapons),
        )
        .expect("the enemies spawn");

    let aim = |app: &mut App, yaw: f32| {
        for _ in 0..3 {
            for (_, hull) in &hulls {
                app.world_mut()
                    .entity_mut(*hull)
                    .insert(Turret { yaw, pitch: 0.0 });
            }
            app.update();
        }
    };

    aim(&mut app, 0.0);
    let forward: Vec<Option<Vec3>> = hulls
        .iter()
        .map(|(_, hull)| app.world().get::<Muzzle>(*hull).map(|muzzle| muzzle.0))
        .collect();

    let yaw = std::f32::consts::FRAC_PI_2;
    aim(&mut app, yaw);

    let mut turning = Vec::new();
    for ((kind, hull), forward) in hulls.iter().zip(forward) {
        let Some(joints) = app.world().get::<AimJoints>(*hull).copied() else {
            continue;
        };
        turning.push(kind.clone());

        let ring = app
            .world()
            .get::<Transform>(joints.yaw)
            .expect("the turret node")
            .rotation;
        let expected = if joints.pitch.is_some() {
            Quat::from_rotation_y(yaw)
        } else {
            Quat::from_euler(EulerRot::YXZ, yaw, 0.0, 0.0)
        };
        assert!(
            ring.angle_between(expected) < 1e-3,
            "`{kind}`'s turret is at {ring:?}, not the sim's aim"
        );

        // The minelayer's turret has no firepoint: its mines drop behind the
        // hull, and its muzzle stays where `spawn_enemy` put it.
        if app
            .world()
            .get::<bft_sim::weapons::Firepoint>(*hull)
            .is_none()
        {
            continue;
        }
        let (Some(before), Some(after)) = (
            forward,
            app.world().get::<Muzzle>(*hull).map(|muzzle| muzzle.0),
        ) else {
            continue;
        };
        assert!(
            before.distance(after) > 0.3,
            "`{kind}`'s muzzle stayed at {before} with the turret turned"
        );
    }

    // m6's tanks, trenchers, drones and RUMVs all have a `turret`.
    for kind in ["ntank", "tren", "dron", "rumv", "htank"] {
        assert!(
            turning.iter().any(|turning| turning.starts_with(kind)),
            "no `{kind}` turned its turret; turning: {turning:?}"
        );
    }
}
