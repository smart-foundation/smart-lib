#!/bin/sh
# ci.sh — run exactly what GitHub Actions runs, on this machine.
#
#   ./scripts/ci.sh              # everything
#   ./scripts/ci.sh engine       # one stage: engine | proto | app
#
# The point is that "green in CI" and "green here" cannot diverge: the workflow
# calls this script's stages, so a fix made for CI is a fix made locally. A stage
# that cannot run here (no GPU stack, no capnp) fails loudly instead of skipping.
set -eu

STAGE="${1:-all}"
REPO_DIR=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$REPO_DIR"

DAEMON_FEATURES="std,rpc,domain-clipboard,domain-database"

step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }

need_capnp() {
    command -v capnp >/dev/null 2>&1 || {
        printf '!! capnp is not installed (apt install capnproto libcapnp-dev)\n' >&2
        exit 1
    }
    printf 'capnp: %s\n' "$(capnp --version)"
}

need_schemas() {
    eval "$("$REPO_DIR/scripts/ci-fetch-schemas.sh" | grep '^export ')"
    export SMART_SCHEMA_DIR
    printf 'schemas: %s\n' "$SMART_SCHEMA_DIR"
}

stage_engine() {
    need_capnp
    need_schemas
    step "engine: build the daemon"
    cargo build --bin smart-service --features "$DAEMON_FEATURES"
    # The full lib suite is enormous (the README warns it OOMs a small box), so the
    # stages that matter for this product run by name.
    step "engine: clipboard domain tests"
    cargo test --lib --features "$DAEMON_FEATURES" -- clipboard
    step "engine: database + archive tests"
    cargo test --lib --features "$DAEMON_FEATURES" -- database
    cargo test --lib --features "$DAEMON_FEATURES" -- archive
}

stage_proto() {
    need_capnp
    need_schemas
    step "proto: a contract client against a real served backend"
    ( cd crates/smart-clipboard-proto && cargo test )
}

stage_app() {
    need_capnp
    need_schemas
    step "app: build and test the gpui product"
    # The app crate compiles its own schema copies (same tree, same build.rs), so
    # it needs the same environment — that is the point of the shared script.
    ( cd crates/smart-clipboard && cargo build && cargo test )
}

case "$STAGE" in
    all) stage_engine; stage_proto; stage_app ;;
    engine) stage_engine ;;
    proto) stage_proto ;;
    app) stage_app ;;
    *) printf 'unknown stage: %s (engine | proto | app | all)\n' "$STAGE" >&2; exit 2 ;;
esac

step "done: $STAGE"
