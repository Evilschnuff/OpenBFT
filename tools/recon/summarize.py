#!/usr/bin/env python3
"""Summarise the mech3ax extraction of Recoil data into small text files.

Reads the extraction cache produced by `unzbd rc` (see docs/data/install.md) and
writes one compact summary per data family into an output directory. The
summaries, not the raw JSON, are what the analysis reads: every file is kept
to a few hundred lines.

No game data is written into the repository; the output directory must live
outside it.

Usage:
    tools/recon/summarize.py [--extract DIR] [--out DIR]

Defaults: --extract $BFT_EXTRACT_DIR
          --out     <extract>/summaries
"""

from __future__ import annotations

import argparse
import collections
import json
import os
import pathlib
import re
import zipfile

MISSIONS = [f"m{n}" for n in range(1, 14)]
MAX_EXAMPLE_LINES = 40


# --------------------------------------------------------------------------
# helpers
# --------------------------------------------------------------------------


class Out:
    """A summary file being built, with a hard line budget."""

    def __init__(self, path: pathlib.Path, limit: int = 300):
        self.path = path
        self.limit = limit
        self.lines: list[str] = []
        self.truncated = False

    def add(self, line: str = "") -> None:
        if len(self.lines) >= self.limit:
            self.truncated = True
            return
        self.lines.append(line)

    def add_all(self, lines) -> None:
        for line in lines:
            self.add(line)

    def write(self) -> None:
        if self.truncated:
            self.lines.append(f"... (truncated at {self.limit} lines)")
        self.path.parent.mkdir(parents=True, exist_ok=True)
        self.path.write_text("\n".join(self.lines) + "\n")
        print(f"  wrote {self.path.name} ({len(self.lines)} lines)")


def read_json(zf: zipfile.ZipFile, name: str):
    with zf.open(name) as fh:
        return json.load(fh)


def histogram(counter: collections.Counter, indent: str = "  ", top: int | None = None):
    """Format a counter as aligned `count  key` lines, most frequent first."""
    items = counter.most_common(top)
    if not items:
        return [indent + "(none)"]
    width = max(len(str(c)) for _, c in items)
    return [f"{indent}{c:>{width}}  {k}" for k, c in items]


def truncate_json(value, max_lines: int = MAX_EXAMPLE_LINES) -> list[str]:
    """Pretty-print a JSON value, cut to max_lines."""
    text = json.dumps(value, indent=1)
    lines = text.splitlines()
    if len(lines) > max_lines:
        lines = lines[:max_lines] + [f"... ({len(text.splitlines())} lines total)"]
    return lines


def name_prefix(name: str) -> str:
    """Collapse a node/texture name to an alphabetic prefix.

    `tank_01` -> `tank_`, `m1gate16` -> `m1gate`, `water01` -> `water`.
    Digits and trailing separators are stripped so that families of numbered
    instances collapse into one bucket.
    """
    stem = name.rsplit(".", 1)[0] if name.lower().endswith(".flt") else name
    prefix = re.match(r"^[A-Za-z_]*[A-Za-z]", stem)
    return prefix.group(0) if prefix else "(numeric)"


def stem(name: str) -> str:
    """Drop a texture name's extension: `rock01.tif` -> `rock01`."""
    return name.rsplit(".", 1)[0] if "." in name else name


def variant_of(value):
    """Name of a mech3ax sum-type variant: {"Object3d": {...}} -> Object3d."""
    if isinstance(value, dict) and len(value) == 1:
        return next(iter(value))
    if isinstance(value, str):
        return value
    return "(unknown)"


def variant_body(value):
    if isinstance(value, dict) and len(value) == 1:
        return next(iter(value.values()))
    return {}


# --------------------------------------------------------------------------
# reader (zrdr.zbd)
# --------------------------------------------------------------------------


