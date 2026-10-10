# Install layout and extraction

The retail install tree and which level is which, the pinned mech3ax commit, and the commands that extract the data.

## Install layout

```
<data root>/                       (~454 MB)
├── Recoil.exe, recoil3dfx.exe     (D3D and Glide executables)
├── messages.dll                   (localised strings; `unzbd rc messages`)
├── maps/m1.zmap … m12.zmap        (12 automaps; no m13.zmap)
├── zbd/
│   ├── image.zbd                  (15 MB; global image/texture archive)
│   ├── interp.zbd                 (119 KB; interpreter scripts)
│   ├── zrdr.zbd                   (776 KB; global reader data)
│   ├── soundsl.zbd / soundsm.zbd / soundsh.zbd   (17 / 29 / 35 MB; low/medium/high fidelity)
│   └── m1 … m13/
│       ├── gamez.zbd              (m1: 5.3 MB)
│       ├── anim.zbd               (m1: 1.1 MB)
│       ├── zrdr.zbd               (m1: 381 KB; per-mission reader data)
│       ├── rtexture2.zbd, rtexture4.zbd          (hardware textures, every mission)
│       └── texture2.zbd [, texture4, texture6, texture8]   (software textures, varies)
├── video/                         (83 MB; Indeo 5 AVIs: intro_01, m01_01 … m06_01, outro_01, zipperlogo, pumpa/b)
├── fonts/eurosti.ttf, eurostib.ttf
└── docs/                          (HTML online manual with GIFs)
```

Which of `m1`–`m13` are the six campaign missions and which are
multiplayer maps is answered below. The briefing videos are named
`m01`–`m06`, matching six campaigns. Per-mission
`zrdr.zbd` is 260–440 KB for `m1`–`m6` and 20–64 KB for `m7`–`m13`.

Not installed: the four CD audio tracks (3:35, 9:11, 8:47, 7:20) and the
PDF manuals.

Confidence: verified (from the installer cabinet).

## mech3ax pin

Repository: <https://github.com/TerranMechworks/mech3ax>.

```
commit cbb838fd8b628417abe61dee6132c350a2375f33
        ("Release candidate 0.7.0-rc3", 2025-11-17)
toolchain 1.91.1 (pinned by the repo's rust-toolchain.toml)
build   cargo build --release      # no warnings
binary  target/release/unzbd
```

**`unzbd rc anim` works for all 13 missions.** The README support matrix
still shows Recoil `anim.zbd` as unsupported, but that table is stale on this
commit: `crates/unzbd/src/commands.rs` dispatches `GameType::RC` to
`mech3ax_anim::rc::read_anim`, and `api-types`' `AnimMission` enum
enumerates `RcM01`…`RcM13` explicitly. Trust the code, not the matrix.

Every command succeeded: **118 invocations, 0 non-zero exits**, covering
interp, messages, the global reader, `image.zbd`, three sound archives, 12
zmaps, and per mission gamez + anim + reader + every texture archive
present. No file that `unzbd rc` supports failed to extract, and no error
text was produced by any command.

The only stderr output was non-fatal bounding-box diagnostics and two
duplicate anim references; all of it is catalogued in quirks.md. In particular
**m6 and m9 extract cleanly**: m9 produces no warnings at all beyond the
two anim-ref duplicates every mission emits, and m6's extra output is the
`EMPTY BBOX_MODEL` diagnostics described in quirks.md, not an error.

Confidence: verified.

## Extraction command table

Driver script: `tools/recon/extract.sh`, a loop over the table below that
logs each command's exit code and stderr to `<extract>/extract.log`. The
output root, `$BFT_EXTRACT_DIR`, mirrors the source tree, with per-mission
subdirectories. It reads `MECH3AX`, `BFT_DATA_DIR` and `BFT_EXTRACT_DIR`.

| Source | Command | Output |
|---|---|---|
| `zbd/interp.zbd` | `unzbd rc interp <in> <out>` | `interp.json` |
| `messages.dll` | `unzbd rc messages <in> <out>` | `messages.json` |
| `zbd/zrdr.zbd` | `unzbd rc reader <in> <out>` | `zrdr.zip` |
| `zbd/image.zbd` | `unzbd rc textures <in> <out>` | `image.zip` |
| `zbd/sounds{l,m,h}.zbd` | `unzbd rc sounds <in> <out>` | `sounds{l,m,h}.zip` |
| `maps/mN.zmap` | `unzbd rc zmap <in> <out>` | `maps/mN.json` |
| `zbd/mN/gamez.zbd` | `unzbd rc gamez <in> <out>` | `mN/gamez.zip` |
| `zbd/mN/anim.zbd` | `unzbd rc anim <in> <out>` | `mN/anim.zip` |
| `zbd/mN/zrdr.zbd` | `unzbd rc reader <in> <out>` | `mN/zrdr.zip` |
| `zbd/mN/{r,}textureN.zbd` | `unzbd rc textures <in> <out>` | `mN/<name>.zip` |

Notes for the extraction step:

- The game argument is always `rc`; the subcommand never varies by mission.
- `gamez.zip` contains `metadata.json`, `textures.json`, `materials.json`,
  `models.json`, `nodes.json`. m1's expands to ~20 MB of JSON from a 5.3 MB
  ZBD, so the cache must stay out of the repo and ideally be converted to a
  compact binary form at import time.
- `anim.zip` contains one JSON per anim def plus `metadata.json`, and
  `*.zan.json` files that are **SI scripts, not anim defs** (anims.md).
- Reader ZIPs contain one `<name>.zrd.json` per entry plus `manifest.json`.
- Texture ZIPs contain one PNG per texture plus `manifest.json`.
- `m13` has no zmap; the command is simply skipped (above).

Confidence: verified.

## REQUIREMENTS §9, question 1

Install layout: **answered** above. 13 mission dirs, 12 zmaps,
per-mission gamez/anim/zrdr/rtexture. Campaign-vs-multiplayer split:
**answered** — m1–m6 are the campaigns (six `campaign1`…`campaign6`
sounds, six `m01`–`m06` briefing videos, 55–92 AI path nets and 80+
enemy placements each, real objectives), m7–m13 the multiplayer maps
(`aiv.zrd` holds only `bft_00`, no path nets, pickups with respawn
timers, placeholder copies of m2's objectives/location files and of
m1's zmap — reader.md, quirks.md). Titles: `MSG_CAMPAIGN<N>_NAME` (messages.md). m9 is a
race map (`race.zrd`, start gates, laps strings), m13 a teleporter
arena; the others are deathmatch (8 spawn points in `net.zrd`).

Confidence: verified.
