# open-bft

An open rebuild of Zipper Interactive's *Recoil* (1999) in Rust and
[Bevy](https://bevyengine.org), for Linux and macOS. The engine is original
code. The game's data is read from your own copy of the game.

**Status:** work in progress. Single-player is playable but incomplete, and
multiplayer is not implemented.
[docs/OPEN-QUESTIONS.md](docs/OPEN-QUESTIONS.md) lists what is missing.

## You need

A copy of *Recoil*: an installed copy, the disc (mounted, or copied to a
folder), or a disc image (`.nrg`, `.iso`, or `.bin` with its `.cue`).

## Setting up

```sh
./setup.sh ~/Downloads/Recoil.nrg   # or an .iso, a disc, an install
cargo run
```

`setup.sh` installs what is missing, asking first: Rust through
[rustup](https://rustup.rs), the pinned toolchain, and on Debian, Ubuntu,
Fedora and Arch the build packages and `unshield`, which unpacks the disc's
installer. On macOS it needs the Xcode Command Line Tools, and Homebrew for
`unshield`. Then it builds the game and runs `open-bft setup`, which:

- recognises what it is given, and without a path looks in Wine's default
  prefix, on media the desktop mounted, and for images in the current
  folder, your Downloads and your Desktop, asking before using what it
  finds;
- from the disc or an image, unpacks the installer into the project's data
  directory, and from an `.nrg` or `.bin` image also writes the music
  (below);
- remembers the install in the config file, builds
  [mech3ax](https://github.com/TerranMechworks/mech3ax)'s `unzbd` at the
  commit pinned in [docs/data/install.md](docs/data/install.md) if none is
  found, and extracts the game's data.

Running it again only does what is still missing. `--yes` answers every
question with yes.

To do it by hand instead: install rustup; on Linux a C toolchain and the
ALSA and udev headers (Debian and Ubuntu: `sudo apt install build-essential
pkg-config libasound2-dev libudev-dev`); on macOS the Xcode Command Line
Tools (`xcode-select --install`); `unshield` for the disc or an image. Then
run `cargo run -- setup <path>`. To use your own `unzbd`, pass `--unzbd`,
set `BFT_UNZBD`, or put it on `PATH`.

## Running

```sh
cargo run -- play m1      # start in a level, skipping the menu
cargo run -- view m1      # open a level in the free-fly viewer
cargo run -- --help       # every subcommand
```

The first build compiles Bevy and takes several minutes.

## Controls

| | |
|---|---|
| `WASD` | drive |
| Mouse | aim the turret |
| Left and right mouse buttons | fire |
| `1`–`9` | pick a weapon slot; with `Shift`, its strong variant |
| `F` | change form |
| `M` | map |
| `Esc` | pause |

Every binding can be changed in the settings. In the viewer: `WASD` move,
`Q`/`E` down and up, hold the right mouse button to look, `Shift` to go
faster.

## Music

The original's music is four CD-audio tracks on the game disc, which are not
part of the install. `setup` writes them out from an `.nrg` or `.bin`/`.cue`
image, which holds them; an `.iso` does not. Otherwise rip them yourself
(`.wav`, `.flac`, `.mp3` and Ogg Vorbis all play) and hand setup the folder,
`cargo run -- setup /path/to/recoil-music`, or set it in the config:

```ron
// config.ron in the platform config directory
(audio: (music_dir: Some("/path/to/recoil-music")))
```

Tracks play shuffled in game. A file named `menu.<ext>` or `track02.<ext>`
loops in the menus.

## VR

Optional, Linux only, built on demand:

```sh
cargo run --features vr -- play m1 --vr
```

It needs an OpenXR runtime (SteamVR, Monado, WiVRn) and the system's OpenXR
loader as `libopenxr_loader.so`, which on Debian and Ubuntu comes from
`libopenxr-dev`. Without them, `--vr` runs the flat game.

## Documentation

- [REQUIREMENTS](docs/REQUIREMENTS.md), [DESIGN](docs/DESIGN.md) and
  [CONVENTIONS](docs/CONVENTIONS.md): what the engine has to do, how it is
  structured, and the workspace, coding and git rules.
- [docs/data/](docs/data/README.md) and
  [READER-SCHEMA](docs/READER-SCHEMA.md): what the game's data looks like,
  measured rather than assumed.
- [docs/observations/](docs/observations/README.md) and
  [docs/specs/](docs/specs/README.md): what the original does in play, and
  the behaviour rules built on that.
- [OPEN-QUESTIONS](docs/OPEN-QUESTIONS.md) and
  [TODO-OBSERVATIONS](docs/TODO-OBSERVATIONS.md): what is not yet known or
  done, and where the engine differs from the original.
- [SAVEGAME](docs/SAVEGAME.md): the save game format.

## Licence

EUPL v1.2 — see [LICENSE](LICENSE).

## Credits and trademarks

Reading the game's formats relies on
[mech3ax](https://github.com/TerranMechworks/mech3ax) by the TerranMechworks
project (EUPL v1.2), which is run as a separate tool and not included here.

*Recoil* is a trademark of its respective owners. This project is not
affiliated with or endorsed by Zipper Interactive or Electronic Arts.
