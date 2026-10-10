"""`observe result <item>`: from an item's marks to its observation entry.

Reads tools/observe/items/<item>.toml, the session files and the marks CSVs
of the recordings it names, runs its analyses, and writes (or replaces) the
item's entry in docs/observations/<topic>.md. The marks are never changed;
a mark voided while marking is shown struck through and left out.

Item format:

    id = "briefing-1"
    topic = "briefing"                # the entry goes in docs/observations/briefing.md
    question = "..."                  # as in plan.md
    precision = "exact, durations approximate"   # as stated in plan.md
    markset = "briefing"
    method = "..."                    # how to record it
    notes = "..."                     # optional; copied into the entry
    pixel_error = 2.0                 # optional; ± px for each point mark
    quote = ["option"]                # optional; wording labels whose wording the
                                      # result depends on (see "Wording" below)
    recordings = [{ session = "2026-10-07-a", file = "briefing-m1.mkv" }]

    [[audio]]                         # optional; audio events as marks
    label = "voice"                   # the label its onsets get in the analyses
    ref = "<file>.wav"                # WAV file or sound file name, as for `onsets`
    threshold = 0.5                   # optional; lowest confidence used

    [[analysis]]
    name = "PROCEED"                  # heading of this result
    type = "duration"                 # count | order | duration | ratio | linear_fit | exp_tau
    kind = "approximate"              # optional; exact | approximate
    group = "start"                   # optional; a label that starts a new group
    ...                               # the type's own fields, below

    count       labels = [...]
    order       labels = [...]       (optional; all labels if absent)
    duration    from = "label", to = "label"
    ratio       numerator = Q, denominator = Q, unit = "...", as_angle = false
                where Q is { distance = [a, b] }, { dx = [a, b] }, { dy = [a, b] },
                { duration = [a, b] }, { value = label } or { count = label }
    linear_fit  label = "...", y = "value" | "x" | "y", unit = "..."
    exp_tau     label = "...", y = "value" | "x" | "y", unit = "..."

Audio. Each `[[audio]]` block runs `onsets` on every recording (found in
the evidence directory by name, size and SHA-256) and adds the onsets as
marks with its label. They are moved onto the video's clock by the
recording's measured audio − video offset (`probe --sync-ref … --record`),
which the entry states; without a measured offset the result refuses.

All marks, audio events included, are taken in frame order.

Wording. A markset key with `wording = true` records the game's own text in
its note, with `key: …` (the message key or reader control) and `desc: …`
(our own description) where known. The entry never copies such a note
whole (docs/CONVENTIONS.md, the copyright rule): it shows the key; failing
that, the description; failing that, the wording itself only for a label in
the item's `quote`, which is for items whose question is about the wording
and names only the labels the result depends on. Everything else reads
"(wording in the marks file)". The full wording stays in the marks file,
outside the repository; the entry records that file's SHA-256 and any
OpenTimestamps proof of it, so the two can be matched later.
"""

from __future__ import annotations

import math
import pathlib
import re
import statistics
import tomllib
from dataclasses import dataclass, field
from fractions import Fraction

from common import (NOT_OBSERVABLE, VOID, Mark, ObserveError, docs_dir, evidence_dir, items_dir, marks_path,
                    marksets_dir, numpy, probe_media, proofs_for, read_marks, sessions_dir, sha256_file, stamp)
from mark import load_markset, read_grab_hashes
from session import Session, read_session

DEFAULT_KIND = {"count": "exact", "order": "exact", "duration": "approximate",
                "ratio": "approximate", "linear_fit": "approximate", "exp_tau": "approximate"}
QUANTITIES = {"distance", "dx", "dy", "duration", "value", "count"}
_ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]*$")


# --------------------------------------------------------------------------
# items
# --------------------------------------------------------------------------


def _analysis_labels(a: dict) -> list[str]:
    t = a.get("type")
    labels = [a["group"]] if a.get("group") else []
    if t in ("count", "order"):
        labels += a.get("labels", [])
    elif t == "duration":
        labels += [a.get("from"), a.get("to")]
    elif t == "ratio":
        for q in (a.get("numerator", {}), a.get("denominator", {})):
            for v in q.values():
                labels += v if isinstance(v, list) else [v]
    elif t in ("linear_fit", "exp_tau"):
        labels.append(a.get("label"))
    return labels


def _check_quantity(q, where: str, problems: list[str]) -> None:
    if not isinstance(q, dict) or len(q) != 1 or next(iter(q)) not in QUANTITIES:
        problems.append(f"{where} must be one of {sorted(QUANTITIES)}")
        return
    kind, v = next(iter(q.items()))
    pair = kind in ("distance", "dx", "dy", "duration")
    if pair != (isinstance(v, list) and len(v) == 2):
        problems.append(f"{where}: {kind} takes {'two labels' if pair else 'one label'}")


