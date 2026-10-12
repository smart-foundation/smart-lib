# 05 — The AI-first surface: calling the contract directly, no CLI

Wave 1, 2026-10-12. The founder's principle: smart-protocol is AI-first — you don't need a CLI at all; a caller invokes the contract directly and does work. The CLI is one client among many and adds NO capability. This document specifies the direct-call path so Wave 2 (harness + daemon) and Wave 3 (CLI + proofs) implement it as architecture, not as a retrofit.

## 1. The direct-call path (what an agent does, end to end)

```
acquire ──▶ call ──▶ observe ──▶ (repeat) ──▶ revoke / drop
```

1. **Acquire.** Connect to a transport, then obtain the network service's capability. The capability IS the authority (charter law #5); nothing downstream interrogates permissions. What acquisition means per transport (the transports already exist — `src/transport/**`):
   - **InProcess**: the caller holds the backend's `Cap<T>` directly — native, zero serialization. Used when the agent and the backend are one process (an embedded consumer, a test).
   - **UnixSocket**: capability transferred by SCM_RIGHTS (Linux-verified in `knowledge/reports/05-transport.md`); the service issues `Cap<network_capnp::network::Client>` on a successful `acquire` exchange.
   - **Shmem**: capability established once at the authenticated bootstrap (the bootstrap channel is a capability transfer); afterwards frame references resolve in the arena.
   - **Tcp**: DEGRADED by design — an in-band token with attenuated minting; the service must state the degradation (the conformance model's vocabulary), and the caller must treat every capability claim over TCP as weaker.
2. **Call.** Every network method the contract declares (76 + this wave's 7 = 83) is callable over the channel. No method requires a CLI-shaped string; arguments are contract structs, not text. An agent building a `CommandRequest` from a prompt and a shell human typing `net wifi on` are the same call — this is the law that keeps the CLI from becoming a capability.
3. **Observe.** Subscriptions are the contract's triplets: `subscribeX` → handle, `pullX → batch`, `cancelX`. Over a channel, pulls are bounded reads (the transport's recv bound is the backpressure). Drops are counted by the service and reported — never silent.
4. **Revoke.** `capability.capnp`'s `revoke`/`revokeControlled`; a revoked capability's calls fail with the contract's disconnected/denied error at the next call (the revocation semantics already proven in `knowledge/reports/04-core.md`).

## 2. Who issues capabilities on a Linux host (the daemon policy)

On Naledzi the OS's own services issue capabilities natively. On Linux, the **smart-service daemon** (Wave 2) is the issuer, and its policy is deliberately minimal and honest:
- Local uid-peer acquisition (SO_PEERCRED, already verified available in `knowledge/02-HOST-CAPABILITIES.md`): the connecting peer's uid decides the capability class.
  - **uid 0 / CAP_NET_ADMIN holder** (checked via `/proc/<pid>/status`, verified) → full control capability (the mutating verbs: setNetworking, setRadio, connect, routes, firewall, proxy).
  - **any other uid** → read-only capability: status, interfaces, counters, dns get, radio states. Mutating calls on this capability return `permissionDenied` — the honest answer, matching what the host would do anyway.
  - **no polkit prompt, no elevation, no suid**: the daemon is NOT a privilege broker; a caller that lacks host privileges gets host-truthful denials, not a GUI dialog.
- Over **Tcp**: tokens only, attenuated (control tokens are never minted; read tokens are), and every response carries the DEGRADED marking.

## 3. The generic service harness (Wave 2's API shape)

The harness turns ANY domain backend into a capnp service — network is the first domain served, not a special case.

```rust
/// What a domain must provide to be served (network implements this once):
pub trait ServiceDispatch {
    /// The schema identity this service serves (for the registry).
    fn schema_id() -> u64;
    /// Every method's availability row — the wire answers for unavailable.
    fn availability() -> &'static [MethodAvailability];
}

/// Serve backend B over channel C. Static dispatch: one monomorphized
/// match per method, no per-call allocation, no dyn on the hot path.
pub fn serve<B, C>(backend: B, channel: C) -> ServeHandle<C>
where B: NetworkBackend, C: Channel;
```

- `serve` runs a recv loop: read a message, dispatch by (interface, method ordinal) — the ordinal is the contract's own, so dispatch is a static match, not a lookup table.
- Subscriptions: `subscribe` mints a server-side subscription id; `pull` reads from a bounded per-subscription queue fed by the backend's event path; `cancel` drops it. The queue bound is the backpressure; overflow increments the drop counter that the next pull reports.
- Errors: the backend's `core::Error` maps to the contract's `T.Error` on the wire — the same total mapping already tested in core (187 tests), now exercised end-to-end per transport.
- Unavailable: a method whose availability row is not REAL answers `notSupported` with the row's reasonClass + reason — the wire never pretends.

## 4. The headless proof (Wave 3, the L6 gate)

A test client with NO parser, NO rendering, NO shell vocabulary — it builds capnp messages directly. Assertions:
1. Connect over each of the four transports; acquire per §1.1.
2. Read: `getInterfaces` + `isNetworkingEnabled` return contract-shaped results; JSON-free, string-free — the objects are compared as readers.
3. Observe: open a status subscription, pull ≥1 event or terminate cleanly, cancel; the drop counter (if nonzero) is reported, not hidden.
4. Control denial: on a read-only capability, `setNetworking(true)` returns `permissionDenied` with the expected kind — capability denial is contract-honest.
5. Tcp degradation: over TCP, the acquisition response carries the DEGRADED marking; the test asserts the caller CAN see it.
6. Zero CLI: the test links no command-parsing code path (a CI dependency check can assert the binary graph excludes the cmd modules).

## 5. What today's tree gives this spec, and what's missing (Wave 2/3's gap list)

**Exists:** the four channels with capability semantics (`src/transport/**`), `Cap<T>` + the revocation proof (core), the backend trait with the acquisition-shaped service boundary (`nicProvider @71`), the registry schemas (`cli.capnp`), the conformance vocabulary.
**Missing (build waves):** the `serve` harness + `ServiceDispatch` (`src/serve/**`), the daemon + its uid-peer policy, the subscription-over-channel queue, the wire-level availability answers, the headless client, and the CLI itself (which, per this spec, is a thin rendering client over exactly this path — nothing more).
