//! `ai.zrd`, `aiv*.zrd`, `net_NN.zrd` and `net.zrd` → [`AiTable`].
//!
//! The reading is the whole of this file; what the values *do* is in the
//! sibling modules. Every key READER-SCHEMA §3.1–3.4 names is read, plus the
//! eight `ai.zrd` keys found in the shipped data that it does not
//! (`FIRE_DWELL`, `DAMAGE_MODIFIER`, `INTERSECT_BVOL`, `ACTIVATE_ON_HIT`,
//! `FIRE_ANIM`, `ALWAYS_LOOK_AT`, `DAMAGE_PART`, and `DEACTIVATE`'s
//! one-element form).

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use bft_assets::{ReaderData, ReaderValue};

use crate::vehicle::tuning::{Difficulty, VehicleKindId};

/// The per-mission `ai.zrd` entry: turret and fixed-emplacement definitions.
pub const AI_ENTRY: &str = "ai.zrd";

/// The per-mission `net.zrd` entry: the deathmatch spawn ring.
pub const NET_ENTRY: &str = "net.zrd";

/// What the AI table could not be built from.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum AiError {
    /// No `aiv*.zrd` at all, so this is not a mission archive. Every shipped
    /// mission has one, even the multiplayer maps (whose only record is the
    /// player's `bft_00`).
    #[error("the mission reader archive has no `{0}` entry")]
    NoUnitEntry(&'static str),
}

/// Which `aiv*.zrd` entry a difficulty reads.
///
/// Difficulty picks a whole file, as it does for `vehicle.zrd` and
/// `puppies.zrd` (READER-SCHEMA §1). **Only m1–m6 carry the variants**; the
/// seven multiplayer maps ship `aiv.zrd` alone, so [`build_ai_table`] falls
/// back to it.
pub fn aiv_entry(difficulty: Difficulty) -> &'static str {
    match difficulty {
        Difficulty::Easy => "aiv_easy.zrd",
        Difficulty::Normal => "aiv.zrd",
        Difficulty::Hard => "aiv_hard.zrd",
    }
}

/// The `net_NN.zrd` entry name for a 1-based net index.
pub fn net_entry(index: usize) -> String {
    format!("net_{index:02}.zrd")
}

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A net's `attack_strategy`: how the group moves while engaging.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AttackStrategy {
    /// Keep walking the waypoint graph and shoot from it. The commonest (163
    /// of 457 nets).
    FollowNet,
    /// Drive straight at the target (150).
    HeadOn,
    /// Strafe across the target's front (98).
    BackForth,
    /// Orbit at the pursuit standoff (27).
    Circle,
    /// Hold position and turn the turret (19).
    Sit,
}

impl AttackStrategy {
    /// Parse a `net_NN.zrd` `attack_strategy` value.
    pub fn parse(text: &str) -> Option<AttackStrategy> {
        Some(match text {
            "follow_net" => AttackStrategy::FollowNet,
            "head_on" => AttackStrategy::HeadOn,
            "back_forth" => AttackStrategy::BackForth,
            "circle" => AttackStrategy::Circle,
            "sit" => AttackStrategy::Sit,
            _ => return None,
        })
    }
}

/// A net's `type`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum NetType {
    /// Walks its graph (430 of 470 nets).
    #[default]
    Standard,
    /// Stays on its node (27).
    Fixed,
    /// Takes cover between attacks (11).
    Defilade,
    /// Takes cover for `hide_times` (1).
    Hide,
}

impl NetType {
    /// Parse a `net_NN.zrd` `type` value.
    ///
    /// **One net in the shipped data has `type: "follow_net"`** — m3's
    /// `net_63`, where a strategy name landed in the type field. It reads as
    /// [`Standard`](Self::Standard), which is what a `follow_net` strategy
    /// wants anyway, and the typo is reported once rather than failing the
    /// load.
    pub fn parse(text: &str) -> Option<NetType> {
        Some(match text {
            "standard" => NetType::Standard,
            "fixed" => NetType::Fixed,
            "defilade" => NetType::Defilade,
            "hide" => NetType::Hide,
            _ => return None,
        })
    }
}

/// One waypoint of a net's graph.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NetNode {
    /// World position, metres.
    pub pos: Vec3,
    /// Up to three neighbour indices into [`NetDef::nodes`]; −1 is "none".
    ///
    /// **The links are not symmetric**: m1's `net_01` has `node_00 → [1]`
    /// while `node_01 → [2, 0]`. [`NetDef::neighbours`] reads them as
    /// authored and does not symmetrise.
    pub links: [i32; 3],
    /// The record's first field. **12 in all 3717 shipped nodes**, so its
    /// meaning is still unknown and nothing reads it.
    pub flag: i32,
}

/// One `net_NN.zrd`: a group's behaviour plus its waypoint graph.
#[derive(Debug, Clone, PartialEq)]
pub struct NetDef {
    /// The 1-based index `aiv.zrd` records refer to.
    pub index: usize,
    /// `name`, which is `net_NN` in every shipped file.
    pub name: String,
    pub net_type: NetType,
    /// `path_width`: corridor half-width in metres. 10 on all but two nets.
    pub path_width: f32,
    /// `activate_rad`: how close the player has to be to wake the group, m.
    pub activate_rad: f32,
    /// `attack_rad`: how close before it opens fire, m.
    pub attack_rad: f32,
    /// `attack_dwell`: seconds between attack decisions.
    pub attack_dwell: f32,
    /// `not_pursuit_dwell`: seconds of lost contact before dropping pursuit.
    pub not_pursuit_dwell: f32,
    /// `pursuit_params` or `pursuit_range` — the two names carry the same
    /// `[min, max]` standoff and no file has both (432 vs 38).
    pub pursuit: [f32; 2],
    /// `return_range`: how far from the net before it goes home, m.
    pub return_range: f32,
    /// `hide_times`: `[min, max]` seconds in cover. `[0, 0]` on all but two.
    pub hide_times: [f32; 2],
    /// `attack_strategy`. `None` on the 13 nets that state none.
    pub strategy: Option<AttackStrategy>,
    /// `attack_buddy`: share a target with the group. **0 in every shipped
    /// net**, so nothing exercises it.
    pub attack_buddy: bool,
    /// `activate_buddy`: one member waking wakes the group (50 of 441 nets).
    pub activate_buddy: bool,
    /// The waypoint graph, in `node_NN` order. 1–97 nodes; "2–16" holds for
    /// the bulk but m3 has a 97-node net.
    pub nodes: Vec<NetNode>,
}

