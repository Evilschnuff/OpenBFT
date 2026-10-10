#!/usr/bin/env python3
"""Plot a level's model vertices against its zmap to fix the coordinate system.

Walks the node tree of an extracted `gamez.zip`, composes each node's
transform with its parents', transforms that node's model vertices into world
space, and draws:

  * a top-down scatter of all vertices, with the zmap features overlaid, and
  * a side view, to identify the up axis.

`--permute` tries every assignment of the three world components to the plot's
(horizontal, vertical, depth) axes and reports which one puts the zmap on top
of the geometry, measured by how much of the zmap's extent is covered by
vertices. That is the evidence for docs/data/coordinates.md.

Plots are derived game data: they are written outside the repository.

Usage:
    tools/recon/plot_level.py --level m1
    tools/recon/plot_level.py --level m1 --permute
"""

from __future__ import annotations

import argparse
import json
import os
import pathlib
import zipfile

import numpy as np

# Axis labels in the order the JSON stores them.
AXES = ("x", "y", "z")


def read_json(zf: zipfile.ZipFile, name: str):
    with zf.open(name) as fh:
        return json.load(fh)


def vec(d) -> np.ndarray:
    return np.array([d["x"], d["y"], d["z"]], dtype=np.float64)


def node_matrix(node) -> np.ndarray:
    """The node's local 4x4 transform.

    An Object3d's `data.Object3d.transform` is one of:
      * "Initial"                 — identity
      * {"Matrix": {r00..r32}}    — row-major 3x3 rotation plus r30..r32
                                    translation (a 3x4 affine transform)
      * {"RotateTranslateScale":} — Euler rotation, translation, scale
    """
    m = np.eye(4)
    data = node.get("data")
    if not isinstance(data, dict) or "Object3d" not in data:
        return m
    transform = data["Object3d"].get("transform")
    if transform == "Initial" or transform is None:
        return m
    if isinstance(transform, dict) and "Matrix" in transform:
        r = transform["Matrix"]
        # Direct3D row-vector storage: row i is where local axis i lands,
        # so it is column i of the column-vector matrix (docs/data/nodes.md).
        m[:3, :3] = np.array(
            [
                [r["r00"], r["r01"], r["r02"]],
                [r["r10"], r["r11"], r["r12"]],
                [r["r20"], r["r21"], r["r22"]],
            ]
        ).T
        m[:3, 3] = [r["r30"], r["r31"], r["r32"]]
        return m
    if isinstance(transform, dict) and "RotateTranslateScale" in transform:
        rts = transform["RotateTranslateScale"]
        rot = vec(rts["rotate"])
        translate = vec(rts["translate"])
        scale = vec(rts.get("scale") or {"x": 1.0, "y": 1.0, "z": 1.0})
        # Euler order assumed XYZ (applied X, then Y, then Z), the order the
        # fields are stored in. Only two nodes per level use this form, so the
        # choice barely affects the plot; it is recorded in docs/data/coordinates.md.
        cx, cy, cz = np.cos(rot)
        sx, sy, sz = np.sin(rot)
        rx = np.array([[1, 0, 0], [0, cx, -sx], [0, sx, cx]])
        ry = np.array([[cy, 0, sy], [0, 1, 0], [-sy, 0, cy]])
        rz = np.array([[cz, -sz, 0], [sz, cz, 0], [0, 0, 1]])
        m[:3, :3] = (rz @ ry @ rx) * scale
        m[:3, 3] = translate
        return m
    return m


