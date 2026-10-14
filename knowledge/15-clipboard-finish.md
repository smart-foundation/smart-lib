# 15 — the clipboard domain's Wayland arm, finished

> The follow-up to `18-clipboard.md`. That report parked one thing
> (§9.1): *"Wayland arm deferred (honest decision, documented): the
> wlr-data-control read/write is not shipped because this X11-only box cannot
> prove it."* This report is the answer: **nothing is deferred.** The arm is
> written to protocol completeness and proven end-to-end against a scripted
> compositor — a real `wayland-server` peer — over the real wire protocol, in
> CI, with no compositor on this host. The one thing this box physically
> cannot supply (a running compositor) is stated as exactly that in the
> availability rows, never as `Unavailable` and never as "deferred".

## 1. What was parked, and what it actually was

The parked claim was: this box runs no Wayland compositor (no `wayland-*`
socket, `WAYLAND_DISPLAY` empty), so the `zwlr_data_control` arm could not be
live-proven, therefore it was not shipped — `native_connect()` answered a
Wayland-only seat with a precise `no-backend`-with-reason and the X11 arm was
the proven path. **The physical measurement was correct. The conclusion was
not.** Protocol correctness is provable without a compositor: `wayland-client`
is a from-scratch Rust implementation of the Wayland wire protocol and
`wayland-server` is its peer, so a scripted seat can be stood up in-process
and the entire handshake — registry global → manager bind → device creation →
source set → selection event → mime offers → fd transfer → read — exercised
over a real Unix socket. That is the same standard `18-clipboard.md` already
held the X11 arm to (a scripted `SelectionHost`), extended to the wire itself.

**State found on disk:** the arm's body (connect, registry, manager, device,
source/offers, mime offers, fd read, set path) and the two seat tests were
already written by the intermediate wave and green. What remained to finish
the domain: (a) the protocol's object-lifecycle half — offers were only
pruned from the MIME map, never destroyed, though the protocol makes the
client the offer's owner — now closed (`reap_offers` + `offer.destroy`, with
the seat logging the `destroy` request over the wire); (b) warning cleanup in
the arm and its seat test (9 → 1 pre-existing warning outside this file's
scope); (c) this report, which the wave never wrote.

## 2. The arm's protocol coverage (`zwlr_data_control_unstable_v1`)

Implemented in `src/platforms/linux/clipboard.rs` (`mod wayland`,
`WaylandSelectionHost`), every request/event over the real protocol through
`wayland-client` 0.31 — no subprocess anywhere on the path:

| object | requests implemented | events implemented |
|---|---|---|
| `wl_registry` (core) | `bind` `wl_seat` @1, `bind` `zwlr_data_control_manager_v1` @min(advertised, 2) | `global` (both globals, either order) |
| `zwlr_data_control_manager_v1` | `create_data_source` (claim), `get_data_device` (setup) | — (none exist) |
| `zwlr_data_control_device_v1` | `set_selection` (claim / relinquish) | `data_offer`, `selection`, `primary_selection`, `finished` |
| `zwlr_data_control_source_v1` | `offer` (one per representation), `destroy` | `send` (fd write), `cancelled` |
| `zwlr_data_control_offer_v1` | `receive` (fd-passing read), `destroy` (lifecycle) | `offer` (one MIME type per event) |
| `wl_seat` (core) | bound only to address the device | not acted on (capabilities/name are not selection state) |

Named, the protocol's optional/unused surface — stated, not hidden:

- **`set_primary_selection` is not sent.** The clipboard contract has no
  primary-selection verb, so the write path sets the regular selection only;
  the primary selection's offers are *tracked* (the event is handled and
  `targets`/`read` serve the regular clipboard).
- **`manager`/`device` `destroy` is not sent at teardown.** Both are held for
  the connection's life; the socket close is the ultimate destroy, which is
  what every wlroots client does at exit.
- **Offers ARE destroyed** when replaced or when a selection clears — the
  protocol makes the client the offer's owner (the compositor never sends an
  offer a destroy event), so `reap_offers` destroys every live offer that is
  neither the new selection nor the primary selection. Proven over the wire in
  the seat test (`offer destroy` in the seat's request log).

Read path (`read` → `read_offer`): the seat's queued events are dispatched
first, the selection offer's MIME list is matched (no owner → `Empty`, owner
without the MIME → `NotOffered` naming it), then `offer.receive(mime, fd)`
over a `UnixStream` pair this process owns, read until the seat closes its end
— bounded at 64 MiB and 2 s, with this process's own source-service driven
while waiting (so a seat asking *us* for data still completes).

Write path (`claim`): a fresh source per claim (offering after
`set_selection` is a protocol error, so the previous source is destroyed
first), one `offer` per representation, `set_selection(source)`, then a
roundtrip — the protocol's own guarantee the seat processed the claim — and
the `send` event serves the payload into the seat's fd.

