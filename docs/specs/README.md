# Behaviour specs

A spec says what the original game does in one area: craters, the
briefing, saving. It is written in plain language, so a player at the
original could check each rule by playing. The engine is built from the
specs, and code that implements one cites it.

A spec records what the data files, the manual and observation settle
(see [.claude/CLAUDE.md](../../.claude/CLAUDE.md) for that order). It
does not describe how the engine does it.

## What a spec holds

- **A title and a header line `Settled <date>`.** The date is the day the
  maintainer approved this version. Agents may draft a spec when asked. A
  draft says `Draft` in place of the header line until it is approved.
- **Rules.** They are numbered, one behaviour each, worded as what a player
  sees or can do. "A crater's rim has N sides" is a rule. "The crater
  system builds N vertices" is not. A number in a rule comes with its
  unit and its source.
- **For every rule, its sources.** These are the observation entries
  (`docs/observations/<topic>.md`, by item id), the data fields (file and
  key, with the `docs/data/` or `READER-SCHEMA.md` section that documents
  them) and the manual pages it rests on. A rule without a source is a
  guess and is marked as one.
- **For every rule, a confidence:**
  - **(V)** observed, or read from the data;
  - **(I)** inferred from observations, without being seen directly;
  - **(U)** a guess, recorded so the engine has something to build on.
    A guess is also listed under Open.
- **An "Open" section.** It lists what the spec cannot settle yet. Each
  question points at the item in
  [observations/plan.md](../observations/plan.md) that will settle it, or
  proposes one. The section stays even when it is empty, and then it says
  so.

## What a spec leaves out

- **Code and pseudocode.** No engine names: no systems, components,
  resources or function names. A spec should still be true if the engine
  were rewritten.
- **Tuning that nothing settles.** If no source fixes a value, the rule
  says so and the question goes under Open. The engine's approximation
  lives in `assets/tuning/`, not in the spec.
- **Copied game data.** A spec names a data field and says what it means.
  It does not repeat tables of the data's values.

## When code and spec disagree

Say so. The spec changes only when new evidence does, such as a new
observation entry, a new reading of the data, or a page of the manual.
It never changes to fit the code. A changed rule gets a new `Settled`
date when the maintainer approves it, and git keeps the old one.
