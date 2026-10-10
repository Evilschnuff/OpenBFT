# Materials and textures

Materials, soil, cycled textures and the texture archives, and how gamez texture names resolve.

Source: the texture summary written by `tools/recon/summarize.py`.

## Counts

m1 references 471 textures and defines 512 materials (465 `Textured`,
47 `Colored`); m2 is the largest at 632/673. Multiplayer maps are about a
third of that (m11: 194/222). The `Colored` count is ~47 in every single
level, suggesting a shared palette of untextured materials.

## Soil

`Soil` is almost never set: m1 has 510 `Default`, 1 `Water`, 1 `Fire`.
Across all levels only `Water` (1–6 per level), `Lava` (1–8), `Fire` (1) and
one `Quicksand` (m9) ever occur. `Seafloor`, `Dirt`, `Mud`, `Grass`,
`Concrete`, `Snow`, `Mech`, `Silt` and `NoSlip` are **never used**.

The material count is misleading, though (water.md). The few
`Water`/`Lava`/`Quicksand` materials are applied to every water/lava polygon
(1–2 % of a level), and the reader keys gravity, slowdowns and weapon
impacts on soil. **Soil is the water/lava mechanism; node names are
secondary** (water.md).

## Cycled textures

**Zero materials have a `cycle` in any level.** Yet interp calls
`CycleTextureSetMap` 487 times, and `CycleTextureSetOn`/`SetSpeed` 71 times
and `SetLooping` 70 times (interp.md). Texture cycling is therefore configured
**at runtime by interp**, not baked into gamez. For animated textures (water surfaces, screens), interp is the only source.

Wired up in the engine, m1's runtime script binds **84 texture cycles and
18 scrolls** onto nodes that exist in the level. `surf1` is a 13-frame loop
at 6 fps.

**Interp is not the only source: `effects.zrd` cycles by texture.** Its 13
records (READER-SCHEMA §2.7) each open on a first frame, and the original
runs the cycle wherever a material shows that frame, not on a named node.
The burning wrecks prove it: `fire_hulk.flt/fire1` and `burning_model/fire1`
show `fire101.tif`, `fire1`'s first frame, and
`smoke_destroy.flt/blacksmoke_face` shows `bsmok1.tif`, `smoke1`'s; no
interp binding names any of them, yet `smoke_destroy` rewinds its cycle with
`ObjectCycleTexture`. Without the table those sprites would stand on frame
one for their whole life (80 s for `fire_40`, the hulk fire). Two first
frames repeat (`boxexp01.tif` opens `bftmbrst` and `exp_box`; the two
splashes are one list under two names), and all 13 cycles' frames resolve
through the cross-mission pool. `fire1` is the only `LOOPING ON`. (V,
`real_textures.rs`.)

Interp's by-name bindings also reach nodes *cloned* after load: the
drone's `drone_fire` cycle is bound by name, and every drone is a clone of
the `dron` prototype, so a clone takes the binding its name carries.

## Texture archives

Each mission ships the same texture set at several resolutions:

| Archive | m1 count | Palette | Note |
|---|---|---|---|
| `rtexture2` | 576 | `None` (RGBA) | hardware, half-size mips |
| `rtexture4` | 576 | `None` (RGBA) | hardware, full size |
| `texture2/4/6[/8]` | 576 | `Global` (569 of 576) | software renderer, palettised |

