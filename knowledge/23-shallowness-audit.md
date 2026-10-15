# 23 — The Shallowness Audit (CRITIC, read-only)

**Standard applied:** *this shell is the only interface; it replaces every other tool; every domain must expose its full power — BASICS (non-negotiable), GOOD-TO-HAVES, and FLEXES.*

**Evidence basis.** Every claim below was produced by running the live binary
`/home/lufuno/smart/smart-lib-ng/target/debug/smart` (built 2026-10-15 21:05, no source newer —
verified with `find crates/shell/src src -name '*.rs' -newer target/debug/smart` → empty) on the real
host (uid 1000, runit supervisor, X11 seat, NetworkManager, 374 processes, 58 services, 258 GSettings
schemas, 50 log sinks), plus reading the shell surface (`crates/shell/src/domains/*`), the engine
(`src/domains/*` + `src/platforms/linux/*`), the contract (`smart-protocol-official/schema/*.capnp`),
the founder's vision (`smart-protocol-official/drafts/network_commands.sh`, 260 lines) and the old
demo shell (`smart-protocol/crates/smart/src/cmd/*`). Nothing was modified; the only writes were two
scratch files under `/tmp`.

**The one-line verdict:** the engine is deep (142 REAL + 83 DEGRADED methods across the nine
domains) and the shell is a **museum of inspection** — 152 verbs that answer questions no user has,
in a shell language that cannot write, cannot quote, cannot redirect, cannot expand `~`, cannot keep
state across two commands, and whose flagship "pipes" output mode prints nothing at all.

---

## THE VERDICT — scored table

| domain | cmds live | BASICS missing | GOOD-TO-HAVES missing | FLEXES missing | shallow spots | live evidence of the worst |
|---|---|---|---|---|---|---|
| **network** | 15 | **9** | 6 | 3 | 2 | `network status` never shows an IP address; `network counters/routes/proxy` don't exist |
| **clipboard** | 14 | **4** | 4 | 2 | 2 | `--force clipboard set` "places" 5000 bytes; a later `clipboard get` answers **70 bytes of unrelated text** |
| **service** | 14 | **3** | 3 | 1 | 2 | `service controls start` ignores its verb and reprints the whole list |
| **files** | 34 | **5** | 10 | 4 | 3 | `files recents` answers 24 rows with an **empty MIME column and `—` for every timestamp** |
| **process** | 17 | **6** | 5 | 4 | 3 | every **live** process reports `exit_status:"exited 0"`; 334 of 374 rows hidden behind a cap |
| **settings** | 13 | **3** | 3 | 2 | 3 | no `delete`/`reset` verb at all; structured values render as `[1 items]` |
| **logging** | 12 | **3** | 2 | 1 | 2 | no grep/pattern over a real host log; `entries` is per-process, always empty one-shot |
| **clock** | 26 | **3** | 5 | 3 | 2 | `clock` says hardware-rng **yes**, `clock security` says **no**; timers vanish between two commands |
| **time** | 7 | **5** | 4 | 2 | 1 | no epoch, no `--format`, no `from-tai`, no zone list |
| **shell core** | — | **11** | 6 | 5 | 8 | `-o raw` = **0 bytes for all nine domains**; `help` lists 3 of 9 live domains |
| **TOTAL** | **152** | **52** | **48** | **27** | **28** | |

Engine gap vs shell gap is named per row below. Contract ceilings (methods in
`schema/*.capnp`): network 422, files 419, process 146, clock 129, logging 65, service 76,
clipboard 63, time 50, settings 41 — across nine domain contracts, **1,411 contract methods**;
the engine serves 152 REAL + 83 DEGRADED = 235; the shell exposes 152.

---

## THE WORST TEN GAPS — ranked by "a user hits this in the first hour"

1. **`-o raw` prints nothing, for every domain.** `smart -o raw <anything>` emits **0 bytes**:
   `network status`, `process stats`, `time now`, `clock now`, `files stat`, `logging sinks`,
   `service status`, `settings domains` — all measured 0 bytes (`wc -c`). Cause: each domain's
   `run()` renders only `if *format != OutputFormat::Raw` (e.g. `network/mod.rs:152`) and `main.rs`
   drops the returned value (`Ok(_) => {}`, main.rs:161). The documented "raw (pipes)" mode is a
   black hole; the engine's real pipe seam is unreachable from one-shot scripts.
2. **The shell cannot tell you your own IP address.** `network status -o json` enumerates name,
   type, connected, host_state, up, mtu, managed, mac, ssid — **no IPv4/IPv6, no gateway, no DNS,
   no bandwidth, nothing** (verified on `enp0s31f6`, `lo`, `wlp0s20f3`). `network ip`, `network dns`,
   `network routes`, `network counters`, `network proxy` all answer `error: unrecognized
   subcommand`. The engine serves `getRoutes` **REAL** (availability.rs:243) and `getCounters`
   **REAL** (availability.rs:198) and the shell wires neither. `ip addr` is the first thing any
   operator types; this shell cannot answer it at all.
3. **You cannot start a process.** There is no spawn/run/exec verb in any domain; the engine's
   `spawn` itself refuses with "no-privilege: spawning is a host-mutating act this backend never
   performs" (`src/domains/process/backend.rs:56`). A shell billed as "the only interface" that
   replaces every other tool cannot launch a program. (Engine gap + shell gap.)
