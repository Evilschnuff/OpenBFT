"""Shared pieces of the observation tools: paths, media probing and decoding,
marks files, hashing and timestamps.

Inputs are recordings of the original game and the game's data files (sounds
from the extraction cache, $BFT_EXTRACT_DIR). Nothing here starts, attaches
to or reads the original program.
"""

from __future__ import annotations

import csv
import hashlib
import json
import math
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from fractions import Fraction

TOOL_DIR = pathlib.Path(__file__).resolve().parent
REPO = TOOL_DIR.parent.parent


class ObserveError(Exception):
    """A failure the user can act on; printed without a traceback."""


# --------------------------------------------------------------------------
# paths (each overridable from the environment, which the tests use)
# --------------------------------------------------------------------------


def docs_dir() -> pathlib.Path:
    return pathlib.Path(os.environ.get("OBSERVE_DOCS", REPO / "docs" / "observations"))


def sessions_dir() -> pathlib.Path:
    return docs_dir() / "sessions"


def items_dir() -> pathlib.Path:
    return pathlib.Path(os.environ.get("OBSERVE_ITEMS", TOOL_DIR / "items"))


def marksets_dir() -> pathlib.Path:
    return pathlib.Path(os.environ.get("OBSERVE_MARKSETS", TOOL_DIR / "marksets"))


EVIDENCE_ENV = "OBFT_EVIDENCE_DIR"


def evidence_dir() -> pathlib.Path:
    """The one directory, outside the repository, that holds the evidence:
    recordings, marks files, frame grabs and measurements from them."""
    value = os.environ.get(EVIDENCE_ENV)
    if not value:
        raise ObserveError(f"set ${EVIDENCE_ENV} to the evidence directory outside the repository "
                           "(recordings, marks files, frame grabs)")
    path = pathlib.Path(value).expanduser()
    outside_repo(path, "evidence directory")
    if not path.is_dir():
        raise ObserveError(f"evidence directory {path} does not exist")
    return path


def session_evidence(session_id: str) -> pathlib.Path:
    """`<evidence>/<session>/`: the session's marks files and frame grabs
    (and, by convention, its recordings)."""
    path = evidence_dir() / session_id
    path.mkdir(parents=True, exist_ok=True)
    return path


def in_evidence(path: pathlib.Path, what: str) -> None:
    """Refuse a recording that is not inside the evidence directory."""
    root = evidence_dir().resolve()
    try:
        path.resolve().relative_to(root)
    except ValueError:
        raise ObserveError(f"{what} {path} is not inside the evidence directory {root} "
                           f"(${EVIDENCE_ENV}); move it there") from None


def scratch_dir() -> pathlib.Path:
    """Where measurements from recordings go by default: `<evidence>/scratch/`."""
    value = os.environ.get("OBSERVE_SCRATCH")
    path = pathlib.Path(value) if value else evidence_dir() / "scratch"
    outside_repo(path, "scratch directory")
    path.mkdir(parents=True, exist_ok=True)
    return path


def inside_repo(path: pathlib.Path) -> bool:
    try:
        path.resolve().relative_to(REPO)
        return True
    except ValueError:
        return False


def outside_repo(path: pathlib.Path, what: str) -> None:
    """Refuse evidence (recordings, marks, frame grabs) that sits in the repository."""
    if inside_repo(path):
        raise ObserveError(f"{what} {path} is inside the repository; evidence stays outside it")


def need(program: str) -> str:
    path = shutil.which(program)
    if not path:
        raise ObserveError(f"{program} not found on PATH (see tools/observe/README.md)")
    return path


def numpy():
    try:
        import numpy as np
    except ImportError as e:
        raise ObserveError("numpy is needed: pip install numpy (see tools/observe/README.md)") from e
    return np


def parse_time(text: str | None) -> float | None:
    """Seconds from '12.5', '1:02.5' or '0:01:02.5'."""
    if text is None:
        return None
    total = 0.0
    for part in text.split(":"):
        total = total * 60 + float(part)
    return total


