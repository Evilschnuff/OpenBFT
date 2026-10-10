# Data reconnaissance scripts

The analysis scripts behind `docs/data/`. They read an extraction of the
Recoil data made with mech3ax's `unzbd` and write compact summaries and
plots. **Everything they produce is derived game data and must stay outside
this repository.**

## Prerequisites

1. A Recoil install.
2. mech3ax built from the commit pinned in `docs/data/install.md`.
3. An extraction directory produced by `extract.sh` below.

All scripts find the extraction directory through `$BFT_EXTRACT_DIR`; the
Python ones also take `--extract DIR`.

## `extract.sh`

Produces the extraction directory: runs all 118 `unzbd rc` commands that
apply to a Recoil install and logs each one's exit status and stderr to
`<extract>/extract.log`. This is the script docs/data/install.md's command
table documents and docs/data/quirks.md's warning inventory quotes.

```sh
MECH3AX=/path/to/mech3ax BFT_DATA_DIR=/path/to/recoil \
  BFT_EXTRACT_DIR=/path/to/extract tools/recon/extract.sh
```

## `summarize.py`

Writes one compact text summary per data family (reader, interp, anim,
gamez nodes, gamez models, textures, messages, sounds, zmap) into
`<extract>/summaries/`, each one to two orders of magnitude smaller than the
JSON it summarises (20 MB for m1's gamez alone). Each file has a line limit
and says so when it truncates.

```sh
python3 tools/recon/summarize.py
python3 tools/recon/summarize.py --extract /path/to/extract --out /tmp/sum
```

Standard library only; no dependencies.

## `plot_level.py`

Determines the coordinate system (docs/data/coordinates.md). Walks a level's
node tree, composes node transforms with their parents', transforms model
vertices to world space, and plots a top-down scatter with the level's
`zmap` features overlaid plus a side view. `--permute` scores every
assignment of world components to plot axes against the zmap and prints the
ranking.

```sh
python3 tools/recon/plot_level.py --level m1 --permute
```

Needs `matplotlib` and `numpy`, best installed in a virtualenv outside the
repository. Plots are written to `<extract>/plots/`.

## `zrd.py`

Renders a reader (`.zrd.json`) entry as an indented key/value tree, or its
key list or deduplicated schema; READER-SCHEMA.md was read off it.

```sh
python3 tools/recon/zrd.py zrdr.zip vehicle --keys
```