def load_item(item_id: str) -> dict:
    path = items_dir() / f"{item_id}.toml"
    if not path.is_file():
        raise ObserveError(f"no item {path}")
    with open(path, "rb") as fh:
        item = tomllib.load(fh)
    problems = []
    for key in ("id", "topic", "question", "precision", "markset", "method"):
        if not isinstance(item.get(key), str) or not item[key].strip():
            problems.append(f"'{key}' is missing")
    if item.get("id") != item_id:
        problems.append(f"id should be '{item_id}'")
    if not _ID_RE.match(item.get("topic", "")):
        problems.append("topic must be lower-case words joined by '-'")
    for r in item.get("recordings", []):
        if not isinstance(r, dict) or set(r) != {"session", "file"}:
            problems.append(f"recording {r} needs exactly session and file")
    try:
        markset = load_markset(item.get("markset", ""))
        labels, wording = set(markset["labels"]), markset["wording"]
    except ObserveError as e:
        problems.append(str(e))
        labels, wording = set(), set()
    for au in item.get("audio", []):
        if not isinstance(au, dict) or not au.get("label") or not au.get("ref"):
            problems.append(f"audio {au} needs label and ref")
            continue
        unknown = set(au) - {"label", "ref", "threshold", "min_gap", "rate"}
        if unknown:
            problems.append(f"audio '{au['label']}' has unknown fields {sorted(unknown)}")
        if au["label"] in labels:
            problems.append(f"audio label '{au['label']}' is also a markset label")
        elif not _ID_RE.match(au["label"]):
            problems.append(f"audio label '{au['label']}' must be lower-case words joined by '-'")
        if labels:
            labels.add(au["label"])
    analyses = item.get("analysis", [])
    if not analyses:
        problems.append("no [[analysis]]")
    for a in analyses:
        where = f"analysis '{a.get('name', '?')}'"
        if not a.get("name"):
            problems.append("an analysis has no name")
        if a.get("type") not in DEFAULT_KIND:
            problems.append(f"{where}: type must be one of {sorted(DEFAULT_KIND)}")
            continue
        if a.get("kind", "exact") not in ("exact", "approximate"):
            problems.append(f"{where}: kind is exact or approximate")
        t = a["type"]
        if t == "count" and not a.get("labels"):
            problems.append(f"{where}: count needs labels")
        if t == "duration" and not (a.get("from") and a.get("to")):
            problems.append(f"{where}: duration needs from and to")
        if t == "ratio":
            _check_quantity(a.get("numerator"), f"{where} numerator", problems)
            _check_quantity(a.get("denominator"), f"{where} denominator", problems)
            if not problems and a.get("kind") == "exact" and shared_labels(a):
                problems.append(f"{where}: a ratio whose parts share marks is approximate")
        if t in ("linear_fit", "exp_tau") and (not a.get("label") or a.get("y") not in ("value", "x", "y")):
            problems.append(f"{where}: {t} needs label and y = value|x|y")
        for label in _analysis_labels(a):
            if labels and label not in labels:
                problems.append(f"{where}: label '{label}' is not in markset '{item.get('markset')}'")
    used = set()
    for a in analyses:
        used |= labels if a.get("type") == "order" and not a.get("labels") else set(_analysis_labels(a))
    for label in item.get("quote", []):
        if labels and label not in wording:
            problems.append(f"quote: '{label}' is not a wording label of markset '{item.get('markset')}'")
        elif labels and label not in used:
            problems.append(f"quote: no analysis uses '{label}', so the result does not depend on its wording")
    if problems:
        raise ObserveError(f"{path}: " + "; ".join(problems))
    return item


# --------------------------------------------------------------------------
# recordings and their marks
# --------------------------------------------------------------------------


@dataclass
class NoteView:
    """What an entry shows of a mark's note (see "Wording" above)."""

    wording: set[str] = field(default_factory=set)
    quote: set[str] = field(default_factory=set)

    def show(self, m: Mark) -> str:
        key = f"key: {m.key}" if m.key else None
        if m.label not in self.wording:
            return "; ".join(p for p in (m.text, key, m.desc) if p)
        if key:
            return key
        if m.desc:
            return m.desc
        if m.text and m.label in self.quote:
            return f"“{m.text}”"
        return "(wording in the marks file)" if m.text else ""

    def cell(self, m: Mark) -> str:
        """The note for the raw readings: shown as above, plus the position
        and frame grab the tool wrote."""
        pos = f"x={m.pos[0]:.1f} y={m.pos[1]:.1f}" if m.pos else None
        grab = f"grab={m.grab}" if m.grab else None
        return "; ".join(p for p in (self.show(m), pos, grab) if p)


