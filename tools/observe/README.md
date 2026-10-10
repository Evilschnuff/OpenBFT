# Observation tools

These scripts turn recordings of the original game into the measurements
in `docs/observations/`. That README gives the method in plain words; this
one gives the commands.

The inputs are recordings and the game's data files. Reference sounds come
from the extraction cache, `$BFT_EXTRACT_DIR`. The tools never start, attach
to, read the memory of or modify the original game, and they never read its
program files.

**Evidence stays out of the repository.** Recordings, marks files, frame
grabs and the measurements taken from them all live in one evidence
directory, named by `$OBFT_EVIDENCE_DIR`:

```
$OBFT_EVIDENCE_DIR/
  <recordings>.mkv           anywhere below here; by convention in <session>/
  <session>/
    <session>.<recording>.<markset>.csv        marks
    <session>.<recording>.<markset>.grabs.sha256
    <recording>.grabs/       frame grabs
  scratch/                   onsets, pitch and changes output
```

The tools refuse an evidence directory inside the repository, and a
recording outside the evidence directory. Entries and session files cite
the evidence by SHA-256.

## Prerequisites

- Python 3.11 or later, plus `numpy`. Install it in a virtualenv outside the
  repository:
  ```sh
  python3 -m venv ~/.venvs/observe && ~/.venvs/observe/bin/pip install numpy
  ```
  `session new --no-probe`, `mark`, `check` and `result` (bar its two
  fits and audio events) run without it; `probe`, `onsets`, `pitch`, `changes` and a probing
  `session new` need it.
- `ffmpeg` and `ffprobe` 5.1 or later, and `mpv` 0.38 or later (for typed
  notes): `brew install ffmpeg mpv`, or your package manager.
- Optional: OpenTimestamps, `pip install opentimestamps-client`. If `ots`
  is on `PATH` (and the machine is online), session files, marks files and
  entries are timestamped as they are written. Each version of a file gets
  its own numbered proof, `<file>.1.ots`, `<file>.2.ots`, …. A version
  that already has one is not stamped again, and no proof is ever
  overwritten. Proofs hold only a hash and are committed. Those of session
  files and marks files go in `docs/observations/sessions/`, and those of
  entries beside them. Set `OBSERVE_NO_STAMP=1` to skip stamping.

## Recording