def sha256_file(path: pathlib.Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        while chunk := fh.read(1 << 20):
            h.update(chunk)
    return h.hexdigest()


# A detached .ots proof starts with this magic, the major version (1), the
# file-hash operation (0x08, SHA-256) and the 32-byte digest of the file.
OTS_MAGIC = b"\x00OpenTimestamps\x00\x00Proof\x00\xbf\x89\xe2\xe8\x84\xe8\x92\x94"
_PROOF_RE = re.compile(r"\.(\d+)\.ots$")


def ots_digest(path: pathlib.Path) -> str | None:
    """The SHA-256 a detached OpenTimestamps proof is for, or None."""
    with open(path, "rb") as fh:
        head = fh.read(len(OTS_MAGIC) + 2 + 32)
    if not head.startswith(OTS_MAGIC) or len(head) < len(OTS_MAGIC) + 34:
        return None
    version, op = head[len(OTS_MAGIC)], head[len(OTS_MAGIC) + 1]
    return head[len(OTS_MAGIC) + 2:].hex() if version == 1 and op == 0x08 else None


def _proofs(name: str, where: pathlib.Path) -> list[tuple[int, pathlib.Path]]:
    found = []
    for p in where.glob(f"{name}.*.ots"):
        m = _PROOF_RE.search(p.name)
        if m and p.name == f"{name}.{m.group(1)}.ots":
            found.append((int(m.group(1)), p))
    return sorted(found)


def proofs_for(path: pathlib.Path, digest: str, where: pathlib.Path | None = None) -> list[pathlib.Path]:
    """The numbered `<file>.<n>.ots` proofs (beside the file, or in `where`)
    that prove exactly this content."""
    return [p for _, p in _proofs(path.name, where or path.parent) if ots_digest(p) == digest]


def stamp(path: pathlib.Path, where: pathlib.Path | None = None) -> pathlib.Path | None:
    """Timestamp this version of a file with OpenTimestamps, if `ots` is installed.

    Proofs are numbered per version, `<name>.1.ots`, `<name>.2.ots`, …, beside
    the file or in `where` (marks files live outside the repository, their
    proofs in it). A version that already has a proof is not stamped again,
    and no proof is ever overwritten. Returns the proof, or None.
    """
    ots = shutil.which("ots")
    if not ots or os.environ.get("OBSERVE_NO_STAMP"):
        return None
    where = where or path.parent
    digest = sha256_file(path)
    existing = _proofs(path.name, where)
    for _, proof in existing:
        if ots_digest(proof) == digest:
            print(f"  {path.name}: this version is already stamped ({proof.name})")
            return proof
    target = where / f"{path.name}.{(existing[-1][0] if existing else 0) + 1}.ots"
    with tempfile.TemporaryDirectory() as tmp:
        copy = pathlib.Path(tmp) / path.name
        shutil.copyfile(path, copy)
        run = subprocess.run([ots, "stamp", str(copy)], capture_output=True, text=True)
        made = pathlib.Path(str(copy) + ".ots")
        if run.returncode or not made.is_file():
            print(f"  ots stamp failed for {path.name}: {run.stderr.strip()}", file=sys.stderr)
            return None
        if ots_digest(made) != digest:
            print(f"  ots wrote a proof of something else for {path.name}; not kept", file=sys.stderr)
            return None
        where.mkdir(parents=True, exist_ok=True)
        with open(target, "xb") as fh:  # never overwrite
            fh.write(made.read_bytes())
    print(f"  timestamped {path.name} -> {target.name}")
    return target


# --------------------------------------------------------------------------
# media
# --------------------------------------------------------------------------


@dataclass
class Media:
    """What ffprobe says about a recording. Times are seconds from the start
    of the container, the clock mpv shows."""

    path: pathlib.Path
    duration: float
    width: int = 0
    height: int = 0
    fps: Fraction | None = None
    avg_fps: Fraction | None = None
    video_codec: str = ""
    video_offset: float = 0.0
    audio_rate: int = 0
    audio_channels: int = 0
    audio_codec: str = ""
    audio_offset: float = 0.0

    @property
    def constant_rate(self) -> bool:
        if not self.fps or not self.avg_fps:
            return False
        return abs(float(self.fps - self.avg_fps)) < 0.001 * float(self.fps)

    def frame_at(self, t: float) -> int:
        """The video frame on screen at time t."""
        return math.floor((t - self.video_offset) * float(self.fps) + 1e-4)

    def time_of(self, frame: int) -> float:
        return self.video_offset + frame / float(self.fps)

    def video_desc(self) -> str:
        if not self.fps:
            return "none"
        return f"{self.width}x{self.height} {self.fps.numerator}/{self.fps.denominator} fps {self.video_codec}"

    def audio_desc(self) -> str:
        if not self.audio_rate:
            return "none"
        return f"{self.audio_rate} Hz {self.audio_channels} ch {self.audio_codec}"


def _fraction(text: str | None) -> Fraction | None:
    if not text or text.endswith("/0"):
        return None
    value = Fraction(text)
    return value if value > 0 else None


def probe_media(path: pathlib.Path) -> Media:
    path = pathlib.Path(path)
    if not path.is_file():
        raise ObserveError(f"no such recording: {path}")
    cmd = [need("ffprobe"), "-v", "error", "-print_format", "json", "-show_format", "-show_streams", str(path)]
    run = subprocess.run(cmd, capture_output=True, text=True)
    if run.returncode:
        raise ObserveError(f"ffprobe failed on {path}: {run.stderr.strip()}")
    info = json.loads(run.stdout)
    fmt = info.get("format", {})
    start = float(fmt.get("start_time") or 0)
    media = Media(path=path, duration=float(fmt.get("duration") or 0))
    for s in info.get("streams", []):
        offset = float(s.get("start_time") or start) - start
        if s.get("codec_type") == "video" and media.fps is None:
            if s.get("disposition", {}).get("attached_pic"):
                continue
            media.width, media.height = int(s["width"]), int(s["height"])
            media.fps = _fraction(s.get("r_frame_rate"))
            media.avg_fps = _fraction(s.get("avg_frame_rate"))
            media.video_codec = s.get("codec_name", "")
            media.video_offset = offset
        elif s.get("codec_type") == "audio" and not media.audio_rate:
            media.audio_rate = int(s.get("sample_rate") or 0)
            media.audio_channels = int(s.get("channels") or 0)
            media.audio_codec = s.get("codec_name", "")
            media.audio_offset = offset
    return media


def need_video(media: Media) -> None:
    if not media.fps:
        raise ObserveError(f"{media.path.name} has no video stream")
    if not media.constant_rate:
        print(f"warning: {media.path.name} has a variable frame rate "
              f"({media.fps} nominal, {media.avg_fps} average); frame numbers are approximate. "
              "Record at a constant frame rate.", file=sys.stderr)


def decode_audio(path: pathlib.Path, rate: int):
    """The first audio stream as mono float64 samples at `rate` Hz."""
    np = numpy()
    cmd = [need("ffmpeg"), "-v", "error", "-nostdin", "-i", str(path), "-map", "0:a:0",
           "-ac", "1", "-ar", str(rate), "-f", "f32le", "-"]
    run = subprocess.run(cmd, capture_output=True)
    if run.returncode:
        raise ObserveError(f"ffmpeg could not decode the audio of {path}: {run.stderr.decode().strip()}")
    return np.frombuffer(run.stdout, dtype="<f4").astype(np.float64)


def video_frames(media: Media, vf: str, width: int, height: int, gray: bool,
                 first: int = 0, last: int | None = None):
    """Yield (frame number, uint8 array) for frames first..last after the
    filter `vf`, which must produce width x height pixels."""
    np = numpy()
    channels = 1 if gray else 3
    cmd = [need("ffmpeg"), "-v", "error", "-nostdin"]
    if first > 0:
        # half a frame early: accurate seeking keeps the first frame at or after it
        cmd += ["-ss", f"{media.time_of(first) - 0.5 / float(media.fps):.6f}"]
    cmd += ["-i", str(media.path), "-map", "0:v:0", "-vf", vf, "-fps_mode", "passthrough"]
    if last is not None:
        cmd += ["-frames:v", str(last - first + 1)]
    cmd += ["-pix_fmt", "gray" if gray else "rgb24", "-f", "rawvideo", "-"]
    size = width * height * channels
    proc = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    finished = False
    try:
        n = first
        while True:
            buf = proc.stdout.read(size)
            if len(buf) < size:
                break
            frame = np.frombuffer(buf, dtype=np.uint8).reshape(height, width, channels)
            yield n, frame
            n += 1
        finished = True
    finally:
        proc.stdout.close()
        err = proc.stderr.read().decode()
        proc.stderr.close()
        # a consumer that stops early closes the pipe, and ffmpeg's exit then means nothing
        if proc.wait() != 0 and finished:
            raise ObserveError(f"ffmpeg failed on {media.path}: {err.strip()}")


def frame_range(media: Media, start: float | None, end: float | None) -> tuple[int, int | None]:
    first = max(0, media.frame_at(start)) if start is not None else 0
    last = media.frame_at(end) if end is not None else None
    if last is not None and last < first:
        raise ObserveError("--to is before --from")
    return first, last


# --------------------------------------------------------------------------
# marks
# --------------------------------------------------------------------------

MARK_FIELDS = ["frame", "time", "label", "note"]
VOID = "void"
NOT_OBSERVABLE = "not-observable"
_POS_RE = re.compile(r"\bx=(-?\d+(?:\.\d+)?) y=(-?\d+(?:\.\d+)?)")
_GRAB_RE = re.compile(r"\bgrab=(\S+)")
_NUM_RE = re.compile(r"-?\d+(?:\.\d+)?")
_TAG_RE = re.compile(r"(key|desc):\s*(.*)", re.I)


@dataclass
class Mark:
    """One row of a marks file (`row` counts data rows from 1), or an audio
    event a result found in the recording (`audio`, numbered from 1)."""

    row: int
    frame: int
    time: float
    label: str
    note: str
    voided_by: int | None = None
    audio: bool = False

    @property
    def ref(self) -> str:
        """How an entry names it: `12` for a marks row, `a3` for an audio event."""
        return f"a{self.row}" if self.audio else str(self.row)

    @property
    def pos(self) -> tuple[float, float] | None:
        m = _POS_RE.search(self.note)
        return (float(m.group(1)), float(m.group(2))) if m else None

    @property
    def grab(self) -> str | None:
        m = _GRAB_RE.search(self.note)
        return m.group(1) if m else None

    def _parts(self) -> dict[str, str]:
        """The note's `;`-separated parts: `key:` and `desc:` by name, the
        rest (bar the position and grab the tool wrote) as `text`."""
        parts, text = {}, []
        for seg in (s.strip() for s in self.note.split(";")):
            if not seg or _POS_RE.fullmatch(seg) or _GRAB_RE.fullmatch(seg):
                continue
            if m := _TAG_RE.fullmatch(seg):
                parts[m.group(1).lower()] = m.group(2).strip()
            else:
                text.append(seg)
        parts["text"] = "; ".join(text)
        return parts

    @property
    def text(self) -> str:
        """What was typed, without `key:`, `desc:`, the position or the grab:
        for a wording key, the game's wording."""
        return self._parts()["text"]

    @property
    def key(self) -> str | None:
        """The message key or reader control the note names (`key: MSG_…`)."""
        return self._parts().get("key") or None

    @property
    def desc(self) -> str | None:
        """A description in our own words (`desc: …`)."""
        return self._parts().get("desc") or None

    @property
    def value(self) -> float | None:
        """The first number in the note's text, e.g. a reading off the HUD."""
        m = _NUM_RE.search(self.text)
        return float(m.group(0)) if m else None


def marks_path(session: str, recording: str, markset: str) -> pathlib.Path:
    """`<evidence>/<session>/<session>.<recording>.<markset>.csv`."""
    return session_evidence(session) / f"{session}.{pathlib.Path(recording).stem}.{markset}.csv"


def ensure_marks_file(path: pathlib.Path) -> None:
    if not path.exists():
        path.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "w", newline="") as fh:
            csv.writer(fh).writerow(MARK_FIELDS)