@dataclass
class Recording:
    index: int
    session: Session
    file: str
    sha256: str
    marks_path: pathlib.Path
    marks_sha256: str
    marks: list[Mark]
    video_fps: Fraction
    game_fps: float | None
    grabs: dict[str, str] = field(default_factory=dict)
    proofs: list[str] = field(default_factory=list)
    view: NoteView = field(default_factory=NoteView)

    audio: list[Mark] = field(default_factory=list)
    offset: tuple[float, float] | None = None

    @property
    def live(self) -> list[Mark]:
        """Marks in force and audio events, in frame order (stable)."""
        marks = [m for m in self.marks if m.label != VOID and m.voided_by is None] + self.audio
        return sorted(marks, key=lambda m: m.frame)

    def seconds(self, frames: int) -> float:
        return frames / float(self.video_fps)

    @property
    def frame_error(self) -> float:
        """± one game frame in seconds (one recorded frame if not measured)."""
        return 1 / self.game_fps if self.game_fps else 1 / float(self.video_fps)


def load_recordings(item: dict, view: NoteView) -> list[Recording]:
    recs = []
    for i, r in enumerate(item.get("recordings", []), start=1):
        session = read_session(r["session"])
        row = session.recording(r["file"])
        if not row:
            raise ObserveError(f"session {session.id} does not list {r['file']}")
        path = marks_path(session.id, r["file"], item["markset"])
        if not path.is_file():
            raise ObserveError(f"no marks {path}; run: observe mark <recording> --set {item['markset']}")
        fps = session.video_fps(r["file"])
        if not fps:
            raise ObserveError(f"session {session.id} gives no video frame rate for {r['file']}")
        digest = sha256_file(path)
        recs.append(Recording(i, session, r["file"], row.get("SHA-256", ""), path, digest,
                              read_marks(path), fps, session.game_fps(r["file"]),
                              read_grab_hashes(path), [p.name for p in proofs_for(path, digest, sessions_dir())],
                              view))
    if item.get("audio"):
        for rec in recs:
            _add_audio(rec, item["audio"])
    return recs


def locate_recording(rec: "Recording") -> pathlib.Path:
    """The recording in the evidence directory: by name, size and SHA-256."""
    size = int((rec.session.recording(rec.file) or {}).get("Size (bytes)", "-1") or -1)
    for path in sorted(evidence_dir().rglob(rec.file)):
        if path.is_file() and path.stat().st_size == size and sha256_file(path) == rec.sha256:
            return path
    raise ObserveError(f"{rec.file} (SHA-256 {rec.sha256[:12]}…) is not in the evidence directory")


def _add_audio(rec: "Recording", blocks: list[dict]) -> None:
    """Find each [[audio]] reference's onsets and add them as marks on the
    video's clock: onset time − the measured audio − video offset."""
    import audio
    rec.offset = rec.session.av_offset(rec.file)
    if rec.offset is None:
        raise ObserveError(f"the item uses audio events, but session {rec.session.id} has no measured "
                           f"audio − video offset for {rec.file}; mark sync events (markset 'sync') and run "
                           "observe probe <recording> --sync-ref <sound> --record")
    media = probe_media(locate_recording(rec))
    for block in blocks:
        rate = int(block.get("rate", 24000))
        ref = audio.load_ref(block["ref"], rate)
        for o in audio.onsets(media, ref, rate, float(block.get("threshold", 0.5)), block.get("min_gap")):
            t = o.time - rec.offset[0]
            rec.audio.append(Mark(len(rec.audio) + 1, media.frame_at(t), t, block["label"],
                                  f"onset of {pathlib.Path(block['ref']).name} at {o.time:.4f} s in the "
                                  f"audio, confidence {o.confidence:.2f}", audio=True))


def groups(rec: Recording, group: str | None) -> list[tuple[str, list[Mark]]]:
    """The recording's live marks, split where a `group` mark starts a new
    group; the group is named by its mark's note, else numbered."""
    marks = [m for m in rec.live if m.label != NOT_OBSERVABLE]
    if not group:
        return [(f"recording {rec.index}", marks)]
    out: list[tuple[str, list[Mark]]] = []
    count = 0
    for m in marks:
        if m.label == group:
            count += 1
            out.append((f"{rec.index}: {rec.view.show(m) or f'#{count}'}", [m]))
        elif out:
            out[-1][1].append(m)
        else:
            out.append((f"{rec.index}: (before any {group})", [m]))
    return out


