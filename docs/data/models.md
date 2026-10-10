# Model details

Polygons, model types, flags, normals, UVs and embedded lights in the gamez models.

Source: the gamez model summary written by `tools/recon/summarize.py`.

Model counts per level run from 1588 (m6) and 1350 (m1) down to 616–910 for
the multiplayer maps, 13 036 models in total; **no model slot is null** in
any level. Models are heavily instanced:
m1 has 18 346 unique vertices across 1350 models, referenced by 2170 nodes
for 25 449 instanced vertices.

## Polygon vertex counts — n-gons are the norm

All 13 levels combined:

```
  4 vertices: 75225      8: 772      12: 273      9: 128
  3 vertices: 20092      7: 324      10: 215     14:  69
  5 vertices:  3608      6: 2446     11:   5     13:   1
```

**Quads dominate (73%), triangles are only 20%, and 7% are 5- to
14-gons.** The mesh builder must triangulate arbitrary convex n-gons, not
just handle quads. A simple fan triangulation is appropriate given these are
authored planar polygons from an OpenFlight pipeline.

## TRI_STRIP is never used

**`tri_strip` is false on every polygon in every level** — zero occurrences
across all 103 158 polygons. The only polygon flag ever set is
`show_backface` (m1: 1574 polygons). Strip decoding can be ignored
entirely; the mesh builder should honour `show_backface` by disabling backface culling per
material.

## Model types

Only two of the three `ModelType` values appear: `Default` and `Facade`;
**`Points` is never used**. Facades are common (m1: 296 of 1350; ~20–25%
everywhere) and every single facade in every level uses
`facade_mode = CylindricalY` — billboards rotating about the y (up) axis,
consistent with coordinates.md. These are the trees, bushes and smoke sprites.

Model flags: `fog` is set on essentially every model (m1: 1349/1350),
`lighting` on ~80% (m1: 1081). `texture_registered`, `morph` and
`clouds` never appear as set flags in the JSON.

## Other features

- **`texture_scroll` is zero on every model in every level** — the scroll
  animation interp's `Object3DSetScroll` sets up (97 calls) is applied at
  runtime, not baked. Read it from interp or ignore it.
- **Morphs exist but are rare**: 12–18 models per level have a non-empty
  `morphs` array.
- **Few models carry point lights, but those carry many**: 14–18 models
  per level (see below).
- **Vertex colours are never used**: zero polygons with `vertex_colors` in
  any level.
- **Multi-material polygons never occur**: every polygon has exactly one
  material with one set of UVs. Mesh splitting is therefore
  one submesh per material, with no per-polygon material blending.

## The emitted JSON, normals, lights and winding

- **The emitted model JSON is not the `api-types` listing**, as with nodes
  (nodes.md). `flags` and polygon `flags` are **objects of booleans**, not
  bitfields, and mech3ax skips serialising the false ones —
  `texture_registered`, `morph`, `texture_scroll`, `in_out` and `unk3` never
  appear in any level, so every flag field needs `#[serde(default)]`.
  `vertices`/`normals`/`morphs`/`bbox_mid` are `{x,y,z}` objects,
  `texture_scroll` and `uv_coords` are `{u,v}`, `material_index` is a bare
  number. Write loaders against the emitted JSON.
- **`normal_indices` is mostly `null`, and `normals` is not per-vertex.**
  **2120 polygons** across the 13 levels carry `normal_indices`, and of the
  456 models with a non-empty `normals` array only **41** have one normal
  per vertex — 400 have fewer, 15 more. `normals` is its own table, indexed
  *only* through `normal_indices` and never parallel to `vertices`. An
  importer that assumed `normals[vertex_index]` would be silently wrong on
  415 models.
- **`uv_coords: null` <-> a `Colored` material, exactly.** 2610 polygon
  materials have no UVs; all 2610 sit on `Colored` materials and no
  `Textured` material ever lacks them. A missing UV set is not a defect.
  UVs are **not** in 0-1 (u reaches 241 552): textures tile.
- **`clouds` and `facade_centroid` are never set** on any of the 13 036
  models, though unlike the three above they are emitted. Recoil has no
  centroid-pivoted billboards.
- **`priority` is non-zero on 795 polygons** (values 1-5 and 10),
  concentrated on textured surfaces — consistent with a transparency
  draw-order reading. **`zone_set` is usually *empty*** (52 568 polygons)
  rather than `[-1]`; the rest carry 1-3 zone ids.
- **3272 point lights across 200 models.** 3224 of the 3272 have exactly
  one position; the rest have 6-10.
- **Model-embedded lights are not level lighting.** There are **3288
  embedded `PointLight`s on 203 `Object3d` nodes**, against exactly one
  `Light` node per level (m1: 1 vs 304; m6: 1 vs 803), but *which* nodes
  carry them settles it (nodes.md). All 18 of m1's carriers are weapon and
  effect prototypes, 14 of them on `*.flt` roots outside the World tree, and
  they are not instanced (18 models, 18 referencing nodes). The `Light` node
  is a directional sun, which is what lights a level. See nodes.md,
  "Hierarchy, lights and transforms", for the per-carrier breakdown.
- **12-14 models per level have zero polygons, and all 148 across the 13
  levels carry lights.** They are pure light carriers referenced by real
  `Object3d` nodes, not corrupt entries: a mesh builder returns nothing for
  them and the node must still be spawned.
- **Levels are small by modern standards.** Fully instanced — every node
  drawing its model — m1 is **27 270 triangles** and the largest, m6, is
  **37 761**. Whatever the import costs, it will not be triangle throughput.
- **Polygon winding is CCW-outward in the shipped coordinates**
  (coordinates.md).

Confidence: verified (counts measured over all 13 levels and asserted in
`crates/bft-assets/tests/real_models.rs`).