def world_vertices(gamez_zip: pathlib.Path, max_points: int | None = None):
    """Return (N,3) world-space vertices and a dict of named landmark points."""
    with zipfile.ZipFile(gamez_zip) as zf:
        nodes = read_json(zf, "nodes.json")
        models = read_json(zf, "models.json")

    # Compose transforms down the tree. Roots are nodes with no parent.
    child_of: dict[int, int] = {}
    for i, node in enumerate(nodes):
        for c in node.get("child_indices") or []:
            child_of[c] = i

    cache: dict[int, np.ndarray] = {}

    def world_matrix(i: int) -> np.ndarray:
        if i in cache:
            return cache[i]
        local = node_matrix(nodes[i])
        parent = child_of.get(i)
        # Guard against a cycle by writing the local transform first.
        cache[i] = local
        if parent is not None and parent != i:
            cache[i] = world_matrix(parent) @ local
        return cache[i]

    chunks = []
    named: dict[str, np.ndarray] = {}
    for i, node in enumerate(nodes):
        model_index = node.get("model_index", -1)
        if model_index is None or model_index < 0 or model_index >= len(models):
            continue
        model = models[model_index]
        if not model:
            continue
        verts = model.get("vertices") or []
        if not verts:
            continue
        m = world_matrix(i)
        local = np.array([[v["x"], v["y"], v["z"]] for v in verts], dtype=np.float64)
        world = local @ m[:3, :3].T + m[:3, 3]
        chunks.append(world)
        name = node.get("name") or ""
        if name and name not in named:
            named[name] = world.mean(axis=0)
    points = np.vstack(chunks) if chunks else np.zeros((0, 3))
    if max_points and len(points) > max_points:
        idx = np.random.default_rng(0).choice(len(points), max_points, replace=False)
        points = points[idx]
    return points, named


def zmap_points(path: pathlib.Path):
    data = json.loads(path.read_text())
    feats = []
    for feature in data.get("features") or []:
        verts = feature.get("vertices") or []
        if verts:
            feats.append(np.array([[v["x"], v["y"], v["z"]] for v in verts]))
    return data, feats


