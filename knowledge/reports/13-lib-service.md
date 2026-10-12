# 13 — the service harness: the AI-first direct-call path, wired

Status date: 2026-10-12. Workspace: `/home/lufuno/smart/smart-lib-ng/`.
Files written (exclusive): `src/serve/{mod,frame,network,tests}.rs`, `examples/smart-service.rs`,
`tests/serve_wire.rs`, this report. One manifest line added and reported in §7: the
`[[example]]` section gating `smart-service` on its features. No other file outside my
ownership was created, edited or deleted. **No git commits.**

**Verdict: DONE and mechanically verified. The wire side of the direct-call path exists: a
`serve<B, C>` loop turns the Linux `NetworkBackend` into a capnp service over any `Channel`,
the daemon binds UnixSocket/Tcp and classifies each connection from its uid-peer credentials,
and 14 hermetic over-the-wire tests prove the whole surface — including all four transports
end-to-end. `cargo test -j 6 --features domain-network`: 314 lib + 22 codegen + 14 serve_wire +
5 smoke + 3 doc = **358 passed, 0 failed, 3 ignored** (the three ignored are the pre-existing
read-only live probes; one run's TCP-loopback flake in `src/transport/tests/tcp.rs` is
wave-3's documented pre-existing race, §6). Warning-clean for every file I own.**

---

## 1. The wire format, as built

The dispatch envelope lives in `src/serve/frame.rs`: fixed-offset **little-endian**, zero
dependencies, and domain-blind — the only domain-specific number in a frame is the `interfaceId`,
which is the contract's own compiled schema id. A frame rides the channel as the single segment
of a one-segment capnp message, word-padded at the tail (the channel's segment law needs the
alignment; the frame's own header lengths are exact and the padding is zero filler the reader
never looks at). No envelope schema was invented — a schema for the envelope would be exactly
the second wire vocabulary this layer refuses.

```
request  : [0..8] interfaceId u64 LE | [8..10] methodOrdinal u16 LE | [10..12] paramsLen u16 LE
           [12..16] flags u32 LE (reserved 0, non-zero is refused) | params bytes | cap bytes
response : [0..2] status u16 LE (0 ok / 1 error) | [2..4] msgLen u16 LE | [4..8] payloadLen u32 LE
           message bytes (UTF-8) | payload bytes
pulls    : SUB_PULL = 0xFFFE, params [subId u64][limit u32]; SUB_CANCEL = 0xFFFF, params [subId u64]
```

The params bytes are the capnp serialisation of the contract's own `<method>Params` struct; the
payload is the contract's own `<method>Results` struct (or, for a subscription, the
service-minted subId followed by the contract's event struct; or, for an error, the serialized
`T.Error`). Error messages carry the failure's own vocabulary prefix: the matrix row's
`reasonClass` for unavailable, `permissionDenied` for a class refusal, `bad-frame:` /
`unknown-interface:` for envelope faults.

## 2. The capability policy (05 §2), exactly as specified

