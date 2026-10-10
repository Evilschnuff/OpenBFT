# Node semantics

What the gamez node list holds: its JSON shape, counts, flags, zones and names, the hierarchy, and the player and enemy subtrees.

Source: the gamez node summary written by `tools/recon/summarize.py`.

**The JSON node schema on this mech3ax commit is not the `NodeRc` enum in
`api-types/src/nodes/rc.rs`.** A node is a *flat* object; the variant name
and its payload sit under `data`:

```json
{ "name": "water02", "flags": {...}, "zone_id": 3, "model_index": 412,
  "area_partition": null, "virtual_partition": null,
  "parent_indices": [...], "child_indices": [...],
  "active_bbox": "Node", "node_bbox": {...}, "index": 17,
  "data": { "Object3d": { "transform": {...}, "signs": 128, ... } } }
```

So `name`, `flags`, `zone_id`, `model_index` and `area_partition` are read
from the top level, and only the transform and variant-specific fields come
from `data`. The loader codes against this shape, not the api-types
listing.

## Counts per level

| Level | Nodes | Object3d | Lod | Empty | model_index == −1 | area_partition |
|---|---|---|---|---|---|---|
| m1 | 4199 | 4041 | 153 | — | 2029 | 468 |
| m2 | 3956 | 3638 | 313 | — | 1673 | 414 |
| m3 | 2463 | 2334 | 124 | — | 1028 | 317 |
| m4 | 3862 | 3670 | 187 | — | 1649 | 327 |
| m5 | 2942 | 2814 | 123 | — | 1278 | 272 |
| m6 | 4955 | 4549 | 114 | 287 | 2494 | 588 |
| m7 | 1070 | 1030 | 35 | — | 330 | 182 |
| m8 | 964 | 928 | 31 | — | 323 | 83 |
| m9 | 1598 | 1562 | 31 | — | 602 | 441 |
| m10 | 1275 | 1239 | 31 | — | 454 | 189 |
| m11 | 1052 | 1016 | 31 | — | 335 | 117 |
| m12 | 1255 | 1219 | 31 | — | 412 | 83 |
| m13 | 2628 | 2582 | 41 | — | 1141 | 344 |

Every level additionally has exactly one `World`, one `Camera`, one
`Display`, one `Window` and one `Light` node — the scene-setup singletons
that interp's `NewCamera`/`NewDisplay`/`NewWindow`/`LightNew` commands
create (interp.md). **`Empty` nodes occur only in m6** (287 of them), which is
why m6 is the only level to emit `EMPTY BBOX_MODEL` diagnostics (quirks.md).

**Roughly a third to a half of all nodes have `model_index == -1`**
(m1: 2029 of 4199; m6: 2494 of 4955; m7: 330 of 1070).
These are grouping/transform nodes; geometry hangs off their children.
A level importer must walk the tree rather than iterate nodes with models.

`area_partition` is set on 7–28% of nodes (m8/m12: 83; m6: 588);
`virtual_partition` is **never** set in any level (0 everywhere), and
`active_bbox` is `"Node"` for every node in every level.

## Flags

Flag frequencies for m1 (others are proportional):

```
active 4199, tree_valid 4199, id_zone_check 4193, unk25 4041,
bbox_node 3969, intersect_surface 3316, altitude_surface 3096,
bbox_model 2170, bbox_child 1852, intersect_bbox 148, clip_to 96,
can_modify 36, landmark 17
```

**`intersect_bbox` is the weapon hit test against the
node's model box.** It is set on 41–181 nodes per level, all of which also
have `intersect_surface`. The case that shows it: m1's 23 palm groups.
Their palms are `Facade` billboards with no surface, and each group's one
solid node, `g10`, is two crossed quads (8 vertices, 2 polygons) whose
`model_bbox` is 10 × 17 × 8 m. Shot against the quads, 0 of 24 rays from
around a tree hit it; against the box, 24 of 24. It is also set on
seagulls, props' `healthy` groups and enemy parts. It is read as a hit
box only (`level::HitBoxOf`), a sensor on its own layer. Driving still
meets the model's surface: whether the box blocks the vehicle in the
original is unknown. (V for the counts; I for the reading.)

