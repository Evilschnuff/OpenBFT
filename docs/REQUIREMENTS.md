# open-bft — Requirements

Companion documents: [DESIGN.md](DESIGN.md), [CONVENTIONS.md](CONVENTIONS.md),
[data/README.md](data/README.md).

---

## 1. Purpose

open-bft is an open, from-scratch reimplementation of the game engine behind
Zipper Interactive's *Recoil* (1999). The engine is original code (Rust + Bevy);
the game's data (levels, models, textures, sounds, scripts) is read at runtime
from the user's own copy of the game. The model is the one used by OpenMW,
OpenRCT2 and similar projects: the original game becomes playable on modern
hardware and operating systems, and the code is free to study and extend.

The project is also a learning vehicle for Rust, ECS architecture and Bevy.
Where a requirement forces a trade-off between "fastest path to a playable
game" and "well-structured, understandable engine", the latter wins unless it
blocks the former outright.

## 2. Definitions

| Term | Meaning |
|---|---|
| Recoil | The 1999 PC game. Six single-player campaigns plus multiplayer maps. |
| BFT | Battle Force Tank, the player vehicle. Gains hovercraft, boat and submarine forms during the campaign. |
| GameZ | Zipper's engine (shared with MechWarrior 3). The `.zbd` file family is its data format. |
| mech3ax | TerranMechworks' Rust extractor for GameZ formats. Its `unzbd` CLI converts `.zbd` files into ZIP archives of JSON and PNG. |
| Data root | The directory holding the user's Recoil install (contains `zbd/`, `maps/`, `strings.dll`, ...). |
| Extracted cache | The directory of `unzbd` output the engine actually reads. Never committed. |
| Faithful | Behaviour that matches the original game as observed by a player. Not bit-identical: the original executable is not reverse-engineered (see §5). |
| Simulation | Everything that changes game state per tick: physics, vehicles, weapons, AI, mission logic, anim events. Runs at a fixed timestep and must be able to run headless. |
| Presentation | Rendering, camera, HUD, audio, menus. Runs at frame rate and reads simulation state. |

## 3. Users and stakeholders

- **The developer (primary).** One person.
- **Players who own Recoil.** Want to play the original campaign and
  multiplayer on macOS, Linux and Windows without a Windows 98 VM.
- **Format researchers.** The mech3ax community. This engine is a consumer of
  their work and a source of feedback on which fields matter.

## 4. Decisions already taken

These are inputs to the requirements, not open questions.

| Decision | Choice |
|---|---|
| Fidelity target | Faithful, data-driven. The engine loads the original `gamez`, `zmap`, `reader`, `interp`, `anim`, texture and sound data and reproduces the original missions, vehicles, weapons and feel as closely as is practical. |
| Reverse engineering the executable | **Out of scope.** Physics constants, weapon statistics and AI behaviour are approximated from observation, from reviews and manuals, and from whatever the `reader`/`anim` data holds. All such values live in editable data files so they can be tuned later. |
| Stack | Rust, Bevy (pinned `=0.19.1`), wgpu. |
| Physics | avian3d for collision and rigid bodies, with a hand-written arcade vehicle controller on top. |
| Asset extraction | mech3ax's `unzbd` CLI run as a separate process. Engine reads JSON + PNG. Direct crate dependency deferred until mech3ax's output stabilises. |
| Licence | EUPL v1.2 (same as mech3ax). |
| Game data in repo | Never. No textures, models, audio, levels, and no test fixtures carved out of them. |
| v1 scope | Single-player campaign **and** multiplayer. Original menus and cutscenes are a later phase (§5.2). Reverse engineering the executable is out of scope in every phase (§5.3). |
| Reference data | The retail EA 1.0 release (`docs/data/install.md`). Requirements marked *(data-dependent)* are verified in the corresponding docs/data file. |

## 5. Scope

### 5.1 In scope for v1

1. Load and render every single-player level and every multiplayer map from
   the original data.
2. Play the six campaigns end to end: objectives, briefing text, progression,
   vehicle form unlocks.
3. The BFT in all four forms (tank, hovercraft, boat, submarine) with a
   third-person chase camera and independently aimed turret.
4. All 18 weapons (9 slots × weak/strong), pickups, ammunition, health.
5. Enemy vehicles and turrets with AI good enough to make the campaign
   playable and challenging in roughly the original way.
6. Destructible scenery where the original data marks it as such.
7. Scripted level events from `anim.zbd` (doors, triggers, explosions,
   cutscene-like camera moves) to the extent needed for the campaign to be
   completable.
