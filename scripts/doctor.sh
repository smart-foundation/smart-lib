#!/bin/sh
# doctor.sh — "why is my clipboard history empty?"
#
# Every check prints one of:
#   ok     the piece is in place
#   no     the piece is missing, followed by the exact command that fixes it
#   ?      cannot be determined from here (and what would determine it)
#
# It never changes host state. Run it after install.sh, or when a copy does not
# appear in smart-clipboard.
set -eu

ok()   { printf '  \033[32mok\033[0m   %s\n' "$*"; }
no()   { printf '  \033[31mno\033[0m   %s\n' "$*"; }
dunno(){ printf '  \033[33m?\033[0m    %s\n' "$*"; }
head_() { printf '\n\033[1m%s\033[0m\n' "$*"; }
FIXES=""

fix() { FIXES="$FIXES
  - $*"; }

head_ "1. the tools"
if command -v capnp >/dev/null 2>&1; then
    ok "capnp $(capnp --version | cut -d' ' -f3)"
else
    no "capnp is not installed (needed to build, not to run)"
    fix "sudo apt install capnproto libcapnp-dev"
fi
if command -v smart-service >/dev/null 2>&1; then
    ok "smart-service on PATH: $(command -v smart-service)"
    BIN=$(command -v smart-service)
elif [ -x "$HOME/.local/bin/smart-service" ]; then
    ok "smart-service at ~/.local/bin/smart-service (add ~/.local/bin to PATH)"
    BIN="$HOME/.local/bin/smart-service"
else
    no "smart-service is not installed"
    fix "cd $(dirname "$0")/.. && ./install.sh --no-app"
    BIN=""
fi
if command -v smart-clipboard >/dev/null 2>&1; then
    ok "smart-clipboard on PATH: $(command -v smart-clipboard)"
elif [ -x "$HOME/.local/bin/smart-clipboard" ]; then
    ok "smart-clipboard at ~/.local/bin/smart-clipboard"
else
    no "smart-clipboard is not installed (the daemon can still record)"
    fix "cd $(dirname "$0")/../crates/smart-clipboard && cargo build --release && cd ../.. && ./install.sh --no-service"
fi

head_ "2. the seat (can anything be captured at all?)"
if [ -n "${WAYLAND_DISPLAY:-}" ]; then
    ok "WAYLAND_DISPLAY=$WAYLAND_DISPLAY (the backend speaks zwlr_data_control)"
elif [ -n "${DISPLAY:-}" ]; then
    ok "DISPLAY=$DISPLAY (the backend speaks the X11 selection protocol)"
else
    dunno "no WAYLAND_DISPLAY and no DISPLAY in this shell"
    fix "run the daemon from your graphical session (the unit does), not from a bare tty"
fi

head_ "3. the daemon"
if command -v systemctl >/dev/null 2>&1 && systemctl --user show-environment >/dev/null 2>&1; then
    STATE=$(systemctl --user is-active smart-service.service 2>/dev/null || true)
    case "$STATE" in
        active) ok "smart-service.service is active" ;;
        *)      no "smart-service.service is ${STATE:-not installed}"
                fix "systemctl --user enable --now smart-service" ;;
    esac
    printf '       last lines:\n'
    journalctl --user -u smart-service -n 5 --no-pager 2>/dev/null | sed 's/^/       /' || true
else
    dunno "systemd --user is unavailable; the daemon must be started by hand"
    fix "$BIN --socket \${XDG_RUNTIME_DIR:-/tmp}/smart/service.sock"
fi

head_ "4. the socket (how a client finds it)"
SOCKET="${XDG_RUNTIME_DIR:-/tmp}/smart/service.sock"
if [ -S "$SOCKET" ]; then
    ok "listening: $SOCKET"
elif [ -S /tmp/smart-service.sock ]; then
    dunno "not at $SOCKET, but a socket is at /tmp/smart-service.sock"
    fix "the app dials both; if you started the daemon by hand with --socket, point the app the same way"
else
    no "no socket at $SOCKET"
    fix "start the daemon and read its startup report: $BIN"
fi

head_ "5. the history store (does it survive a restart?)"
JOURNAL="${XDG_STATE_HOME:-$HOME/.local/state}/smart/database/database.journal"
if [ -f "$JOURNAL" ]; then
    SIZE=$(wc -c < "$JOURNAL" | tr -d ' ')
    if [ "$SIZE" -eq 0 ]; then
        dunno "$JOURNAL exists but is empty — no copy has been archived yet"
    else
        ok "$JOURNAL ($SIZE bytes) — start the daemon, copy something, watch it grow"
    fi
else
    dunno "no journal yet at $JOURNAL"
    fix "the store is created by the daemon on first start (not with --no-history)"
fi

head_ "6. the app's view"
printf '  The app reads the service, never the clipboard. If the daemon is active,\n'
printf '  copy something and open the app; the newest row is the copy.\n'
printf '  Headless check (needs a client): smart clipboard getHistory\n'

if [ -n "$FIXES" ]; then
    head_ "What to do, in order"
    printf '%s\n' "$FIXES"
fi