`classify(peer)` in `src/serve/mod.rs` — uid 0 **or** `CapEff` holding `CAP_NET_ADMIN` (bit 12,
read from `/proc/<pid>/status`; a read, law #8) ⇒ `Control`; any other uid ⇒ `ReadOnly`; **no
peer identity (Shmem, Tcp, InProcess) ⇒ `ReadOnly`** — never a fabricated grant. No polkit, no
elevation, no suid. Every `/proc` read failure is the weaker class. In a `no_std` build the
`/proc` half is compiled out and the policy answers with the uid rule alone — declared, not
hidden (the no_std branches were compile-checked, §5).

The class is the authority gate (law #5): the five mutating verbs (`setNetworking @72`,
`setRadioEnabled @74`, `setMobileData @11`, `setInterfaceMtu @76`, `forgetWifiNetwork @77`) are
refused **before the backend is reached**, and the witness is mechanical — the mock's call log
is asserted empty of those verbs (`read_only_class_refuses_the_mutating_verbs_before_the_backend`).

## 3. Dispatch glue (`src/serve/network.rs`)

One monomorphized match over the contract's own ordinals. Per arm: decode the contract's params
struct from the frame bytes, call the backend, encode the contract's result struct. The only
allocation a call makes is the response frame itself.

* **The 19 servable methods** all round-trip (§4). `getInterfaces` writes `name`/`type`/
  `hwAddress`/`up`/`connected`/`mtU`; two contract facts are honoured rather than papered over:
  the contract's `getInterfaces` result carries **no `total`**, and `NetworkInterface` has **no
  `managed` field**, so the domain's `Page.total` / `InterfaceSnapshot.managed` have no wire home
  here and are not invented.
* **Availability is data, not code.** Every ordinal outside the servable set reads
  `NETWORK_AVAILABILITY` by ordinal and answers `notSupported` with the row's own `reasonClass` +
  `reason` (proven for one row of each class: `no-privilege`, `no-backend`, `not-applicable`).
  An ordinal no row covers is a caller error → `invalidArgument`.
* **Subscriptions.** `subscribeStatus @2` / `subscribeCounters @37` mint a server-side token (the
  backend's own `SubscriptionId`) and answer with the **first event included**, mirroring
  `Network::status_stream`; the service remembers which family each minted id belongs to because
  the id is the in-band handle. `SUB_PULL` reads the backend's `pull_*`; a pull the backend answers
  `temporary` on is "nothing yet" → **ok with an empty payload** (never a fabricated event).
  `SUB_CANCEL` retires the token. A token this connection never minted is `notFound` before the
  backend is reached.
* **Error totality.** A backend error maps to the frame's error channel with the code preserved
  exactly in the payload's `T.Error` and the honest `retryable` flag (05 §3's promise, now
  exercised end-to-end).

## 4. Test evidence

```
$ cargo test -j 6 --features domain-network
running 317 tests (lib)          → 314 passed, 0 failed, 3 ignored   (unchanged baseline + probes)
running 22 tests  (codegen)      → 22 passed
running 14 tests  (serve_wire)   → 14 passed      ← this wave's target
running 5 tests   (smoke)        → 5 passed
running 3 doc-tests              → 3 passed
```

The 14 wire tests, all hermetic (mock backend, no host state beyond `/proc/self/status` reads and
a temp-dir socket):

| test | proves |
|---|---|
| `frame_codecs_round_trip_and_refuse_malformed_spellings` | the frame codecs; every malformed spelling refused with a typed reason |
| `every_servable_read_round_trips_over_an_in_process_channel` | all 10 read methods over an InProcess pair; payloads decode to the expected contract results (incl. the contract's `limit == 0 ⇒ one` pagination law) |
| `every_servable_mutating_verb_round_trips_on_a_control_connection` | all 5 mutating verbs reach the backend — the mock's log names each one |
| `read_only_class_refuses_the_mutating_verbs_before_the_backend` | `permissionDenied` with the contract kind in the payload; the backend log contains **only** the read |
| `unavailable_methods_answer_the_matrix_row_over_the_wire` | one row per reasonClass; message begins with the row's token and repeats the row's reason; code `notSupported` |
| `unknown_interface_and_malformed_frames_are_typed_answers` | `unknown-interface` / `bad-frame`, and the loop survives every fault |
| `subscriptions_round_trip_over_the_wire` | subscribe→first event, bounded pull, nothing-yet empty payload, cancel, `notFound` after cancel, counters subscription, exact per-verb call counts |
| `backend_errors_propagate_over_the_wire_with_their_kind` | a scripted `temporary` survives the seam with code, message and `retryable` |
| `unix_socket_end_to_end_serves_a_client_session` | the daemon-shaped loop (listener + accept in a thread): read + subscription + denial over a real socket |
| `tcp_end_to_end_serves_a_client_session_at_the_honest_class` | TCP loopback: no peer identity ⇒ ReadOnly, read ok, mutating verb refused |
| `shmem_end_to_end_serves_a_client_session` | the arena-constructed pair: status + radios over the zero-copy transport |
| `the_handshake_matches_the_contract_and_refuses_a_foreign_abi` | matched names the network schema id; a foreign namespace gets `incompatible` with the mismatch named |
| `classify_maps_peer_identity_into_the_capability_class` | None/uid-0/unreadable-`/proc`, plus the pure `CapEff` parser (bit 12 set / not set / malformed / absent) |
| `classify_reads_this_process_own_capabilities` | the whole `/proc` path against this process's own kernel-reported capabilities |

**All four transports are proven end-to-end** for the harness: InProcess (the method matrix),
UnixSocket, Tcp and Shmem (one full session each) — the envelope never names a transport, which
is the interchangeability law holding at the service layer.

## 5. Build discipline evidence

```
$ cargo build -j 6 --all-targets --features domain-network      # no warning originates in my files
$ cargo build -j 6 --no-default-features                         # green
$ cargo build -j 6 --no-default-features --features std,domain-files      # green
$ cargo test  -j 6 --no-default-features --features std,domain-files --no-run
```

The two remaining warnings in the tree (`codegen/modules.rs::with_children`,
`codegen/inventory.rs:834` unused `mut`) are the skeleton agent's pre-existing ones. The no_std
fallback branches of `serve` (`spin_loop` wait; the uid-only `classify`) were compile-checked by
temporarily cfg-ing the std halves out — both branches typecheck, then reverted. The daemon
binary was run exactly once with `--transport bogus`, which exits with its typed message before
any bind and touches no host state; its live run (bind, accept, serve) is founder-run per law #8,
and the daemon-shaped loop itself is what `unix_socket_end_to_end…` proves with a mock.

## 6. Observed flake, NOT mine

One run of the full suite failed two tests in `src/transport/tests/tcp.rs`
(`honest_degradation_is_declared_and_mechanical`, `negotiate_hook_round_trips_a_handshake`):
TCP-loopback connect-refused / accept-on-non-socket races in the wave-3 transport agent's files,
the same flake class report 12 §3 recorded. It did not reproduce in the four subsequent full runs
(all green), nor in three targeted runs of that module. I touched nothing under `src/transport/**`.

## 7. Reported needs and deviations (files I do not own)

1. **`build.rs` — one line, requested (the orchestrator dispatches the skeleton agent).**
   `wave_modules`'s specs array declares `core`, `transport`, `platforms`, `conformance`; the
   harness needs the same treatment:
   ```rust
   ( "serve", &["Placeholder: `src/serve/**` is the service-harness wave's file.",
                "Write `src/serve/mod.rs` and it becomes this module automatically, exactly",
                "like core/transport: the serve harness turns a domain backend into a capnp",
                "service over any Channel (knowledge/05 §3)."] ),
   ```
   Until it lands, `src/serve/**` is compiled the same way build.rs's generated tree compiles
   every wave module — a `#[path]` declaration — from `tests/serve_wire.rs` (with
   `extern crate alloc` and the four crate-root re-exports the harness speaks) and from
   `examples/smart-service.rs`. Both shims are documented in place and collapse to
   `use smart_lib::serve::…` the moment the line lands; no behaviour depends on them.
2. **`serve`'s channel parameter is borrowed (`&C`, `C: Channel + ?Sized`) rather than by value**
   as the brief's sketch spelled it. The transport seam hands out `Box<dyn Channel>`, which
   cannot be moved into a by-value generic parameter (size not known), and the loop needs no
   ownership — `recv`/`send` borrow. Consequence: the caller owns the close. The daemon does
   exactly that per session (`serve::orderly_close()`); a handshake-incompatible session returns
   from `serve` and the caller closes it with the typed reason.
3. **`Cargo.toml` — added, and reported here (the brief permits it):**
   ```toml
   [[example]]
   name = "smart-service"
   required-features = ["domain-network", "std"]
   ```
   Without it the example's `#![cfg]` gate strips `main` and every non-default feature set fails
   to compile the example target. **The pre-existing `examples/network_probe.rs` has the same gap**
   (its `#![cfg(all(feature = "domain-network", feature = "std"))]` has no `required-features`
   line), so `cargo test --no-default-features` fails on it in this tree today — the network
   domain agent (or the skeleton agent) should add the same two lines for it.
4. **Capability drops stay the backend surface's own record** (as the brief directs: "Drops
   counted by the backend surface as they already are"). The harness adds no second drop counter
   — that would be a second wire vocabulary — and l3 proves the backend's counting
   (`subscription_pulls_count_the_updates_they_collapse`). If the wire must *carry* the count per
   pull, the trait needs one accessor (`fn dropped_updates(&self) -> u64` on the backend); I did
   not add it because `NetworkBackend` is not my file.

## 8. Decisions the founder may want to rule on

1. **The contract's pull ordinals answer `invalidArgument` naming `SUB_PULL`.** `pullStatus @3` /
   `pullCounters @38` take a **capability** param, and a capability cannot travel as frame bytes —
   that is the transport layer's declared `CapabilityTransfer`, not a gap in the glue. The wire
   therefore routes pulls through `SUB_PULL` (0xFFFE) with the service-minted id, and the
   contract ordinals answer `invalidArgument: pullStatus @3 takes a subscription capability …
   pulls ride SUB_PULL` rather than pretending the capability crossed. The alternative — encoding
   a bare `subId` into the contract's own pull ordinals — would be a second wire vocabulary for
   the same ordinal, which the doctrine forbids.
2. **`getInterfaces`' stable `total` is not on the wire.** The contract's result carries only the
   list; the domain's `Page.total` has no field. A caller wanting the count pages until the list
   is short. Same for `InterfaceSnapshot.managed` (no contract field).
3. **The handshake's schema name is the literal `"smart-protocol/network"`** in the generic
   harness's `matched` arm (the id comes from `NetworkService::schema_id()`). A second domain's
   glue changes that one string; `ServiceDispatch` carries only the id, per the brief's trait
   shape.

## 9. Blockers

* **None for this wave's build.** The single blocking item for the *library API* is §7.1: until
  `build.rs` declares the `serve` wave module, `smart_lib::serve` does not exist as a public path
  (the shims make the code compile and its tests run, but Wave 3's CLI should link the real path).

---

## 10. Post-wiring fix (2026-10-12, after the orchestrator wired `serve` into `build.rs`)

With `serve` a real crate module, `cargo build --features domain-network` failed with four
errors in `src/serve/mod.rs` — and they were all **imports, not logic**, exactly as diagnosed.

**What was wrong.** The crate root is `#![no_std]` + `extern crate alloc` (law #1), and my
`frame.rs`/`network.rs` had done it correctly (`use alloc::format;` …). `mod.rs` reached for
`format!` (line 151) and `std::fs` / `std::thread` (lines 152, 375) without either: in a
`no_std` crate the `std` paths are not in scope **even inside `#[cfg(feature = "std")]`
blocks** — the link has to be made explicitly, which is the house pattern
`src/transport/sys.rs` already uses (`#[cfg(feature = "std")] extern crate std;`). The four
errors were invisible before the wiring because `tests/serve_wire.rs` compiled the same source
inside a **std** test crate whose crate root already declared `extern crate std` — the shim was
masking the defect.

**What I changed (only my files).**

1. `src/serve/mod.rs` — added `use alloc::format;` and
   `#[cfg(feature = "std")] extern crate std;` (with the `sys.rs`-style comment explaining why
   the link is made here), and made the one fully-qualified `alloc::format!` call plain
   `format!` for consistency. The two std-only halves — the `/proc/<pid>/status` capability
   read and the non-blocking yield — stay behind their existing honest gates; with `std` off
   they are not compiled and the module answers with the `alloc`-only rules it declares.
2. `src/serve/tests.rs` was **deleted** and its 14 wire tests moved into `tests/serve_wire.rs`,
   now driven through the harness's **public** API (`smart_lib::serve::{serve, classify, frame,
   CapabilityClass, orderly_close, cap_mask_from_status}`) instead of a `#[path]` copy of the
   source. Every symbol the tests touch is public, so what they prove is what a consumer gets;
   the file is `#![cfg(feature = "domain-network")]`-gated so it compiles to nothing without
   the domain. The distribution is unchanged from the pre-wiring state (14 in `serve_wire`),
   and the source is no longer compiled three times over.
3. `examples/smart-service.rs` — dropped its `#[path]` shim and crate-root re-exports; the
   daemon now simply `use smart_lib::serve::{classify, serve, orderly_close}`. It is the first
   real consumer of the public path.

**Verification (three consecutive runs of each).**

```
$ cargo test -j 6 --features domain-network
running 317 tests (lib)      → 314 passed, 0 failed, 3 ignored     (the 3 pre-existing read-only probes)
running 22 tests  (codegen)  → 22 passed
running 14 tests  (serve_wire)→ 14 passed
running 5 tests   (smoke)    → 5 passed
running 3 doc-tests          → 3 passed
                                       total 358 passed, 0 failed

$ cargo test -j 6 --no-default-features
running 226 tests (lib)      → 225 passed, 0 failed, 0 ignored
running 22 tests  (codegen)  → 22 passed
running 0 tests   (serve_wire)→ 0 passed   (feature-gated: nothing to serve without domain-network)
running 4 tests   (smoke)    → 4 passed
running 3 doc-tests          → 3 passed
                                       total 254 passed, 0 failed

$ cargo build -j 6 --no-default-features --features domain-network   # no std, serve compiled
                                       → clean (the harness itself is genuinely no_std + alloc)
$ cargo build -j 6 --all-targets --features domain-network
                                       → zero warnings in src/serve/**, tests/serve_wire.rs,
                                         examples/smart-service.rs (the two remaining warnings
                                         are the skeleton agent's pre-existing codegen ones)
```

**The bare-metal target does not survive, and the blocker is not mine.**
`cargo build --no-default-features --target x86_64-unknown-none` fails inside the third-party
dependency graph — `log` (104 errors) and `bytes` — pulled in by the workspace's
**unconditional** `netlink-packet-core` / `netlink-packet-route` / `netlink-sys` dependencies.
The failure list contains zero `src/serve` files and reproduces with *every* feature off, so it
predates this wave and cannot be fixed from my files. What I can state mechanically is that the
serve harness **is** bare-metal-capable in source: with `std` off and `domain-network` on it
compiles clean for the host target (`cargo build --no-default-features --features
domain-network`), its only std reaches are the two honestly-gated halves named above, and the
daemon parts that genuinely need a host socket (`examples/smart-service.rs`, the UnixSocket/Tcp
listeners in `src/transport/**`) are already gated on `std` — nothing of mine is left in a
broken configuration. If the workspace wants a true bare-metal build, the netlink line needs to
become an optional dependency behind the Linux platform feature (a skeleton-agency change,
reported).

**One observed flake, not mine.** During the six verification runs the lib target twice failed
a single test, both times in the wave-3 transport agent's files:
`transport::tests::tcp::honest_degradation_is_declared_and_mechanical` (twice) and
`transport::tests::interchange::four_transports_one_api_tcp` (once) — the TCP-loopback
connect/accept race report 12 §3 and report 13 §6 already record. It reproduces with
`--lib` alone (independent of my targets: 5–7 green of 8 runs), and I touched nothing under
`src/transport/**`.
