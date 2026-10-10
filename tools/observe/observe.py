#!/usr/bin/env python3
"""Turn recordings of the original game into documented measurements.

Inputs are recordings (video at 60 fps plus the game's audio) and the game's
data files: reference sounds come from the extraction cache,
$BFT_EXTRACT_DIR. The tools never start, attach to, read the memory of or
modify the original game, and never read its program files.

usage:
    observe.py session new --rec FILE [--rec FILE ...] [--id ID] [--game-version V] [--os OS] ...
    observe.py session add ID --rec FILE [...]
    observe.py session stamp ID
    observe.py probe REC [--from T --to T] [--sync-ref WAV [--marks CSV] [--record]]
    observe.py mark REC --set MARKSET [--session ID] [--candidates CSV]
    observe.py onsets REC --ref WAV [--from T --to T] [--threshold C]
    observe.py pitch REC --ref WAV [--from T --to T] [--peak HZ]
    observe.py changes REC --roi X,Y,W,H [--from T --to T] [--threshold D]
    observe.py result ITEM [--print]
    observe.py check

Times are seconds from the start of the recording (12.5 or 1:02.5), the
clock mpv shows. Recordings, marks files, frame grabs and measurements
(CSV, SVG) live in the evidence directory, $OBFT_EVIDENCE_DIR, outside the
repository. See README.md.
"""

from __future__ import annotations

import argparse
import csv
import pathlib
import statistics
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))

from common import (ObserveError, need_video, parse_time, probe_media, read_marks, scratch_dir,  # noqa: E402
                    session_evidence, write_marks_csv)


def _stem(path: str) -> str:
    return pathlib.Path(path).stem


def _out(args, default: str) -> pathlib.Path:
    return pathlib.Path(args.out) if args.out else scratch_dir() / default


# --------------------------------------------------------------------------
# probe
# --------------------------------------------------------------------------


def _session_of(rec: pathlib.Path, session_id: str | None):
    """The session listing this recording, or None when none does."""
    import session
    try:
        return session.find_session(rec.name, session_id)
    except ObserveError:
        if session_id:
            raise
        return None


def cmd_probe(args) -> None:
    import session as sessions
    import video
    rec = pathlib.Path(args.rec)
    media = probe_media(rec)
    rate = "constant" if media.constant_rate else f"VARIABLE, average {media.avg_fps}"
    print(f"recording: {media.path.name}, {media.duration:.3f} s")
    print(f"  video: {media.video_desc()} ({rate}), starts at +{media.video_offset:.3f} s")
    print(f"  audio: {media.audio_desc()}, starts at +{media.audio_offset:.3f} s")
    session = _session_of(rec, args.session)
    start, end = parse_time(args.start), parse_time(args.end)
    if media.fps:
        need_video(media)
        t = sessions.thresholds_from(args, session)
        print(f"frame-change thresholds: {sessions.describe_thresholds(t)}"
              + (f", as session {session.id} records" if session and session.thresholds() else ""))
        result = video.measure_frame_rate(media, start, end, min_rate=t["min_rate"],
                                          pixel_threshold=int(t["pixel_threshold"]), fraction=t["fraction"])
        for line in video.report_frame_rate(media, result, start is not None or end is not None):
            print(line)
    if args.sync_ref:
        _sync(args, media, session)
    elif args.record:
        raise ObserveError("--record writes the audio/video offset, which needs --sync-ref")


