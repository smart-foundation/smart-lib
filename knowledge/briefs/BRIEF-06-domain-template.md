# BRIEF-06 — Domain implementation template (Waves 3+, per-domain agents)

## Mission pattern
One domain per agent. A domain is DONE when: its trait is published, its Linux backend is real or honestly degraded, its public API is ergonomic, its behavior tests pass, and its availability table is published. You build ONE domain, completely. Later platform agents (macOS/Windows) build against your published trait.

## Your exclusive files (create; you own them)
- `src/domains/<domain>.rs` or `src/domains/<domain>/mod.rs` (+ submodules `backend.rs`, `api.rs`, `types.rs`, `availability.rs`, `tests` — your layout choice, all under your domain's path)
- `src/platforms/linux/<domain>.rs` (the Linux backend implementation)
- your report `knowledge/reports/NN-<domain>.md` and optional `knowledge/lessons/NN-<domain>.md`
You do NOT own and MUST NOT edit: `Cargo.toml`, `build.rs`, `src/lib.rs` (skeleton agent — it auto-wires your files via `#[path]`; the build log names any enabled feature still unwired), `src/core/**` (use its API), `src/transport/**` (use if needed, don't edit), `src/conformance/**` (the conformance wave; your L2-style behavior tests live INLINE in your own files), any other domain's files. No git commits.

## Hard rules (from the charter, non-negotiable)

- **The host is read-only (law #8).** Reading is free; changing host state is banned: no state-changing calls to host services or daemons, no `sudo`, no killing/starting processes, no writing `/etc` or `/var`. Your backend's mutating paths are verified with mock seams and read-only host assertions; the live proof is founder-run. (Law exists because a live-fire D-Bus probe once killed the founder's wifi across reboots.)
- You own ONLY the files listed above. You do NOT own and MUST NOT edit: `Cargo.toml`, `build.rs`, `src/lib.rs`, `src/core/**`, `src/transport/**`, `src/conformance/**`, any other domain's files. No git commits.
- One build at a time, `-j 6`, no `cargo clean`, never `cargo build --all-features` (OOM on this box — `cargo check --all-features` at most).

## Inputs (read-only, in this order)
1. `knowledge/00-CHARTER.md` — the seven laws.
2. The contract schema for your domain: `smart-protocol-official/schema/<domain>.capnp` — and the inventory entry for it in `target/` (OUT_DIR is exposed via `SMART_LIB_CODEGEN_INVENTORY` env during builds; or read `knowledge/reports/06-all-features.md` for the counts). **The schema is the requirements source — enumerate methods from the inventory/schema, never from memory.**
3. `knowledge/02-HOST-CAPABILITIES.md` — YOUR DOMAIN's section of the host map (cost pyramid, native APIs, fragmentation, authority mapping, pitfalls). If your domain's Linux column says a method is `honest-permanent`, it stays Unavailable-with-reason forever — that is correct, not failure.
4. `knowledge/reports/04-core.md` + `src/core/**` — the Error/Result/ID/Cap API you build on.
5. `knowledge/03-CONFORMANCE.md` — L1/L2 are your bar; the availability table format is yours to publish per method.
6. `knowledge/reports/05-transport.md` + `src/transport/**` — use the Channel API if your domain needs a transport path (e.g. remote backends), don't edit it.

## The build checklist

- [ ] **Trait**: `<Domain>Backend`, object-safe (`&self` methods, or `&mut self` where the host requires it — justify), one method per contract method your Linux host can serve, each returning `core::Result` with honest `T.Error` mapping (via core, never hand-rolled error strings). Methods that require authority take `&Cap<T>` at acquisition — no permission strings, no unauthenticated mutation paths. Document the trait's capability story at the top (what Cap grants what).
- [ ] **Honest-Unavailable rule**: every contract method your backend cannot serve on Linux is STILL in the trait with a default or explicit impl returning `Error::unavailable(reason)` with a nameable reason class (no-backend / no-privilege / not-applicable / disabled-by-policy). The public API may hide them; the trait may not lie about them.
- [ ] **Linux backend**: implements the trait over native host APIs per Agent B's map for your domain. Hard rules: NO shelling out to CLI tools (no `pactl`, no `ip`, no `cp -r` — fork-per-op, locale parsing, lost error codes, and the old implementation was condemned for exactly this); NO fabricated values (the old impl hardcoded 47 capability bits, `localhost`, `$HOME` for all special paths, 44100 Hz — every value must trace to a host call or be declared-N/A); zero-copy where the contract says DataHandle (pass handles, not bytes); bounded pulls, no polling loops.
- [ ] **std boundary**: your platform file may `extern crate std;` locally (Linux backend lives in a std process); the public API and core must stay `no_std`+`alloc` compatible.
- [ ] **Public API**: ergonomic wrappers over the trait — the types a caller actually touches (borrowed where possible, owned where necessary), with `#[must_use]` on queries. Keep it SMALL: fewer concepts beat more features; if the public API needs a paragraph to explain a type, it's wrong.
- [ ] **Behavior tests (inline, in your files)**: (a) a mock backend implementing your trait that can inject every `T.Error` variant — prove the mapping is total and honest; (b) pagination/cursor semantics for unbounded lists (a page that goes stale mid-iteration must not silently skip — cursor not offset, unless the contract says otherwise); (c) bounded-pull termination for any streaming; (d) DataHandle paths prove no inline bulk (assert the type reaching the backend is a handle); (e) at least one REAL Linux integration test that runs on this box or is `#[ignore]`d with a documented reason (never silently skipped).
- [ ] **Availability table**: `availability.rs` — per contract method: status REAL/DEGRADED/UNAVAILABLE, reasonClass, reason, matching `knowledge/03-CONFORMANCE.md`'s schema exactly. REAL claims must have a passing test (or the ignore-documented reason above). This table is published data, later consumed by the availability generator.
- [ ] **Tests green**: `cargo test --features domain-<domain>` passes; warning-clean for your files.
- [ ] **Teaching docs**: 5-line doc at the top of your `mod.rs` explaining the domain to the next implementer (what the domain is, what the Linux backend actually does, what is honestly unavailable and why).

## Report
`knowledge/reports/NN-<domain>.md`: checklist status with test-output evidence; the trait API sketch; the availability table; every host-integration decision with its cost; what you deliberately did NOT build and why; anything the macOS/Windows agents must know (that's the handoff document for Wave 4).

## Return (final message, ≤25 lines)
Checklist status, trait method count (of contract methods) + unavailable-with-reason count, test counts, the one thing you learned that changes the next domain's approach.