# Interp scripts

What the interp scripts contain, the build/runtime split, and the runtime subset the engine extracts.

Sources: `interp.json` (122 scripts; full text of the `mN.gs`, `mN_zbd.gs`,
`support\{common,commonmN,loadmN,bftN,missionN,pickup,weapons,tex_fx,
tex_fxmN,world,displaym,initm,morfutil}.gw` families read in full).

**Structure.** Two top-level scripts per mission:

```
m1.gs       source support\commonm1.gw ; source support\loadm1.gw ;
            GameZWriteZBDFile %MissionZBDFile% ; source support\tex_fxm1.gw ; Quit
m1_zbd.gs   source support\commonm1.gw ; GameZReadZBDFile %MissionZBDFile% ;
            source support\tex_fxm1.gw ; Quit
```

`mN.gs` is the **build** path: `loadmN.gw` creates the world (origin,
extents, partition, fog), loads the level `.flt`, then sources
`weapons.gw` (effect/projectile templates with collision flags off),
`bftN.gw` (player and enemy templates, VTOL, helicopter, node renames via
`NodeSetDescription`), `vehicles.gw` (empty), `missionN.gw` (mission
props, `NewObject3D countdown`, LOD/morph node surgery via `morfutil.gw`),
`pickup.gw` (40 pickup templates), adds the sun light, and writes
`gamez.zbd`. Everything it does is baked: the world/camera/display/window
/light nodes, template subtrees, flags (`SetAltitudeSurface`,
`SetIntersectSurface`, `SetLandmark`), and node names are all in nodes.md's
node data. `default.gs` sources `m2.gs`; `initmN.gw` only sets array
sizes.

`mN_zbd.gs` is the **runtime** path the shipped game executes on level
load. It does three things:

1. `commonmN.gw`: `SetMission N`, `set MISSION_DIR`, then `common.gw`:
   reader search path (`RdrSetPath common;mN;mN\aipath`), model/texture
   directories, `AnimSetZBDFile`, the well-known names
   (`worldName world1`, `camName camera1`, `winName window1`,
   `dispName display`, `playerName bft_00`), renderer settings
   (perspective correction, dither, small-polygon reject, lens-flare
   textures `lflare1…4`), then `WeaponSetMaxTetherAltitude 30.0`
   (all missions) and `SetSmallPolygonRejectArea 0.01` (m1, m9).
2. `GameZReadZBDFile` — load gamez.
3. `tex_fxmN.gw` → `tex_fx.gw`: **texture animation setup only**:
   `FindNode`/`FindSubNode` + `Object3DSetScroll on u v` (97 calls:
   arc beam, sizzle steam, freon, holo screens, conveyor `*scroll` nodes)
   and `CycleTextureSetOn n; CycleTextureSetSpeed fps; CycleTextureSetLooping
   on; CycleTextureSetMap tex.tif ×n` (71 cycle sets: surf, gulls, rocket
   blast, fire, water `clwatr01…10`, `watrp*`).

**Decision: partial execution — the runtime subset only, and it can be
implemented as a data extraction rather than an interpreter.**

- *Ignore* (baked into gamez): all of `loadmN`, `bftN`, `missionN`,
  `pickup`, `weapons`, `world`, `displaym`, `initm`, `morfutil`,
  `GameZWriteZBDFile`, `bftmulti.gw` (the multiplayer BFT's LOD surgery is
  in m7–m13's gamez already).
- *Execute / extract* from `mN_zbd.gs` and what it sources:
  1. `Object3DSetScroll on u v` on a node → UV scroll rate for that node's
     materials (`TextureAnim::Scroll { u, v }` per node name).
  2. `CycleTextureSetOn n` … `CycleTextureSetMap` on a node → frame list,
     fps and looping for that node (`TextureAnim::Cycle`).
  3. `WeaponSetMaxTetherAltitude 30.0` → tuning constant for the two
     tether-guided weapons.
  4. `set playerName bft_00`, `worldName world1`, `camName camera1` → the
     well-known node names (identical in all 13 missions).
  5. `SetMission N` → level number (already known from the path).
  Everything else in `common.gw` is renderer configuration for the 1999
  rasteriser and has no equivalent.

Implementation shape (asset-side, in `bft-assets`): a small line-oriented parser over `InterpScripts` that follows
`source` from `mN_zbd.gs`, expands `%VAR%` from `set`, tracks the current
`FindNode`/`FindSubNode` target, and emits `Vec<TextureAnimBinding { node,
sub_node: Option, anim: Scroll|Cycle }>` plus the tether altitude. Verbs
outside the list above are counted and ignored. No conditionals,
variables beyond `set`, or control flow exist in any script (`source`,
`set`, `mkdir` and commands only). So: **no interpreter at runtime**.

**Implemented** as `bft-assets::interp::runtime_bindings`. The grammar holds
exactly as described. Measured across all 13 missions' `mN_zbd.gs` trees (41
scripts reached by `source`, none missing):

- **215 cycle bindings and 157 scrolls**, against the 71/97 counted
  per-script above: those count each *script line*, but `tex_fx.gw` is
  sourced by all 13 missions, so a per-mission extraction sees its bindings
  once each.
- `CycleTextureSetOn n` always agrees with the number of
  `CycleTextureSetMap`s that follow — zero mismatches — which is what makes
  a stateful line parser safe here.
- `CycleTextureSetLooping` is **omitted on 13 cycles** (e.g. `wake_trail`);
  the engine defaults those to not looping, since a one-shot that stops is a
  missing effect and a loop that should have stopped is a visible bug.
- `CycleTextureSetSpeed 0.0` occurs (the player's `rtracks`/`ltracks` tank
  tracks, 4 frames, looping off). Speed 0 means **the game advances the
  frame, not the clock** — the engine drives it from track speed.
- `Object3DSetScroll` is `on` in all 157 cases; `u` is non-zero in 27 of
  them (conveyors, lava, the m3 horizon), `v` in the rest.
- `WeaponSetMaxTetherAltitude 30.0` in all 13, and the well-known names
  (`playerName bft_00`, `worldName world1`, `camName camera1`,
  `winName window1`, `dispName display`) are identical in all 13.
- `WriteTextureSetType`/`WriteTextureSetMap` (`pock1`–`pock3`) are on the
  runtime path too: they are the **impact decal** textures, for the
  weapons. The interp parser counts them as ignored verbs.

Confidence: verified (script contents, the build/runtime split, and the
counts above measured by the implementation); the mapping of
`CycleTextureSet*` to material behaviour is inferred from the verb names and
`effects.zrd`'s identical `SPEED`/`MAPS` vocabulary.
