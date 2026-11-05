"""Summarise a Tracy capture of the game: frame times, costly spans, spikes.

    python3 tools/profile/analyse.py CAPTURE.tracy [--against OTHER.tracy] [options]

Reads the capture through tracy-csvexport (built by build-tracy.sh) in two
streamed passes, so captures of tens of millions of spans fit in memory.
Standard library only.

A frame runs from one Bevy "main app" span to the next. Every span is
charged its self time (its length minus the spans nested in it, as Tracy
computes it) to the frame it starts in, so per-frame figures add up without
counting a schedule and its systems twice. Spans that mostly wait are
marked: the XR frame wait, presenting, and a schedule's own time (it waits
there while its systems run on other threads).

A capture holds everything since the game started, loading included; use
--from/--to (seconds after the first frame) to look at play only.
"""

from __future__ import annotations

import argparse
import array
import bisect
import collections
import glob
import os
import pathlib
import re
import subprocess
import sys

REPO = pathlib.Path(__file__).resolve().parents[2]
FRAME_SPAN = "main app"
SYSTEM = re.compile(r'^system\{name="?(.*?)"?\}$')
WAITING = re.compile(r"wait|present|sleep|vsync|^schedule\{|^multithreaded executor$", re.IGNORECASE)

# Long names lose their middle, not their ends.
WIDTH = 64


def csvexport() -> str:
    found = os.environ.get("TRACY_CSVEXPORT")
    if found:
        return found
    tools = sorted(glob.glob(str(REPO / "target/tracy/*/bin/tracy-csvexport")))
    if not tools:
        sys.exit("no tracy-csvexport; run tools/profile/build-tracy.sh first")
    return tools[-1]


def rows(path: pathlib.Path, *options: str):
    """(name, start ns, length ns, thread) for every finished span."""
    with subprocess.Popen(
        [csvexport(), "-u", "-s", "\t", *options, str(path)],
        stdout=subprocess.PIPE,
        text=True,
        bufsize=1 << 20,
    ) as export:
        assert export.stdout is not None
        header = export.stdout.readline().rstrip("\n").split("\t")
        name_at, start_at = header.index("name"), header.index("ns_since_start")
        length_at, thread_at = header.index("exec_time_ns"), header.index("thread")
        for line in export.stdout:
            fields = line.split("\t")
            length = int(fields[length_at])
            if length >= 0:  # negative: still open when the capture ended
                yield fields[name_at], int(fields[start_at]), length, fields[thread_at]
    if export.returncode:
        sys.exit(f"tracy-csvexport failed on {path}")


def percentile(values, share: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(share * len(ordered)))]


def ms(ns: float) -> float:
    return ns / 1e6


def short(name: str) -> str:
    if len(name) <= WIDTH:
        return name
    keep = (WIDTH - 1) // 2
    return name[:keep] + "…" + name[-(WIDTH - 1 - keep) :]


class Capture:
    """One capture cut into frames, with per-frame self time per span name."""

    def __init__(self, path: pathlib.Path, start: float | None, end: float | None):
        self.path = path
        spans = sorted(
            (begin, length) for name, begin, length, _ in rows(path, "-f", FRAME_SPAN) if name == FRAME_SPAN
        )
        if len(spans) < 3:
            sys.exit(f"{path}: fewer than three '{FRAME_SPAN}' spans; is it a capture of the game?")
        first = spans[0][0]
        low = first + (start or 0) * 1e9
        high = first + end * 1e9 if end is not None else float("inf")
        spans = [s for s in spans if low <= s[0] <= high]
        if len(spans) < 3:
            sys.exit(f"{path}: fewer than three frames between --from and --to")
        self.offset = ms(spans[0][0] - first) / 1000
        # The last span only marks where the last whole frame ends.
        self.starts = [begin for begin, _ in spans[:-1]]
        self.intervals = [b[0] - a[0] for a, b in zip(spans, spans[1:])]
        self.main_work = [length for _, length in spans[:-1]]

        count = len(self.starts)
        lo, hi = self.starts[0], spans[-1][0]
        self.per_frame: dict[str, array.array] = {}
        self.calls: collections.Counter[str] = collections.Counter()
        self.threads: dict[str, float] = collections.defaultdict(float)
        names: dict[str, str] = {}
        starts = self.starts
        for raw, begin, own, thread in rows(path, "-e"):
            if begin < lo or begin >= hi:
                continue
            name = names.get(raw)
            if name is None:
                system = SYSTEM.match(raw)
                name = names[raw] = system.group(1) if system else raw
            frames = self.per_frame.get(name)
            if frames is None:
                frames = self.per_frame[name] = array.array("d", bytes(8 * count))
            frames[bisect.bisect_right(starts, begin) - 1] += own
            self.calls[name] += 1
            self.threads[thread] += own

    @property
    def frames(self) -> int:
        return len(self.starts)

    def mean(self, name: str) -> float:
        values = self.per_frame.get(name)
        return sum(values) / self.frames if values else 0.0