def coverage(points: np.ndarray, zpts: np.ndarray, h: int, v: int) -> float:
    """Score how well the zmap agrees with the geometry in the (h, v) plane.

    The zmap is drawn in the same world units as the geometry, so a correct
    axis assignment should make the two extents match. The score multiplies
    three agreement terms, each in [0, 1]:

      * `inside`  — fraction of zmap vertices inside the geometry's bounds,
      * `occupied` — fraction of those that land on a cell the geometry
        actually occupies (32x32 grid; rejects a plane that merely brackets
        the map), and
      * `extent`  — how closely the zmap's own min/max span matches the
        geometry's span on both axes (rejects a plane of the wrong scale,
        which is what distinguishes the ground plane from a vertical one).
    """
    if len(points) == 0 or len(zpts) == 0:
        return 0.0
    ph, pv = points[:, h], points[:, v]
    lo = np.array([ph.min(), pv.min()])
    hi = np.array([ph.max(), pv.max()])
    span = np.maximum(hi - lo, 1e-6)
    grid = np.zeros((32, 32), dtype=bool)
    gi = np.clip(((np.stack([ph, pv], 1) - lo) / span * 31).astype(int), 0, 31)
    grid[gi[:, 0], gi[:, 1]] = True
    zhv = np.stack([zpts[:, h], zpts[:, v]], 1)
    zi = ((zhv - lo) / span * 31).astype(int)
    inside = (zi >= 0).all(1) & (zi <= 31).all(1)
    if not inside.any():
        return 0.0
    occupied = float(grid[zi[inside][:, 0], zi[inside][:, 1]].mean())
    zspan = np.maximum(zhv.max(0) - zhv.min(0), 1e-6)
    extent = float(np.prod(np.minimum(zspan, span) / np.maximum(zspan, span)))
    return float(inside.mean()) * occupied * extent


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    default_extract = os.environ.get("BFT_EXTRACT_DIR")
    parser.add_argument("--extract", default=default_extract, type=pathlib.Path,
                        required=default_extract is None,
                        help="extraction directory (default $BFT_EXTRACT_DIR)")
    parser.add_argument("--level", default="m1")
    parser.add_argument("--out", default=None, type=pathlib.Path,
                        help="output directory (default <extract>/plots)")
    parser.add_argument("--permute", action="store_true",
                        help="score every axis assignment against the zmap")
    parser.add_argument("--max-points", type=int, default=200000)
    args = parser.parse_args()

    gamez = args.extract / args.level / "gamez.zip"
    zmap = args.extract / "maps" / f"{args.level}.json"
    if not gamez.exists():
        raise SystemExit(f"missing {gamez}")
    out_dir = args.out or (args.extract / "plots")
    out_dir.mkdir(parents=True, exist_ok=True)

    points, named = world_vertices(gamez, args.max_points)
    print(f"{args.level}: {len(points)} world-space vertices")
    for axis, label in enumerate(AXES):
        col = points[:, axis]
        print(f"  {label}: min={col.min():10.1f} max={col.max():10.1f} "
              f"mean={col.mean():9.1f}")

    zdata, feats = ({}, [])
    if zmap.exists():
        zdata, feats = zmap_points(zmap)
        print(f"  zmap min={zdata.get('min')} max={zdata.get('max')}")
    zpts = np.vstack(feats) if feats else np.zeros((0, 3))

    if args.permute and len(zpts):
        print("\naxis assignment scores (higher = zmap lands on geometry):")
        results = []
        for h in range(3):
            for v in range(3):
                if h == v:
                    continue
                score = coverage(points, zpts, h, v)
                results.append((score, h, v))
        for score, h, v in sorted(results, reverse=True):
            depth = [a for a in range(3) if a not in (h, v)][0]
            print(f"  horizontal={AXES[h]} vertical={AXES[v]} "
                  f"(remaining={AXES[depth]}): {score:.3f}")
        best = max(results)
        print(f"  -> best: horizontal={AXES[best[1]]}, vertical={AXES[best[2]]}")

    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt

    # Top-down: the zmap stores y == 0 for every vertex, so x/z is the ground
    # plane and y is the remaining (up) axis.
    fig, axs = plt.subplots(1, 2, figsize=(20, 10))
    axs[0].scatter(points[:, 0], points[:, 2], s=0.05, c="#888", linewidths=0)
    for feature in feats:
        axs[0].plot(feature[:, 0], feature[:, 2], "-", lw=1.5, c="#d62728")
    if zdata:
        mn, mx = zdata.get("min"), zdata.get("max")
        if mn and mx:
            axs[0].plot(
                [mn["x"], mx["x"], mx["x"], mn["x"], mn["x"]],
                [mn["z"], mn["z"], mx["z"], mx["z"], mn["z"]],
                "--", lw=1, c="#1f77b4",
            )
    axs[0].set_title(f"{args.level} top-down: x (horizontal) vs z (vertical), zmap in red")
    axs[0].set_xlabel("x")
    axs[0].set_ylabel("z")
    axs[0].set_aspect("equal")

    axs[1].scatter(points[:, 0], points[:, 1], s=0.05, c="#888", linewidths=0)
    axs[1].set_title(f"{args.level} side view: x vs y (candidate up axis)")
    axs[1].set_xlabel("x")
    axs[1].set_ylabel("y")

    path = out_dir / f"{args.level}-coords.png"
    fig.tight_layout()
    fig.savefig(path, dpi=110)
    print(f"\nwrote {path}")

    # Known-vertical and known-flat objects fix the up axis: a tree should be
    # tall on exactly one axis, a dock or pad flat on that same axis.
    print("\nlocal-space extents of reference objects (dx, dy, dz):")
    with zipfile.ZipFile(gamez) as zf:
        nodes = read_json(zf, "nodes.json")
        models = read_json(zf, "models.json")
    for prefix in ("palm", "tree", "tower", "crane", "antenna", "dock", "pad"):
        for node in nodes:
            name = (node.get("name") or "").lower()
            index = node.get("model_index", -1)
            if not name.startswith(prefix) or index is None or index < 0:
                continue
            model = models[index] if index < len(models) else None
            if not model or not model.get("vertices"):
                continue
            verts = np.array([[v["x"], v["y"], v["z"]] for v in model["vertices"]])
            extent = verts.max(0) - verts.min(0)
            print(f"  {node['name']:<12} dx={extent[0]:7.1f} dy={extent[1]:7.1f} "
                  f"dz={extent[2]:7.1f}")
            break


if __name__ == "__main__":
    main()
