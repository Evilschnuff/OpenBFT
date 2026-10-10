# open-bft save game format

Format version 1.1 (1.1 added the HUD's clock and score). Normative for `crates/bft-sim/src/save/format.rs`,
which is the reference implementation: where the two disagree, fix one of
them in the same change.

This is the file an in-mission save writes. Campaign progress between
missions (`save.ron`, FR-M6) is a separate file and is not covered
here.

## 1. What the original did, and why this is not that

The original saved from the in-game menu or F8 and loaded with F9 (manual,
"Other Controls"). Its string table has messages for refusing to save or load
while the BFT is being carried (`MSG_CANT_SAVE`, `MSG_CANT_LOAD`) and in
multiplayer (`MSG_CANT_SAVE_MP`). Each mission's `startanims.zrd` has a
`load_game_anim` (`load_control`: callbacks 15, 10, 1, which hand the player
control), which a load here calls instead of the intro.

Reproducing the original's format is out of scope (REQUIREMENTS §5), and
nothing in the data describes it. This format is the engine's own, and it is
designed so that saves survive engine updates.

## 2. Design rules

1. **The level is not in the file.** Loading a save loads the level from the
   game data exactly as a new mission does, then applies the save on top. The
   file holds only what differs from that fresh level: moved and hidden nodes,
   damage, the anim VM's state, the dynamic entities. A save stays small and
   stays valid when the engine's reading of the data improves.
2. **Nothing is keyed by `Entity`.** Every reference is a [`Ref`](#5-references)
   built from ids that are stable across runs: gamez node ids, `aiv.zrd`
   instance names, anim definition indices, and positions in the file's own
   lists.
3. **Values are the engine's world, in SI units.** Metres, seconds, radians,
   Y up, right-handed: the space `bft-sim` spawns the level into. Colours are
   linear RGBA.
4. **The format has its own types.** `format.rs` mirrors the handful of
   engine enums it stores (`Form`, `AiMode`, `Objective`, …) rather than
   serialising the engine's own, so renaming something in the sim cannot
   silently change the file. The conversions live next to the types.
5. **Names ride along with indices where the data could shift.** An anim
   record carries its definition's `anim_name`; an enemy-part record its node
   name. A reader that finds a mismatch looks the name up instead, and drops
   the record with a warning if that fails too.

## 3. Encoding and files

- UTF-8 [RON](https://github.com/ron-rs/ron), extension `.ron`, the same as
  `save.ron` and `config.ron`: readable in a text editor and diffable, which
  matters more than size for a single-player save of a few hundred KB.
- Saves live in `<data dir>/saves/`: the quick save in `quicksave.ron`, the
  others in `save-<unix seconds>.ron`. The platform's data dir comes from
  `directories::ProjectDirs::from("org", "open-bft", "open-bft")`.
- A save is written to `<name>.tmp` and renamed over the target, so a crash
  mid-write never leaves a truncated save.

## 4. Versioning and compatibility

The top level starts with two fields every version keeps forever:

```ron
(
    format: "open-bft-save",
    version: (major: 1, minor: 0),
    ...
)
```

A reader first parses only `format`, `version` and `header` (serde ignores
the rest), then decides:

| File version vs. reader | Action |
|---|---|
| same major, any minor | Load. Fields the reader does not know are **ignored**; fields the file lacks take their **defaults**. A newer minor is logged. |
| older major | Parse the text with that major's frozen types (`save/format_vN.rs`) and convert step by step, `vN → vN+1 → …`, in ordinary Rust (`save/migrate.rs`). RON's generic value cannot hold enums, so migrations are typed, not tree edits. Steps are never deleted. |
| newer major | Refuse, with *"written by a newer open-bft"*. The save list still shows it, greyed out, from its header. |
| `format` is not `"open-bft-save"` | Not a save. |

What each kind of change costs:

| Change | Bump |
|---|---|
| Add a field that has a sensible default, or a new optional section | minor |
| Add a variant to an enum a reader never has to accept (only written by newer code) | major, unless the enum declares an `Unknown` fallback |
| Remove or rename a field or variant, change a unit, a coordinate convention or a field's meaning | major: freeze the old `format.rs` as `format_vN.rs` and add the conversion |
| Change the engine's reading of the game data in a way that moves node ids or definition indices | none: that is what the names in rule 5 and the fingerprint (§6) are for |

Rules that keep this working:

- `format`, `version` and `header` are written first, in that order. A
  reader lists saves by parsing only that head: RON steps over an unwanted
  field through its untyped path, which is quadratic in the field's size
  (seconds for a 350 KB save, against milliseconds to parse the whole file
  into its types). A file laid out otherwise still reads, slowly.
- No type in `format.rs` uses `deny_unknown_fields`.
- Every field added after 1.0 is `#[serde(default)]`.
- `None` and empty lists are omitted when writing, and default when reading.
- The round-trip test (`save_roundtrip`) and the golden-file tests in
  `crates/bft-sim/tests/save_format.rs` pin the current version. A test that
  fails because the format changed means: bump, and add a migration if the
  bump is major.

## 5. References

```ron
Node(1234)                 // a level node, by gamez node id
Player                     // the player's hull
Enemy("scou_03")           // an enemy hull, by aiv.zrd instance name
EnemyPart("scou_03", 7)    // node 7 of that enemy's model
Anim(211)                  // anim definition 211's own instance
Copy(4)                    // entry 4 of anims.copies: a copy's instance
CopyPart(4, 0)             // node 0 of that copy's cloned subtree
Pickup(12)                 // entry 12 of pickups
Projectile(3)              // entry 3 of projectiles
```

A **part index** counts the nodes (entities with a node name) of a subtree
breadth-first from its root, children in order, root = 0. Enemy models and
copy subtrees are both cloned breadth-first from a template, so a fresh
clone numbers its nodes the same way. The player's model needs no parts: it
is the level's own `bft_00` subtree, re-parented, and its nodes keep their
gamez ids.

A reference the reader cannot resolve becomes "nothing" (a homing missile
loses its target, a motion is dropped) and is logged once per kind.

## 6. Layout

Everything below is one RON struct. Sections not listed are v1.0 fields that
default when absent.

### `header`

What the load list shows without reading the rest.

| Field | Type | Meaning |
|---|---|---|
| `written_by` | string | `"open-bft 0.1.0"` |
| `created` | u64 | Unix seconds, UTC |
| `label` | string | shown in the list; `"Quick save"` for F8 |
| `level` | u16 | mission number (1–6) |
| `level_name` | string | catalog directory name, `"m1"` |
| `title_key` | string | `MSG_CAMPAIGN1_NAME` |
| `difficulty` | `Easy` \| `Normal` \| `Hard` | the tables the level was built for |
| `play_time` | f32 | seconds of mission time (the anim VM clock) |
| `fingerprint` | struct | see below |

`fingerprint` is `(nodes, node_hash, anims, anim_hash)`: the level's node
count and an FNV-1a 64 hash of every `(node id, name)`, and the anim
definition count and a hash of every `anim_name` in order. A mismatch means
the save was made against different data (another release of the game, a
mod, a changed extractor); the load goes ahead with a warning and rule 5
decides record by record.

### `campaign`

`completed: [u16]`, `unlocked: [Form]`, `mounted: [string]`: the campaign
progress at the time of the save, for the list and for the record. Loading
does not overwrite `save.ron`; a win after a load is recorded on top of it
as any win is.

### `world`

| Field | Meaning |
|---|---|
| `clock` | the anim VM's clock, seconds |
| `ai_tick` | the AI's perception tick counter |
| `rng` | the sim's ChaCha8: `seed` (64 hex digits), `stream`, `word_pos` (decimal u128) |
| `mission` | `objectives: [Inactive \| Active \| Complete]`, `outcome`, `started`, `dying`; since 1.1 `elapsed` (the HUD's mission clock, seconds; a 1.0 save lacks it and the reader takes `clock` instead) and `score` (omitted when 0) |
| `fog` | `color`, `range`, `altitude`: the `WorldEnv` as scripts left it |
| `camera` | `zoom: [f32; 2]`, `clip_near`, `clip_far` |
| `screen_tint` | linear RGBA, or absent |

### `nodes: [NodeRecord]`

One record per node whose state differs from the freshly loaded level:
level nodes and enemy-model parts alike.

```ron
(node: Node(812), t: Some((t: (1.0, 2.0, 3.0), r: (0.0, 0.0, 0.0, 1.0), s: (1.0, 1.0, 1.0))),
 active: Some(false), parent: Some(LevelRoot), opacity: Some(0.4), rest: None, placed: Some(true))
```

- `t`: local transform (`t` translation, `r` quaternion xyzw, `s` scale).
- `active`: the node's active flag (visibility and collision).
- `parent`: `LevelRoot` or `Of(<Ref>)`, when a script re-parented it.
- `opacity`, `rest`, `placed`: the anim VM's per-node state
  (`AnimOpacity`, `AnimRest`, `AnimPlaced`).
- `name`: set on part records, for rule 5.

### `craters: [(node: u32, craters: [Crater])]`

Each crater's `center`, `radius`, `floor_radius`, `depth`, `sides`,
`texture`, `uv_scale`, in the order they were cut. Loading re-cuts them in
that order.

### `player`, `enemies`, `turrets`

A vehicle is `t`, `velocity`, `spin` (angular velocity), `form`,
`unlocked`, `turret: [yaw, pitch]`, `health: (current, max)`, `burning:
(remaining, dps)` and `loadout`. The loadout names weapons by their
`weapons.zrd` id, never by table index; each slot has `weak`/`strong`
entries of `(weapon, ammo, mounted)`, plus `selected`, `strong`, and the two
cooldowns. A weapon's fire interval is not stored: it is the table's.

An enemy adds `instance`, `kind` and `brain` (`mode`, `target`, `timer`,
`waypoint`, `forward`, `los_lost`, `since_seen`). A turret is keyed by its
node id and stores its aim, timers, target, `deactivated`, health and
loadout.

The player additionally stores `twin_next`, the side cannon that fires next.

### `pickups: [Pickup]`

Every pickup standing in the level: `kind`, `amount`, `instance`, `at`,
`respawn`, `respawn_in` (a deathmatch pickup waiting to come back). The
loaded level's own pickups are replaced by this list, because a destroyed
enemy's drops are pickups too and their instance names are not unique.

### `projectiles: [Projectile]`

Everything in flight or lying armed: `weapon` (id), `owner`, `team`, `t`,
`velocity`, `age`, `target`, `armed`, `health`, `landed`.

### `anims`

- `instances`: one record per definition instance that is not in its fresh
  state: `def`, `name`, `armed`, `invalidated`, `started`, `completed`,
  `damage`, `reset_at`, `running` (sequence cursors: `seq`, `event`, the
  three clocks, `started`, `t_running`, `loops_left`, the `If` stack),
  `input`, `input_ends`, `played_on`, `scope`, `skip`, `ran_out`.
- `copies`: the running copies of prototype definitions. Each is an instance
  record plus the subtree it plays on: `On(<Ref>)` for an existing one, or
  `Owned([CloneNode])` for one the VM cloned, each node with its parent index,
  name, model index, LOD band, transform, active flag and per-node anim state.
- `motions`: `ObjectMotion`s in flight, keyed by node, owned by an instance
  ref, sequence and event.
- `lights`: the definitions' own lights, keyed by owner instance and name.

## 7. What is deliberately not saved

- **Anything presentation-only:** sparks, smoke, debris sprites, sounds
  playing, the HUD message log, camera smoothing. They are gone after a load
  and nothing in the mission depends on them.
- **A taken pickup's half-second flight into the vehicle.** Its effect is
  already applied.
- **Per-tick derived state:** ground contact, traction, aim results, the
  player's zone, trail state. The first tick recomputes them.
- **Messages between systems within a tick.** A save is taken between ticks,
  so nothing is in flight that the next tick would not regenerate.
- **Physics solver state:** contacts, sleeping, warm starts.

## 8. When a save is allowed

In a campaign mission, while the player drives: not during a cinematic (the
original's "while the BFT is being transported"), not once the mission has
ended or the death animation has started, and never in multiplayer.