# --------------------------------------------------------------------------
# formatting
# --------------------------------------------------------------------------


def fmt(v: float, u: float | None = None, unit: str = "") -> str:
    unit = f" {unit}" if unit else ""
    if u is None or u <= 0 or not math.isfinite(u):
        return f"{v:.4g}{unit}"
    digits = max(0, 1 - math.floor(math.log10(u)))
    return f"{v:.{digits}f} ± {u:.{digits}f}{unit}"


def _cell(text: str) -> str:
    return text.replace("|", "\\|").replace("\n", " ")


@dataclass
class Outcome:
    name: str
    type: str
    kind: str
    lines: list[str]
    observed: bool


# --------------------------------------------------------------------------
# analyses
# --------------------------------------------------------------------------


def _count(a, recs) -> Outcome:
    labels = a["labels"]
    totals = dict.fromkeys(labels, 0)
    per_group = []
    for rec in recs:
        for name, marks in groups(rec, a.get("group")):
            counts = {lab: sum(1 for m in marks if m.label == lab) for lab in labels}
            for lab in labels:
                totals[lab] += counts[lab]
            per_group.append(f"{name}: " + ", ".join(f"{lab} {counts[lab]}" for lab in labels))
    n = len(per_group)
    lines = [", ".join(f"{lab} {totals[lab]}" for lab in labels) +
             f" (over {n} {'group' if a.get('group') else 'recording'}{'s' if n != 1 else ''})"]
    if a.get("group") or len(recs) > 1:
        lines += [f"  - {g}" for g in per_group]
    return Outcome(a["name"], "count", a.get("kind", "exact"), lines, True)


def _describe(m: Mark, view: NoteView) -> str:
    note = view.show(m)
    return f"{m.label} ({note})" if note else m.label


def _order(a, recs) -> Outcome:
    labels = set(a.get("labels", []))
    seqs = []
    for rec in recs:
        for name, marks in groups(rec, a.get("group")):
            seq = [m for m in marks if not labels or m.label in labels]
            if seq:
                seqs.append((name, seq, rec.view))
    if not seqs:
        return Outcome(a["name"], "order", a.get("kind", "exact"), ["nothing marked"], False)
    shown = [f"{name}: " + " → ".join(_describe(m, view) for m in seq) for name, seq, view in seqs]
    if len(seqs) == 1:
        return Outcome(a["name"], "order", a.get("kind", "exact"), shown, True)
    plain = {tuple(m.label for m in seq) for _, seq, _ in seqs}
    head = (f"the same order in all {len(seqs)}: " + " → ".join(next(iter(plain))) if len(plain) == 1
            else f"the order differs between the {len(seqs)}")
    lines = [head] + [f"  - {s}" for s in shown]
    return Outcome(a["name"], "order", a.get("kind", "exact"), lines, True)


def _pairs(marks: list[Mark], start: str, end: str) -> list[tuple[Mark, Mark]]:
    """Each `start` mark with the first unused `end` mark at or after it
    (successive marks when start == end)."""
    if start == end:
        same = [m for m in marks if m.label == start]
        return list(zip(same, same[1:]))
    ends = [m for m in marks if m.label == end]
    used: set[int] = set()  # id() of the end marks taken
    out = []
    for s in (m for m in marks if m.label == start):
        e = next((e for e in ends if e.frame >= s.frame and id(e) not in used), None)
        if e:
            used.add(id(e))
            out.append((s, e))
    return out


def _duration(a, recs) -> Outcome:
    values, lines, err = [], [], 0.0
    for rec in recs:
        for name, marks in groups(rec, a.get("group")):
            for s, e in _pairs(marks, a["from"], a["to"]):
                d = rec.seconds(e.frame - s.frame)
                err = max(err, rec.frame_error)
                values.append(d)
                gf = f", {d * rec.game_fps:.1f} game frames" if rec.game_fps else ""
                lines.append(f"  - {name}: rows {s.ref}→{e.ref}, {e.frame - s.frame} recorded frames = "
                             f"{fmt(d, rec.frame_error, 's')}{gf}")
    if not values:
        return Outcome(a["name"], "duration", a.get("kind", "approximate"),
                       [f"no `{a['from']}` followed by `{a['to']}` was marked"], False)
    head = f"{fmt(statistics.mean(values), err, 's')} (± one game frame)"
    if len(values) > 1:
        head = (f"mean {fmt(statistics.mean(values), err, 's')} (± one game frame), n = {len(values)}, "
                f"range {min(values):.3f}–{max(values):.3f} s, sd {statistics.stdev(values):.3f} s")
    return Outcome(a["name"], "duration", a.get("kind", "approximate"), [head] + lines, True)


