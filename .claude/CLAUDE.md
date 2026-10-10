# Project rules for agents

## Where game rules come from

This is a data-driven rebuild. Rules come from, in this order:

1. the game's data files, through mech3ax and our loaders
   (`docs/data/`, `docs/READER-SCHEMA.md`);
2. the manual;
3. observation of the original as it runs: recordings of play and the
   measurements taken from them (`docs/observations/`);
4. the behaviour specs in `docs/specs/`, which record what 1–3 settle.

You never open, read, search, disassemble or run a tool on the original's
program files (`Recoil.exe`, `recoil3dfx.exe`). Do not look for analyses
of them elsewhere (web, other repositories, other directories on this
machine). A hook blocks tool calls that name them; do not work around it.

The engine's only use of a program file is the splash loader
(`bft_assets::splash`): at run time it reads the one bitmap in the
user's `Recoil.exe` through the file's standard resource table. You may
edit that loader's source. `messages.dll` is read only through
`bft_assets`'s message loader.

## Specs and observations

- Entries in `docs/observations/` record what was measured, how, and how
  precisely. Never invent or adjust a reading.
- A spec cites the entries, data and manual pages it rests on. Agents may
  draft a spec when asked; the maintainer approves it.
- Code that implements a spec is written from it, designed around Bevy
  ECS, and cites it in a comment (`docs/specs/tracks.md`). Comments say
  "the original". If code and spec disagree, say so; don't change the spec
  to fit the code.

## When nothing settles a rule

Don't guess silently. Take the simplest reading, mark it
`// approximation` (tuning values go in `assets/tuning/`, see
`docs/CONVENTIONS.md`), and list the open question in your final
message so it can become an observation item.

## Also

- `docs/CONVENTIONS.md` holds the coding and git rules.
- Never commit game data, recordings, frame grabs or anything carved out
  of them.
- Stage explicit paths only; never `git add -A` or `git add .`.
