# BRIEF-04 — core: errors, IDs, capability handles, values (Wave 2, Agent D)

## Mission
Build `src/core/**` — the foundation every domain and transport stands on: the error model with a TOTAL mapping to the contract's `T.Error`, the contract's ID types, the capability-handle wrapper (authority-is-a-handle, law 5), and the shared value types. Small, `no_std`+`alloc`, zero-allocation on the hot paths.

## Your exclusive files (create; you own them)
- `src/core/**` (mod.rs, error.rs, ids.rs, capability.rs, values.rs, tests)
- your report `knowledge/reports/04-core.md` and optional `knowledge/lessons/04-core.md`
You do NOT own and must not edit: `Cargo.toml`, `build.rs`, `src/lib.rs` (skeleton agent owns the module declaration for `core` — it pre-declares `pub mod core;`), `src/transport/**`, `src/domains/**`, `src/platforms/**`. No git commits.

## Inputs (read-only)
- Charter: `knowledge/00-CHARTER.md`
- Contract: `smart-protocol-official/schema/types.capnp` (T.Error, T.*Id types, T.Quota, T.Pagination, value types), `capability.capnp` (the authority model: issue/attenuate/transfer/revoke/revokeControlled), `abi.capnp`
- Skeleton: the generated `schema` module (capnp bindings) + `src/lib.rs` documented pattern
- Old implementation's error vocabulary (study, never edit): `smart-protocol/crates/smart-protocol/src/backend.rs` (SmartError enum — the old god-trait's error side is worth keeping as a checklist of variants)
- Conformance model: `knowledge/03-CONFORMANCE.md` (L1: mapping must be TOTAL, both directions)
- Host research: `knowledge/02-HOST-CAPABILITIES.md` (authority-mapping section — how host creds become handles)

## The checklist

- [ ] `Error`: an enum covering the contract's `T.Error` kinds (read the schema — do not guess the variant set), with `From<Error> for <generated T.Error reader/builder>` and the inverse, TOTAL both directions (a test constructs every variant of each side and round-trips; unmapped variants are a build-time total — use exhaustive matches, never a catch-all `_ =>` that silently drops).
- [ ] `Error` carries the honest fields the contract carries (message, retryable where the schema models it) and the constructor helpers the backend writers need: `Error::unavailable(reason: impl Into<String>)`, `Error::permission_denied()`, etc.
- [ ] `Result<T>` type alias used crate-wide.
- [ ] ID types: newtypes over the contract's ID representation for every `T.*Id` in types.capnp (KeyId, Port, EndpointId, MountId, FilesystemId, StoragePoolId, VolumeId, SnapshotId, JobId, and the rest the schema actually declares — enumerate from the schema, not from memory). Each: `Copy`, `Eq`, `Hash`, `Debug`, no allocation, conversions to/from the generated type.
- [ ] Capability handle: `Cap<T>` — a zero-cost newtype over the capnp capability client for the interface `T`, documenting the acquisition contract: holding it IS the authority (law 5); no permission strings anywhere in the crate. Include the acquisition-time mapping hook signature that platform backends will implement (host credential → `Cap<T>`), without implementing any host logic.
- [ ] Value types: the shared values domains need (from the contract: Rect/Vec-style geometry, Version, Duration wrapper over the contract type) — only what domains actually reference; do not invent types the contract lacks.
- [ ] `no_std` + `alloc` clean; `#[must_use]` where appropriate; no `std::` anything.
- [ ] Tests: round-trip totality for Error (every variant both directions), ID conversions, capability handle construction + revoke-propagation semantics test with a mock capability (document how revocation reaches a held `Cap<T>` — if the contract's model makes this automatic via the capability system, show the test proving it; if not, state the invariant loudly in a doc comment).
- [ ] A doc comment at the top of `src/core/mod.rs` explaining the error model in 5 lines for domain implementers (this is the file every domain author reads first — make it teach).

## Report
`knowledge/reports/04-core.md`: checklist status with evidence (test output verbatim), the full Error variant table with its T.Error counterpart, the ID type list (from the schema, with the count), the `Cap<T>` design + revocation semantics, and anything the domain agents must know when writing against your API.

## Return (final message, ≤25 lines)
Checklist status, Error variant count and mapping witness (test output line), ID type count, the `Cap<T>` one-paragraph design, blockers.