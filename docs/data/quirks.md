# Quirks

Every warning mech3ax emitted during extraction, and the inconsistencies between levels and between mech3ax's types and its output.

The 118 extraction commands produced no failure; all output below is
non-fatal diagnostics on stderr, in full.

## Bounding-box diagnostics (gamez only)

mech3ax recomputes node bounding boxes and reports whether its result
matches the stored one. `FAIL` means "recomputed value differs from the
file", not "extraction failed" — the data is passed through unchanged.

| Level | `LOD BBOX_NODE/CHILD FAIL` | `PASS` | `EMPTY BBOX_MODEL FAIL` | `EMPTY BBOX_MODEL PASS` |
|---|---|---|---|---|
| m1 | — | — | — | — |
| m2 | 16 | — | — | — |
| m3 | 10 | — | — | — |
| m4 | 28 | 5 | — | — |
| m5 | 15 | 1 | — | — |
| m6 | 35 | — | 198 | 89 |
| m7–m12 | 15 each | — | — | — |
| m13 | 25 | — | — | — |

Notes:

- **m1 produces no gamez warnings at all**, which is why it was chosen for
  the coordinate-system work in coordinates.md.
- **m6 is the only level with `EMPTY` diagnostics**, because it is the only
  level containing `Empty` nodes (287 of them, nodes.md). 198 of their model
  bounding boxes do not match, 89 do.
- The LOD bbox mismatches are consistent across every level and affect both
  `BBOX_NODE` and `BBOX_CHILD` equally (the counts are always identical),
  suggesting one systematic difference in how LOD bounds were authored
  rather than per-level corruption.
- Practical impact: **do not trust stored LOD/Empty bounding boxes.**
  The loaders should recompute bounds from geometry, which is what a Bevy
  importer would do anyway.

## Duplicate anim references

Every one of the 13 missions emits exactly these two lines, and nothing
else, during `unzbd rc anim`:

```
anim def duplicate anim ref `boat_wake`.`left_wake`
anim def duplicate anim ref `bft_exhaust`.`left_exhaust`
```

Both concern the player vehicle's own effects (wake and exhaust), are
identical in every mission, and are clearly authoring duplicates in the
shared vehicle anim set rather than level data problems. The anim loader
should de-duplicate anim refs by name and expect no others.

## Inconsistencies between levels

- **m7–m12's zmaps are byte-identical copies of `m1.zmap`** (verified by
  checksum: one hash shared by m1, m7, m8, m9, m10, m11 and m12). The
  multiplayer maps ship m1's automap as a placeholder; their real layout is
  not m1's. **Multiplayer maps therefore have no usable automap**, and m13
  has no zmap file at all. The map screen must handle both cases.
- m6 is the outlier in every structural respect: only level with `Empty`
  nodes, most nodes (4955), most anim defs (838), only level with a
  `texture8.zbd`, and the only one with `EMPTY` bbox diagnostics.
- `virtual_partition` is null on every node of every level, and
  `active_bbox` is `"Node"` on every node of every level — two fields that
  are present in the format but unused by this game's data.
- Multiplayer maps m7–m13 have a near-constant 31 LOD nodes and ~57 pickup
  nodes each, consistent with a shared multiplayer template.

## mech3ax API vs. actual JSON

`crates/api-types/src/nodes/rc.rs` describes `NodeRc` as a tagged enum with
per-variant `name`/`flags`/`zone_id`. **The JSON this commit actually emits
is a flat node struct with the variant under `data`** (nodes.md). The same holds
for models (models.md): `ModelFlags`/`PolygonFlags` are declared with
mech3ax's `bit!` macro but **emitted as objects of booleans**, with the
`skip_serializing_if` members absent entirely when false. Similarly the
README support matrix lists RC `anim.zbd` as unsupported while the code
supports it (install.md). Read the emitted JSON, not the type definitions or the
README, when writing the loaders.

Confidence: verified.