8. Sound effects from the original sound archives. Music playback from
   user-supplied files (the original streams from CD).
9. A functional (not art-faithful) HUD: health, weapon/ammo, target reticle,
   automap with objective markers, mission messages.
10. Functional menus: mission select, settings, multiplayer lobby.
11. Multiplayer deathmatch on the dedicated maps, LAN and direct IP, with a
    headless dedicated server option.
12. Developer tools: level viewer, data inspector, debug overlays.
13. Linux and macOS (Apple Silicon) builds; Windows builds best-effort.

### 5.2 Later phases (design must not preclude)

- Faithful reproduction of the original front-end art, HUD art, and video
  briefings/cutscenes.
- Optional modern rendering on Linux with capable GPUs: ray-traced lighting
  (Bevy Solari), upscaling, higher-resolution texture packs. These are an
  alternative look, switchable in settings; the faithful look stays the
  default.
- VR via OpenXR (Linux; no OpenXR runtime exists on macOS).
- Save games in the original format (mech3ax has no Recoil save support).
- Internet matchmaking or master server.
- Mod support beyond editing the data files the engine already reads.
- Contributing Recoil format findings back to mech3ax.

### 5.3 Out of scope

- Disassembling or decompiling `Recoil.exe` or any other original binary.
- Redistributing any original game data, including demo data.
- Emulating the original renderer's exact pixel output.
- Support for MechWarrior 3, Pirate's Moon or Crimson Skies data (the formats
  overlap, but this project targets Recoil only).

## 6. Functional requirements

Priority uses MoSCoW: **M**ust, **S**hould, **C**ould. Requirements marked
*(data-dependent)* cannot be finalised until real game data has been inspected;
docs/data/ records the verification.

### 6.1 Data root and extraction

| ID | Requirement | Pri |
|---|---|---|
| FR-D1 | The engine locates the data root from, in order: a command-line argument, the `BFT_DATA_DIR` environment variable, a user config file, and a short list of conventional install paths. It fails with a clear message listing what it looked for if none is found. **A command-line argument is exempt from the fallback: if the path it names does not validate, the search stops there and reports that path, rather than continuing on to run some other install the user did not ask for**. | M |
| FR-D2 | The engine validates a data root by checking for the expected files (`zbd/` directory, `maps/`, `strings.dll` or `messages.dll`) and reports which are missing. *(data-dependent: exact file list)* | M |
| FR-D3 | The engine can run `unzbd` itself to populate the extracted cache, given a path to the binary, or use a cache the user populated by hand. Extraction is idempotent and skips up-to-date outputs. | M |
| FR-D4 | The extracted cache records the mech3ax version that produced it. A mismatch with the version the engine was tested against produces a warning, not a failure. | S |
| FR-D5 | The engine reads mech3ax output (ZIP archives of JSON and PNG, plain JSON) without any binary `.zbd` parsing of its own. | M |
| FR-D6 | Every mech3ax JSON field the engine relies on is documented in one place, and unknown or extra fields are tolerated. | M |
| FR-D7 | No original game data, extracted or otherwise, is ever written into the repository, including under `assets/` or in tests. Tests that need data read it from a developer-configured root and are skipped, not failed, when it is absent. | M |

### 6.2 World loading and rendering

| ID | Requirement | Pri |
|---|---|---|
| FR-W1 | Every `gamez.zbd` in the install loads into a renderable scene: textures, materials, models and the node hierarchy. | M |
| FR-W2 | Model polygons render with correct texturing, vertex colours, back-face flags, and per-polygon materials. Triangle strips and n-gons are handled. | M |
| FR-W3 | Textured materials support the texture alpha modes mech3ax exposes (none, simple/cut-out, full) and cycled (animated) textures. | M |
| FR-W4 | Facade (billboard) models rotate to face the camera in the mode the data specifies (cylindrical about Y, spherical, etc.). | S |
| FR-W5 | The World node's fog colour and ranges are applied. Lights from the node graph light the scene approximately as in the original. | S |
| FR-W6 | Level-of-detail nodes switch by the ranges in the data. | S |
| FR-W7 | The scene's coordinate handedness and up axis are established from the data and applied consistently so levels are not mirrored or on their side. *(data-dependent)* | M |
| FR-W8 | Levels render at a stable 60 fps on the reference machine (Apple M2) at 1440p. | S |
| FR-W9 | Water surfaces and other soil-typed surfaces are identifiable to the simulation (for boat/submarine forms and terrain behaviour). *(data-dependent: how water volumes are encoded)* | M |

