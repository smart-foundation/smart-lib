# 12 — transport correction: the network domain reads the kernel first (law #9)

Status date: 2026-10-12. Workspace: `/home/lufuno/smart/smart-lib-ng/`.
Files written (exclusive): `src/platforms/linux/netlink.rs`, `src/platforms/linux/network.rs`,
`src/domains/network/{types,availability,tests}.rs`, this report. No file outside my ownership
was created, edited or deleted (Cargo.toml's netlink lines were already in place; nothing outside
my files was touched). **No git commits.**

**Verdict: the P6 transport correction is DONE and mechanically verified. The read path is
kernel-first — netlink events feed a local state store, and `getStatus`/`getInterfaces`/
`pull_*`/`getCounters` are served from it with no daemon hop. `NmClient` shrank from 11 methods
to 6 (one policy sweep + three control verbs + two kernel-read fallbacks), and the D-Bus path is
a counted fallback: a warm store issues ZERO NmClient calls, a cold store exactly one per read
op. `cargo test -j 6 --features domain-network`: **298 passed, 0 failed, 2 ignored** (both live
probes, read-only). Warning-clean for my files. Both `#[ignore]`d read-only live probes PASS on
this host. Two real kernel-path bugs were found and fixed on the way — one of them only the live
probe could catch (see §6).**

---

## 1. What the checkpoint (`1f255ee`) had vs what I completed

| Item | At `1f255ee` (checkpoint) | After this pass |
|---|---|---|
| NetlinkReader seam | 2 methods (`snapshot`, blocking `next_event`); wire parsing tested | 3 methods (+ `drain_events`, the non-blocking form the store calls per read) |
| `MockNetlink` | written, **never constructed** (dead-code warning) | constructed everywhere in the read-path tests; shareable via `Arc<MockNetlink>` so a test can queue events after the store took the reader |
| State store | **absent** | `NetworkStore`: link/address/route/rfkill table, event-fed, revision-counted, topology-counted |
| Read path | every read went to `NmClient` (D-Bus) | served from the store; NM only for policy the kernel does not own, counted |
| `NmClient` | 11 methods incl. per-read `networking_enabled`/`wireless_enabled`/`wwan_enabled`/`devices` | 6 methods: `policy()`, `counters()`, `rfkill()`, `set_networking_enabled`, `set_wireless_enabled`, `set_wwan_enabled` |
| Unused `Mutex` import | the vestige of the unwired store | used: the store's table, the policy cache and the live-subscription map |
| `from_nm_device_type` / `LinkProvenance::NmFallback` | dead code (the unwired fallback) | wired: the link table is synthesised from NM's device view when the kernel cannot answer, marked `NmFallback` |
| `HostStateSource` | type added, unused | carried on `ConnStatus.host_state_source`; the four-state diagnostic asserts each state's source |
| Availability table | 12 REAL / 3 DEGRADED / 62 UNAVAILABLE, old `l3_test` ids | same counts (see §5), ids regenerated from the implementation |
| Tests | 282 lib tests | **298 lib tests** (+16), 2 ignored live probes |
| Live proof | netlink probe not passing (never run) | both read-only probes pass on this host |

## 2. Per-item status

**2.1 [DONE] Wire the store.** `NetworkStore` (`src/platforms/linux/network.rs:828`) holds the
kernel's links/addresses/routes/rfkill behind one lock, fed by
[`NetlinkReader::drain_events`](src/platforms/linux/netlink.rs) and primed once from a full dump.
A read calls `ensure_kernel_state()` → `refresh()` (bounded, non-blocking drain + prime) and then
serves from memory. There is no thread and no timer: the kernel pushes, the read applies. The
store also bumps `topology_revision` when the link set, a link's administrative state, or the
address set changes — that is what invalidates the cached NM policy sweep, so policy re-sweeps
exactly once per topology change and never on a timer.
Evidence: `kernel_events_feed_the_store` (event applied, revision rises, `LinkGone` →
`notFound`, no daemon call), `get_status_four_states_distinct_with_explicit_sources`,
`get_interfaces_enumerates_the_kernel_link_table`, `warm_store_reads_issue_zero_nm_calls`.