impl NetDef {
    /// The neighbours of a node, as indices into [`Self::nodes`].
    ///
    /// Drops the −1 padding and any index the file states that the graph does
    /// not have, so a caller can walk the links without bounds checks.
    pub fn neighbours(&self, node: usize) -> impl Iterator<Item = usize> + '_ {
        self.nodes
            .get(node)
            .into_iter()
            .flat_map(|node| node.links)
            .filter_map(|link| usize::try_from(link).ok())
            .filter(|link| *link < self.nodes.len())
    }

    /// The node nearest a world position, or `None` for an empty graph.
    pub fn nearest_node(&self, to: Vec3) -> Option<usize> {
        self.nodes
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                a.pos
                    .distance_squared(to)
                    .total_cmp(&b.pos.distance_squared(to))
            })
            .map(|(index, _)| index)
    }
}

/// One `aiv*.zrd` record: where an instance starts and which net it walks.
#[derive(Debug, Clone, PartialEq)]
pub struct UnitPlacement {
    /// The record's key: `"scou_03"`, `"rumv_easy_01"`, `"bft_00"`.
    pub instance: String,
    /// The `vehicle.zrd` kind the key's prefix names.
    ///
    /// The suffix stripped is the **trailing `_<digits>` only**, so
    /// `rumv_easy_01` is a `rumv_easy` — a separate enemy type with its own
    /// `vehicle.zrd` entry, not a difficulty variant of `rumv`
    /// (READER-SCHEMA §1).
    pub kind: VehicleKindId,
    /// The 1-based `net_NN` index, or `None` for the player's `0`.
    pub net: Option<usize>,
    /// World position, metres. y is ground height.
    pub pos: Vec3,
    /// Yaw about +Y in **radians**, converted from the record's degrees.
    pub heading: f32,
}

impl UnitPlacement {
    /// Whether this record is the player's, which is the only `bft` one.
    pub fn is_player(&self) -> bool {
        self.kind.as_str() == crate::vehicle::tuning::PLAYER_KIND
    }

    /// The transform this unit spawns at.
    pub fn transform(&self) -> Transform {
        Transform {
            translation: self.pos,
            rotation: Quat::from_rotation_y(self.heading),
            ..default()
        }
    }
}

/// One `ai.zrd` `TURRET/<glob>` entry: a fixed emplacement baked into gamez.
#[derive(Debug, Clone, PartialEq)]
pub struct TurretDef {
    /// The node-name glob this entry matches: `"tur_1**"`, `"hel_*1"`.
    pub glob: String,
    /// `HEALTH`. 1e9 means indestructible (the helicopter's guns); **0.0 is
    /// also in the data** (m6's `a3gard*`, m10's `a2tur_**`) and reads the
    /// same way — a turret that cannot be shot down, since `Health::new(0)`
    /// would be dead on arrival.
    pub health: f32,
    /// `PARTS`, as authored. See [`Self::yaw_part`] and friends for the
    /// positional reading; the list is 0, 2, 3 or 4 names long.
    pub parts: Vec<String>,
    /// `WEAPON/NAME`: a `weapons.zrd` `BALLISTICS` key.
    pub weapon: String,
    /// `WEAPON/AMMO`. `None` on the 14 entries that state none, which read as
    /// unlimited.
    pub ammo: Option<u32>,
    /// `WEAPON/FIRE_RATE` in **seconds between shots**, always.
    ///
    /// Unlike `weapons.zrd`'s `FIRE_RATE`, which is bimodal (docs/data/reader.md), this
    /// column is one unit throughout: its 0.06–30 range has no gap, and the
    /// SAM sites' 20–30 with 6–12 rounds of `wep_9.2` is a missile every half
    /// minute rather than 30 a second.
    pub fire_interval: f32,
    /// `WEAPON/FIRE_LIMITS`: `[min, max]` seconds of one burst.
    pub fire_limits: Option<[f32; 2]>,
    /// `WEAPON/DETECTION_RANGE`, metres.
    pub detection_range: f32,
    /// `WEAPON/BASE_MOVES`: the whole emplacement rotates, not just the
    /// turret part. Set on the helicopter guns and m4's `trntur**`.
    pub base_moves: bool,
    /// `WEAPON/FIRE_DWELL`: seconds held between bursts, where stated.
    pub fire_dwell: Option<f32>,
    /// `WEAPON/DAMAGE_MODIFIER`: scales this turret's outgoing damage.
    pub damage_modifier: Option<f32>,
    /// `LOS`, as authored: **0, 1 or 2, not a flag**. 0 is "ignores line of
    /// sight" on the helicopter guns; 1 and 2 appear on m6/m10's `a2tur**`
    /// and `a3gard*` and their difference is unknown, so both read as
    /// "requires line of sight" ([`Self::needs_los`]).
    pub los: Option<i64>,
    /// `MSL_LOCK`: a missile may lock onto this. Present-with-no-value in the
    /// data, which is how the reader spells a bare flag (READER-SCHEMA §0.2).
    pub missile_lock: bool,
    /// `TARGETS`: `"vtol"` on every SAM site — it shoots the set-piece VTOL,
    /// not the player.
    pub targets: Option<String>,
    /// The anim played on death: this entry's own `DESTROY_ANIM`, else the
    /// file's top-level one.
    pub destroy_anim: String,
    /// `DEACTIVATE`: the turret shuts down when this node (or `node/child`)
    /// goes inactive. m2's is `["powstation", "healthy"]`, m7's `["tower_00"]`.
    pub deactivate: Option<(String, Option<String>)>,
    /// `EFFECT`: an ambient effect name and its rate (`["lightning", 0.01]`).
    pub effect: Option<(String, f32)>,
    /// `FIRE_ANIM`: an anim played when it fires (m6's `a2tur**`).
    pub fire_anim: Option<String>,
    /// `ALWAYS_LOOK_AT`: track the player even out of detection range.
    pub always_look_at: bool,
    /// `ACTIVATE_ON_HIT`: dormant until something shoots it.
    pub activate_on_hit: bool,
    /// `INTERSECT_BVOL`: `0` on m6's four entries; meaning unknown.
    pub intersect_bvol: Option<i64>,
    /// `DAMAGE_PART`: the child node damage is billed to. One entry states
    /// `"my_butt"`, which is a developer joke and names no node.
    pub damage_part: Option<String>,
}