### 6.3 Player vehicle

| ID | Requirement | Pri |
|---|---|---|
| FR-V1 | The BFT drives as an arcade tank: keyboard throttle and steering, low gravity, airborne over jumps, turns in place when stationary. | M |
| FR-V2 | The turret aims independently of the hull using the mouse; the camera follows the turret direction and auto-centres behind the hull as in the original. | M |
| FR-V3 | Vehicle forms: tank, hovercraft (fast, low friction, drifts), boat (floats on water, cannot submerge), submarine (full 3D movement below the water surface, only where water is deep enough). Form availability is a campaign unlock; form switching is driven by terrain where the original did so. *(data-dependent: whether switching is automatic, manual, or zone-triggered)* | M |
| FR-V4 | The vehicle collides with terrain and scenery, takes collision and fall damage where the original did, and cannot leave the level bounds. | M |
| FR-V5 | Health, and repair via pickups. Destruction ends the mission with a restart option. | M |
| FR-V6 | Vehicle tuning is read from the original reader data (`vehicle.zrd`, `player.zrd`, form-transition rules) where present; an engine-side data file supplies anything missing and can override. No tuning constants in code. | M |
| FR-V7 | Input is rebindable and supports keyboard+mouse. Gamepad support is a Could. | S/C |
| FR-V8 | Difficulty levels (the data has easy/standard/hard variants of vehicle and AI parameters) selectable per campaign start. | S |

### 6.4 Weapons

| ID | Requirement | Pri |
|---|---|---|
| FR-A1 | Nine weapon slots (keys 1–9), each with a weak and a strong variant, 18 weapons total. `weapons.zrd` holds exactly these 18 plus one enemy-only missile; names, slots and full ballistics are in READER-SCHEMA §2.3. | M |
| FR-A2 | Projectiles are simulated entities with travel time, gravity where applicable, homing where applicable, and area damage on detonation. | M |
| FR-A3 | Ammunition per weapon; refills dropped by destroyed enemies matching their armament; weapon pickups placed in levels. | M |
| FR-A4 | The reticle indicates a valid target (colour change red→green) when the aim ray hits something damageable. | M |
| FR-A5 | Enemies use the same weapon system and the same damage rules as the player. | M |
| FR-A6 | Weapon statistics are read from the original reader data (`weapons\*.zrd`, ammo keys) where present; an engine-side data file supplies anything missing and can override. | M |

### 6.5 Enemies and AI

| ID | Requirement | Pri |
|---|---|---|
| FR-E1 | Enemy types are instantiated from level data: which model, which weapons, where placed. *(data-dependent: where spawn lists live — nodes, reader, or anim)* | M |
| FR-E2 | Enemy behaviours: stationary turrets, patrolling vehicles, engage-on-sight with pursuit and lead-aim, retreat or idle out of range. Flying units (VTOL, helicopter) with altitude-holding movement. Behaviour parameters (`attack_strategy`, radii, dwell times) come from reader data. | M |
| FR-E3 | Enemies navigate using the authored path networks in the per-mission reader data (`net_*`, `follow_net`), with local steering for avoidance. The data has hand-authored networks, so no navmesh is needed. | M |
| FR-E4 | Destroyed enemies play a destruction effect, drop pickups, and can complete objectives. | M |

### 6.6 Missions and campaign

| ID | Requirement | Pri |
|---|---|---|
| FR-M1 | Six campaigns, each with 4–6 objectives, loaded from the original data. Objective text comes from the localisation data (`messages.dll`/`strings.dll`). *(data-dependent)* | M |
| FR-M2 | Objectives can be completed in the order the original allowed, including alternate paths. | S |
| FR-M3 | The automap (`m*.zmap`) is displayed on the HUD with the current objective marked using the `objective` index on map features. | M |
| FR-M4 | Scripted events from `anim.zbd` (activation prerequisites, sequences, object motion, light and fog state, sound, `DetonateWeapon`, `CallAnimation` chains, conditionals) run with enough coverage that every campaign objective can be completed. | M |
| FR-M5 | Mission briefing text is shown before a mission; voiced briefings are a Could pending audio extraction. | S/C |
| FR-M6 | Campaign progress (missions completed, forms unlocked) persists between runs in the engine's own save format. | M |
| FR-M7 | Any mission can be started directly from a developer/debug menu with any form unlocked. | M |
| FR-M8 | A campaign mission can be saved while the player drives and loaded later with its full state: objectives, damage, the scripts' progress, every enemy, pickup and shell. The format is versioned and specified in `SAVEGAME.md`, so saves survive engine updates. | S |

