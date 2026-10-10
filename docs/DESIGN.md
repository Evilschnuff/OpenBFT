# open-bft — Design

Derived from [REQUIREMENTS.md](REQUIREMENTS.md) and checked against the
real data (see [data/README.md](data/README.md) and
[READER-SCHEMA.md](READER-SCHEMA.md)); the data-dependent choices are
decided there.

---

## 1. Design principles

1. **Data-driven from the original files.** The engine has no knowledge of
   specific levels, enemies or weapons baked in. Everything comes from the
   extracted cache. **The reader archives (`zrdr.zbd`) are the game's own
   database** — vehicle physics, weapon ballistics, enemy placement, AI
   parameters, pickups and objectives are all read from them. The RON
   files in `assets/tuning/` are an **override layer** applied on top, not
   the primary source; they exist to correct values the original stores in
   units we have not pinned down and to hold the constants our controller
   needs that the original has no equivalent for. The manual (`MANUAL.PDF`,
   `Recoil_ref.pdf`) is a legitimate source for what the data leaves
   unnamed: weapon names, vehicle forms, mission briefs.
2. **Simulation is separate from presentation.** `bft-sim` runs at a fixed
   60 Hz, takes `PlayerInput` values, and never touches rendering, audio, or
   windows. `bft-game` draws whatever `bft-sim` produced. This is what makes
   headless servers, deterministic replays and fast unit tests possible, and
   it is cheap only if done from the start.
3. **One crate owns the mech3ax format.** `bft-assets` is the only crate that
   deserialises mech3ax output. It converts to engine-owned types. Format
   churn is contained.
4. **Bevy idioms, not a custom engine layer.** States, schedules, system sets,
   assets, observers and events are used as Bevy intends. No `Layer` trait, no
   scene-graph wrapper, no manager singletons where a resource or component
   will do.
5. **Unknowns are recorded, not guessed silently.** Anything learned from the
   data goes in `docs/data/`. Anything approximated goes in a tuning
   file with a comment saying it is an approximation.

## 2. System context

```
 User's Recoil install (data root)          open-bft repo
 ┌──────────────────────────────┐            ┌───────────────────┐
 │ zbd/…/gamez.zbd, anim.zbd    │            │ assets/*.ron      │  our own tuning data
 │ zbd/interp.zbd, zrdr.zbd     │            └───────────────────┘
 │ zbd/*tex*.zbd, image.zbd     │
 │ zbd/sounds{l,m,h}.zbd        │
 │ maps/m*.zmap, strings.dll    │
 └───────────────┬──────────────┘
                 │ unzbd rc <cmd>  (mech3ax, pinned commit, separate process)
                 ▼
 Extracted cache  (§5.1)
 ┌──────────────────────────────┐
 │ manifest.json (mech3ax ver.) │
 │ gamez/*.gamez.zip (5 JSON)   │
 │ textures/*.tex.zip (PNG)     │
 │ anim/*.anim.zip              │
 │ reader/*.rdr.zip             │
 │ sounds/*.snd.zip             │
 │ interp.json, messages.json   │
 │ zmap/*.zmap.json             │
 └───────────────┬──────────────┘
                 │ Bevy AssetServer + custom AssetLoaders
                 ▼
 bft-assets ──▶ bft-sim ──▶ bft-game ──▶ bft-bin
 (formats)     (rules)     (drawing)    (App wiring, CLI)
```

## 3. Crate architecture

| Crate | Role | Depends on | Bevy features |
|---|---|---|---|
| `bft-assets` | Data root discovery, extraction driver, mech3ax JSON/PNG → engine asset types (`LevelData`, `TextureSet`, `ReaderData`, `AnimSet`, `ZMap`, `SoundBank`, `Messages`). | bevy (asset, no render) | `bevy_asset` |
| `bft-sim` | Fixed-timestep simulation: physics (avian3d), vehicle, weapons, damage, AI, mission state, anim event VM, multiplayer replication of sim state. | bft-assets, avian3d, bevy_replicon | none render-related |
| `bft-game` | Presentation: level spawning into meshes/materials, camera, HUD, menus, audio, debug overlays, client networking glue. | bft-sim, bft-assets | full DefaultPlugins |
| `bft-bin` | `open-bft` binary. Subcommands: `play` (default), `view <level>`, `server`, `extract`, `inspect`. | all | — |

Dependency direction is strictly downward in that table. `bft-sim` must
compile with `bevy` default features off plus `bevy_asset`, `bevy_transform`,
`bevy_hierarchy`, `bevy_state`, `bevy_time`; CI builds it that way to enforce
NFR-4.

`bft-sim` holds the gameplay; `bft-game` is presentation only.