impl TurretDef {
    /// The `PARTS` name to yaw, or `None` for a turret with no yaw part.
    ///
    /// `PARTS` is positional — yaw, pitch, muzzle — and **the two-element
    /// form drops the yaw part**, not the muzzle: `a2tur_**`'s
    /// `["gun", "firepoint"]` matches an `a2gun.flt` subtree whose `healthy`
    /// child *is* `gun` with a `firepoint` under it, so there is nothing to
    /// traverse and only the barrel elevates.
    pub fn yaw_part(&self) -> Option<&str> {
        (self.parts.len() >= 3).then(|| self.parts[0].as_str())
    }

    /// The `PARTS` name to pitch.
    pub fn pitch_part(&self) -> Option<&str> {
        match self.parts.as_slice() {
            [_, pitch, ..] if self.parts.len() >= 3 => Some(pitch.as_str()),
            [pitch, _] => Some(pitch.as_str()),
            _ => None,
        }
    }

    /// The `PARTS` names to fire from, in order.
    ///
    /// Usually one; m6's `a3tur**` has `["turret", "gun01", "firepoint1",
    /// "firepoint2"]` and fires from both alternately.
    pub fn muzzle_parts(&self) -> &[String] {
        match self.parts.len() {
            0 | 1 => &[],
            2 => &self.parts[1..],
            _ => &self.parts[2..],
        }
    }

    /// Whether this turret needs line of sight to its target.
    ///
    /// `LOS: [0]` is the only value that means "no"; 1 and 2 both mean yes.
    pub fn needs_los(&self) -> bool {
        self.los != Some(0)
    }

    /// Whether this turret can be destroyed at all.
    ///
    /// The 1e9 and 0.0 healths are both "no" — see [`Self::health`].
    pub fn is_destructible(&self) -> bool {
        self.health > 0.0 && self.health < INDESTRUCTIBLE_HEALTH
    }

    /// Whether this turret shoots at the player, rather than at the set-piece
    /// VTOL its `TARGETS` names.
    pub fn targets_player(&self) -> bool {
        self.targets.is_none()
    }

    /// How specific this glob is, for resolving the overlaps in the data.
    ///
    /// **The globs are not disjoint**: m6 and m10–m11 carry both `a2tur**`
    /// and `a2tur_**`, and the first matches every node the second does. The
    /// two describe different models (`sontrig1.flt` with a `turret` part
    /// against `a2gun.flt` with none), so the engine cannot have been taking
    /// the file's first match — `a2tur_**` would then match nothing at all,
    /// and its hand-authored health and parts would be dead data.
    ///
    /// So the most specific glob wins, measured as the length of the literal
    /// prefix before the first wildcard: `a2tur_` (6) beats `a2tur` (5).
    /// Ties fall back to file order.
    pub fn specificity(&self) -> usize {
        self.glob.find(['*', '?']).unwrap_or(self.glob.len())
    }

    /// Whether a gamez node name matches this entry's glob.
    pub fn matches(&self, name: &str) -> bool {
        glob_matches(&self.glob, name)
    }
}

/// The `HEALTH` the data uses for "cannot be destroyed".
pub const INDESTRUCTIBLE_HEALTH: f32 = 1e9;

/// Everything the AI reads for one mission, resolved once per level load.
#[derive(Resource, Debug, Clone, Default)]
pub struct AiTable {
    /// The nets, **indexed by position, not by `net_NN` number**. Look one up
    /// through [`Self::net`], which applies the 1-based offset.
    pub nets: Vec<NetDef>,
    /// Every `aiv*.zrd` record, including the player's `bft_00`.
    pub units: Vec<UnitPlacement>,
    /// The `ai.zrd` turret entries, **already sorted most-specific first**, so
    /// [`Self::turret_for`] can take the first match.
    pub turrets: Vec<TurretDef>,
    /// `net.zrd`: the 8 deathmatch starts, as position and heading in radians.
    pub deathmatch_spawns: Vec<(Vec3, f32)>,
}

impl AiTable {
    /// One net by the 1-based index an `aiv.zrd` record states.
    pub fn net(&self, index: usize) -> Option<&NetDef> {
        self.nets.iter().find(|net| net.index == index)
    }

