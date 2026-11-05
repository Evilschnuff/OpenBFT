#!/usr/bin/env bash
# Record a running `--features profile` build without the Tracy GUI.
#
#   tools/profile/capture.sh [seconds] [label] [address]
#
# Writes target/profile/<date>-<time>-<label>.tracy. Start the game first;
# the capture connects, records for <seconds> (default 30) and stops.
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
seconds=${1:-30}
label=${2:-capture}
address=${3:-127.0.0.1}

capture=$(ls "$repo"/target/tracy/*/bin/tracy-capture 2>/dev/null | sort -V | tail -1)
if [[ -z "$capture" ]]; then
    echo "no tracy-capture; run tools/profile/build-tracy.sh first" >&2
    exit 1
fi

mkdir -p "$repo/target/profile"
file="$repo/target/profile/$(date +%Y%m%d-%H%M%S)-$label.tracy"
"$capture" -a "$address" -o "$file" -s "$seconds" -f
echo "$file"
