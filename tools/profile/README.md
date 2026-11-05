# Profiling tools

Measure where the game's frame time goes, with the
[Tracy](https://github.com/wolfpld/tracy) profiler. With the `profile`
feature, every Bevy system, schedule and render step is a span; a capture
records them, and `analyse.py` turns a capture into a text summary. All
output stays in `target/`.

## Once: build Tracy's headless tools

```sh
cargo fetch                       # so the Tracy client's source is on disk
tools/profile/build-tracy.sh      # -> target/tracy/<version>/bin
```

The Tracy server must be the version the game's client speaks; the script
reads it from the `tracy-client-sys` crate pinned in `Cargo.lock` (Tracy
0.14.0 for Bevy 0.19.1). If you use the Tracy GUI as well, use the same
version.

## Each time

```sh
cargo run -p bft-bin --release --features vr,profile -- play m1 --vr   # terminal 1
tools/profile/capture.sh 30 table-m1                                   # terminal 2
python3 tools/profile/analyse.py target/profile/<file>.tracy
python3 tools/profile/analyse.py after.tracy --against before.tracy
```

Profile a `--release` build: the dev profile's `opt-level = 1` for our own
crates makes them look slower than they are. The Tracy client keeps
everything in memory until a capture connects, and the capture then
receives all of it, so a capture holds the whole run since start-up,
loading included. Don't leave a profile build running for long without
one.

`analyse.py` streams the export in two passes, so a capture of tens of
millions of spans takes about a minute and under 2 GB. Options:
`--from`/`--to` (seconds after the first frame: leave loading out, or
compare like with like), `--against` (an earlier capture), `--budget` (ms,
default 11.1 for 90 Hz), `--top`, `--worst` (slowest frames to break down),
`--filter` (regex on span names, e.g. `cut|cutaway`).

For a frame rate without profiling, any build takes `--log-fps`, which
prints frames per second and frame time once a second.

The scene moves the numbers more than most changes do (21–29 ms between
10-second stretches of one run), so compare runs from the same spot, doing
the same thing.

## Reading the summary

- A frame runs from one Bevy `main app` span to the next; its interval is
  what the headset sees. In VR, SteamVR paces frames, so intervals sit at
  the budget until work runs over.
- Spans are charged their self time (length minus nested spans), so a
  schedule and its systems aren't counted twice.
- Spans marked `*` mostly wait: the XR frame wait, presenting, and a
  schedule's own time (it waits there while its systems run on other
  threads). That time shrinks as work grows.
- Render-world spans run on the render thread alongside the next frame's
  main world (VR rendering is pipelined), so the two threads' totals
  overlap in time.
- These are CPU times, but SteamVR's `end_frame` waits for the GPU: at 50 %
  render resolution it fell from 3.8 to 1.8 ms.
