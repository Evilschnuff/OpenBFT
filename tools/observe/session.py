"""`observe session`: the session file that ties recordings to their hashes.

A session is one sitting at the original game. Its file,
docs/observations/sessions/<id>.md, records when, on what, and which
recordings were made (file name, size, SHA-256: the files stay in the
evidence directory, outside the repository), the game's measured frame rate
with the thresholds used to measure it, and each recording's audio − video
offset once measured.

A session file is never rewritten: `session add` appends recordings, and
`probe --record` fills in a recording's offset cell. Each version can be
timestamped (`session stamp`).
"""

from __future__ import annotations

import datetime
import pathlib
import re
import statistics
import string
from dataclasses import dataclass, field
from fractions import Fraction

from common import (TOOL_DIR, ObserveError, in_evidence, need_video, probe_media, sessions_dir,
                    sha256_file, stamp)

TEMPLATE = TOOL_DIR / "templates" / "session.md"
FILL_IN = "(fill in)"
NOT_MEASURED = "(not measured)"
FPS_FIELD = "Game frame rate (measured)"
THRESHOLDS_FIELD = "Frame-change thresholds"
OFFSET_COLUMN = "A/V offset (s)"
# probe's defaults: a frame is new when more than `fraction` of its pixels
# change by more than `pixel_threshold` grey levels; a one-second window with
# at least `min_rate` new pictures counts as motion
DEFAULT_THRESHOLDS = {"pixel_threshold": 6, "fraction": 0.002, "min_rate": 10.0}
_FIELD_RE = re.compile(r"^- ([^:]+): (.*)$")
_FPS_RE = re.compile(r"(\d+)/(\d+) fps")
_NUM_RE = re.compile(r"[-+]?\d+(?:\.\d+)?")
_PAIR_RE = re.compile(r"(pixel_threshold|fraction|min_rate)=([0-9.]+)")


@dataclass
class Session:
    id: str
    path: pathlib.Path
    fields: dict[str, str] = field(default_factory=dict)
    recordings: list[dict[str, str]] = field(default_factory=list)

    def recording(self, name: str) -> dict[str, str] | None:
        return next((r for r in self.recordings if r["File"] == name), None)

    def game_fps(self, name: str) -> float | None:
        """The game's frame rate for a recording: its own probe, else the
        session's measured value."""
        rec = self.recording(name) or {}
        for text in (rec.get("Game fps", ""), self.fields.get(FPS_FIELD, "")):
            m = _NUM_RE.search(text)
            if m and float(m.group(0)) > 0:
                return float(m.group(0))
        return None

    def video_fps(self, name: str) -> Fraction | None:
        rec = self.recording(name) or {}
        m = _FPS_RE.search(rec.get("Video", ""))
        return Fraction(int(m.group(1)), int(m.group(2))) if m else None

    def av_offset(self, name: str) -> tuple[float, float] | None:
        """(audio − video offset, its uncertainty) in seconds, if measured."""
        nums = _NUM_RE.findall((self.recording(name) or {}).get(OFFSET_COLUMN, ""))
        return (float(nums[0]), float(nums[1]) if len(nums) > 1 else 0.0) if nums else None

    def thresholds(self) -> dict[str, float] | None:
        pairs = dict(_PAIR_RE.findall(self.fields.get(THRESHOLDS_FIELD, "")))
        return {k: float(v) for k, v in pairs.items()} if len(pairs) == 3 else None


def session_path(session_id: str) -> pathlib.Path:
    return sessions_dir() / f"{session_id}.md"


def read_session(session_id: str) -> Session:
    path = session_path(session_id)
    if not path.is_file():
        raise ObserveError(f"no session file {path}")
    session = Session(session_id, path)
    header: list[str] | None = None
    for line in path.read_text().splitlines():
        if m := _FIELD_RE.match(line):
            session.fields.setdefault(m.group(1).strip(), m.group(2).strip())
        elif line.startswith("| File |"):
            header = [c.strip() for c in line.strip().strip("|").split("|")]
        elif header and line.startswith("|") and not line.startswith("|---"):
            cells = [c.strip() for c in line.strip().strip("|").split("|")]
            session.recordings.append(dict(zip(header, cells)))
        elif header and not line.startswith("|"):
            header = None
    return session


def all_sessions() -> list[Session]:
    return [read_session(p.stem) for p in sorted(sessions_dir().glob("*.md"))]


def find_session(recording: str, session_id: str | None) -> Session:
    """The session whose recordings table lists this file name."""
    if session_id:
        session = read_session(session_id)
        if not session.recording(recording):
            raise ObserveError(f"session {session_id} does not list {recording}; "
                               f"add it with: observe session add {session_id} --rec <path>")
        return session
    found = [s for s in all_sessions() if s.recording(recording)]
    if not found:
        raise ObserveError(f"no session lists {recording}; create one with: observe session new --rec <path>")
    if len(found) > 1:
        raise ObserveError(f"{recording} is listed by sessions {', '.join(s.id for s in found)}; pass --session")
    return found[0]


def describe_thresholds(t: dict[str, float]) -> str:
    return (f"pixel_threshold={t['pixel_threshold']:g}, fraction={t['fraction']:g}, min_rate={t['min_rate']:g} "
            f"(a recorded frame is new when more than {t['fraction'] * 100:g} % of its pixels change by more "
            f"than {t['pixel_threshold']:g} grey levels; a second with at least {t['min_rate']:g} new "
            f"pictures counts as motion)")


