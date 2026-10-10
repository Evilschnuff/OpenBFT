# Water

How water, lava and quicksand are built in the levels and how the engine finds them.

Sources: m1/m3/m6 `nodes.json` + `materials.json` (water/lava node dump),
per-level polygon-soil counts, `player.zrd`, `vehicle.zrd`, `weapons.zrd`
(`tools/recon/zrd.py`).

**Soil drives surface behaviour.** The count of
`Soil::Water` *materials* is small (1–6 per level) because a level has
only a few water textures, but those materials are applied to every water
polygon. Polygons per soil, all nodes with a model:

| Level | Default | Water | Lava | Fire | Quicksand | Water/Lava polys on `altitude_surface` nodes |
|---|---|---|---|---|---|---|
| m1 | 13418 | 157 | — | 84 | — | 145 / — |
| m2 | 18906 | 246 | 20 | 24 | — | 246 / 20 |
| m3 | 11160 | — | 329 | 24 | — | — / 320 |
| m4 | 18131 | 217 | 3 | 24 | — | 217 / 3 |
| m5 | 13290 | 66 | — | 24 | — | 59 / — |
| m6 | 17557 | 133 | 127 | 42 | — | 79 / 127 |
| m7–m13 | | 0–205 | 0–71 | 24–33 | 3 (m9) | |

`Fire` is the 24 shared effect polygons (`fire1`, `fireblast`, `firepoly`,
`napscroll` — weapon effect templates), not terrain. `m10` has no water.

The reader data confirms that soil is the runtime mechanism: `player.zrd`
has `nom_gravity 40`, `wat_gravity 9`, `qsd_gravity 3`, `qsand_sink`,
`lava_sink`; every vehicle mode has `quicksand_slowdown` and
`lava_slowdown`; `weapons.zrd` `IMPACT` is keyed by surface `default` /
`water` / `quicksand`; `vehicle.zrd` amphib/sub modes have `mode_alt`
offsets relative to the surface. **The vehicle samples the soil of the
`altitude_surface` polygon beneath it** and that decides water, lava,
quicksand and default behaviour.

How water is built in m1 (identical convention in m6, m13; m3 has lava in
the same roles):

| Node | zone | flags | model | material soil / texture | Role |
|---|---|---|---|---|---|
| `water02`, `water03` | 32 | altitude, intersect, clip_to | 1 / 12 polys | Water / `ocean1.tif` | the **riding surface** of the open sea at y = 0 (bbox y −0…0) |
| `waterup01…03` | 14 | none | 1–8 polys | Water / `ocean1.tif` | the **visual surface** above the sub-navigable pool; no collision flags |
| `underwater01…03` | 14 | altitude, intersect | 10–27 polys | Default / `mnsand1.tif` | the **sea floor** (bbox y −24…0) the sub rides on |
| `water_miss` | −1 | altitude, intersect | none | — | empty attachment node for water impact effects (every level) |
| `surf1`, `surf2` | 0 | altitude, intersect, clip_to | 1–2 polys | Default / `surf01.tif` (cycled by interp) | animated surf overlay on the beach |

So there are two water regimes:

1. **Shallow / open water** (`waterNN`, zone 32): a `Soil::Water` altitude
   surface. Tank form is slowed and sinks (`wat_gravity`), amphib rides on it
   at `mode_alt −0.1`. Nothing lies beneath; the sub cannot be used here
   (the `body_to_sub` transition is only allowed from amphib, and there is
   no floor). Hover crosses at `mode_alt 0.5`.
2. **Deep water** (zone 14): `waterupNN` is a `Soil::Water` polygon *without*
   collision flags and `underwaterNN` is a `Soil::Default` floor 24 m below
   with collision flags. The sub's allowed volume is **between the
   `underwater*` floor (altitude surface) and the `waterup*` surface (the
   nearest `Soil::Water` polygon above)**, bounded laterally by the
   `underwater*` model's extent. The amphib rides on `waterup*` as a
   surface (its y = 0 matches the sea level) even though the node has no
   altitude flag — the game must be treating `Soil::Water` polygons as an
   altitude surface for surface forms regardless of the flag; the engine
   does the same (treat every `Soil::Water` polygon as a
   `Layer::Water` sensor surface and every `altitude_surface` polygon as
   ground).

`zone_id` 14 and 32 are the water zones in m1 and m6 (m6 also 15 for
`wateraz`, 1 for `clwater*`); zones are otherwise level-specific numbers.
Lava (m3, m6, m8, m12: `lavascroll*`, `lavapool*`, `flow*`, `Soil::Lava`,
zone 11/13 in m6) uses the same pattern as shallow water with
`lava_slowdown`/`lava_sink` and damage. Quicksand exists only as three
polygons in m9 plus runtime patches created by freon/MD weapons
(`declient.zrd` `QUICK_SAND`).