### 3.1 Engine-owned data types

`bft-assets` produces engine types, not mech3ax types. The mapping:

| mech3ax (api-types) | Engine type (bft-assets) | Notes |
|---|---|---|
| `GameZ { textures, materials, models, nodes }` | `LevelData` | One per `gamez.zbd`. Nodes are `Vec<NodeRc>`-shaped but converted into an engine enum with resolved child/parent indices. |
| `Texture { name, mip_index }` | `TextureRef` | Name is the key into a `TextureSet`. |
| `Material::Textured/Colored`, `Soil`, `CycleData` | `MaterialDesc` | `Soil` copied verbatim as engine `Soil` (14 variants). |
| `Model { vertices, normals, polygons, lights, flags, model_type, facade_mode, texture_scroll }` | `ModelDesc` | Model and polygon `flags` are emitted as objects of booleans with the false members left out, so every flag field defaults to false. `normals` is its own table, reached only through `normal_indices`, not parallel to `vertices`. Polygons are kept as index lists; triangulation is done in `bft-game` mesh building, not in the loader, so `bft-sim` can build colliders from the same data. |
| `NodeRc::{World, Object3d, Lod, Light, Empty, Camera, Display, Window}` | `NodeDesc` enum | The emitted JSON is a **flat** node object with the variant under `data`, not the tagged enum `api-types` documents; mirror the JSON. Camera/Display/Window are loaded but only logged. Stored `Lod`/`Empty` bounding boxes disagree with the geometry in every level but m1; bounds are recomputed, not trusted. |
| `Zmap { min, max, features[{color, vertices, objective}] }` | `ZMap` | Used directly by the automap. |
| `Script { name, datetime, lines }` (interp) | `InterpScript` + `RuntimeBindings` | Lines kept as strings. No interpreter: `bft-assets::interp::runtime_bindings` extracts the runtime subset (texture scroll/cycle bindings, tether altitude, well-known node names) from the `mN_zbd.gs` path (docs/data/interp.md). |
| reader `serde_json::Value` tree | `ReaderValue` (Int/Float/Str/List/Null) | Recursive enum. A file is a list alternating `"KEY", value`; an empty value is JSON `null`; some values are positional records. Accessors: `get`, `get_all` (keys repeat), `pairs` (ordered), `nth` (positional). Typed views per file in `bft-assets::reader::{vehicle, player, weapons, pickup, sounds, ai, puppies, mission, briefing}`. |
| `AnimDef`, `SeqDef`, `Event`, `EventData` | `AnimDefDesc` etc. | Large. Loaded as-is into a mirrored enum; only the variants Recoil uses (docs/data/anims.md) get behaviour. |
| `TextureManifest` + PNGs | `TextureSet { images: HashMap<String, Handle<Image>>, info: HashMap<String, TextureInfo> }` | Alpha mode from `TextureAlpha`. |
| sound ZIP of WAV | `SoundBank { HashMap<String, Handle<AudioSource>> }` | |
| `messages.json` | `Messages { entries: HashMap<String, String> }` | |

Mirroring rule: the serde structs in `bft-assets::mech3ax` are written by hand
from mech3ax's `api-types` and use `#[serde(default)]` liberally and never
`deny_unknown_fields`. Fields the engine ignores are omitted. `unk*` fields
are never read; the one `ptr` read is the anim defs' node binding
(docs/data/anims.md). When mech3ax's output changes, only this module and
its tests change.

Alternative considered: depending on `mech3ax-api-types` directly via git. It
would remove the mirror but tie our Rust version and Bevy build to their
workspace and their unstable API. Revisit when mech3ax tags a stable release.

## 4. Runtime architecture

### 4.1 App states

```
AppState: Boot → FrontEnd → Loading(LevelId) → InGame → (Paused overlay)
                            └── Viewer(LevelId)   (dev tool, no sim)
GameMode (sub-state of InGame): Campaign | Deathmatch | Race | Teleport
NetRole (resource): Local | Host | Client | DedicatedServer
```

- `Boot`: find data root, load or verify extracted cache, load global assets
  (messages, global reader, interp, sound table). Shows progress.
  The global reader is what `VehicleTable`, `WeaponTable`, `PickupTable` and
  `SoundTable` are built from, so it is a hard dependency of `InGame`.
- `FrontEnd`: menus. Sim not running.
- `Loading`: load `LevelData`, `TextureSet`, `AnimSet`, `ZMap` **and the
  per-mission `ReaderData`** for the level; build the level's AI, pickup and
  mission tables from it; spawn the level (sim entities + presentation
  entities); then transition. `GameMode` follows the level: m1–m6 are
  `Campaign`, m9 is `Race`, m13 is `Teleport`, the rest `Deathmatch`.