**`rtexture2` and `rtexture4` hold exactly the same 576 names** — 576
shared, 0 unique to either. They are not a partition but a **resolution
pair**: the same texture at 32×32 in `rtexture2` and 64×64 in `rtexture4`
(m1's most common sizes shift from 32×32 ×255 to 64×64 ×170). The engine
loads `rtexture4` and treat `rtexture2` as the low-memory tier.
The `texture*` family is the software renderer's palettised equivalent and
can be ignored.

Alpha modes (m1, identical across all its archives): `None` 374,
`Full` 202, `Simple` 0. `Simple` appears only in `image.zbd` (17 of 433).
So level textures are either opaque or have a full alpha channel;
**one-bit cutout alpha is not used at level scope**.

## Do gamez names match ZIP entries?

**Not exactly — an extension must be stripped.** `gamez`'s `textures.json`
stores source names like `dockp01k.tif` (and some without extension, like
`lflare1`), while the texture archives store bare stems (`dockp01k`).
After stripping the extension, **444 of m1's 471 references resolve** in
`rtexture2`/`rtexture4`, and 132 archive textures go unreferenced.

The 27 unresolved references are the interesting part. They are effect and
HUD textures — `ammolaser`, `ammoarcgun`, `ammosonic`, `amphchip`,
`hovchp01`, `subchp01`, `lockon01…13`, `watexp01`, `bubble`, `turb01`,
`ash_dirt`, `lav_dirt`, `snow_dirt`, `sulph_dirt`. **None of them is in
`image.zbd` either**, but every one is present in *other missions'*
`rtexture*` archives (e.g. `ammolaser` is in m2–m13, `watexp01` in
m4/m5/m6/m11/m13).

So: **texture lookup needs a cross-mission texture pool, not per-mission
isolation.** A texture reference that misses the current mission's archive
must fall back to a global pool assembled from the other missions (and
`image.zbd`). This is a real constraint on the asset loader's design and
is recorded in the catalog (`texture_pool`).

`image.zbd` (433 textures) holds UI and font assets — `lucida_console_*`
glyph sheets, `back_m7`…`back_m12` menu backgrounds — at irregular sizes
(100×100, 157×40, 84×768). It is the UI archive; level textures do not
come from it.

**Fog ranges vary enough to matter to a camera.** m1 550–650, m2/m7
650–825, m5/m11 320–390, m13 800–900, with fog colours from near-white
(0.92 grey, the snow levels) to near-black (m13). A viewer or chase camera
that stands off by a fixed distance is inside its own fog on the
short-range levels; the standoff has to be read from `fog_range.0`.

## The texture archives and material JSON

- **Manifest shape.** `manifest.json` at the ZIP root:
  `{ "texture_infos": [...], "global_palettes": [...] }`, one `<name>.png`
  per texture also at the root. Enums serialise as **strings**
  (`"alpha": "Full"`, `"soil": "Water"`, `"stretch": "Horizontal"`), not
  integers, despite being mech3ax `num!` enums.
- **De-palettisation confirmed.** `"palette": "None"` on all 576 `rtexture*`
  entries and all 433 `image.zbd` entries, so `global_palettes` is never
  needed and the loader does not mirror it.
- **`TextureStretch` is not a size correction.** 43 of m1's 576 are
  non-`None` (`Both` 22, `Horizontal` 20, `Vertical` 1), but mech3ax emits
  **every** PNG at its declared `width`×`height` — checked on all 576. It is
  a flag for the 1999 hardware, and applying it as a UV scale would
  double-count what the geometry's UVs already encode. Recorded, not applied.
- **`materials.json` stores `texture_index`, not a name**, indexing the same
  `gamez`'s `textures.json`. `ColoredMaterial.color` components are **0–255
  floats**, not 0–1 (maximum across all 13 levels is exactly 255.0).
- **`flag` is `false` on all 4649 textured materials** in all 13 levels, and
  `cycle` is null on all of them. Both claims above hold across the whole game,
  not just m1; `flag`'s meaning is unlearnable from this data.
- **One genuinely dangling texture reference.** m9's `textures.json` names
  `surf00.tif`, which is in **no** archive of any mission (the surf cycle
  ships as `surf01`–`surf13`, and `surf00` is absent from the software
  `texture*` archives too). Every other reference in every level resolves
  through the cross-mission pool. The texture and level loaders treat a
  missing texture as a warning, not an error, for this one polygon's sake.
- **Interp cycle frames resolve too, bar six.** Of the 304 distinct
  `CycleTextureSetMap` names, 298 are in the pool; the six `spray01`–`spray06`
  are in no archive, the same class of dangling reference as `surf00`.

Confidence: verified.
