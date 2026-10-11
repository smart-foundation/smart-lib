# 09 — network domain: NM backend, host-global switch + radios (the incident's closure)

Status date: 2026-10-12. Workspace: `/home/lufuno/smart/smart-lib-ng/`.
Files written (exclusive): `src/domains/network/{mod,types,backend,api,availability,tests}.rs`,
`src/platforms/linux.rs`, `src/platforms/linux/network.rs`, `examples/network_probe.rs`, this report.
Nothing outside my ownership was created, edited or deleted. No git commits.

**Verdict: P2/P3/P4 done and mechanically verified. Contract surface is 76 `Network` methods
(`@0`–`@75`, enumerated from the OUT_DIR inventory, not memory). The Linux backend speaks a
hand-rolled NetworkManager D-Bus client (no `nmcli`/`ip` shellouts) over the system bus; the read
surface is proven live (read-only), every host-mutating path is behind a mockable seam and proven on
mocks only (law #8). `cargo test -j 6 --features domain-network` is green: 272 passed, 0 failed,
1 ignored (the read-only live probe); warning-clean for my files; the feature law holds with
`domain-network` off.**

Pending (NOT mine): P5 — the founder's live bring-back. My binary (`examples/network_probe.rs`)
gates the mutating verbs behind `--i-know-this-changes-host-state` and documents them as the
founder-run vehicle; the domain never turns a switch.

---

## 1. Checklist status (BRIEF-06)

- **[DONE] Trait.** `domains::network::backend::NetworkBackend`: object-safe, 76 methods — one
  per `Network` contract method `@0`–`@75` — plus `cancel_subscription` (`StatusSubscription.cancel
  @1`) and two default helpers (`network_caps()`, `last_unavailable_reason()`). Every method returns
  `core::Result<T>`. **No per-method `Cap` param** (law #5): authority is the held service
  reference; `nicProvider @71` — the one capability-shape accessor — is mapped to the NM path the
  backend already holds and returns `Unavailable` because the NIC-driver surface is not built this
  milestone. The trait's capability story is the trait's top doc.
  Evidence: `grep -cE '^\s+fn ' src/domains/network/backend.rs` = **79** (76 + 3). Inventory line
  `interface network.capnp:Network methods=76`.
- **[DONE] Honest-Unavailable rule.** Every contract method Linux cannot serve is still a trait
  method with a default body returning `Error::not_supported("<reasonClass>: <what>")`. The reason
  class in the message matches the row in `availability.rs`.
  Evidence: `domains::network::tests::unavailable_methods_answer_not_supported_with_a_reason`
  asserts `connect` answers `notSupported` with `message.starts_with("no-privilege:")`,
  `!retryable`.
- **[DONE] Linux backend** over native NM D-Bus per the host map §2 ("NM D-Bus for privileged /
  wifi-connect ops"). **No shellouts** (`nmcli`/`ip`/`wpa_cli` never invoked); the D-Bus client is
  ~300 lines behind the seam. Zero fabricated values: `strength`/`speed` are read as 0 = "not
  measured" (documented), never a placeholder. std boundary: `src/platforms/linux/network.rs`
  declares `extern crate std;`; the public API and the domain stay `no_std + alloc`.
- **[DONE] Public API.** `domains::network::api::Network<'a, B>` — a borrowing, zero-cost wrapper
  exposing `summary()` (networking + radios, the CLI-shaped query), `networking_enabled()`,
  `status()`, `interfaces()`, `status_stream()`/`close_status_stream()`; `#[must_use]` on queries.
- **[DONE] Behavior tests (inline, mock-based).** `Mock` NM client behind `NmClient` injects all 20
  `T.Error` codes (mapping total through the seam, both directions via core); pagination
  (`limit==0` means one); bounded pulls on status + counter subscriptions; sysfs counters read from
  a real temp dir; rfkill hard/soft parsed and proven not conflated with policy; `hostState` tokens
  round-trip (POD and through a real capnp message).
  Evidence: `platforms::linux::network` **21 passed**, `domains::network` **7 passed**.
- **[DONE] Availability table.** `domains::network::availability` — one row per contract method,
  REAL/DEGRADED/UNAVAILABLE + `reasonClass`, matching `03-CONFORMANCE.md` §3.2. Every REAL/DEGRADED
  row names an `l3_test`.
  Evidence: `availability_rows_are_consistent` + `availability_real_rows_have_a_backing_test`
  assert each claimed test id exists; no row uses the flagged `unknown` class.
- **[DONE] REAL Linux integration test.** `platforms::linux::network::tests::live_reads_networkmanager`
  — read-only over the real bus, `#[ignore]`d (documented) so the default hermetic suite never
  touches the network. It reads real NM state; it **passes** when run with `cargo test -- --ignored`.
  Nothing host-changing is ever called (law #8).
- **[DONE] Tests green / warning-clean.** `cargo test -j 6 --features domain-network`: **272
  passed, 0 failed, 1 ignored**. `cargo build --all-targets`: no warning originates in
  `src/domains/network/**`, `src/platforms/linux*`, or `examples/network_probe.rs`.
- **[DONE] Teaching doc.** `src/domains/network/mod.rs` opens with the 5-line domain explainer.

## 2. Test-output evidence

```
$ cargo test -j 6 --lib --features domain-network
test result: ok. 272 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out

  domains::network::tests ........................ 7 passed
  platforms::linux::network::tests ............... 21 passed (+1 ignored live probe)
```

The four-state regression test (`platforms::linux::network::tests::get_status_four_states_distinct`):

```
the incident's four failure modes must be four distinct statuses
assert states == [NetworkingDisabled, RadioOff, NoFirmware, CarrierDown]  ✓ (4 distinct values)
```

The read-only live probe (founder/runner-invoked, reads only):

```
$ cargo run --example network_probe -- status
enp0s31f6        kind=ethernet  connected=false hostState=carrier-down
lo               kind=loopback  connected=true  hostState=managed
p2p-dev-wlp0s20f3 kind=unknown  connected=false hostState=managed
wlp0s20f3        kind=wifi      connected=true  hostState=managed
$ cargo run --example network_probe -- radios
networkingEnabled = true
  radio wifi      enabled=true hardBlocked=false softBlocked=false
  radio cellular  enabled=true hardBlocked=false softBlocked=false
```

(An unplugged ethernet reads `carrier-down` live — the exact shape the domain exists to make
distinguishable from `networking-disabled` / `radio-off` / `no-firmware`.)

## 3. Trait API sketch

```rust
pub trait NetworkBackend {
    // Implemented surface (Linux overrides; the rest default to honest Unavailable).
    fn get_interfaces(&self, page: Pagination) -> Result<Page<InterfaceSnapshot>>;            // @0
    fn get_status(&self, iface: NetworkInterfaceId) -> Result<ConnStatus>;                    // @1 (hostState)
    fn subscribe_status(&self, iface) -> Result<SubscriptionId>;                              // @2
    fn pull_status(&self, sub) -> Result<ConnStatus>;                                         // @3
    fn cancel_subscription(&self, sub) -> Result<()>;                                         // StatusSubscription.cancel
    fn get_counters(&self, iface, window: Duration) -> Result<InterfaceCounters>;            // @35
    fn subscribe_counters(&self, iface) -> Result<SubscriptionId>;                            // @37
    fn pull_counters(&self, sub) -> Result<InterfaceCounters>;                                // @38
    fn is_networking_enabled(&self) -> Result<bool>;                                          // @73
    fn set_networking_enabled(&self, enabled: bool) -> Result<()>;                            // @72
    fn set_radio_enabled(&self, radio: &str, enabled: bool) -> Result<()>;                    // @74
    fn list_radio_states(&self, page) -> Result<Page<RadioState>>;                            // @75
    fn is_mobile_data_enabled(&self) -> Result<bool>;                                         // @12
    fn set_mobile_data_enabled(&self, enabled: bool) -> Result<()>;                          // @11
    fn is_data_roaming_enabled(&self) -> Result<bool>;                                        // @14
    fn set_data_roaming_enabled(&self, enabled: bool) -> Result<()>;                         // @13
    // + 62 default methods: connect, disconnect, VPN, hotspot, DHCP, DNS, QoS, firewall,
    //   slicing, DTN, NAN, telemetry, ping, traceroute, nicProvider, ... each
    //   `Err(Error::not_supported("<class>: <reason>"))`.
}
```

`ConnStatus.host_state: HostState` — the enum whose 7 tokens (`managed / unmanaged /
networking-disabled / radio-off / carrier-down / no-firmware / unknown`) are the incident's whole
point; the four diagnostic tokens are distinct typed values the compiler cannot collapse.

## 4. The D-Bus seam design (`NmClient`)

`platforms::linux::network` defines the seam the backend needs and nothing more:

```
trait NmClient {
  networking/wireless/wwan {enabled,hardware_enabled}() -> Result<bool>
  set_networking_enabled / set_wireless_enabled / set_wwan_enabled(b) -> Result<()>   // the mutators
  devices() -> Result<Vec<DeviceSnapshot>>         // path, iface, DeviceType, State,
                                                   // FirmwareMissing, Managed, carrier, Real, hw, mtu, driver
  rfkill() -> Result<Vec<RfkillState>>             // hard/soft per radio, from /dev/rfkill
  counters(iface) -> Result<InterfaceCounters>     // from <sysfs>/<iface>/statistics/*
}
```

Two implementations: **`RealNmClient`** (production) — SASL `EXTERNAL` + `BEGIN` + `NEGOTIATE_UNIX_FD`
+ `Hello` (unique name) over `/run/dbus/system_bus_socket`, a self-contained capnp-free D-Bus
marshaller/reader, `org.freedesktop.DBus.Properties.{Get,Set}`, `NM.Enable`, `NM.GetDevices`, plus
`/sys` and `/dev/rfkill` reads; and **`Mock`** in the tests. `LinuxNetworkBackend<C: NmClient>` is
generic over the seam, so the four-state function, the write mappings, pagination and error
injection are all exercised without touching the host.

Three D-Bus wire bugs were found and fixed by diffing my bytes against a captured reference
(`dbus-send` through a logging proxy) and by reading the real reply stream — all now green:
1. **String length**: the `u32` length excludes the trailing NUL (the NUL is a separate byte); I had
   it inverted → the reference client's input was malformed → daemon reconnect loop.
2. **`Hello` then the `NameAcquired` signal**: the reference daemon closes a connection that calls a
   routed service before `Hello`, then emits a signal right after `Hello`. Fix: send `Hello` on
   connect and skip interleaved SIGNAL messages when reading a reply.
3. **`read_message` header bound**: the last header-field element is not 8-padded (arrays don't pad
   the final element), so the field loop ends at `16 + fields_len`, not at the 8-aligned body start.

## 5. NM mapping, as implemented (verified read-only on this box)

| contract method | NM operation | status |
|---|---|---|
| `isNetworkingEnabled @73` | `Properties.Get NetworkingEnabled` (readonly `b`) | REAL |
| `setNetworking @72` | `Enable(bool)` | DEGRADED (impl + mock-verified; live authority NM/polkit-gated → founder proof) |
| `setRadioEnabled @74` `"wifi"` | `Properties.Set WirelessEnabled` | DEGRADED (same) |
| `setRadioEnabled @74` `"cellular"` | → `Properties.Set WwanEnabled` (routes to the one `setMobileData` home) | DEGRADED |
| `listRadioStates @75` | `WirelessEnabled`/`WirelessHardwareEnabled` + `/dev/rfkill` (hard/soft) | REAL |
| `getStatus @1` + `hostState` | `NetworkingEnabled`, `WirelessEnabled`, per-device `State`/`Managed`/`FirmwareMissing`/`Wired.Carrier` | REAL |
| `getInterfaces @0` | `GetDevices` + per-device `Interface`/`DeviceType`/`Mtu`/`HwAddress`/`Real` | REAL |
| `getCounters @35` | `<sysfs>/<iface>/statistics/*` | REAL |
| `subscribe/pull/cancel` | token mint + live re-read per pull (bounded by caller) | REAL |
| `setMobileData @11` / `isMobileDataEnabled @12` | `WwanEnabled` set/get | DEGRADED / REAL |
| `setDataRoaming @13` / `isDataRoamingEnabled @14` | (ModemManager absent) not-applicable / measured-absent `false` | UNAVAILABLE / REAL |
| the other 62 methods | NM subsystem/privilege surface not implemented this milestone | UNAVAILABLE + reason |

`hostState` mapping (the `classify_host_state` function, load-bearing order):

```
!NetworkingEnabled                       → networking-disabled   (master switch outranks everything)
!Managed                                 → unmanaged
FirmwareMissing                          → no-firmware
wifi && !WirelessEnabled                 → radio-off
wired Carrier == false                   → carrier-down
State == 20 (unavailable), no named cause → unknown               (honest gap, never "managed")
otherwise                               → managed
```

Interface ids are FNV-1a/64 of the ifname — a deterministic, stable handle, **not** a host credential
and **not** fabricated data, so `getStatus(id)` is reproducible without a hidden session table.

## 6. Availability table (as implemented)

77 rows = the 76 `Network` methods + the `cancelSubscription` helper.
**REAL 12, DEGRADED 3, UNAVAILABLE-with-reason 62** (of the 76 contract methods: 11 REAL, 3
DEGRADED, 62 UNAVAILABLE).

- **DEGRADED (detail names the degraded capability):** `setNetworking`, `setRadioEnabled`,
  `setMobileData` — the map is implemented and mock-verified (Enable(true/false), WirelessEnabled,
  WwanEnabled), but the *host write authority* is gated by NM/polkit and polkitd is absent on this
  box, so I cannot certify the live write; the mock claims the mapping, the founder certifies the
  bring-back. (Conformance §3.4: DEGRADED names the capability and keeps an `l3_test`.)
- **UNAVAILABLE reason classes:** `no-privilege` for the CAP_NET_ADMIN/NM-write surface
  (connect/disconnect, DNS/QoS/firewall/IPv6 writes, up/down, MAC randomisation); `no-backend` for
  the NM subsystem work not built (VPN, hotspot, DHCP, scan/ping/traceroute, topology, slicing, DTN,
  NAN, NIC driver, telemetry); `not-applicable` for host-absent concepts (satellite, cellular modem,
  data roaming). Every row answers `notSupported` at runtime whose message begins with the class.

## 7. Host-integration decisions and their cost

- **NM D-Bus as the primary target** (host map §2): cost is the ~300-line hand-rolled client and the
  `Hello`+signal handling a full client needs; benefit is one dependency-free seam over the daemon
  that is already running, and privileged/wifi ops route through it.
- **`hardBlocked`/`softBlocked` from `/dev/rfkill`, `enabled` from NM policy:** keeps the two
  blocked bits genuinely separate from policy (the schema's explicit warning); `/dev/rfkill` is
  opened read-only and, if unreadable, soft reads as not-blocked (documented limitation — the soft
  bit needs a readable rfkill source).
- **`strength`/`speed` reported 0 = "not measured"** rather than a placeholder — I do not read the
  active-AP signal in this milestone; the honest zero is documented, never a fabricated dBm.
- **`real` devices only** (skip NM `Real=false` pseudo-devices); on this box the p2p device reports
  `Real=true` and is included, matching the live device set exactly.
- **setDataRoaming = not-applicable** (no ModemManager) rather than a vacuous success — the roaming
  toggle is genuinely a no-op concept here.
- **Platform aggregator `src/platforms/linux.rs`:** I created it (was unassigned in the charter
  table; the module tree needs an OS file to wire a cell). It only contains
  `#[cfg(feature="domain-network")] #[path="linux/network.rs"] pub mod network;`. A later Linux cell
  agent adds its own gated `pub mod <domain>;` line. **Flag for the orchestrator/founder to confirm.**
- **Feature law:** `cargo build --lib --no-default-features --features "std,domain-files"` compiles
  clean — `platforms::linux` has no `network` cell and `domains::network` is not wired, so disabling
  the feature removes the codegen and the backend.

## 8. What is honestly Unavailable, and why (62 methods)

The subsystem/privilege surface (`02-HOST-CAPABILITIES.md` §1.2): VPN, hotspot/tethering, DHCP
request/release, per-interface DNS/QoS/firewall, up/down, connect/disconnect, scan, ping/traceroute,
topology, power-profile, handoff, slicing, telemetry, DTN, Wi-Fi-Aware NAN, and the NIC-driver
provider — each needs a real service (VPN plugin, `hostapd`, ModemManager, netlink CAP_NET_ADMIN,
etc.) or is polkit-gated with polkitd absent here. The trait keeps all of them (law 3: "the trait
may not lie about them"), and each returns `notSupported` with a nameable class — the first honest
Linux milestone is exactly the read surface + the host-global verbs, per the host map's pyramid.

## 9. CLI-shape implication (for later, per the plan's P6)

The four new verbs map 1:1 onto subcommands with no invented flags: `smart network networking`
(print `isNetworkingEnabled`), `smart network status [iface]` (per-interface `connected` +
`hostState` — the token is the argument users read to tell a switched-off host from a dead
interface), `smart network radios` (each radio's `enabled`/`hardBlocked`/`softBlocked`),
`smart network set-networking <true|false>`, and `smart network set-radio <wifi|cellular>
<true|false>`; `listUnavailable` should enumerate exactly the 62 `UNAVAILABLE` rows so a caller can
see which verbs the backend refuses and why. The proof binary already exercises this exact shape
(`network_probe status|radios|networking|set-networking|set-radio`), gating the two mutating verbs
behind `--i-know-this-changes-host-state` so the bring-back is deliberately a founder action.

## 10. Blockers / notes for the orchestrator

1. **No blocker for my wave.** P5 (live bring-back) is founder-run; my binary is its vehicle.
2. `src/platforms/linux.rs` created as the unassigned per-OS aggregator (see §7) — confirm ownership.
3. `knowledge/availability/linux/network.toml` (conformance §3.1 generator output) is the
   conformance-implementation wave's file; my availability table is the source it should freeze.
