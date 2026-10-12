# 06 — The CLI grammar, mechanical

Wave 1, 2026-10-12. The plan's Part 3 is the law; this document is the law made checkable: a grammar table with one worked example per verb family, so Wave 3 generates the shell without inventing anything. Face = the rebuild's `net` short form; aliases = the extraction's `network …` forms.

## 1. The shape law, applied

`net <task> [<noun>] <verb> [selectors] [flags]` — flags are the contract parameter names verbatim (no synonyms), selectors are `--where <field> <op> <value>` over the listed contract type, verbs come from the closed vocabulary.

## 2. Worked examples (one per verb family)

| Family | Command | Route | `--json` shape | Exit |
|---|---|---|---|---|
| status (default) | `net` | summary C | `{networkingEnabled:bool, radios:[…], interfaces:[…]}` | 0 |
| status | `net wifi stats` | getStatus | `{ssid, security, strength, speed, bssid, channel, band}` | 0 |
| list | `net wifi list` (alias `network wifi saved`) | listKnownWifiNetworks | `{networks:[{ssid, autoConnect, lastConnected, security}], total}` | 0 |
| list | `net routes` | getRoutes | `{routes:[{destinationCidr, gateway, metric, interfaceId}], total}` | 0 |
| get | `net interface wlan0` | getStatus | `{interface, connected, hostState, type, strength, speed, meter:…}` | 0 / 3 / 4 |
| on/off | `net wifi off` | setRadioEnabled | `{}` (void) | 0 / 1 / 3 / 4 |
| on/off | `net networking off` | setNetworking | `{}` | 0 / 4 |
| up/down | `net interface wlan0 down` | downInterface | `{}` | 0 |
| set | `net interface wlan0 mtu 1400` | setInterfaceMtu | `{}` | 0 |
| set | `net dns set 1.1.1.1 8.8.8.8` | setDnsServers | `{}` | 0 |
| set | `net proxy set http://proxy:3128` | setProxyConfig | `{}` | 0 |
| connect | `net connect "Home"` | wifiConnect | `{}` + phased progress on the human side only | 0 / 1 |
| disconnect | `net disconnect` | wifiDisconnect | `{}` | 0 |
| forget | `net forget "Old"` | forgetWifiNetwork | `{}` | 0 |
| start/stop | `net firewall allow "firefox" outbound` | setFirewallRules (replace-set) | `{}` | 0 |
| add/remove | `net routes add 10.0.0.0/8 192.168.1.1` | getRoutes→setRoutes | `{routes:[…]}` (the new table) | 0 |
| apply | `net profile work` | composition (wifi+vpn+dns+proxy+routes, atomic) | `{applied:[…steps…]}` | 0 / 1 (first failure names the step) |
| watch | `watch net bandwidth` | subscribeCounters/pullCounters | one object per event: `{seq, interfaceId, rx, tx}`, `--count` bounds | 0 |
| watch | `net ports --live` | socket family | `{seq, socket…}` per change | 0 |
| where | `net connections where remote-port == 443` | socket family | the filtered list | 0 / 2 (bad field → usage) |
| diagnose | `net diagnose` | composition | the guided report | 0 |
| bench | `net dns bench` | resolveBatch + timing | ranked servers | 0 |
| resolve | `net dns resolve example.com` | DnsService.resolve | `{answers:[…], server, tookMs}` | 0 |
| reverse | `net dns reverse 1.1.1.1` | DnsService.reverseResolve | `{name}` | 0 |
| export/import | `net export > state.net.toml` | CLI-local | TOML of CLI-local profile state | 0 |
| remote | `@server1 net status` | transport law | identical to local `net status` + `--verbose` names the transport | 0 / 5 |
| unavailable | any command whose availability row ≠ REAL | — | `{error:{kind:"notSupported", reasonClass, reason}}` | 3 |

## 3. The honesty rules, mechanical

- `net --help` and per-command help read the registry (`listCommands`/`listUnavailable`): a command whose availability row on THIS host is not REAL is **not listed** — it exists in the registry with its reason (`net registry list-unavailable` shows the hidden ones with reasons).
- `--verbose` appends the schema identity of every call made: `network.capnp:Network.wifiConnect @7` — the audit trail; the same line proves the CLI called something.
- Success output is only ever printed after an `ok` result: no `net wifi on` prints "WiFi enabled" unless `setRadioEnabled` returned ok. (Grep-rule for Wave 3's tests: no success string may exist in a path where the call result is discarded.)
- DEGRADED rows print what is degraded in `--verbose` and mark JSON output with the degradation fields the availability schema carries.
- Machine mode: `NO_COLOR`/non-TTY strip everything; JSON keys are contract field names; no emoji, no decoration, no extra fields.

## 4. Selectors, streaming, and exit codes (summary of the laws)

- Selectors: `--where <field> <op> <value>`, ops `== != > < ~=`, `and`/`or`; a field that doesn't exist in the contract type is a usage error naming the valid fields (generated from the schema).
- Streaming: `watch` = bounded pull loop; `--interval`, `--count`, `--follow`; drops counted and reported in the stream's terminal summary.
- Exit codes: `0` ok · `1` call failed · `2` usage · `3` unavailable-with-reason · `4` permission · `5` timeout — published in `net --help` and the man page.

## 5. Where the grammar meets contract limits (flagged for Wave 2/3)

1. `setNetworking`/`setRadioEnabled` are DEGRADED until the founder's live proof (mock-verified) — the grammar displays them as available but `--verbose` marks the live-authority caveat (the honest state of mock-verified control).
2. The firewall verbs compose a replace-set mutator: `net firewall allow X` is `getFirewallRules → add → setFirewallRules` — correct, but the JSON shows the whole rule set so the user sees the hammer; documented in help.
3. `net profile` atomicity is client-side: a failure mid-sequence reports the applied steps; true transactional networking is not a contract guarantee (the contract's model is per-call), and help says so.
4. `@host` over TCP carries the degraded capability marking — the grammar's `--verbose` surfaces it every time, because a caller must never be able to miss it.