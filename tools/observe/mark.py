"""`observe mark`: mark events in a recording by hand, in mpv.

The markset (tools/observe/marksets/<set>.toml) names the keys; each key
press appends (frame, time, label, note) to
$OBFT_EVIDENCE_DIR/<session>/<session>.<recording>.<set>.csv. That CSV is
evidence and stays out of the repository; its timestamp proofs (if `ots`
is installed) go beside the session file, which holds no evidence.

Markset format:

    name = "briefing"
    description = "what the set is for"

    [[key]]
    key = "p"              # an mpv key name
    label = "proceed"      # what the row records
    help = "PROCEED is first visible"
    note = false           # ask for a note (typed text) with the mark
    pos = false            # record the mouse position, in recording pixels
    grab = false           # save the frame as a PNG next to the recording
    wording = false        # the note carries the game's wording; an entry
                           # shows its `key:` or `desc:` part instead (see result.py)

A note is `;`-separated parts: free text, `key: MSG_…` (a message key or
reader control) and `desc: …` (a description in our own words).
"""

from __future__ import annotations

import json
import pathlib
import re
import subprocess
import tempfile
import tomllib

from common import (NOT_OBSERVABLE, VOID, ObserveError, ensure_marks_file, in_evidence, marks_path,
                    marksets_dir, need, need_video, outside_repo, probe_media, read_candidate_times,
                    read_marks, session_evidence, sessions_dir, sha256_file, stamp)
from session import find_session

LUA = pathlib.Path(__file__).resolve().parent / "mark.lua"
# mpv's keys for moving through the recording, and the script's own
RESERVED = {"SPACE", ".", ",", "q", "Q", "LEFT", "RIGHT", "UP", "DOWN", "ESC", "ENTER",
            "BS", "TAB", "Shift+TAB", "F1", "!", "[", "]", "{", "}"}
BUILT_IN = {"key": "!", "label": NOT_OBSERVABLE, "note": True,
            "help": "what you tried that could not be seen (note)"}
_LABEL_RE = re.compile(r"^[a-z0-9][a-z0-9-]*$")


def load_markset(name: str) -> dict:
    path = marksets_dir() / f"{name}.toml"
    if not path.is_file():
        raise ObserveError(f"no markset {path}")
    with open(path, "rb") as fh:
        data = tomllib.load(fh)
    problems = []
    if data.get("name") != name:
        problems.append(f"name should be '{name}'")
    keys = data.get("key", [])
    if not keys:
        problems.append("no [[key]] entries")
    seen_keys, seen_labels = set(), set()
    for k in keys:
        key, label = k.get("key"), k.get("label")
        if not key or not label or not k.get("help"):
            problems.append(f"key entry {k} needs key, label and help")
            continue
        if key in RESERVED:
            problems.append(f"key '{key}' is reserved for moving through the recording")
        if not _LABEL_RE.match(label) or label in (VOID, NOT_OBSERVABLE):
            problems.append(f"label '{label}' must be lower-case words joined by '-', "
                            f"and not '{VOID}' or '{NOT_OBSERVABLE}'")
        if key in seen_keys:
            problems.append(f"key '{key}' is bound twice")
        if label in seen_labels:
            problems.append(f"label '{label}' is used twice")
        unknown = set(k) - {"key", "label", "help", "note", "pos", "grab", "wording"}
        if k.get("wording") and not k.get("note"):
            problems.append(f"key '{key}' records wording, so it needs note = true")
        if unknown:
            problems.append(f"key '{key}' has unknown fields {sorted(unknown)}")
        seen_keys.add(key)
        seen_labels.add(label)
    if problems:
        raise ObserveError(f"{path}: " + "; ".join(problems))
    data["labels"] = seen_labels | {NOT_OBSERVABLE}
    data["wording"] = {k["label"] for k in keys if k.get("wording")}
    return data


def grabs_sidecar(csv_path: pathlib.Path) -> pathlib.Path:
    """`sha256sum` lines for the frame grabs a marks file names."""
    return csv_path.with_name(csv_path.stem + ".grabs.sha256")


def read_grab_hashes(csv_path: pathlib.Path) -> dict[str, str]:
    side = grabs_sidecar(csv_path)
    if not side.is_file():
        return {}
    pairs = (line.split(None, 1) for line in side.read_text().splitlines() if line.strip())
    return {name.strip(): digest for digest, name in pairs}


def _hash_grabs(csv_path: pathlib.Path, grabs_dir: pathlib.Path) -> int:
    known = read_grab_hashes(csv_path)
    lines = []
    for m in read_marks(csv_path):
        name = m.grab
        if name and name not in known and (grabs_dir / name).is_file():
            known[name] = sha256_file(grabs_dir / name)
            lines.append(f"{known[name]}  {name}\n")
    if lines:
        with open(grabs_sidecar(csv_path), "a") as fh:
            fh.writelines(lines)
    return len(lines)


def mark(args) -> None:
    rec = pathlib.Path(args.rec)
    in_evidence(rec, "recording")
    media = probe_media(rec)
    need_video(media)
    markset = load_markset(args.set)
    session = find_session(rec.name, args.session)
    csv_path = marks_path(session.id, rec.name, markset["name"])
    ensure_marks_file(csv_path)
    existing = read_marks(csv_path)
    keys = [dict(k) for k in markset["key"]] + [BUILT_IN]
    grabs_dir = (pathlib.Path(args.grabs) if args.grabs
                 else session_evidence(session.id) / f"{rec.stem}.grabs")
    if any(k.get("grab") for k in keys):
        outside_repo(grabs_dir, "frame grab directory")
        grabs_dir.mkdir(parents=True, exist_ok=True)
    config = {
        "csv": str(csv_path.resolve()),
        "markset": markset["name"],
        "fps_num": media.fps.numerator,
        "fps_den": media.fps.denominator,
        "video_offset": media.video_offset,
        "rec_stem": rec.stem,
        "grabs_dir": str(grabs_dir.resolve()),
        "keys": keys,
        "rows": len(existing),
        "live": [m.row for m in existing if m.label != VOID and m.voided_by is None],
        "candidates": read_candidate_times(pathlib.Path(args.candidates)) if args.candidates else [],
    }
    print(f"marking {rec.name} with '{markset['name']}' into {csv_path}")
    print("  F1 in mpv lists the keys; '.' and ',' step frames; BS voids the last row")
    with tempfile.TemporaryDirectory() as tmp:
        cfg = pathlib.Path(tmp) / "observe.json"
        cfg.write_text(json.dumps(config))
        cmd = [need("mpv"), f"--script={LUA}", f"--script-opts=observe-config={cfg}",
               "--pause", "--keep-open=yes", "--hr-seek=yes", "--osd-level=1", "--osd-fractions",
               f"--title=observe {markset['name']}: {rec.name}", *args.mpv_arg, str(rec)]
        subprocess.run(cmd, check=False)
    after = read_marks(csv_path)
    print(f"{len(after) - len(existing)} new row(s), {len(after)} in {csv_path.name}")
    if grabs_dir.is_dir():
        hashed = _hash_grabs(csv_path, grabs_dir)
        if hashed:
            print(f"  hashed {hashed} frame grab(s) into {grabs_sidecar(csv_path).name}")
    if len(after) > len(existing):
        stamp(csv_path, sessions_dir())
