# READER-SCHEMA — shape and meaning of the `zrdr.zbd` entries

Produced from `unzbd rc reader` output of the retail v1.0 data. Companion to
`docs/data/reader.md`. Every key carries a confidence tag:

- **V** verified by cross-referencing another file (gamez nodes, anim
  defs, messages, sound archive, interp scripts) or by arithmetic;
- **I** inferred from the key name and value ranges alone;
- **U** unknown; the shape is recorded but not the meaning.

Tool: `tools/recon/zrd.py <zip> <entry> [--path A/B] [--keys|--schema]`
renders any entry as an indented key/value tree from the extraction cache.

## 0. Format conventions

1. **Key/value lists.** A reader file is one JSON value: a list whose
   elements alternate `"KEY", <value>` where `<value>` is a list. Nesting
   repeats the convention, so `["vehicle", ["bft", ["common_mode", [...]]]]`
   is a three-level map. Keys are ordered and may repeat (e.g. several
   `SEQUENCE_DEFINITION` keys in one animation definition); a loader must
   keep a `Vec<(String, Value)>`, not a `HashMap`, at least for anim
   sources. The outermost value is wrapped in one extra single-element
   list. (V)
2. **Null values.** A key with no value is followed by JSON `null`, not by
   an empty list (mech3ax collapses a one-element list into its element;
   an empty list becomes `null`). `["ANIMATION", null, "SOUND", null]` means
   "keys present, nothing assigned". A loader must accept `null` wherever
   a list is expected. (V)
3. **Mixed lists.** Some values are positional records rather than maps:
   `["snd_zero", "zero.wav", "3D", "HIGH", "MED", "LOW", [22050,16,1], …]`.
   These are described per file below. A loader needs both `get(key)` and
   `nth(i)` access on the same `ReaderValue`. (V)
4. **Numbers.** Integers and floats are distinct in the JSON (`8` vs
   `8.0`); the reader format has separate int/float types. Angles are in
   **degrees** in reader data (see `aiv.zrd`, `location.zrd`) even though
   gamez stores radians. Distances are in world units = metres (docs/data/coordinates.md). Times are seconds. (V for degrees and metres; times I)
5. **Names.** Node references are bare gamez node names. Model references
   end in `.flt` and name a gamez subtree loaded from that OpenFlight file
   (the node keeps the `.flt` suffix, e.g. `pturret.flt`). Anim references
   use the `ANIMATION_NAME` of a definition, not the file name. Sound
   references use logical names `snd_*` resolved through `sounds.zrd`.
   Message references use `MSG_*` keys resolved through `messages.dll`.
   Image references (briefing pictures, weapon icons) are bare stems in
   `image.zbd`. (V, each checked at least once)
6. **Wildcards.** In animation-definition sources a `NAME` may contain `*`
   (`tur_1**`, `sndbg0*`). The build tool instantiated one compiled anim
   def per gamez node matching the glob (`sndbg0*` → `sndbg01`…`sndbg04`
   in `anim.zbd`), and `*` inside sequence node names is substituted with
   the same match (`tpobj*2`). At runtime the compiled `anim.zbd` is
   already expanded; only `ai.zrd`'s turret keys still need glob matching
   against node names. (V)