def _last(marks: list[Mark], label: str) -> Mark | None:
    found = [m for m in marks if m.label == label]
    return found[-1] if found else None


def _quantity(q: dict, marks: list[Mark], rec: Recording, pixel_error: float):
    """(value, uncertainty, unit) of a quantity within one group, or None."""
    kind, v = next(iter(q.items()))
    if kind in ("distance", "dx", "dy"):
        a, b = _last(marks, v[0]), _last(marks, v[1])
        if not (a and b and a.pos and b.pos):
            return None
        dx, dy = b.pos[0] - a.pos[0], b.pos[1] - a.pos[1]
        value = {"distance": math.hypot(dx, dy), "dx": abs(dx), "dy": abs(dy)}[kind]
        return value, math.sqrt(2) * pixel_error, "px"
    if kind == "duration":
        pairs = _pairs(marks, v[0], v[1])
        if not pairs:
            return None
        s, e = pairs[-1]
        return rec.seconds(e.frame - s.frame), rec.frame_error, "s"
    if kind == "value":
        m = _last(marks, v)
        return (m.value, 0.0, "") if m and m.value is not None else None
    return float(sum(1 for m in marks if m.label == v)), 0.0, ""


def _qname(q: dict) -> str:
    kind, v = next(iter(q.items()))
    return f"{kind}({', '.join(v) if isinstance(v, list) else v})"


def shared_labels(a: dict) -> list[str]:
    """Marks a ratio's numerator and denominator both use: their errors are
    correlated, which the combined ± ignores."""
    def labels(q):
        v = next(iter(q.values()))
        return set(v) if isinstance(v, list) else {v}
    return sorted(labels(a["numerator"]) & labels(a["denominator"]))


def _ratio(a, recs, pixel_error: float) -> Outcome:
    by_name: dict[str, list[tuple[float, float]]] = {}
    lines = []
    shared = shared_labels(a)
    kind = "approximate" if shared else a.get("kind", "approximate")
    for rec in recs:
        for name, marks in groups(rec, a.get("group")):
            n = _quantity(a["numerator"], marks, rec, pixel_error)
            d = _quantity(a["denominator"], marks, rec, pixel_error)
            if not n or not d or d[0] == 0:
                lines.append(f"  - {name}: incomplete (needs {_qname(a['numerator'])} and {_qname(a['denominator'])})")
                continue
            r = n[0] / d[0]
            rel = math.hypot(n[1] / n[0] if n[0] else 0.0, d[1] / d[0])
            u = abs(r) * rel
            shown = fmt(r, u, a.get("unit", ""))
            if a.get("as_angle"):
                ang = math.degrees(math.atan(r))
                shown += f" = {fmt(ang, math.degrees(u / (1 + r * r)), '°')}"
            lines.append(f"  - {name}: {shown} ({fmt(n[0], n[1], n[2])} / {fmt(d[0], d[1], d[2])})")
            by_name.setdefault(name.split(": ", 1)[-1], []).append((r, u))
    if not by_name:
        return Outcome(a["name"], "ratio", kind, ["nothing complete was marked"] + lines, False)
    head = []
    for name, vals in by_name.items():
        rs = [v for v, _ in vals]
        u = max(max(e for _, e in vals), statistics.stdev(rs) if len(rs) > 1 else 0.0)
        head.append(f"{name}: {fmt(statistics.mean(rs), u, a.get('unit', ''))}"
                    + (f" (mean of {len(rs)})" if len(rs) > 1 else ""))
    text = "; ".join(head)
    if shared:
        text += (f" (numerator and denominator share {', '.join(f'`{x}`' for x in shared)}; the ± treats "
                 "their errors as independent, so it is approximate)")
    return Outcome(a["name"], "ratio", kind, [text] + lines, True)


def _points(a, marks: list[Mark], rec: Recording):
    pts = []
    first = None
    for m in marks:
        if m.label != a["label"]:
            continue
        if a["y"] == "value":
            y = m.value
        else:
            y = m.pos[0 if a["y"] == "x" else 1] if m.pos else None
        if y is None:
            continue
        first = m.frame if first is None else first
        pts.append((rec.seconds(m.frame - first), y))
    return pts


