# Session $id

- Date: $date
- Game version: $game_version
- OS/VM: $os
- Game resolution: $resolution
- Game frame rate (measured): $game_fps
- Frame-change thresholds: $thresholds
- Recorder: $recorder

## Recordings

The files stay in the evidence directory, outside the repository. Size and
SHA-256 are of the file as recorded. `observe probe` measured the game's
frame rate in each with the thresholds above, and fills in the audio − video
offset once sync events are marked (`probe --sync-ref … --record`).

| File | Size (bytes) | SHA-256 | Video | Audio | Game fps | A/V offset (s) |
|---|---|---|---|---|---|---|
$recordings

## Setup notes

$notes

## Marks

`observe mark` writes one CSV per recording and markset into the evidence
directory, `$$OBFT_EVIDENCE_DIR/$id/$id.<recording>.<markset>.csv`, with the
frame grabs beside it. They stay out of the repository; an observation entry
cites each marks file by its SHA-256, and their timestamp proofs, if any,
sit beside this file.