def read_marks(path: pathlib.Path) -> list[Mark]:
    """All rows, void rows included; a voided row carries the voiding row."""
    with open(path, newline="") as fh:
        reader = csv.DictReader(fh)
        if reader.fieldnames is None or reader.fieldnames[:4] != MARK_FIELDS:
            raise ObserveError(f"{path} does not start with the header {','.join(MARK_FIELDS)}")
        marks = []
        for i, row in enumerate(reader, start=1):
            try:
                marks.append(Mark(i, int(row["frame"]), float(row["time"]), row["label"].strip(),
                                  (row.get("note") or "").strip()))
            except (TypeError, ValueError) as e:
                raise ObserveError(f"{path}: data row {i} is malformed: {e}") from e
    by_row = {m.row: m for m in marks}
    for m in marks:
        if m.label == VOID:
            target = re.search(r"row (\d+)", m.note)
            if target and int(target.group(1)) in by_row:
                by_row[int(target.group(1))].voided_by = m.row
    return marks


def write_marks_csv(path: pathlib.Path, rows) -> None:
    """Write candidate marks (frame, time, label, note) to a scratch CSV."""
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "w", newline="") as fh:
        w = csv.writer(fh)
        w.writerow(MARK_FIELDS)
        for frame, time, label, note in rows:
            w.writerow([frame, f"{time:.6f}", label, note])


def read_candidate_times(path: pathlib.Path) -> list[float]:
    with open(path, newline="") as fh:
        reader = csv.DictReader(fh)
        if not reader.fieldnames or "time" not in reader.fieldnames:
            raise ObserveError(f"{path} has no 'time' column")
        return sorted(float(r["time"]) for r in reader if r.get("time"))
