#!/bin/bash
# Extraction driver for Recoil data via mech3ax's unzbd.
#
# Runs every `unzbd rc` command that applies to a Recoil install and logs
# each one's exit status and stderr to $DST/extract.log, one record per
# command, so that docs/data/install.md and docs/data/quirks.md can quote them verbatim.
#
# Output is derived game data: keep $DST outside the repository.
#
#   MECH3AX=/path/to/mech3ax BFT_DATA_DIR=/path/to/recoil \
#   BFT_EXTRACT_DIR=/path/to/extract tools/recon/extract.sh
set -u
for var in MECH3AX BFT_DATA_DIR BFT_EXTRACT_DIR; do
  if [ -z "${!var:-}" ]; then
    echo "set $var (see tools/recon/README.md)" >&2
    exit 1
  fi
done
UNZBD="$MECH3AX/target/release/unzbd"
SRC="$BFT_DATA_DIR"
DST="$BFT_EXTRACT_DIR"
if [ ! -x "$UNZBD" ]; then
  echo "unzbd not found at $UNZBD (set MECH3AX to the mech3ax checkout)" >&2
  exit 1
fi
LOG="$DST/extract.log"
mkdir -p "$DST"
: > "$LOG"

run() {  # run <label> <outpath> <args...>
  local label="$1"; shift
  local out="$1"; shift
  mkdir -p "$(dirname "$out")"
  local err
  err=$("$UNZBD" rc "$@" "$out" 2>&1 >/dev/null)
  local rc=$?
  {
    echo "### $label"
    echo "CMD: unzbd rc $* $out"
    echo "EXIT: $rc"
    if [ -n "$err" ]; then echo "STDERR:"; echo "$err" | sed 's/^/  /'; fi
    echo
  } >> "$LOG"
  if [ $rc -ne 0 ]; then echo "FAIL($rc) $label"; else echo "ok   $label"; fi
}

# --- global files ---
run "interp"   "$DST/interp.json"       interp   "$SRC/zbd/interp.zbd"
run "messages" "$DST/messages.json"     messages "$SRC/messages.dll"
run "reader-global" "$DST/zrdr.zip"     reader   "$SRC/zbd/zrdr.zbd"
run "image"    "$DST/image.zip"         textures "$SRC/zbd/image.zbd"
for f in l m h; do
  run "sounds-$f" "$DST/sounds$f.zip"   sounds   "$SRC/zbd/sounds$f.zbd"
done

# --- zmaps ---
for n in $(seq 1 13); do
  src="$SRC/maps/m$n.zmap"
  [ -f "$src" ] && run "zmap-m$n" "$DST/maps/m$n.json" zmap "$src"
done

# --- per-mission ---
for n in $(seq 1 13); do
  d="$SRC/zbd/m$n"
  [ -d "$d" ] || continue
  run "gamez-m$n"  "$DST/m$n/gamez.zip"  gamez  "$d/gamez.zbd"
  run "anim-m$n"   "$DST/m$n/anim.zip"   anim   "$d/anim.zbd"
  run "reader-m$n" "$DST/m$n/zrdr.zip"   reader "$d/zrdr.zbd"
  for t in rtexture2 rtexture4 texture2 texture4 texture6 texture8; do
    [ -f "$d/$t.zbd" ] && run "tex-m$n-$t" "$DST/m$n/$t.zip" textures "$d/$t.zbd"
  done
done
echo "--- done; log at $LOG"
