"""A small dependency-free SVG line plot (pitch: speed over time).

One series, so no legend: the title names it. Hovering a point shows its
values. Light and dark colours follow the viewer's colour scheme.
"""

from __future__ import annotations

import html
import math
import pathlib

WIDTH, HEIGHT = 720, 360
LEFT, RIGHT, TOP, BOTTOM = 64, 24, 48, 48

STYLE = """
  .surface { fill: #fcfcfb; }
  .grid { stroke: #e6e5e1; stroke-width: 1; }
  .ref { stroke: #898781; stroke-width: 1; }
  .axis { fill: #898781; font: 12px system-ui, sans-serif; }
  .title { fill: #0b0b0b; font: 600 14px system-ui, sans-serif; }
  .sub { fill: #52514e; font: 12px system-ui, sans-serif; }
  .line { fill: none; stroke: #2a78d6; stroke-width: 2; stroke-linejoin: round; stroke-linecap: round; }
  .dot { fill: #2a78d6; stroke: #fcfcfb; stroke-width: 2; }
  .hit { fill: transparent; }
  .hit:hover + .dot, .dot:hover { r: 5; }
  @media (prefers-color-scheme: dark) {
    .surface { fill: #1a1a19; }
    .grid { stroke: #383835; }
    .title { fill: #ffffff; }
    .sub { fill: #c3c2b7; }
    .line { stroke: #3987e5; }
    .dot { fill: #3987e5; stroke: #1a1a19; }
  }
"""


def _ticks(lo: float, hi: float, count: int = 5) -> list[float]:
    if hi <= lo:
        hi = lo + 1
    raw = (hi - lo) / count
    mag = 10 ** math.floor(math.log10(raw))
    step = next(m * mag for m in (1, 2, 2.5, 5, 10) if m * mag >= raw)
    first = math.ceil(lo / step) * step
    return [first + i * step for i in range(int((hi - first) / step + 1e-9) + 1)]


def _fmt(v: float) -> str:
    return f"{v:.6g}"


def line_plot(path: pathlib.Path, title: str, subtitle: str, x_label: str, y_label: str,
              points: list[tuple[float, float, str]], ref_y: float | None = None) -> None:
    """points: (x, y, hover text); a None y breaks the line."""
    shown = [(x, y, t) for x, y, t in points if y is not None]
    xs = [p[0] for p in points] or [0.0, 1.0]
    ys = [p[1] for p in shown] + ([ref_y] if ref_y is not None else [])
    ys = ys or [0.0, 1.0]
    x0, x1 = min(xs), max(xs)
    y0, y1 = min(ys), max(ys)
    pad = (y1 - y0) * 0.1 or abs(y0) * 0.01 or 1.0
    y0, y1 = y0 - pad, y1 + pad
    x1 = x1 if x1 > x0 else x0 + 1
    pw, ph = WIDTH - LEFT - RIGHT, HEIGHT - TOP - BOTTOM

    def sx(v: float) -> float:
        return LEFT + (v - x0) / (x1 - x0) * pw

    def sy(v: float) -> float:
        return TOP + (1 - (v - y0) / (y1 - y0)) * ph

    out = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" '
           f'viewBox="0 0 {WIDTH} {HEIGHT}" role="img" aria-label="{html.escape(title)}">',
           f"<style>{STYLE}</style>",
           f'<rect class="surface" width="{WIDTH}" height="{HEIGHT}"/>',
           f'<text class="title" x="{LEFT}" y="20">{html.escape(title)}</text>',
           f'<text class="sub" x="{LEFT}" y="36">{html.escape(subtitle)}</text>']
    for t in _ticks(y0, y1):
        out.append(f'<line class="grid" x1="{LEFT}" x2="{LEFT + pw}" y1="{sy(t):.1f}" y2="{sy(t):.1f}"/>')
        out.append(f'<text class="axis" x="{LEFT - 6}" y="{sy(t) + 4:.1f}" text-anchor="end">{_fmt(t)}</text>')
    for t in _ticks(x0, x1, 8):
        out.append(f'<text class="axis" x="{sx(t):.1f}" y="{TOP + ph + 18}" text-anchor="middle">{_fmt(t)}</text>')
    out.append(f'<text class="axis" x="{LEFT + pw / 2}" y="{HEIGHT - 8}" '
               f'text-anchor="middle">{html.escape(x_label)}</text>')
    out.append(f'<text class="axis" transform="translate(14 {TOP + ph / 2}) rotate(-90)" '
               f'text-anchor="middle">{html.escape(y_label)}</text>')
    if ref_y is not None:
        out.append(f'<line class="ref" x1="{LEFT}" x2="{LEFT + pw}" y1="{sy(ref_y):.1f}" y2="{sy(ref_y):.1f}"/>')
    segment: list[str] = []
    for x, y, _ in points + [(0.0, None, "")]:
        if y is None:
            if len(segment) > 1:
                out.append(f'<polyline class="line" points="{" ".join(segment)}"/>')
            segment = []
        else:
            segment.append(f"{sx(x):.1f},{sy(y):.1f}")
    if len(shown) <= 400:
        for x, y, tip in shown:
            cx, cy = f"{sx(x):.1f}", f"{sy(y):.1f}"
            out.append(f'<g><circle class="hit" cx="{cx}" cy="{cy}" r="8"><title>{html.escape(tip)}</title></circle>'
                       f'<circle class="dot" cx="{cx}" cy="{cy}" r="4"/></g>')
    out.append("</svg>")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(out) + "\n")