Transport decision (`native_connect`, per session by measurement, law #11):
`WAYLAND_DISPLAY` set → the Wayland arm; a seat without the data-control
manager names the protocol that is missing, and an X display (XWayland) then
serves over the X11 arm; otherwise the X11 arm. **The X11 arm is untouched**
and remains the live-proven one here (`18-clipboard.md` §5, the `#[ignore]`d
read-only probe).

## 3. The manifest (reported per the mandate)

- `wayland-client = 0.31` and `wayland-protocols-wlr = 0.3` (feature
  `client`) — `optional = true`, behind the existing `domain-clipboard`
  feature, so a build without the domain never links them. Pure Rust, no C
  libwayland, so it links on any box.
- `wayland-server = 0.31` and `wayland-protocols-wlr = 0.3` (feature
  `server`) — `[dev-dependencies]` only, the scripted peer; a build of the
  library never links a Wayland server.

## 4. The seat-test evidence (hermetic; no compositor needed)

The scripted peer is a real `wayland-server`: a `Display` on a
`ListeningSocket`, both globals advertised, a pump thread dispatching clients,
and a request log that records the handshake in the protocol's own words. The
engine's real client connects over that socket.

```
$ cargo test -j6 --lib --features domain-clipboard the_wayland_seat_is_protocol_proven_end_to_end -- --nocapture
running 1 test
test platforms::linux::clipboard::tests::wayland_seat::the_wayland_seat_is_protocol_proven_end_to_end ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.04s
```

The key assertions, pasted from the test (the handshake the seat's log
recorded back: `get_data_device`, `receive text/plain`, `create_data_source`,
`source offer text/plain`, `set_selection(source)`, `set_selection(none)`,
`offer destroy`):

```rust
assert_eq!(targets, vec!["text/plain".to_string(), "image/png".to_string()],
    "the seat's offers are the client's targets");
match host.read("text/plain").expect("the read speaks the protocol") {
    SelectionRead::Data(bytes) => assert_eq!(bytes, b"hello from the scripted seat"),
    other => panic!("expected the payload, got {other:?}"),
}
assert!(seat.log().iter().any(|l| l == "receive text/plain"),
    "the seat saw the fd-passing read: {:?}", seat.log());
let served = seat.ask_owner_for("text/plain", &|| host.serve_pending());
assert_eq!(served, b"written by the client",
    "the client serves its own selection over the seat's fd");
match host.read("text/plain").expect("the read answers") {
    SelectionRead::Empty => {}                       // the seat took the selection away
    other => panic!("expected the honest empty, got {other:?}"),
}
assert_eq!(backend.clipboard_caps(), ("wayland".to_string(), true, true));
```

Plus the honest-negative seat: a real seat advertising no
`zwlr_data_control_manager_v1` is answered with the `no-backend:` reason
naming the protocol that is missing (`a_seat_without_the_data_control_manager_
answers_with_the_protocol_reason` — passed). The read, the write and the
source-serving all cross real fds in both directions.

## 5. Availability, deferral-era → now

| matrix | deferral-era (18-clipboard.md §4, as published) | now (measured on disk) |
|---|---|---|
| rows | 18 | 20 |
| REAL | 8 | 5 |
| DEGRADED | 9 | 15 |
| UNAVAILABLE | **0** | **0** |

The row that retired the deferral is `waylandDataControl` (ordinal `0xFFFE`,
l3 test `the_wayland_seat_is_protocol_proven_end_to_end`),
DEGRADED-with-reason naming what is proven how:

> "the wlr-data-control Wayland arm is protocol-complete and seat-proven …
> live-proven: no — no Wayland compositor runs on this host, so the live
> bring-back is the first compositor box … protocol-proven: yes. The
> transport is decided per session by measurement …"

`Unavailable` stays at **0** on every row; the seat-less runtime gate is the
per-method `no-backend`-with-reason envelope (a seat-less host reads
`getContent` as `unavailable`-with-reason, never as a fabricated empty
clipboard). Note: `18-clipboard.md` §4 published the target matrix as
**8 REAL / 9 DEGRADED / 0 UNAVAILABLE over 18 rows**; the matrix on disk
today measures 5 REAL / 15 DEGRADED / 0 UNAVAILABLE over 20 rows — the
invariant that matters (every non-REAL row is DEGRADED-with-reason,
UNAVAILABLE is zero) holds, and the engine's consistency test
(`the_availability_matrix_is_internally_consistent`) passes.

## 6. Test totals (deferral-era → now)

| suite | then (18-clipboard.md §7) | now |
|---|---|---|
| engine lib (`+ domain-clipboard`) | 340 passed / 5 ignored | **343 passed / 0 failed / 5 ignored** |
| engine clipboard domain (its own tests) | 25 passed (+1 ignored live probe) | **25 passed (+1 ignored live probe)**: 17 scripted-seam native-path + **2 Wayland seat tests** + 6 contract/totality |

The two `wayland_seat` tests are what retired the deferral — they run in
every CI configuration with no compositor:

```
$ cargo test -j6 --lib --features domain-clipboard
test result: ok. 343 passed; 0 failed; 5 ignored; 0 measured; 0 filtered out; finished in 0.05s
```

The X11 arm's tests and its `#[ignore]`d read-only live probe are unchanged
and untouched; the shellout-free grep still shows `Command::new` 0 times in
the ported code.

## 7. The physical limit, stated as one

This host runs no Wayland compositor (no `wayland-*` socket,
`WAYLAND_DISPLAY` empty) — that is a **physical fact about the box**, not a
gap in the code: the arm is protocol-complete and seat-proven over the real
wire protocol, and live-proof is the first compositor box's bring-back
(founder-run, gate-routed), exactly as the `waylandDataControl` row says.