**2.2 [DONE] Shrink `NmClient` to policy + control.** The trait is now
`policy() -> NmPolicy` (ONE sweep: the manager policy bits + NM's device view),
`counters()`/`rfkill()` (kernel reads behind the seam, used only when netlink cannot answer) and
the three control verbs. `RealNmClient::policy()` implements the sweep over the existing D-Bus
helpers (5 × `Properties.Get` + `GetDevices`): several round trips on the wire, but **one seam
call**, which is the unit the fallback is counted in. Reads fall back to NM only for NM-policy
state (the master switch, radio policy, per-device `Managed`, and the whole link table when
netlink is dead), and every fallback is counted by `fallback_calls` and surfaced via
`nm_fallback_calls()`.
Evidence: `cold_store_reads_issue_exactly_one_counted_fallback` (table in §4),
`netlink_unavailable_falls_back_to_the_nm_device_view` (fault-injected netlink ⇒ link table
synthesised from NM's device view, `LinkProvenance::NmFallback`, exactly one counted call, warm
reads then zero, `kernel_error()` reports why the kernel could not answer).

**2.3 [DONE] Construct and use `MockNetlink`.** Every read-path test now builds a scripted
kernel; `MockNetlink` is also constructible behind an `Arc` so a subscription test can queue
events after the store took the reader (a subscription that spans kernel events).
Evidence: 32 tests in `platforms::linux::{network,netlink}::tests`, all hermetic (no socket, no
host state).

**2.4 [DONE] The acceptance assertions.**
*Fallback recovery:* `a_returning_kernel_replaces_the_fallback_link_table` — with netlink
faulted, NM's device view becomes the link table; the moment the kernel answers again the
synthesised table is replaced wholesale (never a mixed table).
*Counting mock on the NmClient seam:*
`warm_store_reads_issue_zero_nm_calls` (getStatus, getInterfaces, pullStatus, pullCounters,
isNetworkingEnabled, listRadioStates, isMobileDataEnabled on a warm store ⇒ zero further
`NmClient` calls) and `cold_store_reads_issue_exactly_one_counted_fallback` (per-op cold/warm
table, printed with `--nocapture`, asserted).
*Four-state diagnostic with explicit sources:* `get_status_four_states_distinct_with_explicit_sources`
asserts all four tokens AND their sources: `(NetworkingDisabled, NmPolicy)`, `(RadioOff, NmPolicy)`,
`(NoFirmware, Kernel)`, `(CarrierDown, Kernel)`; `rfkill_blocked_radio_is_radio_off_from_the_kernel`
adds a kernel-sourced `radio-off` (rfkill) and proves a blocked radio is never read as
`no-firmware`.
*Subscriptions serve bounded batches from the store, drops counted:*
`status_subscription_bounded_pull` (caller-bounded pulls, zero drops when nothing changed,
cancel ⇒ `notFound`) and `subscription_pulls_count_the_updates_they_collapse` (three kernel
events land between pulls ⇒ the pull serves the LATEST fact and `dropped_updates() == 2`, never
silent).

**2.5 [DONE] Availability table regenerated from the implementation** — §5.

## 3. Test-output evidence

```
$ cargo test -j 6 --features domain-network
running 300 tests
test result: ok. 298 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.04s
running 22 tests   (tests/codegen.rs)
test result: ok. 22 passed; 0 failed; 0 ignored
running 5 tests    (tests/smoke.rs)
test result: ok. 5 passed; 0 failed; 0 ignored
running 3 tests    (doc-tests)
test result: ok. 3 passed; 0 failed; 0 ignored

$ cargo build -j 6 --all-targets --features domain-network   # no warning originates in my files
$ cargo build -j 6 --lib --no-default-features --features "std,domain-files"   # feature law intact
```

Per-module (lib): `platforms::linux::network::tests` 33 tests (32 passed + 1 ignored live
probe), `platforms::linux::netlink::tests` 16 tests (15 passed + 1 ignored live probe),
`domains::network::tests` 7 passed, `platforms::*` together 47 tests (45 passed + 2 ignored live
probes). Baseline was 282; now **298 (+16)**.

**Observed flake, NOT mine (wave-3 transport, pre-existing):** `transport::tests::tcp::
inline_data_handles_round_trip_and_buffer_arms_are_refused` failed 1-in-6 runs during this pass
(assert at `src/transport/tests/tcp.rs:142`), and one baseline run before my changes aborted the
test process with `IO Safety violation: owned file descriptor already closed` in the same
module. Both are TCP-loopback races in `src/transport/**`, which I do not own and did not touch.
My modules' tests are deterministic across the 10+ runs above.

The four-state diagnostic (`--nocapture` not needed; it is a plain assertion):

```
assert states == [
  (NetworkingDisabled, NmPolicy),
  (RadioOff,          NmPolicy),
  (NoFirmware,        Kernel),
  (CarrierDown,       Kernel),
]  ✓ 4 distinct states, 2 kernel-sourced, 2 policy-sourced
```

The read-only live probes (`cargo test -- --ignored`, reads only — netlink dumps + NM
properties; never a host switch):

```
live dump: 3 links ["lo", "enp0s31f6", "wlp0s20f3"], 4 addresses, 11 routes
live: netlink links = ["enp0s31f6(ethernet)", "lo(loopback)", "wlp0s20f3(wifi)"], NM seam calls = 1, store revision = 0

$ cargo run --example network_probe -- status
enp0s31f6        kind=ethernet  connected=false hostState=carrier-down
lo               kind=loopback  connected=true  hostState=managed
wlp0s20f3        kind=wifi      connected=true  hostState=managed
$ cargo run --example network_probe -- radios
networkingEnabled = true
  radio wifi      enabled=true hardBlocked=false softBlocked=false
  radio cellular  enabled=true hardBlocked=false softBlocked=false
```

Same live shape as the `9ce258e` waypoint (`enp0s31f6` ⇒ `carrier-down`, wifi ⇒ `managed`) — now
answered by the kernel: netlink `IFLA_CARRIER` for `carrier-down`, `/sys/class/net/*/wireless`
for the `wifi` token, and exactly ONE NM seam call for the whole run.

## 4. The seam-count proof (NmClient calls per read op)

"Before" = the D-Bus read path at `9ce258e` (call sites quoted from
`git show 9ce258e:src/platforms/linux/network.rs`: `real_devices()` → `client.devices()`;
`status_of` → `networking_enabled()` + `wireless_enabled()`; `list_radio_states` → 5 property
reads + `rfkill()`; `get_counters` → `find_device()` + `counters()`). **Every call, on every
read, forever.**

| read op | before (per read) | after: cold | after: warm |
|---|---|---|---|
| `getStatus` | 3 (`devices`, `networking_enabled`, `wireless_enabled`) | **1** | **0** |
| `getInterfaces` | 1 (`devices`) | **1** (for NM's `managed` bit) | **0** |
| `pullStatus` | 3 | 1 | 0 |
| `getCounters` | 2 (`devices` + `counters`) | **0** (kernel `IFLA_STATS64`) | **0** |
| `subscribeStatus` | 1 (`devices`) | 0 | 0 |
| `isNetworkingEnabled` | 1 | 1 | 0 |
| `listRadioStates` | 5 (4 properties + `rfkill`) | 1 | 0 |
| `isDataRoamingEnabled` | 1 (`devices`) | 0 (kernel `wwan` link) | 0 |

Printed table from `cold_store_reads_issue_exactly_one_counted_fallback` (`--nocapture`):

```
read op                  cold  warm
getStatus                   1     0
getInterfaces               1     0
getCounters                 0     0
isNetworkingEnabled         1     0
listRadioStates             1     0
```

Two further counts are asserted: `warm_store_reads_issue_zero_nm_calls` (7 read ops on a warm
store ⇒ the counter does not move) and `counters_fall_back_to_the_seam_when_the_kernel_reports_none`
(a link with no `IFLA_STATS64` ⇒ exactly 1 counted `/sys` read through the seam).

## 5. Availability delta

Regenerated from the implementation, not from assumption. Counts: **12 REAL, 3 DEGRADED,
62 UNAVAILABLE — unchanged in count** (the transport correction changed *which transport serves*
each row, and the evidence that backs it, not how many rows answer).

* REAL rows whose source of truth moved from D-Bus to netlink (status unchanged, `l3_test` ids
  regenerated): `getInterfaces`, `getStatus`, `subscribeStatus`, `pullStatus`, `getCounters`,
  `subscribeCounters`, `pullCounters`, `isDataRoamingEnabled` (a cellular modem is now a kernel
  fact — the `wwan` net device — so this read needs no daemon hop at all).
* REAL rows still served by NM policy (the counted sweep): `isNetworkingEnabled`,
  `isMobileDataEnabled`, `listRadioStates` — real reads of real NM state, honest about the
  source.
* **No DEGRADED → REAL promotion, and that is the honest answer, not an omission:** every
  DEGRADED row is a *host-mutating* verb (`setNetworking`, `setRadioEnabled`, `setMobileData`).
  No netlink path can own a mutating verb (rtnetlink's write verbs are deliberately absent from
  the seam), and law #8 reserves the live proof for the founder. The DEGRADED reason text now
  names that explicitly instead of only citing polkit.
* One UNAVAILABLE reason was wrong and is corrected: `getIpv6Config @67` now says the kernel half
  (per-link addresses via `RTM_GETADDR`) is in the state store while the contract's full field
  set is not assembled — rather than implying nothing is readable.
* No other UNAVAILABLE row changed: they are the subsystem/privilege surface (VPN, hotspot, DHCP,
  QoS, firewall, slicing, DTN, NAN, NIC driver), which the transport correction does not touch.

## 6. Two kernel-path defects found and fixed (one only the live probe could catch)

Both are recorded because they were shipped-candidate code in the checkpoint:

1. **A dump reply was folded as if it arrived under the request's kind.** The kernel answers
   `RTM_GETLINK`/`GETADDR`/`GETROUTE` dumps with `RTM_NEWLINK`/`NEWADDR`/`NEWROUTE` messages;
   the checkpoint's `snapshot()` only matched `GetLink`/`GetAddress`/`GetRoute`, so the link
   table came back **empty on a host with three interfaces**. Caught by the `#[ignore]`d
   read-only live probe (which is why it exists). Fixed by `fold_dump_replies`, which accepts
   both kinds; regression test `dump_replies_fold_the_new_message_kinds_into_the_snapshot` drives
   it through real serialized wire bytes.
2. **One datagram can carry several netlink messages, and only the first was parsed.** The
   crate's `deserialize` parses exactly one message per buffer and silently ignores the rest —
   the same class of bug as the hand-rolled D-Bus marshaller's (report 09 §4), this time from
   trusting a well-maintained crate's convenience API. This is what made this host's
   `enp0s31f6` vanish from the link table while `lo` and `wlp0s20f3` survived. Fixed by
   `parse_datagram`, which walks the datagram using the header's own length field as the stride
   (framing, not a second marshaller) and errors on a length that does not fit rather than
   truncating. Regression tests: `one_datagram_carries_several_events_and_all_are_parsed`,
   `a_bad_framing_length_is_an_error_not_a_truncation`. The reader also keeps the events a
   datagram carried beyond the first in a pending queue, so nothing is dropped.
3. (Found while fixing #2's fallout) **`ARPHRD_*` says `ether` for a normal wlan interface** — the
   wireless-ness lives in the phy, not the link type — so the kernel-derived `kind` token read
   `ethernet` for this box's wifi. Fixed by the kernel's own marker
   `/sys/class/net/<name>/wireless` (a sysfs read: kernel, no daemon hop, no guess), with a
   hermetic regression test (`the_wireless_marker_names_a_wlan_device_that_the_wire_calls_ether`).

## 7. Design decisions the founder should confirm

1. **Policy precedence (kernel facts vs the master switch).** The classifier keeps the original
   precedence — `!NetworkingEnabled` outranks every per-device reason — and fetches the policy
   sweep on every `getStatus`, even when the kernel's facts already explain the state. That is
   what makes "cold store ⇒ exactly one counted call" true and keeps a switched-off host reading
   `networking-disabled` rather than `carrier-down`. The cost is one cached D-Bus sweep per read
   before the cache is warm; the benefit is that the incident's primary diagnostic is never
   shadowed by a hardware fact. If the founder prefers the stricter law-#9 reading (a link the
   kernel fully explains answers with zero seam calls, cold), the order is a three-line change.
2. **`carrier-down` is a non-radio fact.** `IFLA_CARRIER == 0` on a *wifi* link means "not
   associated" (and on a radio, operstate DOWN with no carrier is also what a not-yet-associated
   radio reports), so the carrier-down verdict is deliberately restricted to non-radio links. A
   wifi that is admin-up, radio-enabled and simply not connected therefore reads `managed` with
   `connected: false`, which is what the D-Bus path reported too.
3. **NM refutes, never asserts, the no-firmware marker.** `stalled()` (admin up, operstate DOWN,
   no carrier) fires for a radio that is up but has not associated, and netlink cannot tell the
   two apart. So when NM's device view *has* an entry, its `FirmwareMissing` decides; the kernel
   marker decides only when NM cannot answer. Confirm this is the preferred split.
4. **Policy snapshot caching.** The NM policy view is cached against the store's topology
   revision and never on a timer, so `isNetworkingEnabled` can be one topology event stale (it
   re-sweeps the moment a link or address changes, and `refresh_policy()` forces it). A live
   time-based TTL was rejected on purpose: it would make the acceptance assertions
   time-dependent. If a stale master-switch read matters more than a millisecond-level sweep
   cost, the TTL is a two-line addition.
5. **`getInterfaces` now enumerates the kernel link table**, so virtual links NM's device list
   used to hide (container veths, bridges, tun devices) appear. That is the kernel's truth; the
   caller sees more interfaces than the D-Bus path showed. Their `managed` bit reports `false`
   (NM does not enumerate them) — never a guess.
6. **Ownership note (unchanged from report 09 §7):** `src/platforms/linux.rs` is the per-OS
   aggregator, created by the network cell because the module tree needed it. Still
   unassigned in the charter table — please confirm.

## 8. Blockers

* **No blocker for this wave.** The founder's live bring-back (P5, `examples/network_probe.rs`
  `set-networking`/`set-radio`, gated behind `--i-know-this-changes-host-state`) remains
  founder-run and is untouched by this pass — the read side it prints is now kernel-first.
* `knowledge/availability/linux/network.toml` (conformance §3.1 generator output) is the
  conformance-implementation wave's file; `src/domains/network/availability.rs` is the source it
  should freeze, now regenerated.
