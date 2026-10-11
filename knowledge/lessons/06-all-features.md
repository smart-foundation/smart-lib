# 06 — lessons: all features, wave modules, the inventory

Appended by the Wave-1 skeleton agent (task 06). Costs a debugging cycle each.

## capnp traversal budget: the bug that silently truncates output

1. **`capnp`'s reader spends a finite traversal budget, and every pointer dereference costs.**
   `capnp::serialize::read_message` with `ReaderOptions::new()` allows 8×1024×1024 words; a
   message repeated N times eventually returns `Err(ReadLimitExceeded)` from the same accessor
   that worked a second earlier. My first inventory re-walked the node list for every lookup
   (~650 walks of a 90k-word message) and the tail of the tree silently stopped reading: 28
   interfaces claimed, 23 listed, `network.capnp` lost three, `system.capnp` lost both.
   **The code swallowed the error with `.ok()?`** — that is what turned a loud failure into a
   quiet lie.
   Rules that follow: (a) index a capnp message **once**, then look up from a map; (b) never
   `.ok()?` a read whose result you are counting — a missing node must be a panic, not a skip;
   (c) assert internal consistency (`emitted lines == summary totals`) so truncation cannot pass.

## `displayNamePrefixLength` includes the separator

2. `displayNamePrefixLength` counts the characters **before** a node's simple name **including**
   the separator that follows it: `:` at file scope (`files.capnp:` is 13) and `.` inside a type
   (`files.capnp:Files.` is 19). So `display[..len-1]` is `<file>[:<scope>]` and
   `display[len..]` is the simple name. Two traps I hit: slicing at `len` (gives `files.capnp:`,
   never matches a schema file), and assuming the file node has no `:` — it is the odd one out,
   its "name" is the extension (`abi.capnp` with prefix 4 → scope `abi`), which is exactly why
   file nodes fall out of a "must look like `<file>.capnp`" filter for free.

## Compiling the whole contract

3. **`--all-features` builds codegen but not the library.** 89 schemas → ~125 MB of generated
   Rust in one crate → rustc OOM-killed (SIGKILL) on a 15 GB box after 13 minutes.
   `cargo check --all-features` survives (metadata only, ~9 s). Treat "enable everything" as a
   codegen/feature-graph audit, never as a build target: this is charter law #2 with a number.
4. **Only three well-known types carry all the shape information** the conformance rule table
   keys on: `capability.capnp:Capability`, `types.capnp:Pagination`, `types.capnp:DataHandle`,
   and `types.capnp:Error` for the result-union rule. Resolve them by `(file, name)` from the
   request's own node set rather than by hardcoded ids.
5. There is exactly one type actually named `Capability` in the tree; every other match a grep
   finds is a different type (`CapabilityDescriptor`, `CapabilityHandle`, `CapabilityRequest`,
   `CapabilityIndex`, `CapabilityChange`, `CapabilityCheck`, `CapabilityRequirement`, …).
   `find(|info| info.local == "Capability")` keyed by file is safe; `grep "^struct Capability"`
   is not.

## Registration pattern

6. **An inline `pub mod x {}` placeholder in a shared file is a trap.** The moment the owner
   writes `src/x/mod.rs` somebody has to edit the shared file to switch to `pub mod x;`. The
   `#[path]`-from-a-generated-file form flips automatically, and I watched it flip twice during
   this session (core at 14:14, transport at 14:42) with nobody touching a shared file.
7. A module wired with `#[path = "<abs>/src/transport/mod.rs"]` resolves its *own* submodules
   normally (`pub mod tcp;` → `src/transport/tcp.rs`). Nothing special for the owner to learn.
8. **cargo gives each feature set its own build-script `OUT_DIR`**, so several feature-set
   artifacts coexist in `target/debug/build/smart-lib-*/out`. Identify a directory by its
   artifact content (the `smart_lib_modules.rs` header names the enabled features), **not** by
   `ls -dt` — a directory's mtime only changes when entries are added or removed, so a fresh
   overwrite looks older than an unrelated new directory.
9. `cargo::rerun-if-changed` on a **missing** path makes cargo consider the build script dirty on
   every build. Only emit it for directories that exist (I only watch `src/core` etc. once they
   appear).
