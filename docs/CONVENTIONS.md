# Conventions and workspace

The workspace shape and the rules every change follows.

## Workspace

```
crates/
  bft-assets/   formats → engine types
  bft-sim/      simulation, headless-capable
  bft-game/     presentation
  bft-xr/       optional VR over OpenXR, layered on bft-game; only bft-bin
                depends on it, behind its `vr` feature (off by default)
  bft-bin/      binary `open-bft` with clap subcommands
assets/         own assets only: tuning/*.ron
docs/
  REQUIREMENTS.md, DESIGN.md, CONVENTIONS.md, READER-SCHEMA.md,
  SAVEGAME.md, OPEN-QUESTIONS.md, TODO-OBSERVATIONS.md
  data/         one file per data topic, indexed by data/README.md
  observations/ measurements of the original in play: plan, sessions,
                one file per topic, method in observations/README.md
  specs/        behaviour specs built on data, manual and observations
tools/recon/    the analysis scripts behind docs/data/
tools/observe/  the observation tools behind docs/observations/; their
                evidence (recordings, marks, frame grabs) lives outside
                the repository, in $OBFT_EVIDENCE_DIR
```

## Pinned dependencies (in `[workspace.dependencies]`, exact versions)

Verified on crates.io against Bevy 0.19.1; every entry is an `=` pin.

| Crate | Version | Used by | Why this one |
|---|---|---|---|
| bevy | =0.19.1 | all | `default-features = false` at the workspace level (see below) |
| avian3d | =0.7.0 | bft-sim | declares `bevy ^0.19`; 0.6.1 is the 0.18 line. `default-features = false` at the workspace level, like `bevy` |
| bevy_replicon | =0.44.1 | bft-sim, bft-game | declares `bevy ^0.19` |
| bevy_replicon_renet | =0.20.0 | bft-sim | the release that pairs with replicon 0.44; 0.19.0 still wants 0.43 |
| bevy_egui | =0.42.0 | bft-game (devtools) | declares the 0.19 subcrates |
| bevy_mod_openxr, bevy_mod_xr | =0.6.0 | bft-xr | declare `bevy ^0.19` and `wgpu ^29`. Not built on macOS |
| openxr | =0.21.1 | bft-xr | the version bevy_mod_openxr pulls in; default `loaded` feature opens the system's OpenXR loader at run time, so no SDK is needed to build |
| clap | =4.6.7, `derive` | bft-bin | latest 4.x; MSRV 1.85 |
| serde =1.0.229, serde_json =1.0.151, thiserror =2.0.20 | | | |
| ron | =0.12.2 | bft-assets, bft-sim | already in the lockfile via Bevy, so pinning it keeps one copy |
| zip | =8.6.0, `deflate` only, no defaults | bft-assets | the current stable (9.0 is pre-release). `default-features = false` drops aes/bzip2/lzma/zstd/xz |
| rand | =0.10.2 | bft-sim | matches the copy Bevy 0.19 already pulls in |
| rand_chacha | =0.10.0 | bft-sim | the sibling on `rand_core 0.10` |
| directories | =6.0.0 | bft-assets | current |
| anyhow | =1.0.104 | bft-bin only | current |

### `bft-sim` Bevy feature set

`bft-sim/Cargo.toml` depends on `bevy` with
`features = ["bevy_asset", "bevy_state", "bevy_log", "serialize"]`
plus whatever `avian3d` needs. It must **not** enable `bevy_render`,
`bevy_winit`, `bevy_audio`, `bevy_ui`. CI job `headless-sim` runs
`cargo check -p bft-sim` to prove it.

A workspace member may *add* features to an inherited dependency but never
remove them, so `default-features = false` on `bft-sim`'s `bevy` line is
rejected outright while the workspace entry has defaults on. The fix is to
put `default-features = false` on the **workspace** `bevy` entry and have
every consumer name what it needs: `bft-assets` takes `bevy_asset`,
`bft-sim` the four above, and `bft-game`/`bft-bin` spell out Bevy's own
defaults `["2d", "3d", "ui", "audio"]`. Any new crate that wants a renderer
must do the same.

### Plugin naming

Every crate exposes one top-level plugin: `AssetsPlugin`, `SimPlugin`,
`GamePlugin`, `VrPlugin`. Sub-plugins are private to the crate unless another crate
needs them. `bft-bin` adds `AssetsPlugin` + `SimPlugin` for `server`, and
all three for `play`/`view`.

### System sets (in `bft-sim::schedule`)

```rust
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum SimSet { Input, Vehicle, Weapons, Physics, Damage, Ai, Anim, Mission, Replicate }
```
configured in `FixedUpdate` in that chain order. `Time<Fixed>` is 60 Hz.

## Coding rules

- `cargo fmt`, `cargo clippy --all-targets -- -D warnings` clean.
- No `unwrap()`/`expect()` on data-derived values outside tests. Loaders
  return `Result<_, LoadError>` with the file and field in the message.
- Sim systems never read `Time<Real>` or `Time<Virtual>`; they read
  `Time<Fixed>` or nothing.
- No `f32` constants that are gameplay tuning in code. They go in
  `assets/tuning/*.ron` with a `// approximation` comment where guessed.
- Public items have a one-line doc comment saying what, not how.
- Module per subsystem, `mod.rs`-free (`foo.rs` + `foo/` when split).
- Tests: `#[test]` units next to code; data-backed tests use
  `bft_assets::test_support::data_root()` which returns `None` and the test
  returns early (not fails) when `BFT_DATA_DIR` is unset.
- Never add files under `assets/` that came from the game. `.gitignore`
  blocks `*.zbd`, `*.zmap`, `/extracted/`, `/cache/` and the cache's own
  extensions (`*.gamez.zip`, `*.tex.zip`, `*.anim.zip`, `*.rdr.zip`,
  `*.zmap.json`); extend it when the cache gains a new kind of file.
- **The copyright rule.** No file from the game, and nothing derived from
  one, is committed: the engine links to the user's install the way OpenRA
  does. Art, layouts, strings and sounds are read from the install through
  the extraction cache at run time. No PNGs cut from a texture archive, no
  layout numbers copied into code or a `.ron` (they are *parsed* from the
  file, never hard-coded from it), no screenshots of the running game in
  the repository or its tests. A test may assert what a parser finds in the
  real file, skipped without an install. Without an install, or when a
  piece is missing, fall back to our own work; nothing may panic.
  Documentation may quote the game's own wording in short excerpts
  (an option's label, a menu item, a one-line message) where it
  discusses that wording, citing the message key or reader control when
  there is one. No creative text (briefings, objectives, story, credits,
  manual passages beyond a sentence), no bulk lists of strings, and
  nothing in code or assets is taken from a quote: the engine shows the
  game's text only through its keys, read from the install.

## Git

- A branch per change. Squash-merge to `main`.
- Commit messages: imperative, first line ≤ 72 chars, body says why.
