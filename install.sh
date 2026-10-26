#!/bin/sh
# install.sh — put smart-lib on this machine and start recording the clipboard.
#
#   ./install.sh                 # daemon + session unit + desktop entry + app
#   ./install.sh --no-app        # daemon only (no GPU/gpui toolchain needed)
#   ./install.sh --no-service    # binaries only (no systemd unit)
#   ./install.sh --prefix /usr/local
#   ./install.sh --uninstall
#
# WHAT THIS FIXES
#
# The tree could serve the contract, but a machine could not *run* it: the daemon
# lived in `examples/`, nothing started it, and the app's socket path was a guess.
# This script is the missing step between "it compiles" and "I copied something,
# and it is in my history".
#
# WHAT IT INSTALLS, AND WHERE
#
#   $PREFIX/bin/smart-service                     the daemon
#   $PREFIX/bin/smart-clipboard                   the app (unless --no-app)
#   ~/.config/systemd/user/smart-service.service  a user unit (unless --no-service)
#   ~/.local/share/applications/smart-clipboard.desktop + the icon
#
# It does not use sudo, does not write outside $PREFIX and $HOME, does not touch
# /etc, and enables the unit only when systemd is the running init (`--user`
# units work anywhere systemd does; the script degrades to a printed command
# everywhere else).
#
# EXIT CODES
#   0  installed and, where possible, started
#   1  a prerequisite is missing
#   2  a build failed
set -eu

PREFIX="${PREFIX:-$HOME/.local}"
SCHEMA_DIR="${SMART_SCHEMA_DIR:-}"
DO_APP=1
DO_SERVICE=1
ACTION=install
JOBS="${JOBS:-}"

# The features the daemon is built with. Clipboard is the product; database is
# where its history lives; rpc + std are the wire and the host.
# --no-default-features on purpose: the daemon serves the clipboard and the
# store its history lives in, and nothing else. The engine's defaults include
# domains whose native seams need libraries (a bus, an audio daemon) that a
# clipboard daemon has no business linking.
DAEMON_FEATURES="std,rpc,domain-clipboard,domain-database,domain-discovery"

say() { printf '%s\n' "$*"; }
step() { printf '\n==> %s\n' "$*"; }
die() { printf '\n!! %s\n' "$*" >&2; exit 1; }

usage() {
    sed -n '2,40p' "$0" | sed 's/^# \{0,1\}//'
}

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix) PREFIX="$2"; shift 2 ;;
        --prefix=*) PREFIX="${1#*=}"; shift ;;
        --schemas|--schema-dir) SCHEMA_DIR="$2"; shift 2 ;;
        --schemas=*|--schema-dir=*) SCHEMA_DIR="${1#*=}"; shift ;;
        --no-app) DO_APP=0; shift ;;
        --no-service) DO_SERVICE=0; shift ;;
        --uninstall|--remove) ACTION=uninstall; shift ;;
        -j|--jobs) JOBS="$2"; shift 2 ;;
        -j*|--jobs=*) JOBS="${1#*=}"; JOBS="${JOBS#-j}"; shift ;;
        -h|--help) usage; exit 0 ;;
        *) die "unknown argument: $1 (try --help)" ;;
    esac
done

REPO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
BIN_DIR="$PREFIX/bin"
UNIT_DIR="$HOME/.config/systemd/user"
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"

# ── uninstall ────────────────────────────────────────────────────────────────
if [ "$ACTION" = uninstall ]; then
    step "Stopping and disabling the session unit"
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user disable --now smart-service.service 2>/dev/null || true
    fi
    rm -f "$UNIT_DIR/smart-service.service"
    rm -f "$BIN_DIR/smart-service" "$BIN_DIR/smart-clipboard"
    rm -f "$APP_DIR/smart-clipboard.desktop" "$ICON_DIR/smart-clipboard.svg"
    if command -v systemctl >/dev/null 2>&1; then
        systemctl --user daemon-reload 2>/dev/null || true
    fi
    # The history is the user's data, so it is reported, never deleted.
    STATE="${XDG_STATE_HOME:-$HOME/.local/state}/smart"
    say "Binaries and unit removed. Your clipboard history is still in:"
    say "    $STATE/database/database.journal"
    say "Delete that directory yourself if you want it gone."
    exit 0
fi

# ── prerequisites ────────────────────────────────────────────────────────────
step "Checking prerequisites"
command -v cargo >/dev/null 2>&1 || die "cargo is not on PATH. Install Rust (https://rustup.rs) and re-run."
command -v capnp >/dev/null 2>&1 || die "the capnp compiler is not on PATH.
    Debian/Ubuntu:  sudo apt install capnproto libcapnp-dev
    Fedora:         sudo dnf install capnproto capnproto-devel
    Arch:           sudo pacman -S capnproto
    (the build reports the version it used; the project pins the 0.27 capnp line)"

if [ -z "$SCHEMA_DIR" ]; then
    for candidate in \
        "$REPO_DIR/../smart-protocol-official/schema" \
        "$HOME/smart/smart-protocol-official/schema" \
        "/home/lufuno/smart/smart-protocol-official/schema"
    do
        if [ -d "$candidate" ]; then SCHEMA_DIR="$candidate"; break; fi
    done
fi
if [ -z "$SCHEMA_DIR" ]; then
    say "The contract's schema tree was not found next to this checkout."
    say "Clone it beside the engine, or pass its path:"
    say ""
    say "    git clone https://github.com/smart-foundation/smart-protocol-official ../smart-protocol-official"
    say "    ./install.sh --schemas /path/to/smart-protocol-official/schema"
    exit 1
