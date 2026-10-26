#!/bin/sh
# ci-apply-fmt.sh — apply the formatting patch a CI run published.
#
#   ./scripts/ci-apply-fmt.sh [run-id]
#
# The hygiene job runs `cargo fmt` on the two product crates, then emits what it
# would write as gzip/base64 chunks in GitHub annotations (`FMTCHUNK 001 <b64>`),
# because neither the step log nor an artifact is reachable from a restricted
# network — and because GitHub caps a step at ten annotations, the patch is
# compressed and chunked. This script reads them back and applies the diff.
#
# It reads, it never pushes: the patch lands in the working tree and the next
# commit carries it.
set -eu

RUN="${1:-}"
REPO="${REPO:-smart-foundation/smart-lib}"

if [ -z "$RUN" ]; then
    RUN=$(gh run list --repo "$REPO" --limit 20 \
        --json databaseId,status -q '.[] | select(.status == "completed") | .databaseId' \
        | head -1)
fi
[ -n "$RUN" ] || { printf 'no completed run found; pass a run id\n' >&2; exit 1; }

JOB=$(gh api "repos/$REPO/actions/runs/$RUN/jobs" \
    -q '.jobs[] | select(.name | startswith("hygiene")) | .id')
[ -n "$JOB" ] || { printf 'run %s has no hygiene job\n' "$RUN" >&2; exit 1; }

PATCH=$(mktemp)
# Annotations come back newest-first, and a patch is worthless out of order, so
# the chunk number is the sort key.
gh api --paginate "repos/$REPO/check-runs/$JOB/annotations" \
    -q '.[] | .message' \
    | sed -n 's/^FMTCHUNK \([0-9]*\) /\1 /p' \
    | sort -n -k1,1 \
    | while read -r number chunk; do
        printf '%s' "$chunk"
    done > "$PATCH.b64"
# The chunks are base64 of a gzip stream, concatenated in order: decode once,
# decompress once.
base64 -d < "$PATCH.b64" | gzip -dc > "$PATCH"
rm -f "$PATCH.b64"

if [ ! -s "$PATCH" ]; then
    printf 'run %s published no formatting patch (the tree was already formatted)\n' "$RUN"
    rm -f "$PATCH"
    exit 0
fi

printf 'applying %s bytes of formatting to the working tree\n' "$(wc -c < "$PATCH")"
git apply --verbose "$PATCH"
rm -f "$PATCH"
printf 'done — review with `git diff`, then commit\n'