def _linear_fit(a, recs) -> Outcome:
    np = numpy()
    lines = []
    fitted = 0
    unit = a.get("unit", "")
    for rec in recs:
        for name, marks in groups(rec, a.get("group")):
            pts = _points(a, marks, rec)
            if len(pts) < 3:
                lines.append(f"  - {name}: {len(pts)} point(s), a line needs 3")
                continue
            x, y = np.array(pts).T
            design = np.vstack([x, np.ones_like(x)]).T
            (slope, icpt), res, *_ = np.linalg.lstsq(design, y, rcond=None)
            resid = y - (slope * x + icpt)
            s2 = float(resid @ resid) / (len(x) - 2)
            cov = s2 * np.linalg.inv(design.T @ design)
            ss = float(((y - y.mean()) ** 2).sum())
            r2 = 1 - float(resid @ resid) / ss if ss else 1.0
            per_s = f"{unit}/s" if unit else "/s"
            lines.append(f"  - {name}: slope {fmt(slope, math.sqrt(cov[0, 0]), per_s)}, "
                         f"at the first mark {fmt(icpt, math.sqrt(cov[1, 1]), unit)}, R² {r2:.4f}, n = {len(x)}")
            fitted += 1
    head = "least-squares line, time from each group's first mark" if fitted else "too few points"
    return Outcome(a["name"], "linear_fit", a.get("kind", "approximate"), [head] + lines, fitted > 0)


def fit_exp(x, y):
    """y = a + b·exp(−(x − x₀)/τ): (τ, τ_low, τ_high, a, b, at the search's
    edge). The τ range is where the residual sum of squares stays within
    SSE_min·(1 + 1/(n−3))."""
    np = numpy()
    span = float(x.max() - x.min())
    steps = np.diff(np.unique(x))
    lo = float(steps.min()) / 10 if len(steps) else span / 100

    def sse(tau):
        design = np.vstack([np.ones_like(x), np.exp(-(x - x.min()) / tau)]).T
        coef, *_ = np.linalg.lstsq(design, y, rcond=None)
        r = y - design @ coef
        return float(r @ r), coef

    taus = np.geomspace(lo, span * 20, 600)
    errs = np.array([sse(t)[0] for t in taus])
    k = int(np.argmin(errs))
    a_, b_ = taus[max(0, k - 1)], taus[min(len(taus) - 1, k + 1)]
    for _ in range(60):  # golden section on log τ
        m1 = math.exp(math.log(a_) + 0.382 * (math.log(b_) - math.log(a_)))
        m2 = math.exp(math.log(a_) + 0.618 * (math.log(b_) - math.log(a_)))
        if sse(m1)[0] < sse(m2)[0]:
            b_ = m2
        else:
            a_ = m1
    tau = math.sqrt(a_ * b_)
    best, coef = sse(tau)
    limit = best * (1 + 1 / max(1, len(x) - 3))
    inside = taus[errs <= limit]
    low = float(min(inside.min(), tau)) if len(inside) else tau
    high = float(max(inside.max(), tau)) if len(inside) else tau
    return tau, low, high, float(coef[0]), float(coef[1]), k in (0, len(taus) - 1)


def _exp_tau(a, recs) -> Outcome:
    np = numpy()
    lines = []
    fitted = 0
    unit = a.get("unit", "")
    for rec in recs:
        for name, marks in groups(rec, a.get("group")):
            pts = _points(a, marks, rec)
            if len(pts) < 4:
                lines.append(f"  - {name}: {len(pts)} point(s), the fit needs 4")
                continue
            x, y = np.array(pts).T
            tau, low, high, a0, b0, edge = fit_exp(x, y)
            text = (f"  - {name}: τ = {tau:.4g} s (range {low:.4g}–{high:.4g} s), "
                    f"settles at {fmt(a0, None, unit)}, starts {fmt(a0 + b0, None, unit)}, n = {len(x)}")
            if edge:
                text += "; the best τ is at the edge of the search: no exponential is evident"
            lines.append(text)
            fitted += 1
    head = "least squares of y = a + b·exp(−t/τ), t from each group's first mark" if fitted else "too few points"
    return Outcome(a["name"], "exp_tau", a.get("kind", "approximate"), [head] + lines, fitted > 0)


def run_analysis(a: dict, recs: list[Recording], pixel_error: float) -> Outcome:
    t = a["type"]
    if t == "count":
        return _count(a, recs)
    if t == "order":
        return _order(a, recs)
    if t == "duration":
        return _duration(a, recs)
    if t == "ratio":
        return _ratio(a, recs, pixel_error)
    if t == "linear_fit":
        return _linear_fit(a, recs)
    return _exp_tau(a, recs)


