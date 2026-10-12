# 04 — CLI route table: every draft command → its contract route

Wave 1, 2026-10-12. Source of coverage: `smart-protocol-official/drafts/network_commands.sh` (BATCH 4 extraction) + `network_commands.txt` (the rebuild). The standard is `cli.capnp`'s registry law: a command is a thin surface over an EXISTING service contract, bound by schema identity (`T.SchemaId` + interface + method). Disposition legend: **R** = routed to a method · **C** = composition (named methods) · **L** = CLI-local (client-side workflow, no contract route needed — justified) · **G** = gap (decision below).

The grammar face is the rebuild (`net …`); the extraction's `network …` forms are aliases (one row covers both where identical).

## 1. Network family — the read surface

| Command | Route |
|---|---|
| `net` / `network` (dashboard) | **C** `isNetworkingEnabled` + `listRadioStates` + `getInterfaces` + first-page `getStatus` per interface (the `NetworkSummary` shape) |
| `net status` / `network info|connected|type` | **C** `getInterfaces` + `getStatus` (`connected` = any interface connected; `type` = interface `type` tokens) |
| `net interfaces` / `network interfaces` | **R** `getInterfaces` (pagination) |
| `net interface <name>` / `network interface <name> [info]` | **R** `getStatus` (+ `getCounters` for the rx/tx bars) |
| `net interface <name> up|down` | **R** `upInterface` / `downInterface` |
| `net interface <name> mtu <n>` | **R** `setInterfaceMtu @76` — **gap closed this wave** (the interface already *reported* minMtu/maxMtu; now it can be set) |
| `net ip|ip4|ip6` / `network ip…` | **R** `getAddressAssignment` (per interface; `ip` aggregates) |
| `network gateway` | **C** `getRoutes` (default route) — gap closed this wave |
| `network public-ip` | **G→SCOPE-OUT** to the web family (a public-IP lookup is an HTTP workflow; `web.capnp fetch` owns it) |
| `network hostname [set]` | **G→SCOPE-OUT** to the `system` domain batch (verified: no hostname surface in `network.capnp`; `system.capnp` is that batch's business) |

## 2. Network family — wifi and radios

| Command | Route |
|---|---|
| `net wifi on|off` / `network wifi on|off` | **R** `setRadioEnabled("wifi", …)` — the open-token radio set added in the incident wave |
| `net scan` / `network wifi scan` | **R** `wifiScan` (pagination) |
| `net connect "<ssid>" [pass]` / `network wifi connect` | **R** `wifiConnect` (credential = `T.SecretKeyId` — a CLI that takes a password on the command line resolves it through the secret service first; the plaintext-password path of the old CLI is dead) |
| `net disconnect` / `network wifi disconnect` | **R** `wifiDisconnect` |
| `net forget "<ssid>"` / `network wifi forget` | **R** `forgetWifiNetwork @77` — **gap closed this wave** |
| `net wifi known` / `network wifi saved` | **R** `listKnownWifiNetworks @78` (KnownWifiNetwork: ssid, autoConnect, lastConnected, open security token) — **gap closed this wave** |
| `net wifi stats` / `network wifi signal|speed|security|channel|band|bssid` | **R** `getStatus` → `wifi` sub-struct (`WifiStatus` carries them; `security` is the open token per the 2046 rule) |
| `network wifi set-auto-connect "<ssid>" on|off` | **R** `wifiSetAutoConnect` |
| `network wifi connect --band 5ghz|--hidden` | **G→SCOPE-OUT**: band and hidden are per-network preferences the contract's `NetworkConfig` does not carry; the radio/open-token surface already allows a future `NetworkConfig` field. Named as a candidate for the wifi batch, not forced into this wave. |
| `network ethernet [info]` / `network ethernet on|off` | **R** `getStatus` / `setRadioEnabled("ethernet", …)` |

## 3. Network family — routing, dns, firewall, proxy, counters

| Command | Route |
|---|---|
| `network routes` / `net routes` | **R** `getRoutes @79` — **gap closed this wave** |
| `network route add\|remove\|default` | **C** `getRoutes` → mutate → `setRoutes @80` (full-set mutator, same honest shape as `setFirewallRules`) — **gap closed this wave** |
| `net dns` / `network dns` (servers) | **R** `getDnsServers` |
| `net dns set <ip>` / `network dns set …\|--auto` | **R** `setDnsServers` |
| `net dns resolve <host>` / `network dns resolve` | **R** `dns.capnp: DnsService.resolve` |
| `net dns reverse <ip>` | **R** `DnsService.reverseResolve` |
| `net dns bench` / `network dns benchmark` | **C** `DnsService.resolveBatch` + client timing (a benchmark is a workflow over resolution; honest that the chart is client-side) |
| `network dns cache\|flush\|cache stats` / `net dns` cache verbs | **R** `DnsService.cacheStatus` / `flushCache` / `getCacheStats` |
| `network dns doh on|off\|server`, `dot …` | **R** `DnsService.getConfig`/`setConfig` — verified: `DnsConfig.defaultTransport @2` is the open transport token and `transportSecurity @3` is `TlsConfig`, so DoH/DoT are VALUES, not booleans; the CLI passes tokens |
| `network dns custom add\|remove <name> <ip>` | **G→SCOPE-OUT** to the dns batch (local record override has no surface; the dns batch decides the shape — a `customRecords` list or a resolver-pin) |
| `net firewall …` / `network firewall [on|off]` | **G→SCOPE-OUT (partial)**: per-app/per-direction rules route to `getFirewallRules`/`setFirewallRules`; a global firewall on/off is a policy switch the contract does not model — named for the firewall batch |
| `network firewall rules` / `rule add\|remove\|enable\|disable` | **C** `getFirewallRules` / `setFirewallRules` (replace-set) |
| `network firewall stats` / `rule <id>` hit stats | **R** `getFirewallRules` → `hitCount @13` — **gap closed this wave** (a counter is measured or absent; never assumed) |
| `net proxy [set <url>\|off]` / `network proxy on|off\|set\|bypass` | **R** `getProxyConfig @81` / `setProxyConfig @82` — **gap closed this wave** (the HOST's proxy; `web.capnp` keeps its fetch-scoped `setProxyConfig` — two scopes, both named, no rival) |
| `network bandwidth` / `net bandwidth` | **R** `getCounters` |
| `net usage [--window]` / `network traffic [<iface>]` | **C** `getCounters` + window aggregation (client-side over the contract's counters) |
| `net latency --window 5m` / `net history --window 1h` | **C** `subscribeCounters`/`pullCounters` + client history (history retention is CLI-local — honest that it is client-side) |
| `net connections [--live] [--sort]` / `network connections` | **C** `socket.capnp: SocketService` (per-socket `getInfo`; the full enumeration is the socket batch's `list` gap — see sibling gaps) + `getCounters` |
| `net ports` / `network ports\|port <n>` | **C** `socket.capnp` (listening sockets; same socket-batch gap) |
| `net ping <host> [--live] [--count]` / `network ping` | **R** `ping` |
| `net traceroute <host>` | **R** `traceroute` |
| `net speed [--server]` / `network speed` | **G→SCOPE-OUT**: a vendor speed-test is a workflow over a third-party service, not a system capability; compose later from `web.capnp` + counters if ever wanted |
| `network whois <domain>` | **G→SCOPE-OUT** to the web family |
| `net wol <mac>` | **G→SCOPE-OUT** to the Ethernet hardware batch (magic-packet send is link-layer hardware work) |
| `net neighbors` / `net discover` (LAN scan) | **G→SCOPE-OUT (named)**: mDNS/SSDP LAN discovery has no contract home; `discovery.capnp` is the smart-protocol SERVICE registry (verified), not network browsing. Named for a dedicated LAN-discovery batch — deliberately not bolted onto this one. Neighbor tables = `getRoutes`-adjacent / socket batch |
| `net vpn [connect\|disconnect <profile>]` | **R** `attachVpn` / `detachVpn` / `listVpnProfiles` |
| `net vpn killswitch on|off` | **G→SCOPE-OUT (named)**: kill-switch is firewall composition (deny-all except tunnel) — the firewall batch's business; CLI can compose it from firewall rules once the global-switch gap closes |
| `net profile <name>` (atomic wifi+vpn+dns+proxy) | **C** CLI-level atomic workflow over `wifiConnect` + `attachVpn` + `setDnsServers` + `setProxyConfig` + `setRoutes`; the PROFILES themselves are CLI-local state (`net export > state.net.toml` — client-side config, documented as such) |
| `@server1 net …` | **L** transport law (Part 3.9 of the plan), not a route |
| `net diagnose` | **C** client-level guided report over `summary` + four-state status + counters + dns + routes |
| `watch network\|bandwidth\|wifi\|connections` | **R** subscription triplets (`subscribeStatus`/`pullStatus`/`cancel…` collapsed to `--watch`) |

## 4. Sibling families — routes verified per-command, gaps reported to their batches

| Family | Routes (verified) | Gaps → owning batch |
|---|---|---|
| **bluetooth** | on/off → `BluetoothService.enable/disable`; scan → `startScan`/`stopScan`/`ScanSubscription`; pair/unpair → `pair`/`unpair`/`respondToPairing`; connect/disconnect → `connectProfile`/`disconnectProfile`; gatt read/write → `readCharacteristic`/`writeCharacteristic`; gatt subscribe → `setNotify`/`subscribeGatt`; devices/paired/connected → `getDevice`/`getPairedDevices`/`getConnectedDevices`; name → `setName`/`getName`; audio profile connect → `connectProfile`; `watch bluetooth` → subscribe* | **advertise (start/stop BLE advertising) — no surface; alias — no surface; auto-connect — no surface; trust/block — no pairing-policy surface; audio codec select — no surface; discoverable on/off — no surface; Auracast — present (`AuracastService`) so the extraction's broadcast verbs route** |
| **dns** | see §3 (the family spans `network.capnp` + `dns.capnp`) | **custom local records — no surface** |
| **tunnels** | `tunnel create --type wireguard\|openvpn --config …` → **C** `setVpnProfile` + `attachVpn` (the profile manager lives in network.capnp); `tunnel create --type ssh` → **R** `tunnel.capnp: TunnelService.open` (a true tunnel); connect/disconnect/destroy → `attachVpn`/`detachVpn`/`removeVpnProfile`; `tunnel <id> set endpoint\|dns\|keepalive\|kill-switch\|auto-connect` → **C** profile update (kill-switch/auto-connect fields: **verify in VpnProfile — if absent, VPN-batch gap**); export/import → **L** config-file workflow (CLI-local, honest); presets → **L**; `watch tunnels` → `subscribeVpnConnection`/`TunnelSubscription` | **VpnProfile field coverage for the `set` verbs — verified in Wave 2 against the real struct; listed here as the tunnel batch's first check** |
| **discovery** | `discovery peers` / `peers scan` → **R** `discovery.capnp: Discovery.lookup/get` + `DiscoverySubscription` (smart-protocol service peers); advertise → **C** `DiscoveryRegistry.register` (registering THIS host's services, not network advertising) | **mDNS/SSDP — see §3 (no home; own batch)** |
| **web** | get/post/put/patch/delete → **R** `WebService.fetch`; download [--progress\|--resume] → **C** `fetchStream` + client write (resume = Range header composition — **verify WebService carries headers; if not, web-batch gap**); ws (list/send/close) → **R** `openWebSocket` + `WebSocketConnection` | **download resume / header composition — verified in Wave 2** |
| **sockets** | `socket info` → **R** `SocketService` (getInfo family); `socket list` → **R** subscription/enumeration — **no "list all sockets" method; socket-batch gap (add `listSockets` or drive from an event backlog)** | **list-all-sockets — socket batch** |

## 5. The gap ledger (closed this wave vs owned by other batches)

**Closed in this wave (network.capnp, committed by the founder):** `setInterfaceMtu @76`, `forgetWifiNetwork @77`, `listKnownWifiNetworks @78` + `KnownWifiNetwork`, `getRoutes @79`, `setRoutes @80` + `Route`, `getProxyConfig @81`, `setProxyConfig @82` + `ProxyConfig`, `FirewallRule.hitCount @13`. Validator exit 0; every schema compiles; header + SCHEMAS.md regenerated.

**Owned by other batches (routed here so no one re-discovers them):** bluetooth advertise/alias/trust/auto-connect/codec/discoverable (bluetooth batch) · dns custom records (dns batch) · VpnProfile field coverage + kill-switch (vpn/firewall batch) · socket list-all (socket batch) · web resume/headers (web batch) · LAN mDNS/SSDP discovery (own batch) · system hostname (system batch) · public-ip/whois/speed (web batch) · wol (hardware batch) · wifi band/hidden preferences (wifi batch).

**CLI-local (no contract route, documented as client-side):** `net profile` files, `net export/import`, `net history` retention, `net ui`, `@host` transport selection, ranking/benchmark charts, the dashboard's layout itself.