- `InGame`: sim runs in `FixedUpdate`; presentation in `Update`/`PostUpdate`.
- `Paused`: a computed/overlay state that stops `FixedUpdate` sim sets
  locally; not available in multiplayer clients.

### 4.2 Schedules and system sets

```
FixedUpdate (60 Hz, bft-sim):
  SimSet::Input      collect PlayerInput per controlled entity (local or from net)
  SimSet::Vehicle    vehicle controller writes forces/velocities
  SimSet::Weapons    firing, projectile spawn, ammo
  SimSet::Physics    avian3d step (its own schedule, ordered here)
  SimSet::Damage     hit resolution, health, destruction, pickups
  SimSet::Ai         enemy decision + steering (may run every Nth tick)
  SimSet::Anim       anim event VM: prerequisites, sequences, node transforms
  SimSet::Mission    objective state, triggers, win/lose
  SimSet::Replicate  (host/server) snapshot marking for replicon

Update (frame rate, bft-game):
  raw input → PlayerInput staging   (before FixedUpdate via PreUpdate)
  camera follow (interpolated), HUD, audio triggers, effects, debug overlays
```

avian3d runs inside `FixedUpdate` with interpolation enabled for rendered
transforms (`bevy_transform_interpolation`, which avian pulls in). All
gameplay-relevant queries against physics (raycasts for aim, ground probes)
happen in `FixedUpdate`.

### 4.3 Entities

Simulation entities carry sim components only; presentation attaches render
components to the *same* entity in `bft-game` on spawn (via observers on
`Added<Vehicle>`, `Added<Projectile>`, etc.). This keeps one entity per
game object and avoids a parallel "view" hierarchy. On a dedicated server
`bft-game` is simply not loaded, so no render components exist.

Core sim components:

```
// identity & placement
NodeId(u32)              index into LevelData.nodes for level-spawned entities
LevelStatic              immovable geometry (collider only)
Health { current, max }  vehicles and turrets only; scenery health lives in anim defs
Team(u8)

// vehicles
Vehicle { kind: VehicleKindId, form: VehicleForm, unlocked: FormSet }
VehicleForm::{Track, Hover, Amphib, Sub}    // = vehicle.zrd mode keys
Turret { yaw, pitch }
PlayerInput { throttle, steer, turret_delta, pitch, fire, select_slot, .. }
Controlled(ControllerId)  Local | Net(ClientId) | Ai

// weapons
WeaponLoadout { slots: [SlotState; 9], selected: u8, strong: bool }   // weak+strong per slot
Projectile { weapon, owner, team, age, target, armed }
Pickup { kind, amount, respawn, instance }   // instance = "pu03700"

// ai
AiBrain { net, state, target, waypoint, strategy, .. }   // net_NN.zrd group + waypoint graph

// mission
Objective { index, state }         (on a MissionState resource, not entities)
AnimInstance { def, armed, running, damage, .. }   (one per AnimDef in the level)
```

### 4.4 Level spawn

`bft-sim::level::spawn_level(LevelData)` walks the node tree:

- `World` → a `LevelRoot` entity with fog/area info as a resource
  (`WorldEnv`); its partitions become a spatial index resource for AI and
  culling hints.
- `Object3d` with `model_index >= 0` → entity with `Transform` from
  `Transformation`, `NodeId`, `ModelRef(model_index)`, and a static trimesh
  collider unless flags say otherwise. Parent/children build the Bevy
  hierarchy.
- `Lod` → entity with `LodSwitch { range }` and children.
- `Light` → `LevelLight` component; presentation turns it into a Bevy light.
- `Empty` → entity with transform only (anchor for anim events).

Transforms and winding are taken as stored. The shipped data is already
right-handed with counter-clockwise outward faces, so
`AxisConvention::GAMEZ_DEFAULT` is the identity: import neither negates an
axis nor reverses winding.

`bft-game` observes `Added<ModelRef>` and attaches `Mesh3d` +
`MeshMaterial3d` built by the mesh builder (§6).

**Template subtrees.** A level's node list contains parentless subtrees
that are *not* part of the world: enemy vehicle bodies (`scou`, `rumv`, …),
the 40 pickup models (`pu000`–`pu039`) and the projectile/effect models
(`*.flt`). The build-time interp scripts loaded them so the shipped
`gamez.zbd` would carry them; the game clones them at runtime. `spawn_level`
must keep them out of the world and expose them as `LevelTemplates` for
`clone_template`.

**Initial visibility comes from anim.** A node's `active` flag at
mission start is whatever each anim def's `reset_state` event list says
(`healthy` on, `destroyed` off, force fields on). `spawn_level` therefore
does not decide visibility; `SimSet::Anim` applies every def's `reset_state`
before the first frame.