### 6.7 HUD, UI and menus

| ID | Requirement | Pri |
|---|---|---|
| FR-U1 | In-game HUD: health, current weapon and ammo, reticle, automap, objective and event messages. Functional layout; original art is a later phase. | M |
| FR-U2 | Front end: main menu, campaign/mission select, settings (video, audio, input), multiplayer host/join, pause menu. | M |
| FR-U3 | A custom cursor with the reticle's off/in-range/on-target states (our own crosshair, drawn in code, when the original's art is not available). | C |
| FR-U4 | All player-visible strings come from a localisation table; original strings from `messages.dll` where they exist. | S |

### 6.8 Audio

| ID | Requirement | Pri |
|---|---|---|
| FR-S1 | Sound effects load from the extracted sound archives and play positionally in 3D. | M |
| FR-S2 | Sound events referenced by the anim data (static and dynamic sounds) play at the right node. | S |
| FR-S3 | Music plays from user-supplied files in a configured directory (the original streamed CD audio, which is not in the install). | S |

### 6.9 Multiplayer

| ID | Requirement | Pri |
|---|---|---|
| FR-N1 | Deathmatch on the dedicated multiplayer maps for 2–8 players over LAN or direct IP. | M |
| FR-N2 | Server-authoritative simulation. Clients send inputs; the server owns game state and replicates it. | M |
| FR-N3 | A headless dedicated server build that runs the simulation without a window or GPU. | M |
| FR-N4 | Client-side prediction for the local vehicle and interpolation for remote entities so play feels responsive at LAN latencies (under ~50 ms). | S |
| FR-N5 | Score, respawn, and match end conditions (frag limit, time limit). | M |
| FR-N6 | Single-player runs on the same simulation code path (a local server in-process), so gameplay behaviour is identical online and offline. | M |

### 6.10 Developer tools

| ID | Requirement | Pri |
|---|---|---|
| FR-T1 | A level viewer mode: load any level, free-fly camera, no simulation. | M |
| FR-T2 | A data inspector that prints or displays the loaded node tree, models, materials, reader values and anim definitions for a level. | M |
| FR-T3 | Debug overlays: colliders, node bounding boxes, partition grid, AI state, network stats. | S |
| FR-T4 | `docs/data/`: living documents, one file per topic, of everything learned about the real data (file names, coordinate system, field meanings, quirks). Every data-dependent requirement points at one of them once verified. | M |

## 7. Non-functional requirements

| ID | Requirement |
|---|---|
| NFR-1 | **Platforms, in priority order:** Linux (Vulkan) is the primary target and the platform for power-user features (high-end GPUs, optional ray tracing, VR). macOS 13+ (Metal, Apple Silicon) must run the faithful game and is the day-to-day development machine. Windows 10+ (DX12 or Vulkan) is supported opportunistically: it should build and run, but is not tested per milestone. Single codebase, no platform-specific gameplay code. |
| NFR-2 | **Performance:** 60 fps at 1440p on Apple M2 in the heaviest campaign level (the faithful-gameplay floor); the Linux desktop reference machine is to be named once available. Simulation tick budget under 4 ms at 60 Hz with 8 players and 200 projectiles. |
| NFR-3 | **Determinism:** the simulation is a fixed-timestep loop that depends only on its inputs and state. Frame rate never affects gameplay. Bit-exact cross-machine determinism is *not* required (server authority covers multiplayer). |
| NFR-4 | **Headless capability:** the simulation crate compiles and runs without `bevy_render`, `bevy_winit` or audio. |
| NFR-5 | **Legal:** EUPL v1.2. No original data in the repository or CI. No code derived from disassembly. Third-party assets and code have compatible licences and attribution. |
| NFR-6 | **Data isolation:** every assumption about mech3ax's output format lives in `bft-assets`. Other crates never parse mech3ax JSON. |
| NFR-7 | **Build hygiene:** `cargo build`, `cargo test`, `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` pass on every commit to `main`. Dependencies pinned to exact versions. |
| NFR-8 | **Testability:** pure logic (vehicle model, damage, objective state machine, anim sequencing) is unit-testable without a window. Data-driven loaders have tests that run against a configured data root and skip cleanly without one. |
| NFR-9 | **Error handling:** data problems (missing file, unexpected JSON shape) produce a logged, actionable error naming the file and field. The engine never panics on bad data in release builds. |
| NFR-10 | **Startup:** a cached level loads in under 5 seconds on the reference machine. First-time extraction may take longer and shows progress. |

