# Reader (`zrdr.zbd`)

What the global and per-mission reader archives hold, and where the shipped files differ from READER-SCHEMA.md.

Full schema: **`docs/READER-SCHEMA.md`**. Summary of what
matters for the engine:

**Format.** A file is a nested list alternating `"KEY", value`; a key with
no value is followed by `null` (mech3ax collapses one-element lists, so
`[]` → `null`). Keys repeat (`SEQUENCE_DEFINITION` ×N), so keep order and
allow duplicates. Some values are positional records mixing scalars and
lists (`sounds.zrd`, `aiv.zrd`, `puppies.zrd`, `objectives.zrd`). Angles
in degrees, distances in metres, times in seconds.

**Global archive (127 entries):**

- `vehicle.zrd` (+ `_easy`, `_hard`): 25 vehicle types (player `bft`, 23
  enemies, `stealth`) — health, weapons rows, chase-camera offsets, AI
  ranges, death drops, and per-mode (`track`/`hover`/`amphib`/`sub`)
  physics: suspension quad, collision hull, `rates`, `turns`, damping,
  friction, mass, `mode_alt`, soil slowdowns, and the anim-name pairs for
  every form transition (`t2a_anims` …). **This is the vehicle tuning
  table.**
- `player.zrd`: gravity **40 m/s²** (9 in water, 3 in quicksand), camera
  constants, max slope, burning state, low-shield warning.
- `weapons.zrd`: `BALLISTICS` with **19** weapons `wep_S.V` (slot 1–9 ×
  weak/strong + enemy-only `wep_9.2`): damage, ammo limit, range,
  velocity, gravity, splash radius, homing (`ACCELERATION`, `TURN_RATE`,
  `LOCK_ON`, `TETHER_GUIDED`), mines (`MINE`, `REMOTE_DETONATE`,
  `RELOAD`), **hitscan `BEAM`** on the laser designator, laser sabre and
  arc sabre, plus per-surface impact animations/sounds/effects and
  `KILL_ANIMATION` names. Names and descriptions via `MSG_DESC_*`.
- `pickup.zrd`: 40 pickup kinds → sound + HUD icon. Models are the 40
  `pu000`…`pu039` template subtrees (order fixed by `support\pickup.gw`).
- `sounds.zrd`: the **logical-name → wav table** (`snd_m1obj1` →
  `m1obj1.wav`) with 3D/range flags — closes sounds.md's open item.
- `effects.zrd` (13 sprite cycles), `weather.zrd` (rain m2, snow m5),
  `briefing.zrd` (13 briefing layouts), `fmv.zrd`, HUD/menu layouts.
- 101 **animation-definition sources** (enemies, effects, explosions,
  transitions): already compiled into every `anim.zbd`; never parsed at
  runtime.

**Per-mission archive (m1: 149 entries):**

- `ai.zrd`: turret definitions keyed by node-name glob (`tur_1**`) with
  health, part names, weapon, fire timing, detection range.
- `aiv.zrd` (+ `_easy`/`_hard`): **every enemy vehicle instance and the
  player start** as `[net_index, [x,y,z], heading_deg]`, keyed
  `<type>_<NN>`; the type is a `vehicle.zrd` key and a gamez template root.
- `net_NN.zrd` (55–92 per mission): per-group AI parameters (`activate_rad`,
  `attack_rad`, dwell times, `attack_strategy` ∈ {follow_net, head_on,
  back_forth, circle, sit}, buddy flags) and a small waypoint graph.
- `net.zrd`: 8 deathmatch spawn points `[x,y,z,heading]` (every map).
- `puppies.zrd` (+ variants): pickup placements `[kind, amount, pos, rot,
  respawn_s]`; respawn 0 in campaign, 30/60/180 in multiplayer.
- `objectives.zrd`: per objective the briefing picture, three `MSG_*` keys,
  initial state, completion node(s), read/hint sounds and times.
- `location.zrd` (debug teleport points), `movers.zrd` (moving nodes),
  `declient.zrd` (crater/quicksand deformation), `startanims.zrd`
  (`NEW_GAME_START`/`LOAD_GAME_START` anim names), `anim.zrd` (compile
  include list), and the mission's own anim sources (`start_single_mN`,
  `deathmN`, `countdown`, `mNobjects`, `mNpickup`, `mNstartcam`, `mNvtols`).

Multiplayer maps carry `ai.zrd`, `aiv.zrd` (player only), `puppies.zrd`,
`net.zrd`, `race.zrd` (m9), placeholder copies of m2's `objectives.zrd` and
`location.zrd`, and their prop anim sources. No path nets.

## Vehicles