**Level semantics are resolved** (docs/data/reader.md): enemies and the
player start come from the per-mission `aiv.zrd`, turrets from `ai.zrd`
node-name globs, pickups from `puppies.zrd`, water from polygon `Soil`.
None of them is a node-name guess, so the `LevelSemantics` pass reads the
level's `ReaderData` rather than scanning node names.

## 5. Data pipeline

### 5.1 Data root and cache (`bft-assets::root`)

```
DataRoot { path }        found per FR-D1; validated per FR-D2
CacheDir { path }        default: <platform cache dir>/<hash of root path>
CacheManifest { mech3ax_version, mech3ax_commit, created, entries: [{source, output, source_mtime, source_size}] }
```

Extraction (`bft-assets::extract`) shells out to `unzbd` with the command
table from mech3ax's README (`rc sounds|interp|reader|messages|textures|gamez|anim|zmap`),
skipping entries whose source size+mtime match the manifest. Output layout
mirrors the source tree under the cache so per-mission `gamez.zbd` files do not
collide. Only the hardware texture archives (`rtexture2`, `rtexture4`) are
extracted; the software renderer's palettised `texture{2,4,6,8}.zbd` tiers
vary by mission and are skipped. There are 13 mission directories but 12
zmaps: m13 has none, and the catalog tolerates the gap.

### 5.2 Asset loading

Bevy `AssetLoader` impls, one per engine type, keyed by extension conventions
inside the cache (`.gamez.zip`, `.tex.zip`, `.anim.zip`, `.rdr.zip`,
`.zmap.json`, ...). Bevy's `AssetServer` is pointed at the cache via a custom
`AssetSource` named `cache://`. This gives hot-reload and async loading for
free.

Textures: a `TextureSet` loader reads the ZIP once, decodes each PNG into an
`Image` via labelled sub-assets, and applies `TextureAlpha` to
choose `AlphaMode`. Names in `gamez` carry a source extension (`dockp01k.tif`)
that must be stripped to match the archive's bare stems. **Texture lookup is
cross-mission**: a level references effect and HUD textures held in
*other* missions' archives, so a miss falls back to a pool assembled from
every `rtexture4` plus `image.zbd` rather than failing. Nearest or linear filtering is a settings toggle (the
original was point-sampled at low resolution; linear looks better).

### 5.3 Tuning data (`assets/`) — an override layer

The original's own values are primary. Each table is built at load as
*reader values, then RON overrides applied*, so every RON leaf is `Option`
and an empty file is the correct default:

| Table | Primary source | RON override file |
|---|---|---|
| `VehicleTable` (per kind, per form) | `vehicle{,_easy,_hard}.zrd` | `tuning/vehicle.ron` |
| `PlayerConstants` (gravity, camera, slope) | `player.zrd` | `tuning/vehicle.ron` (`player` section) |
| `WeaponTable` (19 weapons) | `weapons.zrd` `BALLISTICS` | `tuning/weapons.ron` |
| `PickupTable` (40 kinds) | `pickup.zrd` | — |
| `AiTable` (nets, units, turrets) | `ai.zrd`, `aiv*.zrd`, `net_*.zrd` | `tuning/ai.ron` |
| `SoundTable` (`snd_*` → wav) | `sounds.zrd` | — |
| chase camera offsets | `vehicle.zrd` `camback`, `player.zrd` | `tuning/camera.ron` |

`tuning/ai.ron` holds only what the original has no equivalent for: the
knobs of *our* steering implementation (reaction ticks, lead factor, aim
error, whisker lengths). Likewise the derived controller constants
(spring stiffness, engine force) that we compute from reader values live in
`vehicle.ron` with a `// approximation` comment and a note saying which
reader field they were derived from.

Difficulty (`Easy`/`Normal`/`Hard`) selects whole reader files
(`vehicle_easy.zrd`, `aiv_hard.zrd`, `puppies_easy.zrd`), it does not scale
values at runtime.

## 6. Rendering

- One Bevy `Mesh` per `ModelDesc` (shared across nodes referencing the same
  `model_index`). Polygons become triangles: `TRI_STRIP` flag → strip
  expansion; otherwise fan triangulation of the n-gon. Per-polygon material
  index groups triangles into one mesh per (model, material) pair; a node with
  a multi-material model spawns one child entity per material group. In
  Recoil's data no polygon is a strip and every polygon has one material,
  but 73 % are quads and 7 % are 5- to 14-gons, so the fan path is the one
  that matters. Face normals are generated only where a polygon has no
  `normal_indices`.