def describe_reader_value(value, depth: int = 0) -> str:
    """One-line shape description of a reader JSON value."""
    if isinstance(value, list):
        if not value:
            return "[]"
        kinds = {type(v).__name__ for v in value}
        return f"list[{len(value)}] of {'/'.join(sorted(kinds))}"
    if isinstance(value, str):
        return f"str({value[:24]!r})"
    return type(value).__name__


def reader_keys(entry):
    """Extract the key/value pairing used by reader files.

    Reader data is a nested list where a string is a key and the item that
    follows it is that key's value list. Returns (keys, depth).
    """
    keys: list[str] = []
    if isinstance(entry, list):
        i = 0
        while i < len(entry):
            if isinstance(entry[i], str) and i + 1 < len(entry) and isinstance(entry[i + 1], list):
                keys.append(entry[i])
                i += 2
            else:
                i += 1
    return keys


def list_depth(value, cap: int = 12) -> int:
    if not isinstance(value, list) or not value or cap == 0:
        return 0
    return 1 + max((list_depth(v, cap - 1) for v in value), default=0)


def summarize_reader(zip_path: pathlib.Path, out: Out, label: str) -> None:
    out.add(f"# reader summary: {label}")
    out.add(f"# source: {zip_path.name}")
    out.add()
    with zipfile.ZipFile(zip_path) as zf:
        names = sorted(n for n in zf.namelist() if n.endswith(".zrd.json"))
        out.add(f"{len(names)} .zrd entries")
        out.add()
        example_shown = False
        for name in names:
            data = read_json(zf, name)
            depth = list_depth(data)
            # Reader files are a single-element list wrapping the body.
            body = data[0] if isinstance(data, list) and len(data) == 1 else data
            keys = reader_keys(body)
            entry = name[: -len(".json")]
            out.add(f"- {entry}: depth={depth} top_len={len(data) if isinstance(data, list) else '-'}"
                    f" keys={len(keys)}")
            if keys:
                shown = keys[:14]
                out.add(f"    keys: {', '.join(shown)}" + (" …" if len(keys) > len(shown) else ""))
            if not example_shown and keys:
                example_shown = True
                out.add()
                out.add(f"  EXAMPLE ({entry}, truncated):")
                out.add_all("  " + line for line in truncate_json(data))
                out.add()
    out.write()


# --------------------------------------------------------------------------
# interp
# --------------------------------------------------------------------------


def summarize_interp(path: pathlib.Path, out: Out) -> None:
    data = json.loads(path.read_text())
    out.add("# interp summary: every script, its line count, and its first 15 lines")
    out.add(f"# source: {path.name}")
    out.add()
    out.add(f"{len(data)} scripts")
    out.add()
    verbs: collections.Counter = collections.Counter()
    for script in data:
        lines = script.get("lines", [])
        for line in lines:
            first = line.split()[0] if line.split() else ""
            if first:
                verbs[first] += 1
    out.add(f"command vocabulary ({len(verbs)} distinct verbs), by frequency:")
    out.add_all(histogram(verbs, top=60))
    out.add()
    for script in data:
        lines = script.get("lines", [])
        out.add(f"- {script.get('name')}: {len(lines)} lines  ({script.get('datetime','')})")
    out.add()
    out.add("FIRST 15 LINES OF EACH SCRIPT")
    for script in data:
        lines = script.get("lines", [])
        out.add()
        out.add(f"## {script.get('name')} ({len(lines)} lines)")
        out.add_all("  " + line for line in lines[:15])
    out.write()


# --------------------------------------------------------------------------
# anim
# --------------------------------------------------------------------------


