"""Audio measurements against a known sound from the game's data.

`onsets` finds where the reference plays in a recording by normalised
cross-correlation; the peak value (1 = the same waveform) is the
confidence. `pitch` follows one spectral peak of the reference through the
recording; the ratio of the two frequencies is the playback speed.

The reference is a WAV file, or a sound name looked up in the extraction
cache ($BFT_EXTRACT_DIR). A sound taken from the cache is decoded from a
temporary directory and never written anywhere else.
"""

from __future__ import annotations

import os
import pathlib
import tempfile
import zipfile
from dataclasses import dataclass

from common import Media, ObserveError, decode_audio, numpy

# The extraction cache's sound archive (engine layout, then tools/recon's).
SOUND_ARCHIVES = ("sounds/soundsh.snd.zip", "soundsh.zip")


def load_ref(ref: str, rate: int):
    """The reference sound as mono samples at `rate` Hz."""
    path = pathlib.Path(ref)
    if path.is_file():
        return decode_audio(path, rate)
    name = path.name if path.suffix.lower() == ".wav" else path.name + ".wav"
    extract = os.environ.get("BFT_EXTRACT_DIR")
    if not extract:
        raise ObserveError(f"{ref} is not a file, and $BFT_EXTRACT_DIR is not set to look it up")
    for archive in SOUND_ARCHIVES:
        zpath = pathlib.Path(extract) / archive
        if not zpath.is_file():
            continue
        with zipfile.ZipFile(zpath) as zf:
            names = {pathlib.PurePosixPath(n).name.lower(): n for n in zf.namelist()}
            if name.lower() in names:
                with tempfile.TemporaryDirectory() as tmp:
                    wav = pathlib.Path(tmp) / name
                    wav.write_bytes(zf.read(names[name.lower()]))
                    return decode_audio(wav, rate)
    raise ObserveError(f"{name} is in none of {', '.join(SOUND_ARCHIVES)} under {extract}")


def recording_audio(media: Media, rate: int, start: float | None, end: float | None):
    """(samples, time of the first sample) for the stretch start..end."""
    if not media.audio_rate:
        raise ObserveError(f"{media.path.name} has no audio stream")
    x = decode_audio(media.path, rate)
    t0 = media.audio_offset
    lo = max(0, round(((start if start is not None else t0) - t0) * rate))
    hi = len(x) if end is None else max(lo, round((end - t0) * rate))
    return x[lo:hi], t0 + lo / rate


# --------------------------------------------------------------------------
# onsets
# --------------------------------------------------------------------------


def ncc(x, r, floor_db: float = -60.0):
    """Normalised cross-correlation of reference r at every offset in x.

    Windows quieter than `floor_db` (RMS, full scale = 0 dB) score 0.
    """
    np = numpy()
    n = len(r)
    if len(x) < n or n == 0:
        return np.zeros(0)
    out_len = len(x) - n + 1
    nfft = 1 << int(np.ceil(np.log2(max(4 * n, 1 << 16))))
    step = nfft - n + 1
    spec_r = np.conj(np.fft.rfft(r, nfft))
    num = np.empty(out_len)
    for s in range(0, out_len, step):
        c = np.fft.irfft(np.fft.rfft(x[s:s + nfft], nfft) * spec_r, nfft)
        k = min(step, out_len - s)
        num[s:s + k] = c[:k]
    cum = np.concatenate(([0.0], np.cumsum(x * x)))
    energy = np.maximum(cum[n:] - cum[:-n], 0.0)
    quiet = energy / n < 10 ** (floor_db / 10)
    den = np.linalg.norm(r) * np.sqrt(energy)
    with np.errstate(divide="ignore", invalid="ignore"):
        out = np.where(quiet | (den == 0), 0.0, num / den)
    return np.clip(out, -1.0, 1.0)


def pick_peaks(c, threshold: float, min_gap: int):
    """Local maxima of c at or above threshold, strongest first claiming
    ±min_gap samples around them; returned in time order."""
    np = numpy()
    if len(c) < 3:
        return []
    mid = c[1:-1]
    idx = np.where((mid >= threshold) & (mid >= c[:-2]) & (mid > c[2:]))[0] + 1
    blocked = np.zeros(len(c), dtype=bool)
    taken = []
    for k in idx[np.argsort(-c[idx], kind="stable")]:
        if blocked[k]:
            continue
        taken.append(int(k))
        blocked[max(0, k - min_gap):k + min_gap + 1] = True
    return sorted(taken)