- `StandardMaterial` with `unlit` when the model lacks the `LIGHTING` flag,
  `double_sided`+`cull_mode: None` when `SHOW_BACKFACE`, `fog_enabled` from the
  `FOG` flag, `alpha_mode` from texture alpha (`Blend` for full alpha; level
  textures are opaque or fully alpha'd, one-bit cutout appears only in
  `image.zbd`, docs/data/materials.md). Vertex colours are never used in this game's data but
  the attribute is still populated when present, since §10 wants it available
  to a custom shader.
- Cycled textures and scroll come from **interp, not from `gamez`** (docs/data/materials.md and docs/data/interp.md): no material in any level has `CycleData` and every
  model's `texture_scroll` is zero, yet the runtime interp script binds 71
  cycle sets and 97 scrolls by node name. `bft-assets::interp::runtime_bindings`
  yields `TextureAnimBinding { node, sub_node, anim }`; the renderer applies
  `Cycle { frames, fps, looping }` by swapping `base_color_texture` and
  `Scroll { u, v }` via `uv_transform`. Materials are cloned per animated
  node so the change does not affect sharers. The `CycleData` path stays in
  the loader in case another game's data uses it.
- Facades: a `Facade { mode }` component and a `PostUpdate` system rotating
  the entity toward the camera around the allowed axis. Every facade in the
  game is `CylindricalY`; `Points` models never occur.
- Fog: `DistanceFog` on the camera from `World.fog_color/fog_range`;
  `fog_altitude` is a Should (custom material later).
- Lights: `Light` nodes → `PointLight` with colour and range. Ambient from a
  tuning constant until the data says otherwise.
- Camera: `Camera3d` in `bft-game::camera` with two behaviours —
  `ChaseCamera` (in-game) and `FlyCamera` (viewer).

A custom `Material` (WGSL) is a later step if `StandardMaterial` cannot match
the original's look (vertex-colour modulation, fog altitude bands). The mesh
builder is written so the material type is a single swap point.

## 7. Simulation subsystems

### 7.1 Physics and terrain (`bft-sim::physics`)

- avian3d, `PhysicsPlugins::new(FixedUpdate)`. Gravity is **40 m/s²** from
  `player.zrd` `nom_gravity`, with 9 in water and 3 in quicksand. The
  original's "floaty" feel is high gravity plus strong engines, not low
  gravity; do not tune from 9.8.
- Static level geometry: `Collider::trimesh_from_mesh` equivalent built from
  `ModelDesc` polygon data (not from the render mesh, so it works headless).
  One collider per Object3d node; `RigidBody::Static`.
