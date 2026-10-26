#!/bin/sh
# ci-fetch-schemas.sh — put the contract's schema tree where the build expects it.
#
# The engine and the app both compile the schemas from
# `smart-protocol-official`; neither vendors them. CI and a fresh clone therefore
# need the same two lines, so they live here once:
#
#   ./scripts/ci-fetch-schemas.sh            # beside this repo (default)
#   SMART_SCHEMA_DIR=/some/where ./scripts/ci-fetch-schemas.sh
#
# Pins a commit, not a branch: a schema that changes under a build is a contract
# change, and a contract change must be a reviewed line in this file.
set -eu

PROTOCOL_REPO="${PROTOCOL_REPO:-https://github.com/smart-foundation/smart-protocol-official}"
PROTOCOL_SHA="${PROTOCOL_SHA:-9175b06bb7b2ddee87af4dd12aa02f07a4d3402f}"
REPO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
DEST="${1:-$REPO_DIR/../smart-protocol-official}"

if [ -n "${SMART_SCHEMA_DIR:-}" ] && [ -d "$SMART_SCHEMA_DIR" ]; then
    printf 'schema tree already present: %s\n' "$SMART_SCHEMA_DIR"
    exit 0
fi

if [ ! -d "$DEST/.git" ]; then
    printf 'cloning %s\n' "$PROTOCOL_REPO"
    git clone --quiet --no-checkout "$PROTOCOL_REPO" "$DEST"
fi

printf 'checking out %s\n' "$PROTOCOL_SHA"
git -C "$DEST" fetch --quiet origin "$PROTOCOL_SHA" 2>/dev/null || git -C "$DEST" fetch --quiet origin
git -C "$DEST" checkout --quiet "$PROTOCOL_SHA"

SCHEMA="$DEST/schema"
[ -f "$SCHEMA/clipboard.capnp" ] || {
    printf '!! %s has no clipboard.capnp — the checkout is not the schema tree\n' "$SCHEMA" >&2
    exit 1
}
printf 'schema tree: %s\n' "$SCHEMA"
printf 'export SMART_SCHEMA_DIR=%s\n' "$SCHEMA"