def report(capture: Capture, budget: float, top: int, worst: int, pattern: re.Pattern | None) -> None:
    intervals = [ms(i) for i in capture.intervals]
    work = [ms(length) for length in capture.main_work]
    seconds = ms(sum(capture.intervals)) / 1000
    over = sum(1 for i in intervals if i > budget * 1.5)
    print(f"{capture.path.name}: {capture.frames} frames over {seconds:.1f} s, from {capture.offset:.1f} s")
    print(f"budget {budget:.2f} ms; frames longer than 1.5 x budget (missed): {over} ({100 * over / capture.frames:.1f} %)")
    print()
    print(f"{'ms':<34}{'mean':>8}{'p50':>8}{'p95':>8}{'p99':>8}{'max':>8}")
    for label, values in (("frame interval", intervals), (f"'{FRAME_SPAN}' span (main world)", work)):
        mean = sum(values) / len(values)
        print(
            f"{label:<34}{mean:8.2f}{percentile(values, .5):8.2f}{percentile(values, .95):8.2f}"
            f"{percentile(values, .99):8.2f}{max(values):8.2f}"
        )
    print()

    names = [n for n in capture.per_frame if pattern is None or pattern.search(n)]
    names.sort(key=capture.mean, reverse=True)
    print(f"Spans by self time per frame, summed over threads (top {top}; * = mostly waiting)")
    print(f"{'span':<{WIDTH + 2}}{'mean':>7}{'p95':>7}{'max':>7}{'calls':>7}{'%bud':>6}")
    for name in names[:top]:
        values = [ms(v) for v in capture.per_frame[name]]
        mark = "*" if WAITING.search(name) else " "
        print(
            f"{mark}{short(name):<{WIDTH + 1}}{capture.mean(name) / 1e6:7.3f}{percentile(values, .95):7.3f}"
            f"{max(values):7.3f}{capture.calls[name] / capture.frames:7.1f}"
            f"{100 * capture.mean(name) / 1e6 / budget:6.1f}"
        )
    print()

    print("Threads by self time per frame (ms)")
    for thread, total in sorted(capture.threads.items(), key=lambda t: -t[1])[:8]:
        print(f"  thread {thread:<20}{ms(total) / capture.frames:8.3f}")
    print()

    slowest = sorted(range(capture.frames), key=lambda f: -capture.intervals[f])[:worst]
    print(f"Slowest {worst} frames and what they spent it on (self ms, waiting spans left out)")
    for frame in slowest:
        working = [n for n in capture.per_frame if not WAITING.search(n)]
        heavy = sorted(working, key=lambda n: -capture.per_frame[n][frame])[:5]
        at = capture.offset + ms(capture.starts[frame] - capture.starts[0]) / 1000
        spent = ", ".join(f"{short(n)} {ms(capture.per_frame[n][frame]):.2f}" for n in heavy)
        print(f"  at {at:.2f} s: {ms(capture.intervals[frame]):.2f} ms  {spent}")


def compare(before: Capture, after: Capture, top: int, pattern: re.Pattern | None) -> None:
    names = set(before.per_frame) | set(after.per_frame)
    if pattern is not None:
        names = {n for n in names if pattern.search(n)}
    deltas = sorted(names, key=lambda n: -abs(after.mean(n) - before.mean(n)))
    mean = lambda c: ms(sum(c.intervals)) / c.frames
    print(f"{before.path.name} -> {after.path.name}")
    print(f"frame interval mean {mean(before):.2f} -> {mean(after):.2f} ms")
    print()
    print(f"Largest changes in self time per frame (ms, top {top})")
    print(f"{'span':<{WIDTH + 2}}{'before':>8}{'after':>8}{'delta':>8}")
    for name in deltas[:top]:
        a, b = before.mean(name) / 1e6, after.mean(name) / 1e6
        print(f" {short(name):<{WIDTH + 1}}{a:8.3f}{b:8.3f}{b - a:+8.3f}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("capture", type=pathlib.Path)
    parser.add_argument("--against", type=pathlib.Path, help="an earlier capture to compare with")
    parser.add_argument("--from", dest="start", type=float, help="seconds after the first frame to start at")
    parser.add_argument("--to", dest="end", type=float, help="seconds after the first frame to stop at")
    parser.add_argument("--budget", type=float, default=1000 / 90, help="frame budget in ms (default 90 Hz)")
    parser.add_argument("--top", type=int, default=30)
    parser.add_argument("--worst", type=int, default=5)
    parser.add_argument("--filter", help="only spans whose name matches this regex")
    args = parser.parse_args()
    pattern = re.compile(args.filter) if args.filter else None
    capture = Capture(args.capture, args.start, args.end)
    if args.against:
        compare(Capture(args.against, args.start, args.end), capture, args.top, pattern)
    else:
        report(capture, args.budget, args.top, args.worst, pattern)


if __name__ == "__main__":
    main()