Asserted in `crates/bft-sim/tests/real_vehicle.rs`.

- **The per-mode physics keys are sparse.**
  Counted over the 25 kinds and 28 mode blocks of `vehicle.zrd`: `mode`,
  `rates`, `turns`, `turn_damping`, `rate_damping` and `collision_d` are on
  all 28, `sounds` on 27, `mode_alt` on 25, `mass` and `a_damping` on 23,
  the soil slowdowns on 19, `ok_to_amphib` on 15 — but **`platform`,
  `collision`, `friction`, `chas_smooth`, `chas_pitch` and `chas_roll` are on
  only 5**, and the eight `*_anims` transition keys on only 4. Those five are
  the player's four modes plus `stealth`'s `basic_mode`; the four are the
  player's. So **only the player has a suspension quad, a collision hull and
  a transition graph in the data** — an enemy vehicle's hull and springs have
  to be derived or borrowed.
- **`mode_alt` is also spelled `mode_alts`**, on three `track_mode` blocks
  (`rumv`, `rumv_easy`, `rumv_2`). Same meaning, same shape; a reader that
  looks for one spelling silently gives those three a ride height of zero.
- **`stealth` has a `basic_mode`**, a fifth mode key beyond
  track/hover/amphib/sub. It is the invulnerable camera stand-in (health
  10000) and its `common_mode` carries `position` and `rotation`, which no
  other kind has — it is placed by the reader rather than by `aiv.zrd`.
- **`common_mode` has four keys §2.1 does not list**, each on one kind:
  `camera_ud_swing` (4 floats, `bft`), `start_anims` (`amphi`, an anim name),
  `gun_pitch` (`lock`, at the *common* level rather than per mode), and
  `activation2` (`dron`, `dron_2`).
- **The player's unlimited ammo does not survive an f32.** `bft`'s `wep_1.0`
  row carries 123 456 789 rounds, which rounds to 123 456 792 as a float.
  Weapon ammo has to be read as an integer; every other column in the row is
  small enough that the float round-trip is lossless.
- **There is no `bft_multi` model.** The multiplayer maps do not use a
  different player build: **all 13 levels carry the same `bft_00` subtree**,
  hanging off the World node rather than as a parentless template (nodes.md,
  "The player subtree"), and no level has a node whose name begins
  `bft_multi`. The subtree is `bft_00/body/` with `chassis/turret/gun` under
  it and its `node_bbox` is x ±2.83, y 0–2.825, z −3.33…3.39, matching the
  `collision` ring exactly. It **grows through the campaign** as weapon
  models are added under `gun`: 49 nodes in m1, 57 in m2, 63–65 in m3/m4,
  and 72 in m5–m13, which is every weapon. m1 alone also has a parent-less
  `bft` template node, a 49-node copy of the same tree.

- **`friction` reads as `[static, kinetic, rolling]`, not `[longitudinal,
  lateral, rolling]`.** READER-SCHEMA §2.1 gives the latter with an (I). Over
  all five mode blocks in the game that carry the key, `[0] > [1]` always and
  the ratio `[1]/[0]` never leaves 0.50–0.67:

  | | `[0]` | `[1]` | `[1]/[0]` |
  |---|---|---|---|
  | `bft` track | 150 | 80 | 0.53 |
  | `bft` amphib | 140 | 90 | 0.64 |
  | `bft` hover | 80 | 40 | 0.50 |
  | `bft` sub | 300 | 200 | 0.67 |
  | `subt` sub | 120 | 80 | 0.67 |

  That band is exactly where kinetic/static friction sits physically, and the
  consistency across four different vehicle modes is hard to explain under
  the other reading — which also has to account for a *submarine* with a
  longitudinal friction of 300. The engine implements it as static and
  kinetic with hysteresis, which produces the two-stage grip-then-slide
  behaviour the original has. Five samples is not proof, so this stays
  inferred, but it is the better-supported inference.

Confidence: verified (key counts, spellings and the node survey above are
measured over all 25 kinds and all 13 levels); **inferred** for the
`friction` reinterpretation, on five samples. READER-SCHEMA §5's units
question is otherwise settled in `bft_sim::vehicle`'s module docs.

## Weapons and pickups

Asserted in `crates/bft-sim/tests/real_weapons.rs`.

- **`LOCK_ON` is a float, not a mode enum.** READER-SCHEMA §2.3 reads it as
  "0 none, 1 lock-on, 2 small"; the values are 0.1 (`wep_8.1`, `wep_9.1`),
  1.0 (`wep_8.0`, `wep_9.0`), 2.0 (`wep_9.2`) and **0.0** (`wep_7.1`). The
  0.1 rules out an integer mode. The 0.0 on the arc sabre matters for the
  `Mover` derivation: a weapon homes only on a *positive* lock-on, or that
  `BEAM` weapon becomes a missile.
