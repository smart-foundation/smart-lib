#!/bin/sh
# ci-annotate.sh — run a command, and if it fails, re-emit its compiler errors as
# GitHub annotations.
#
# Why: on GitHub-hosted runners the raw step log lives behind a blob URL that a
# restricted network cannot read, but **annotations** are part of the API. So a
# failing build's errors are turned into `::error::` lines, which is how a
# reviewer (or an agent) sees `error[E0599]: no method named …` without the log.
#
#   ./scripts/ci-annotate.sh cargo build --bin smart-service --features …
set -u

LOG=$(mktemp)
"$@" >"$LOG" 2>&1
STATUS=$?
cat "$LOG"

if [ "$STATUS" -ne 0 ]; then
    printf '::error::the command failed: %s\n' "$*"
    # Error headline, then the location line rustc puts under it.
    grep -E '^(error(\[[A-Z0-9]+\])?:|  --> | *expected | *found )' "$LOG" \
        | head -80 \
        | while IFS= read -r line; do
            printf '::error file=build.rs::%s\n' "$(printf '%s' "$line" | tr -d '\r' | sed 's/%/%25/g')"
        done
    # The last line usually names the failing crate and target.
    tail -5 "$LOG" | while IFS= read -r line; do
        printf '::error::%s\n' "$(printf '%s' "$line" | tr -d '\r' | sed 's/%/%25/g')"
    done
fi
rm -f "$LOG"
exit "$STATUS"