    /// The turret entry that governs a gamez node name.
    ///
    /// The most specific matching glob, because the data's globs overlap —
    /// see [`TurretDef::specificity`].
    pub fn turret_for(&self, name: &str) -> Option<(usize, &TurretDef)> {
        self.turrets
            .iter()
            .enumerate()
            .find(|(_, def)| def.matches(name))
    }

    /// The enemy placements: every unit that is not the player.
    pub fn enemies(&self) -> impl Iterator<Item = &UnitPlacement> + '_ {
        self.units.iter().filter(|unit| !unit.is_player())
    }

    /// The player's own placement, which is `aiv.zrd`'s `bft_00`.
    ///
    /// The **first** such record: m2's `aiv_easy.zrd` states `bft_00` twice,
    /// with identical values, and the reader keeps repeated keys in order
    /// (READER-SCHEMA §0.1).
    pub fn player_start(&self) -> Option<&UnitPlacement> {
        self.units.iter().find(|unit| unit.is_player())
    }
}

// ---------------------------------------------------------------------------
// Glob matching
// ---------------------------------------------------------------------------

/// Whether `name` matches a reader glob, in which `*` is any run of
/// characters and `?` is one.
///
/// Its own function rather than a crate: the vocabulary is 19 patterns, all of
/// the shapes `tur_1**`, `hel_*1` and `a3gard*`, and the match is case
/// sensitive because gamez node names are (READER-SCHEMA §0.6).
pub fn glob_matches(pattern: &str, name: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let name: Vec<char> = name.chars().collect();

    // Iterative backtracking rather than recursion: a `**` pattern against a
    // long name recurses once per character otherwise, and node names are
    // untrusted data.
    let (mut p, mut n) = (0usize, 0usize);
    // Where to resume if the current `*` turns out to have matched too little.
    let mut star: Option<(usize, usize)> = None;

    loop {
        match pattern.get(p) {
            Some('*') => {
                // Collapse a run of stars: `tur_1**` is `tur_1*`.
                star = Some((p, n));
                p += 1;
            }
            Some('?') if n < name.len() => {
                p += 1;
                n += 1;
            }
            Some(c) if n < name.len() && *c == name[n] => {
                p += 1;
                n += 1;
            }
            // Out of pattern with name left over, or a literal mismatch:
            // give the last star one more character and retry.
            _ => {
                if pattern.get(p).is_none() && n == name.len() {
                    return true;
                }

                match star {
                    Some((sp, sn)) if sn < name.len() => {
                        p = sp + 1;
                        n = sn + 1;
                        star = Some((sp, n));
                    }
                    _ => return false,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Build a mission's AI table from its reader archive.
///
/// `reader` is the **per-mission** archive. `overrides` is
/// `assets/tuning/ai.ron`, which corrects the parsed values; it holds only
/// our own steering knobs plus a per-glob and per-net escape hatch, because
/// everything else here is stated by the game.
///
/// A mission with no `ai.zrd` or no nets is not an error — the seven
/// multiplayer maps have no nets and every map's `ai.zrd` names globs that
/// match nothing in it — but a mission with no `aiv*.zrd` is, because that is
/// where the player start comes from.
pub fn build_ai_table(
    reader: &ReaderData,
    difficulty: Difficulty,
    overrides: &super::AiOverrides,
) -> Result<AiTable, AiError> {
    let mut table = AiTable {
        nets: read_nets(reader),
        units: read_units(reader, difficulty)?,
        turrets: read_turrets(reader),
        deathmatch_spawns: read_deathmatch_spawns(reader),
    };

    overrides.apply(&mut table);

    Ok(table)
}

/// Read every `net_NN.zrd` the archive holds, in index order.
///
/// The indices are not contiguous — m1 uses 65 of 91, and `aiv.zrd` names
/// numbers no file defines — so the scan is over the archive's own entry
/// names rather than a 1..N loop.
fn read_nets(reader: &ReaderData) -> Vec<NetDef> {
    let mut nets: Vec<NetDef> = reader
        .names()
        .filter_map(net_index_of)
        .filter_map(|(name, index)| {
            let value = reader.read(&name)?;
            parse_net(index, value)
        })
        .collect();

    nets.sort_by_key(|net| net.index);
    nets
}

/// `"net_07.zrd"` → `("net_07.zrd", 7)`, and `None` for anything else.
///
/// `net.zrd` itself is excluded: it is the deathmatch ring, not a path net
/// (READER-SCHEMA §3.4), and has no number to parse.
fn net_index_of(entry: &str) -> Option<(String, usize)> {
    let digits = entry.strip_prefix("net_")?.strip_suffix(".zrd")?;
    let index = digits.parse::<usize>().ok()?;

    Some((entry.to_string(), index))
}

/// One `net_NN.zrd`.
fn parse_net(index: usize, value: ReaderValue<'_>) -> Option<NetDef> {
    let float = |key: &str| value.get(key).and_then(|v| v.float());
    let pair = |key: &str| value.get(key).and_then(|v| v.float_array::<2>());
    let flag = |key: &str| value.get(key).and_then(|v| v.flag()).unwrap_or(false);

    let name = value
        .get("name")
        .and_then(|v| v.string())
        .unwrap_or("")
        .to_string();

    let net_type = match value.get("type").and_then(|v| v.string()) {
        Some(text) => NetType::parse(text).unwrap_or_else(|| {
            // m3's net_63 has a strategy name here. Loud once, then on.
            warn!(
                "{}: type `{text}` is not a net type; reading it as standard",
                net_entry(index)
            );
            NetType::Standard
        }),
        None => NetType::Standard,
    };

    let strategy = value
        .get("attack_strategy")
        .and_then(|v| v.string())
        .and_then(AttackStrategy::parse);

    // The two spellings of the same key; no shipped file has both.
    let pursuit = pair("pursuit_params")
        .or_else(|| pair("pursuit_range"))
        .unwrap_or([0.0, 0.0]);

    let mut nodes = Vec::new();
    // In file order, which is `node_00`, `node_01`, …: the link indices are
    // positions in this list, so the order is the graph.
    for (key, record) in value.pairs() {
        if !key.starts_with("node_") {
            continue;
        }
        if let Some(node) = parse_net_node(record) {
            nodes.push(node);
        } else {
            warn!(
                "{}: `{key}` is not a waypoint record; skipping",
                net_entry(index)
            );
        }
    }

    Some(NetDef {
        index,
        name,
        net_type,
        path_width: float("path_width").unwrap_or(DEFAULT_PATH_WIDTH),
        activate_rad: float("activate_rad").unwrap_or(0.0),
        attack_rad: float("attack_rad").unwrap_or(0.0),
        attack_dwell: float("attack_dwell").unwrap_or(0.0),
        not_pursuit_dwell: float("not_pursuit_dwell").unwrap_or(0.0),
        pursuit,
        return_range: float("return_range").unwrap_or(0.0),
        hide_times: pair("hide_times").unwrap_or([0.0, 0.0]),
        strategy,
        attack_buddy: flag("attack_buddy"),
        activate_buddy: flag("activate_buddy"),
        nodes,
    })
}

/// The `path_width` a net that states none gets. Every shipped net states it.
pub const DEFAULT_PATH_WIDTH: f32 = 10.0;

/// One `node_NN`: `[flag, [x,y,z], [l0, l1, l2]]`.
fn parse_net_node(record: ReaderValue<'_>) -> Option<NetNode> {
    let flag = record.nth(0)?.int()? as i32;
    let [x, y, z] = record.nth(1)?.float_array::<3>()?;
    let links = record.nth(2)?;

    let mut out = [-1i32; 3];
    for (slot, link) in out.iter_mut().zip(links.items()) {
        *slot = link.int().unwrap_or(-1) as i32;
    }

    Some(NetNode {
        pos: Vec3::new(x, y, z),
        links: out,
        flag,
    })
}

/// Read the difficulty's `aiv*.zrd`, falling back to `aiv.zrd`.
///
/// The fallback is the shape of the data, not generosity: m7–m13 ship
/// `aiv.zrd` alone (READER-SCHEMA §1).
fn read_units(reader: &ReaderData, difficulty: Difficulty) -> Result<Vec<UnitPlacement>, AiError> {
    let wanted = aiv_entry(difficulty);
    let entry = reader
        .read(wanted)
        .or_else(|| reader.read(aiv_entry(Difficulty::Normal)))
        .ok_or(AiError::NoUnitEntry(wanted))?;

    Ok(entry
        .pairs()
        .into_iter()
        .filter_map(|(instance, record)| parse_placement(instance, record))
        .collect())
}

/// One `aiv.zrd` record: `<kind>_<NN>: [net_index, [x,y,z], heading_deg]`.
fn parse_placement(instance: &str, record: ReaderValue<'_>) -> Option<UnitPlacement> {
    let net = record.nth(0)?.int()?;
    let [x, y, z] = record.nth(1)?.float_array::<3>()?;
    let heading_deg = record.nth(2)?.float()?;

    Some(UnitPlacement {
        instance: instance.to_string(),
        kind: VehicleKindId(kind_of(instance).to_string()),
        // 0 is "no net", which is the player's value and nobody else's.
        net: (net > 0).then_some(net as usize),
        pos: Vec3::new(x, y, z),
        heading: heading_deg.to_radians(),
    })
}

/// The `vehicle.zrd` kind an `aiv.zrd` instance key names.
///
/// Strips the trailing `_<digits>` and nothing else, so `rumv_easy_01` is a
/// `rumv_easy`. Public because the same rule has to hold wherever an instance
/// name is resolved.
pub fn kind_of(instance: &str) -> &str {
    match instance.rsplit_once('_') {
        Some((kind, suffix))
            if !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit()) =>
        {
            kind
        }
        _ => instance,
    }
}

/// Read `ai.zrd`'s turret entries, most specific first.
fn read_turrets(reader: &ReaderData) -> Vec<TurretDef> {
    let Some(entry) = reader.read(AI_ENTRY) else {
        return Vec::new();
    };

    let fallback_anim = entry
        .get("DESTROY_ANIM")
        .and_then(|v| v.string())
        .unwrap_or("")
        .to_string();

    let Some(turrets) = entry.get("TURRET") else {
        return Vec::new();
    };

    let mut defs: Vec<TurretDef> = turrets
        .pairs()
        .into_iter()
        .filter_map(|(glob, value)| parse_turret(glob, value, &fallback_anim))
        .collect();

    // Most specific first, ties in file order: `sort_by_key` is stable, so
    // reversing the key preserves the original order within a specificity.
    defs.sort_by_key(|def| std::cmp::Reverse(def.specificity()));
    defs
}

/// One `TURRET/<glob>` entry.
fn parse_turret(glob: &str, value: ReaderValue<'_>, fallback_anim: &str) -> Option<TurretDef> {
    let weapon_block = value.get("WEAPON");
    let weapon = |key: &str| weapon_block.and_then(|block| block.get(key));

    let name = weapon("NAME").and_then(|v| v.string())?.to_string();

    let deactivate = value.get("DEACTIVATE").and_then(|v| {
        let names = v.strings();
        Some((
            names.first()?.to_string(),
            names.get(1).map(|name| name.to_string()),
        ))
    });

    let effect = value.get("EFFECT").and_then(|v| {
        Some((
            v.nth(0)?.string()?.to_string(),
            v.nth(1).and_then(|rate| rate.float()).unwrap_or(0.0),
        ))
    });

    Some(TurretDef {
        glob: glob.to_string(),
        health: value.get("HEALTH").and_then(|v| v.float()).unwrap_or(0.0),
        parts: value
            .get("PARTS")
            .map(|v| v.strings().into_iter().map(str::to_string).collect())
            .unwrap_or_default(),
        weapon: name,
        // Not `u32::MAX`: an absent `AMMO` is a turret with no stated
        // magazine, and what that should mean is the loadout's decision.
        ammo: weapon("AMMO")
            .and_then(|v| v.int())
            .map(|ammo| ammo.max(0) as u32),
        fire_interval: weapon("FIRE_RATE").and_then(|v| v.float()).unwrap_or(0.0),
        fire_limits: weapon("FIRE_LIMITS").and_then(|v| v.float_array::<2>()),
        detection_range: weapon("DETECTION_RANGE")
            .and_then(|v| v.float())
            .unwrap_or(0.0),
        base_moves: weapon("BASE_MOVES").and_then(|v| v.flag()).unwrap_or(false),
        fire_dwell: weapon("FIRE_DWELL").and_then(|v| v.float()),
        damage_modifier: weapon("DAMAGE_MODIFIER").and_then(|v| v.float()),
        los: value.get("LOS").and_then(|v| v.int()),
        // `has`, not `flag`: the key carries `null` in every entry that has
        // it, which is how the reader spells a bare flag.
        missile_lock: value.has("MSL_LOCK"),
        targets: value
            .get("TARGETS")
            .and_then(|v| v.string())
            .map(str::to_string),
        destroy_anim: value
            .get("DESTROY_ANIM")
            .and_then(|v| v.string())
            .unwrap_or(fallback_anim)
            .to_string(),
        deactivate,
        effect,
        fire_anim: value
            .get("FIRE_ANIM")
            .and_then(|v| v.string())
            .map(str::to_string),
        always_look_at: value
            .get("ALWAYS_LOOK_AT")
            .and_then(|v| v.flag())
            .unwrap_or(false),
        activate_on_hit: value
            .get("ACTIVATE_ON_HIT")
            .and_then(|v| v.float())
            .is_some_and(|value| value != 0.0),
        intersect_bvol: value.get("INTERSECT_BVOL").and_then(|v| v.int()),
        damage_part: value
            .get("DAMAGE_PART")
            .and_then(|v| v.string())
            .map(str::to_string),
    })
}

/// Read `net.zrd`'s 8 deathmatch starts.
///
/// Every map has one, campaign maps included (READER-SCHEMA §3.4).
fn read_deathmatch_spawns(reader: &ReaderData) -> Vec<(Vec3, f32)> {
    let Some(entry) = reader.read(NET_ENTRY) else {
        return Vec::new();
    };

    entry
        .items()
        .iter()
        .filter_map(|record| {
            let [x, y, z, heading_deg] = record.float_array::<4>()?;
            Some((Vec3::new(x, y, z), heading_deg.to_radians()))
        })
        .collect()
}

/// Every level's turret nodes, matched against a table's globs.
///
/// Returns the node name, the entity and the index of the [`TurretDef`] that
/// governs it. Separate from the spawning so a test can check the matching
/// without a world, and so `bft-game` can report a count.
pub fn match_turret_nodes<'a>(
    table: &AiTable,
    nodes: impl IntoIterator<Item = (&'a str, Entity)>,
) -> Vec<(String, Entity, usize)> {
    nodes
        .into_iter()
        .filter_map(|(name, entity)| {
            let (index, _) = table.turret_for(name)?;
            Some((name.to_string(), entity, index))
        })
        .collect()
}

/// How many nodes each glob matched, for the load-time report.
pub fn turret_match_census(matches: &[(String, Entity, usize)]) -> HashMap<usize, usize> {
    let mut census: HashMap<usize, usize> = HashMap::default();

    for (_, _, index) in matches {
        *census.entry(*index).or_default() += 1;
    }

    census
}

/// Shared fixtures for this crate's tests.
///
/// The override tests need a [`NetDef`] and a [`TurretDef`] to apply to, and
/// both have enough fields that spelling them out twice would let the two
/// copies drift.
#[cfg(test)]
pub mod tests_support {
    use super::*;

    /// A net with m1's `net_01` values and an empty graph.
    pub fn net_def() -> NetDef {
        NetDef {
            index: 1,
            name: "net_01".to_string(),
            net_type: NetType::Standard,
            path_width: DEFAULT_PATH_WIDTH,
            activate_rad: 200.0,
            attack_rad: 200.0,
            attack_dwell: 0.7,
            not_pursuit_dwell: 2.0,
            pursuit: [0.0, 0.0],
            return_range: 100.0,
            hide_times: [0.0, 0.0],
            strategy: Some(AttackStrategy::FollowNet),
            attack_buddy: false,
            activate_buddy: false,
            nodes: Vec::new(),
        }
    }

    /// A turret with m1's `tur_1**` values and no parts.
    pub fn turret_def() -> TurretDef {
        TurretDef {
            glob: "tur_1**".to_string(),
            health: 6.0,
            parts: Vec::new(),
            weapon: "wep_1.0".to_string(),
            ammo: Some(100),
            fire_interval: 0.1,
            fire_limits: Some([0.8, 2.5]),
            detection_range: 300.0,
            base_moves: false,
            fire_dwell: None,
            damage_modifier: None,
            los: None,
            missile_lock: false,
            targets: None,
            destroy_anim: "destroy_turret".to_string(),
            deactivate: None,
            effect: None,
            fire_anim: None,
            always_look_at: false,
            activate_on_hit: false,
            intersect_bvol: None,
            damage_part: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::{net_def, turret_def as turret};
    use super::*;
    use serde_json::Value;

    fn value(text: &str) -> Value {
        serde_json::from_str(text).expect("valid test JSON")
    }

    #[test]
    fn a_glob_matches_the_shapes_the_data_uses() {
        assert!(glob_matches("tur_1**", "tur_101"));
        assert!(glob_matches("tur_1**", "tur_111"));
        assert!(!glob_matches("tur_1**", "tur_201"));
        assert!(glob_matches("hel_*1", "hel_11"));
        assert!(glob_matches("hel_*1", "hel_21"));
        assert!(!glob_matches("hel_*1", "hel_12"));
        assert!(glob_matches("a3gard*", "a3gard1"));
        assert!(glob_matches("sam_**", "sam_01"));
        // Exact, and case sensitive.
        assert!(glob_matches("tower_00", "tower_00"));
        assert!(!glob_matches("tur_1**", "TUR_101"));
        // A star may match nothing.
        assert!(glob_matches("a2tur**", "a2tur"));
    }

    /// The overlap the data actually has: `a2tur**` matches every node
    /// `a2tur_**` does, and the more specific entry has to win or its
    /// hand-authored health and parts are unreachable.
    #[test]
    fn the_more_specific_glob_wins_the_overlap() {
        let json = value(
            r#"[["DESTROY_ANIM", ["destroy_turret"], "TURRET", [
                 "a2tur**",  ["HEALTH", [1e9], "PARTS", ["turret", "gun", "firepoint"],
                              "WEAPON", ["NAME", ["wep_7.0"], "DETECTION_RANGE", [300]]],
                 "a2tur_**", ["HEALTH", [30], "PARTS", ["gun", "firepoint"],
                              "WEAPON", ["NAME", ["wep_1.1"], "DETECTION_RANGE", [98]]]
               ]]]"#,
        );
        let archive =
            ReaderData::from_entries([(AI_ENTRY.to_string(), json)].into_iter().collect());

        let turrets = read_turrets(&archive);

        let table = AiTable {
            turrets,
            ..default()
        };
        let (_, matched) = table.turret_for("a2tur_01").expect("a2tur_01 matches");
        assert_eq!(matched.glob, "a2tur_**");
        assert_eq!(matched.health, 30.0);

        let (_, matched) = table.turret_for("a2tur00").expect("a2tur00 matches");
        assert_eq!(matched.glob, "a2tur**");
        assert!(!matched.is_destructible(), "1e9 health cannot be shot down");
    }

    #[test]
    fn parts_are_positional_and_the_two_element_form_drops_the_yaw() {
        let three = TurretDef {
            parts: ["turret", "gun", "firepoint"].map(str::to_string).to_vec(),
            ..turret()
        };
        assert_eq!(three.yaw_part(), Some("turret"));
        assert_eq!(three.pitch_part(), Some("gun"));
        assert_eq!(three.muzzle_parts(), ["firepoint".to_string()]);

        let two = TurretDef {
            parts: ["gun", "firepoint"].map(str::to_string).to_vec(),
            ..turret()
        };
        assert_eq!(two.yaw_part(), None, "no yaw part to traverse");
        assert_eq!(two.pitch_part(), Some("gun"));
        assert_eq!(two.muzzle_parts(), ["firepoint".to_string()]);

        // m6's a3tur**: one yaw, one pitch, two muzzles.
        let four = TurretDef {
            parts: ["turret", "gun01", "firepoint1", "firepoint2"]
                .map(str::to_string)
                .to_vec(),
            ..turret()
        };
        assert_eq!(four.yaw_part(), Some("turret"));
        assert_eq!(four.pitch_part(), Some("gun01"));
        assert_eq!(four.muzzle_parts().len(), 2);

        // m2's bouy_**: PARTS is null.
        let none = TurretDef {
            parts: Vec::new(),
            ..turret()
        };
        assert_eq!(none.yaw_part(), None);
        assert_eq!(none.pitch_part(), None);
        assert!(none.muzzle_parts().is_empty());
    }

    #[test]
    fn an_instance_key_names_its_kind_without_eating_a_variant_suffix() {
        assert_eq!(kind_of("scou_03"), "scou");
        assert_eq!(kind_of("rumv_easy_01"), "rumv_easy");
        assert_eq!(kind_of("ntank_2_11"), "ntank_2");
        assert_eq!(kind_of("bft_00"), "bft");
        // No numeric suffix: the whole key is the kind.
        assert_eq!(kind_of("minelay"), "minelay");
    }

    #[test]
    fn a_placement_converts_degrees_to_radians_and_zero_means_no_net() {
        let json = value(r#"[0, [2222.1, 3.9, 1294.2], 166.5]"#);
        let player = parse_placement("bft_00", ReaderValue::new(&json)).expect("parses");

        assert_eq!(player.net, None, "the player walks no net");
        assert!(player.is_player());
        assert!((player.heading - 166.5_f32.to_radians()).abs() < 1e-6);

        let json = value(r#"[1, [2529.4, 16.6, 1597.6], 80.0]"#);
        let enemy = parse_placement("rumv_easy_01", ReaderValue::new(&json)).expect("parses");

        assert_eq!(enemy.net, Some(1));
        assert_eq!(enemy.kind.as_str(), "rumv_easy");
        assert!(!enemy.is_player());
    }

    /// The two spellings of the pursuit standoff, and the link padding.
    #[test]
    fn a_net_reads_either_pursuit_spelling_and_drops_the_minus_one_links() {
        let json = value(
            r#"[["version", [105], "name", ["net_02"], "type", ["standard"],
                 "path_width", [10], "activate_rad", [80], "attack_rad", [60],
                 "attack_dwell", [1], "not_pursuit_dwell", [2],
                 "pursuit_range", [30, 45], "return_range", [50],
                 "hide_times", [0, 0], "attack_strategy", ["head_on"],
                 "attack_buddy", [0], "activate_buddy", [1],
                 "node_00", [[12], [0, 0, 0], [1, -1, -1]],
                 "node_01", [[12], [10, 0, 0], [0, -1, -1]]]]"#,
        );
        let net = parse_net(2, ReaderValue::root(&json)).expect("parses");

        assert_eq!(net.pursuit, [30.0, 45.0], "pursuit_range reads as pursuit");
        assert_eq!(net.strategy, Some(AttackStrategy::HeadOn));
        assert!(net.activate_buddy && !net.attack_buddy);
        assert_eq!(net.nodes.len(), 2);
        assert_eq!(net.nodes[0].flag, 12);
        assert_eq!(net.neighbours(0).collect::<Vec<_>>(), vec![1]);
        assert_eq!(net.neighbours(1).collect::<Vec<_>>(), vec![0]);
    }

    /// A net whose links point past the end of its own graph must not panic a
    /// caller that walks them.
    #[test]
    fn out_of_range_links_are_dropped() {
        let json = value(
            r#"[["name", ["net_01"], "type", ["standard"],
                 "node_00", [[12], [0, 0, 0], [7, -1, -1]]]]"#,
        );
        let net = parse_net(1, ReaderValue::root(&json)).expect("parses");

        assert!(net.neighbours(0).next().is_none());
        assert!(net.neighbours(99).next().is_none(), "and an unknown node");
    }

    /// The typo in m3's net_63: a strategy name in the `type` field.
    #[test]
    fn an_unknown_net_type_reads_as_standard() {
        let json = value(r#"[["name", ["net_63"], "type", ["follow_net"]]]"#);
        let net = parse_net(63, ReaderValue::root(&json)).expect("parses");

        assert_eq!(net.net_type, NetType::Standard);
    }

    #[test]
    fn the_deathmatch_ring_converts_its_headings() {
        let json = value(r#"[[[2500, 1, 1430, 180], [2525, 1, 1455, -135]]]"#);
        let archive =
            ReaderData::from_entries([(NET_ENTRY.to_string(), json)].into_iter().collect());

        let spawns = read_deathmatch_spawns(&archive);

        assert_eq!(spawns.len(), 2);
        assert_eq!(spawns[0].0, Vec3::new(2500.0, 1.0, 1430.0));
        assert!((spawns[0].1 - std::f32::consts::PI).abs() < 1e-6);
    }

    #[test]
    fn a_mission_with_no_units_is_an_error_and_one_with_no_nets_is_not() {
        let empty = ReaderData::default();
        assert_eq!(
            build_ai_table(&empty, Difficulty::Normal, &default()).err(),
            Some(AiError::NoUnitEntry("aiv.zrd"))
        );

        // A multiplayer map: one `bft_00`, no nets, an `ai.zrd` whose globs
        // match nothing in the level.
        let json = value(r#"[["bft_00", [0, [0, 0, 0], 0]]]"#);
        let archive =
            ReaderData::from_entries([("aiv.zrd".to_string(), json)].into_iter().collect());

        let table = build_ai_table(&archive, Difficulty::Normal, &default()).expect("builds");
        assert_eq!(table.units.len(), 1);
        assert!(table.nets.is_empty());
        assert!(table.enemies().next().is_none());
        assert!(table.player_start().is_some());
    }

    /// m7–m13 have no `aiv_hard.zrd`, so a hard game there reads `aiv.zrd`.
    #[test]
    fn a_missing_difficulty_variant_falls_back_to_the_normal_file() {
        let json = value(r#"[["bft_00", [0, [1, 2, 3], 0]]]"#);
        let archive =
            ReaderData::from_entries([("aiv.zrd".to_string(), json)].into_iter().collect());

        let table = build_ai_table(&archive, Difficulty::Hard, &default()).expect("builds");

        assert_eq!(
            table.player_start().map(|start| start.pos),
            Some(Vec3::new(1.0, 2.0, 3.0))
        );
    }

    /// m2's `aiv_easy.zrd` states `bft_00` twice. The first wins, and the
    /// duplicate does not become a second player.
    #[test]
    fn a_repeated_instance_key_keeps_both_records_and_the_first_is_the_player() {
        let json = value(
            r#"[["bft_00", [0, [1715.3, 39.6, 656.3], -90.0],
                 "bft_00", [0, [1715.3, 39.6, 656.3], -90.0]]]"#,
        );
        let archive =
            ReaderData::from_entries([("aiv.zrd".to_string(), json)].into_iter().collect());

        let table = build_ai_table(&archive, Difficulty::Normal, &default()).expect("builds");

        assert_eq!(table.units.len(), 2, "the reader keeps repeated keys");
        assert_eq!(
            table.player_start().map(|start| start.pos.x),
            Some(1715.3),
            "and the first is the start"
        );
    }

    #[test]
    fn the_nearest_net_node_is_the_one_to_walk_from() {
        let net = NetDef {
            nodes: vec![
                NetNode {
                    pos: Vec3::ZERO,
                    links: [1, -1, -1],
                    flag: 12,
                },
                NetNode {
                    pos: Vec3::new(100.0, 0.0, 0.0),
                    links: [0, -1, -1],
                    flag: 12,
                },
            ],
            ..net_def()
        };

        assert_eq!(net.nearest_node(Vec3::new(90.0, 0.0, 0.0)), Some(1));
        assert_eq!(net.nearest_node(Vec3::new(10.0, 0.0, 0.0)), Some(0));
        assert_eq!(net_def().nearest_node(Vec3::ZERO), None, "an empty graph");
    }
}
