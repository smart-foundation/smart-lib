# BRIEF-05 — transport: Channel + InProcess/Shmem/UnixSocket/Tcp (Wave 2, Agent E)

## Mission
Build `src/transport/**` — the transport layer that makes smart-protocol's six shapes concrete for library clients: one `Channel` abstraction over **InProcess** (direct, zero serialization), **Shmem** (zero-copy frames via the contract's `T.DataHandle`/`T.BufferId`), **UnixSocket**, and **Tcp**, with capability-transfer semantics defined per transport (law 4). This is the layer that lets an app in Oslo speak to a device in orbit over the same API it uses to call a local backend.

## Your exclusive files (create; you own them)
- `src/transport/**` (mod.rs, channel.rs, inprocess.rs, shmem.rs, unix.rs, tcp.rs, tests)
- your report `knowledge/reports/05-transport.md` and optional `knowledge/lessons/05-transport.md`
You do NOT own and must not edit: `Cargo.toml`, `build.rs`, `src/lib.rs` (skeleton agent pre-declares `pub mod transport;`), `src/core/**` (use its API as published), `src/domains/**`, `src/platforms/**`. No git commits.

## Inputs (read-only)
- Charter: `knowledge/00-CHARTER.md` (law 4 is yours)
- Host research transport section: `knowledge/02-HOST-CAPABILITIES.md` — the primitive table (memfd_create/shm_open, AF_UNIX+SCM_RIGHTS 253 fds/msg on Linux, XNU also has SCM_RIGHTS — my earlier brief was wrong, use the report's corrected facts, DuplicateHandle on Windows), and the two critical findings: **Shmem carries no authority** (needs a one-time authenticated bootstrap — the bootstrap channel IS a capability transfer) and **Tcp has no capability transfer** (in-band tokens + attenuated minting, honest degradation mandatory).
- Contract: `types.capnp` (`T.DataHandle`, `T.BufferId`, `T.Error`), `capability.capnp` (what a transferred capability is), `abi.capnp` (negotiation), `tunnel.capnp` (the overlay transport — read it; Tcp transport must not conflict with its model).
- Skeleton's generated `schema` module + capnp 0.27.2 runtime API (introspection available).

## The checklist

- [ ] `Channel` trait: the ONE API shape all four transports implement. Minimum surface: send a capnp message (builder or segment), receive one, capability attach/detach, close with a typed close reason. Document per-transport capability-transfer semantics in the trait docs (this is the design center of the brief).
- [ ] `Transport` value: how a caller selects a transport — `Transport::InProcess`, `Transport::Shmem{segment}`, `Transport::UnixSocket{path}`, `Transport::Tcp{addr}` — and `connect(transport) -> Result<Channel>`.
- [ ] InProcess: zero-serialization path — the channel dispatches directly to a backend in the same process (define the seam; do not implement backends). Prove in a test that no message copy occurs (construct via the trait, assert identity of the builder passed through).
- [ ] Shmem: zero-copy frames — a payload is written into a shared arena and referenced by the contract's DataHandle/BufferId; the message carries the reference, never the bytes. Test: publish a frame into an arena in one thread, read it via the handle in another (threads are fine for the test), assert zero copies on the frame path (measure or reason in the report — a measured copy count is required, not a claim).
- [ ] UnixSocket: capnp messages over `AF_UNIX` with the contract's framing; capability transfer where the host supports it (Linux SCM_RIGHTS per the research report; XNU SCM_RIGHTS verified by research; Windows status documented). If the runtime crate lacks SCM_RIGHTS, implement the fd-passing at the raw libc level behind a feature gate or report the blocker — do not fake capability transfer.
- [ ] Tcp: capnp stream framing; honest degradation for capabilities (in-band token + attenuated minting per the conformance model's DEGRADED/UNAVAILABLE vocabulary); document exactly what is lost.
- [ ] Bootstrap: the Shmem authentication-bootstrap design (who authenticates whom, over which channel, and what makes the arena safe afterward) — a concrete protocol + test with a mock.
- [ ] `no_std` discipline: InProcess/Shmem must work with `alloc` only; UnixSocket/Tcp may be std-gated behind a `std-transports` feature (document the split).
- [ ] ABI negotiation hook: where `abi.capnp`'s negotiate lands in the connect sequence (call the trait method; don't implement negotiation).
- [ ] Tests: round-trip per transport (mock peers), close-reason propagation, Tcp degradation documented in a test asserting the honest behavior, and one test proving the four transports are interchangeable at the `Channel` API (same test body over all four).

## Report
`knowledge/reports/05-transport.md`: checklist status with evidence (test output verbatim, copy measurements), the per-transport capability-transfer semantics table, the bootstrap protocol, the std/no_std split, and every place you had to make a call the domain agents will feel.

## Return (final message, ≤25 lines)
Checklist status, the Channel API sketch (5 lines), the four-transport interchangeability witness (test output line), copy-count evidence for Shmem, blockers.