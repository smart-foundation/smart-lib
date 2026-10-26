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
CLEAN=$(mktemp)
"$@" >"$LOG" 2>&1
STATUS=$?
cat "$LOG"
# Strip ANSI colour: `CARGO_TERM_COLOR=always` is useful in the log and fatal to
# any pattern that expects `error[E0277]:` to start a line.
sed 's/\x1b\[[0-9;]*m//g' "$LOG" > "$CLEAN"

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
    ' "$CLEAN" | head -25 | while IFS= read -r line; do
        printf '::error::%s\n' "$(printf '%s' "$line" | tr -d '\r' | sed 's/%/%25/g' | cut -c1-900)"
    done
    # Test failures: the assertion, and the left/right pair that explains it.
    #
    # A panic is two lines — `thread 'x' panicked at src/f.rs:12:5:` and then the
    # message, which is the only line carrying *why* (a `Display`ed error, an
    # `expect`'s text). So the message line is captured with it; grep alone
    # would drop it.
    awk '
        /^(failures:|    [a-z_][a-z_0-9]*::|  left:| right:|assertion)/ { print "::error::" $0; next }
        /thread .* panicked at/ { print "::error::" $0; want = 1; next }
        want { print "::error::" $0; want = 0 }
    ' "$CLEAN" | head -24 \
        | while IFS= read -r line; do
            printf '%s\n' "$(printf '%s' "$line" | tr -d '\r' | sed 's/%/%25/g' | cut -c1-700)"
        done
    # The last lines name the failing crate and target.
    tail -6 "$CLEAN" | while IFS= read -r line; do
        printf '::error::%s\n' "$(printf '%s' "$line" | tr -d '\r' | sed 's/%/%25/g' | cut -c1-400)"
    done
fi
rm -f "$LOG" "$CLEAN"
exit "$STATUS"