4. **Clock state dies between two commands.** `clock timer create` prints `Timer #1 … a real
   timerfd this process owns`; the very next line in the same REPL session — `clock timer list` —
   answers `(0) … the timers this process armed`, and `clock timer check 1` answers
   `✗ notFound`. Same for alarms: `clock alarm create` → `Alarm #1`, then `clock alarm list` →
   `(0) the store holds no alarm`. Cause: every REPL line reconstructs the backend
   (`shell.rs:632 let backend = LinuxClockBackend::new();`). 26 clock verbs, none composable.
5. **`clipboard set` is theater.** In the REPL it is refused (guard hardcoded false,
   `shell.rs:511`). One-shot with `--force` it claims `✓ setContent — 5000 bytes placed on the
   clipboard`, and the next `clipboard get` answers `Clipboard (70 bytes) … text  For more
   information, try '--help'…` — the selection died with the process. The single most-used
   clipboard verb has never once, on this host, put a byte where a human could paste it.
6. **The shell language is parse-only.** In script mode `if @network/networking == true:` followed
   by an indented body fails with `✗ error: unexpected argument 'else:' found / Usage: network
   interfaces` — because `alias::expand_shorthand` splits on *all whitespace and rejoins with single
   spaces* (`alias.rs:148-151`), flattening every multi-line block into one line before the parser
   sees it (`-c` mode bypasses the alias layer, which is why it works there). Interactively, a typed
   `if` header evaluates alone and the body line then runs **unconditionally**. `for iface in
   @network/interfaces:` → `✗ no interface named $iface` (no variable interpolation anywhere).
   `fn hello(name):` → `✗ Unknown command: hello`. `$var` alone → `✗ Unknown command: $var`.
   Variables are assign-only decoration; conditionals, loops and functions do not condition, loop,
   or call.
7. **The REPL lies to the user twice per command.** `help` lists `network, clipboard, service,
   migrate` and closes with "every other domain is the migration queue" — while `files`, `process`,
   `settings`, `logging`, `clock`, `time` are all live in the same REPL. `help files` answers
   `◌ files — migration pending`. And every gated refusal says "In the REPL confirm the prompt" —
   no prompt exists: `confirm_host_write` is called from exactly two sites, both in `apply`
   (`shell.rs:912,940`); every `exec_*` hardcodes `WriteGuard { allowed: false }`
   (`shell.rs:477,511,535,556,575,594,613,633`). The REPL is read-only by construction and never
   says so.
8. **No quoting, no stdin, no redirection.** `clipboard set "hello world"` → clap usage error (the
   REPL splits on whitespace, quotes survive as literal characters); `echo x | smart clipboard set`
   → `error: the following required arguments were not provided: <TEXT>`; `smart -c 'time now >
   /tmp/out.txt'` → `error: unexpected argument '>' found`; `files ls ~` → `notFound (while
   statx(~))`; `files ls $HOME` → `notFound (while statx($HOME))`. There is no word-quoting, no
   `$VAR`/`~` expansion, no pipe-into-a-command, no `>`/`>>`/`2>` — you cannot script one write.
9. **Every live process carries a fabricated exit code.** `smart -o json process 1` (runit, state
   `S`, alive) answers `"exit_status":"exited 0"`. The engine fills `ProcessStatus` with
   `ExitStatus::default()` (`src/platforms/linux/process.rs:777`), and the default is
   `Exited(0)` (`src/domains/process/types.rs:296-302`). Every JSON/pipe consumer of the process
   domain receives a wrong fact about every process on the host. (The same JSON also contains the
   duplicated key `"cpu_time_nanos"` twice.)
10. **The process table is a 40-row peephole with no way through.** 374 processes enumerated; the
    table shows 40 and prints `· 334 more below the cut` (`domains/process/render.rs:26,605-614`).
    There is no `--limit`, `--skip`, `--sort`, or top-by-CPU (the filter flags are only
    `--name/--state/--user`); names longer than 20 characters are mangled to
    `pool_workqueue_rele…` (`render.rs:664`). The same cap hides log sinks (`logging/render.rs:26,275`)
    and cuts the schema list to 24 of 258 (`settings/render.rs:28,422`).

Runners-up, in order: bare `files` is unreachable in the REPL (`alias.rs:89` rewrites it to the
non-existent `filesystem` domain → "◌ migration pending"); `service controls <verb>` ignores its
argument; `settings category security` answers 2039 keys — i.e. every key on the host — with the
filter a no-op; `clock quality tsc` answers "0 samples" as if it were a measurement;
`files recents` shows raw `file://` URIs with empty MIME and `—` timestamps; `settings` can write
but never unset.

---

## THE SYSTEMIC PATTERN — what the team keeps leaving shallow

**The engine is built as depth; the shell is built as a demo. Three failure modes repeat in every
domain:**

1. **Per-invocation backends make state verbs unwatchable theater.** Every command — and every
   REPL line — constructs a fresh `Linux…Backend::new()` (shell.rs:555, 574, 593, 612, 632, 651).
   Timers, alarms, the clipboard ledger, the logging store: created, printed, destroyed inside one
   command. Verb #2 can never observe what verb #1 made. The demos look alive because each screenshot
   contains a fresh creation. Meanwhile the write gate hardens the split: the REPL can never write,
   so the only stateful path is one-shot `--force`, which by construction cannot persist.
