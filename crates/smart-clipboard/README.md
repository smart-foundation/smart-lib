# smart-clipboard

A gpui window over the smart-protocol **clipboard** contract.

It shows what has reached the clipboard — text, links, screenshots, file
references — lets you search it, preview it, and ask the service to hold an item
again. That is the whole product.

**It owns no clipboard.** No X11 selection, no `wlr-data-control`, no
`wl-paste`, no keystroke injection. **It stores nothing.** No history file, no
SQLite, no cache on disk. **It starts nothing.** No daemon, no subprocess.

Everything it shows came from a service it found with `discovery.capnp`, and
everything it does is a capnp call on `clipboard.capnp`. If smart-lib is not
running, the app says so and lists the endpoints it tried — it does not become
the backend.

```
crates/smart-clipboard        this crate: gpui windows, rows, inspector, palette
crates/smart-clipboard-proto  the contract client: discovery → clipboard → capability
                              (no gpui, no engine, testable headless)
```

The full specification — UX, wireframes, layout rationale, states, accessibility,
and the honest list of what is not done — is
[`knowledge/30-smart-clipboard-app.md`](../../knowledge/30-smart-clipboard-app.md).

## Build

gpui pulls a real platform stack (X11/Wayland, Vulkan), so this crate is its own
workspace and is excluded from the engine's build.

The daemon is a real binary with a session unit, so the short path is one
command from the repository root:

```bash
./install.sh          # builds + installs the daemon and the app, starts the unit
```

By hand, if you would rather see every step:

```bash
# 0. the contract's schema tree, beside this checkout (pinned commit)
./scripts/ci-fetch-schemas.sh

# 1. the daemon (clipboard + the database its history lives in)
cargo build --release --bin smart-service --features std,rpc,domain-clipboard,domain-database
./target/release/smart-service --socket /tmp/smart-service.sock

# 2. the app
cd crates/smart-clipboard && cargo run --release
```

The app finds the service by trying, in order:

1. `--socket <path>` (or `--addr tcp://host:port`)
2. `$SMART_SERVICE_SOCKET`
3. `$XDG_RUNTIME_DIR/smart/service.sock`
4. `/tmp/smart-service.sock`

Nothing answered? The window becomes the list of those four attempts and what
each said. There is no spinner, because there is nothing to hide.

The schemas are compiled by `crates/smart-clipboard-proto/build.rs` from the
canonical tree, found the same way the engine finds it:

```bash
SMART_SCHEMA_DIR=/path/to/smart-protocol-official/schema cargo run --release
```

## Keys

| Key | Action |
|---|---|
| type | search (always live, 140 ms debounce) |
| `↑` `↓` | move |
| `⏎` | copy the first representation back |
| `⇧⏎` | offer every representation at once (`setOffer`) |
| `P` | pin / unpin |
| `⌫` | delete, with a confirmation |
| `G` | list ↔ gallery |
| `S` | sort (this window only) |
| `T` | theme |
| `R` | refresh |
| `⌘/Ctrl+K` | quick palette |
| `⌘/Ctrl+F` | read what the clipboard holds now |
| `esc` | clear the query, then deselect |

`smart-clipboard --palette` opens straight into the quick palette.

## Two windows, one session

* **Library** — 1180×760 native window: facet rail, rows or gallery, a payload /
  contract inspector, and a status bar that always names the endpoint, the
  schema id it verified, the capability class, and how many contract methods
  this session actually exercised.
* **Quick palette** — 680×480 borderless card at the top of the screen, opened
  with `⌘K` or `--palette`: type, `⏎`, done. It copies; it never pastes.

Every local computation is labelled local. The rail's facets, the sort, the
sensitive-item heuristic and the 64 MiB payload ceiling all run in this process,
and the UI says so on screen — the contract's search is text-only, so a filter
that implied it searched the whole history would be a lie.

## Status of this tree

Written against the contract, but **not compiled**: the environment this was
authored in had no Rust toolchain and no crates.io access. `cargo build` on a
real box is the next step, and the two places most exposed to gpui's API drift
(window dismissal and `RenderImage` construction) are isolated in
`src/app.rs` on purpose. See §16 of the spec.