Node naming is a reliable *secondary* signal (`water*`, `waterup*`,
`underwater*`, `clwater*`, `ocean*`, `wateraz`, `waterdrop`,
`lavascroll*`, `lavapool*`) but many water polygons sit in anonymous
`gNNNN` group nodes, so **soil is the authority, names are for debugging.**

Decision: **strategy B (soil-driven) is the primary
strategy**, refined as above; strategy A (named nodes) only for debug
overlays. Water volumes for the sub: for each connected set of
`Soil::Water` polygons, `surface_y` = their height, `floor_y` = the
altitude-surface polygons directly below (ray down), lateral extent = the
polygon footprint. Where no floor exists within a few metres the volume
is shallow (no sub).

## Strategy B over all 13 levels

Asserted in `crates/bft-sim/tests/real_physics.rs`.

- **Strategy B works, and its output per level is** `volumes (divable)`:
  m1 4 (1), m2 6 (2), m3 28 (10), m4 9 (2), m5 4 (4), m6 22 (10), m7 2 (0),
  m8 10 (1), m9 2 (1), m10 0 (0), m11 4 (4), m12 4 (0), m13 6 (6). m1 comes
  out exactly as this section describes: **one divable pool** (the zone-14
  `waterup`/`underwater` pair) and **three shallow** (the open sea, riding on
  its own polygon with nothing below). The shallow/deep rule and the
  "unflagged `Soil::Water` is still a surface" reading hold against all 13
  levels.
- **Three levels have no water, not one.** The water column of the table above is
  `—` for m3 and m10; m8 is the third. m3 and m8 are the lava levels. Counted
  as *collider triangles* through the real pipeline (a quad is two
  triangles), the liquid totals are:

  | | m1 | m2 | m3 | m4 | m5 | m6 | m7 | m8 | m9 | m10 | m11 | m12 | m13 |
  |---|---|---|---|---|---|---|---|---|---|---|---|---|---|
  | water | 297 | 442 | — | 309 | 131 | 257 | 79 | — | 377 | — | 88 | 222 | 187 |
  | lava | — | 46 | 597 | 6 | — | 216 | — | 163 | — | — | — | 27 | — |
  | quicksand | — | — | — | — | — | — | — | — | 8 | — | — | — | — |

  m9's quicksand is 8 triangles from the 3 polygons this section names.
- **The name check has to exclude the floor names.** A deep-water floor is
  `Soil::Default` by design, so `underwaterNN` and
  `oceanfloorN` are correctly not liquid — but a naive "named for water,
  carries no water soil" check flags them: 14 false positives across the 13
  levels (m1 3, m6 2, m11 6) and no true ones. With floors excluded the only
  liquid-named nodes in the entire game that carry no liquid soil are
  **m4's `water_level` and `water_level2`, and m6's `a2lava`** — three nodes,
  reported as warnings and not acted on.
- **`intersect_surface` without `altitude_surface` is a real class of
  geometry**, which settles the question in favour of "walls":

  | Level | ride + collide | collide only | ride only | decorative |
  |---|---|---|---|---|
  | m1 | 3063 | 253 | 33 | 850 |
  | m6 | 3927 | 589 | 45 | 394 |
  | m13 | 1953 | 280 | 21 | 374 |

  Collide-only is 5–12 % of nodes; ride-only is 21–45 and reads as authoring
  slips. So the water pass looks for floors only among `altitude_surface`
  triangles, and a wall can never become a liquid volume's floor.
- **Cost.** m6, the biggest level, builds
  1949 colliders and 22 water volumes in **35 ms**, and steps its physics set
  in **0.31 ms** with no dynamic bodies (M-series macOS, unoptimised test
  binary). Collider caching is not needed.

Confidence: verified (polygon/material/flag data, reader keys, the volume,
flag and cost figures, the shallow/deep rule and the "unflagged
`Soil::Water` is a surface" reading, all over 13 levels).

## REQUIREMENTS §9, question 4

Water: **answered** above. Soil *is* the mechanism (`Soil::Water`/`Lava`
/`Quicksand` on the altitude-surface polygons; `player.zrd` and
`vehicle.zrd` key their gravity/slowdown on it); node names are a
secondary signal. Sub volume = between the `underwater*` floor and the
`waterup*` surface (zone 14 in m1/m6).

Confidence: verified with inferred details (sub volume rule).