`altitude_surface` and `intersect_surface` are the two that matter for
physics: they are set by interp's `SetAltitudeSurface` /
`SetIntersectSurface` (interp.md) and mark, respectively, the surfaces the vehicle
rides on and the surfaces it collides with. **They are the collision
authority — use them instead of inferring collision from names.**
`landmark` (interp's `SetLandmark`) is rare and marks navigation landmarks.

## Zone ids

`zone_id` is a small signed integer, `-1` meaning "no zone". m1:
`{3: 1324, -1: 1189, 0: 695, 2: 509, 1: 431, 32: 37, 14: 7, 4: 4, 9: 3}`.
Multiplayer maps use far fewer (m7: `{-1: 751, 0: 254, 2: 35, 1: 30}`).
Zones pair with `Execution::ByZone` in anim defs (anims.md), so they are the
trigger-region mechanism.

## Names that look like gameplay

Counted by regex over node names (m1 / m6 / m7 / m13):

| Category | Pattern | m1 | m6 | m7 | m13 |
|---|---|---|---|---|---|
| water | `water|ocean|sea` | 9 | 14 | 1 | 3 |
| enemies | `tank|turret|drone|scout|helicop|vtol|samsite|…` | 81 | 67 | 14 | 14 |
| pickups | `^pu|pup_|pickup` | 63 | 57 | 57 | 57 |
| player start | `start|spawn|bft` | 5 | 4 | 4 | 4 |
| doors/gates | `door|gate` | 48 | 35 | 5 | 5 |
| triggers/volumes | `trig|bvol|zone` | 41 | 53 | 44 | 40 |
| destructibles | `healthy|destroyed|piece|part|collide` | 789 | 1139 | 198 | 562 |

The destructible convention is consistent and important for the weapons: a
destructible object is a group node with `healthy*` and `destroyed*`
children plus `piece*`/`part*` debris and a `collide*` hull. Every level has
about 40 `pu*` nodes (pickup spawn points) and 40 `bvol*` nodes (bounding
volumes / triggers). Multiplayer maps carry the same ~57 pickup nodes,
consistent with a shared multiplayer item set. **Doors/gates** are named
nodes (`rdoorA`, `cargodoor`, `maingate`, `m1gate16`, `doorleft/right`)
driven by anim sequences.

Water naming is stable across levels: `waterupNN` (surface plane),
`underwaterNN` (the volume below it), `waterNN`, plus `water_miss` in every
single level and occasional `clwaterN`/`oceanN`/`wateraz`. See water.md.

## Hierarchy, lights and transforms

Every figure below is measured over all 13 levels and asserted in
`crates/bft-assets/tests/real_levels.rs`.

- **Node ids are array positions; the emitted `index` field is `0` on every
  node of every level.** `parent_indices`, `child_indices` and the partition
  grid all index the node array. There is no id field to read.
- **`parent_indices` is the hierarchy authority, not `child_indices`.** The two
  never contradict each other (0 cases in any level) but `child_indices`
  **omits** 83–874 edges per level. Building m1's tree from `child_indices`
  reaches 261 of 4199 nodes from the World; from `parent_indices`, 3214.

  | Level | nodes | omissions | World tree via children | via parents |
  |---|---|---|---|---|
  | m1 | 4199 | 468 | 261 | 3214 |
  | m6 | 4955 | 874 | — | 3374 |
  | m9 | 1598 | 441 | — | 947 |

  The omitted nodes are exactly the ones the World's **partition grid**
  references (m1 468 = 468, m9 441 = 441, set-equal), so the original runtime
  walks children *plus* partitions. m6 additionally omits its 287 `Empty`
  nodes, which are parented elsewhere.
- **A level has 174–220 root trees, not one.** m1 174, m6 220, m7–m12 177 each,
  m4 207. The World is one of them and reaches 33–80% of nodes; the rest are the
  scene-setup singletons and the pickup, enemy and effect *templates*
  (`pu000`–`pu009`, `scou_easy`, `ntank`, `redsprks.flt`, `md_impact.flt`, …),
  25–49 nodes each.
- **`area_partition` is a grid index pair, not a coordinate.** `partitions` is
  emitted `[z_row][x_col]` (m1: 17×16) and `area_partition {x, z}` indexes it as
  `partitions[z][x]` — 468/468 in m1, 588/588 in m6, 441/441 in m9, zero misses.
  Cell `min`/`max` are full `Vec3`s whose **`y` carries a height band**, varying
  in 52 of m1's 272 cells; `values` is empty everywhere.
- **The `Light` node is a directional sun, not a point light.** One per level,
  always named `sunlight`, with `directed_source` set and `directional` clear.
  Its `color` is **(0,0,0) in every level**, so a renderer must substitute
  white; what varies is `orientation` (Euler radians) and the
  `diffuse`/`ambient` split, which **sums to exactly 1.0 in all 13**:

  | | m1 | m2 | m3 | m4 | m5 | m6 | m13 |
  |---|---|---|---|---|---|---|---|
  | diffuse | .55 | .35 | .40 | .50 | .20 | .40 | .25 |
  | ambient | .45 | .65 | .60 | .50 | .80 | .60 | .75 |

- **Model-embedded lights are weapon and effect templates, not level
  lighting.** Per carrier in m1, all 18
  are effect prototypes — `frctwirl` 94, `freon_fly` 55, `frcleft`/`frcright` 15
  each (the freon weapon), `plights` 20/20/30 (`md_impact.flt`, `lg_impact.flt`,
  `sm_impact.flt`), `sparky` 12×4 (`spark_red/blue/orange/green.flt`),
  `piece1`–`4` (`redsprks.flt` debris), `mine_red`, `rmine_blue`, `lh_flare` —
  and **14 of the 18 hang off `*.flt` template roots outside the World tree**.
  They are also **not instanced**: 18 models with lights, referenced by exactly
  18 nodes. So level lighting is the `Light` node after all, and these belong to
  the weapons and the anim VM.
- **`data` is a flat string for `Empty`.** m6's 287 `Empty` nodes emit
  `"data": "Empty"`, not `{"Empty": {...}}`; all are childless with
  `model_index: -1` and exactly one parent.
- **The three transform forms, and the `Matrix` reading.** `"Initial"` (a
  bare string, 2732 of m1's 4041), `{"Matrix": {r00…r32}}` (1307) and
  `{"RotateTranslateScale": …}` (2). There is no scale-only or
  translation-only form: `Matrix` is a 3×4 affine carrying rotation *and*
  translation (`r30`–`r32`). All 1307 of m1's 3×3 blocks are **proper
  rotations** — determinant +1 to f32 precision, unit rows, no scale or
  shear — and 1190 of them are the identity (translation only). The matrix
  is Direct3D **row-vector**, `v' = v * M + t`. Row `i` is the image of
  local axis `i`, so in `M * v` terms the stored rows are the columns. The
  zmap comparison in coordinates.md checks positions only, which cannot tell
  a matrix from its transpose. Two checks settle it. mech3ax's
  `object_matrix` builds it "y, x, z and then transposed" and scales per
  row. And every parent's `child_bbox` (65–68 non-trivial cases over the 13
  levels, by filter) equals the union of its children's boxes only under the
  row-vector reading (22 of 68 under `M * v`). Read as `M * v`, every node
  that is not at 0° or 180° draws with the inverse rotation: gates, chain
  links and cranes turn the wrong way.
- **`fog_altitude` is inverted**: `min > max` in every level (m1 48/24, m3
  8/−12), so it reads as "clear at `min`, full at `max`". `fog_type` is `Linear`
  and `fog_density` `0.0` in all 13. The World's `area` is **not anchored at the
  origin** — m6 is −512/−512/4608/5120, m9 −768/−512, m13 2560/2560→4352/5120.
  World and `Display` colours are **0–1**, unlike mech3ax's 0–255 material
  colours.
- **`Lod` has no `level` field**; mech3ax's `field00` is the 0/1 value, splitting
  604/641 across the 13 levels. `field68` is 1 on all 1245 LOD nodes. `range` is
  a clean distance band (0–256 ×173, 256–512 ×154, 0–25 ×102, …), with
  `512–10000000` ×36 meaning "no far limit".
- **Node `flags` is an object of 17 booleans**, and unlike model flags the false
  ones *are* emitted. `active` is set on every node of every level, so nothing
  is hidden by flag. Applying the collision rule under "Flags"
  (`altitude_surface || intersect_surface`) gives 1248 colliders in m1, 1758 in
  m2, 1927 in m6, 1002 in m13.
- **Two isolated defects, both non-fatal.** m6's node 4787 (`db`, an `Empty`)
  has `parent_indices: [4955]`, out of range for a 4955-node array. m9's node 37
  has **eight parents** (`sgate1`–`sgate8`, the race gates) — the only
  multi-parent node in the game. An importer must tolerate both.

Confidence: verified.

## The player start

Asserted in `crates/bft-game/tests/real_viewer.rs`.

- **The `bft_00` node is never the player start**, in any level
  (READER-SCHEMA §3.2). The *node* carries a
  transform in m1 only — a `RotateTranslateScale` at (2417, 0, 1117), the
  build script's placeholder — and `transform: "Initial"` in the other
  twelve, i.e. an identity at the origin. coordinates.md's placement figure describes
  that placeholder, not the start.

  The real start is **`aiv.zrd`'s `bft_00` record**, present in all 13 levels
  as `[net_index, [x, y, z], heading_deg]`, and it carries a heading the node
  never had:

  | Level | start | heading |
  |---|---|---|
  | m1 | 2222.1, 3.9, 1294.2 | 166.5 |
  | m2 | 1715.3, 39.6, 656.3 | -90 |
  | m3 | 3168, -28, 3995 | -26 |
  | m4 | 2276, 5.7, 2965 | 90 |
  | m5 | 1463, 24, 1940 | 90 |
  | m6 | 1253.1, 32, 163.1 | 180 |
  | m7 | 1560, -4, 614 | 0 |
  | m8 | 298, 24, 334 | -90 |
  | m9 | 529, 16.5, 1598 | 0 |
  | m10 | 917, 8, 528 | 253 |
  | m11 | 1474, 0, 1360 | 120 |
  | m12 | 1081, 12, 933 | -90 |
  | m13 | 3456, 0, 4709 | 0 |

  m1's differs from the node by ~240 units: the node is over water, the
  reader value is on the pier by the hangar. The campaign missions list
  75–144 units in `aiv.zrd`; the multiplayer maps hold only `bft_00`.
- **`node_bbox` is node-local, not world space.** m11's model-bearing nodes
  span -1749..1749 in x and z, centred on their own origin, for a level whose
  world area is 0–2560. Combined with the point above — eleven levels leave
  every node at the identity with the geometry in the mesh vertices — neither
  node boxes nor node transforms locate a level in the world.
- **The partition grid is what does.** Each cell is a 256×256 world column
  carrying `y_min`/`y_max`, so unioning the *occupied* cells gives a level's
  extent in all three axes: m1 0–4096 × -72..162 × 0–4352, m11 0–2560 ×
  -58..20 × 0–2560, m13 2560–4352 × -197..162 × 2560–5120. The grid spans the
  whole declared area whether or not a cell holds anything, so empty cells
  have to be skipped.
- **The world node's `area` overstates the content.** m1 declares
  0–4096 × 0–4352 for terrain that occupies x 1287–2769, z 1201–3424 (coordinates.md).
  Treating the declaration as the level's extent puts a camera framed on it
  875 units from the middle, beyond m1's own 650-unit fog plane.
- **Every level's sky is a node named `horizon` at the world origin.** Always
  `transform: "Initial"`, always `lighting: false, fog: false` on its model
  (which is how the data says "this is the sky"), and always a ring
  straddling its own origin: ±1749 (m5, m11) to ±3264 (m3, m8). m2, m6 and
  m10 model it as a *group* of 2–4 children rather than one mesh.

  The geometry is three rings — bottom, a horizon band, top — e.g. m1
  -220 / 20 / 757.7 against terrain spanning -72..162. The band sits at or
  just above ground level, so the dome is built to be seen from inside, at
  tank height.

  **It has to follow the camera in x and z.** Left at the origin it clips
  straight through the playable area, because no level's content is centred
  there: m1's dome reaches 2560 while its terrain sits around (2200, 1700).
  Height must *not* follow, or the horizon line stays at eye level however
  high the player climbs. `startanims.zrd` naming the compiled def
  `horizon-start_single_player` (READER-SCHEMA §4) is further confirmation
  the engine knows this node by name.

## Enemy templates

Asserted in `crates/bft-sim/tests/real_ai.rs` and measured by running m1.

- **A level carries only the prototypes its own `aiv.zrd` places.** m1 has
  `scou`, `scou_easy`, `rumv`, `rumv_easy`, `ntank`, `tren`, `tren_easy`,
  `dron`, `minelay`, `lock` and `bft` as parentless roots and none of the
  other twelve enemy kinds; m6, which places nearly all of them, has 20.
  m7–m13 have none, which is consistent with their having no `aiv` enemies.
  So a template that is absent is not a fault in the loader — but a
  placement naming one that is absent would be, and none does. (V.)
- `tren_easy` has a template in m1 and **no placements anywhere**, in any
  difficulty. A leftover. (V.)
- **An enemy prototype's subtree is not shaped like an emplacement's.** It
  is `<kind>/healthy/` holding LOD groups (`l1`–`l3`), ten to twelve
  `collideNN` proxies, `support00`–`03`, a `shadow`, a `target` node, and
  the turret described under "Enemy turrets" below. No `ai.zrd` `PARTS`
  entry names its parts. (V.)
- **Exactly one root name repeats, on every level: `rfpg_mzl.flt`.** Two
  roots, same one-child shape (`rfpg_mzl.flt/rfpgmuzzleburst`), *different*
  model indices — 790 and 696 in m1 — so they are two variants of the pulse
  gun's muzzle flash rather than a duplicate, presumably the weak and strong
  gun's. Every other root name is unique, so a name→root map is always
  exactly one shorter than the root list. `FIRE/ANIMATION` needs both. (V on all 13 levels.)
- The `collideNN` children are the original's own collision proxies.
  The AI does **not** clone them: `vehicle.zrd`'s `collision` ring
  already gives the hull one box, and spawning both would put a trimesh
  inside a cuboid on the same body. (Design note, not a data finding.)
- **Cloning works and the placements are sound.** m1's 83 enemies all get a
  model, none settles more than 1.8 m from its `aiv.zrd` record, and none
  falls through the terrain. Reported at `info!` by
  `bft_game::enemies::report_enemy_placement`. (V.)

Confidence: verified (counts and subtree shapes).

## The player subtree

Measured over all 13 levels' cached gamez by dumping the subtree's names,
parents, transforms and model indices; asserted in
`crates/bft-assets/tests/real_player_model.rs`, including where the turret
and its firepoint are.

- **`bft_00` is not a template root. It is a child of the World node**, in
  all 13 levels (parent id 0, which is `world_node` everywhere). Only m1's
  `bft` twin is parentless, and it is the only `bft*` root in the game.

  Two consequences:

  1. `LevelTemplates::from_level` walks `LevelData::root_nodes`, so it never
     contains `bft_00` and `clone_template` cannot reach it. The player's
     model is **re-parented onto the hull, not cloned**.
  2. Being in the World subtree, it is spawned as scenery with the level:
     unless the player's model takes it, every level has a BFT standing at
     the placeholder position, visible but easy to miss because the levels
     are large and the placeholder is off the player's path.
- **Nearly every node of the subtree collides**: 48 of m1's 49 and 71 of
  m5's 72 have `flags.collides()`, so the level spawner gives each one a
  `RigidBody::Static` and a trimesh collider. Re-parenting those under the
  dynamic hull makes the model lie flat, float, then tip and rotate — a
  static body does not follow its parent, and avian resolves it against the
  hull box. `bft_game::player_model` removes `RigidBody`, `Collider`,
  `ColliderDensity` and `LevelStatic` from all 49–72 nodes as it re-parents.
  (V, by running m1.)

- **reader.md's node counts are exactly right**: 49 in m1, 57 in m2, 65 in m3, 63
  in m4, 72 in m5–m13. m1 alone carries a second 49-node copy named `bft`,
  parentless, and the two are the same size. `bft_00` is the one to use: it
  is the name `playerName` binds (interp.md) and the only one every level has.
  (V.)
- **`turret` and `gun` are two separate rotation joints, not one.** Both are
  transform-only nodes (`model_index: None`) whose transform is a pure
  translation with an identity rotation: `turret` at `(0, 0.3, −1.75)` under
  `chassis`, and `gun` at `(0, 1.0, 0.875)` under `turret`. A node with no
  mesh, a translation-only transform and mesh-bearing children is a joint,
  and the original put the barrel's pitch on the inner one.

  **So one combined rotation is wrong for the real model.** Composing yaw
  and pitch into a single `Quat::from_euler(YXZ, yaw, pitch, 0)` on one
  `TurretPart` entity pitches the whole turret ring rather than the barrel.
  `aim_turret` splits it: yaw on the `turret` node, pitch on `gun`, with the single-entity path kept for
  vehicles that have no model. (V on all 13.)
- **The firepoints are `fpnt_l`, `fpnt_c` and `fpnt_r`**, transform-only
  children of `gun` at `(∓1.5102894, 0, −3.0)` and `(0, −0.07, −3.05)`.
  Identical in all 13 levels. `fpnt_c` is the muzzle; `play.rs`'s
  `Muzzle(0, aim_y, −3.5)` was an approximation and this replaces it. The
  naming differs from the turret convention in `ai.zrd`
  (`firepoint`, `firepoint1`), so no shared lookup is possible. (V.)
- **The subtree carries every form's geometry at once, selected by node.**
  From m5 on (72 nodes), `cpolys` holds `tank_chassis` and `sub_chassis`,
  `tpolys` holds `t_turret` and `s_turret`, `wings` holds `wings_track`,
  `wings_hover` and `wings_sub`, `shadow` holds `track_shadow`,
  `hover_shadow` and `amphib_shadow`, and each morph group holds a track and
  a pontoon (`rtracks`/`rpont` under `rtrack_morph`/`rpont_morph`). The
  early levels have only the tank set, which is why the tree grows.

  So a form change is a **visibility swap over named nodes**, not a model
  swap — and the `*_morph` groups' names say the original tweened between
  them. The engine shows the track set and leaves the switching to the anim
  VM, which is what `t2a_anims` and friends drive. (V.)
- **The weapon models under `gun` are one node per weapon**, named for the
  weapon (`HEM`, `MDM`, `P-HEM`, `NAPALM`, `FREON`, `LASER`, `ARC`,
  `sonic`, `LO-EM`, and `RFPG_{L,R}`/`ERFPG_{L,R}` for the weak and strong
  pulse guns), several with a `*SCROLL`/`*scroll` or beam child that
  the interp texture scroll drives. This is the growth reader.md measured: m1 has
  no `LO-EM`, m2–m4 add weapons, m5 has the full set. Showing the selected
  weapon and hiding the rest goes with the loadout. (V.)
- **`bft_00`'s own transform is the build-script placeholder**, and only in
  m1 is it non-trivial: the `rotate.y = −1.2217305` rad = −70° at
  `(2417, 0, 1117)` that coordinates.md names as the one non-trivial Euler in the level.
  The other twelve are identity at the origin. The player model overwrites
  it with the identity when it re-parents, so the hull's placement is the model's. (V.)
- **The `collision` ring is a hand-authored outline, not a bounding box.**
  Each of the player's four modes states twelve points at `y = 1`: six at the
  front and back corners (`z = ±3.34`) and six bulging out at the waist. For
  `track` the waist is `x = ±2.83` against `±2.70` at the corners; `hover`
  goes to `±3.75`, `amphib` to `±2.40`, `sub` to `±2.20`. So the footprint is
  a chamfered hull, widest in the middle, and **the four forms differ by more
  than a metre across**.

  Taking min/max over the points — which is what `hull_box` does, and what
  a box collider would do — extends the widest point to every
  corner and makes the vehicle fatter at its ends than the original drew it.
  `hull_collider` extrudes the real outline into a convex hull instead.
  `hull_box` is kept for the debug overlay and for enemies.

  The height is still ours: every point of every ring is at `y = 1`, so the
  ring is flat and states the footprint only. `HULL_HEIGHT` stays an
  approximation from `bft_00`'s node bbox (y 0…2.825).

  Only 5 of 28 mode blocks carry a ring — the player's four and `stealth`'s
  one — so every enemy still gets the box, which is all its data supports.
  Asserted in `crates/bft-sim/tests/real_vehicle.rs`. (V.)
- **The aim joints are not direct children of the hull.** `turret` is four
  levels down (`bft_00/body/chassis/turret`), so a scan of the hull's
  *direct* children finds the stand-in and nothing on a real model: the gun
  does not move at all. The joints are recorded on an `AimJoints` component when the vehicle is
  built instead. (V, by playing m1.)
- **`target` and `new_skin`/`regenerate.flt`/`skin` are not geometry to
  draw as-is.** `target` is a transform-only node at `(0, 2, −0.5)` — the
  point enemies should aim at instead of the hull origin. `skin` is the
  regeneration shield effect, a mesh that should be hidden until a pickup
  triggers it. The player model hides `new_skin`; enemy aim does not use
  `target` yet. (V.)

## Enemy turrets

- **Every enemy prototype with a turret calls it `turret`, with the barrel
  `gun` as its child**, both transform nodes at identity rotation — the
  same two-joint shape as `bft_00`. So one lookup serves all of them; only
  the RUMV's *second* gunner, `turret_1/gun_1`, is named differently, and
  nothing aims it. The
  muzzle is `firepoint` on the tanks, `fpnt_c` on the scout, and only the
  pair `fpnt_l`/`fpnt_r` on the drone and hovertank. The minelayer's
  `turret` has no `gun` and no firepoint; `lock`, `amphi` and `subt` have
  no `turret` at all and carry their firepoints on the chassis. (V, m6's
  20 prototypes; `real_ai.rs`.)

## REQUIREMENTS §9, question 6

Node flags / zone ids: **answered** above and in water.md. `altitude_surface` /
`intersect_surface` are the collision authority; `zone_id` pairs with
anim `ByZone` execution and `objectives.zrd` `HINT_ZONE`; water zones
are level-specific numbers (14/32 in m1).

Confidence: verified.