fi
[ -f "$SCHEMA_DIR/clipboard.capnp" ] || die "$SCHEMA_DIR does not look like the schema tree (no clipboard.capnp)."
say "schema tree:  $SCHEMA_DIR"
say "prefix:       $PREFIX"
if [ -n "$JOBS" ]; then say "jobs:         $JOBS"; fi

CARGO_JOBS=""
if [ -n "$JOBS" ]; then CARGO_JOBS="-j $JOBS"; fi

# ── the daemon ───────────────────────────────────────────────────────────────
step "Building the daemon (clipboard + database)"
(
    cd "$REPO_DIR"
    SMART_SCHEMA_DIR="$SCHEMA_DIR" cargo build --release --bin smart-service \
        --no-default-features --features "$DAEMON_FEATURES" $CARGO_JOBS
) || die "the daemon did not build. The compiler output above is the whole story; nothing was installed."
mkdir -p "$BIN_DIR"
install -m 0755 "$REPO_DIR/target/release/smart-service" "$BIN_DIR/smart-service"
say "installed $BIN_DIR/smart-service"

# ── the app ──────────────────────────────────────────────────────────────────
if [ "$DO_APP" = 1 ]; then
    step "Building the app (gpui)"
    if ! pkg-config --exists wayland-client xkbcommon 2>/dev/null; then
        say "The gpui app needs Wayland/X11 development headers:"
        say "    sudo apt install libwayland-dev libxkbcommon-dev libx11-xcb-dev \\"
        say "                     libvulkan-dev libfontconfig-dev libfreetype-dev libssl-dev"
    fi
    BUILT=1
    (
        cd "$REPO_DIR/crates/smart-clipboard"
        SMART_SCHEMA_DIR="$SCHEMA_DIR" cargo build --release $CARGO_JOBS
    ) || BUILT=0
    if [ "$BUILT" = 1 ] && [ -f "$REPO_DIR/crates/smart-clipboard/target/release/smart-clipboard" ]; then
        install -m 0755 \
            "$REPO_DIR/crates/smart-clipboard/target/release/smart-clipboard" \
            "$BIN_DIR/smart-clipboard"
        mkdir -p "$APP_DIR" "$ICON_DIR"
        sed "s|^Exec=smart-clipboard$|Exec=$BIN_DIR/smart-clipboard|; s|^Exec=smart-clipboard --palette$|Exec=$BIN_DIR/smart-clipboard --palette|" \
            "$REPO_DIR/packaging/smart-clipboard.desktop" > "$APP_DIR/smart-clipboard.desktop"
        install -m 0644 "$REPO_DIR/packaging/smart-clipboard.svg" "$ICON_DIR/smart-clipboard.svg"
        command -v update-desktop-database >/dev/null 2>&1 && \
            update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
        say "installed $BIN_DIR/smart-clipboard + desktop entry"
    else
        # The daemon is the part that records; the app can be installed later.
        # Do not fail the install over a GPU toolchain, but say exactly what is
        # missing and how to retry — never leave the user with a silent skip.
        say ""
        say "The app did not build, so it was NOT installed. The daemon is installed"
        say "and will start recording. Retry the app on its own with:"
        say "    cd $REPO_DIR/crates/smart-clipboard && cargo build --release"
        say "    ./install.sh --no-service"
    fi
else
    step "Skipping the app (--no-app)"
fi

# ── the session unit ─────────────────────────────────────────────────────────
if [ "$DO_SERVICE" = 1 ]; then
    step "Installing the session unit"
    mkdir -p "$UNIT_DIR"
    # Point the unit at the prefix that was actually used, rather than assuming
    # ~/.local: an operator with --prefix /usr/local must not get a unit that
    # silently runs a different binary.
    sed "s|%h/.local/bin/smart-service|$BIN_DIR/smart-service|" \
        "$REPO_DIR/packaging/systemd/smart-service.service" > "$UNIT_DIR/smart-service.service"
    say "wrote $UNIT_DIR/smart-service.service"
    if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
        systemctl --user daemon-reload
        if systemctl --user enable --now smart-service.service; then
            sleep 1
            if systemctl --user is-active --quiet smart-service.service; then
                say "smart-service is running for this session"
            else
                say "the unit was enabled but is not active yet — check:"
                say "    journalctl --user -u smart-service -n 40 --no-pager"
            fi
        else
            say "could not start the unit; start it by hand with:"
            say "    systemctl --user enable --now smart-service"
        fi
    else
        say "systemd --user is not available here. Start the daemon yourself with:"
        say "    $BIN_DIR/smart-service --socket \${XDG_RUNTIME_DIR:-/tmp}/smart/service.sock &"
    fi
else
    step "Skipping the session unit (--no-service)"
    say "start the daemon with: $BIN_DIR/smart-service"
fi

# ── the proof ────────────────────────────────────────────────────────────────
step "What to check, in order"
cat <<EOF
1. the daemon answered:
       systemctl --user status smart-service --no-pager | head -3
       journalctl --user -u smart-service -n 20 --no-pager
2. the socket exists (the app dials this by convention):
       ls -l "\${XDG_RUNTIME_DIR:-/tmp}/smart/service.sock"
3. copy something in any application, then:
       smart-clipboard            # it should be the first row
4. the history survives a restart:
       systemctl --user restart smart-service && smart-clipboard
EOF

if [ ! -e "${XDG_RUNTIME_DIR:-/tmp}/smart/service.sock" ]; then
    say ""
    say "note: the socket is not there yet. If the unit is not running, start the"
    say "daemon in the foreground once to see its startup report:"
    say "    $BIN_DIR/smart-service --socket \${XDG_RUNTIME_DIR:-/tmp}/smart/service.sock"
fi

exit 0
