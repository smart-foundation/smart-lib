# smart-lib

The Linux implementation of the [smart-protocol](https://github.com/smart-foundation/smart-protocol-official) contract.

## What this is

smart-lib is **one implementation** of the smart-protocol contract. It is not the implementation. Any third party can write their own implementation of the same contract, and every consumer (including smart-cli) will work identically.

## Architecture

```
smart-protocol (the contract)
       │
       ├── smart-lib (this repo — Linux implementation)
       │     ├── Engine traits + platform backends
       │     ├── Service that serves the contract over capnp RPC
       │     └── The coupled shell (being retired domain-by-domain)
       │
       └── smart-cli (contract-only consumer — does not know smart-lib)
```

## The contract is the API

Every domain, every method, every type is defined in [smart-protocol-official](https://github.com/smart-foundation/smart-protocol-official). This repo implements them. The schema is the authority — not this code.

## Building

```bash
# Build the library
cargo build -j6

# Build the daemon (serves the contract over capnp RPC)
cargo build -j6 --example smart-service --features rpc,std,domain-network

# Build with specific domains
cargo build -j6 --features domain-files,domain-process,domain-settings
```

## Testing

```bash
# Engine tests (one domain at a time — the full suite OOMs this box)
cargo test -j6 --lib --features domain-network
cargo test -j6 --lib --features domain-files

# Serve tests
cargo test -j6 --test serve_rpc

# Shell tests (one binary at a time)
cargo test -j6 -p shell --test network_cmd
```

## The coupled shell

`crates/shell/` contains the shell that is being retired domain-by-domain. Each domain that is ported to the contract loses its module here. When the last domain is ported, this crate is deleted.

The shell's presentation (tables, glyphs, truncation) is being ported to smart-cli's presenter framework.

## Install (this is the part that was missing)

The tree could be built; it could not be *run*. There is now a daemon binary, a
session unit, and a desktop entry, so a machine records copies after a single
command:

```bash
git clone https://github.com/smart-foundation/smart-protocol-official ../smart-protocol-official
./install.sh            # smart-service + systemd --user unit + smart-clipboard + desktop entry
./scripts/doctor.sh     # if anything is not working, this says which piece and how to fix it
```

`smart-service` serves the clipboard domain over the native seat (Wayland
`zwlr_data_control` or X11's ICCCM protocol — never `wl-paste`/`xclip`), watches
the selection so **copies made by other applications are captured**, keeps the
history in a `database.kv` journal under `$XDG_STATE_HOME/smart/database` so it
survives a restart, and serves each client connection on its own thread.

```bash
cargo build --release --bin smart-service --features std,rpc,domain-clipboard,domain-database
./target/release/smart-service --help
```

CI (`.github/workflows/ci.yml`) has four jobs, all green: it builds the daemon,
runs its clipboard/database/archive tests, boots it on a bare runner and waits for
its socket; round-trips the contract client against a real served backend; builds
the gpui app against the real platform stack and runs its pure tests; and checks
that both product crates are formatted and that no shellout has appeared. So a
green run means "drop it on a desktop and it starts". `./scripts/ci.sh` runs the
same stages locally.

## Products (the first one)

`crates/smart-clipboard/` is the first finished domain made into a product: a
gpui desktop app that is a **pure client** of the clipboard contract. It reads
history, previews and payloads from whatever service serves `clipboard.capnp`,
searches them, and asks the service to hold an item again. It owns no clipboard
backend, no selection transport and no store — it never writes a byte to disk.

`crates/smart-clipboard-proto/` is its contract client (discovery → clipboard →
capability), with no engine and no gpui, so it can be tested headless and reused.
Both crates are their own workspaces and are excluded from this one: the engine's
build never pays for a product's dependency tree.

The specification — UX, wireframes, layout rationale, states, accessibility —
is `knowledge/30-smart-clipboard-app.md`.

## Key principles

1. **No shellouts.** Zero `Command::new` outside tests. Every backend reads the native mechanism.
2. **Honest availability.** Every method publishes REAL / DEGRADED / Unavailable with a measured reason. No fabricated numbers.
3. **Shellout-free, and provably so.** `python3 scripts/check-no-shellouts.py` — a grep cannot
   state this rule, because the tree's comments quote the spawns it refuses to make
   (`Command::new("xclip")`) and the tests start a child to have a process to observe.
4. **No hardcoded paradigms.** Transports are open tokens, not closed enums. The transport set is extensible.
5. **The contract is the authority.** This code implements it; it does not define it.

## License

MIT OR Apache-2.0