def summarize_anim(extract: pathlib.Path, out: Out) -> None:
    out.add("# anim summary: EventData histogram across all levels + short sequences")
    out.add()
    events_all: collections.Counter = collections.Counter()
    activation_all: collections.Counter = collections.Counter()
    execution_all: collections.Counter = collections.Counter()
    per_level_defs: dict[str, int] = {}
    per_level_scripts: dict[str, int] = {}
    prereq_shapes: collections.Counter = collections.Counter()
    name_prefixes: collections.Counter = collections.Counter()
    samples: list[tuple[str, dict]] = []

    for mission in MISSIONS:
        zip_path = extract / mission / "anim.zip"
        if not zip_path.exists():
            continue
        with zipfile.ZipFile(zip_path) as zf:
            # `*.zan.json` are SI scripts (ObjectMotionSiScript payloads),
            # not anim definitions; count them separately.
            names = [
                n for n in zf.namelist()
                if n != "metadata.json" and not n.endswith(".zan.json")
            ]
            per_level_scripts[mission] = sum(
                1 for n in zf.namelist() if n.endswith(".zan.json")
            )
            per_level_defs[mission] = len(names)
            for name in names:
                anim_def = read_json(zf, name)
                activation_all[str(anim_def.get("activation"))] += 1
                execution_all[variant_of(anim_def.get("execution"))] += 1
                name_prefixes[name_prefix(anim_def.get("anim_name", ""))] += 1
                for prereq in anim_def.get("activ_prereqs") or []:
                    prereq_shapes[variant_of(prereq)] += 1
                seqs = anim_def.get("sequences") or []
                for seq in seqs:
                    for event in seq.get("events") or []:
                        events_all[variant_of(event.get("data"))] += 1
                for event in (anim_def.get("reset_state") or {}).get("events") or []:
                    events_all[variant_of(event.get("data"))] += 1
                if mission == "m1" and len(samples) < 3:
                    total = sum(len(s.get("events") or []) for s in seqs)
                    if 2 <= total <= 6 and seqs:
                        samples.append((name, anim_def))

    out.add("anim defs (and SI scripts) per level:")
    out.add_all(
        f"  {m}: {c} defs, {per_level_scripts.get(m, 0)} si-scripts"
        for m, c in per_level_defs.items()
    )
    out.add(f"  total: {sum(per_level_defs.values())} defs, "
            f"{sum(per_level_scripts.values())} si-scripts")
    out.add()
    out.add("AnimActivation distribution (all levels):")
    out.add_all(histogram(activation_all))
    out.add()
    out.add("Execution variant distribution (all levels):")
    out.add_all(histogram(execution_all))
    out.add()
    out.add("activ_prereqs variant shapes (all levels):")
    out.add_all(histogram(prereq_shapes))
    out.add()
    out.add(f"EventData variant histogram (all levels, {sum(events_all.values())} events):")
    out.add_all(histogram(events_all))
    out.add()
    out.add("anim_name prefixes (all levels, top 40):")
    out.add_all(histogram(name_prefixes, top=40))
    out.add()
    out.add("THREE COMPLETE SHORT SEQUENCES (m1)")
    for name, anim_def in samples:
        out.add()
        out.add(f"## {name}")
        out.add(f"   activation={anim_def.get('activation')} "
                f"execution={variant_of(anim_def.get('execution'))} "
                f"reset_time={anim_def.get('reset_time')}")
        for seq in anim_def.get("sequences") or []:
            body = {
                "name": seq.get("name"),
                "seq_state": seq.get("seq_state"),
                "events": [
                    {"start": e.get("start"), "data": e.get("data")}
                    for e in (seq.get("events") or [])
                ],
            }
            out.add_all("   " + line for line in truncate_json(body, 60))
    out.write()


# --------------------------------------------------------------------------
# gamez
# --------------------------------------------------------------------------


