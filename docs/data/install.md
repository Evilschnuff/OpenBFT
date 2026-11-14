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

## The disc and its installer cabinet

What `open-bft setup` reads when given the disc instead of an install.
Measured on a Nero image (`.nrg`, footer version 2) of the retail disc.

**Tracks.** Five, from the image's `DAOX` chunk: track 1 is data (ISO
9660, 2048-byte sectors, 216,497 sectors; volume label `RECOIL`), tracks
2–5 are CD audio (2352-byte sectors) of 3:35, 9:11, 8:47 and 7:20, the
four music tracks above. The image stores the audio little-endian, as a
BIN rip does: read that way, the mean step between left-channel samples is
0.056 of the mean level over one second of track 2, against 1.21 read
big-endian.

**The data track is shorter than its file system says.** The primary
volume descriptor gives 216,801 sectors, 304 more than the track holds.
One file reaches past the end: `SETUP1.WAV` (the installer's sound, from
sector 216,431 to 216,646), so it is cut short in the image. Nothing setup
needs is affected, but a reader must report a cut file rather than fail on
it.

**Disc root.** InstallShield 5 setup (`SETUP.EXE`, `SETUP.INS`,
`_SYS1.CAB`, `_USER1.CAB`, `LAYOUT.BIN`) and one data cabinet, `DATA1.CAB`
(245,410,644 bytes). There is no `DATA1.HDR`: the cabinet carries its own
header. A Joliet reader sees the name in lower case.

**The cabinet.** `unshield` reads it with no options: 13 components, 13
file groups, 220 files. Every path inside a group is already relative to
the install root, except the sound and font groups:

| File group | Goes to | Holds |
|---|---|---|
| Game Files | `.` | the executables, `messages.dll`, `maps/`, the global and per-mission `zbd/` archives, `docs/` |
| Hardware Textures | `.` | `zbd/mN/rtexture2.zbd`, `rtexture4.zbd` |
| Software Textures | `.` | `zbd/mN/texture4.zbd` and some `texture6`/`texture8` |
| Software Textures Minimum | `.` | `zbd/mN/texture2.zbd` |
| AVI Files | `.` | `video/*.avi` |
| Sound Low Fidelity | `zbd/` | `soundsl.zbd` |
| Sound Medium Fidelity | `zbd/` | `soundsm.zbd` |
| Sound High Fidelity | `zbd/` | `soundsh.zbd` |
| Font Files | `fonts/` | `eurosti.ttf`, `eurostib.ttf` |
| Uninstal Files, A3D DLLs, Indeo 5 codec, Msvc DLLs | not unpacked | Windows system files, outside the install tree |

Unpacking every group but the last row gives the tree under "Install
layout". `unshield -d <dir> -g <group> x` writes a group's files under
`<dir>/<group>`, with the spaces in the group's name replaced by
underscores (`Game_Files`). The disc's own `VIDEO` directory is not needed: the cabinet's
AVI Files group holds the videos.

Confidence: verified (`unshield c`, `g` and `l` on the cabinet; the
track table and ISO directory read from the image).

## mech3ax pin

Repository: <https://github.com/TerranMechworks/mech3ax>.

```
commit cbb838fd8b628417abe61dee6132c350a2375f33
        ("Release candidate 0.7.0-rc3", 2025-11-17)
toolchain 1.91.1 (pinned by the repo's rust-toolchain.toml)
build   cargo build --release      # no warnings
binary  target/release/unzbd
```

`open-bft setup` builds the same commit with `cargo install --locked` into
the platform data directory when no `unzbd` is found. The commit is also
`MECH3AX_COMMIT` in `bft-assets`' `extract.rs`, and a unit test fails if
the two differ. It builds with the workspace's 1.98.1 toolchain; mech3ax
only emits dead-code warnings.

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
