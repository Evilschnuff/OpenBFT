# Coordinate system

Up axis, unit scale, handedness and the transform forms of the gamez data.

Method: `tools/recon/plot_level.py` walks m1's node tree, composes each
node's transform with its parents', transforms that node's model vertices to
world space (25 449 instanced vertices from 18 346 unique ones across 1 350
models), and plots a top-down scatter with the `m1.zmap` features overlaid
plus a side view. `--permute` scores all six assignments of world components
to plot axes by how well the zmap agrees with the geometry. The plot is
written into the extraction directory, outside the repo.

**Up axis: `y`. Ground plane: `x`/`z`.** Four independent confirmations:

1. Permutation scores: the x/z plane scores 0.078, both y-containing planes
   score 0.033 and 0.024. x/z wins.
2. The zmap header's `min`/`max` have **`y` exactly 0 in both** for every
   mission, varying only in x and z (m1: 0…2768.8 × 0…3423.5). Feature
   vertices do carry a `y`, but only as a small elevation — m1's spans
   0…99.6 against x 1287…2769 and z 1201…3424. So the automap's extent is
   an x/z projection and y is the (much shorter) height axis.
3. The `World` node's `area` is `left/top/right/bottom` = 0…4096 × 0…4352,
   and its partition entries are keyed by `x` and `z` with `min`/`max`
   carrying the y (height) range. The world is partitioned in x/z.
4. Local-space extents of known objects: `palm02` (a palm tree) is
   dx=7.3, **dy=12.2**, dz=0.0 — tall in y, a flat facade; `docksnd` (a
   dock) is dx=93.0, **dy=2.0**, dz=143.6 — flat in y, wide in x/z.

In the plot the red zmap coastline traces the dense geometry cluster and
the blue zmap bounding box brackets it; the side view shows y confined to
roughly −220…758 while x and z span thousands of units.

**Unit scale: 1 unit = 1 metre.** The player vehicle subtree (`bft_00`,
821 vertices) measures 5.66 × 3.30 × 7.40 units — a 5.7 m wide, 7.4 m long,
3.3 m tall hovercraft. A palm tree is 12.2 units. Fog range is 550…650
units, a plausible metre draw distance.

**Handedness: right-handed, no axis negation, no winding reversal.**
Measured two ways over the shipped data:

1. **Signed volume** (divergence theorem) of every *closed* `Default` model
   in m1/m6/m7/m13 — **371 of 373 are positive**, i.e. the polygon winding
   is counter-clockwise seen from outside under a **right-handed** cross
   product. (The 2 negatives look like authoring slips in the original
   data.)
2. **Stored normals**: the face normal from that same winding versus the
   averaged `normal_indices` normals, over every polygon that has them in
   all 13 levels — **2108 agree, 12 disagree**.

So the vertex data as stored is already consistent with Bevy's right-handed,
CCW-front convention, and `AxisConvention::GAMEZ_DEFAULT` is the identity.
Negating z and reversing the winding would turn every closed model inside
out. Whatever the original D3D runtime
did with these numbers, the *extracted* data needs no handedness change.

Both measurements are kept as assertions in
`crates/bft-assets/tests/real_models.rs`.

**Transforms.** A node's transform lives at `data.Object3d.transform` and is
one of three forms, counted over m1's 4 041 Object3d nodes:

| Form | Count | Meaning |
|---|---|---|
| `"Initial"` | 2 732 | identity |
| `{"Matrix": {r00…r32}}` | 1 307 | 3×3 rotation applied to row vectors (nodes.md), `r30`–`r32` translation |
| `{"RotateTranslateScale": …}` | 2 | Euler rotation, translation, scale |

The `Matrix` form is a 3×4 affine transform, not a rotation plus a separate
translation field. **Euler order is XYZ** (rotation applied X, then Y,
then Z, matching field order), in **radians** — m1's one non-trivial case is
`bft_00` at `rotate.y = -1.2217305` rad = −70.0°, a clean round figure in
degrees, which corroborates radians. Only two nodes per level use this
form; the order is confirmed where it matters, on the anim VM's
`ObjectRotateState` events (anims.md, "Cinematics").

`matrix_signs` from the api-types documentation appears here as
`data.Object3d.signs` (m1's first Object3d has `signs = 128`); its meaning
is **unknown** and was not needed for the plot.

Confidence: verified.

## REQUIREMENTS §9, question 2

Coordinate system: **answered** above. Y up, X/Z ground plane,
1 unit = 1 metre, right-handed with no winding correction, gamez rotations
in radians (Euler order XYZ), reader headings in degrees.

Confidence: verified.