def summarize_gamez(extract: pathlib.Path, out: Out) -> None:
    """Node histograms.

    In this mech3ax build a node is a flat object: `name`, `flags`,
    `zone_id`, `model_index`, `area_partition` and the bounding boxes live at
    the top level, and the per-variant payload sits under `data`
    ({"Object3d": {...}}), which is also where an Object3d's transform is.
    """
    out.add("# gamez summary: node-variant and name-prefix histograms per level")
    out.add("# node schema: flat struct; variant under .data, transform under .data.Object3d")
    out.add()
    for mission in MISSIONS:
        zip_path = extract / mission / "gamez.zip"
        if not zip_path.exists():
            continue
        with zipfile.ZipFile(zip_path) as zf:
            nodes = read_json(zf, "nodes.json")
        variants: collections.Counter = collections.Counter()
        prefixes: collections.Counter = collections.Counter()
        no_model = 0
        zones: collections.Counter = collections.Counter()
        flags_set: collections.Counter = collections.Counter()
        transforms: collections.Counter = collections.Counter()
        active_bbox: collections.Counter = collections.Counter()
        has_ap = has_vp = 0
        for node in nodes:
            variant = variant_of(node.get("data"))
            variants[variant] += 1
            name = node.get("name")
            if isinstance(name, str):
                prefixes[name_prefix(name)] += 1
            if node.get("model_index", 0) == -1:
                no_model += 1
            if node.get("area_partition") is not None:
                has_ap += 1
            if node.get("virtual_partition") is not None:
                has_vp += 1
            zones[node.get("zone_id")] += 1
            active_bbox[str(node.get("active_bbox"))] += 1
            for flag, value in (node.get("flags") or {}).items():
                if value is True:
                    flags_set[flag] += 1
            if variant == "Object3d":
                transform = variant_body(node.get("data")).get("transform")
                transforms[variant_of(transform)] += 1
        out.add(f"## {mission}: {len(nodes)} nodes")
        out.add("  node variants:")
        out.add_all(histogram(variants, indent="    "))
        out.add(f"  nodes with model_index == -1: {no_model}")
        out.add(f"  nodes with area_partition set: {has_ap}; virtual_partition set: {has_vp}")
        out.add("  Object3d transform variants:")
        out.add_all(histogram(transforms, indent="    "))
        out.add("  active_bbox:")
        out.add_all(histogram(active_bbox, indent="    "))
        out.add(f"  zone_id: {dict(zones.most_common(10))}")
        out.add(f"  flags set: {dict(flags_set.most_common(20))}")
        out.add("  name prefixes (top 24):")
        out.add_all(histogram(prefixes, indent="    ", top=24))
        out.add()
    out.write()


def summarize_models(extract: pathlib.Path, out: Out) -> None:
    """Polygon size distribution, TRI_STRIP, facades, morphs, lights."""
    out.add("# model summary: polygon sizes, model types, flags per level")
    out.add()
    grand_poly: collections.Counter = collections.Counter()
    for mission in MISSIONS:
        zip_path = extract / mission / "gamez.zip"
        if not zip_path.exists():
            continue
        with zipfile.ZipFile(zip_path) as zf:
            models = read_json(zf, "models.json")
        poly_sizes: collections.Counter = collections.Counter()
        model_types: collections.Counter = collections.Counter()
        facade_modes: collections.Counter = collections.Counter()
        model_flags: collections.Counter = collections.Counter()
        poly_flags: collections.Counter = collections.Counter()
        n_lights = n_morphs = n_scroll = n_vcolor = n_multimat = 0
        n_empty = 0
        for model in models:
            if model is None:
                n_empty += 1
                continue
            model_types[str(model.get("model_type"))] += 1
            if str(model.get("model_type")) == "Facade":
                facade_modes[str(model.get("facade_mode"))] += 1
            for flag, value in (model.get("flags") or {}).items():
                if value is True:
                    model_flags[flag] += 1
            if model.get("lights"):
                n_lights += 1
            if model.get("morphs"):
                n_morphs += 1
            scroll = model.get("texture_scroll") or {}
            if scroll.get("u") or scroll.get("v"):
                n_scroll += 1
            for poly in model.get("polygons") or []:
                count = len(poly.get("vertex_indices") or [])
                poly_sizes[count] += 1
                grand_poly[count] += 1
                for flag, value in (poly.get("flags") or {}).items():
                    if value is True:
                        poly_flags[flag] += 1
                if poly.get("vertex_colors"):
                    n_vcolor += 1
                if len(poly.get("materials") or []) > 1:
                    n_multimat += 1
        total_poly = sum(poly_sizes.values())
        out.add(f"## {mission}: {len(models)} model slots ({n_empty} null), {total_poly} polygons")
        out.add("  model types:")
        out.add_all(histogram(model_types, indent="    "))
        if facade_modes:
            out.add("  facade modes:")
            out.add_all(histogram(facade_modes, indent="    "))
        out.add("  model flags set:")
        out.add_all(histogram(model_flags, indent="    "))
        out.add("  polygon vertex counts:")
        out.add_all(histogram(poly_sizes, indent="    ", top=12))
        out.add("  polygon flags set:")
        out.add_all(histogram(poly_flags, indent="    "))
        out.add(f"  models with lights={n_lights} morphs={n_morphs} texture_scroll={n_scroll}")
        out.add(f"  polygons with vertex_colors={n_vcolor} multi-material={n_multimat}")
        out.add()
    out.add("## all levels: polygon vertex counts")
    out.add_all(histogram(grand_poly, indent="    ", top=16))
    out.write()


