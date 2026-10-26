#!/bin/sh
# ci-annotate.sh — run a command, and if it fails, re-emit its compiler errors as
# GitHub annotations.
#
# Why: on GitHub-hosted runners the raw step log lives behind a blob URL that a
# restricted network cannot read, but **annotations** are part of the API. So a
# failing build's errors become `::error::` lines — whole error blocks, flattened
# — which is how a reviewer (or an agent) sees `error[E0599]: no method named …`
# and the types it expected without the log.
#
#   ./scripts/ci-annotate.sh cargo build --bin smart-service --features …
set -u

LOG=$(mktemp)
"$@" >"$LOG" 2>&1
STATUS=$?
cat "$LOG"

if [ "$STATUS" -ne 0 ]; then
    printf '::error::the command failed: %s\n' "$*"
    awk '
        /error(\[[A-Z0-9]+\])?:/ {
            if (inblock) { print buf }
            inblock = 1; buf = $0; next
        }
        inblock {
            if ($0 ~ /^[[:space:]]*$/) { print buf; inblock = 0; buf = "" }
            else { buf = buf " | " $0 }
        }
        END { if (inblock) print buf }
    ' "$LOG" | head -25 | while IFS= read -r line; do
        printf '::error::%s\n' "$(printf '%s' "$line" | tr -d '\r' | sed 's/%/%25/g' | cut -c1-900)"
    done
    # The last lines name the failing crate and target.
    tail -6 "$LOG" | while IFS= read -r line; do
        printf '::error::%s\n' "$(printf '%s' "$line" | tr -d '\r' | sed 's/%/%25/g' | cut -c1-400)"
    done
fi
rm -f "$LOG"
exit "$STATUS"
