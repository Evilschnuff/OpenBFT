# Observations

What the original game does as it runs, measured from recordings of play.
Some rules are not settled by the data files or the manual. This is where
they get settled, and the specs in [docs/specs/](../specs/README.md) cite
what is found here.

Nothing here comes from the original's program. The tools read only
recordings and the game's data files. They never start the game, attach to
it, read its memory, change it or open its program files.

## The method

1. **Ask.** A question goes into [plan.md](plan.md) as an item, with the
   precision it needs. Its item file (`tools/observe/items/<item>.toml`)
   says how to record it, which set of keys to mark it with, and how to
   compute the answer.
2. **Record.** Play the original and record it with OBS: video at a
   constant 60 fps plus the game's own sound, lossless or at a high
   bitrate. When timing matters, show key presses on screen so every press
   has a frame. The recordings go in the evidence directory (below). Then
   `observe session new` writes a session file. It holds the date, the game
   version, the OS or VM, the game's resolution, and each recording's file
   name, size and SHA-256. It also holds the game's own frame rate, measured
   from the recording, and the thresholds the measurement used. Where a
   result will combine sounds with marked frames, mark a few events that are
   seen and heard at once, and `observe probe` records the recording's
   audio − video offset in the session file.
3. **Mark.** Step through the recording frame by frame in mpv. At each
   event, press the key the item's markset gives for it. Each press adds
   one row to the marks file, in the evidence directory: frame, time, label
   and a note. Some events a
   program can find, such as a known sound from the data, a change in one
   part of the screen or a sound's pitch. For those, the tools suggest
   frames, but a person still looks and marks.
4. **Compute.** `observe result <item>` runs the item's analysis on the
   marks: a count, an order, a duration, a ratio, a straight line or a time
   constant. Each result comes with its uncertainty. An item can also use
   a known sound's onsets as events; they are moved onto the video's clock
   by the measured audio − video offset, which the entry states.
5. **Write the entry.** The same command writes the item's entry into
   `<topic>.md` here, or replaces it. The entry has the question, how it was
   recorded and measured, the recordings and marks by hash, every raw
   reading (the game's wording shown by its key; see below), the result
   with its uncertainty, its kind, and notes.

## Rules

- **Readings are never edited after the fact.** A marks file only grows.
  If a mark is wrong, a later row voids it, and the entry shows both. If a
  reading is in doubt, record again; don't adjust it. An entry is only
  ever rewritten by running `result` again on the same marks or on more
  marks.
- **An entry says what was not observable.** Anything that could not be
  seen goes in its "Not observable" section with what was tried. This
  covers something hidden, too fast for a frame, or out of the camera's
  reach. So does any analysis that nothing in the recordings measured.
  "Not observable" is an answer too. It is not left blank.
- **Evidence stays outside the repository.** That means recordings, marks
  files, frame grabs and the measurements taken from them. They all live in
  one evidence directory, `$OBFT_EVIDENCE_DIR`, and the tools refuse one
  inside the repository. Entries and session files name them by SHA-256, so
  anyone holding the files can check that they are the ones measured. The
  repository holds the session files, the entries, the item and markset
  files and any timestamps. Each entry records the SHA-256 of
  every marks file it was computed from, beside the recording's hash, and
  the name of the timestamp proof of exactly that version if there is one.
  That way the full readings can be matched to the entry later.
- **Every result says how precise it is.** A duration is good to one game
  frame at each end. The game's frame rate is measured for each recording,
  never assumed. A point marked on screen is good to a few pixels, and the
  item says how many. A ratio whose two parts share marked points is
  approximate, because its uncertainty treats them as independent.
- **Repeat what needs it.** An approximate item is recorded three times.
  An exact item is recorded once, and again only if two runs disagree.
- **Each result has a kind.** It is *exact* if it is a count, an order, a
  yes or no, or a wording. It is *approximate* if it is a measurement with
  an uncertainty. It is *not observable* if it was tried and could not be
  seen.
- **The game's wording is quoted, never reused.** An entry may quote a
  message or an option's wording in a short excerpt, within the limits of
  the copyright rule in [CONVENTIONS.md](../CONVENTIONS.md). Prefer the key
  (`MSG_*`, a `dialog.zrd` control) and quote only what the result needs.
  When marking, a note gives the wording, then `key:` with the key when it
  can be told, or `desc:` with a description in our own words. The entry
  shows the key, or failing that the description. It quotes the wording
  itself only when the item's question is about the wording, and then only
  for the labels the result depends on. The full wording stays in the marks
  file. The engine still reads its strings from the install.
- **Timestamps when available.** If OpenTimestamps (`ots`) is installed,
  session files, marks files and entries are timestamped as they are
  written. Each version gets its own numbered proof (`<file>.1.ots`,
  `<file>.2.ots`, …), and no proof is ever overwritten. A marks file's
  proofs sit beside its session file, since they hold only a hash. That
  can show later that the recordings and marks came before the entries
  and specs that cite them.

## Files

- [plan.md](plan.md): the open items.
- `sessions/<id>.md`: one per sitting, with the recordings it produced,
  and the timestamp proofs.
- `<topic>.md`: the entries, one section per item, written by
  `observe result`.
- [tools/observe/README.md](../../tools/observe/README.md): the commands,
  what to install, and the item and markset formats.