def summarize_textures(extract: pathlib.Path, out: Out) -> None:
    """Texture counts, alpha modes, sizes; gamez names vs ZIP entries."""
    out.add("# texture/material summary per level and per archive")
    out.add()

    def manifest_of(zip_path: pathlib.Path):
        with zipfile.ZipFile(zip_path) as zf:
            manifest = read_json(zf, "manifest.json")
            pngs = {n for n in zf.namelist() if n.endswith(".png")}
        infos = manifest.get("texture_infos", manifest) if isinstance(manifest, dict) else manifest
        return infos, pngs, manifest

    for mission in MISSIONS:
        gamez_path = extract / mission / "gamez.zip"
        if not gamez_path.exists():
            continue
        with zipfile.ZipFile(gamez_path) as zf:
            tex_refs = read_json(zf, "textures.json")
            materials = read_json(zf, "materials.json")
        ref_names = {t if isinstance(t, str) else t.get("name") for t in tex_refs}
        # gamez texture references keep the source extension (`x.tif`); the
        # texture archives store bare stems (`x`). Compare both ways.
        ref_stems = {stem(n) for n in ref_names if n}
        out.add(f"## {mission}: gamez references {len(ref_names)} textures, "
                f"{len(materials)} materials")
        soils: collections.Counter = collections.Counter()
        mat_kinds: collections.Counter = collections.Counter()
        cycled = 0
        for mat in materials:
            kind = variant_of(mat)
            mat_kinds[kind] += 1
            body = variant_body(mat)
            soils[str(body.get("soil", "Default"))] += 1
            if body.get("cycle"):
                cycled += 1
        out.add("  material kinds:")
        out.add_all(histogram(mat_kinds, indent="    "))
        out.add("  Soil values:")
        out.add_all(histogram(soils, indent="    "))
        out.add(f"  cycled textures: {cycled}")
        for archive in ("rtexture2", "rtexture4", "texture2", "texture4", "texture6", "texture8"):
            tex_path = extract / mission / f"{archive}.zip"
            if not tex_path.exists():
                continue
            infos, pngs, _ = manifest_of(tex_path)
            alphas: collections.Counter = collections.Counter()
            sizes: collections.Counter = collections.Counter()
            palettes: collections.Counter = collections.Counter()
            names = set()
            for info in infos:
                names.add(info.get("name"))
                alphas[str(info.get("alpha"))] += 1
                sizes[f"{info.get('width')}x{info.get('height')}"] += 1
                palettes[variant_of(info.get("palette"))] += 1
            out.add(f"  {archive}: {len(infos)} textures, {len(pngs)} PNGs")
            out.add(f"    alpha: {dict(alphas)}")
            out.add(f"    palette: {dict(palettes)}")
            out.add(f"    sizes (top 6): {dict(sizes.most_common(6))}")
            stems = {stem(n) for n in names if n}
            missing = ref_stems - stems
            extra = stems - ref_stems
            out.add(f"    name match (extension-stripped): {len(ref_stems & stems)}/{len(ref_stems)}"
                    f" gamez refs resolve; {len(extra)} archive textures unreferenced")
            if missing:
                out.add(f"    unresolved gamez refs: {sorted(missing)[:5]}")
            if archive == "rtexture4":
                r2_path = extract / mission / "rtexture2.zip"
                if r2_path.exists():
                    infos2, _, _ = manifest_of(r2_path)
                    names2 = {stem(i.get("name")) for i in infos2 if i.get("name")}
                    out.add(f"    rtexture2 vs rtexture4: {len(stems & names2)} shared names, "
                            f"{len(names2 - stems)} only in r2, {len(stems - names2)} only in r4")
        out.add()

    image_path = extract / "image.zip"
    if image_path.exists():
        infos, pngs, _ = manifest_of(image_path)
        alphas = collections.Counter(str(i.get("alpha")) for i in infos)
        sizes = collections.Counter(f"{i.get('width')}x{i.get('height')}" for i in infos)
        out.add(f"## image.zbd (global): {len(infos)} textures, {len(pngs)} PNGs")
        out.add(f"  alpha: {dict(alphas)}")
        out.add(f"  sizes (top 10): {dict(sizes.most_common(10))}")
        out.add(f"  example names: {[i.get('name') for i in infos[:8]]}")
    out.write()