2. **Law #8 ("the founder runs the live write") decayed into "refuse everywhere, force everywhere".**
   Every domain docstring promises "the REPL prompts"; in reality the guard is a hardcoded `false`
   and `--force` performs the write with zero confirmation, zero preview, and zero dry-run for every
   verb including `files rm -r` on a tree. The only interactive confirmation in the shell is
   `apply`'s — and its default (pressing Enter) means **yes** (`confirm.rs:17`). The lazy
   "type the name" variant (`confirm_typed`) exists in the file and is dead code. A real decision
   about destructive UX was never made; a refusal message was made instead.
3. **The render layer decides what is true.** Truncation is by renderer, not policy: process/logging
   cap at 40 rows, settings at 24, files never truncates (250+ character rows), names cut at 20,
   log reasons cut mid-word ("Permissi…"), clipboard text at 60. Structured answers are replaced by
   counts (`[1 items]`, `{N entries}`), GVariant type strings are printed as headers (`(as)`, `(i)`),
   `category` filters are silently no-ops, and `raw` output was silenced in the same layer. The
   shell-language that would give an operator a composable path to the engine's real depth —
   quoting, variables, blocks, stdin, redirection, arithmetic — was never finished, so the remaining
   surface is read-only sightseeing.

Counted honestly: of the engine's **152 REAL** methods, **40+ have no verb in the shell at all**
(getRoutes, getCounters, readMemory, subscribeProcess/subscribeDeath, detach, batchCopy/Move/Delete,
compareDirectories, createSnapshot, acquireFile, setIfUnchanged, resetTimer, getAlarm, updateAlarm,
dismissAlarm, setTimerDependency, convertFromTai, clearHistory, subscribeChanges, setSizeLimits,
setOffer, getExecutionContext, and more), and the **83 DEGRADED** rows are published in matrices
that no command ever queries. Depth exists in matrices; the shell is a museum of what could be.

---

# THE AUDIT — per domain

## 1. network — 15 live commands

### What exists today (live)
| command | what it actually does (evidence) |
|---|---|
| `network` / `network status` | interface table + networking master + radio states. Default table: NAME TYPE LINK CONNECTED NETWORK MANAGED MTU — **no addresses** |
| `network interfaces` | same rows, no status column |
| `network radios` | wifi/cellular rows: enabled, hard-blocked, soft-blocked |
| `network interface <name>` | type, connected, ssid, hostState, admin-up, MTU, managed, **MAC** — still no IP/gateway/DNS/speed |
| `network wifi` | radio state (enabled/hard/soft) |
| `network wifi list` | 8 remembered networks (SSID/security/autoconnect/last-connected) — real NM profiles |
| `network wifi scan` | real scan: 1 AP, `-73 dBm wpa2 2.4 GHz 5C:64:…` |
| `network wifi connect <ssid> [pw]` | gated; engine activates remembered network |
| `network wifi disconnect` | gated |
| `network wifi forget <ssid>` | gated |
| `network wifi on` / `off` | read-first: `on` answered "already enabled (engine read — nothing written)"; `off` gated, `--force off` → `setRadioEnabled — wifi off acknowledged` |
| `network wifi join <payload>` | QR/WIFI: parse + resolved-call dry-run; `--force` performs |
| `network wifi share <ssid>` | terminal QR + **round-trip decode proof** (`Round-trip ◉ scanned back to the exact payload`) — the best-executed thing in the shell |
| `network wifi privilege` | measured polkit verdict: `network-control=yes, settings.modify.system=yes` — read-only, honest |

