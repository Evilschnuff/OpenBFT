# open-bft

An open rebuild of Zipper Interactive's *Recoil* (1999), in Rust + [Bevy](https://bevyengine.org)
(rendering via wgpu → Metal on macOS, Vulkan on Linux).

The documentation is in `docs/`:

- [docs/REQUIREMENTS.md](docs/REQUIREMENTS.md) — what the engine has to do and why.
- [docs/DESIGN.md](docs/DESIGN.md) — how it is structured.
- [docs/CONVENTIONS.md](docs/CONVENTIONS.md) — workspace, dependency pins,
  coding and git rules.
- [docs/data/README.md](docs/data/README.md) — what the real Recoil data
  looks like, measured rather than assumed.
- [docs/READER-SCHEMA.md](docs/READER-SCHEMA.md) — the shape and meaning of
  the reader (`zrdr.zbd`) entries.
- [docs/SAVEGAME.md](docs/SAVEGAME.md) — the save game format.
- [docs/OPEN-QUESTIONS.md](docs/OPEN-QUESTIONS.md) — what is not yet known
  about the original game, and what the engine does not yet do.
- [docs/TODO-OBSERVATIONS.md](docs/TODO-OBSERVATIONS.md) — where play shows
  the engine differing from the original.
- [docs/observations/README.md](docs/observations/README.md) — what the
  original does as it runs, measured from recordings of play, and the
  [plan](docs/observations/plan.md) of what to observe next.
- [docs/specs/README.md](docs/specs/README.md) — behaviour specs: what the
  game does, rule by rule, with the data, manual pages and observations
  each rule rests on.

## Requirements

- [rustup](https://rustup.rs). The toolchain version is pinned in
  `rust-toolchain.toml` and rustup installs it automatically on first build.
  Do **not** install Rust via Homebrew — it ignores the pin and shadows rustup
  on `PATH`.
- macOS: Xcode Command Line Tools (`xcode-select --install`).

## Running

```sh
cargo run                 # play (the default)
cargo run -- view m1      # open a level in the free-fly viewer
cargo run -- server       # headless dedicated server, no window or GPU
cargo run -- --help       # every subcommand
cargo run --features vr -- play m1 --vr   # in a VR headset (Linux, OpenXR)
```

VR is optional and off by default: without `--features vr` none of it is
built. It needs an OpenXR runtime (SteamVR, Monado, WiVRn) and the system's
OpenXR loader under its unversioned name `libopenxr_loader.so`, which on
Debian/Ubuntu comes from `libopenxr-dev` (`libopenxr-loader1` alone ships only
`.so.1`); with neither answering, `--vr` runs the flat game.

The first build compiles Bevy's full dependency tree and takes several minutes.
Later builds are incremental. Dependencies are built optimised even in debug
(see `[profile.dev.package."*"]`), so the game runs at a usable framerate while
our own crates stay fast to recompile.

Controls in game: `WASD` drive, the mouse aims the turret, the left and right
mouse buttons fire, `1`–`9` pick a weapon slot and `Shift` its strong
variant, `F` changes form, `M` shows the map, `Esc` pauses. Every binding can
be changed in the settings. In the viewer: `WASD` move, `Q`/`E` down/up, hold
the right mouse button to look, `Shift` to go faster.

## Layout

A cargo workspace:

- `crates/bft-assets` — mech3ax JSON/PNG → Bevy assets. Isolated deliberately:
  it is the piece coupled to mech3ax's unstable output format, so churn hits
  one crate.
- `crates/bft-sim` — fixed-timestep simulation at 60 Hz: physics, vehicles,
  weapons, damage, AI, missions, anim VM. Builds with Bevy's render, window,
  audio and UI features **off**, so a dedicated server needs neither a GPU nor
  a window; CI's `headless-sim` job enforces it.
- `crates/bft-game` — presentation: level rendering, camera, HUD, menus,
  audio, dev tools and the level viewer.
- `crates/bft-bin` — thin binary; builds `open-bft` and parses its subcommands.
- `assets/` — **our own assets only**, including `assets/tuning/*.ron`, an
  override layer on top of the game's own reader data (see
  [assets/tuning/README.md](assets/tuning/README.md)).
- `docs/` — requirements, design, conventions, data notes, the reader schema,
  the save format, the open questions, observations and specs (above).
- `tools/recon/` — the analysis scripts behind `docs/data/`.
- `tools/observe/` — the tools that turn recordings of the original into the
  entries in `docs/observations/`. Recordings, marks and frame grabs stay
  outside the repository, in `$OBFT_EVIDENCE_DIR`.

Dependencies are pinned to exact versions in `[workspace.dependencies]`: Bevy
is pre-1.0 and mech3ax's output format is explicitly unstable, so upgrades are
deliberate. Note that `bevy` is inherited with `default-features = false` and
each crate names the features it needs — that is what lets `bft-sim` drop the
renderer, since a workspace member can add features but never remove them.

## Game assets

No game data is committed to this repo, ever — no textures, models, audio,
level files, and no test fixtures carved out of them. The engine is original
code; the copyrighted data stays with whoever owns the game.

Assets are extracted from your own copy of Recoil with
[mech3ax](https://github.com/TerranMechworks/mech3ax)'s `unzbd` CLI, which
outputs JSON + PNG, and read from a user-supplied install directory at runtime.

### Music

The original's music is four CD-audio tracks on the game disc. They are not
part of the install and cannot be extracted with the rest: **rip them
yourself** (any CD ripper; `.wav`, `.flac`, `.mp3` and Ogg Vorbis all play)
and point the config at the directory:

```ron
// config.ron in the platform config directory
(audio: (music_dir: Some("/path/to/recoil-music")))
```

Tracks play shuffled in game. A file named `menu.<ext>` (or `track02.<ext>`,
the CD's first audio track) loops in the menus instead; without one the menus
are quiet. The same `audio` block holds
`master`, `effects` and `music` volumes (0–1) and `muted`.

## Licence

EUPL v1.2 — see [LICENSE](LICENSE).

## Credits and trademarks

Reading the game's formats relies on
[mech3ax](https://github.com/TerranMechworks/mech3ax) by the TerranMechworks
project (EUPL v1.2), which is run as a separate tool and not included here.

*Recoil* is a trademark of its respective owners. This project is not
affiliated with or endorsed by Zipper Interactive or Electronic Arts; the
names are used only to say which game it works with.
