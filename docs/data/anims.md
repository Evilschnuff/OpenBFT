# Anim definitions

The compiled anim definitions: fields, activation and timing, event handlers, instancing, cinematics and the missions they drive.

Sources: compiled `mN/anim.zip` for all 13 levels (4798 defs; field-level
stats computed with a script over every def), the reader anim *sources*
(`countdown`, `deathm1`, `m1objects`, `m1pickup`, `m1startcam`,
`start_single_m1`, `start_north`, `m1flufvtol`, `teleprtr`, `bft_trans`)
for the human-readable vocabulary, and interp.md's runtime script.
`anim.zbd` starts with a header mentioning "DKit release version 3" —
Zipper's tool version stamp.

## Relationship to reader sources

Every compiled def comes from a reader `ANIMATION_DEFINITION`; the file
name is `<name>-<anim_name>[-<anim_root_name>].json`. Wildcards in the
source `NAME` (`sndbg0*`) were expanded at build time into one def per
matching node (`sndbg01-fly_bags1-healthy` … `04`). **The engine reads
only the compiled `anim.zbd`; reader anim sources are documentation.**

## Top-level fields (compiled JSON)

| Field | Values (all levels) | Meaning |
|---|---|---|
| `name` | node name | the **anim root object**: node the def is attached to; `objects[]`/`nodes[]` list the nodes it touches |
| `anim_name` | unique per level | what `CallAnimation`, `startanims.zrd`, `vehicle.zrd` `*_anims`, `ai.zrd` `DESTROY_ANIM`, `weapons.zrd` `KILL_ANIMATION` refer to |
| `anim_root_name` | node under `name` | scope for resolving `healthy`/`destroyed`/`dbase` (non-unique names) |
| `activation` | `OnCall` 2897, `WeaponHit` 1339, `WeaponOrCollideHit` 331, `OnStartup` 121, `CollideHit` 110 | see below |
| `execution` | `"None"` 4663, `{ByRange:{min,max}}` 90, `"ByZone"` 45 | see below |
| `reset_time` | `-1` 3368, `0` 1173, `null` 220, small positives 40, `-99` 3 | see below |
| `health` | 0 for OnCall; 0.01–20 for hit activations | damage threshold: accumulated hit damage ≥ `health` activates (0 = any hit) |
| `activ_prereqs` | `Animation` 334, `Object` 30, `Parent` 27; `activ_prereq_min_to_satisfy` | gates activation: named anims must have started / node must be active or inactive |
| `anim_refs` | `CallObjectConnector {name, local_name}` (boat wake, exhaust) | sub-animations instantiated by name |
| `has_callbacks` 54, `network_log` 303, `save_log` | | callbacks to engine; replication and save hints |
| `reset_state` | event list (`ObjectActiveState` mostly) | applied at load and on reset: **this is where a level's initial node visibility comes from** (`healthy` on, `destroyed` off, `frcoff` off …) |
| `sequences[]` | `name`, `seq_state` (`Initial` 12430, `OnCall` 3252), `events[]` | `Initial` sequences run when the def activates; `OnCall` ones only via `CallSequence` |
| `static_sounds`, `dynamic_sounds`, `lights`, `effects`, `puffers` | name lists | resources the def uses (sound names via `sounds.zrd`; effects via `effects.zrd`) |

## Activation semantics (for the anim VM)

- `OnStartup` (121): armed when the level starts. Combined with `execution:
  None` it runs immediately (m10 sign loops, m13 teleporter screens);
  combined with `ByRange` it waits until the player comes within `max` (m1's
  nine `call_fgull_fly*` seagull flocks within 40 m, m13's `bft_to_*`
  teleporters within 15.5 m; the bounds are squared, below). So `execution`
  is a *condition on top of* activation, not an alternative to it.
- `OnCall`: activate only through `CallAnimation` from another def, from
  `startanims.zrd` (`start_single_player` / `load_control`), from reader
  references (`vehicle.zrd` transition anims, `ai.zrd`/`vehicle` death anims,
  `weapons.zrd` kill/impact anims) or from the engine (`deathmN`
  `bft_00`, `countdown_from_9`).