def summarize_messages(path: pathlib.Path, out: Out) -> None:
    """messages.json is {"language_id": N, "entries": [{key, id, value}, …]}."""
    data = json.loads(path.read_text())
    entries = data.get("entries", []) if isinstance(data, dict) else data
    out.add("# messages summary: all keys with the first 60 chars of each value")
    out.add()
    if isinstance(data, dict):
        out.add(f"language_id: {data.get('language_id')}")
        out.add()
    pairs = []
    for item in entries:
        if isinstance(item, dict):
            pairs.append((str(item.get("key")), str(item.get("value", ""))))
        elif isinstance(item, list) and len(item) >= 2:
            pairs.append((str(item[0]), str(item[-1])))
    out.add(f"{len(pairs)} messages")
    out.add()
    for key, value in pairs:
        flat = value.replace("\n", "\\n").replace("\r", "")
        out.add(f"{key}: {flat[:60]}")
    out.write()


def summarize_sounds(extract: pathlib.Path, out: Out) -> None:
    out.add("# sound summary: per-archive file lists and cross-archive comparison")
    out.add()
    archives: dict[str, dict[str, int]] = {}
    for fidelity in ("l", "m", "h"):
        zip_path = extract / f"sounds{fidelity}.zip"
        if not zip_path.exists():
            continue
        with zipfile.ZipFile(zip_path) as zf:
            wavs = {
                info.filename: info.file_size
                for info in zf.infolist()
                if info.filename.lower().endswith(".wav")
            }
        archives[fidelity] = wavs
        total = sum(wavs.values())
        out.add(f"## sounds{fidelity}.zbd: {len(wavs)} wav files, {total/1e6:.1f} MB uncompressed")
        prefixes = collections.Counter(name_prefix(n) for n in wavs)
        out.add("  name prefixes (top 25):")
        out.add_all(histogram(prefixes, indent="    ", top=25))
    if len(archives) > 1:
        sets = {f: set(v) for f, v in archives.items()}
        keys = sorted(sets)
        common = set.intersection(*sets.values())
        out.add()
        out.add(f"shared names across sounds{'/'.join(keys)}: {len(common)}")
        for fidelity in keys:
            only = sets[fidelity] - common
            out.add(f"  only in sounds{fidelity}: {len(only)} {sorted(only)[:5]}")
        ref = keys[0]
        ratios = []
        for name in sorted(common)[:200]:
            base = archives[ref][name]
            if base:
                ratios.append(tuple(archives[f][name] / base for f in keys))
        if ratios:
            avg = [sum(r[i] for r in ratios) / len(ratios) for i in range(len(keys))]
            out.add("  mean size ratio vs sounds" + ref + ": "
                    + ", ".join(f"{k}={a:.2f}" for k, a in zip(keys, avg)))
    out.add()
    out.add("EXAMPLE NAMES (first 40 of the high-fidelity archive)")
    if "h" in archives:
        out.add_all("  " + n for n in sorted(archives["h"])[:40])
    out.write()