## 8. Constraints and assumptions

- **Bevy `=0.19.1`, avian3d `0.7.x`, Rust 1.98.1, edition 2024.** Bevy upgrades are deliberate events with a migration pass, never incidental.
- **mech3ax is unreleased on `main`.** The `main` branch has Recoil `anim.zbd` reading code that the README's support table does not list. The project builds `unzbd` from a **pinned commit** of `main`, recorded in `docs/data/install.md`.
- **mech3ax `gamez` for Recoil has "horribly dirty hacks for M6 and M9"** (changelog 0.6.0). Expect those two levels to be the roughest.
- **No `motion.zbd`, `mechlib.zbd`, `planes.zbd` in Recoil.** Models are in `gamez.zbd`; there is no separate skeletal-animation format. "Animation" in this engine means scripted node transforms and events (`anim.zbd`), plus cycled textures and texture scroll.
- **Save games:** mech3ax has no Recoil save support. v1 uses its own save format (`SAVEGAME.md`).
- **Music:** not in the install; streamed from CD. Out of the data pipeline entirely.
- The data (docs/data/reader.md) shows that vehicle tuning, weapons, enemy AI, path networks, spawns, pickups and effects are all in reader (`zrdr.zbd`) data. Understanding the reader format is therefore the critical path for milestones M2–M4.

## 9. Open questions

Each is answered from the data, in the docs/data file named after it.

1. Exact directory layout of a Recoil install: is `gamez.zbd` one file or one per mission directory? What are the mission directory names? How many `m*.zmap` files exist (the mech3ax changelog mentions "M6 and M9", suggesting at least 9 levels including multiplayer maps)? Answered in [data/install.md](data/install.md).
2. Coordinate system: up axis, handedness, unit scale, and Euler rotation order in `Transformation::RotationTranslation`. Answered in [data/coordinates.md](data/coordinates.md).
3. Where mission definitions live: `zrdr.zbd` (reader), `interp.zbd` scripts, `anim.zbd`, or a combination. What the interp scripts actually contain. Answered in [data/anims.md](data/anims.md) and [data/interp.md](data/interp.md).
4. How water is represented: `Soil::Water`/`Seafloor` materials on polygons, zones (`zone_id`), or something else. How the game knows where the submarine is allowed. Answered in [data/water.md](data/water.md).
5. Where enemy spawns, pickups and the player start are defined. Answered in [data/reader.md](data/reader.md).
6. What `NodeFlags` and `zone_id` mean for visibility and collision. Answered in [data/nodes.md](data/nodes.md).
7. Whether `anim.zbd` for Recoil extracts cleanly with mech3ax `main`, and which event types Recoil actually uses. Answered in [data/anims.md](data/anims.md).
8. Which sounds correspond to which weapons/vehicles (naming conventions in the sound archives). Answered in [data/sounds.md](data/sounds.md).
9. Weapon and vehicle names and slot assignments from `messages.dll` strings. Answered in [data/reader.md](data/reader.md).

## 10. Milestones

| # | Milestone | Exit criterion |
|---|---|---|
| M0 | Data reconnaissance | `docs/data/` answers §9 questions 1, 2, 7 and has a first pass at the rest. `unzbd` pinned and extraction reproducible. |
| M1 | Level viewer | Every level renders with textures in the fly-camera viewer. Coordinate system correct. |
| M2 | Drivable tank | BFT tank form drives on one level with collision, chase camera, turret, and one weapon that damages a placeholder target. |
| M3 | Combat slice | All weapons, pickups, health, two enemy types with AI, destructible scenery, HUD basics, sound. One campaign mission completable start to finish. |
| M4 | Campaign | All six campaigns completable; all vehicle forms; anim events; automap; briefings; progression saved. |
| M5 | Multiplayer | Deathmatch on all dedicated maps, LAN and direct IP, dedicated headless server. |
| M6 | Polish | Settings, input rebinding, performance targets met on all three platforms, packaged builds. |

Milestones M2–M4 are single-player. Multiplayer (M5) comes after the campaign
is complete, but the fixed-timestep, input-driven simulation it needs is a
constraint from M2 onward.