- **Node flags are the collision authority** (docs/data/nodes.md), not node
  names and not soil: `altitude_surface` (set on ~74% of m1's nodes) marks
  what the vehicle rides on, `intersect_surface` (~79%) what it collides
  with. Nodes with neither are decorative and get no collider.
  `intersect_surface` alone means "wall, but not driveable": 5–12 % of
  nodes per level are such walls (docs/data/water.md).
- Each collider carries `SurfaceInfo { soil_by_triangle, dominant }` from the
  polygon materials; per-triangle soil lookup via hit triangle index is how
  the vehicle decides water/lava/quicksand/ground.
- **Soil drives surface behaviour** (docs/data/water.md). `Soil::Water`,
  `Lava` and `Quicksand` are set on exactly the liquid polygons, and the
  reader keys its physics on them (`wat_gravity`, `quicksand_slowdown`,
  `lava_slowdown`, per-surface weapon impacts). Water volumes are built from
  connected `Soil::Water` triangles, not from node names: `surface_y` is the
  polygon height, `floor_y` the nearest `altitude_surface` triangle below
  (absent → shallow, no sub). Node names (`water*`, `underwater*`, `lava*`)
  are a debug overlay and a consistency check only.
- Collision layers: `Level`, `Vehicle`, `Projectile`, `Pickup`, `Trigger`,
  `Water`.

### 7.2 Vehicle (`bft-sim::vehicle`)

A "raycast vehicle" on a dynamic `RigidBody`: four suspension rays hold the
hull above ground, forces applied in the hull frame. **Forms are named after
the data**: `vehicle.zrd` gives each vehicle a `common_mode` block plus
one `<form>_mode` block per form it supports, and the player has all four.

| Form | Ground model | Data |
|---|---|---|
| Track | Suspension rays on ground; strong lateral friction; turn in place. | `track_mode`, `mode_alt 0`. Default; 19 of 25 vehicle kinds have it. |
| Hover | Softer spring, low lateral friction, altitude controller; crosses water. | `hover_mode`, `mode_alt 0.5`, `alt_control` gains. |
| Amphib | Rides the water surface; bobbing from `amphib_wave`. | `amphib_mode`, `mode_alt -0.1`. |
| Sub | No suspension; thrust with pitch control, clamped between floor and surface. | `sub_mode`, `mode_alt -0.5`, `gun_pitch` limits. Deep water only. |

Each mode block supplies the suspension quad (`platform`), the 12-point
collision hull, `rates`, `turns`, damping, `friction`, `mass`, soil
slowdowns, and the **pair of anim names played for every transition**
(`t2a_anims` … `s2a_anims`). Our raycast parameters are derived from those
with the mapping written down in `vehicle.rs`.

**Transition graph** (from the key set): track↔amphib, track↔hover,
amphib↔hover, amphib↔sub. There is no direct track↔sub; the sub is reachable
only from amphib. Switching is automatic on entering water in Track form when
Amphib is unlocked (`ok_to_amphib`), and manual otherwise; `FormChanged`
carries the two anim names for the VM to play. Unlocks come from the three
chip pickups (`pu037`–`pu039`) and gate which forms the switcher may pick.
Turret is a child transform driven by `PlayerInput.turret_delta`, clamped by
`gun_pitch` where the mode has it.

### 7.3 Weapons (`bft-sim::weapons`)

`WeaponTable` built from `weapons.zrd` `BALLISTICS`: **19 weapons**
keyed `wep_<slot>.<variant>`, slots 1–9 weak/strong plus an enemy-only small
missile. The mover is derived from the data's own flags — `BEAM` → hitscan,
`MINE` → mine, `LOCK_ON`/`TURN_RATE` → homing, `GRAVITY > 0` → ballistic,
else straight. **Three weapons are hitscan** (laser designator, laser sabre,
arc sabre), so the `Beam` mover is real, not speculative.

Firing spawns a `Projectile` entity with a kinematic mover and a small
collider on the `Projectile` layer; on hit → `DamageEvent` for entities with
`Health`, `NodeDamage` for level geometry (§7.4). Impact effects are chosen
per surface: `weapons.zrd` `IMPACT` has `default`, `water` and `quicksand`
branches matching the hit polygon's `Soil`. Mines are stationary projectiles
with a proximity trigger and optional remote detonation. The nuke is a splash
of radius 256 with a screen effect on the presentation side. Ammo limits,
pickup kinds and the per-vehicle starting loadout all come from the reader.

### 7.4 Damage and destruction (`bft-sim::damage`)

Two paths, because the original splits them:

- **Vehicles and turrets** carry `Health` (from `vehicle.zrd` and `ai.zrd`).
  `DamageEvent` → `Health` → `Destroyed`, which spawns the kind's drops
  (`pickups`/`nanite` rows) and calls its death animation
  (`DESTROY_ANIM`, or the weapon's `KILL_ANIMATION` for the killing blow).
- **Scenery has no `Health` component at all.** A destructible prop is an
  *anim def* with `WeaponHit`/`CollideHit` activation and a `health`
  threshold; the def's own sequences swap `healthy` for `destroyed`, throw
  pieces and play effects. So a projectile hitting level geometry emits
  `NodeDamage { node, amount }` and the anim VM (§7.6) owns the outcome.
  `Destructible` and `DestroyEffect` components do not exist.

### 7.5 AI (`bft-sim::ai`)

**The original's AI data is read, not re-invented**. Per campaign
mission: `aiv.zrd` places every enemy instance (`<kind>_<NN>` → net index,
position, heading), `net_<NN>.zrd` gives that group its activation and attack
radii, dwell times, pursuit and return ranges, an `attack_strategy` and a
1–97 node waypoint graph, and `ai.zrd` defines turrets by node-name glob.
Enemy bodies are cloned from the level's template subtrees (§4.4) and reuse
the player's vehicle controller with `Controlled::Ai`.

`AiBrain` states follow the data: `Dormant` until the player is inside
`activate_rad` (straight-line distance, no line of sight), a buddy wakes the
group, or the unit is hit; `Roam` — ours, not the data's — walks the net
without hunting while the player is inside `ai.ron`'s `roam_range`, so units
in view are not parked; `Patrol` along the net,
`Engage` applying the authored strategy (`follow_net`, `head_on`,
`back_forth`, `circle`, `sit`), `Return` beyond `return_range`, `Hide` for
`defilade`/`hide` nets. Perception is distance + line-of-sight raycast (hull middle to hull
middle, not ground to ground) every N ticks, a brain fires only at a
target it currently sees, and nothing sees the player while a cinematic
holds the input. A unit whose weapon is a mine (the minelayer's P-HEM) lays
it in front of itself rather than aiming it; steering is seek/arrive along the waypoint graph with terrain probe
rays. **No navmesh and no invented patrol routes** — FR-E3's "no navmesh in
v1" holds because the original ships a hand-authored network. Turret enemies
skip locomotion and aim the `turret`/`gun`/`firepoint` child nodes named in
`ai.zrd`. Lead-aim uses target velocity and projectile speed; the aim error
and reaction cadence are ours (`ai.ron`).

Multiplayer maps carry no `aiv` enemies and no nets; only turrets.

Flying units (VTOL, helicopter, parachute) are **anim-scripted set pieces**,
not AI agents, so there is no air-vehicle controller in v1.

### 7.6 Mission and scripting (`bft-sim::mission`, `bft-sim::anim`)

- `MissionScript` is built from the level's reader: `objectives.zrd`
  gives each objective its briefing picture, three `MSG_*` text keys, initial
  active state, completion condition and hint sounds; `startanims.zrd` names
  the entry animation (`start_single_player`, or `load_control` on a loaded
  game); the player start is `aiv.zrd/bft_00`; death is the `deathmN`
  animation. Campaign titles are `MSG_CAMPAIGN<N>_NAME`. There are 32
  objectives across the six campaign missions. No hand-authored RON campaign
  fallback is needed. The multiplayer maps ship placeholder copies of m2's
  `objectives.zrd` and `location.zrd`, so no mission script is built for
  them.
- **Objectives complete on node active-state changes.** Each objective names
  a gamez node (or an `object, child` pair; a pickup instance like
  `pu03700` is accepted too, though no shipped objective uses one); the
  mission's anim sequences flip that node's active flag when
  the goal is met, and the mission system watches for the change from its
  value at mission start. There is no tag on the destroyed entity and no
  dedicated callback, so `objective_tag` does not exist.
- Anim VM: each `AnimDef` becomes an `AnimInstance`. At level start every
  def's `reset_state` is applied (initial visibility, §4.4) and `OnStartup`
  defs are armed. Activation is `OnCall` (from another def, from
  `startanims.zrd`, or from the engine on form change, enemy death or weapon
  kill), or damage-driven (`WeaponHit`/`CollideHit` accumulating toward the
  def's `health`), gated by `activ_prereqs`. `execution` is a *condition on
  top of* activation, polled per tick: `ByRange` needs the player within N
  metres, `ByZone` needs the player in the root node's `zone_id`.
  `reset_time` decides re-arming (−1 never, 0 immediately, positive after a
  delay).
- Sequences execute events in order; an event with no start offset runs in
  the same tick as its predecessor, otherwise the offset counts from the
  animation, the previous event or the sequence start. `Loop` and the four
  `If` conditions (`AnimationLod`, `PlayerRange`, `RandomWeight`,
  `NodeUndercover`) are core VM features, not handlers. Everything else is a
  registry keyed by `EventData` variant; unimplemented variants log once and
  are skipped. Handlers mutate sim components and emit presentation events
  (sound, effect, fog change) through `AnimOutput` for `bft-game` to draw.
- **Interp is not a runtime language** (docs/data/interp.md). Each mission
  has a build script that produced the shipped `gamez.zbd` and a runtime
  script that only binds texture scroll/cycle animation, sets the weapon
  tether altitude and names the well-known nodes. That subset is extracted as
  data by `bft-assets::interp`; there is no interpreter.

### 7.7 Multiplayer (`bft-sim::net`)

- `bevy_replicon` (pinned) with a UDP transport crate (`bevy_replicon_renet`).
- Server (host or dedicated) runs the full sim. Clients send `PlayerInput`
  messages each tick; the server applies them to the client's vehicle.
- Replicated components: `Transform` (quantised), `LinearVelocity`,
  `VehicleForm`, `Turret`, `Health`, `WeaponLoadout` (owner only),
  `Projectile` spawn/despawn, `Score`, and **node active-state changes**
  (the anim VM's `ObjectActiveState` results), since doors, destroyed props
  and objective state are all expressed that way. 303 anim defs carry a
  `network_log` flag, which is the original's own marker for "this def's
  effects must reach clients"; use it to scope what gets replicated.
- Client prediction for the local vehicle and interpolation for others is
  FR-N4 (Should); v1 first ships with server-authoritative + interpolation,
  prediction added if LAN feel demands it.
- Single-player: `NetRole::Local` runs the server systems in-process with no
  transport, satisfying FR-N6.

## 8. Presentation subsystems (`bft-game`)

- `level_view`: observers attaching meshes/materials/lights to sim entities.
- `camera`: chase camera (spring-arm, autocentre behind hull, looks along
  turret), fly camera (viewer).
- `hud`: Bevy UI. Health bar, weapon/ammo, reticle colour from an aim raycast
  result the sim publishes, automap drawn from `ZMap` features as a UI
  image or 2D camera render target, objective/message log. Only the
  campaign has real automaps: m7–m12 ship byte-identical copies of
  `m1.zmap` and m13 has none, so the UI does not present them as maps.
- `menus`: Bevy UI state-driven screens; input rebinding via
  a plain action → bindings table saved in `config.ron` (every action is a
  button, so `leafwing-input-manager` is not used).
- `audio`: `bevy_audio` (or `bevy_kira_audio` if spatial audio needs it) playing
  `SoundBank` handles on `SoundEvent`s from the sim. The CD's four audio
  tracks are not installed but can be ripped as WAV; FR-S3's music
  directory can point at them.
- `effects`: muzzle flash, explosions, tracers — simple billboards/particles
  from our own assets; the original's puffer effects come via anim events
  later.
- `devtools`: `bevy_egui` inspector panels (node tree, anim instances, AI
  state), gizmo overlays, level picker.

## 9. Cross-cutting

- **Config:** `config.ron` in the platform config dir: data root, cache dir,
  unzbd path, video, audio, keybinds, player name.
- **Logging:** `tracing` via Bevy's `LogPlugin`; data errors at `error!` with
  file and field; unknown-variant warnings at `warn!` once per variant.
- **Errors:** `thiserror` enums per loader; `anyhow` only in `bft-bin`.
- **Determinism:** fixed timestep; no `Time<Real>` in sim systems; RNG is a
  seeded `ChaCha8` resource stepped in the sim; system ordering explicit via
  sets. Floating-point cross-platform bit equality not attempted.
- **Saves:** `save.ron` with campaign progress; in-mission saves under
  `<data dir>/saves/` in the versioned format of `SAVEGAME.md`.
- **Testing:** unit tests in `bft-sim` with `MinimalPlugins` and hand-built
  `LevelData`; loader tests behind `BFT_DATA_DIR`, skipped if unset; a
  `cargo test --workspace` never touches the network.

## 10. Extensibility (later-phase features the design must not preclude)

Target platform for all of these is Linux with a capable GPU. macOS keeps
the faithful game only.

| Feature | What makes it possible | What to keep true now |
|---|---|---|
| Ray-traced lighting (Bevy Solari), upscaling (DLSS/FSR), post-processing | Bevy's render graph and wgpu feature gating; `StandardMaterial` already feeds the PBR/RT paths. | All material construction goes through `bft-game::materials::build_material` and mesh building through `mesh_builder`. A "modern look" setting swaps those two implementations; nothing outside `bft-game` changes. Keep vertex colours and soil in the engine-owned mesh data so a custom shader can use them. |
| Higher-resolution texture packs | `TextureSet` is keyed by texture name. | An override directory checked before the extracted cache; no other code path knows where an image came from. |
| VR (OpenXR via community Bevy crates) | Sim runs at fixed 60 Hz independent of render rate; presentation reads interpolated transforms. | The camera stays a component (`ChaseCamera`/`FlyCamera`) that presentation systems own; gameplay code never reads the camera transform for logic (aim uses the turret, not the view). HUD elements are built so they can be re-parented into world space (Bevy world-space UI) rather than assuming the primary window. Input goes through `Action` bindings, so controllers map without touching the sim. |
| Custom WGSL materials for the original look (fog altitude bands, vertex-colour modulation) | Bevy `Material` trait. | Same swap point as above. |
| Mods / data overrides | Everything is loaded via the asset server from `cache://` and `assets/`. | Add an `override://` asset source ahead of both once needed; loaders do not care. |

Anything here that would force a change in `bft-sim` or `bft-assets` is a
design smell; raise it before implementing.

## 11. Risks and unknowns

| Risk | Impact | Mitigation |
|---|---|---|
| mech3ax `gamez` for Recoil is rough (M6/M9 hacks) | Some levels fail to load or render wrong | Load per-level with error isolation; report; contribute findings upstream. |
| avian3d trimesh performance on large levels | Frame/tick budget | Per-node colliders + avian's BVH; convert far static nodes to `Sensor`-less coarse colliders if needed. |
| Bevy 0.20 release mid-project | Migration cost | Stay on 0.19.1 until a milestone is done; migrate once, between milestones. |
| Feel mismatch without executable RE | Game "plays wrong" | The original's own physics constants are read. What remains is the **unit ambiguity** in a handful of reader fields (`rates`, `turns`, `friction`, the `FIRE_RATE` split, four opaque weapon-row columns — READER-SCHEMA §5). Settle by driving side-by-side with the original in a VM; corrections go in the RON override layer, never in code. |
