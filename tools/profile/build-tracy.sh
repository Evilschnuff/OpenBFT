#!/usr/bin/env bash
# Build Tracy's headless tools (tracy-capture, tracy-csvexport) at the
# version the game's Tracy client speaks, into target/tracy/<version>/bin.
#
# The version is read from the tracy-client-sys crate Cargo.lock pins, so
# run `cargo fetch` (or any build with `--features profile`) first.
# Needs curl, cmake, ninja and a C++20 compiler; the CMake step downloads
# Tracy's own dependencies (capstone, zstd and so on) from the network.
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo"

sys=$(awk '/^name = "tracy-client-sys"$/ { getline; gsub(/[^0-9.]/, ""); print; exit }' Cargo.lock)
if [[ -z "$sys" ]]; then
    echo "tracy-client-sys is not in Cargo.lock; build once with --features profile" >&2
    exit 1
fi
header=$(ls "${CARGO_HOME:-$HOME/.cargo}"/registry/src/*/tracy-client-sys-"$sys"/tracy/common/TracyVersion.hpp 2>/dev/null | head -1)
if [[ -z "$header" ]]; then
    echo "tracy-client-sys $sys is not downloaded yet; run cargo fetch" >&2
    exit 1
fi
part() { sed -n "s/.*$1 = \([0-9]*\);.*/\1/p" "$header"; }
version="$(part Major).$(part Minor).$(part Patch)"

out="$repo/target/tracy/$version"
src="$out/src/tracy-$version"
mkdir -p "$out/src" "$out/bin"
if [[ ! -d "$src" ]]; then
    curl -sSfL "https://github.com/wolfpld/tracy/archive/refs/tags/v$version.tar.gz" -o "$out/src/tracy.tar.gz"
    tar -xzf "$out/src/tracy.tar.gz" -C "$out/src"
fi

for tool in capture csvexport; do
    cmake -S "$src/$tool" -B "$out/build-$tool" -G Ninja -DCMAKE_BUILD_TYPE=Release -DNO_ISA_EXTENSIONS=ON
    nice -n 19 cmake --build "$out/build-$tool" -j 8
    cp "$out/build-$tool/tracy-$tool" "$out/bin/"
done

echo "Tracy $version tools in $out/bin (matches tracy-client-sys $sys)"