- `WeaponHit` / `CollideHit` / `WeaponOrCollideHit`: the def's root object
  is damageable; when projectile damage (or a collision impulse) on any
  node in its subtree accumulates to `health`, the def activates. `health`
  is therefore the hit points of destructible props and enemies (`m1objects`
  `HEALTH [5]`, `mNvtols`, `prbox_hit01` 0.2). `WeaponHit` defs with health
  0 fire on the first hit (impact sparks).

  **A hit counts for the def whose `anim_root_name` node it lands under**,
  not the whole root object's subtree, and not every node the def lists in
  `objects`/`nodes`. Those lists are what the script touches. One object
  often carries several hit defs, one per piece: m1's `fuel_station` has two
  defs of that same name, rooted at the roof posts `postright` and
  `postwrong`. Whichever post dies first drops the roof its way (`destroyed`
  or `destroyed1`), which is the left/right ramp. m1's `frcgen`
  (`genleft`/`genright`/`shwave`), `maingate1`
  (`grating1`/`healthy1`/`healthy2`) and m6's elevator `beltrig`
  (`btrigupon`/`btrigdownon`) have the same shape. Read as hit targets, the
  lists would fire sibling defs together and let a listed name the object
  lacks match level-wide: m1's `gate_part1` lists a `dbase` its gate does
  not have, so any wreck's `dbase` would break it. **92 hit defs name an
  `anim_root_name` their object lacks** (m4's `hit_warehous_51` on
  `whous_1`, m6/m13's `hit_wall_102` on `dwall02`). Each is a multi-piece
  prop's def for a piece that model does not have, and takes no hits. (V:
  all 13 levels surveyed; `real_anim_events.rs`.)

  **Listed names are bound to nodes by `ptr`.** Each `objects`/`nodes`
  entry's `ptr` identifies the node the name is bound to: `39845900 + 196 ×
  node id`, the same base in all 13 levels. That accounts for every entry
  but the sound nodes created at runtime (their values lie past the last
  node). So a listed name means one node, not "the nearest of that name" nor
  "every one": m1's `hshit_1` gate resets a `dbase` its model lacks, bound
  to `vtol2`'s wreck. Read level-wide, that reset would hide every wreck
  base in m1 at load. The lighthouse's `part1` is bound to the `vwbus`'s. In
  m6 the bindings pick the real `belev`/`beltrig` tree over the duplicate
  one. (V.)

  **`reset_time -1` is not "one-shot" for a hit def.** 1762 of the 1780 hit
  defs carry it, and the ones that must fire once say so themselves: 933
  `InvalidateAnimation` themselves, 574 more hide their own `anim_root_name`
  node, so nothing can hit it again. m1's generator emitters
  (`left_shoot`/`right_shoot`, 0.2) do neither. Each hit sends the 8 m
  `shwave` across, and a hit on `shwave` (1.0) runs `destroy_the_gen`: the
  field drops, and 2.9 s later the arena's `beam_on1`–`6` are called. Those
  are `ByZone` and light while the player is in zone 2. Read as one-shot,
  two missed shockwaves would leave m1 unwinnable. A def ended by its own
  `StopSequence` completes like any other. Range/zone defs carry `-1` the
  same way (m6's teleporters `bft_to_N` do not invalidate themselves) but
  still run once; not yet checked. (V for the hit counts; I for the
  reading.)

  **A `CallAnimation` with `AtNode` places prototypes only.** 3516 such
  calls target a def rooted at a prototype (effects, wrecks). Eight target
  a def rooted in the level: m1's `bay_hitN` → `powerbay_sparksN` at the
  `sparks` node under the fuse, `hivolt` → `hivolt_fx` and m10's
  `snake_loop`, both at their own root. Moving the root there would shift
  each fuse by ~3.4 km and leave a hole in the arena wall. A level-rooted def
  stays where the level put it. (V.)

  **SAM sites do not shoot the player.** `ai.zrd`'s `sam_**` has
  `TARGETS ["vtol"]`. m1's `sam_01`–`06` sit on the arena's beam
  emitters. (V.)
- `execution`: `None` = plain; `ByRange {min,max}` = *also* activates when
  the player is within the band of the root object. **The bounds are squared
  metres**: all 90 `max` values in the game are exact squares of round
  numbers (30.25 = 5.5², 240.25 = 15.5², 1600 = 40², 160000 = 400²) and
  every `min` is 0. Read as metres, m1's seagulls would wait for the player
  at 1600 m instead of 40. Triggers: `m1_exit_trigger` 5.5 m (30.25),
  teleporters 15.5 m (240.25), radar spin 200 m (40000); `ByZone` =
  activates while the player is in the root object's `zone_id`
  (`ocean_sound_dock`, ambient sounds; `HINT_ZONE` in objectives uses the
  same ids). Both are polled every tick, not edge-triggered — the def's own
  `reset_time` decides re-arming.
- `reset_time`: `-1` = no timed reset (stays in its end state; for hit
  defs see above);
  `0` = resets immediately on completion, i.e. can be re-triggered at once
  (pickups, impact effects, `countdown_from_9`); `t > 0` = re-arms `t`
  seconds after completion; `null` = not applicable (looping
  `boat_wake_trail` with `network_log`); `-99` special (3 cases, U).
  The reader form `RESET_TIME [t1, t2]` (m1pickup `[-1, -1]`) compiles to
  a single value; the second is unknown.
- `activ_prereqs`: `Animation {name, required}` = that def must have
  *started* (not completed: m1's `gate_lower` and `rocket_hit11`–`13` call
  `m1pickup`, whose prerequisites they are, while still running); `Object {name, active}` / `Parent {name, active}` = the node
  (or the root's parent) must be in the given active state;
  `activ_prereq_min_to_satisfy` = how many of the non-required ones must
  hold. Doors open only after windows are broken (`dh_doors_open`).

## Timing semantics

- Events run **in list order**; `start: null` (41 171 of 53 167 events)
  means "same time as the previous event" — sequences are mostly
  instantaneous state flips with a few timed motions.
- `start: {offset: kind, time: t}` with `kind` ∈ `Animation` (2459),
  `Event` (2262), `Sequence` (649): `t` seconds after the animation
  started / after the previous event *started* / after the sequence
  started. The reader spelling is `START_TIME ["ANIMATION_OFFSET"|
  "EVENT_OFFSET"|"SEQUENCE_OFFSET", t]` (`countdown_from_9` chains nine
  sounds with `EVENT_OFFSET 1`).
- Timed events carry their own `run_time` (`ObjectMotion`,
  `ObjectMotionFromTo`, `ObjectOpacityFromTo`, `FbfxColorFromTo`,
  `LightAnimation`…). **A timed event blocks its sequence until it is
  done**, and `OffsetFrom::Event` counts from the previous event's *start*.
  See "Handlers" below for the evidence.
- `Loop {Count: n}` (−1 = forever, 911 cases) re-runs the sequence from
  its first event; `If/Elseif/Else/Endif` conditions are `AnimationLod`
  (1204 — LOD level, presentation), `PlayerRange` (666 — **squared**
  distance to the player, like `ByRange`: all 31 distinct values are exact
  squares of round numbers), `RandomWeight` (342), `NodeUndercover` (56).
  The VM needs exactly these four condition kinds.
- `Callback {value}` (119 events, 54 defs): engine hooks. **99 is the
  mission-end signal**: it fires in exactly the six `mN_exit_animation`
  definitions and nowhere else, while 2 also fires in every
  `mN_start_animation`, so keying the win on 2 would end the mission
  during the opening camera. Full observed set: 1/10/15 (`load_control` hands
  the player control), 2/11/14/20 (camera handshakes), 17 (m13's
  teleports), 25/26/27, 99 (win), 911–914 (mission props). The objective
  system does **not** use callbacks beyond the win (it watches node active
  states, reader.md).
- `ObjectActiveState` (14 591) is the workhorse: toggles a node's `active`
  flag, which is visibility + collision for its subtree. `InvalidateAnimation`
  (1286) disarms a def permanently; `ResetAnimation` re-arms it.

## What the mission logic needs from the VM

1. Load all defs; apply every `reset_state` at level start (initial
   visibility); activate `OnStartup` defs; run `startanims.zrd`'s
   `NEW_GAME_START` anim.
2. Support `CallAnimation`/`CallSequence`/`StopAnimation`/`StopSequence`/
   `InvalidateAnimation`/`ResetAnimation`, `Loop`, `If` with the four
   conditions, `ObjectActiveState`, `Callback`, the three start-offset
   kinds and `reset_time`.
3. Feed hit damage per node into `WeaponHit` defs and player range/zone
   into `ByRange`/`ByZone` defs.
4. Expose node active-state changes as events; objectives subscribe (reader.md).

Everything else (`ObjectMotion*`, `Sound*`, `Effect`, `Light*`, `Fbfx*`,
`ObjectConnector`, `ObjectCycleTexture`, `ObjectMotionSiScript` with the
`.zan` frame data) is presentation work and can be stubbed without
breaking mission flow, with one exception: `ObjectMotionFromTo` on
doors/gates changes collision geometry (the handlers must move colliders).

Confidence: verified (field values, counts, cross-references); inferred
(exact polling vs edge semantics of `ByRange`/`ByZone`, the `reset_time`
positive case, callback value meanings).

## Definition shapes and the missions

Asserted in `crates/bft-assets/tests/real_anim.rs` and
`crates/bft-sim/tests/real_mission.rs`. Measured over all 13 `anim.zip`s
(4798 defs, 15 682 sequences, 53 167 events) and the six campaign
`objectives.zrd`.

- **`ByRange` and `PlayerRange` are squared distances** (above): 121
  samples, every one an exact square of a round number. The engine keeps
  the file's squared values and compares against `distance_squared`, so no
  root is taken per tick.
- **`objects`/`nodes`/`lights`/`static_sounds`/`dynamic_sounds`/`effects`
  are arrays of records, not of strings.** Each element is an object with
  a `name` plus pointers, padding and (for `objects`) a base64 `affine`
  blob. A loader that expects `Vec<String>` gets an empty list with no
  error, and every hit-activated definition then matches no node. (V)
- **`reset_state` is `{ "events": [...] }`**, an object wrapping the list,
  present on 3108 of the 4798 defs. Sequences carry a separate
  `reset_state` *string* (`"Initial"`); the two are different fields. (V)
- **`anim_name` is not unique within a level.** 5–12 collisions each,
  almost all effect templates whose name ends `.flt` (`spark_blue.flt` ×3)
  plus a handful of props (m1's `fuel_station` ×2). The name index must be
  a multi-map; `CallAnimation` starts every match, which is what an effect
  template wants. No name reached from reader data collides. (V)
- **`network_log` and `save_log` are `Option<bool>`**, null on most defs.
  `has_callbacks` agrees with the events on all 4798. (V)
- **`anim_refs` has a `CallAnimation` variant** (14 cases) beside the 112
  `CallObjectConnector`s the table above lists. (V)
- **18 `CallAnimation`s and 26 `CallSequence`s in the shipped game dangle.**
  Six distinct animation names (`lavbub.flt`, `cjunkdest02`, `frc_twirl`,
  `pspipe_destroy_07`, `stop_the_belt`, `destroy_vtol1`) and one sequence
  name (`run_skin_sequence`, called by `regen_skin` and `shock_vehicle` in
  all 13 levels) name nothing. They are the original's own authoring
  slips; the VM warns once per name and carries on. (V)
- **m9 has no `OnStartup` definition at all.** The race map is started by
  its countdown gates. Every campaign level has 9–30. (V)
- **Every campaign level has exactly one `mN_exit_animation`** and it is
  the only definition in that level carrying `Callback 99`. (V)

On the missions (READER-SCHEMA §3.6):

- **`NEW_GAME_START` is `start_single_player` only in m1**; m2–m6 use
  `mN_start_animation`. A default would leave five missions dead. (V)
- **The `objectives.zrd` record is loose**: the state
  word is optional (the exit objectives of m1–m5 carry none), the `COMP`
  key is optional (m3's objective 5), m6 adds `FINAL_MISSION [1]` at file
  level and `AUTOPLAY` inside two records, and `HINT_SOUND` is also
  spelled `HINT_SND`. A positional parser that counts fields misreads
  four of the six missions. (V)
- **No shipped objective completes on a pickup.** m2's objective 1, the
  amphib chip, completes on `["got_amphib"]`, a real gamez node. All 26 node
  conditions across the six missions resolve to existing nodes and every one
  of them starts `active: true`, so the rule is uniformly "complete when the
  node's active state changes from what it was at mission start". (V)
- **The two-element condition is `[object, child]` scoped to the object's
  subtree**, verified for all five: `powstation`→`powerbad`,
  `fact`→`fcpdown`, `fntrocket`→`healthy`, `comctr`→`dbase`,
  `clf_door`→`clfon`. `healthy` occurs 186× level-wide in m4 and `dbase`
  36× in m5, so a level-wide lookup watches the wrong node. (V)
- **Nothing activates an `INACTIVE` objective.** It is not the previous
  objective's completion: m2's objective 1 is `INACTIVE` with nothing before
  it and m3's objective 3 sits `INACTIVE` between two `ACTIVE` ones, so
  briefing order is not dependency order. What all 16 `INACTIVE` objectives
  *do* have is a dedicated definition that flips exactly their node
  (`check_get_amphib`, `obj_trigger1`, `check_get_hover`,
  `tp6_teleport_access`, …). `INACTIVE` means "not worth briefing until it
  happens", and the flip both reveals and completes the objective. The one
  objective this engine activates itself is the exit, once the rest are
  done. (V for the flippers, I for the reading.)

Confidence: verified (every count, name and shape above is measured over
the shipped files); **inferred** for the meaning of `INACTIVE` and for the
`AUTOPLAY`/`HINT_ACTIVE` keywords, which are recorded but not acted on.

## Handlers

Measured over all 13 `anim.zip`s and asserted in
`crates/bft-sim/tests/real_anim_events.rs`.

- **Timed events block their sequence.** `expnuke`'s `frame_buffer_effects`
  is three `FbfxColorFromTo`s with no offsets — alpha 0.25→0.9 (0.2 s),
  0.9→0.9 (0.5 s), 0.9→0 (0.5 s) — a fade in, a hold and a fade out that
  only work in series. m1's `generic_6` flings each part with `ObjectMotion`
  (`gravity −9.8`, `run_time: null`) and the very next, untimed event
  switches the part off, which only makes sense once it has landed. Parallel
  motion is authored as parallel sequences (`bftmbrst`'s `burst_start`,
  `scale_burst`, `drift_burst`). (V for the shapes; I for the reading, but
  no other reading fits.)
- **`OffsetFrom::Event` counts from the previous event's start.** m2's
  gates run a 3 s `ObjectMotionFromTo` and then `{Event, 3.0}` swaps the
  gate models — at the end of the drop, not three seconds after it. (I)
- **`ObjectMotionFromTo`'s `from`/`to` are absolute local values.** m1's
  `dh_door_right` is at `(20, 2, −185.721)` in gamez and its motion starts
  from exactly that; m2's `locked` has an identity transform and drops from
  `(0, 0, 0)` to `(0, −6, 0)`. `scale` is absolute too. (V)
- **SI scripts store the quaternion's scalar in the field named `x`.**
  Over all 176 scripts mean |x| is 0.64 and mean |w| 0.05; read as
  `(x, y, z, w)` m1's VTOL flies upside down. Translation `delta` is a
  rate: `base + delta·(end − start)` meets the next frame's `base` to within
  1 mm everywhere. All scripts have `spline_interp: false` and frames
  0.25 s apart, so linear playback is exact for translation. (V)
- **`LightAnimation`'s `range`/`color` are changes over `run_time`, not
  targets**: `vtol_destruction1` animates a light of range 20.5 by
  `max +10` and then `max −30`. (I)
- **All 1204 `LightState`s name their definition's own `lights` entry**,
  never a gamez node and never the sun. (V)
- **`ObjectOpacityState`/`FromTo`'s `state` is the node's active flag**:
  fades run from `{1.0, true}` to `{0.0, false}`, visible to gone. (I)
- **Definitions invalidate themselves as they start** (`dh_doors_open`,
  `misc01`…), so `InvalidateAnimation` must disarm without stopping the run
  in progress. (V)
- **`CallAnimation.parameters`**: `AtNode` 3365 (two thirds on `*.flt`
  effect templates — play the template *at* that node), `WithNode` 13,
  none 1969; `operand_node` 410 (m1's `oldpiergone_01` plays `generic_6` on
  `oldpier_01`). `INPUT_NODE` occurs 2522 times, all inside effect and
  connector templates. (V)
- **Not in Recoil at all**: `DetonateWeapon`, `PufferState`,
  `FbfxCsinwaveAtNode`/`Screen`. `proximity_damage` is false on all 4798
  definitions. `forward_rotation` only ever has the `Time` form. (V)

**Handler coverage.** "Done" means implemented and
covered by a headless test in `crates/bft-sim/tests/anim_events.rs`.

| Variant | Events | Status | Used for |
|---|---:|---|---|
| `ObjectActiveState` | 14 591 | done (11) | visibility + collision of every prop, objective flips |
| `CallAnimation` | 5 347 | done; `wait_for_completion` (70) not honoured | effect templates at nodes, entry chains, destruction |
| `ObjectMotion` | 5 219 | partial: stops at first landing; `rnd_xz` as ± spread; `forward_rotation` as a tumble about own x; `impact_force`/`complex` ignored | debris, pickups popping, rotors, smoke drift |
| `ObjectMotionFromTo` | 3 705 | done; `morph` (373) timed but not shaped | doors, gates, elevators, effect growth |
| `Sound` | 3 093 | done → sound layer | everything audible |
| `CallSequence` | 2 983 | done (11) | trails, sub-sequences |
| `Effect` | 2 299 | done → effects (placeholder sprites) | explosions, smoke, fire |
| `StopSequence` | 1 716 | done (11) | stop trails |
| `If`/`Endif` | 1 471 | VM (11) | LOD and range branches |
| `InvalidateAnimation` | 1 286 | done: disarms only | one-shot props |
| `ObjectConnector` | 1 274 | partial: model stretched along z between the ends | pickup beams, exhaust and wake trails |
| `Loop` | 1 254 | VM (11) | trails, looping props |
| `ObjectScaleState` | 1 250 | done | template resets |
| `LightState` | 1 204 | done → `PointLight` | muzzle and explosion lights |
| `Else` / `Elseif` | 843 / 797 | VM (11) | |
| `ObjectOpacityFromTo` | 576 | done | fades |
| `FbfxColorFromTo` | 447 | done → full-screen tint | flashes, the nuke |
| `ObjectTranslateState` | 430 | done | template resets, the elevator lifting `bft_00` |
| `StopAnimation` | 383 | done (11) | |
| `LightAnimation` | 313 | done; `range_alt` (1) ignored | light flicker and fade |
| `ObjectRotateState` | 194 | done | template resets, missiles at `INPUT_NODE` |
| `ObjectMotionSiScript` | 176 | done, linear | VTOL flights (m1–m6), the bus |
| `ObjectCycleTexture` | 169 | done: rewinds the interp cycle | wakes, splashes |
| `SoundNode` | 149 | done → sound layer | looping fire, generators |
| `Callback` | 119 | done (11) | control hand-over, mission end |
| `CallObjectConnector` | 104 | partial: ends passed live, `save_index` ignored | trail segments |
| `ObjectAddChild` | 103 | done; sound-node children passed to 13 | sounds riding props |
| `FbfxCsinwaveFromTo` | 78 | simplified: holds its timing, draws nothing | screen ripple |
| `ObjectOpacityState` | 52 | done | |
| `FogState` | 19 | done → `WorldEnv` (altitude kept, not drawn) | m2–m6 sky changes, exits |
| `CameraState` | 16 | done: zoom and clip | death and exit cameras |
| `ResetAnimation` | 16 | done (11) | m13 teleporters, m4/m6 |
| `ObjectDeleteChild` | 13 | done | `redsprks.flt` |
| `CameraFromTo` | 6 | done: zoom | the six `bft_00` death anims |
| `AnimVerbose` | 1 | done: logged | m5 `start_clones` |

Confidence: verified for every count and shape; inferred where marked.

## Instancing

Counted over all 13 levels' compiled defs against their `gamez` roots.

- **1944 definitions are rooted at a prototype** (a parentless non-World
  root): the effect templates (`lg_impact.flt`, `kill_laser`,
  `expredparts.flt`), the wrecks (`hulk_small`), the pickups and the enemy
  kinds (`scou_easy`). All but 18 are plain `OnCall` with no prerequisite
  and no `execution`; the 18 are level scripts that happen to live on a
  root (`call_flufvtol`, `call_chopper`, `radar_spin`, m2's `litening`,
  m6's `total_destruction`). m1 alone has 155. (V.)
- **A prototype's definition is played as a copy wherever a call places
  it**: `AtNode`, a `CallObjectConnector`'s ends, an `operand_node`. The
  unplaced calls to prototype-rooted definitions are scripts, not effects —
  `m1_start_animation` and `weapon_impact` on `camera1`,
  `countdown_from_9` — and keep the one instance. (Inferred from the call
  shapes; the engine source is not available.)
- **A copy lives until it completes, and the scripts rely on that.**
  `hulk_small` ends in `hulk_stay`, an `ObjectActiveState` under
  `Loop -1`: the wreck stays because its script never ends. Trail loops
  (`exp_medium.flt`'s `p1trail`) are stopped by the part's
  `bounce_sequence` on landing, which is what lets the copy finish. (V.)
- **An enemy's death animation is the definition named after its kind**,
  rooted at that kind's prototype, which is also what its model is cloned
  from: `scou_easy_01` → `scou_easy`. No reader key names it; the name
  matches in every level. It plays on the dead enemy's own model (`shadow`
  and `healthy` off, `hulk_small` at `healthy`, `fire_40` by connector,
  `deadman1.flt`), with a 0.4 `RandomWeight` choosing between a standing
  wreck and a hull flung up and blown apart. (V.)
- **Operand calls split by their target's root.** `generic_6` (312) and
  `under_water_exp` (8) are prototype-rooted and play a copy on the
  operand. `destroy_bunker`, `destroy_bridge`, `destroy_turret` and
  `fly_sign` are mostly one world-rooted script played on other props (83
  of 90 calls). Each of m1–m6 has exactly **one** `destroy_turret`, rooted
  at its first turret (`tur_101`, m4's `tur_201`, m6's `a2tur_01`), so
  `ai.zrd`'s `DESTROY_ANIM` has to be played on every other turret as an
  operand. (V.)
- **A copy can run out of cursors on its first tick and still be
  running.** `kill_laser`'s only `Initial` sequence does nothing but
  `CallSequence` its others, which land after the sequence ends. (V.)

## Cinematics

Asserted in `crates/bft-sim/tests/real_cinematics.rs`, which also pins the
entry animations, the callback groups, the intros and the exits.

- **`camera1` looks down its −Z**, Bevy's camera convention, under the SI
  rotation read scalar-in-`x`. Over all six intros, sampled every 0.5 s, its
  −Z points at the VTOL (m1: at the tractor once it drives) in 228 of 229
  samples and its +Z in none. The play camera copies `camera1`'s world
  transform unchanged. (V)
- **`ObjectRotateState`'s Euler order is `Rz·Ry·Rx`** (x applied first),
  what `AxisConvention` already does: it reproduces every VTOL fly-in's last
  SI quaternion to ≤ 0.11° for m2–m6, and the other five orders miss by
  1.2–5.7° where the angles are not degenerate. `camera1`'s reset rotation
  is *not* its script's end (62–169° apart): the reset re-aims the camera
  for the hand-off. (V)
- **m6's parked VTOL is never meant to be seen.** m6's reset puts `vtol1` at
  `(741, 22.7, −356)`, 720 m from the bay; the `m6_airdrop_loop` it calls
  starts within 1 cm of where the fly-in ended. Taken literally for one
  tick, the parked pose makes `m6_do_exit_trigger` (called in the same
  reset) read `PlayerRange` as "far" and fly the VTOL off with the player
  in it; the engine poses the called scripts' first frames with the reset.
  (V for the poses; I for the ordering, which the data cannot show.)
- **m1's BFT rides the trailer out.** The intro starts the truck at 14 s and
  hands over at 22.95 s: by then the trailer has driven 76 m (≈ 8.5 m/s) and
  the camera ends 18 m behind it. `bft_00` is a child of `world1`, not of
  `trailr`, and m1's intro fires no `Callback 11`, so the player is
  simulated on the moving bed rather than carried; the engine has to carry
  it. (V)
- **The airdrop hover runs only after the hand-off** — it is called from the
  reset — and stops once the player is 27 m (m6: 25 m) from the VTOL. With no
  player position at all, `PlayerRange` is false and the VTOL leaves at
  once, so the range must be the player's from the first tick. (V)
- **m2's hand-off pose is upright to within a degree**: the reset's 2.5°
  pitch and −2° roll largely cancel once composed. (V)
- **The landed VTOL's bay is 3.72 m clear** floor to ceiling, and the door
  frame over the ramp comes down to ≈ 4.4 m above the ramp: room for the
  2.825 m BFT only if the hull rides on its tracks. A 1 m gap separates
  `interior_floor` from `cdoor_floor` at the hinge; the ramp itself is
  ≈ 15°. (V)
- **Script-moved level colliders drift under avian.** A level node is a
  static body; when a script moves an ancestor (`vtol1`), avian writes the
  child's pose back through the parent's lagging `GlobalTransform`, and the
  error compounds: left alone, m3's `interior_floor` ends the fly-in rolled
  15°, although nothing scripts it. The sim restores static level bodies'
  local transforms after the step. (V)

Not settled by the data, recorded for the open questions: what `Callback 20`
means, whether the original let the player skip intros and with which key,
and whether input was locked during the exits (`2`/`14` fire there too, so
this engine locks it).

## The original's save games

- **Save/load existed in-mission.** Manual: Esc menu has *Save Game* (in
  game only) and *Load Game*; keys **F8** save, **F9** load. Messages
  130–140: `MSG_CANT_SAVE`/`MSG_CANT_LOAD` ("…while the BFT is being
  transported"), `MSG_CANT_SAVE_MP`/`MSG_CANT_LOAD_MP`,
  `MSG_SAVE_OVERWRITE_HDR/MSG`, `MSG_SAVE_FAILED_MSG`.
- **The original's format is out of scope** (REQUIREMENTS §5); nothing in
  the data describes it.
- **A load calls `load_game_anim`**, `load_control` in all six missions:
  one sequence, callbacks 15, 10, 1 — control to the player. The engine
  restores the world, then calls it instead of the intro.
- **`save_log`** is true on 14–21 defs per mission, all transient effects
  and trails (`muzzleburst.flt`, `wake_trail(_fade)`, `bftsplsh.flt`, …)
  plus `bft_00` and the VTOLs. Reading it as "not worth saving" fits, but
  it is a hypothesis; open-bft saves every copy regardless.

## Impacts and debris trails

- **A definition can be rooted inside a prototype, not at its root.**
  `smoke1` and `smoke2` hang off the model node under `smoke1.flt` and
  `smoke2.flt` (all 13 levels; m1–m6 also have a second `smoke1` under
  `smoke_damage.flt`). They are placed like any effect — `exp_medium`'s
  debris trails, and the other `pNtrail` sequences, call them `AtNode`
  every 0.2 s — so "prototype" means *anywhere in a template subtree*.
  Keyed on template roots only, no trail puff would be drawn. (V,
  `real_anim_events.rs`.)
- **An `ObjectOpacityFromTo` end's `state` is the node's active state at
  that end, and the node shows while either end does.** `smoke1` fades
  `{0.0, false}` → `{1.0, true}`; read literally, the puff would be hidden
  for its whole fade-in and pop in at full strength.
- **A falling `ObjectMotion` lands on a liquid's surface too.** Every
  `bounce_sequence`/`bounce_sound` variant for `water` and `lava` in the
  game is null, and what falls into the sea (`train_fall1`–`4`,
  `pcar_flies`) keeps its splash in `default`, so a liquid landing runs the
  default. The trails depend on it: a trail stops only in its part's
  bounce sequence, and a part that sank never landed. open-bft also runs
  the sequence (silently) when a motion's `run_time` ends in the air, so
  no trail outlives its part; that fallback is ours.
- **`weapons.zrd`'s `IMPACT` names are all definitions in every level**
  (V, all 13): `impact_miss`, `water_miss`, `ric_green.flt`, `lg_impact.flt`,
  `greenparts`, `expnuke.flt`, `csinwave`, `small_fire_effect`, …, and
  resolve the way `FIRE/ANIMATION` does, by anim name or root. Read as:
  `ANIMATION` where a shot misses; `DAMAGE_ANIM_ON_HEALTH` (else
  `DAMAGE_ANIMATION`) on a target left alive, by its health fraction;
  `KILL_ANIMATION` on a kill; `ANIMATION_ALWAYS` plays `ANIMATION` on a hit
  as well; `EFFECT` (an `effects.zrd` name) on every impact. `impact_miss`
  — the mortars' and missiles' — is orange debris, `flare_orange.flt` and
  the `exp_bunk` sprite, all in its first tick. The miss/hit split is our
  reading of the key names; the original's exact rule is not in the data.
- **Every `effects.zrd` model is a prototype in every level** except the
  two splashes' (`splash1_splash`, `bsplsh_splash`), and every one has an
  identity root transform. `videxp.flt`, `boxexp.flt`, `expcrate.flt` and
  `expbunk.flt` all call their one model node `expld4`.

## REQUIREMENTS §9, question 3

Mission definition location: **answered**. Objectives: per-mission
`objectives.zrd` (picture, `MSG_M<n>_O<k>_{SUMMARY,OBJ,COMP}` text keys,
initial state, completion node(s), sounds) + `messages.dll` (32
objectives over six missions) + zmap feature `objective` ids for the
automap. Briefing: `briefing.zrd` layouts + the same message keys +
`video/m0N_01.avi`. Player start: `aiv.zrd/bft_00` (position + heading)
— not the gamez node transform. Mission script entry: `startanims.zrd`
→ anim `start_single_player` / `load_control`. Death: anim `deathmN`
(`bft_00`), triggered by the engine when player health reaches 0.
Countdowns: anim `countdown_from_9` on the empty node `countdown`.
Win: no explicit win condition record; the last objective's completion
(m1: the VTOL pickup `m1_exit_trigger`, `ByRange` 30.25 = 5.5 m) ends
the mission via **`Callback` 99** from `mN_exit_animation` (2 also fires
during `mN_start_animation`); `MSG_LOAD_NEXT_MISSION`. Lose: player
death, whose cinematic is anim **`bft_00`** — `deathmN.zrd` is the
reader *source* file's name, and the compiled definition is
`bft_00-bft_00-body`. Form unlocks: the `pu037`–`pu039` chip pickups
(`MSG_*_PICKUP`), placed by `puppies.zrd` (m2 objective 1 is "collect
the amphib chip"). Details in READER-SCHEMA §3, reader.md and above.

Confidence: verified with inferred details (objective completion rule, win/lose wiring).

## REQUIREMENTS §9, question 7

anim.zbd extraction: **answered** in install.md and above. Works on all 13 missions;
semantics of activation, execution, reset and event timing are above;
the reader anim sources are the readable form of the same data and are
not needed at runtime.

Confidence: verified.