def _sync(args, media, session) -> None:
    import audio
    import session as sessions
    if args.marks:
        files = [pathlib.Path(args.marks)]
    elif session:
        files = sorted(session_evidence(session.id).glob(f"{session.id}.{media.path.stem}.*.csv"))
    else:
        raise ObserveError("--sync-ref needs --marks, or a session that lists the recording")
    marks = [m for f in files for m in read_marks(f) if m.label == args.sync_label and m.voided_by is None]
    if not marks:
        raise ObserveError(f"no '{args.sync_label}' marks in {', '.join(f.name for f in files) or 'any marks file'}")
    ref = audio.load_ref(args.sync_ref, args.rate)
    found = audio.onsets(media, ref, args.rate, args.threshold, None)
    offsets = []
    for m in marks:
        near = [o for o in found if abs(o.time - m.time) <= args.window]
        if not near:
            print(f"  sync at frame {m.frame}: no onset of the reference within ±{args.window} s")
            continue
        o = min(near, key=lambda o: abs(o.time - m.time))
        offsets.append(o.time - m.time)
        print(f"  sync at frame {m.frame} ({m.time:.3f} s): sound at {o.time:.4f} s "
              f"(confidence {o.confidence:.2f}), audio − video {o.time - m.time:+.4f} s")
    if not offsets:
        raise ObserveError("no sync event had its sound nearby; nothing measured")
    # a visual mark is good to one game frame (one recorded frame if the rate is unknown)
    game_fps = session.game_fps(media.path.name) if session else None
    frame = 1 / game_fps if game_fps else 1 / float(media.fps)
    error = max(frame, statistics.stdev(offsets) if len(offsets) > 1 else 0.0)
    mean = statistics.mean(offsets)
    spread = f", range {min(offsets):+.4f}…{max(offsets):+.4f} s" if len(offsets) > 1 else ""
    print(f"audio − video offset: {mean:+.4f} ± {error:.4f} s over {len(offsets)} event(s){spread}")
    if args.record:
        if not session:
            raise ObserveError("--record needs a session that lists the recording")
        sessions.record_offset(session, media.path.name, mean, error, len(offsets))


# --------------------------------------------------------------------------
# onsets, pitch, changes
# --------------------------------------------------------------------------


def cmd_onsets(args) -> None:
    import audio
    media = probe_media(pathlib.Path(args.rec))
    ref = audio.load_ref(args.ref, args.rate)
    found = audio.onsets(media, ref, args.rate, args.threshold, args.min_gap,
                         parse_time(args.start), parse_time(args.end))
    out = _out(args, f"{_stem(args.rec)}.onsets.{_stem(args.ref)}.csv")
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w", newline="") as fh:
        w = csv.writer(fh)
        w.writerow(["time", "frame", "confidence"])
        for o in found:
            w.writerow([f"{o.time:.4f}", "" if o.frame is None else o.frame, f"{o.confidence:.3f}"])
    print(f"reference {args.ref}: {len(ref) / args.rate:.3f} s; {len(found)} onset(s) at confidence ≥ {args.threshold}")
    for o in found:
        frame = "" if o.frame is None else f"  frame {o.frame}"
        print(f"  {o.time:10.4f} s{frame}  confidence {o.confidence:.3f}")
    print(f"wrote {out}")


def cmd_pitch(args) -> None:
    import audio
    import plot
    media = probe_media(pathlib.Path(args.rec))
    ref = audio.load_ref(args.ref, args.rate)
    peaks = audio.ref_peaks(ref, args.rate, args.window)
    if not peaks:
        raise ObserveError("the reference has no spectral peak")
    print("reference peaks: " + ", ".join(f"{f:.1f} Hz ({db:+.1f} dB)" for f, db in peaks))
    f_ref = min(peaks, key=lambda p: abs(p[0] - args.peak))[0] if args.peak else peaks[0][0]
    print(f"following {f_ref:.2f} Hz")
    lo, hi = (float(v) for v in args.speeds.split(","))
    points = audio.track_pitch(media, args.rate, f_ref, args.window, args.hop, (lo, hi),
                               parse_time(args.start), parse_time(args.end))
    out = _out(args, f"{_stem(args.rec)}.pitch.{_stem(args.ref)}.csv")
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w", newline="") as fh:
        w = csv.writer(fh)
        w.writerow(["time", "frame", "freq_hz", "speed", "salience_db"])
        for p in points:
            w.writerow([f"{p.time:.4f}", "" if p.frame is None else p.frame, f"{p.freq:.3f}",
                        f"{p.speed:.5f}", f"{p.salience:.1f}"])
    good = [p for p in points if p.salience >= args.min_salience]
    svg = out.with_suffix(".svg")
    plot.line_plot(
        svg, f"Playback speed of {_stem(args.ref)} in {pathlib.Path(args.rec).name}",
        f"peak at {f_ref:.1f} Hz in the reference; points with salience ≥ {args.min_salience:g} dB",
        "time in the recording (s)", "speed (recording ÷ reference)",
        [(p.time, p.speed if p.salience >= args.min_salience else None,
          f"{p.time:.3f} s, frame {p.frame}: speed {p.speed:.4f} ({p.freq:.1f} Hz, {p.salience:.0f} dB)")
         for p in points], ref_y=1.0)
    if good:
        speeds = [p.speed for p in good]
        print(f"{len(good)} of {len(points)} windows with salience ≥ {args.min_salience:g} dB: "
              f"median speed {statistics.median(speeds):.4f}, range {min(speeds):.4f}–{max(speeds):.4f}")
    else:
        print(f"no window has salience ≥ {args.min_salience:g} dB: the peak was not found")
    print(f"wrote {out} and {svg.name}")


