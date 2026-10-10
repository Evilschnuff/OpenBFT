"""Video measurements: the game's own frame rate, and frames where a screen
region changes.

The recording runs at a fixed rate (60 fps); the game draws at its own. A
game frame shown for two recorded frames repeats, so counting the recorded
frames that differ from their predecessor over a stretch of motion gives
the game's rate.
"""

from __future__ import annotations

import collections
import statistics
from dataclasses import dataclass, field

from common import Media, ObserveError, frame_range, numpy, video_frames

PROBE_WIDTH = 160


@dataclass
class FrameRate:
    fps: float | None
    windows: int
    low: float | None = None
    high: float | None = None
    runs: collections.Counter = field(default_factory=collections.Counter)
    every_frame_new: bool = False


def new_frames(media: Media, first: int = 0, last: int | None = None,
               pixel_threshold: int = 6, fraction: float = 0.002):
    """For every recorded frame after the first, whether it shows a new
    picture: more than `fraction` of its pixels (downscaled, grey) differ
    from the previous frame's by more than `pixel_threshold` grey levels."""
    np = numpy()
    width = PROBE_WIDTH
    height = max(2, round(PROBE_WIDTH * media.height / media.width / 2) * 2)
    vf = f"scale={width}:{height}:flags=area"
    new, prev = [], None
    for _, frame in video_frames(media, vf, width, height, True, first, last):
        cur = frame.astype(np.int16)
        if prev is not None:
            new.append(bool((np.abs(cur - prev) > pixel_threshold).mean() > fraction))
        prev = cur
    return new


def _runs(flags) -> collections.Counter:
    """How many recorded frames each picture stayed on screen."""
    runs, length = collections.Counter(), None
    for is_new in flags:
        if is_new:
            if length is not None:
                runs[length] += 1
            length = 1
        elif length is not None:
            length += 1
    return runs


def measure_frame_rate(media: Media, start: float | None = None, end: float | None = None,
                       min_rate: float = 10.0, **thresholds) -> FrameRate:
    """The game's frame rate from distinct recorded frames.

    With a stretch (start/end) of continuous motion: distinct frames over
    its length. Without: the median over one-second windows in which at
    least `min_rate` new pictures appear (menus and pauses drop out).
    """
    fps = float(media.fps)
    first, last = frame_range(media, start, end)
    flags = new_frames(media, first, last, **thresholds)
    if not flags:
        raise ObserveError("no frames decoded")
    if start is not None or end is not None:
        rate = sum(flags) * fps / len(flags)
        return FrameRate(rate, 1, rate, rate, _runs(flags), all(flags))
    window = max(1, round(fps))
    rates, motion = [], []
    for i in range(0, len(flags) - window + 1, window):
        chunk = flags[i:i + window]
        rate = sum(chunk) * fps / window
        if rate >= min_rate:
            rates.append(rate)
            motion.extend(chunk)
    if not rates:
        return FrameRate(None, 0)
    q = statistics.quantiles(rates, n=4) if len(rates) > 1 else [rates[0]] * 3
    return FrameRate(statistics.median(rates), len(rates), q[0], q[2], _runs(motion), all(motion))


def report_frame_rate(media: Media, result: FrameRate, stretch: bool) -> list[str]:
    if result.fps is None:
        return ["game frame rate: no stretch of motion found; give --from/--to around one"]
    where = "over the given stretch" if stretch else \
        f"median of {result.windows} one-second windows with motion, quartiles {result.low:.2f}–{result.high:.2f}"
    lines = [f"game frame rate: {result.fps:.2f} fps ({where})"]
    if result.runs:
        total = sum(result.runs.values())
        common = ", ".join(f"{k} frame(s) ×{n}" for k, n in sorted(result.runs.items())[:6])
        lines.append(f"  each picture stayed on screen for: {common} ({total} pictures)")
    if result.every_frame_new:
        lines.append(f"  every recorded frame was new: the game may draw faster than the recording's "
                     f"{float(media.fps):.0f} fps; this is a lower bound")
    return lines


def changes(media: Media, roi: tuple[int, int, int, int], threshold: float, gap: int,
            start: float | None = None, end: float | None = None):
    """Runs of frames in which the region differs from the frame before.

    Returns (events, trace): events are (first frame, last frame, peak
    difference); trace is (frame, mean absolute difference, 0–255, over the
    region's RGB values) for every frame after the first.
    """
    np = numpy()
    x, y, w, h = roi
    if w <= 0 or h <= 0 or x < 0 or y < 0 or x + w > media.width or y + h > media.height:
        raise ObserveError(f"region {x},{y},{w},{h} is not inside the {media.width}x{media.height} picture")
    first, last = frame_range(media, start, end)
    vf = f"format=rgb24,crop={w}:{h}:{x}:{y}"
    trace, prev = [], None
    for n, frame in video_frames(media, vf, w, h, False, first, last):
        cur = frame.astype(np.int16)
        if prev is not None:
            trace.append((n, float(np.abs(cur - prev).mean())))
        prev = cur
    events: list[list] = []
    for n, diff in trace:
        if diff <= threshold:
            continue
        if events and n - events[-1][1] <= gap:
            events[-1][1] = n
            events[-1][2] = max(events[-1][2], diff)
        else:
            events.append([n, n, diff])
    return [tuple(e) for e in events], trace