- **`PITCH_RATE` [2] is a key the schema does not list**, on `wep_8.1` and
  `wep_9.1` — exactly the two `TETHER_GUIDED` weapons. A separate vertical
  turn rate alongside `TURN_RATE`. (I)
- **`IMPACT` carries a `FREEZE` block** on `wep_3.0` (freon), a six-float
  block of the same shape as the `HEAT` (`wep_6.1`) and `DESIGNATE`
  (`wep_6.0`) ones §2.3 lists. All four special children — those three plus
  `ANIMATION_ALWAYS` (both nukes) — are **siblings of the three surfaces**
  inside `IMPACT`, not children of one, so a parser that walks `IMPACT`'s
  pairs as surfaces reads them as a fourth. (V)
- **`FIRE_RATE` is on all 19 weapons**, not 18 as §2.3's bracket says, and
  its unit split is settled: **≥ 3.0 is shots per second, below it is
  seconds between shots**. Nothing in the file lies between 2.0 and 12.0.
  Cross-checked against the player's `interval` column in `vehicle.zrd` for
  all 18 of its rows (READER-SCHEMA §5.2).
- **`wep_7.0` (sonic cannon) has only a `default` impact block**; the other
  18 carry all three surfaces. Every weapon has `default`.
- **`pickup.zrd` holds 40 kinds, not 43**, and three names differ from
  §2.4: the form chips are `PUP_AMPHIB`, `PUP_HOVER`, `PUP_SUB` (not
  `MSG_*_PICKUP`), and a plain **`NANITE`** sits alongside `NANITE100` and
  `NANO-CANISTER`. 40 kinds against 40 `pu000`–`pu039` templates is a
  bijection. The HUD icons run to **`gun_13`**, not `gun_12`. (V)
- **The kind-to-template mapping comes from `support\pickup.gw`, and its
  order is not `PICKUP_DATA`'s.** The script loads `lomissammo, gmissammo,
  nukeammo, gnukeammo` (→ `pu013`–`pu016`) while `PICKUP_DATA` lists
  missile, *nuke*, guided missile, guided nuke — so deriving the order from
  the reader puts the nuke's model on the guided missile, and the same
  swap again in the guns block (`pu030`–`pu033`). §2.4's own list is right;
  the reader's order is not a substitute for it. (V)
- **Only m1–m6 have `puppies_easy.zrd`/`puppies_hard.zrd`.** The seven
  multiplayer maps (m7–m13) ship `puppies.zrd` alone — difficulty has no
  meaning in deathmatch — so a per-difficulty reader has to fall back.
  §3.5 implies all three everywhere. Campaign records all have `respawn`
  0; every multiplayer record has 30, 60 or 180. (V)
- **The `pu*` nodes in gamez are the 40 templates only.** No level carries
  a `pu03700`-style *instance* node, so the `pu<NNN><ii>` names
  `objectives.zrd` references are created at runtime from `puppies.zrd`
  order, per kind. (V for the absence, I for the numbering rule.)

Confidence: verified (counts, names, key presence and the orders above are
measured over the shipped files and all 13 missions); **inferred** for
`PITCH_RATE`'s meaning and the `pu<NNN><ii>` instance numbering.

## REQUIREMENTS §9, question 5

Spawns: **answered**. Enemy vehicles: `aiv.zrd` records
`[net, pos, heading]` keyed `<type>_<NN>`, cloned from gamez template
subtrees (`scou`, `rumv`, …) with stats from `vehicle.zrd` and behaviour
from `net_NN.zrd`. Turrets/SAMs/helicopter guns: baked gamez nodes
matched by `ai.zrd` globs (`tur_1**`). Pickups: `puppies.zrd` records
(kind, amount, pos, respawn) instantiating `pu000`–`pu039` templates.
Player: `aiv.zrd/bft_00`; deathmatch: `net.zrd` (8 points).

Confidence: verified with inferred details.

## REQUIREMENTS §9, question 9

Weapon/vehicle names: **answered from data** — `weapons.zrd` (19 ids,
slots 1–9 × weak/strong) + `MSG_DESC_*`/`MSG_*_W` strings; vehicle
display names in `vehicle.zrd` `common_mode/name`; form names in the
`MSG_*_PICKUP` strings and `vehicle.zrd` mode keys (`track`, `hover`,
`amphib`, `sub`). Interp decision (interp.md): no interpreter; extract texture
animation bindings from the `mN_zbd.gs` runtime path.

Confidence: verified.