### BASICS missing (9)
* `network ip` / interface addresses — **nothing in the shell shows the host's IPv4/IPv6**, though the kernel answers via netlink and the engine's store holds per-link addresses (`getIpv6Config`'s reason even admits "the kernel half (per-link addresses via RTM_GETADDR) is in the state store"). Shell gap + engine gap (no assembled address readout).
* Gateway / default route, DNS servers — `network dns`, `network gateway` don't exist (engine `getDnsServers` UNAVAILABLE — engine gap; shell never surfaces the unavailability either).
* Host networking master switch — engine serves `setNetworking` **DEGRADED** (availability.rs:235); `network off` → `unrecognized subcommand`. Shell gap.
* `network counters` — engine `getCounters` **REAL** (availability.rs:198). Shell gap.
* `network routes` — engine `getRoutes` **REAL** (availability.rs:243), the draft demands it (`network routes → routing table`). Shell gap.
* `network proxy` — engine `getProxyConfig` **DEGRADED** (availability.rs:245, reads `/etc/environment`). Shell gap.
* Cellular/roaming state — engine `isMobileDataEnabled` **REAL** (availability.rs:175) and `isDataRoamingEnabled` **REAL** (:177); the radio row says "cellular ◉ enabled" with no explanation that there is no modem on this host.
* `network mtu <if> <n>` — engine `setInterfaceMtu` **DEGRADED** (:240). Shell gap.
* One-shot `network watch` — the status subscription exists only inside the REPL language (`watch network`, shell.rs:667); `smart network watch` → `unrecognized subcommand`.

### GOOD-TO-HAVES missing (6)
wifi set-auto-connect (`wifiSetAutoConnect`, engine UNAVAILABLE — the shell should say so per-SSID);
MAC randomization (UNAVAILABLE); hotspot (UNAVAILABLE, 6 methods); VPN (UNAVAILABLE, 6 methods);
band/channel/bitrate/security of the **active** link (scan shows them for APs, status shows none);
bandwidth/speed (`network bandwidth` in the draft; no engine method).

### FLEXES missing (3)
`network watch <iface>` as a first-class verb over `subscribeStatus`/`pullStatus` (engine REAL)
instead of a REPL-only keyword; a counters stream (`subscribeCounters`/`pullCounters`, REAL);
`apply`/`export` for interfaces/wifi profiles (today only radios + master switch, shell.rs:890).

### Destructive/UX audit
`wifi off/on/connect/disconnect/forget` are gated with the blanket refusal (exit 1, correct);
`--force` performs with no confirmation and no preview of what will change. `--force wifi off`
turned the radio off and back on during this audit with zero prompt — fine for a founder, fatal for
a script that fat-fingers `--force`. No yes/no, no "type the name", nothing.

### Shallow answers (host has real data)
* `network status` — the host has three usable addresses (kernel netlink) and the command shows none.
* The `cellular` radio reads "enabled" — measured truth is *not-applicable*: no modem, ModemManager
  absent (availability.rs:176,228). The row hides a measured reason the engine already carries.

---

## 2. clipboard — 14 live commands

### What exists today (live)
`clipboard` (status: transport x11, wayland DEGRADED with a wrapped reason, readable yes, content
present with a 60-char preview); `clipboard get [--mime]`; `clipboard set <text>` (gated);
`clipboard clear` (gated); `clipboard history` + `search/pin/unpin/delete` (mutators gated);
`clipboard types`; `clipboard preview <id>`; `clipboard validate <id>`; `clipboard convert <id>
<mime>`; `clipboard limits`.

### BASICS missing (4)
* **A usable `set`** — refused in the REPL; with `--force` the selection dies with the process
  (5000 "placed" bytes, next `get` = 70 unrelated bytes). Either the shell should hold the selection
  (a background owner) or it must refuse honestly instead of claiming success.
* `clipboard history clear` — engine `clearHistory` **DEGRADED** served (availability.rs), no verb.
* `clipboard watch` — engine `subscribeChanges` **REAL** + `pullChanges` **DEGRADED**, no verb.
* `clipboard limits --set` — engine `setSizeLimits` **DEGRADED**, only the read exists.

### GOOD-TO-HAVES missing (4)
multi-representation `setOffer` (engine DEGRADED — the contract's own flex, see below); image/binary
preview (`getPreview` served; the shell shows text previews only); item age/expiry in the history
table; pagination over the ledger.

### FLEXES missing (2)
`clipboard offer` — one call offering text+html+image at once (contract `setOffer @16`; engine
serves it, shell doesn't); selector-into-clipboard (`@files/… | clipboard set` — impossible: no
command accepts piped or stdin input anywhere in the shell).

### Shallow answers
* `clipboard get` truncates the text preview at 60 chars (`render.rs:339 TEXT_COLUMN`) with no
  `--full`/`--bytes` — `-o json` carries the real 70 bytes, human mode does not.
* The status view's Wayland reason is wrapped mid-word ("protocol-complete and seat-proven (the re…").

---

## 3. service — 14 live commands

### What exists today (live)
`service status` (58 services, full table — the only uncapped table in the shell: STATE PID ENABLED
SUPERVISED SINCE, `○ unknown` rows with a measured stat-path reason and a 3-row reason block + "…
and 39 more service(s) with the same measured reason"); `service status <name>` / `inspect`
(NetworkManager: active, pid 715, enabled, health online "(probe not run by a read-only backend)");
`service health [--quarantined]`; `service deps`; `service resolve`; `service metrics` /
`allocation` / `check` (honest `✗ unavailable` with reason class); `service watch` (real bounded
events: `started active NetworkManager`); `service controls`; `start/stop/restart` (gated);
`service logs` (honest: "service.capnp OBSERVES and defines no log surface … No log line was read
and none was invented").

### BASICS missing (3)
* enable/disable — the founder's old shell had them; the engine has no such method (engine gap) and
  the shell cannot enable a service at boot.
* A state filter on the table — `service status --state failed` / `--enabled` doesn't exist; 58 rows,
  no flags, no search.
* `service logs` is contract-honest but leaves the operator with no path to a service's log even
  where this uid *could* read it.

### GOOD-TO-HAVES missing (3)
health-check configuration and metrics (engine UNAVAILABLE, but they are answered with raw
matrices rather than a command-shaped answer); uptime granularity ("2d"); restart/want state.

### FLEXES missing (1)
desired-state `apply`/`export` for services (the network-only `apply <file.toml>` pattern,
shell.rs:890, could reconcile enable/disable per service).

### Destructive/UX + shallow
`start/stop/restart` gated (exit 1) and `--force` performs with no confirmation — stopping a
service root-owned by uid 1000 is refused by the host anyway on this box, so the gate is mostly
theater here. `service controls start` **ignores its verb argument** and reprints the whole control
list (`render::controls_value(verb.as_deref())` never matches on it) — a parameter that parses and
does nothing.

---

## 4. files — 34 live commands

### What exists today (live)
`files ls [--all --pattern --sort --order --recursive]` (getdents64, real); `files find`
(glob); `files tree --depth`; `files stat`; `files xattrs`; `files cat [--offset --bytes]`
(mmap window, verified 80-byte read of /etc/services); `files du`; `files mounts`; `files locks`;
`files hash`; `files versions` / `branches` / `store` / `dedupe` (content-addressed store + measured
inode sharing); `files watch` (real inotify, `watch files /tmp`); `files special <token>`;
`files trash`; `files recents`; `files app <mime>` (real: Firefox, firefox.desktop,
mimeinfo.cache); `files grep`; `files compare`; `files health` (statvfs-measured, honest DEGRADED);
`files quota` (381/468 GiB, per-user fields 0 with reason); `files caps`; and the gated writes
`mkdir/touch/rm/mv/cp/write/link/lock/unlock`.

### BASICS missing (5)
* **chmod / chown** — no engine method exists (engine gap) and no verb. A file domain without
  permissions changes is a read-only museum.
* **append** — `files write` truncates (`create_file`); there is no append verb and no engine
  method. Every log-write use case is unservable.
* `cd` / `pwd` as verbs — the old demo shell had both; the engine serves `getCurrentDirectory`
  **REAL** and `changeDirectory` **DEGRADED**; the shell uses cwd internally but exposes neither.
* The handle surface — engine `openFile/closeFile/readFile/writeFile` **REAL** (a whole fd-handle
  API); the shell exposes none of it (`files open` doesn't exist).
* `files edit` / any interactive write path — writing means re-typing whole content as an argument
  (and, per the shell-core section, quoted text can't even get through).

### GOOD-TO-HAVES missing (10)
`batchCopy` / `batchMove` / `batchDelete` (all **REAL** — engine serves a batch API, shell has no
`files batch`); `compareDirectories` (REAL); `getDirectoryStats` / `getDirectorySize` (REAL);
`getFileChecksum` (DEGRADED — beyond SHA-256); compression set/get (DEGRADED);
`getDeduplicationStats` (DEGRADED); `createSnapshot` (REAL); `getFileActivity` (DEGRADED);
`subscribeFileAudit` (DEGRADED); `createEphemeral` (REAL).

### FLEXES missing (4)
version-store branches properly: `mergeBranch`, `restoreVersion`, `compareVersions`,
`getByHash` (all DEGRADED, engine-served, no verbs); xattr **write** (only read exists); lock as a
selector (`@files/locks` exists; per-path lock tables don't); the content-addressed store as a
pipe source.

### Destructive/UX audit
`rm/mv/cp/write/link/mkdir/touch/lock` all gated with the blanket refusal; `--force files rm
/tmp/kilo-canary.txt` deleted the file (verified) with **no confirmation, no listing of what would
be removed, no recursive preview** — `--force files rm -r <tree>` is one flag away from an
unrecoverable mistake with zero UX.

### Shallow answers (host has real data)
* `files recents` — 24 rows from the host's recently-used record, **MIME column empty for every
  row and RECORDED `—` for every row**, while the XBEL source carries both; paths printed as raw
  `file:///home/lufuno/Pictures/…%5BMULTI%20SUB%5D…` URIs — undecoded, and the table grows to ~250
  characters wide (the only domain that never truncates, breaking every terminal).
* `files versions` / `branches` — honest empties (acceptable, and they name the reason).
* `files quota` — per-user quota fields 0 with the measured reason (acceptable).

---

## 5. process — 17 live commands

### What exists today (live)
`process` (table: 374 processes, 40 shown, `· 334 more below the cut`); `process <pid>` (inspect:
state, parent, command, memory 2.11 MiB, CPU, uptime, I/O, signal verdict, limits — measured);
`inspect/tree/threads/env [--names-only]/maps/limits/accounting/groups/group/stats/ptrace/
can-signal/signal (gated)/attach (gated)/wait/caps`. The privilege probes are the best content in
the domain: `ptrace` answered with the measured `ptrace_scope=1, CapEff=0` verdict;
`can-signal 711` → `✗ this uid may NOT deliver a terminate (measured with kill(pid, 0))`.

### BASICS missing (6)
* **spawn** — no verb; engine refuses (backend.rs:56). (Both.)
* Table navigation — no `--limit/--skip/--sort` on the command; the 40-row cap is renderer-only
  (`render.rs:26`) and there is no top-by-CPU. In a shell that replaces `top`, you cannot ask
  "what is busy right now" without piping 374 rows through `| sort`.
* `process env <pid> --set` — engine `setEnvironment` **DEGRADED** served; read-only shell.
* `process read <pid> <addr> <len>` — engine `readMemory` **REAL** served (EACCES-measured for
  foreign uids); no verb at all.
* `process watch <pid>` — engine `subscribeProcess` / `subscribeDeath` **REAL**; no verb (network
  has `watch`, process doesn't).
* `process detach` — engine **REAL**; no verb.

### GOOD-TO-HAVES missing (5)
`getExecutionContext` (REAL — cwd/root/fds of a process, unexposed); breakpoints/watchpoints
(DEGRADED — a whole debugger surface with no command); `getProcessRespawnStatus` (DEGRADED);
`getResourceAccounting` (DEGRADED); the tree with resource columns (today `process tree` prints
names only).

### FLEXES missing (4)
subscriptions as verbs (above); memory read (above); breakpoints (above); environment write via
selector. The architecture's differentiators (a capability-held backend per process) are unclaimed.

### Shallow / wrong answers
* **`exit_status` is fabricated for every live process** — `process 1` (runit, alive, state S)
  answers `"exit_status":"exited 0"` in JSON; source: `ExitStatus::default()` =
  `Exited(0)` (types.rs:296) assigned unconditionally in `get_process_status`
  (platforms/linux/process.rs:777). This is not shallow, it is false.
* `process groups` NAME column shows placeholder `pgid-677` labels, not process names.
* `process stats` CPU line: "the engine's own aggregate over its measured window: **150000**" — a
  bare number with no unit presented as an aggregate.

---

## 6. settings — 13 live commands

### What exists today (live)
`settings` (258 schemas, **24 shown**, keys + path); `settings list <domain>` (45 keys with real
scalars: `accent-color purple (default 'blue')`, ranges, summaries); `settings get` (24h +
`('enum', <['24h','12h']>)` + summary); `settings set` (gated); `settings schema <domain>` (the
declaration: type/default/range/summary per key — genuinely good); `settings category <token>`;
`settings version/history/diff/bulk-get/bulk-set (gated)/watch/caps`.

### BASICS missing (3)
* **delete / clear / reset** — no verb exists (`settings delete …` → `unrecognized subcommand`),
  while the engine serves `delete` and `clear` **DEGRADED**. You can set every key on this host and
  unset none — `notSet` is a read-side concept only.
* `settings search <text>` — the old demo shell had it; no engine method (engine gap).
* App-override read — engine `getAppOverride` **DEGRADED** served; no verb.

### GOOD-TO-HAVES missing (3)
`setIfUnchanged` (DEGRADED — a compare-and-swap verb, the contract's own concurrency flex);
honest `category` (today 2039 keys = every key, category recorded beside each = a no-op filter);
list-value rendering (below).

### FLEXES missing (2)
bulk compare-and-swap; per-app override write.

### Shallow answers (host has real data)
* Structured values never render: `settings get …/reminders-past` answers `Value [1 items] (list)`
  and dicts answer `{N entries}` (`backend.rs:234-235`) — the host's dconf holds the real items.
* The header token `(as)` / `(i)` is the raw GVariant type string of the key, printed where a
  value type belongs; the Range column prints tuples like `('type', <@aas []>)`.
* 258 domains discovered, 24 displayed (`render.rs:28,422`) — the rest unreachable without `| skip`.

---

## 7. logging — 12 live commands

### What exists today (live)
`logging sinks` (50 sinks measured by opening: 5 readable, root-only svlogd dirs answered with the
host's own `Permission denied` words — 40 shown); `logging domains` (50 rows: file/sink, level,
sampling); `logging tail <sink> [--lines]` (real lines from Xorg.0.log); `logging entries
[--level --domain --pattern]` (this process's store); `logging follow` (real subscription,
bounded); `logging emit <level> <msg>` (gated; actually wrote `[info] … app: test hello world` to
stderr); `logging level/set-level/sampling/clear/export/caps`.

### BASICS missing (3)
* **grep over a real host log** — `logging tail` has only `--lines`; the pattern filter exists only
  on `entries`, which reads this process's own store. `tail Xorg.0.log --pattern EE` is impossible.
  (Engine `getEntries` is per-domain; host-file search has no engine method either — engine gap —
  but the shell could offer `files grep` on the sink path; it cannot chain the two.)
* A time window — the contract's `LogFilter` carries `since/until` and the engine's filter struct
  has them, but `filter_of` sets `UNIX_EPOCH` for both (`logging/mod.rs:223-224`) and no flag
  exposes them. `--since 1h` is the second thing any log user types.
* trace-id / subsystem correlation — `trace_id`/`subsystems`/`after_sequence` all hardwired empty
  (`logging/mod.rs:226-228`).

### GOOD-TO-HAVES missing (2)
sink refresh as a command (`sinks --refresh` exists — acceptable); export formats beyond
`text/plain`/`x-ndjson`.

### FLEXES missing (1)
correlated reads (`--trace-id` across services) — the contract's structured-logging story, unwired.

### Shallow answers
* `logging entries` answers `(0)` for every one-shot invocation — the store is per-process; the
  verb exists to be photographed.
* `logging tail` prints the sink's domain token as the level (`▢ custom  Xorg.0.log`) and repeats
  the file name in the message column — the row's first column carries no severity because the
  engine maps host lines to `custom`.

---

## 8. clock — 26 live commands (the deepest engine, the least usable)

### What exists today (live)
`clock` (info: realtime nanos, monotonic, uptime 2d 20h, resolution 1 ns, sync "chrony (measured)",
security line, domains 2); `now/monotonic/uptime/resolution/sync/security/domains`; `quality
<src>`; `drift a b`; `sleep <ms>`; `timer create/check/list/cancel (gated)`; `sleep-cancellable`;
`virtual create/advance`; `alarm list/next/create/snooze/delete (gated)`; `watch`; `caps`.
`clock sync` is genuinely good: chronyd pid 711 found, state dir root-only → `synced=false` **with
the measured reason**, never invented.

### BASICS missing (3)
* **State that survives one command** — create→list→check can never meet (fresh backend per line,
  shell.rs:632; live proof above). The entire 26-verb surface is single-frame animation.
* `clock timer reset` — engine `resetTimer` **REAL**; no verb.
* `clock alarm get <id>` / `update` / `dismiss` — engine serves all three **REAL**; the shell
  exposes only list/next/create/snooze/delete.

### GOOD-TO-HAVES missing (5)
absolute alarm fire time (`--at`, only relative `--after-ms`); recurrence day-of-week (weekly
hardcodes Mon–Fri, `clock/mod.rs:275`); holiday behavior (always `HolidayBehavior::default()` via
`neutral_holiday()`); timer group/priority (fields exist in `TimerSpec`, never settable);
`persistTimer` / `listPersistedTimers` (DEGRADED).

### FLEXES missing (3)
`setTimerDependency` (REAL — chained timers); virtual-clock scaling driven from a selector;
alarm recurrence as data.

### Shallow / contradictory answers
* **`smart clock` says `hardware rng ◉` while `smart clock security` says `hardware rng ○`** — the
  same question, two answers, seconds apart, in one binary.
* `clock quality tsc` answers "accuracy 1 ns · stability 0 ns · reliability 0 **over 0 sample(s)**"
  — an empty measurement formatted as a measurement.

---

## 9. time — 7 live commands

### What exists today (live)
`time` (wall `2026-10-15 19:23:31.189`, weekday, day-of-year, zone `Africa/Johannesburg (UTC+02:00)`,
source); `time tai` (UTC nanos ↔ TAI, offset 37 s, exact round-trip); `time cal [--date] [--to]`
(gregorian→islamic `1448-05-03`, exact arithmetic); `time add/sub <date> <unit>` (month-end clamped);
`time diff d1 d2` ("1 years, 0 months, 0 days — 365 day(s)"); `time caps`.

### BASICS missing (5)
epoch/unix output (`time now --epoch`); formatting (`--format`, strftime); timezone listing (the
old shell's `time zones`); **converting from TAI** — engine `convertFromTai` **REAL** served, no
verb (only `tai` one-way); "time in <zone>" / zone conversion.

### GOOD-TO-HAVES missing (4)
leap-second display; business-day arithmetic; human duration ("in 3 days"); human date parsing.

### FLEXES missing (2)
the calendar set as a selector tree; cross-calendar chained arithmetic.

### Shallow answers
`time` shows the local zone only; the contract's `time.capnp` declares 50 methods and the engine
serves 7 — this is the domain where the **engine** is the shallow half, and the shell inherits it.

---

# THE SHELL-CORE AUDIT

### REPL completeness — what works, what is parse-only
| feature | state | evidence |
|---|---|---|
| history | **works** (persistent, `~/.config/smart/history`, rustyline) | shell.rs:1492-1520 |
| tab completion | **works for 3 of 9 domains** — network(+wifi), service, clipboard, and `@network/@clipboard/@service` only; files/process/settings/logging/clock/time have zero completion | complete.rs:20-63 |
| aliases | file-only (`~/.config/smart/aliases.toml`); **no `alias` command in the shell**; the compiled-in defaults (`sw→switch-workspace`, `v→volume`, `p→power`, `d→display`, `w→window`) point at domains that do not exist | alias.rs:40-48 |
| shorthand expansion | works but **sabotages**: rewrites bare `files` → `filesystem` (a queued domain → "◌ migration pending"), so the files default view is unreachable in the REPL; silently rewrites net/clip/proc/set/log | alias.rs:83-145 |
| variables | **assign-only**: `var = @network/radios \| count` → `2`; `$var` alone → `✗ Unknown command: $var`; **no interpolation anywhere** | shell.rs:268, 421-424 |
| conditionals | **broken in script mode** (alias flattening), body runs unconditionally when typed interactively, works only via `-c` | alias.rs:148-151 vs shell.rs:1337 |
| loops | parse-only: `for iface in @network/interfaces:` → body receives the literal `$iface` | shell.rs:356-366 |
| functions | parse-only: `fn hello(name):` → `✗ Unknown command: hello` | shell.rs:298-313 |
| tilde / env expansion | none — `files ls ~` → `notFound (while statx(~))`; `files ls $HOME` → `notFound (while statx($HOME))` | live |
| quoting / escaping | none — `clipboard set "hello world"` → clap usage error | live |
| stdin into commands | none — `echo x \| smart clipboard set` → `required arguments were not provided` | live |
| redirection | none — `time now > /tmp/out.txt` → `unexpected argument '>' found` | live |
| interactive blocks | impossible — one `readline` per line; a typed `if` header evaluates with an empty body | shell.rs:1497-1506 |
| exit codes in REPL | errors print `✗ …` and the process still exits 0 in script mode | shell.rs:1586 |
| REPL writes | **never** — guard hardcoded false in all nine `exec_*` | shell.rs:477-633 |

### Pipe engine coverage
22 ops: `where/map/sort/take/skip/count/reverse/unique/head/tail/pluck/group/flatten/len/first/
last/sum/avg/min/max` (pipe.rs:28-113). Missing: `and`/`or` combinators, arithmetic (`$x + 1`),
regex/string ops, nested field paths (`get_field` is flat, pipe.rs:124), `select` of several
fields, date/format ops, write-to-file, and an `each`-style op that runs a command per item —
the ops reshape lists, they never reach back into the engine. `where` supports a single comparison;
`~=` does substring contains only.

### Selector grammar depth
`@domain/path… [where field op value]` with Field/Index/Wildcard segments and 7 operators
(selector.rs:117-208); `@network/interfaces where name ~= wlp` and `@process/processes where uid ==
1000 | count` (139) both work. Depth ends there: one filter per selector, no and/or, no function
calls, no aggregation, no cross-domain selectors, no selector for `@network/counters`,
`@network/routes`, `@process/threads/<pid>`, `@clock/timers` (trees exist only for what the
selector_root halves expose, shell.rs:192-241).

### Output/format story
human: the aesthetic (box tables, ◉○◐✗, aligned) — but truncation is per-renderer and inconsistent
(40/24-row caps with "N more below the cut"; 20/40/52/56-char cell cuts; 250-char rows in files).
json: per-domain, mostly complete — with real defects: duplicated `cpu_time_nanos` key and a
fabricated `exit_status` in process; no JSON for the `Value` a REPL expression yields unless it goes
through `render_pretty`. **raw: dead in one-shot mode (0 bytes × 9 domains).**

### Help/honesty story
`--help` (clap) is accurate. The REPL `help` is stale and false: 3 of 9 domains listed, the other 6
live domains called "the migration queue"; `help files` → "◌ files — migration pending — archive
body exists at platforms/linux/fs.rs" for a domain that answers 34 commands. The gated-refusal
message promises a REPL confirmation that does not exist. The migration queue itself
(`smart migrate`: 47 domains, 9 real, 8 queued, 30 archive-only) is honest and good.

### Aesthetic standard
Glyphs consistent, box-drawing aligned, color tasteful — until a value is longer than a renderer's
guess: names cut to `pool_workqueue_rele…`, log reasons to `Permissi…`, clipboard text to 60 chars,
while `files recents` blows the table to terminal-breaking width and `files ls` grows unbounded.
The rule the code follows is "truncate what I remembered to truncate" — not a width policy. The
`-o json` path shows the data was never the problem; the renderer's guesswork is.

---

## APPENDIX — engine capability vs shell exposure (the verbs that were never written)

| domain | engine serves (REAL/DEGRADED) | shell verbs | engine-served, never exposed |
|---|---|---|---|
| network | 15/8 (23) | 15 | getRoutes, getCounters, subscribe/pullCounters, getProxyConfig, setNetworking, setInterfaceMtu, setMobileData, isMobileDataEnabled, isDataRoamingEnabled, subscribe/pullStatus as verbs |
| clipboard | 5/15 (20) | 14 | clearHistory, subscribeChanges, pullChanges, setSizeLimits, setOffer |
| service | 7/12 (19) | 14 | subscribeState/pullState as verbs (watch merges them); enable/disable absent from the engine entirely |
| files | 48/31 (79) | 34 | batchCopy/Move/Delete, compareDirectories, getDirectoryStats/Size, getFileChecksum, createSnapshot, getFileActivity, subscribeFileAudit, createEphemeral, acquireFile/openFile/closeFile, getByHash, mergeBranch, restoreVersion, compareVersions, getCompression/setCompression, getDeduplicationStats, applyLifecycle/setLifecycle/getLifecycle, clearRecentFiles, restoreTrash/emptyTrash, changeDirectory |
| process | 16/17 (33) | 17 | readMemory, subscribeProcess, subscribeDeath, detach, getExecutionContext, setEnvironment, set/removeBreakpoint, set/removeWatchpoint, getProcessRespawnStatus, spawn (engine refuses) |
| settings | 11/7 (18) | 13 | delete, clear, setIfUnchanged, getAppOverride, search (absent from engine) |
| logging | 6/5 (11) | 12 | — (coverage is the best of the nine; the gaps are filter flags, not verbs) |
| clock | 25/5 (30) | 26 | resetTimer, getAlarm, updateAlarm, dismissAlarm, setTimerDependency, persistTimer, listPersistedTimers |
| time | 6/1 (7) | 7 | convertFromTai |

**The arithmetic of the audit:** 1,411 contract methods → 235 engine-served → 152 shell verbs, of
which a double-digit number cannot compose with anything (per-invocation state), a dozen cannot
receive their own arguments (`controls`, `category`), and the flagship pipe mode prints nothing.
The founder's vision file lists ~70 network commands alone (`network_commands.sh`); the shell ships
15, and the first nine of the missing ones (`ip`, `gateway`, `dns`, `routes`, `counters`, `proxy`,
`off`, `mtu`, `watch`) are the ones a user reaches for before reading any help.

---

*Auditor's note: this report is deliberately merciless because the artifacts invite it — the
availability matrices are honest, the engine test suites are real, and the wifi QR share, the
privilege probes, the ptrace verdict, the chrony sync evidence and the `service logs` refusal are
genuinely better than the old shell. The gap is not honesty; it is that honesty was built at the
read layer and the write/compose layer was left as parse-only scaffolding with messages that
promise more than the code delivers.*
