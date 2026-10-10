"""Tests for tools/observe on synthetic media made by ffmpeg in a temp dir.

Run from the repository root:

    python3 -m unittest discover -s tools/observe/tests -v

Skipped without ffmpeg or numpy; the mpv test is skipped without mpv.
"""

from __future__ import annotations

import contextlib
import csv
import io
import json
import os
import pathlib
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest

TOOL = pathlib.Path(__file__).resolve().parent.parent
sys.path.insert(0, str(TOOL))

try:
    import numpy  # noqa: F401
    HAVE_NUMPY = True
except ImportError:
    HAVE_NUMPY = False
HAVE_FFMPEG = bool(shutil.which("ffmpeg") and shutil.which("ffprobe"))

import observe  # noqa: E402

FPS = 60
FRAME = 1 / FPS


def ffmpeg(*args: str) -> None:
    subprocess.run(["ffmpeg", "-v", "error", "-nostdin", "-y", *args], check=True)


def run(*argv: str) -> str:
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        code = observe.main(list(argv))
    if code:
        raise AssertionError(f"observe {' '.join(argv)} failed:\n{out.getvalue()}")
    return out.getvalue()


@unittest.skipUnless(HAVE_FFMPEG and HAVE_NUMPY, "needs ffmpeg, ffprobe and numpy")
class SyntheticMedia(unittest.TestCase):
    def setUp(self):
        self.tmp = pathlib.Path(tempfile.mkdtemp(prefix="observe-test-"))
        self.env = {k: os.environ.get(k) for k in
                    ("OBSERVE_DOCS", "OBSERVE_SCRATCH", "OBSERVE_ITEMS", "OBSERVE_MARKSETS", "OBSERVE_NO_STAMP",
                     "OBFT_EVIDENCE_DIR", "PATH")}
        self.evidence = self.tmp / "evidence"
        self.evidence.mkdir()
        os.environ["OBSERVE_DOCS"] = str(self.tmp / "docs")
        os.environ["OBFT_EVIDENCE_DIR"] = str(self.evidence)
        os.environ.pop("OBSERVE_SCRATCH", None)  # the default: <evidence>/scratch
        os.environ["OBSERVE_NO_STAMP"] = "1"

    def tearDown(self):
        for k, v in self.env.items():
            if v is None:
                os.environ.pop(k, None)
            else:
                os.environ[k] = v
        shutil.rmtree(self.tmp, ignore_errors=True)

    # -- media makers ------------------------------------------------------

    def wav(self, name: str, source: str, rate: int = 22050) -> pathlib.Path:
        path = self.tmp / name
        ffmpeg("-f", "lavfi", "-i", source, "-ar", str(rate), "-ac", "1", "-c:a", "pcm_s16le", str(path))
        return path

    def recording(self, name: str, video: str, audio: list[str] | None = None,
                  seconds: float = 3.0) -> pathlib.Path:
        """A lossless mkv: `video` is a lavfi graph at 60 fps; `audio` more
        ffmpeg arguments: inputs after the video and the maps (or silence).
        It goes in the evidence directory, where recordings live."""
        path = self.evidence / name
        args = ["-f", "lavfi", "-i", video]
        if audio is None:
            args += ["-f", "lavfi", "-i", f"anullsrc=r=48000:cl=stereo:d={seconds}", "-map", "0:v", "-map", "1:a"]
        else:
            args += audio
        args += ["-t", str(seconds), "-c:v", "ffv1", "-c:a", "pcm_s16le", "-ar", "48000", str(path)]
        ffmpeg(*args)
        return path

    # -- tests -------------------------------------------------------------

    def test_pitch_of_a_sped_up_tone(self):
        ref = self.wav("tone.wav", "sine=frequency=440:duration=1", rate=22050)
        speed = 1.25
        rec = self.recording(
            "pitch.mkv", f"color=gray:s=320x240:r={FPS}:d=3",
            ["-i", str(ref), "-filter_complex",
                         f"[1:a]asetrate={22050 * speed:.0f},aresample=48000,adelay=500:all=1,apad[a]",
                         "-map", "0:v", "-map", "[a]"])
        out = self.tmp / "pitch.csv"
        run("pitch", str(rec), "--ref", str(ref), "--out", str(out))
        with open(out) as fh:
            rows = [r for r in csv.DictReader(fh) if 0.6 <= float(r["time"]) <= 1.2]
        self.assertGreater(len(rows), 10)
        for r in rows:
            self.assertAlmostEqual(float(r["speed"]), speed, delta=speed * 0.005, msg=r)
        self.assertTrue(out.with_suffix(".svg").is_file())

    def test_onsets_of_a_click(self):
        ref = self.wav("click.wav", "anoisesrc=d=0.05:c=white:a=0.5:seed=3")
        times = (1.25, 2.5)
        graph = ("[2:a]aresample=48000,asplit=2[s0][s1];"
                 + ";".join(f"[s{i}]adelay={int(t * 1000)}:all=1[c{i}]" for i, t in enumerate(times))
                 + ";[1:a][c0][c1]amix=inputs=3:normalize=0:duration=first[a]")
        rec = self.recording(
            "click.mkv", f"color=gray:s=320x240:r={FPS}:d=4",
            ["-f", "lavfi", "-i", "anoisesrc=d=4:c=pink:r=48000:a=0.01:seed=7", "-i", str(ref),
                         "-filter_complex", graph, "-map", "0:v", "-map", "[a]"], seconds=4)
        out = self.tmp / "onsets.csv"
        run("onsets", str(rec), "--ref", str(ref), "--out", str(out))
        with open(out) as fh:
            rows = list(csv.DictReader(fh))
        self.assertEqual(len(rows), len(times), rows)
        for r, t in zip(rows, times):
            self.assertLess(abs(float(r["time"]) - t), FRAME, r)
            self.assertEqual(int(r["frame"]), int(t * FPS))
            self.assertGreater(float(r["confidence"]), 0.8)

    def test_changes_in_a_region(self):
        change = 97
        rec = self.recording(
            "change.mkv",
            f"color=gray:s=320x240:r={FPS}:d=3,"
            f"drawbox=x=100:y=50:w=40:h=40:color=red:t=fill:enable='gte(n,{change})',"
            f"drawbox=x=10:y=10:w=20:h=20:color=blue:t=fill:enable='gte(n,30)'")
        out = self.tmp / "changes.csv"
        run("changes", str(rec), "--roi", "100,50,40,40", "--out", str(out))
        with open(out) as fh:
            rows = list(csv.DictReader(fh))
        self.assertEqual([int(r["frame"]) for r in rows], [change])
        self.assertAlmostEqual(float(rows[0]["time"]), change / FPS, places=4)
        # the same from a later start, to check seeking keeps frame numbers
        run("changes", str(rec), "--roi", "100,50,40,40", "--from", "1.0", "--out", str(out))
        with open(out) as fh:
            self.assertEqual([int(r["frame"]) for r in csv.DictReader(fh)], [change])

    def test_probe_game_frame_rate_and_sync(self):
        flash = 90
        click = self.wav("sync.wav", "anoisesrc=d=0.05:c=white:a=0.5:seed=5")
        offset = 0.05
        rec = self.recording(
            "probe.mkv",
            f"testsrc2=s=320x240:r=30:d=3,fps={FPS},"
            f"drawbox=x=0:y=0:w=320:h=240:color=white:t=fill:enable='eq(n,{flash})'",
            ["-i", str(click), "-filter_complex",
                         f"[1:a]aresample=48000,adelay={int((flash / FPS + offset) * 1000)}:all=1,apad[a]",
                         "-map", "0:v", "-map", "[a]"])
        marks = self.tmp / "sync.csv"
        marks.write_text(f"frame,time,label,note\n{flash},{flash / FPS:.6f},sync,\"\"\n")
        text = run("probe", str(rec), "--sync-ref", str(click), "--marks", str(marks))
        rate = float(text.split("game frame rate: ")[1].split(" fps")[0])
        self.assertAlmostEqual(rate, 30.0, delta=0.5)
        measured = float(text.split("audio − video offset: ")[1].split(" ±")[0])
        self.assertLess(abs(measured - offset), FRAME)

    def test_evidence_stays_outside_the_repository(self):
        outside = self.tmp / "elsewhere.mkv"
        shutil.copy(self.recording("in.mkv", f"color=gray:s=320x240:r={FPS}:d=1", seconds=1), outside)
        import session
        with self.assertRaisesRegex(observe.ObserveError, "not inside the evidence directory"):
            session._recording_row(outside, None)
        os.environ["OBFT_EVIDENCE_DIR"] = str(TOOL)
        from common import evidence_dir
        with self.assertRaisesRegex(observe.ObserveError, "inside the repository"):
            evidence_dir()

    def test_thresholds_go_into_the_session(self):
        rec = self.recording("fps.mkv", f"testsrc2=s=320x240:r=30:d=3,fps={FPS}")
        run("session", "new", "--id", "2026-01-02-a", "--rec", str(rec), "--pixel-threshold", "8")
        text = (self.tmp / "docs" / "sessions" / "2026-01-02-a.md").read_text()
        self.assertIn("pixel_threshold=8, fraction=0.002, min_rate=10", text)
        self.assertIn("Game frame rate (measured): 30.00 fps", text)
        other = self.recording("fps2.mkv", f"testsrc2=s=320x240:r=30:d=3,fps={FPS}")
        out = io.StringIO()
        with contextlib.redirect_stderr(out):
            self.assertEqual(observe.main(["session", "add", "2026-01-02-a", "--rec", str(other),
                                           "--pixel-threshold", "6"]), 1)
        self.assertIn("start a new session", out.getvalue())
        run("session", "add", "2026-01-02-a", "--rec", str(other))  # takes the session's own
        self.assertIn("frame-change thresholds: pixel_threshold=8", run("probe", str(other)))

    def test_stamps_are_numbered_per_version(self):
        bin_dir = self.tmp / "bin"
        bin_dir.mkdir()
        fake = bin_dir / "ots"  # writes a proof header for the file's SHA-256, as ots does
        fake.write_text(f"""#!{sys.executable}
import hashlib, sys
sys.path.insert(0, {str(TOOL)!r})
from common import OTS_MAGIC
path = sys.argv[2]
digest = hashlib.sha256(open(path, "rb").read()).digest()
open(path + ".ots", "wb").write(OTS_MAGIC + bytes([1, 8]) + digest + b"\\xf0")
""")
        fake.chmod(0o755)
        os.environ["PATH"] = f"{bin_dir}{os.pathsep}{os.environ['PATH']}"
        os.environ.pop("OBSERVE_NO_STAMP")
        rec = self.recording("stamp.mkv", f"color=gray:s=320x240:r={FPS}:d=1", seconds=1)
        session = self._session(rec)
        sessions = self.tmp / "docs" / "sessions"
        first = sessions / f"{session}.md.1.ots"
        self.assertTrue(first.is_file())
        before = first.read_bytes()
        run("session", "stamp", session)  # unchanged: no new proof
        self.assertEqual(sorted(p.name for p in sessions.glob("*.ots")), [first.name])
        path = sessions / f"{session}.md"
        path.write_text(path.read_text().replace("(fill in)", "86Box", 1))
        run("session", "stamp", session)
        self.assertEqual(sorted(p.name for p in sessions.glob("*.ots")), [first.name, f"{session}.md.2.ots"])
        self.assertEqual(first.read_bytes(), before)

    def test_result_applies_the_audio_video_offset(self):
        items = self.tmp / "items"
        marksets = self.tmp / "marksets"
        items.mkdir()
        marksets.mkdir()
        os.environ["OBSERVE_ITEMS"] = str(items)
        os.environ["OBSERVE_MARKSETS"] = str(marksets)
        (marksets / "go.toml").write_text('name = "go"\ndescription = "test"\n'
                                          '[[key]]\nkey = "s"\nlabel = "start"\nhelp = "start"\n')
        (marksets / "sync.toml").write_text((TOOL / "marksets" / "sync.toml").read_text())
        (items / "go-1.toml").write_text("""
id = "go-1"
topic = "go"
question = "How long from the start to the click?"
precision = "approximate"
markset = "go"
method = "Synthetic."
recordings = [{ session = "2026-01-01-a", file = "av.mkv" }]

[[audio]]
label = "click"
ref = "click.wav"

[[analysis]]
name = "start to click"
type = "duration"
from = "start"
to = "click"
""")
        click = self.wav("click.wav", "anoisesrc=d=0.05:c=white:a=0.5:seed=5")
        item = items / "go-1.toml"
        item.write_text(item.read_text().replace('ref = "click.wav"', f'ref = "{click}"'))
        offset, flash = 0.05, 90
        graph = ("[1:a]aresample=48000,asplit=2[s0][s1];"
                 f"[s0]adelay={int((flash / FPS + offset) * 1000)}:all=1[c0];"
                 f"[s1]adelay={int((2.5 + offset) * 1000)}:all=1[c1];"
                 "[c0][c1]amix=inputs=2:normalize=0,apad[a]")
        rec = self.recording(
            "av.mkv", f"color=gray:s=320x240:r={FPS}:d=3,"
                      f"drawbox=x=0:y=0:w=320:h=240:color=white:t=fill:enable='eq(n,{flash})'",
            ["-i", str(click), "-filter_complex", graph, "-map", "0:v", "-map", "[a]"])
        session = self._session(rec, "--game-fps", "30")
        sync = self.evidence / session / f"{session}.av.sync.csv"
        sync.parent.mkdir(parents=True, exist_ok=True)
        sync.write_text(f"frame,time,label,note\n{flash},{flash / FPS:.6f},sync,\"\"\n")
        (self.evidence / session / f"{session}.av.go.csv").write_text(
            "frame,time,label,note\n120,2.000000,start,\"\"\n")
        with self.assertRaisesRegex(observe.ObserveError, "no measured audio − video offset"):
            import result
            result.load_recordings(result.load_item("go-1"), result.NoteView())
        run("probe", str(rec), "--sync-ref", str(click), "--record")
        cell = (self.tmp / "docs" / "sessions" / f"{session}.md").read_text()
        self.assertIn("| +0.05", cell)
        err = io.StringIO()
        with contextlib.redirect_stderr(err), contextlib.redirect_stdout(io.StringIO()):  # not rewritten
            self.assertEqual(observe.main(["probe", str(rec), "--sync-ref", str(click), "--record"]), 1)
        run("result", "go-1")
        text = (self.tmp / "docs" / "go.md").read_text()
        # the click at 2.55 s in the audio is at 2.5 s on the video's clock: frame 150
        self.assertIn("rows 1→a2, 30 recorded frames = 0.500 ± 0.033 s", text)
        self.assertIn("recording 1 +0.05", text)
        self.assertIn("**Audio events.**", text)

    def _session(self, rec: pathlib.Path, *extra: str) -> str:
        run("session", "new", "--id", "2026-01-01-a", "--date", "2026-01-01", "--rec", str(rec),
            "--no-probe", *extra)
        return "2026-01-01-a"

    def test_result_from_hand_written_marks(self):
        items = self.tmp / "items"
        marksets = self.tmp / "marksets"
        items.mkdir()
        marksets.mkdir()
        os.environ["OBSERVE_ITEMS"] = str(items)
        os.environ["OBSERVE_MARKSETS"] = str(marksets)
        (marksets / "demo.toml").write_text("""
name = "demo"
description = "test"
[[key]]
key = "g"
label = "group"
help = "starts a group"
note = true
[[key]]
key = "s"
label = "start"
help = "start"
[[key]]
key = "e"
label = "end"
help = "end"
[[key]]
key = "y"
label = "yes"
help = "yes"
[[key]]
key = "n"
label = "no"
help = "no"
[[key]]
key = "a"
label = "pa"
help = "point a"
pos = true
[[key]]
key = "b"
label = "pb"
help = "point b"
pos = true
[[key]]
key = "v"
label = "reading"
help = "a value"
note = true
[[key]]
key = "w"
label = "said"
help = "wording the question is about"
note = true
wording = true
[[key]]
key = "x"
label = "shown"
help = "other wording"
note = true
wording = true
""")
        (items / "demo-1.toml").write_text("""
id = "demo-1"
topic = "demo"
question = "Does it work?"
precision = "exact"
markset = "demo"
method = "Synthetic."
quote = ["said"]
recordings = [{ session = "2026-01-01-a", file = "result.mkv" }]

[[analysis]]
name = "words"
type = "order"
labels = ["said", "shown"]

[[analysis]]
name = "answers"
type = "count"
labels = ["yes", "no"]
group = "group"

[[analysis]]
name = "sequence"
type = "order"
labels = ["start", "yes", "end"]

[[analysis]]
name = "wait"
type = "duration"
from = "start"
to = "end"

[[analysis]]
name = "size"
type = "ratio"
numerator = { distance = ["pa", "pb"] }
denominator = { dx = ["pa", "pb"] }
group = "group"

[[analysis]]
name = "trend"
type = "linear_fit"
label = "reading"
y = "value"

[[analysis]]
name = "decay"
type = "exp_tau"
label = "reading"
y = "value"
""")
        rec = self.recording("result.mkv", f"color=gray:s=320x240:r={FPS}:d=1", seconds=1)
        session = self._session(rec, "--game-fps", "30")
        # readings: an exponential, y = 10 + 50·exp(−t/0.5), every 0.25 s
        import math
        readings = "".join(f"{60 + 15 * i},{(60 + 15 * i) / FPS:.6f},reading,"
                           f"\"{10 + 50 * math.exp(-0.25 * i / 0.5):.4f}\"\n" for i in range(8))
        path = self.evidence / session / f"{session}.result.demo.csv"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("frame,time,label,note\n"
                        "0,0.000000,group,\"first\"\n"
                        "10,0.166667,start,\"\"\n"
                        "20,0.333333,yes,\"\"\n"
                        "130,2.166667,end,\"\"\n"
                        "40,0.666667,pa,\"x=100.0 y=100.0\"\n"
                        "41,0.683333,pb,\"x=130.0 y=140.0\"\n"
                        "42,0.700000,no,\"\"\n"
                        "43,0.716667,void,\"row 7\"\n"
                        "50,0.833333,group,\"second\"\n"
                        "51,0.850000,no,\"\"\n"
                        "52,0.866667,not-observable,\"the floor was dark\"\n"
                        + readings
                        + "170,2.833333,said,\"Quoted Words\"\n"
                          "171,2.850000,said,\"Secret A; key: MSG_A\"\n"
                          "172,2.866667,shown,\"Secret B; desc: a warning\"\n"
                          "173,2.883333,shown,\"Secret C\"\n")
        # an OpenTimestamps proof of this marks file beside the session file, and a
        # stale one of another version
        from common import OTS_MAGIC, sha256_file
        proofs = self.tmp / "docs" / "sessions"
        proof = proofs / f"{path.name}.2.ots"
        proof.write_bytes(OTS_MAGIC + b"\x01\x08" + bytes.fromhex(sha256_file(path)) + b"\xf0")
        stale = proofs / f"{path.name}.1.ots"
        stale.write_bytes(OTS_MAGIC + b"\x01\x08" + bytes(32) + b"\xf0")
        run("result", "demo-1")
        topic = self.tmp / "docs" / "demo.md"
        text = topic.read_text()
        self.assertIn("## demo-1", text)
        self.assertIn("yes 1, no 1 (over 2 groups)", text)  # row 7 was voided
        self.assertIn("1: first: yes 1, no 0", text)
        self.assertIn("(exact): recording 1: start → yes → end", text)
        self.assertIn("120 recorded frames = 2.000 ± 0.033 s", text)  # ± one frame at 30 fps
        self.assertIn("first: 1.67 ± 0.18", text)  # 50 ± 2.8 px / 30 ± 2.8 px
        self.assertIn("share `pa`, `pb`; the ± treats their errors as independent", text)
        self.assertIn("the floor was dark", text)
        self.assertIn("~~7~~", text)
        tau = float(text.split("τ = ")[1].split(" s")[0])
        self.assertAlmostEqual(tau, 0.5, delta=0.005)
        self.assertIn("**Kind.** exact (words, answers, sequence); approximate (wait, size, trend, decay)", text)
        # wording: the key, else our description, else quoted only for a `quote` label
        self.assertIn("said (“Quoted Words”) → said (key: MSG_A) → shown (a warning) → "
                      "shown ((wording in the marks file))", text)
        for secret in ("Secret A", "Secret B", "Secret C"):
            self.assertNotIn(secret, text)
        # the marks file by hash, with the proof of exactly this version
        self.assertIn(f"`{sha256_file(path)}` | {proof.name} |", text)
        self.assertNotIn(stale.name, text)
        # a second run replaces the entry and keeps the rest of the file
        topic.write_text(text + "\nSomething else.\n")
        run("result", "demo-1")
        again = topic.read_text()
        self.assertEqual(again.count("## demo-1"), 1)
        self.assertIn("Something else.", again)
        # quoting is only for labels an analysis uses and that record wording
        import result
        item = items / "demo-1.toml"
        item.write_text(item.read_text().replace('quote = ["said"]', 'quote = ["start"]'))
        with self.assertRaisesRegex(observe.ObserveError, "not a wording label"):
            result.load_item("demo-1")

    @unittest.skipUnless(shutil.which("mpv"), "needs mpv")
    def test_mark_in_mpv(self):
        marksets = self.tmp / "marksets"
        marksets.mkdir()
        os.environ["OBSERVE_MARKSETS"] = str(marksets)
        (marksets / "keys.toml").write_text(
            'name = "keys"\ndescription = "test"\n'
            '[[key]]\nkey = "a"\nlabel = "alpha"\nhelp = "alpha"\n')
        rec = self.recording("mark.mkv", f"color=gray:s=320x240:r={FPS}:d=3")
        session = self._session(rec)
        sock = self.tmp / "mpv.sock"
        proc = subprocess.Popen(
            [sys.executable, str(TOOL / "observe.py"), "mark", str(rec), "--set", "keys",
             "--mpv-arg=--vo=null", "--mpv-arg=--ao=null", f"--mpv-arg=--input-ipc-server={sock}"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=os.environ.copy())
        try:
            for _ in range(100):
                if sock.exists():
                    break
                time.sleep(0.1)
            client = socket.socket(socket.AF_UNIX)
            client.connect(str(sock))
            reader = client.makefile("r")

            def send(*command):
                client.sendall((json.dumps({"command": list(command), "request_id": 1}) + "\n").encode())
                while True:
                    reply = json.loads(reader.readline())
                    if reply.get("request_id") == 1:
                        return reply

            for _ in range(100):
                if send("get_property", "time-pos").get("error") == "success":
                    break
                time.sleep(0.05)
            for frame in (90, 120, 150):
                send("seek", f"{frame / FPS:.6f}", "absolute+exact")
                for _ in range(100):
                    t = send("get_property", "time-pos").get("data")
                    if t is not None and abs(t - frame / FPS) < 1e-3:
                        break
                    time.sleep(0.05)
                send("keypress", "a")
            send("keypress", "BS")
            time.sleep(0.3)
            send("quit")
            reader.close()
            client.close()
            proc.wait(timeout=20)
        finally:
            if proc.poll() is None:
                proc.kill()
        path = self.evidence / session / f"{session}.mark.keys.csv"
        with open(path) as fh:
            rows = list(csv.DictReader(fh))
        self.assertEqual([(int(r["frame"]), r["label"]) for r in rows[:3]],
                         [(90, "alpha"), (120, "alpha"), (150, "alpha")])
        self.assertAlmostEqual(float(rows[0]["time"]), 1.5, places=4)
        self.assertEqual((rows[3]["label"], rows[3]["note"]), ("void", "row 3"))


class ShippedFiles(unittest.TestCase):
    """The item and markset files in the repository, and plan.md."""

    def test_items_and_marksets_load(self):
        text = run("check")
        self.assertNotIn("FAIL", text)
        self.assertEqual(text.count("ok "), len(list((TOOL / "items").glob("*.toml"))))

    def test_items_match_the_plan_word_for_word(self):
        import re
        import tomllib
        plan = (TOOL.parent.parent / "docs" / "observations" / "plan.md").read_text()
        items = plan.split("## Items", 1)[1]
        entries = re.findall(r"^- \*\*([a-z0-9-]+)\*\* (.*?)(?=^- |\Z)", items, re.M | re.S)
        self.assertEqual(len(entries), len(list((TOOL / "items").glob("*.toml"))))
        for item_id, text in entries:
            text = " ".join(text.split())
            question, precision = re.fullmatch(r"(.*) \((.*)\)", text).groups()
            with open(TOOL / "items" / f"{item_id}.toml", "rb") as fh:
                item = tomllib.load(fh)
            self.assertEqual(" ".join(item["question"].split()), question, item_id)
            self.assertEqual(item["precision"], precision, item_id)
            self.assertEqual(item.get("recordings"), [], f"{item_id}: nothing has been recorded yet")


if __name__ == "__main__":
    unittest.main()