def _parabolic(y_m: float, y_0: float, y_p: float) -> float:
    den = y_m - 2 * y_0 + y_p
    return 0.0 if den == 0 else max(-0.5, min(0.5, 0.5 * (y_m - y_p) / den))


@dataclass
class Onset:
    time: float
    frame: int | None
    confidence: float


def onsets(media: Media, ref, rate: int, threshold: float, min_gap: float | None,
           start: float | None = None, end: float | None = None) -> list[Onset]:
    x, t0 = recording_audio(media, rate, start, end)
    c = ncc(x, ref)
    gap = round((min_gap if min_gap is not None else len(ref) / rate / 2) * rate)
    found = []
    for k in pick_peaks(c, threshold, max(1, gap)):
        d = _parabolic(c[k - 1], c[k], c[k + 1]) if 0 < k < len(c) - 1 else 0.0
        t = t0 + (k + d) / rate
        found.append(Onset(t, media.frame_at(t) if media.fps else None, float(c[k])))
    return found


# --------------------------------------------------------------------------
# pitch
# --------------------------------------------------------------------------


def _frames(x, n: int, hop: int):
    np = numpy()
    if len(x) < n:
        return np.zeros((0, n))
    return np.lib.stride_tricks.sliding_window_view(x, n)[::hop]


def _nfft(n: int) -> int:
    return 1 << (4 * n - 1).bit_length()


def ref_peaks(ref, rate: int, window: float, count: int = 5):
    """The strongest spectral peaks of the reference (Hz, dB), strongest first."""
    np = numpy()
    n = max(16, round(window * rate))
    if len(ref) < n:
        ref = np.pad(ref, (0, n - len(ref)))
    nfft = _nfft(n)
    w = np.hanning(n)
    power = np.mean(np.abs(np.fft.rfft(_frames(ref, n, max(1, n // 2)) * w, nfft, axis=1)) ** 2, axis=0)
    logp = 10 * np.log10(power + 1e-30)
    mid = logp[1:-1]
    idx = np.where((mid > logp[:-2]) & (mid >= logp[2:]))[0] + 1
    idx = idx[np.argsort(-logp[idx])][:count]
    peaks = []
    for k in idx:
        d = _parabolic(logp[k - 1], logp[k], logp[k + 1])
        peaks.append(((k + d) * rate / nfft, float(logp[k] - logp.max())))
    return peaks


@dataclass
class PitchPoint:
    time: float
    frame: int | None
    freq: float
    speed: float
    salience: float


def track_pitch(media: Media, rate: int, f_ref: float, window: float, hop: float,
                speeds: tuple[float, float], start: float | None = None,
                end: float | None = None) -> list[PitchPoint]:
    """The peak near f_ref × speed, frame by frame.

    Salience is the peak's height above the median of the search band, in
    dB: low values mean the reference's peak is not what was found.
    """
    np = numpy()
    x, t0 = recording_audio(media, rate, start, end)
    n = max(16, round(window * rate))
    step = max(1, round(hop * rate))
    nfft = _nfft(n)
    lo = int(np.floor(f_ref * speeds[0] * nfft / rate))
    hi = int(np.ceil(f_ref * speeds[1] * nfft / rate))
    if lo < 1 or hi >= nfft // 2:
        raise ObserveError(f"the search band {f_ref * speeds[0]:.0f}–{f_ref * speeds[1]:.0f} Hz "
                           f"does not fit below {rate // 2} Hz; change --speeds or --rate")
    w = np.hanning(n)
    frames = _frames(x, n, step)
    points = []
    for b in range(0, len(frames), 256):
        mag = np.abs(np.fft.rfft(frames[b:b + 256] * w, nfft, axis=1))
        logm = 20 * np.log10(mag + 1e-30)
        for i in range(len(logm)):
            band = logm[i, lo:hi + 1]
            k = lo + int(np.argmax(band))
            d = _parabolic(logm[i, k - 1], logm[i, k], logm[i, k + 1])
            f = (k + d) * rate / nfft
            t = t0 + ((b + i) * step + n / 2) / rate
            points.append(PitchPoint(t, media.frame_at(t) if media.fps else None, f, f / f_ref,
                                     float(logm[i, k] - np.median(band))))
    return points