def describe_analysis(a: dict, pixel_error: float) -> str:
    t = a["type"]
    per = f", per group started by `{a['group']}`" if a.get("group") else ""
    if t == "count":
        return f"count of the marks {', '.join(f'`{lab}`' for lab in a['labels'])}{per}"
    if t == "order":
        which = ", ".join(f"`{lab}`" for lab in a.get("labels", [])) or "all marks"
        return f"order of {which}{per}"
    if t == "duration":
        return (f"time from each `{a['from']}` to the next `{a['to']}`{per}, from recorded frame "
                f"numbers; ± one game frame at the session's measured rate")
    if t == "ratio":
        angle = "; also as the angle whose tangent it is" if a.get("as_angle") else ""
        return (f"{_qname(a['numerator'])} / {_qname(a['denominator'])}{per}; point marks ± {pixel_error:g} px, "
                f"durations ± one game frame{angle}")
    y = {"value": "the number in the note", "x": "the mark's x position", "y": "the mark's y position"}[a["y"]]
    model = "a least-squares line" if t == "linear_fit" else "a least-squares fit of a + b·exp(−t/τ)"
    return f"{model} through (time, {y}) of the `{a['label']}` marks{per}"


# --------------------------------------------------------------------------
# the entry
# --------------------------------------------------------------------------


def begin_marker(item_id: str) -> str:
    return f"<!-- observe:begin {item_id} -->"


def end_marker(item_id: str) -> str:
    return f"<!-- observe:end {item_id} -->"


def render_entry(item: dict, recs: list[Recording], outcomes: list[Outcome], markset: dict) -> str:
    pixel_error = float(item.get("pixel_error", 2.0))
    out = [begin_marker(item["id"]), f"## {item['id']}", ""]
    out += [f"**Question.** {item['question'].strip()}", "",
            f"**Stated precision.** {item['precision']}", ""]
    out += [f"**Method.** {item['method'].strip()}", "",
            f"Marked with the markset `{markset['name']}`: "
            + "; ".join(f"`{k['label']}` ({k['help']})" for k in markset["key"]) + ".", ""]
    for a in item["analysis"]:
        out.append(f"- *{a['name']}*: {describe_analysis(a, pixel_error)}.")
    out += ["", "**Recordings.** The recordings and marks files stay in the evidence directory, outside the "
            "repository; the hashes identify them, and a marks file's timestamp proof sits beside the session "
            "file.", "",
            "| # | Session | File | SHA-256 | Marks | Marks SHA-256 | Marks timestamp | Video fps | Game fps |",
            "|---|---|---|---|---|---|---|---|---|"]
    notes = []
    for r in recs:
        game = f"{r.game_fps:.2f}" if r.game_fps else "not measured"
        if not r.game_fps:
            notes.append(f"Recording {r.index}: the game's frame rate was not measured; "
                         "durations carry ± one recorded frame instead of one game frame.")
        proof = ", ".join(r.proofs) if r.proofs else "none"
        out.append(f"| {r.index} | [{r.session.id}](sessions/{r.session.id}.md) | {_cell(r.file)} | `{r.sha256}` | "
                   f"{r.marks_path.name} | `{r.marks_sha256}` | {proof} | {r.video_fps} | {game} |")
    grabs = [(r, m.grab) for r in recs for m in r.marks if m.grab]
    if grabs:
        out += ["", "Frame grabs (outside the repository):", ""]
        for r, g in grabs:
            digest = r.grabs.get(g)
            out.append(f"- {r.index}: {g} " + (f"`{digest}`" if digest else "(not hashed)"))
    if any(r.offset is not None for r in recs):
        out += ["", "**Audio events.** Found with `observe onsets` and moved onto the video's clock by "
                "subtracting each recording's measured audio − video offset: "
                + "; ".join(f"recording {r.index} {r.offset[0]:+.4f} ± {r.offset[1]:.4f} s" for r in recs
                            if r.offset is not None)
                + ". A duration between an audio event and a mark also carries that offset's uncertainty.", "",
                "| # | Event | Label | Frame | Time (s) | Note |", "|---|---|---|---|---|---|"]
        for r in recs:
            for m in r.audio:
                out.append(f"| {r.index} | {m.ref} | {m.label} | {m.frame} | {m.time:.3f} | {_cell(m.note)} |")
            if r.offset is not None and not r.audio:
                out.append(f"| {r.index} | | | | | (no onsets found) |")
    raw = "**Raw readings.** Every row of the marks files, as written while marking."
    if markset["wording"]:
        raw += (" Notes that record the game's wording show its key or a description instead"
                + (f", or the wording itself for {', '.join(f'`{q}`' for q in sorted(item.get('quote', [])))}"
                   if item.get("quote") else "")
                + " (docs/CONVENTIONS.md, the copyright rule). The full wording is in the marks files,"
                  " which the hashes above identify.")
    out += ["", raw, "",
            "| # | Row | Frame | Time (s) | Label | Note |", "|---|---|---|---|---|---|"]
    for r in recs:
        for m in r.marks:
            note = _cell(r.view.cell(m))
            row = f"{m.row} | {m.frame} | {m.time:.3f} | {m.label} | {note}"
            if m.voided_by:
                voided = f"{note} (voided by row {m.voided_by})".lstrip()
                row = f"~~{m.row}~~ | ~~{m.frame}~~ | ~~{m.time:.3f}~~ | ~~{m.label}~~ | {voided}"
            out.append(f"| {r.index} | {row} |")
        if not r.marks:
            out.append(f"| {r.index} | | | | (no marks) | |")
    out += ["", "**Result.**", ""]
    for o in outcomes:
        kind = o.kind if o.observed else "not observable"
        out.append(f"- *{o.name}* ({kind}): {o.lines[0]}")
        out += [f"  {line}" for line in o.lines[1:]]
    kinds: dict[str, list[str]] = {}
    for o in outcomes:
        kinds.setdefault(o.kind if o.observed else "not observable", []).append(o.name)
    out += ["", "**Kind.** " + "; ".join(f"{k} ({', '.join(v)})" for k, v in kinds.items()), ""]
    unseen = [(r, m) for r in recs for m in r.live if m.label == NOT_OBSERVABLE]
    missing = [o.name for o in outcomes if not o.observed]
    out.append("**Not observable.**" + ("" if unseen or missing else " Nothing was marked as not observable."))
    if unseen or missing:
        out.append("")
        out += [f"- recording {r.index}, frame {m.frame}: {r.view.show(m) or '(no note)'}" for r, m in unseen]
        out += [f"- *{name}*: nothing in the recordings measured it." for name in missing]
    out.append("")
    if item.get("notes", "").strip():
        notes.insert(0, item["notes"].strip())
    out.append("**Notes.** " + (" ".join(notes) if notes else "None."))
    out += ["", end_marker(item["id"])]
    return "\n".join(out)


