#!/bin/sh
# One step from a fresh checkout to a playable game:
#
#   ./setup.sh ~/Downloads/Recoil.nrg      # or an .iso, a disc, an install
#   ./setup.sh                             # look for the game and ask
#
# Installs what cargo cannot (Rust itself, and on Linux the build packages
# and unshield, each after asking), builds the game, then runs
# `open-bft setup` with the arguments given (`open-bft setup --help`).
# Pass --yes to answer every question with yes.
set -eu

repo=$(cd "$(dirname "$0")" && pwd)
yes=0
for arg do
    case "$arg" in
        -y | --yes) yes=1 ;;
    esac
done

say() { printf '%s\n' "$*"; }
fail() { printf 'setup.sh: %s\n' "$*" >&2; exit 1; }

# Ask a yes/no question; Enter means yes.
ask() {
    [ "$yes" = 1 ] && return 0
    [ -t 0 ] || fail "$1 Run this in a terminal, or pass --yes."
    printf '%s [Y/n] ' "$1"
    read -r answer
    case "$answer" in
        "" | y | Y | yes | Yes) return 0 ;;
        *) return 1 ;;
    esac
}

# --- Rust ---------------------------------------------------------------
# The toolchain the project pins; rustup fetches it below.
toolchain=$(sed -n 's/^channel *= *"\([^"]*\)".*/\1/p' "$repo/rust-toolchain.toml")
[ -n "$toolchain" ] || fail "cannot read the toolchain from rust-toolchain.toml"

if ! command -v rustup >/dev/null 2>&1; then
    if [ -x "$HOME/.cargo/bin/rustup" ]; then
        PATH="$HOME/.cargo/bin:$PATH"
    elif ask "Rust is not installed. Install it with rustup (https://rustup.rs)?"; then
        command -v curl >/dev/null 2>&1 || fail "installing rustup needs curl"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
            sh -s -- -y --profile minimal --default-toolchain none
        PATH="$HOME/.cargo/bin:$PATH"
    else
        fail "the game is built with Rust; install it from https://rustup.rs and run this again"
    fi
fi
# Only when missing, and without updating rustup itself: a setup script has
# no business changing the user's Rust install beyond what it needs.
if ! rustup toolchain list | grep -q "^$toolchain-"; then
    say "Installing Rust $toolchain (pinned in rust-toolchain.toml)..."
    rustup toolchain install "$toolchain" --profile minimal --no-self-update
fi

# --- System packages ----------------------------------------------------
# Bevy needs a C toolchain and the ALSA and udev headers on Linux; unshield
# unpacks the disc's installer.
install_missing() {
    manager=$1; shift
    [ $# -eq 0 ] && return 0
    say "Missing packages: $*"
    if ask "Install them with sudo $manager?"; then
        case "$manager" in
            apt-get) sudo apt-get install -y "$@" ;;
            dnf) sudo dnf install -y "$@" ;;
            pacman) sudo pacman -S --needed --noconfirm "$@" ;;
        esac
    else
        say "Continuing without them; the build or the disc step may fail."
    fi
}

case "$(uname -s)" in
    Linux)
        missing=""
        if command -v apt-get >/dev/null 2>&1; then
            for pkg in build-essential pkg-config libasound2-dev libudev-dev unshield; do
                dpkg-query -W -f='${Status}' "$pkg" 2>/dev/null | grep -q "install ok installed" ||
                    missing="$missing $pkg"
            done
            # shellcheck disable=SC2086 # one word per package
            install_missing apt-get $missing
        elif command -v dnf >/dev/null 2>&1; then
            for pkg in gcc pkgconf-pkg-config alsa-lib-devel systemd-devel unshield; do
                rpm -q "$pkg" >/dev/null 2>&1 || missing="$missing $pkg"
            done
            # shellcheck disable=SC2086
            install_missing dnf $missing
        elif command -v pacman >/dev/null 2>&1; then
            for pkg in base-devel pkgconf alsa-lib systemd-libs unshield; do
                pacman -Q "$pkg" >/dev/null 2>&1 || missing="$missing $pkg"
            done
            # shellcheck disable=SC2086
            install_missing pacman $missing
        else
            say "Unknown package manager. Make sure a C compiler, pkg-config, the"
            say "ALSA and udev development headers, and unshield are installed."
        fi
        ;;
    Darwin)
        if ! xcode-select -p >/dev/null 2>&1; then
            xcode-select --install || true
            fail "install the Xcode Command Line Tools in the dialog, then run this again"
        fi
        if ! command -v unshield >/dev/null 2>&1; then
            if command -v brew >/dev/null 2>&1 && ask "Install unshield (for the disc or an image) with Homebrew?"; then
                brew install unshield
            else
                say "Without unshield, setup works from an installed copy only."
            fi
        fi
        ;;
esac

# --- Build and set up ----------------------------------------------------
# Built in the repository, where the pinned toolchain applies, with the
# same profile `cargo run` uses, so playing afterwards rebuilds nothing.
say "Building the game (the first build takes several minutes)..."
(cd "$repo" && cargo build --locked --bin open-bft)

# Run from here, so relative paths and setup's search of the current folder
# mean what the user meant. RUSTUP_TOOLCHAIN makes the `unzbd` build that
# setup may start use the pinned toolchain too.
RUSTUP_TOOLCHAIN=$toolchain "$repo/target/debug/open-bft" setup "$@"

say ""
say "To play: cd $repo && cargo run"