def summarize_zmap(extract: pathlib.Path, out: Out) -> None:
    out.add("# zmap summary: extents and feature counts per map")
    out.add()
    for mission in MISSIONS:
        path = extract / "maps" / f"{mission}.json"
        if not path.exists():
            out.add(f"## {mission}: (no zmap)")
            continue
        data = json.loads(path.read_text())
        features = data.get("features") or []
        verts = [v for f in features for v in (f.get("vertices") or [])]
        objectives = collections.Counter(f.get("objective") for f in features)
        out.add(f"## {mission}: {len(features)} features, {len(verts)} vertices")
        out.add(f"  header min={data.get('min')}")
        out.add(f"  header max={data.get('max')}")
        out.add(f"  unk04={data.get('unk04')}")
        out.add(f"  objective values: {dict(objectives.most_common(8))}")
        # Per-axis vertex ranges: the header's y is always 0, but vertices
        # carry a small y elevation. This is the evidence for y being up.
        for axis in ("x", "y", "z"):
            vals = [v[axis] for v in verts]
            if vals:
                out.add(f"  vertex {axis}: {min(vals):.1f} … {max(vals):.1f}")
    out.write()


# --------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    default_extract = os.environ.get("BFT_EXTRACT_DIR")
    parser.add_argument("--extract", default=default_extract, type=pathlib.Path,
                        required=default_extract is None,
                        help="extraction directory (default $BFT_EXTRACT_DIR)")
    parser.add_argument("--out", default=None, type=pathlib.Path)
    args = parser.parse_args()

    extract: pathlib.Path = args.extract
    out_dir: pathlib.Path = args.out or (extract / "summaries")
    if not extract.is_dir():
        raise SystemExit(f"extraction directory not found: {extract}")
    out_dir.mkdir(parents=True, exist_ok=True)
    print(f"summarising {extract} -> {out_dir}")

    if (extract / "zrdr.zip").exists():
        summarize_reader(extract / "zrdr.zip", Out(out_dir / "reader-global.txt"), "global zrdr.zbd")
    for mission in MISSIONS:
        path = extract / mission / "zrdr.zip"
        if path.exists():
            summarize_reader(path, Out(out_dir / f"reader-{mission}.txt"), f"{mission}/zrdr.zbd")
    if (extract / "interp.json").exists():
        summarize_interp(extract / "interp.json", Out(out_dir / "interp.txt", limit=600))
    summarize_anim(extract, Out(out_dir / "anim.txt", limit=400))
    summarize_gamez(extract, Out(out_dir / "gamez-nodes.txt", limit=600))
    summarize_models(extract, Out(out_dir / "gamez-models.txt", limit=700))
    summarize_textures(extract, Out(out_dir / "textures.txt", limit=700))
    if (extract / "messages.json").exists():
        summarize_messages(extract / "messages.json", Out(out_dir / "messages.txt", limit=900))
    summarize_sounds(extract, Out(out_dir / "sounds.txt", limit=400))
    summarize_zmap(extract, Out(out_dir / "zmap.txt", limit=200))
    print("done")


if __name__ == "__main__":
    main()
