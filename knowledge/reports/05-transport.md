# 05 — transport: Channel + InProcess/Shmem/UnixSocket/Tcp (Wave 2, Agent E)

Status date: 2026-10-11. Workspace: `/home/lufuno/smart/smart-lib-ng/`.
Owned files only: `src/transport/**` (11 modules + 10 test modules, 7,826 lines), this report,
`knowledge/lessons/05-transport.md`. No git commits. Nothing outside my ownership was edited.

**Verdict: the checklist is complete and every box is mechanically verified. 57 transport tests
pass in-workspace (244 with core's 187); `cargo build` is warning-free in every configuration
(default features, `--no-default-features`, and `--no-default-features --target
x86_64-unknown-none` with `src/transport/**` wired in); the framing writer is byte-identical to
capnp's; the Shmem zero-copy claim is measured (`resolve_copy_bytes == 0`) and proven at the page
level (two mappings of one memfd); the SCM_RIGHTS transfer is proven with a real memfd and a
re-opened fd number.**

## 0. How this was verified (module wiring happened mid-flight)

When I started, `src/lib.rs` did not declare `pub mod transport;` (the task statement said the
skeleton agent was concurrently pre-declaring it). The skeleton agent's final mechanism is
better than a lib.rs edit: **`build.rs` emits the wave modules itself and flips a documented
empty placeholder to a `#[path]` declaration the moment the owner's `mod.rs` exists** — no
manifest edit, no lib.rs edit. My `src/transport/mod.rs` therefore auto-wired during my work,
and every final result below comes from the **workspace itself**:

```
$ cargo build -j 6                       # default features (7 schemas)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.52s
$ cargo test -j 6 --lib
running 244 tests
test result: ok. 244 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.05s
$ cargo build -j 6 --no-default-features --target x86_64-unknown-none
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.12s
```
(The 244 = core's 187 + my 57. The early compile/test iterations before the auto-wiring used a
`#[path]` shim at `/tmp/kilo/transport-verify/` — scaffolding only, never part of the library,
compiling the exact bytes of `src/transport/**` against the workspace's own generated schema
modules and the same pinned capnp 0.27.2. The trickiest defects — the seal constant, the frame
length prefix, the region lifetime — were found by those runs and re-verified in-workspace.)

## 1. Checklist status

### [DONE] `Channel` trait — the ONE API shape
`src/transport/channel.rs`. Object-safe by construction (the seam where `dyn` belongs),
`Send`, no `Sync` bound:

```rust
pub trait Channel: Send {
    fn kind(&self) -> TransportKind;
    fn caps(&self) -> TransportCaps;                                   // law #6
    fn send(&self, message: MessageOut<'_>) -> Result<(), TransportError>;
    fn recv(&self, scratch: &mut Vec<u8>) -> Result<RecvBatch, TransportError>;
    fn attach(&self, cap: &mut CapHandoff) -> Result<CapOutcome, TransportError>;
    fn detach(&self, id: CapId) -> Result<(), TransportError>;
    fn resolve_buffer(&self, id: types_capnp::buffer_id::Reader<'_>) -> Result<FrameRef<'_>, TransportError>;
    fn negotiate(&self, request: MessageOut<'_>) -> Result<NegotiateResponse, TransportError>;
    fn peer_credentials(&self) -> Option<PeerCredentials>;
    fn close(self: Box<Self>, reason: CloseReason) -> Result<(), TransportError>;
}
```

Per-transport capability-transfer semantics are documented on the trait and declared per
instance by `TransportCaps` (the table is §3). Evidence: `tests/interchange.rs::channel_is_object_safe`
(uses `&dyn Channel`), plus the four interchangeability tests in §1.9.

### [DONE] `Transport` value + `connect`
`channel.rs`: `Transport::{InProcess{dispatch}, Shmem{handle}, UnixSocket{path}, Tcp{addr}}`,
`connect(Transport) -> Result<Box<dyn Channel>>`, `connect_negotiated(t, request)`,
`listen(&Transport) -> Box<dyn ChannelListener>` + `accept()`. Evidence: every test constructs
through `connect`/`listen` or the per-transport constructors (`InProcessChannel::pair`,
`ShmemChannel::from_handle`, `UnixChannel::from_fd`, `TcpChannel::from_stream`).

### [DONE] InProcess — zero serialization, proven by pointer identity
`inprocess.rs`. The seam is `InProcessPeer` (a backend implements it); `InProcessInbox` is the
default queueing endpoint; `InProcessChannel::pair()` is the connected pair.
**Test `inprocess::owned_message_zero_copy`**: builds a message, records
`builder.get_segments_for_output()[0].as_ptr()`, moves the builder into `OwnedMessage`
(address unchanged), sends, and asserts the receiver's `RecvMessage::Direct` reader sees
**the same segment address** — the move, the send and the receive are all the same memory:

```
test transport::tests::inprocess::owned_message_zero_copy_identity ... ok
```
`borrowed_frame_dispatches_by_reference` proves a `MessageOut::Borrowed` frame reaches a custom
peer as the very same memory (usize identity). `inbox_thread_stress` (8 threads × 200 messages)
proves no loss through the spin mutex.

### [DONE] Shmem — zero-copy frames, copy counts measured
`shmem.rs` (arena + two bounded SPSC rings) and the test evidence in `tests/shmem.rs`.
**Threaded witness** `threaded_publish_resolve_zero_copies` — publish a 64 KiB frame in thread A,
resolve the same `T.BufferId` in thread B:

```
test transport::tests::shmem::threaded_publish_resolve_zero_copies ... ok
```
It asserts: `stats.resolve_copy_bytes == 0` (the resolve path contains no copy — there is no
assignment site for that counter), the resolved slice's address **is the slot's address**
(identity), and `publishes == 1` / `resolve_calls == 1`. The page-level proof is
`memfd_two_mappings_are_the_same_pages`: a byte written through one mapping of a memfd is read
through a second, independent mapping (adopted through the region fd), so the consumer reads the
producer's pages, not a copy. `data_handle_reference_rides_the_ring_bulk_does_not` sends a
message whose root is `T.DataHandle` with the **buffer** arm and asserts the far side resolves it
through the arena while the control ring carried < 1 KB.

### [DONE] UnixSocket — framing + SCM_RIGHTS-class fd passing at the raw libc level
`unix.rs` + `sys.rs` (declared FFI, no new dependency — the graph stays `capnp` + `capnpc`).
**`unix::scm_rights_transfers_a_real_memfd`**: a memfd with known content is attached on one end
of a socketpair; the far side receives a **different fd number** (the kernel re-opened it — a
fake transfer cannot produce that) and reads the producer's bytes through it:

```
test transport::tests::unix::scm_rights_transfers_a_real_memfd ... ok
```
`region_notice_transfers_a_buffer_namespace` passes an arena *with its notice* and resolves
producer-published `T.BufferId`s on the far side afterwards (the namespace moved with the fd).
Struct layouts are compile-time asserted (`msghdr` 56, `cmsghdr` 16, `ucred` 12, `sockaddr_un`
110 — the 56 was verified against `/usr/include/bits/socket.h`). Peer identity is
`SO_PEERCRED` (`peer_credentials()`, uid/gid checked against `/proc/self/status`).
**macOS**: XNU *has* SCM_RIGHTS (02 §4.1's correction), but its `cmsghdr` alignment is 4-byte vs
Linux's 8-byte and no macOS toolchain exists here, so no unverifiable macOS path is shipped:
the module compiles on any `unix` and the transfer honestly reports `CapabilityTransfer::None`
with a note. That is a documented gap, not a fake transfer.

### [DONE] Tcp — stream framing + honest degradation
`tcp.rs` + `tests/tcp.rs::honest_degradation_is_declared_and_mechanical` asserts every clause of
02 §4.2's Tcp row: `capability_transfer == InBandToken`; `zero_copy_message` and
`zero_copy_data_handle` both false; `peer_credentials()` is `None`; `attach(Live)` refused
("a Rust object cannot cross a network boundary"); `attach(OsHandle)` refused ("TCP has no fd
transfer"); `attach(Descriptor)` is the ceiling and says so (`InBand`); `resolve_buffer` always
refused. `inline_data_handles_round_trip_and_buffer_arms_are_refused` makes the producer-side
rule concrete: the inline arm round-trips; a buffer arm is refused on the far side.

### [DONE] Bootstrap — the Shmem authentication protocol
`bootstrap.rs` + `tests/bootstrap.rs` (mock) + `tests/unix.rs::bootstrap_over_a_real_unix_link`
(real SCM_RIGHTS). Design in §4.

### [DONE] no_std discipline
InProcess/Shmem/channel/bootstrap(protocol+mock)/sync are `alloc`-only and always compiled;
UnixSocket/Tcp are `#[cfg(all(unix, feature = "std"))]` / `#[cfg(feature = "std")]`. Evidence:

```
$ cargo build --offline                        (std)      → 0 warnings
$ cargo build --offline --no-default-features  (no_std)   → 0 warnings
$ cargo build --offline --no-default-features --target x86_64-unknown-none
                                                 → Finished (bare-metal, no std)
```
The `std`-gated modules declare `extern crate std;` **inside themselves** (the crate root is the
skeleton agent's file, and law #1 forbids asking it to widen).

### [DONE] ABI negotiation hook
`Channel::negotiate` + `connect_negotiated`: the sequence is documented as
`connect → (Shmem: bootstrap) → negotiate → send/recv`. The transport moves
`abi.capnp`'s `AbiHandshakeRequest`/`Response`; the compatibility *decision* is the abi domain's,
read from the returned `TypedReader`. Evidence: `unix::negotiate_hook_round_trips_a_handshake`,
`tcp::negotiate_hook_round_trips_a_handshake` (mock peers answer; the caller reads the verdict).

### [DONE] Tests
Per-transport round trips (mock peers), close-reason propagation on all four, the Tcp
honest-degradation test, and the four-transport interchangeability witness (§1.9). My suite is
57 tests, all passing in-workspace (filter `transport::tests::` from the 244-test
`cargo test --lib` run):

```
test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
```

### 1.9 The interchangeability witness
`tests/interchange.rs`: one generic body over four factories, talking only to `Box<dyn Channel>`,
asserting send/recv of a real contract message, the *declared* capability outcome per transport,
the honest refusal of the rest, and close-reason propagation:

```
INTERCHANGE InProcess: one API shape, PASS
INTERCHANGE Shmem: one API shape, PASS
INTERCHANGE UnixSocket: one API shape, PASS
INTERCHANGE Tcp: one API shape, PASS
test transport::tests::interchange::four_transports_one_api_{inprocess,shmem,unix,tcp} ... ok
```

## 2. Per-transport capability-transfer semantics (the design centre)

Declared per instance by `TransportCaps`; asserted by tests; mirrors 02 §4.2.

| Transport | `capability_transfer` | What moves as authority | Framing | Declared gaps (`caps().note`) |
|---|---|---|---|---|
| InProcess | `Native` | the handle itself (`Box<dyn Any + Send>` moves) | direct dispatch, no bytes | no OS peer identity; no arena rides the channel (pass one as `Live`) |
| Shmem | `BootstrapOnly` | **nothing by itself**; the one-time bootstrap exchanges the region fd; handles live in the arena afterwards | bounded SPSC rings + slot arena; bulk by `T.BufferId` | no peer identity (paired with Unix); control ring copies (measured); no revocation channel — owner re-checks per call |
| UnixSocket | `OsHandle` (Linux) / `None` (other unix) | an fd (253/batch per 02 §0; 8/frame here), re-opened by the kernel; a passed memfd transfers a `BufferId` namespace | length-prefixed capnp over SOCK_STREAM | non-Linux: ancillary not compiled; receive is 2 declared copies; a passed fd cannot be un-passed |
| Tcp | `InBandToken` | **nothing**; descriptor bytes only, verified per call by the capability service; peer unknown ⇒ backend may only mint attenuated, time-boxed handles | length-prefixed capnp over TCP | no fd transfer; no peer identity; `DataHandle.buffer` has no namespace (producer must choose inline <1 KB or a declared copy); 3 copies |

## 3. The bootstrap protocol

**Problem** (02 §4.2): a raw shared arena is spoofable — on hosts where it has a name, any
process that can open that name can write frames and forge `BufferId`s.

**Who authenticates whom** — both sides, once, with the kernel as referee:
* the **owner** checks the peer's `SO_PEERCRED` (pid/uid/gid) against `PeerExpectation`
  *before* the offer;
* the **peer** authenticates the *region*: it receives the fd over SCM_RIGHTS (the kernel
  guarantees the same object), and an **anonymous memfd cannot be opened by name**, so holding
  the fd *is* exclusive knowledge of the arena.

```
Owner                              Peer
  | ---- Offer{version,epoch,config,root, fd} --> |  (SCM_RIGHTS)
  |        (peer checks identity + region)        |
  | <---------------- Accept{root, epoch} -------- |
  |        (both mark the shared state ESTABLISHED)
```
Over a raw `AF_UNIX` socket with `SCM_RIGHTS` (`UnixBootstrapLink`); a `MockLink` (in-memory
duplex, synthetic credentials) is the test double. The encoding is deliberately *not* capnp:
fixed little-endian, version-tagged, because it exists to establish the channel the ABI rides.

**What makes the arena safe afterward**: (1) fd-object identity; (2) seals
`F_SEAL_SHRINK|F_SEAL_GROW|F_SEAL_SEAL` (geometry fixed under the peer, seals locked, mapping
stays writable — the seal *probe* reports the enforced set); (3) the namespace root
(`epoch << 32`, unique per arena — out-of-range ids are refused with a typed error);
(4) the shared state word (`INERT` until both sides complete; `ArenaNotEstablished` before);
(5) per-call validity re-checks by the owner (no revocation channel exists on Shmem — data,
not authority).

Refusals are typed (`VersionMismatch`, `GeometryMismatch`, `EpochMismatch`, `UnexpectedPeer`,
`CorruptRegion`), and **a refused bootstrap is never a half-open arena**.

## 4. Copy-count measurements (the numbers for the report)

| Path | copies | Evidence |
|---|---|---|
| InProcess message send+recv | **0** | pointer-identity assertions (`owned_message_zero_copy_identity`, `borrowed_frame_dispatches_by_reference`) |
| Shmem publish (producer → slot) | 1 | `ArenaStats::publishes` |
| Shmem resolve (consumer → bytes) | **0** | `resolve_copy_bytes` has no increment site; `threaded_publish_resolve_zero_copies` asserts 0; `memfd_two_mappings_are_the_same_pages` proves shared pages |
| Shmem control ring (the referencing message) | 1 write + 1 read | `control_copy_bytes` (48 KiB payload → <1 KB on the ring) |
| UnixSocket send | 1 (frame assembly) | `write_scratch` reuse; declared in `caps().note` |
| UnixSocket recv | 2 (kernel, then capnp parse) | declared in `caps().note` |
| Tcp send | 1 | declared |
| Tcp recv | 2 (kernel, then capnp parse) | declared |

## 5. Calls the domain agents will feel

1. **`T.DataHandle.buffer` is refused on Tcp by the transport, enforced by the producer.**
   The Tcp channel declares `zero_copy_data_handle: false` and always answers
   `BufferNotResolvable`, but it does **not** walk every message to catch a violating producer
   (that would cost a full traversal per message and buy nothing the declaration does not). The
   domain layer must read `caps()` and choose inline-below-1 KB at call time
   (`types.capnp:276-286`, 02 §4.3 rule 1). Witness: `tcp::inline_data_handles_round_trip_and_buffer_arms_are_refused`.
2. **`Cap<T>` is deliberately not consumed here.** I defined no dependency on `src/core`
   (it landed concurrently): the seam carries `Box<dyn Any + Send>` (`CapHandoff::Live`) for
   in-process handles and in-band descriptor bytes for everything else. If core's `Cap<T>`
   (a capnp client, not `'static`-trivially-sendable) is to cross InProcess, it must be wrapped
   in a `Send` newtype by whoever builds the handoff. No file under `src/core` was touched.
3. **`TransportError` is transport-local** (like `io::Error`), with a total mapping to the
   contract's `T.ErrorCode` (`error_code()`, `write_t_error()`). Core owns the library's
   `Error`; my mapping is published so core can adopt or replace it without touching transports.
4. **`recv` is a bounded pull returning `TransportError::Empty`** on non-blocking transports
   (InProcess/Shmem) and blocking on socket transports (`caps().blocking` says which). "Nothing
   yet" is a real answer, never a fabricated message.
5. **`attach` takes `&mut CapHandoff`**: on refusal the value is untouched; on success the value
   moves into the channel (a same-kind placeholder remains in the caller's slot — documented).
6. **`Empty` is the only retryable claim** (`write_t_error` sets `retryable` accordingly).
7. **Close is `close(self: Box<Self>, reason)`** — consuming a channel requires boxing it first.
8. **Region handoffs carry `{fd, notice}`**: to give a peer a `BufferId` namespace over Unix
   you attach both the fd and the arena geometry (the bootstrap offer's shape). The far side
   adopts it and keeps it for the channel's lifetime.

## 6. Things the tests forced (worth knowing)

* **`F_SEAL_SEAL` is 0x0001, not 0x0010.** My first constants made 0x0010 (`F_SEAL_FUTURE_WRITE`)
  the "lock" bit, which blocks every *future writable mapping* — i.e. it broke the peer transfer
  itself. `memfd_two_mappings_are_the_same_pages` caught it mechanically. This is exactly the
  "verified, not claimed" discipline working.
* **`msghdr` is 56 bytes on this glibc**, verified against `/usr/include/bits/socket.h` (my first
  arithmetic said 48; the FFI struct was right, the assert was wrong).
* **The ancillary buffer must be 8-byte aligned** (`[u8; 48]` is alignment 1; a `*mut Cmsghdr`
  write through it would be UB in Rust even though the kernel accepts any alignment).
* **Frame length prefixes count kind + payload, never themselves.** My first writer counted the
  four length bytes too; the symptom was a receiver waiting for three phantom bytes (a hang the
  tests caught).
* **`mock` links must not block**: an empty mailbox answers `Empty`, and `run_bootstrap` waits
  for a verdict with a bounded spin (a verdict is mandatory in both directions).
* **Region mappings outlive their batch**: a `BufferId` namespace survives the `recv` that
  delivered the notice (the first version drained it away — caught by
  `region_notice_transfers_a_buffer_namespace`).
* **Segments on the wire are in words, not bytes**: `flat_message_len`/`write_flat_message`
  replicate capnp's stream format, verified byte-identical to `capnp::serialize::write_message`
  (`framing::writer_is_byte_identical_to_capnp`).

## 7. Blockers / requests

1. **None for wiring.** `build.rs` auto-wired `pub mod transport;` via
   `#[path]` when my `mod.rs` appeared (skeleton-owned file, no action needed).
2. **`Cargo.toml` (wire agent): the `std-transports` feature.** UnixSocket/Tcp currently gate on
   the existing `std` feature (which also widens capnp). When `std-transports` exists, the gates
   move from `feature = "std"` to `feature = "std-transports"` — two lines in `unix.rs`, `tcp.rs`,
   `mod.rs`, `channel.rs`.
3. **Core (Agent D)**: no blocker (no dependency). Optional convergence: `TransportError`'s
   `error_code()`/`write_t_error()` could become a `From<TransportError> for core::Error` if core
   wants one error vocabulary at the API surface.
4. **Verified here**: Linux/glibc FFI, SCM_RIGHTS, SO_PEERCRED, memfd+mmap+seals, framing
   byte-identity. **Not verified [U]**: macOS cmsghdr alignment variant, Windows AF_UNIX +
   SCM_RIGHTS — neither can be compiled on any target installed here, so neither is shipped.
