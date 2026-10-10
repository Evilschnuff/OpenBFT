# Sounds

The three sound archives, their naming, and how sound names resolve through `sounds.zrd`.

Source: the sound summary written by `tools/recon/summarize.py`.

The three archives hold **exactly the same 280 file names** — 280 shared,
zero unique to any archive. They are pure quality tiers of one sound set,
selected by the installer, and all are mono PCM WAV (`manifest.json` plus
one `.wav` per entry):

| Archive | Files | Uncompressed | Dominant format | Mean size vs `h` |
|---|---|---|---|---|
| `soundsl.zbd` | 280 | 17.4 MB | 11025 Hz, 8-bit mono (271) | 0.41 |
| `soundsm.zbd` | 280 | 30.6 MB | 22050 Hz, 8-bit mono (214) | 0.74 |
| `soundsh.zbd` | 280 | 35.0 MB | 22050 Hz, 16-bit mono (131) | 1.00 |

The tiers are not perfectly uniform — a handful of files are 44100 Hz/16-bit
in every archive, and `soundsh` keeps 86 files at 8-bit — but the pattern is
low = half rate, medium = full rate 8-bit, high = full rate 16-bit.
**The engine reads `soundsh` only**; the other two carry no unique
content.

Naming convention, by prefix frequency:

- `m*` (51 files) — mission speech and mission-specific cues.
- `campaign1`…`campaign6` — one per campaign mission, matching the six
  briefing videos (install.md) and confirming six campaigns.
- Weapon/effect families: `arc*` (6: `arc01`…`arc05`, `arc_fire1`,
  `arcon`, `arcstart`), `expl*` (8), `exp_hit*` (4), `sonic*` (2),
  `elect*` (2), `ric_sizzle*` (3).
- Enemy/vehicle: `enemy_engine*` (10), `chopper`, `novtol*` (2),
  `morph*` (2) — the form-change sound.
- Environment: `hydro*` (5), `steam*` (4), `watfal*` (2), `splashbg*` (2),
  `bncewter` (bounce on water) vs `bncehard`, `collaps*` (3), `stone*` (2).
- UI/feedback: `warn*` (3), `ammo_out`, `rollover`, `comp1`, `charge`.
- Damage/death: `deadscr1`…`deadscr4`, `deadguy`, `dmsplat*` (2),
  `brain*` (3).

Sounds are referenced by bare name (no extension) from anim `Sound` events
(anims.md) and reader data, e.g. `snd_review`, `snd_m1obj1` in
`m1/objectives.zrd` — note those reader names are **not** file names in the
archives, so there is an indirection: the reader sound table `sounds.zrd`
(READER-SCHEMA §2.5).

Confidence: verified (counts, formats, shared names); inferred (the meaning
of the name families).

## The sound table and what plays

Asserted in `crates/bft-assets/tests/real_sounds.rs` and
`crates/bft-game/tests/real_audio.rs`.

- **The indirection above is closed by `sounds.zrd`** (READER-SCHEMA §2.5:
  every keyword is followed by its own value). 280 records
  in 16 sets, **272 distinct logical names**: `snd_beamdown`, `snd_frcgen`,
  `snd_frcfield`, `snd_novtol`, `snd_minewat` and `snd_stone` repeat across
  mission sets, always naming the same file. (V)
- **Flag counts over the 280 records**: `3D` 181, `RANGE` 139, `PURGEABLE`
  66, `LOOPED` 51, `VOICE` 40, `FREQUENCY` 15, `VOLUME` 6, `HARDWARE` 5.
  Ranges run from 10 m to 2000 m (`snd_explring`). **One is reversed**:
  `snd_gunside` is `[50, 20]`; the parser orders it. (V)
- **15 of the 280 WAVs are named by no record**: `arc01-1`, `brain1-1`/`-2`,
  `elect1-1`, `frcfield-1`/`-2`, `frcgen-1`/`-2`, `hydro4-1`, `novtol-1`,
  `splashbg-1`, `steam1-1`, `steam2-1`, `stone-1`, `watfal1-1`. Every one is
  a `-1`/`-2` variant of a named file. Whether the original picked them at
  random is unknown; nothing plays them. (V for the list, U for the use)
- **Every WAV in `soundsh` is mono integer PCM** (8- or 16-bit, 11025–44100
  Hz, including odd rates 22000 and 22040), which Bevy's `wav` decoder plays
  unconverted. (V)
- **Every sound name the game uses resolves.** All 3093 anim `Sound` events
  and 149 `SoundNode` events across the 13 levels, and every sound in
  `weapons.zrd`, `pickup.zrd` and `vehicle.zrd`, name a record whose file is
  in the archive. (V)
- **Anim sound events** carry `at_node {name, pos}` (`Sound`, 69 with null,
  97 with a non-zero offset) or `translate {AtNode {name, pos}}`
  (`SoundNode`, 56 of 149). `SoundNode.active_state` is `true` in all 149,
  and every `SoundNode` sound but `snd_m1intro` is `LOOPED`: it starts a
  node-bound loop and nothing in the data stops one, so the engine keeps it
  for as long as the node exists and mutes it while the node is inactive. (V
  for the shapes, I for the stop rule)
- **No anim event is a puffer**: none of the 53 167 events is a
  `PufferState`. Effects arrive as `Effect {name, at_node}` (2299), naming
  nine of the 13 `effects.zrd` sprite cycles: `exp_crate` 1145, `fire1` 366,
  `videxp` 266, `exp_bunk` 241, `smoke1` 209, `exp_box` 29, `exp_fireup` 26,
  `watblast` 14, `splash_dam` 3. `fire1` is the one with `LOOPING ON`. (V)
- **The form-transition animations carry no sounds**, so the morph sounds
  (`snd_morphup`, `snd_morphdown`, `snd_vehamp_morph`, `snd_vehsub_morph`)
  are chosen by `assets/tuning/sounds.ron`, keyed by target form. (V for the
  absence, I for the mapping)

Confidence: verified except where marked.

## REQUIREMENTS §9, question 8

Sound naming: **answered** above + READER-SCHEMA §2.5: `sounds.zrd`
maps `snd_*` logical names to `.wav` names with 3D/range flags.

Confidence: verified.