7. **File paths** inside values use the original authors' Windows tree
   (`..\data\common\zrdr\enemies\drone.zrd`). The archive stores every entry
   by its bare file name; the `weapons\`, `enemies\`, `explosns\`, `vtol\`
   prefixes exist only in these path values, and entries with the same stem
   in different source directories were merged by name (there are no
   collisions in the shipped data). (V)

## 1. Which archive holds what

| Archive | Entries | Gameplay-relevant | Anim-definition sources | UI/config |
|---|---|---|---|---|
| `zbd/zrdr.zbd` (global) | 127 | `vehicle{,_easy,_hard}`, `player`, `weapons`, `pickup`, `sounds`, `effects`, `weather`, `briefing` | 101 files (`anim`, enemies, effects, explosions, `regen`, `shock`, `load_control`, `deathmulti`, `bftmulti` …) | `hud` (§2.7a), `cockpit`, `dialog`, `detail`, `fmv`, `fonts` |
| `zbd/mN/zrdr.zbd` (m1–m6) | 55–150 | `ai`, `aiv{,_easy,_hard}`, `net_NN` (55–92 files), `net`, `puppies{,_easy,_hard}`, `objectives`, `location`, `movers`, `declient`, `startanims` | `anim` (the include list), `start_single_mN`, `start_*`, `deathmN`, `mNobjects`, `mNpickup`, `mNstartcam`, `mNvtols`, `countdown`, level props | — |
| `zbd/mN/zrdr.zbd` (m7–m13) | 14–20 | `ai`, `aiv` (player only), `puppies`, `net`, `location`, `objectives` (placeholder copies of m2's), `movers`, `declient`, `race` (m9) | `anim`, map props, `countdown`/`startgate` (m9), `teleprtr`/`bft_trans` (m13) | — |

Difficulty variants: `vehicle_easy`/`vehicle_hard` (global) and
`aiv_easy`/`aiv_hard`, `puppies_easy`/`puppies_hard` (per campaign mission)
are complete copies with a handful of values changed; the engine picks one
file per difficulty, it does not merge. `rumv_easy`/`scout_easy`/`tren_easy`
are *separate enemy types* (own vehicle key, own anim source) placed
explicitly by `aiv*.zrd`, not a difficulty switch. (V)

## 2. Global archive

### 2.1 `vehicle.zrd`, `vehicle_easy.zrd`, `vehicle_hard.zrd`

Top level: 25 keys, one per vehicle type: `bft` (player), `stealth`
(an invulnerable stand-in used by the intro/multiplayer camera, health
10000), and the enemies `rumv`, `scou`, `amphi`, `lock`, `ntank`, `tren`,
`dron`, `ftank`, `ltank`, `ldestank`, `arc`, `stank`, `htank`, `subt`,
`minelay`, plus variants `rumv_easy`, `scou_easy`, `tren_easy`, `ntank_2`,
`tren_2`, `ltank_2`, `rumv_2`, `dron_2`. Each key is a map with a
`common_mode` block and one or more mode blocks: `track_mode` (19 types),
`hover_mode` (4), `amphib_mode` (2), `sub_mode` (2), `basic_mode`
(`stealth` only). The player has all four. The type key is what `aiv.zrd`
instance names are derived from (`scou_03` → `scou`) and what the interp
build scripts loaded as template subtrees (`LoadGameGen scout.flt scou`). (V)

`common_mode` keys:

| Key | Shape | Meaning | Conf. |
|---|---|---|---|
| `name` | `[str]` or `[str, str]` | display name words (`"Sub","Tank"`); absent on `bft` | V |
| `health` | `[f]` or `[f, f]` | hit points; player `[140, 60]` = hull, and probably shield/second bar | I |
| `weapons` | list of 9-element rows | see below | partly I |
| `camback` | 3 × `[x,y,z]` | chase-camera offsets in hull space (behind/up); three entries = three camera modes | I |
| `aimy` | `[f, f]` | turret aim height(s) above hull origin | I |
| `activation` / `activation2` | `[f]` | radius (m) at which the unit wakes up; 120–300 | I |
| `pursuit_range` | `[f, f]` | (min, max) distance the unit keeps from the target while attacking | I |
| `not_pursuit_dwell` | `[f]` | seconds to wait before giving up pursuit | I |
| `return_range` | `[f]` | distance from home beyond which it returns | I |
| `pickups` | `[str, int]` | drop on death: pickup kind (a `pickup.zrd` key) and amount | V |
| `nanite` | `[int, int]` | nanite (health) drop on death: count and value | I |
| `track_switch` | `[f,f,f]` | player only; speeds at which track animation stage changes | I |
| `sounds` | map | `skid`, `weapon_up`, `weapon_down`, `weapon_select`, `pinging` → `snd_*` | V |

`weapons` rows: `[weapon_id, a, b, ammo, interval, c, d, e, f]`, e.g. player
`["wep_1.0", 1, 1, 123456789, 0.15, 5, 100, 1, 2]`, scout
`["wep_1.0", 1, 1, 5000, 0.15, 5, 325, 0, 1]`. `weapon_id` is a
`weapons.zrd` `BALLISTICS` key (V). `ammo` matches `AMMO_LIMIT` for the
player's unlimited RFPG and is the magazine for enemies (V). `interval` is
seconds between shots (0.11–2.5; matches `ai.zrd` turret `FIRE_RATE`
values) (I). `b` is 1 for the player's `wep_1.0`/`wep_1.1` and every
enemy's single weapon, 0 for the player's other 16 rows: **initially
owned** (I). `d` is 325/275 for enemies (engagement range, m) and
5–300 for the player (I, weaker). `a`, `c`, `e`, `f` are small integers
(1–7, 5–10, 0/1, 1/2): U — candidates are HUD slot group, burst count,
strong/weak flag and a damage class. The engine reads `weapon_id`,
`ammo`, `interval`, `b` and treats the rest as opaque until play-testing.

Mode blocks (`track_mode` etc.) share these keys:

| Key | Shape | Meaning | Conf. |
|---|---|---|---|
| `mode` | `[str]` | `track` / `hover` / `amphib` / `sub` | V |
| `platform` | 4 × `[x,y,z]` | hull-space corners of the suspension/ground-sample quad (player track: ±2.25 × ±2.4) | I (matches the 5.7 × 7.4 m model) |
| `collision` | 12 × `[x,y,z]` | hull-space collision hull points (y = 1) | I |
| `mode_alt` / `mode_alts` | `[f]` | ride height offset: track 0, amphib −0.1, hover 0.5, sub −0.5 | I |
| `rates` | `[f, f]` | forward and reverse top speed, m/s (player track 48/65?) or accel/top; see note | I |
| `turns` | `[f, f]` | turn rate stationary / moving, rad/s | I |
| `turn_damping`, `rate_damping` | `[f]`, `[f, f]` | damping constants for yaw and speed | I |
| `friction` | `[f, f, f]` | **static, kinetic**, rolling friction. Was read as longitudinal/lateral; `[0] > [1]` in all five blocks that have it and `[1]/[0]` is 0.50–0.67, the kinetic/static band (docs/data/reader.md) | I |
| `chas_smooth`, `chas_pitch`, `chas_roll` | `[f]`, `[f,f,f]` | visual chassis smoothing and pitch/roll response (gain, per-speed, clamp) | I |
| `collision_d` | `[f, f]` | collision damage factor and threshold | I |
| `mass`, `a_damping` | `[f]` | mass (2.2–2.5 "tonnes") and angular damping | I |
| `quicksand_slowdown`, `lava_slowdown` | `[f]` | speed multiplier on `Soil::Quicksand` / `Soil::Lava` | V (soil use confirmed, docs/data/water.md) |
| `ok_to_amphib` | `[int]` | 1 = may transform to amphib from this mode | I |
| `alt_control` | `[f,f,f]` | hover altitude controller gains | I |
| `gun_pitch` | `[f, f]` | sub only: turret pitch limits (−0.17…0.5 rad) | I |
| `hover_wave`, `amphib_wave`, `sub_wave` | 7 × f | bobbing oscillation parameters (amplitude, frequency, phase per axis) | I |
| `t2a_anims` … `s2a_anims` | `[str, str]` | anim names played for each form transition: tracks↔amphib↔hover, amphib↔sub. Each is a pair "leave old form", "enter new form" (`tracks_to_none`, `none_to_amphib`); the defs live in the global `anim.zrd` source and compiled `anim.zbd` (`bft_00-*`) | V |
| `sounds` | map | `engine`, `skid`, `collide`, `land`, `pitch_scale`, `volume_scale` | V |

`rates` note: player track `[48, 65]`, amphib `[22, 50]`, hover `[20, 55]`,
sub `[25, 55]`; enemies `[12, 45]`. Two readings fit: (top speed m/s,
acceleration) or (reverse, forward). Tune-tested as (top speed, reverse)
in m/s (`bft_sim::vehicle`'s module docs). (I)

Transition graph implied by the keys: the player has no direct
tracks↔sub pair; sub is reachable only from amphib (`a2s`, `s2a`). (V)

### 2.2 `player.zrd`

Flat map, 19 keys, all player-specific physics/camera constants:

| Key | Value | Meaning | Conf. |
|---|---|---|---|
| `camera_zone` | 0.7 | dead-zone fraction before the camera starts following turret yaw | I |
| `max_cam_yaw_rate` | 0.87 | rad/s | I |
| `mouse_push` | `[0.002, 0.01]` | mouse sensitivity (fine, coarse) | I |
| `fp_cam_el_rate`, `fp_cam_el_lim` | 3.5, `[-0.6, 0.5]` | first-person camera elevation rate and limits (rad) | I |
| `underwater_cam` | 8 floats | sub camera: fog density, near, far, RGB colour (104,30,85 → purple-blue), tint strength | I |
| `camera_elastic`, `max_cam_tether_angle` | 0.04, 20 | chase-camera spring and max angle (deg) | I |
| `nom_gravity`, `wat_gravity`, `qsd_gravity` | 40, 9, 3 | gravity in normal / water / quicksand, m/s² — **40 m/s², not 9.8** | V (units from §4) |
| `qsand_sink`, `lava_sink` | 0.4, 0.3 | sink rate multipliers | I |
| `splash_rate` | `[3, 1.5]` | splash effect spawn cadence | I |
| `max_slope` | 0.8192 | cos of max climbable slope (≈35°) | I |
| `burning_anim` | `["burning_model", 4]` | anim + duration while on fire | I |
| `make_hot`, `make_cold` | `wep_6.1`, `wep_3.1` | weapons that set/clear the burning state (laser sabre, napalm → hot? actually `wep_3.1` = napalm is `make_cold`, so the roles are: napalm ignites, laser… read as "weapon whose hit toggles fire") | U |
| `low_shield_snd` | `["snd_lowshield", 10, "snd_warn3"]` | warning sound, threshold, alarm | V |

The 40 m/s² gravity with 1 unit = 1 m is the "low gravity feel" of the
original inverted: jumps are short and fast. The engine takes
`nom_gravity` as the primary value; `wat_gravity` applies while the
altitude surface has `Soil::Water`.

### 2.3 `weapons.zrd`

Top level: `VERSION 2`, four warning sounds (`LOCK_ON_WARNING`,
`NO_AMMO_WARNING`, `TRIGGER_INACTIVE`, `WEAPON_INACTIVE` → `snd_*`), and
`BALLISTICS`, a map of **19** weapons keyed `wep_S.V` where `S` is the HUD
slot 1–9 and `V` the variant (0 = weak, 1 = strong; `wep_9.2` is the
enemy-only small lock-on missile). (V — the slot digit matches the
`pickup.zrd` gun images and the manual's 9 × 2 layout.)

| id | NAME | DESC (messages) | slot/variant |
|---|---|---|---|
| wep_1.0 / 1.1 | RFPG / ERFPG | Rapid Fire Pulse Gun / Enhanced | 1 |
| wep_2.0 / 2.1 | HEM / MDM | High Explosive / Molecular Distortion Mortar | 2 |
| wep_3.0 / 3.1 | FREON / NAPALM | Freon Burst Cannon / Napalm Cannon | 3 |
| wep_4.0 / 4.1 | P-HEM / P-MDM | Proximity HE / MD Mine | 4 |
| wep_5.0 / 5.1 | (P-HEM) / (P-MDM) | Remote HE / MD Mine (`MSG_DESC_R_*`) | 5 |
| wep_6.0 / 6.1 | LASERD / LASER | Laser Designator / Laser Sabre | 6 |
| wep_7.0 / 7.1 | sonic / ARC | Sonic Burst Cannon / Arc Sabre | 7 |
| wep_8.0 / 8.1 | LO-EM | Lock On / Tether Guided Explosive Missile | 8 |
| wep_9.0 / 9.1 | LO-EM | Lock On / Tether Guided Practical Nuke | 9 |
| wep_9.2 | LO-SM | Small Lock On Missile (enemies) | — |

Per-weapon keys (present-count out of 19 in brackets):

| Key | Shape | Meaning | Conf. |
|---|---|---|---|
| `NAME` [19], `DESC` [19] | `[str]` | short HUD name; `MSG_DESC_*` message key | V |
| `KILL_VERB` [18] | `[str]` | `MSG_KILL_VERB_*` for the multiplayer kill line (`wep_6.1` has a literal `"executed"`) | V |
| `AMMO_LIMIT` [19] | `[int]` | max carried; 123456789 = unlimited | V |
| `DAMAGE` [19] | `[f]` | hit points removed on direct hit (1.2 … 140) | V (consistent with vehicle `health`) |
| `RANGE` [19] | `[f]` | max travel distance, m | I |
| `FIRE_RATE` [**19**] | `[f]` | **≥ 3.0 is shots per second, below it seconds between shots** — nothing lies between 2.0 and 12.0. Checked against the player's `interval` column (docs/data/reader.md) | V |
| `VELOCITY` [16] | `[f]` | projectile speed m/s (60–120) | I |
| `GRAVITY` [16] | `[f]` | 9.8 for ballistic (mortars, mines), 0 for straight/homing | V (ballistic set matches) |
| `ACCELERATION` [5], `TURN_RATE` [5], `LOCK_ON` [6] | `[f]` | homing missiles: accel m/s², turn rad/s. **`LOCK_ON` is a float, not a mode**: 0.1 (tether-guided), 1.0 (lock-on), 2.0 (`wep_9.2`), **0.0** (`wep_7.1`, a beam). Only a positive value means "homes" | I; float-ness V |
| `PITCH_RATE` [2] | `[f]` | `wep_8.1`, `wep_9.1` — the two `TETHER_GUIDED` weapons: a separate vertical turn rate | I |
| `TETHER_GUIDED` [2] | `[int]` | `wep_8.1`, `wep_9.1`: steered by the player's aim (interp `WeaponSetMaxTetherAltitude 30`) | V |
| `MINE` [4], `RELOAD` [9], `REMOTE_DETONATE` [2] | `[int]` | mines; reload = one-at-a-time (unverified); remote = set off by the detonate key — the manual: "they will not detonate until you trigger them" (Spacebar in the original). Proximity mines trip on hostile traffic; `IMPACT_PROXIMITY` is the blast, not the trigger | I; remote V (manual) |
| `IMPACT_PROXIMITY` [19] | `[f]` | splash / trigger radius m (0 for direct-hit guns, 256 for the nuke) | V |
| `IMPACT_TYPE` [11] | `[int]` | 0 or 2; 2 on area weapons | U |
| `CRATER` [6] | `[int,int]` | read as radius m, depth dm, capped by `declient.zrd` `RADIUS`/`DEPTH` (HEM, both HE mines, `wep_8.1`, both nukes). The second value always exceeds the first, so it cannot be metres of depth | I |
| `FLYOUT_HEALTH` [11] | `[int]` | projectile can be shot down; hit points | I |
| `BEAM` [3] | `[f, f, int]` | hitscan weapons `wep_6.0`, `6.1`, `7.1`: (?, width, ?) | I (existence V) |
| `MULTI_TARGET` [1] | `[int]` | arc sabre chains to several targets | I |
| `CATCHES_FIRE` [1] | `[int]` | napalm sets targets burning (`player.zrd` `burning_anim`) | I |
| `QUICKSAND` [3] | `[int,int]` | creates a quicksand patch (`declient.zrd`) — the freon/MD weapons | I |
| `FIRE` [19] | map or mixed | `ANIMATION` (muzzle `.flt`), `SOUND`, `RANDOM_ROTATE` | V |
| `FLYOUT` [19] | map or mixed | `MODEL` (`.flt` projectile model), `MODEL_ANIMATION`, `ANIMATION` (trail e.g. `smktrl1.flt`), `SOUND` | V |
| `IMPACT` [19] | map | per surface: `default`, `water`, `quicksand` (keys match `Soil`), plus `HEAT` (`wep_6.1`), **`FREEZE` (`wep_3.0`)**, `DESIGNATE` (`wep_6.0`) and `ANIMATION_ALWAYS` (both nukes). Those four are **siblings of the surfaces**, not children of one — a parser walking `IMPACT`'s pairs as surfaces reads them as a fourth. Every weapon has `default`; only `wep_7.0` lacks the other two | V |
| `IMPACT/<surface>/*` | | `ANIMATION` (impact `.flt` or anim name like `impact_miss`, `water_miss`), `KILL_ANIMATION` (`kill_*` defs played on the victim), `DAMAGE_ANIMATION`, `DAMAGE_ANIM_ON_HEALTH` (list of `[fraction, anim]` thresholds), `EFFECT` (`effects.zrd` name), `SOUND` (one or a random pick of several), `BOUNCE_SOUND` | V |
| `BOUNCE` [1] | map | mines: `SOUND` | V |

`impact_miss` and `water_miss` are gamez node names present in every level
(`water_miss` has no model; it is an empty node that impact effects are
attached to). (V)

### 2.4 `pickup.zrd`

`PICKUP_DATA`: map of **40** pickup kinds (not 43 — measured)
→ `{SOUND: snd_*, IMAGE: gun_NN}`.
Kinds are `<WEAPON>_AMMO` and `<WEAPON>_WEAPON` for ERFPG, HEMORTAR,
QMORTAR (=MDM), FREON, NAPALM, P_HEMINE, P_QMINE, R_HEMINE, R_QMINE,
LOCKON_LASER (=designator), LASER_SABRE, SONIC_CANNON, ARC_SABRE, MISSILE,
NUKE, GUIDED_MISSILE, GUIDED_NUKE, plus `NANO-CANISTER`, `NANITE100`
(health), and the form chips (amphib, hover, sub — `MSG_*_PICKUP`
strings). `IMAGE` is the HUD icon in `image.zbd` (`gun_01`…`gun_12`), so
weapons sharing an icon (`gun_02` for HEM/MDM, `gun_03` for all mines)
share a slot. The chips are keyed `PUP_AMPHIB`/`PUP_HOVER`/`PUP_SUB`, and
a plain **`NANITE`** sits alongside `NANITE100` and `NANO-CANISTER`; icons
run to **`gun_13`**, not `gun_12`. (V)

`ANIMATION_DEFINITIONS`: the source of the compiled `pu000`…`pu039` anim
defs (spin + bob + `OBJECT_CONNECTOR` beam). Runtime uses the compiled
copy in each level's `anim.zbd`. (V)

Pickup *models* are the 40 template subtrees `pu000`…`pu039` loaded by
`support\pickup.gw` in this order. **This order is authoritative and is not
`PICKUP_DATA`'s**: the script loads `lomissammo, gmissammo, nukeammo,
gnukeammo` where `PICKUP_DATA` lists missile, *nuke*, guided missile,
guided nuke, so deriving the mapping from the reader puts the nuke's model
on the guided missile. `pu000` erfpg ammo, `pu001` HE mortar
ammo, `pu002` MD mortar ammo, `pu003` freon, `pu004` napalm, `pu005`–`pu008`
mine ammo ×4, `pu009` designator, `pu010` laser, `pu011` sonic, `pu012`
arc, `pu013` LO missile, `pu014` guided missile, `pu015` nuke, `pu016`
guided nuke, `pu017`–`pu033` the matching guns, `pu034` mini nanite, `pu035`
nanite100, `pu036` nanite2, `pu037` amphib chip, `pu038` hover chip,
`pu039` sub chip. Each is a root node (no parent) with children `box`,
`g6` (LOD group) and `bvol` (pickup trigger volume). Runtime instances are
named `pu<NNN><ii>` (`pu03700`). (V for the list and
the subtree; instance naming I)

### 2.5 `sounds.zrd`

`SYNTAX 2`, `SOUND_PATH`, `SETS`: 16 sets (`COMMON` 173 entries,
`BRIEFING1`…`6`, `DIALOG`, `M1`…`M6`, `M9`, `M10`). Each entry is a
positional record:

```
["snd_chainbreak", "chainbreak.wav", "3D", "PURGEABLE", "RANGE", [60, 180],
 "HIGH", [22050, 16, 1], "MED", [22050, 8, 1], "LOW", [11025, 8, 1]]
```

logical name, file name in `sounds{l,m,h}.zbd` (without the archive
choosing), then **keywords, each value directly after its keyword**: `RANGE
[min, max]` attenuation distances, `VOLUME f` (a gain, 0.75–0.8 on the six
that have it) and `HIGH`/`MED`/`LOW` each followed by its format triple
(rate, bits, channels); bare flags `3D` (positional), `LOOPED`, `PURGEABLE`,
`VOICE` (the 40 speech lines), `FREQUENCY` (rate varied at runtime: engines,
machines) and `HARDWARE`, in no fixed order, so a parser that expects fixed
positions misreads every record. 280 records, 272 distinct names (six repeat
across mission sets, same file each time); one range is written reversed
(`snd_gunside [50, 20]`). docs/data/sounds.md has the counts. **This is the
indirection docs/data/sounds.md asked for**: `snd_m1obj1` → `m1obj1.wav`.
Sets `M1`…`M6` hold mission speech; `M9`/`M10` a few multiplayer cues; other
multiplayer maps use `COMMON` only. (V)

### 2.6 `briefing.zrd`

`CAMPAIGN1`…`CAMPAIGN13`: briefing-screen layouts, one per level, with
`IMAGE_PATH`, `FONTS` (index, face, size, style, weight, RGB),
`BACKGROUND_IMAGES` (`[stem, x, y]` in `image.zbd`), `BACKGROUND_TEXT`
(`[MSG_key, x, y, font]`), `BUTTONS` (`TRANSPORT_PROGRESS` bar), and
`PRIMITIVES` (positions for `MISSION_NAME`, `OBJECTIVE_SUMMARY`,
`OBJECTIVE_DESC` with `WORDWRAP`, `OBJECTIVE_PICT`, `MESSAGES`,
`TRANSMISSION_HALTED`, `LOCATORn`). The objective *content* is not here;
it comes from the mission's `objectives.zrd` + messages. Campaign names are
`MSG_CAMPAIGN<N>_NAME` (1 Island Storm, 2 Dust and Metal, 3 Thermal
Meltdown, 4 Poison Valley, 5 Cold Kill Chaos, 6 Network Showdown, 7
RiverWorks, 8 Crater Chaos, 9 Beach Rally, 10 City of Cranium, 11 Frozen
Tundra, 12 Wicked Waterways, 13 Transporter Trauma). (V)

A `FONTS` record is `[id, face, size, [r, g, b], flag, weight]`, the flag
0 or 1. A layout has the same shape as a `dialog.zrd` dialog
(§2.7b) and is parsed by the same code (`bft_assets::dialog::
parse_briefings`); `TRANSPORT_PROGRESS` is a `FILLBITMAP` bar;
`CAMPAIGN7`–`13` have no `PRIMITIVES`. What the fields mean is docs/data/menus.md. (V)

### 2.7 `effects.zrd`, `weather.zrd`, `fmv.zrd`, UI files

- `effects.zrd`: `EFFECTS` list of 13 sprite-cycle effects:
  `[model.flt, "NAME", [name], "SPEED", [fps], ("LOOPING", ["ON"]), "MAPS", [tex…]]`.
  Referenced by weapon `IMPACT/EFFECT` and anim `Effect` events by `NAME`
  (`smoke1`, `fire1`, `exp_bunk`…). This, together with interp's
  `CycleTextureSet*`, is where texture animation lives. (V)
- `weather.zrd`: `MISSION2` rain, `MISSION5` snow: `TYPE`, `PARTICLES`,
  `COLOR`, `WIND_DIR`, `WIND_VEL`, `GRAVITY`, `ALPHA_GRADIENT`. Other
  missions have no weather. (V)
- `fmv.zrd`: playlists (`PLAYAVI`, `SHOWIMAGE`, `WAIT`, `FADEOUT`, `BLUR`)
  for `INTRO`, `ATTRACT`, `MISSIONOVER`, `M0`…`M6`, demo screens. (V)
- `cockpit.zrd`, `dialog.zrd`, `detail.zrd`, `fonts.zrd`: menu layouts,
  graphics-detail presets and an unused cockpit layout (`SCORE`, `SPEED`,
  `LEFT_BACKGROUND`… — its art is in no
  archive). `hud.zrd` is §2.7a, `dialog.zrd` §2.7b; `detail.zrd` (detail
  presets per CPU/RAM/texture memory, `HUD_TYPEI`/`II` defaults) and
  `cockpit.zrd` are not read.

### 2.7a `hud.zrd`

The in-game HUD's layout, in pixels of a 640×480 screen. Parsed by
`bft_assets::hud::parse_hud`; what each part means, and how it was
checked, is docs/data/hud.md. A map of sections, in this order:

| Key | Value | Meaning |
|---|---|---|
| `VERSION` | `[i]` | 8 |
| `FONTS` | map `NAME → [face, height, width, weight, 0/1]` | `OBJ_SUMMARY`, `OBJ_DESCRIPTION`, `MESSAGES`, `AMMO`, `STRINGS`; Windows faces, sizes in 480-line pixels |
| `TYPEI` | `[[x0, y0, x1, y1]]` | the 3D view with no strips: the whole screen, i.e. the canvas |
| `TYPEII` | `[rect, placed…, [names…]]` | the view between the strips, the strip images, and their wider variants (`tops800`…) |
| `RETICULE` | `[str, str, str]` | in range, nothing in range, damageable |
| `SENSOR` | `[placed, zoom, [x, y], [w, h], f, placed, rect]` | `monitor`; zoom; the view's offset and size; an unused range; `targ_ret` (centred); the enemy health bar |
| `SHIELD` | `[placed, [x, y, digits, bool], rect]` | `health`; the percentage readout; the bar |
| `NANITE` | `[rect, placed…]` | the Nano-Canister slots |
| `OBJECTIVE` | `[f, placed, rect, [r, g, b], [x, y], [x, y], rect, [a, b]]` | slide time; `objectab`; the bar; its colour; summary and description offsets; the tab's fill slot; text box |
| `STATS` | `[[x, y] …]` | score, clock, then six unread records |
| `TARGET` | `[str × 5]` | `lock_on`, threat left, right, up, down |
| `WEAPON` | 9 × `[str × 5, str × 2, x, y, x, y]` | per slot: five frames, two minis, icon and ammo position on the icon strip |
| `MODES` | 4 × `[off, on, pik, x, y]` | tank, amphib, hover, sub |
| `BRIEFING` | map | the transport screen (§2.7b) |

A *placed* record is `[texture, x, y, "TRUE"/"FALSE", "TRUE"/"FALSE"]`
(transparent, centred); a *rect* is two corners. Positions under `SENSOR`,
`SHIELD` and `OBJECTIVE` are relative to the section's frame image
(`objectab`'s to `monitor`). (V)

### 2.7b `dialog.zrd`

The menus' layouts, in pixels of a 640×480 screen. Parsed by
`bft_assets::dialog::parse_dialogs`; meanings and checks are docs/data/menus.md. `SHARED_IMAGE_PATH`, then one map per dialog (18: `MAINMENU0`–`3`,
`OPTIONSPANEL`, `NEWGAMEPANEL`, `SAVE_GAME_DIALOG`, `LOAD_GAME_DIALOG`,
`CHEAT_CODE_DIALOG`, `NETEXIT`, `MPEXIT`, `CONTROLS_DIALOG`,
`COMMANDS_DIALOG`, `CREDITSPANEL`, `CONFIRM_QUIT`, `MP_NEW_GAME`,
`WOL_NEW_GAME`, `MESSAGEBOX`):

| Key | Value | Meaning |
|---|---|---|
| `IMAGE_PATH` | `[path]` | the original authors' art directory; unused |
| `FONTS` | `[[id, face, size, [r, g, b], flag, weight] …]` | text styles by id |
| `BACKGROUND_IMAGES` | `[[bitmap, x, y] …]` | drawn in order; y may be negative |
| `BACKGROUND_TEXT` | `[[MSG_key, x, y, font] …]` | fixed text (one dialog) |
| `BACKGROUND_VIDEOS` | `[[avi, x, y, [r, g, b]] …]` | the controls screen's pumps; not read |
| `BUTTONS` | map `NAME → control` | the controls, whatever their kind |
| `PRIMITIVES` | map `NAME → {POSITION, FONT?, WORDWRAP?, BITMAP?}` | places the game fills |
| `CURSOR` | `{BITMAP [name]}` | the pointer |

A control is a map of: `POSITION [x, y]`; `BITMAP [name]` or `[name, dx,
dy]`; `ROLLOVER {BITMAP?, SOUND [snd, volume?]}`; `ACTIVATE {BITMAP?,
SOUND?}`; `DISABLE {BITMAP}`; `LABEL [text, x, y, font]` or a list of
them; and one kind key — `CHECKED {TEXT [text, x, y, font]}`, `CYCLE
[{TEXT, BITMAP?} …]`, `RADIO [control …]` (no position of its own),
`FILLBITMAP [name]`, `LISTSIZE`/`LIST_FONT`/`SELECTED_FONT`/`SPACING`/
`LIST_OFFSET`, `FONT`+`TEXTOFFSET`, or `RECT`/`SCROLL_RATE`/
`SCROLLING_TEXT [[[text, x, gap, font]] …]`. Text and sub-bitmap positions
are relative to the control's. Bitmap names are mixed case; `image.zbd`'s
are lower case. (V)

### 2.8 `enemies\*.zrd`, `explosns\*.zrd`, `vtol\*.zrd`, `regen`, `shock`, …

All 101 remaining global entries are `ANIMATION_DEFINITIONS` sources:
`ANIMATION_LIST` → repeated `ANIMATION_DEFINITION` maps with `NAME` (root
node, may be a glob), `ANIMATION_NAME`, `ANIMATION_ROOT_NAME`,
`ACTIVATION` (`ON_CALL`, `ON_STARTUP`, `WEAPON_HIT`, `COLLIDE_HIT`,
`WEAPON_OR_COLLIDE_HIT`), `EXECUTION_BY_RANGE [max]` or
`EXECUTION_BY_ZONE`, `RESET_TIME [t]` or `[t, t]`, `HEALTH`,
`ACTIVATION_PREREQUISITE` (`ANIMATION_LIST`, `OBJECT`, `PARENT`),
`SAVE_LOG`/`NETWORK_LOG`, `RESET_STATE` (event list) and repeated
`SEQUENCE_DEFINITION` (`NAME`, optional `ACTIVATION ON_CALL`, then events
in the same vocabulary as mech3ax's `EventData`, with `START_TIME
["EVENT_OFFSET"|"ANIMATION_OFFSET"|"SEQUENCE_OFFSET", t]`). The enemy
files (`drone`, `helicop`, `lockon`, `minelay`, `mortar`, `ntank`, `rumv`,
`samsite`, `scout`, `turret`, …) hold the enemies' destruction, damage
and effect sequences, **not** their stats (those are in `vehicle.zrd` and
per-mission `ai.zrd`). Every one of these sources is compiled into each
mission's `anim.zbd` through the include list in the mission's `anim.zrd`,
so **the engine never needs to parse these sources**; they are the readable
form of `anim.zbd`. (V — checked `countdown`, `deathm1`, `m1objects`,
`m1pickup` against the compiled defs.)

## 3. Per-mission archive

### 3.1 `ai.zrd` — turret and fixed-emplacement definitions

```
DESTROY_ANIM: ["destroy_turret"]
TURRET:
  tur_1**:  HEALTH [6]  PARTS ["turret","gun","firepoint"]
            WEAPON { NAME ["wep_1.0"] AMMO [100] FIRE_RATE [0.1] FIRE_LIMITS [0.8, 2.5] DETECTION_RANGE [300] }
            EFFECT ["lightning", 0.01]
```

| Key | Meaning | Conf. |
|---|---|---|
| `DESTROY_ANIM` (top) | anim played when a turret dies; per-entry override possible (`sam_**` → `destroy_samsite`) | V |
| `TURRET/<glob>` | glob over gamez node names: `tur_1**` matches `tur_101`…`tur_111`, `sam_**` → `sam_01`…, `hel_*1` → `hel_11` (the helicopter's guns, renamed by interp `NodeSetDescription`), `fcturr**` (m7) | V |
| `HEALTH` | hit points (1e9 = indestructible, used for the helicopter guns) | V |
| `PARTS` | child node names for the yaw part, pitch part and muzzle: `turret`, `gun`, `firepoint` under `<glob>/pturret.flt/healthy/` | V (subtree checked) |
| `WEAPON/NAME`, `AMMO`, `FIRE_RATE` (s between shots), `FIRE_LIMITS` (`[min, max]` seconds of sustained fire?), `DETECTION_RANGE` (m), `BASE_MOVES` (1 = whole base rotates) | | I except NAME/AMMO V |
| `LOS [0]` | ignore line of sight | I |
| `MSL_LOCK` | missile can lock onto it | I |
| `TARGETS ["vtol"]` | SAM sites shoot the VTOL, not the player | V (mission 1 story) |
| `EFFECT [name, rate]` | ambient effect (`lightning`) | I |
| `DEACTIVATE [object, child]` (m7) | turret shuts down when `powstation/healthy` goes inactive | I |

Turrets are baked into gamez at their positions; they are found by glob
over node names. (V)

### 3.2 `aiv.zrd`, `aiv_easy.zrd`, `aiv_hard.zrd` — vehicle placement

Flat map, one key per unit instance: `<type>_<NN>` (`scou_03`,
`rumv_easy_01`, `ntank_12`) plus `bft_00` for the player. Value is the
positional record `[net_index, [x, y, z], heading_deg]`:

- `net_index` selects `net_<NN>.zrd` (1-based; `0` for the player = none).
  The instance's position equals the chosen net's `node_00`. (V)
- `[x, y, z]` world position in metres; y is ground height. (V)
- `heading_deg` yaw in degrees (player 166.5 ≈ `location.zrd` "Tour A
  Campaign 1 Start" heading −193.2 + 360). (V)

**The player start is `aiv.zrd/bft_00`**, not the `bft_00` node's gamez
transform (that one is the build script's placeholder at 2417, 0, 1117
rotated −70°). (V) Instances (`scou_03`) do not exist as gamez nodes; the
type prefix names the template root node (`scou`, `rumv`, …) that gamez
holds *outside* the world tree (no parent). Spawning = clone the template
subtree at the record's position. m1 normal: 84 units (16 scouts, 30 RUMVs,
12 trenchers, 13 napalm tanks, 1 minelayer, 2 drones, 9 easy variants);
`aiv_hard` adds 3 units, `aiv_easy` removes 4. Multiplayer maps have only
`bft_00`. (V)

### 3.3 `net_NN.zrd` — AI path networks and behaviour

One file per unit group (`net_01` … `net_91` in m1, 55–92 per campaign
mission, none on multiplayer maps). Flat map:

| Key | Shape | Meaning | Conf. |
|---|---|---|---|
| `version` | 105 | format version | V |
| `name`, `type` | `net_NN`, `standard` (78) / `fixed` (11) / `defilade` (1) / `hide` (1) | `fixed` = stays on its node, `defilade`/`hide` = cover behaviour | I |
| `path_width` | 10 | corridor half-width (m) around the path | I |
| `activate_rad`, `attack_rad` | m | wake-up and open-fire radii from the player | I |
| `attack_dwell`, `not_pursuit_dwell` | s | time between attack decisions / before dropping pursuit | I |
| `pursuit_params` `[f,f]` or `pursuit_range` `[f,f]` | | either one; same role as `vehicle.zrd` `pursuit_range` (min/max standoff) | I |
| `return_range` | m | leash distance from the net | I |
| `hide_times` | `[f,f]` | min/max seconds in cover | I |
| `attack_strategy` | `follow_net` (30) / `head_on` (19) / `back_forth` (14) / `circle` (10) / `sit` (5) | movement pattern while engaging; absent on some `fixed` nets | V (vocabulary) |
| `attack_buddy`, `activate_buddy` | 0/1 | share target / wake-up with the group | I |
| `node_NN` | `[12, [x,y,z], [l0, l1, l2]]` | waypoint: flag (always 12 — U), position, up to three neighbour indices (−1 = none). Nets are small graphs (1–97 nodes, mostly short chains) | V (links checked) |

### 3.4 `net.zrd` — multiplayer spawn ring

Not a path net. A list of 8 records `[x, y, z, heading_deg]`: deathmatch
start positions. In m1 they form a ring of radius 50 around (2500, 1480) at
45° steps; multiplayer maps have 8 hand-placed ones. (V for shape, I for
meaning — consistent with `net.zrd` existing on every map including the
campaign ones, and with the 8-player limit.)

### AI files across all 13 missions

Measured across all 13 missions' `ai.zrd`, `aiv*.zrd`, `net_NN.zrd` and
`net.zrd`; asserted in `crates/bft-sim/tests/real_ai.rs`.

**`ai.zrd` (§3.1).**

- `FIRE_RATE` here is **seconds between shots throughout**, unlike
  `weapons.zrd`'s bimodal column (§2.3). Its values run
  0.06–30 with the 2–12 band populated, so there is no gap to split on; and
  a SAM site's 30 with 6–12 rounds of `wep_9.2` is a missile every half
  minute rather than 30 a second. (V by exhaustion of the range.)
- **The globs are not disjoint, and the most specific wins.** m6, m10 and
  m11 carry both `a2tur**` and `a2tur_**`; the first matches every node the
  second does. They name different models — `a2tur00` is `sontrig1.flt`
  with a `turret` part and 1e9 health, `a2tur_01`–`03` are `a2gun.flt` with
  no turret part and 30 — so a first-match-in-file-order reading would
  leave `a2tur_**` matching nothing and its authored values unreachable.
  Resolved by literal-prefix length, ties in file order. (V, subtrees
  checked.)
- `PARTS` is **0, 2, 3 or 4 names, never 1**, and is positional. The
  two-element form (`fcturr**`, `a2tur_**`, `braintur_*`) drops the *yaw*
  part, not the muzzle: its `a2gun.flt` subtree is `healthy/gun/firepoint`,
  with nothing to traverse. m6's `a3tur**` is
  `[turret, gun01, firepoint1, firepoint2]` — one yaw, one pitch, two
  muzzles. `bouy_**` is `null`. (V.)
- `PARTS` names are found by **search of the matched node's subtree, not by
  a fixed path**: `firepoint` is `<glob>/pturret.flt/healthy/turret/gun/
  firepoint`, a grandchild of the pitch part, and the depth differs per
  model. Breadth-first, so `healthy`'s parts win over the identically named
  pieces under `destroyed`. (V.)
- `LOS` is **0, 1 or 2 — not a flag**. 0 (the helicopter guns) is the only
  value that means "ignore line of sight"; 1 and 2 appear on m6/m10's
  `a2tur**` and `a3gard*` and their difference is unknown. (V for the
  values, U for 1 vs 2.)
- `HEALTH` of **0.0 occurs and reads as indestructible**, as 1e9 does
  (m6's `a3gard*`, m10/m11's `a2tur_**`). (I.)
- **Eight further keys** are present in the shipped data:
  `WEAPON/FIRE_DWELL` (0.5/1/3 s, the pause after a burst),
  `WEAPON/DAMAGE_MODIFIER` (0.2/0.25/0.5), `INTERSECT_BVOL [0]`,
  `ACTIVATE_ON_HIT [1.0]`, `FIRE_ANIM ["a2pwrup"]`, `ALWAYS_LOOK_AT [1]`,
  `DAMAGE_PART ["my_butt"]` (a developer joke; it names no node), and
  `DEACTIVATE`'s **one-element form** (m7's `["tower_00"]` against m2's
  `["powstation", "healthy"]`). (V for presence, I for meaning.)
- 130 turret entries across the 13 missions, 19 distinct globs. Every
  mission defines at least `tur_1**`, `sam_**` and the three `hel_*N`.
  **A def whose glob matches nothing is normal** — m1 defines `tur_2**`,
  `tur_3**` and `fcturr**` and has no such nodes; m10–m13 have no turret
  nodes at all. (V.)

**`net_NN.zrd` (§3.3).**

- Graphs are **1–97 nodes**. The distribution is heavily weighted
  to the short end (104 two-node nets) but m3 has one of 97. (V.)
- **12 links across 3 nets point past the end of their own graph**: m2's
  `net_23` (6 of them, 7 nodes), m3's `net_47` (5, 7) and m6's `net_62`
  (1, 3). They read as nets edited down without their neighbour lists being
  fixed. A loader must filter rather than index. m1's `net_82` is a
  one-node net with no links at all. (V.)
- **Links are not symmetric.** m1's `net_01` has `node_00 → [1]` while
  `node_01 → [2, 0]`, so "the neighbour I did not come from" is not always
  available and a walk needs a fallback. (V.)
- `type` has a **fifth value in the data**: m3's `net_63` is
  `type: "follow_net"`, a strategy name in the type field. One occurrence;
  reads as `standard`. So the counts are 431 standard (430 + the typo), 27
  fixed, 11 defilade, 1 hide, of 470 nets. (V.)
- `attack_buddy` is **0 in all 441 nets that state it**. `activate_buddy`
  is 1 on 50. `hide_times` is `[0, 0]` on 455 of 457; only two nets state a
  real pair. `path_width` is 10 on 468 of 470 (two are 5). (V.)
- The waypoint `flag` is **12 in all 3717 shipped nodes**, which is why its
  meaning is still unknown. (V.)
- `pursuit_params` (432 nets) and `pursuit_range` (38) are the two
  spellings of one key; no file has both, and 165 of the former state
  `[0, 0]`, which is "no opinion" and falls back to the kind's own
  `vehicle.zrd` `pursuit_range`. (V for the exclusivity, I for the
  fallback.)

**`aiv.zrd` (§3.2).**

- The kind is the key with the **trailing `_<digits>` stripped and nothing
  else**, so `rumv_easy_01` is a `rumv_easy` — a separate `vehicle.zrd`
  entry with its own health, not a difficulty variant of `rumv`. Every kind
  every mission places exists in `vehicle.zrd` under that rule. (V.)
- m2's `aiv_easy.zrd` states **`bft_00` twice**, with identical values. A
  loader keeping a `Vec` of pairs (§0.1) sees both; the first is the start.
  (V.)
- m1: 84/80/87 units at normal/easy/hard, so `aiv_hard` adds 3 and
  `aiv_easy` removes 4 — as §3.2 says. Every campaign mission has 55+
  enemies; every multiplayer map has exactly one record, `bft_00`. (V.)
- **A unit's start is its net's `node_00`** for a clear majority but not
  all: several units share a net and only one can sit on node 0. (V.)

### 3.5 `puppies.zrd`, `puppies_easy.zrd`, `puppies_hard.zrd` — pickup placement

List of records `[kind, amount, [x, y, z], [rx, ry, rz], respawn_s]`:
`kind` is a `pickup.zrd` key (`ERFPG_AMMO`, `NANO-CANISTER`, `NANITE100`,
`MISSILE_WEAPON` …), `amount` the rounds / health value, rotation always
zero, `respawn_s` 0 on campaign maps (no respawn) and 30/60/180 on
multiplayer maps. **Only m1–m6 carry the `_easy`/`_hard` variants**; m7–m13
ship `puppies.zrd` alone, so a per-difficulty reader has to fall back. m1: 64 pickups (identical in all three difficulties);
m2: 53/56/56. Multiplayer maps: 10–54. ("Puppies" = pick-ups.) (V)

### 3.6 `objectives.zrd`

```
READ_TIME [8]  REVIEW_DELAY [4]  REVIEW_SOUND ["snd_review"]  OBJECTIVE_SOUND ["snd_objective"]
OBJECTIVE1: ["obj1fgate", "MSG_M1_O1_SUMMARY", "MSG_M1_O1_OBJ", "MSG_M1_O1_COMP", "ACTIVE",
             "READ_SOUND", "HINT_SOUND", "HINT_TIME", ["frcoff"], ["snd_m1obj1", 24], ["snd_m1hint1"], [180]]
```

Each `OBJECTIVEn` is a positional record: briefing picture stem in
`image.zbd` (V), summary / description / completion `MSG_*` keys (V),
initial state `ACTIVE` or `INACTIVE`, then keyword/value pairs:
`READ_SOUND [snd, delay?]`, `HINT_SOUND`/`HINT_SND [snd]`,
`HINT_TIME [s]`, `HINT_ACTIVE`, `HINT_ZONE [zone_id]`, `AUTOPLAY`. The
**first list after the state** is the completion condition: `[node]` or
`[object, child]` naming gamez nodes whose active state the mission's anim
sequences flip when the objective is achieved (`frcoff` is switched on by
`force_off`; `nb_force` switched off by `north_beach_access`;
`powstation/powerbad` switched on by `health_to_destroyed`). The polarity
differs per objective, so the rule is "complete when the named node's
active state changes from what it was at mission start" — verified
over all 26 conditions in the six missions, every one of which
resolves to a real node that starts `active: true`. An objective with no
node list (`OBJECTIVE4` in m1, "go to landing site") completes by the VTOL
pickup anim (`m1_exit_trigger`, `EXECUTION_BY_RANGE` 30.25 = 5.5 m) firing
`Callback 99`. (V)

From the shipped files:

- **The record is loose.** The state word is
  *optional* — the exit objectives of m1–m5 carry neither a state nor a
  node list — and so is the `COMP` key (m3's objective 5 has only
  `SUMMARY` and `OBJ`). m6 adds `FINAL_MISSION [1]` at file level and
  `AUTOPLAY` inside two records. A parser that counts positions misreads
  four of the six missions; the one in `bft_assets::reader::mission` walks
  the record until it meets a known keyword instead. (V)
- **A valueless keyword takes an inline `null`** (`"HINT_ACTIVE", null,
  "HINT_ZONE", [...]`) rather than a trailing list, so folding that null
  into the trailing value sequence shifts every later value by one and
  robs m1's objective 2 of its completion node. (V)
- **No campaign objective completes on a pickup.** m2's objective 1 reads
  `["got_amphib"]`, a real gamez node. The engine still accepts a pickup
  instance (`pu03700`) there because `PickedUp.instance` makes it free. (V)
- **The `[object, child]` child is scoped to the object's subtree**, not
  level-wide: `fntrocket`→`healthy` means the `healthy` under
  `fntrocket`, and m4 has 186 nodes of that name. Verified for all five
  pairs. (V)

Objective order corresponds to the zmap features' `objective` field
(0-based: m1 zmap has 0…3 for `OBJECTIVE1`…`4`, −1 for the coastline). (V)

Multiplayer maps ship a byte-for-byte copy of m2's `objectives.zrd` and
`location.zrd`; ignore them there. (V)

### 3.7 `location.zrd`

Map of display name → `[heading_deg, [x, y, z]]`: developer "tour"
teleport points (`Tour A  Campaign 1 Start`, `Tour C  #1 Power Room`) and
weapon locations (`Gun ERFPG 2`). Debug aid; useful for a dev-tools
teleport menu and as a cross-check of the start heading convention. (V)

### 3.8 `movers.zrd`

Flat list of node names that move at runtime (`["vtol1", "trailr"]`) and
therefore need per-frame partition updates. Read as a hint for dynamic
colliders. (I)

### 3.9 `declient.zrd`

`VERSION 1`, `CRATER` (`DEFAULT_TEXTURE`, `DEFAULT_ANIM he_dirt.flt`,
`TEXTURE_PARMS`, `SLOPE`, `RADIUS`, `DEPTH`, `POINTS`, `TEXTURE_ANIM` map
of ground texture → crater texture), `QUICK_SAND` (texture list, geometry).
Terrain deformation parameters for weapon craters ("deform client").
Optional feature; parameters recorded for the weapons and effects. (I)

Present in all 13 missions; `RADIUS 25`,
`DEPTH 1.7`, `SLOPE 15`, `POINTS 6` everywhere, `QUICK_SAND`'s `SLOPE` 20
in m1/m3 and 15 elsewhere. `TEXTURE_ANIM` is a map of ground texture →
`[crater_texture]` or `[crater_texture, dirt_anim.flt]` (m3, m6, m13 name
anims); absent in m5/m11, `null` in m10. The dirt anims (`he_dirt.flt`,
`ash_dirt.flt`, `lav_dirt.flt`, `snow_dirt.flt`, `sulph_dirt.flt`) are
compiled anim defs named with the `.flt`, and each calls `impact_miss`.
The crater textures are 32×32 scorch tiles in each level's texture
archive (m12 has none of its own). Our readings of the scalars are in
`bft_sim::crater`'s module docs: all (I). `POINTS` is read as the rim's
side count (I); the original's craters are hexagons, a floor and six walls
(as recalled from the original).

Which ground takes a crater is not in this file or in soil (every textured
material in all 13 levels is `Default` soil bar the liquids): it is the
gamez node flag **`can_modify`**, on 14–82 `gNNN` nodes per level, which
draw with sand, dirt, grass, dirt roads, lava, snow and ice. Nearly every
`TEXTURE_ANIM` ground texture lives on those nodes; docks, buildings and
metal never do. m12 has none, so takes no craters. (V: measured over all
13 levels; m1's 67 non-`can_modify` level hits dig nothing in
`real_craters.rs`.) The data leaves some plain ground unflagged — m1's
sand road off the pier, `g943`, interleaved with the flagged sand `g32` —
and `assets/tuning/craters.ron` adds such nodes by name. A texture rule
was tried instead and rejected: it also flags m4's tunnel floors, m10's
rooftops, m3's bridges and m7's canyon rock.

### 3.10 `anim.zrd` (per mission) and `startanims.zrd`

`anim.zrd`: `ANIMATION_DEFINITIONS { GRAVITY [-9.8], ANIMATION_PATH,
ANIMATION_LIST { ANIMATION_DEFINITION_FILE [path] … } }` — the include
list the build tool compiled into `anim.zbd` (global `anim.zrd`, the enemy
sources, `start_single_mN`, `load_control`, `start_*`, `deathmN`, then the
mission props). Runtime does not need it. (V)

`startanims.zrd`: `NEW_GAME_START`, `LOAD_GAME_START ["load_control"]` —
**the anim names the engine calls when a new game or a loaded game
starts**. This is the entry point of the mission script. **Only m1's
`NEW_GAME_START` is `start_single_player`**; m2–m6 use
`mN_start_animation`, so the name has to be read rather than defaulted.
`LOAD_GAME_START` is `load_control` in all six. The entry animation calls
the intro camera, the VTOL drop, ocean sounds, `frc_motion`, etc.
Campaign missions only. (V)

### 3.11 `start_*.zrd`, `deathmN.zrd`, `countdown.zrd`, `mNobjects.zrd`, `mNpickup.zrd`, `mNstartcam.zrd`, `mNvtols.zrd`, props

Anim-definition sources (§2.8) compiled into the mission's `anim.zbd`:

- `start_single_mN` → `horizon-start_single_player`: the mission opener.
- `start_north` (m1) → `horizon-start_north`: a mid-mission script
  (`north_beach_access`, rocket range, seagulls) called by name.
- `deathmN` → `bft_00-…`: player death cinematic (camera shake,
  explosion rings, `CAMERA_STATE`).
- `countdown` → `countdown-countdown_from_9`: 9-second voice countdown on
  the empty node `countdown` (m1 rocket launch, m9 race start).
- `mNobjects`: destructible props with `HEALTH` and `WEAPON_HIT` sequences.
- `mNpickup` → `vtol1-m1pickup`, `m1_exit_trigger` (range 5.5 m, the
  mission-end trigger), `m1_exit_animation` (uses `OBJECT_MOTION_SI_SCRIPT`
  `.zan` motion scripts).
- `mNstartcam` → `camera1-m1_start_animation`: intro camera path
  (`CALLBACK` values 15, 10, 1 = handshake with the engine; `load_control`
  uses the same values in `give_player_control`).
- `mNvtols`: `destroy_vtol1/2` with `HEALTH`.

### 3.12 Multiplayer-only entries

`race.zrd` (m9): `version 0`, `cp_count 4` — number of checkpoints for
the race mode (`MSG_LAPS`, `MSG_ONE_LAP_TO_GO`, `startgate.zrd` gates,
`countdown.zrd`). m13: `teleprtr.zrd`/`bft_trans.zrd` — teleporter screens
with `EXECUTION_BY_RANGE 15.5` `ON_STARTUP` anims that translate the
player. m10: `jumps.zrd` morphing ramps. These are all anim sources; the
game mode itself (deathmatch vs race) is not declared in reader data
beyond `race.zrd`'s presence. (V for shapes, I for mode selection)

## 4. How reader entries reference gamez and anim

| From | Reference | Resolves to | Verified |
|---|---|---|---|
| `aiv.zrd` key prefix | `scou` | gamez template root node `scou` (parentless subtree) | yes |
| `aiv.zrd` value | `net_index` | `net_<NN>.zrd` in the same archive | yes |
| `ai.zrd` `TURRET/<glob>` | `tur_1**` | gamez nodes under `world1` matching the glob | yes |
| `ai.zrd` `PARTS` | `turret`, `gun`, `firepoint` | child names inside the matched turret's `healthy` subtree | yes |
| `ai.zrd`/`vehicle.zrd` `DESTROY_ANIM`, `*_anims` | `destroy_turret` | compiled anim def `anim_name` (`tur_101-destroy_turret-healthy`) | yes |
| `objectives.zrd` picture | `obj1fgate` | `image.zbd` PNG stem | yes |
| `objectives.zrd` condition | `frcoff` / `powstation,powerbad` | gamez node / object+child; flipped by anim `ObjectActiveState` events | yes |
| `objectives.zrd` texts | `MSG_M1_O1_OBJ` | `messages.dll` | yes |
| `puppies.zrd` kind | `ERFPG_AMMO` | `pickup.zrd` key → model `pu000` via the `pickup.gw` order | yes (order inferred) |
| `weapons.zrd` `FLYOUT/MODEL` etc. | `he_morta.flt` | gamez template subtree loaded by `weapons.gw` (present as a parentless node named with `.flt`) | yes |
| `weapons.zrd` `KILL_ANIMATION` | `kill_mortar` | compiled anim def in every level (from global `kill_*.zrd`) | yes |
| `*` `SOUND` | `snd_ricsizzle1` | `sounds.zrd` → `ricsizzle1.wav` in `soundsh.zbd` | yes |
| `startanims.zrd` | `start_single_player` | compiled anim def `horizon-start_single_player` | yes |

Compiled anim defs reference gamez nodes by name in `objects[]`,
`nodes[]` and in every event's `node`/`name` field; names are unique
within a level except deliberate multi-instances (`healthy` ×132, `dbase`
×85) which are always addressed through their `anim_root_name` scope. (V)

## 5. Questions about units and meanings

1. `vehicle.zrd` weapon-row columns `a`, `c`, `e`, `f` and the exact unit
   of `rates`, `turns`, `friction` — settle by driving.
2. `weapons.zrd` `FIRE_RATE` unit split between guns and launchers.
   **Answered** (docs/data/reader.md): ≥ 3.0 is shots per second,
   below it seconds between shots; nothing in the file lies between 2.0 and
   12.0. Cross-checked against the player's `interval` column.
3. Objective activation order: what activates `INACTIVE` objectives?
   **Answered** (docs/data/anims.md): nothing does. Briefing order is not
   dependency order — m2's objective 1 is `INACTIVE` with nothing before it,
   m3's objective 3 sits `INACTIVE` between two `ACTIVE` ones — and all 16
   `INACTIVE` objectives instead have a dedicated animation that flips
   exactly their node. `INACTIVE` means "not worth briefing until it
   happens"; the flip reveals and completes the objective in one. The engine
   activates only the exit objective, and only once the rest are done.
4. `net_NN` node flag value 12 and `type` `defilade`/`hide` behaviour.
5. `player.zrd` `make_hot`/`make_cold` semantics. The weapon data does
   **not** settle this: `CATCHES_FIRE` is on `wep_3.1` (napalm) alone, which is
   also `make_cold`, while `make_hot` names `wep_6.1` (the laser sabre),
   which carries the `HEAT` block. So the two keys are not "ignites" and
   "extinguishes" in the obvious order. The engine drives `Burning` from
   `CATCHES_FIRE` and `burning_anim`, which play has not confirmed.