In OBS: the video at a **constant** 60 fps (OBS's default), MKV, lossless
(x264 CRF 0, or "Indistinguishable" at least) or a high bitrate. Record the
game's own sound as an audio track, and keep the microphone off it. Capture
the game window at its own size, or say in the session file how it was
scaled. When key presses matter, show them with an on-screen key overlay.
Put the recordings in the evidence directory.

## Commands

All of them go through `observe.py`; `--help` on any command lists its
options. Times are seconds from the start of the recording (`12.5` or
`1:02.5`), the clock mpv shows. Measurements go to
`$OBFT_EVIDENCE_DIR/scratch/` unless `--out` says otherwise.

```sh
OBS=tools/observe/observe.py
export OBFT_EVIDENCE_DIR=~/obft-evidence
```

### `session new` / `session add` / `session stamp`

```sh
E=$OBFT_EVIDENCE_DIR
python3 $OBS session new --rec $E/2026-10-07-a/craters-1.mkv --rec $E/2026-10-07-a/craters-2.mkv \
  --game-version "1.0 EA English" --os "Windows 98 SE in 86Box 4.2" \
  --resolution 640x480 --recorder "OBS 31, 60 fps CFR, x264 CRF 0" \
  --notes "3dfx renderer, sound high"
python3 $OBS session add 2026-10-07-a --rec $E/2026-10-07-a/briefing-1.mkv
python3 $OBS session stamp 2026-10-07-a
```

`session new` writes `docs/observations/sessions/<date>-a.md` (or `--id`)
from `templates/session.md`. It lists each recording's file name, size,
SHA-256, video and audio format, and the game's frame rate, which `probe`
measures (`--no-probe` skips that; `--game-fps` gives it). The thresholds
the measurement used go into the session file: `--pixel-threshold`
(default 6 grey levels), `--fraction` (default 0.002, i.e. 0.2 % of the
pixels) and `--min-rate` (default 10 new pictures/s). `session add` and
`probe` use the session's own thresholds and refuse others; a new
session can use new ones. Fields you don't pass read "(fill in)". Fill
them in and run `session stamp`, which stamps each new version. A session
file is never rewritten: `session add` only appends recordings, and
`probe --record` only fills in an offset that was not yet measured.

### `probe <rec>`

```sh
python3 $OBS probe $E/2026-10-07-a/craters-1.mkv
python3 $OBS probe $E/2026-10-07-a/craters-1.mkv --from 20 --to 30
python3 $OBS mark $E/2026-10-07-a/briefing-1.mkv --set sync
python3 $OBS probe $E/2026-10-07-a/briefing-1.mkv --sync-ref <sound file name> --record
```

`probe` prints the formats and the game's own frame rate. A recorded frame
counts as new if enough of its pixels differ from the frame before. Over a
stretch of motion, the share of new frames times 60 fps is the game's rate.
Without `--from`/`--to`, it takes the median over one-second windows that
have motion, and menus and pauses drop out. It also prints how many recorded
frames each picture stayed on screen. If every frame is new, the game may
run faster than the recording, and the rate is only a lower bound.

With `--sync-ref`, it measures the offset between audio and video. Mark
sync events with the `sync` markset: the first frame an event shows whose
sound is the reference, such as a shot's muzzle flash. `probe` finds the
reference's onset near each mark and prints audio time minus video time.
The `sync` marks come from the recording's marks files, or from
`--marks`. The offset's uncertainty is one game frame, or the spread of
the events if that is larger. `--record` writes the offset into the
session file, where `result` reads it.

### `mark <rec> --set <markset>`

```sh
python3 $OBS mark $E/2026-10-07-a/craters-1.mkv --set crater-drive
python3 $OBS mark $E/2026-10-07-a/craters-1.mkv --set crater-drive \
  --candidates $E/scratch/craters-1.changes.10_10_80_40.csv
```

This opens mpv paused, with `mark.lua`. The markset's keys each add a row
`frame,time,label,note` to
`$OBFT_EVIDENCE_DIR/<session>/<session>.<recording>.<markset>.csv`. The
session is the one whose table lists the recording's file name; pass
`--session` if more than one does. In mpv:

| Key | Does |
|---|---|
| `.` / `,` | one frame forward / back (mpv's own) |
| `SPACE`, arrows | play/pause, seek (mpv's own) |
| markset keys | mark the frame on screen; some ask for a note, some record the mouse position in recording pixels (`x=… y=…`), some save a frame grab |
| `!` | not observable: type what you tried that could not be seen |
| `BS` | void the last row: appends `void,"row N"` (nothing is deleted) |
| `TAB` / `Shift+TAB` | jump to the next / previous candidate (`--candidates`) |
| `F1` | list the keys |

The frame grabs go to `<session>/<recording>.grabs/` in the evidence
directory, or to `--grabs`; the tool refuses a directory inside the
repository. When mpv closes, each new grab's SHA-256 is appended to
`<marks>.grabs.sha256` beside the marks file, in `shasum -a 256 -c`
format. The entry lists the grabs with these hashes.

### `onsets <rec> --ref <wav>`

```sh
python3 $OBS onsets $E/2026-10-07-a/briefing-1.mkv --ref <sound file name>
python3 $OBS onsets $E/2026-10-07-a/craters-1.mkv --ref /path/to/sound.wav --threshold 0.6
```

This finds the times at which the reference plays, by normalised
cross-correlation. The confidence is the peak correlation: 1 means the same
waveform, and mixing with other sounds or 3D attenuation lowers it. A sound
played at another pitch is not found; use `pitch` for that. `--ref` takes a
WAV file, or a file name (with or without `.wav`) inside the cache's sound
archive (`sounds/soundsh.snd.zip`, or `soundsh.zip` from `tools/recon`). It
does not take a logical `snd_*` name from `sounds.zrd`: look that up
(`docs/data/sounds.md`) and pass the file name. The output CSV is
`time,frame,confidence`.

### `pitch <rec> --ref <wav> [--from --to]`

```sh
python3 $OBS pitch $E/2026-10-07-a/engine.mkv --ref <sound file name> --from 12 --to 20
python3 $OBS pitch $E/2026-10-07-a/engine.mkv --ref <sound file name> --peak 180 --speeds 0.8,1.5
```

This gives the playback speed of a known sound over time. It prints the
reference's strongest spectral peaks and follows one of them: the strongest,
or the one nearest `--peak`. It looks for the peak in the recording in
short windows, within the speed range `--speeds`. Speed is the recording's
frequency over the reference's. *Salience* is the peak's height above the
band's median, in dB, and a low salience means it is not the reference.
Output: `time,frame,freq_hz,speed,salience_db` as CSV, plus an SVG plot of
speed over time.

### `changes <rec> --roi x,y,w,h`

```sh
python3 $OBS changes $E/2026-10-07-a/canisters-1.mkv --roi 20,440,90,24 --threshold 3
```

This lists the frames where a screen region changes, by frame differencing
(mean absolute difference over the region's RGB values, 0–255). Changed
frames close together (`--gap`) count as one change. The output is a CSV of
candidate marks (`frame,time,label,note`) for `mark --candidates`;
`--trace` also writes every frame's difference.

### `result <item>`

```sh
python3 $OBS result craters-1
python3 $OBS result craters-1 --print   # show the entry, write nothing
```

This reads `items/<item>.toml` and the marks of every recording it lists,
taken in frame order.
It runs the analyses and writes the item's entry into
`docs/observations/<topic>.md`, between `observe:begin`/`observe:end`
comments. An entry that is already there is replaced; the rest of the file
is kept. Voided rows are shown struck through and left out of the analysis.
`not-observable` rows go in the entry's "Not observable" section.

On a wording mark, the entry shows the note's `key:`, else its `desc:`. It
quotes the wording only for a label in the item's `quote`, and otherwise
writes "(wording in the marks file)". This follows the copyright rule in
`docs/CONVENTIONS.md`. Next to each recording's hash, the entry records the
SHA-256 of the marks file it was computed from. It also names any
`<marks>.<n>.ots` proof whose stamped digest is that file's, so a stale
proof of an earlier version is not cited.

An item's `[[audio]]` blocks add a known sound's onsets as events. Each
recording is found in the evidence directory by name, size and SHA-256, and
the onsets are moved onto the video's clock by the recording's measured
audio − video offset. The entry states the offset and lists the events. If
no offset has been measured, `result` refuses.

A ratio whose numerator and denominator share marks is always
approximate. Its line says which marks are shared and that the ± treats
them as independent.

### `check`

```sh
python3 $OBS check
```

This loads every markset and item and reports problems: a reserved key, a
label the markset lacks, a malformed analysis.

## Markset files (`marksets/<set>.toml`)

```toml
name = "briefing"
description = "what the set is for"

[[key]]
key = "p"            # an mpv key name; not one of . , SPACE q Q arrows ESC ENTER BS TAB F1 ! [ ] { }
label = "proceed"    # lower-case words joined by '-'
help = "PROCEED is first visible"
note = false         # ask for a typed note
pos = false          # record the mouse position (for distances and slopes)
grab = false         # save the frame as a PNG outside the repository
wording = false      # the note records the game's wording (needs note = true)
```

A note is `;`-separated parts: free text, `key: …` and `desc: …`. On a
`wording` key the free text is the game's wording, exactly. Add `key:` with
the `MSG_*` key or `dialog.zrd` control when it can be told, else `desc:`
with what it is in your own words: `<the wording>; key: MSG_…` or
`<the wording>; desc: refuses to save while carried`. Entries never copy
a wording note whole (see `result` below).

## Item files (`items/<item>.toml`)

Each item has an id, topic, question and stated precision, both as in
`docs/observations/plan.md`. It also has a markset, a `method` (how to
record), optional `notes` and `pixel_error` (± px per point mark, default
2), and an optional `quote`. `quote` names the wording labels whose
wording the entry may quote: only for an item whose question is about the
wording, and only labels an analysis uses (`check` enforces the second).
Then come `recordings` (`[{ session = "…", file = "…" }]`, empty until
something is recorded), any `[[audio]]` blocks, and one or more
`[[analysis]]` blocks. An `[[audio]]` block has a `label` (not one of the
markset's), a `ref` (as for `onsets`) and an optional `threshold`; the
analyses use its label like a markset's. An analysis has a name,
a type, an optional `kind` (`exact`/`approximate`) and an optional `group`:
a label whose marks start a new group, named by their note.

| Type | Fields | Result |
|---|---|---|
| `count` | `labels` | how many marks of each label, per group and in total |
| `order` | `labels` (optional) | the sequence of marks per group, with notes; whether it is the same in all |
| `duration` | `from`, `to` | each `from` to the next `to`, in seconds ± one game frame; mean, range, sd |
| `ratio` | `numerator`, `denominator`, `unit`, `as_angle` | per group, with the uncertainties combined |
| `linear_fit` | `label`, `y`, `unit` | least-squares slope and intercept with standard errors, R² |
| `exp_tau` | `label`, `y`, `unit` | τ of y = a + b·exp(−t/τ), with the range where the fit stays within 1/(n−3) of its best |

A ratio's quantities are `{ distance = [a, b] }`, `{ dx = [a, b] }` or
`{ dy = [a, b] }` (between point marks, ± √2·`pixel_error`),
`{ duration = [a, b] }`, `{ value = label }` (the first number in the note)
or `{ count = label }`. For the fits, `y` is `value` (a number typed in the
note) or `x`/`y` (a point mark's position), and time runs from each group's
first mark.

The game's frame rate for the "± one game frame" comes from the session
file's recordings table. If no rate was measured, the session-wide value is
used, and failing that one recorded frame, and the entry says so.

## Tests

```sh
~/.venvs/observe/bin/python -m unittest discover -s tools/observe/tests -v
```

The tests make synthetic media with ffmpeg in a temp evidence directory
and check four things. A tone played back 1.25× fast must come out as 1.25
within 0.5 %. Two noise clicks must be found within one frame of where they
were placed. A colour change in a region must be found at its frame, while
one outside the region is ignored. A 30 fps picture inside a 60 fps file
must measure 30 fps, with its audio/video offset.

They also check:
- `result` on hand-written marks: wording, proofs and shared-point ratios;
- `result` with an audio event moved by a recorded offset;
- that the thresholds are recorded and kept per session;
- that proofs are numbered per version and never overwritten (with a stand-in `ots`);
- that evidence is refused inside the repository and outside the evidence directory;
- marking in a headless mpv;
- that the item files match `plan.md` word for word.

The media tests are skipped without ffmpeg and numpy, and the mpv test
without mpv.