def thresholds_from(args, session: Session | None = None) -> dict[str, float]:
    """The thresholds asked for; a session already measured keeps its own."""
    asked = {k: getattr(args, k) for k in DEFAULT_THRESHOLDS if getattr(args, k, None) is not None}
    own = session.thresholds() if session else None
    if own:
        differ = {k: v for k, v in asked.items() if float(v) != own[k]}
        if differ:
            raise ObserveError(f"session {session.id} measured with {describe_thresholds(own)}; "
                               "start a new session to measure with other thresholds")
        return own
    return {**DEFAULT_THRESHOLDS, **asked}


def _next_id(date: str) -> str:
    for letter in "abcdefghijklmnopqrstuvwxyz":
        if not session_path(f"{date}-{letter}").exists():
            return f"{date}-{letter}"
    raise ObserveError(f"too many sessions on {date}; pass --id")


def _recording_row(path: pathlib.Path, thresholds: dict[str, float] | None) -> tuple[str, float | None]:
    in_evidence(path, "recording")
    media = probe_media(path)
    game_fps = None
    if thresholds and media.fps:
        import video
        need_video(media)
        print(f"  measuring the game's frame rate in {path.name} ...")
        game_fps = video.measure_frame_rate(media, min_rate=thresholds["min_rate"],
                                            pixel_threshold=int(thresholds["pixel_threshold"]),
                                            fraction=thresholds["fraction"]).fps
    print(f"  hashing {path.name} ...")
    digest = sha256_file(path)
    fps_cell = f"{game_fps:.2f}" if game_fps else NOT_MEASURED
    row = (f"| {path.name} | {path.stat().st_size} | {digest} | {media.video_desc()} | "
           f"{media.audio_desc()} | {fps_cell} | {NOT_MEASURED} |")
    return row, game_fps


def new(args) -> None:
    date = args.date or datetime.date.today().isoformat()
    session_id = args.id or _next_id(date)
    path = session_path(session_id)
    if path.exists():
        raise ObserveError(f"{path} exists; sessions are not rewritten (use 'session add')")
    thresholds = None if args.no_probe else thresholds_from(args)
    rows, rates = [], []
    names = [pathlib.Path(r).name for r in args.rec]
    if len(set(names)) != len(names):
        raise ObserveError("two recordings share a file name; rename one")
    for rec in args.rec:
        row, fps = _recording_row(pathlib.Path(rec), thresholds)
        rows.append(row)
        if fps:
            rates.append(fps)
    text = string.Template(TEMPLATE.read_text()).substitute(
        id=session_id,
        date=date,
        game_version=args.game_version or FILL_IN,
        os=args.os or FILL_IN,
        resolution=args.resolution or FILL_IN,
        game_fps=(f"{args.game_fps:.2f} fps" if args.game_fps else
                  f"{statistics.median(rates):.2f} fps" if rates else NOT_MEASURED),
        thresholds=describe_thresholds(thresholds) if thresholds else NOT_MEASURED,
        recorder=args.recorder or FILL_IN,
        recordings="\n".join(rows),
        notes=args.notes or FILL_IN,
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text)
    print(f"wrote {path}")
    stamp(path)


def _table(lines: list[str]) -> tuple[int, int, list[str]]:
    """(header line, end of the table, column names)."""
    try:
        header = next(i for i, line in enumerate(lines) if line.startswith("| File |"))
    except StopIteration:
        raise ObserveError("the session file has no recordings table") from None
    end = header + 1
    while end < len(lines) and lines[end].startswith("|"):
        end += 1
    return header, end, [c.strip() for c in lines[header].strip().strip("|").split("|")]


def add(args) -> None:
    session = read_session(args.id)
    thresholds = None if args.no_probe else thresholds_from(args, session)
    if thresholds and not session.thresholds():
        raise ObserveError(f"session {session.id} was made without measuring; add with --no-probe")
    lines = session.path.read_text().splitlines()
    _, end, _ = _table(lines)
    new_rows = []
    for rec in args.rec:
        name = pathlib.Path(rec).name
        if session.recording(name):
            raise ObserveError(f"{session.id} already lists {name}")
        new_rows.append(_recording_row(pathlib.Path(rec), thresholds)[0])
    lines[end:end] = new_rows
    session.path.write_text("\n".join(lines) + "\n")
    print(f"added {len(new_rows)} recording(s) to {session.path}")
    stamp(session.path)


def record_offset(session: Session, name: str, offset: float, error: float, events: int) -> None:
    """Fill in a recording's audio − video offset; a measured one is kept."""
    lines = session.path.read_text().splitlines()
    header, end, columns = _table(lines)
    if OFFSET_COLUMN not in columns:
        raise ObserveError(f"{session.path} has no '{OFFSET_COLUMN}' column")
    col = columns.index(OFFSET_COLUMN)
    for i in range(header + 2, end):
        cells = [c.strip() for c in lines[i].strip().strip("|").split("|")]
        if cells[0] != name:
            continue
        if cells[col] != NOT_MEASURED:
            raise ObserveError(f"{session.id} already gives {name} an offset ({cells[col]}); "
                               "it is not rewritten")
        cells[col] = f"{offset:+.4f} ± {error:.4f} ({events} sync event{'s' if events != 1 else ''})"
        lines[i] = "| " + " | ".join(cells) + " |"
        session.path.write_text("\n".join(lines) + "\n")
        print(f"recorded the offset for {name} in {session.path}")
        stamp(session.path)
        return
    raise ObserveError(f"session {session.id} does not list {name}")


def stamp_cmd(args) -> None:
    session = read_session(args.id)
    if not stamp(session.path):
        print("nothing stamped: install OpenTimestamps (pip install opentimestamps-client) and go online")