def cmd_changes(args) -> None:
    import video
    media = probe_media(pathlib.Path(args.rec))
    need_video(media)
    try:
        roi = tuple(int(v) for v in args.roi.split(","))
        assert len(roi) == 4
    except (ValueError, AssertionError):
        raise ObserveError("--roi is x,y,w,h in recording pixels") from None
    events, trace = video.changes(media, roi, args.threshold, args.gap,
                                  parse_time(args.start), parse_time(args.end))
    out = _out(args, f"{_stem(args.rec)}.changes.{'_'.join(map(str, roi))}.csv")
    write_marks_csv(out, [(first, media.time_of(first), "change",
                           f"frames {first}-{last}, peak difference {peak:.1f}")
                          for first, last, peak in events])
    if args.trace:
        with open(args.trace, "w", newline="") as fh:
            w = csv.writer(fh)
            w.writerow(["frame", "time", "difference"])
            for n, d in trace:
                w.writerow([n, f"{media.time_of(n):.6f}", f"{d:.3f}"])
    print(f"{len(events)} change(s) in {args.roi} above {args.threshold:g} (mean |Δ| per channel, 0–255)")
    for first, last, peak in events:
        print(f"  frame {first:7d}  {media.time_of(first):10.3f} s  to frame {last}  peak {peak:.1f}")
    print(f"wrote {out} (candidate marks: observe mark ... --candidates {out.name})")


# --------------------------------------------------------------------------
# argument parsing
# --------------------------------------------------------------------------


def _range(p: argparse.ArgumentParser) -> None:
    p.add_argument("--from", dest="start", help="start time in the recording (s or m:s)")
    p.add_argument("--to", dest="end", help="end time in the recording (s or m:s)")


def _thresholds(p: argparse.ArgumentParser) -> None:
    p.add_argument("--pixel-threshold", type=int, help="grey levels for a pixel to count as changed (default 6)")
    p.add_argument("--fraction", type=float, help="share of changed pixels for a new picture (default 0.002)")
    p.add_argument("--min-rate", type=float, help="new pictures/s for a second to count as motion (default 10)")