def topic_header(topic: str) -> str:
    return (f"# Observations: {topic}\n\n"
            "Entries are written by `tools/observe/observe.py result <item>` from the item files in\n"
            "`tools/observe/items/` and the marks of the recordings they name. To change an entry,\n"
            "change the item or record again, then rerun; never edit a reading. Method:\n"
            "[README.md](README.md).\n")


def write_entry(topic: str, item_id: str, entry: str) -> pathlib.Path:
    path = docs_dir() / f"{topic}.md"
    text = path.read_text() if path.exists() else topic_header(topic)
    pattern = re.compile(re.escape(begin_marker(item_id)) + r".*?" + re.escape(end_marker(item_id)), re.S)
    if pattern.search(text):
        text = pattern.sub(lambda _: entry, text, count=1)
    else:
        text = text.rstrip("\n") + "\n\n" + entry + "\n"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    return path


def result(args) -> None:
    item = load_item(args.item)
    markset = load_markset(item["markset"])
    if not item.get("recordings"):
        raise ObserveError(f"item {item['id']} lists no recordings yet; nothing to write")
    recs = load_recordings(item, NoteView(markset["wording"], set(item.get("quote", []))))
    pixel_error = float(item.get("pixel_error", 2.0))
    outcomes = [run_analysis(a, recs, pixel_error) for a in item["analysis"]]
    entry = render_entry(item, recs, outcomes, markset)
    if args.print:
        print(entry)
        return
    path = write_entry(item["topic"], item["id"], entry)
    print(f"wrote {item['id']} into {path}")
    stamp(path)


def check(args) -> None:
    """Load every markset and item, so a broken one shows before a session."""
    failures = 0
    for path in sorted(marksets_dir().glob("*.toml")):
        try:
            load_markset(path.stem)
        except ObserveError as e:
            failures += 1
            print(f"FAIL {e}")
    items = sorted(items_dir().glob("*.toml"))
    for path in items:
        try:
            item = load_item(path.stem)
            print(f"ok   {item['id']:<14} {item['markset']:<18} -> {item['topic']}.md, "
                  f"{len(item['analysis'])} analysis(es), {len(item.get('recordings', []))} recording(s)")
        except ObserveError as e:
            failures += 1
            print(f"FAIL {e}")
    if failures:
        raise ObserveError(f"{failures} file(s) failed")
