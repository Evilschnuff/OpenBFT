#!/usr/bin/env python3
"""Render a reader (.zrd.json) entry as a compact key/value tree.

Reader files are nested lists in which a string is a key and the list that
follows it is the value.  This tool prints that structure one key per line
so a schema can be read without paging through raw JSON.

usage: zrd.py <zip> <entry> [--path KEY[/KEY...]] [--depth N] [--max-lines N]
       zrd.py <zip> <entry> --keys            # only the top-level key list
       zrd.py <zip> <entry> --schema          # key paths with value shapes, deduplicated

Relative zip paths are resolved against the extraction directory,
$BFT_EXTRACT_DIR.
"""
import argparse
import json
import os
import sys
import zipfile
from collections import OrderedDict

EXTRACT = os.environ.get("BFT_EXTRACT_DIR", ".")


def load(zip_name, entry):
    path = zip_name if os.path.isabs(zip_name) else os.path.join(EXTRACT, zip_name)
    if not entry.endswith(".json"):
        entry += ".zrd.json" if not entry.endswith(".zrd") else ".json"
    data = json.loads(zipfile.ZipFile(path).read(entry))
    # every file is wrapped in a single-element outer list
    while isinstance(data, list) and len(data) == 1 and isinstance(data[0], list):
        data = data[0]
    return data


def VALUE_OK(v):
    # a key's value is a list, or null when the key carries no value
    return isinstance(v, list) or v is None


def is_kv(lst):
    """A list is key/value shaped if it alternates str, (list|null)."""
    if not isinstance(lst, list) or len(lst) < 2:
        return False
    return all(isinstance(lst[i], str) and VALUE_OK(lst[i + 1]) for i in range(0, len(lst) - 1, 2)) and len(lst) % 2 == 0


def pairs(lst):
    return [(lst[i], lst[i + 1] if lst[i + 1] is not None else []) for i in range(0, len(lst), 2)]


def fmt_scalar(v):
    if isinstance(v, float):
        return f"{v:.6g}"
    return json.dumps(v)


def render(node, indent, out, depth, max_depth):
    if depth > max_depth:
        out.append("  " * indent + "…")
        return
    if is_kv(node):
        for k, v in pairs(node):
            if not v:
                out.append("  " * indent + f"{k}: []")
            elif all(not isinstance(x, list) for x in v):
                out.append("  " * indent + f"{k}: [{', '.join(fmt_scalar(x) for x in v)}]")
            else:
                out.append("  " * indent + f"{k}:")
                render(v, indent + 1, out, depth + 1, max_depth)
    elif isinstance(node, list):
        # mixed list: scalars first, then nested lists
        scal = [x for x in node if not isinstance(x, list)]
        if scal:
            out.append("  " * indent + "[" + ", ".join(fmt_scalar(x) for x in scal) + "]")
        for x in node:
            if isinstance(x, list):
                if is_kv(x):
                    out.append("  " * indent + "-")
                    render(x, indent + 1, out, depth + 1, max_depth)
                elif all(not isinstance(y, list) for y in x):
                    out.append("  " * indent + "- [" + ", ".join(fmt_scalar(y) for y in x) + "]")
                else:
                    out.append("  " * indent + "-")
                    render(x, indent + 1, out, depth + 1, max_depth)
    else:
        out.append("  " * indent + fmt_scalar(node))


def shape(v):
    if not v:
        return "[]"
    if all(not isinstance(x, list) for x in v):
        ts = sorted({type(x).__name__ for x in v})
        return f"[{len(v)}x{'/'.join(ts)}]"
    if is_kv(v):
        return "kv"
    return f"list({len(v)})"


def schema(node, prefix, acc):
    if is_kv(node):
        for k, v in pairs(node):
            p = prefix + "/" + k
            acc.setdefault(p, OrderedDict())
            acc[p][shape(v)] = acc[p].get(shape(v), 0) + 1
            if is_kv(v) or (isinstance(v, list) and any(isinstance(x, list) for x in v)):
                schema(v, p, acc)
    elif isinstance(node, list):
        for x in node:
            if isinstance(x, list):
                schema(x, prefix + "/*", acc)


def walk_path(node, path):
    for key in path.split("/"):
        if not key:
            continue
        if key.isdigit() and isinstance(node, list):
            node = node[int(key)]
            continue
        if not is_kv(node):
            raise SystemExit(f"cannot descend into '{key}': not key/value shaped")
        found = [v for k, v in pairs(node) if k == key]
        if not found:
            raise SystemExit(f"key '{key}' not found; have {[k for k, _ in pairs(node)]}")
        node = found[0]
    return node


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("zip")
    ap.add_argument("entry")
    ap.add_argument("--path", default="")
    ap.add_argument("--depth", type=int, default=12)
    ap.add_argument("--max-lines", type=int, default=200)
    ap.add_argument("--keys", action="store_true")
    ap.add_argument("--schema", action="store_true")
    a = ap.parse_args()
    node = walk_path(load(a.zip, a.entry), a.path)
    if a.keys:
        if is_kv(node):
            for k, v in pairs(node):
                print(f"{k}: {shape(v)}")
        else:
            print("not key/value shaped:", shape(node))
        return
    if a.schema:
        acc = OrderedDict()
        schema(node, "", acc)
        for p, shapes in acc.items():
            print(f"{p}  {dict(shapes)}")
        return
    out = []
    render(node, 0, out, 0, a.depth)
    for line in out[: a.max_lines]:
        print(line)
    if len(out) > a.max_lines:
        print(f"... ({len(out)} lines total)")


if __name__ == "__main__":
    main()