def parser() -> argparse.ArgumentParser:
    ap = argparse.ArgumentParser(prog="observe.py", description=__doc__.split("\n\n")[0],
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="command", required=True)

    s = sub.add_parser("session", help="session files: new, add, stamp")
    ssub = s.add_subparsers(dest="action", required=True)
    n = ssub.add_parser("new", help="create docs/observations/sessions/<id>.md")
    n.add_argument("--id", help="session id (default: <date>-a, -b, ...)")
    n.add_argument("--date", help="YYYY-MM-DD (default: today)")
    n.add_argument("--rec", action="append", required=True,
                   help="a recording inside $OBFT_EVIDENCE_DIR (repeatable)")
    n.add_argument("--game-version")
    n.add_argument("--os", help="OS or VM the game ran on")
    n.add_argument("--resolution", help="the game's own resolution")
    n.add_argument("--game-fps", type=float, help="the game's frame rate, if measured otherwise")
    n.add_argument("--recorder", help="recorder and settings, e.g. 'OBS 31, 60 fps CFR, x264 CRF 0'")
    n.add_argument("--notes", help="setup notes")
    n.add_argument("--no-probe", action="store_true", help="do not measure the game's frame rate")
    _thresholds(n)
    a = ssub.add_parser("add", help="add recordings to a session")
    a.add_argument("id")
    a.add_argument("--rec", action="append", required=True)
    a.add_argument("--no-probe", action="store_true")
    _thresholds(a)
    t = ssub.add_parser("stamp", help="timestamp this version of the session file (a new numbered .ots)")
    t.add_argument("id")

    p = sub.add_parser("probe", help="the game's frame rate and the audio/video offset")
    p.add_argument("rec")
    _range(p)
    _thresholds(p)
    p.add_argument("--session", help="session id (default: the one that lists the recording, if any)")
    p.add_argument("--sync-ref", help="the sound of the sync event (WAV or cache name)")
    p.add_argument("--marks", help="marks CSV holding the sync events (default: the recording's marks files)")
    p.add_argument("--record", action="store_true", help="write the measured offset into the session file")
    p.add_argument("--sync-label", default="sync")
    p.add_argument("--window", type=float, default=0.5, help="± s around a sync mark to look for the sound")
    p.add_argument("--threshold", type=float, default=0.5)
    p.add_argument("--rate", type=int, default=24000)

    m = sub.add_parser("mark", help="mark events by hand in mpv")
    m.add_argument("rec")
    m.add_argument("--set", required=True, help="markset name in tools/observe/marksets/")
    m.add_argument("--session", help="session id (default: the one that lists the recording)")
    m.add_argument("--candidates", help="CSV with a time column (from changes/onsets): TAB jumps through it")
    m.add_argument("--grabs", help="where frame grabs go (default: <recording>.grabs/ next to it)")
    m.add_argument("--mpv-arg", action="append", default=[], help="passed to mpv (repeatable)")

    o = sub.add_parser("onsets", help="where a known sound plays in the recording")
    o.add_argument("rec")
    o.add_argument("--ref", required=True, help="WAV file or sound name in $BFT_EXTRACT_DIR")
    _range(o)
    o.add_argument("--threshold", type=float, default=0.5, help="lowest confidence reported (0–1)")
    o.add_argument("--min-gap", type=float, help="s between onsets (default: half the reference)")
    o.add_argument("--rate", type=int, default=24000, help="analysis sample rate")
    o.add_argument("--out", help="CSV (default: scratch)")

    q = sub.add_parser("pitch", help="playback speed of a known sound over time")
    q.add_argument("rec")
    q.add_argument("--ref", required=True, help="WAV file or sound name in $BFT_EXTRACT_DIR")
    _range(q)
    q.add_argument("--peak", type=float, help="follow the reference peak nearest this frequency (Hz)")
    q.add_argument("--window", type=float, default=0.1, help="analysis window (s)")
    q.add_argument("--hop", type=float, default=0.025, help="step between windows (s)")
    q.add_argument("--speeds", default="0.5,2", help="speed range searched, lo,hi")
    q.add_argument("--min-salience", type=float, default=10.0, help="dB above the band's median to plot")
    q.add_argument("--rate", type=int, default=24000, help="analysis sample rate")
    q.add_argument("--out", help="CSV (default: scratch); the plot is the same name with .svg")

    c = sub.add_parser("changes", help="frames where a screen region changes")
    c.add_argument("rec")
    c.add_argument("--roi", required=True, help="x,y,w,h in recording pixels")
    _range(c)
    c.add_argument("--threshold", type=float, default=2.0, help="mean |Δ| (0–255) for a change")
    c.add_argument("--gap", type=int, default=1, help="changed frames this close are one change")
    c.add_argument("--out", help="CSV of candidate marks (default: scratch)")
    c.add_argument("--trace", help="also write every frame's difference to this CSV")

    r = sub.add_parser("result", help="run an item's analysis and write its entry")
    r.add_argument("item")
    r.add_argument("--print", action="store_true", help="print the entry instead of writing it")

    sub.add_parser("check", help="validate every markset and item file")
    return ap


def main(argv=None) -> int:
    args = parser().parse_args(argv)
    try:
        if args.command == "session":
            import session
            {"new": session.new, "add": session.add, "stamp": session.stamp_cmd}[args.action](args)
        elif args.command == "mark":
            import mark
            mark.mark(args)
        elif args.command == "result":
            import result
            result.result(args)
        elif args.command == "check":
            import result
            result.check(args)
        else:
            {"probe": cmd_probe, "onsets": cmd_onsets, "pitch": cmd_pitch, "changes": cmd_changes}[args.command](args)
    except ObserveError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
