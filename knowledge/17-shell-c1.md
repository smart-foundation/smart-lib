# 17 — C1: the Smart Shell comes home to the working stack

> The convergence's first slice. A `shell` crate in the engine's workspace
> produces the `smart` binary: the old shell's engine-agnostic core, salvaged;
> the network domain rendered from the engine that already works; every other
> domain presented as an explicit migration queue, never as fakes.
> Live-proven on this host, read-only, no daemon, no old shell.

## 1. The crate layout

```
smart-lib-ng/
  Cargo.toml                         # + members = ["crates/shell"] (one line)
  crates/shell/
    Cargo.toml                       # bin `smart` + lib `shell`; default = domain-network
    src/
      lib.rs                         # modules + OutputFormat (Human|Json|Raw, as before)
      main.rs                        # clap front-end: network + migrate; bare `smart` → REPL
      shell.rs                       # REPL + AST + evaluator (REPAIRED — see §3)
      selector.rs                    # selector grammar + network evaluation (REPAIRED tail)
      pipe.rs                        # the 20-op pipe engine (group-order repaired)
      migrate.rs                     # the migration queue AS DATA (new)
      domains/network/mod.rs         # the network command module on the new engine (new)
      domains/network/render.rs      # the aesthetic over the engine's OWNED types (new)
      value.rs                       # pretty render + escaped JSON (replaces the old
                                     #   quoting-unsafe hand-rolled JSON) (new)
      widgets.rs                     # make_bar + the box-drawing table (new)
      error.rs                       # the shell's error over core::Error (new)
      alias.rs color.rs confirm.rs fuzzy.rs installer.rs progress.rs
      history.rs complete.rs         # salvaged (complete.rs repaired + WIRED)
    tests/
      common/mod.rs                  # the scripted NetworkBackend (never the host)
      shell_language.rs              # 17: parser (if/elif/else, for, fn, assign, watch) + eval
      pipes.rs                       # 4: all 20 pipe ops + the substring where + errors
      selectors.rs                   # 6: grammar, paths, filters, honest pending answers
      network_cmd.rs                 # 13: the command surface + gated control verbs + JSON
      migrate.rs                     # 6: the queue's data invariants
```

The engine seam is the corrected architecture's: the shell embeds its engine
**in-process** through the per-domain backend trait object
(`Box<dyn NetworkBackend>` — `crates/shell/src/domains/network/mod.rs:29`),
exactly as the original shell embedded its library through the backend trait
object. No socket on the local path; capnp nowhere on the local path.

The feature matrix works: `cargo check -p shell --no-default-features -j6`
compiles, and with the domain off every network answer is an honest
"the shell was built without the domain-network feature" (never a fake).

## 2. The salvage map

**Copied as-is** (engine-agnostic, zero schema references — the salvage the
dispatch predicted would copy cleanly):
`alias.rs`, `color.rs`, `confirm.rs`, `fuzzy.rs`, `installer.rs`,
`progress.rs`, `history.rs`; and the grammar half of `selector.rs`
(`Value`, `SelectorSegment`, `Filter`, `parse`, `parse_filter`,
`parse_value`, `get_field`, `apply_filter`, `resolve_path`) and all of
`pipe.rs`'s twenty operations.

**Repaired while porting** (the known half-real parts):
- `complete.rs` — the completer was BUILT and never wired. It is wired now:
  `ShellHelper` implements rustyline's `Completer` and `run_shell` installs it
  (`shell.rs:1035`). Its data is trimmed to the real surface: help and
  completion list what is real, never what is fake. Unit-tested (4 tests).
- `shell.rs` — (a) **`elif`/`else` dispatch**: the old parser built
  `If { elifs: vec![], else_body: vec![] }`, so they parsed and silently did
  nothing. The block parser fills them (`parse_if_block`) and the evaluator
  dispatches them — and a conditional now yields its taken branch's last
  value, so the dispatch is observable. (b) **`watch`**: was a sleep(1s)
  print loop; it is the engine store's real event stream — subscribe per
  interface, pull per round, the store revision is the event, `--count N`
  bounds it. (c) **`apply`/`export`**: were a hard-coded `audio.volume` and an
  ignored path; both are real over the engine with a real `[network]` section
  (reconcile: read-first, engine call only where the desired state differs).
  (d) the dead `Expr::Pipe` branch is **live**: `var = @network/interfaces |
  count` evaluates through the same evaluator as a command pipe.
  (e) the comparison grammar: the old `parse_expr` turned
  `@x/level < 20` into a selector with a garbage field name, so a comparison
  against a selector never compared; the expression parser now scans the
  comparison around the (piped) selector, and a `where` filter's `==` is the
  selector's own syntax, not the expression's. (f) the one-line
  `for …: body` parse lost its body. (g) the host-write gate (law #8):
  mutating verbs proceed only on `--force` (one-shot) or a terminal
  confirmation (REPL); a pipe never confirms. (h) a non-TTY stdin runs the
  same evaluator line-by-line (script mode), so the language is provable.
- `pipe.rs` — `group` used a `HashMap`, so the same input grouped in an
  arbitrary order between runs; first-seen order now (stable output is part
  of the aesthetic).
- `widgets.rs` — `make_bar` (from the old `cmd/mod.rs`) plus the box-drawing
  table every renderer draws with; salvaged from the old table style.

**Deleted** (not copied): the dead second REPL (`src/repl.rs`); the
`build_capnp!` macro and `capnp_text_lossy` with their `Box::leak`
pathology (the network module renders the engine's owned types — nothing
leaks, nothing desynchronises); the god-trait `SmartBackend` references; the
46 fake domain arms; the hand-rolled quoting-unsafe JSON (replaced by
`value::to_json`, which escapes — test-locked).

**New**: `error.rs`, `migrate.rs`, `value.rs`, `widgets.rs`,
`domains/network/**`, `main.rs`, `lib.rs`, the test suites.

## 3. The network rendering, as built

Every renderer reads the engine's OWNED types — `InterfaceSnapshot`,
`ConnStatus`, `RadioState`, `KnownWifiNetwork`, `WifiNetwork`, `Page`,
`NetworkSummary` — never a capnp `Reader<'static>`. The command surface:

| command | what it renders |
|---|---|
| `network` | the interface table: `🌐 Network` / state / NAME TYPE LINK CONNECTED MANAGED MTU |
| `network status` | the master switch, radios (hard+soft separate, never folded into enabled), interfaces with `hostState` + its kernel/nm-policy source |
| `network interface <name>` | type, connected, hostState, admin-up, MTU, managed, MAC; `up`/`down` answers honestly that this backend exposes no such verb |
| `network wifi list` | the remembered networks — the engine's real profile read; an honest empty when there are none |
| `network wifi scan` | a real engine call; the engine's refusal (not built, polkit-gated) is the answer |
| `network wifi connect/disconnect/forget` | real engine calls, gated |
| `network wifi on/off` | **real engine calls** (`setRadioEnabled`) — the old fake arms; read-first, so an already-set radio writes nothing |
| `network radios` / `network interfaces` | the radio / interface tables |
| `@network/*` | the same data as selector values (`interfaces`, `radios`, `known`, `status`, `networking`) |
| `watch network [--count N]` | the store's real bounded events |
| `apply` / `export` | the real `[network]` section, reconciled/observed over the engine |

## 4. The migration queue (as data)

46 domains: **1 real today** (network), **12 queued** (the recon's value
order), **33 archive-only**. `smart migrate` renders it; the honest answers
an unmigrated domain gives are generated from the same table
(`migrate::pending_reason`), so the queue and the answers can never disagree.

| # | domain | status | recon count | archive body |
|---|---|---|---|---|
| ◉ | network | **real-today** | real engine | netlink + NM, live-proven |
| 1 | clipboard | queued | 3/3 methods | `platforms/linux/clipboard.rs` (real, 295) |
| 2 | queue | queued | 8/8 methods | `platforms/linux/queue.rs` (real, 60) |
| 3 | service | queued | 8/8 methods | `platforms/linux/service/` (systemd, openrc, sysvinit, runit) |
| 4 | media | queued | 8/13 | `platforms/linux/media.rs` (real, 95) |
| 5 | database | queued | 8/14 | `platforms/linux/database.rs` (real, 99) |
| 6 | files | queued | 8/17 | `platforms/linux/files.rs` (real, 110) |
| 7 | audio | queued | 6/7 | `platforms/linux/audio.rs` (real, 173) |
| 8 | power | queued | 4/7 | `platforms/linux/power.rs` (real, 294) |
| 9 | process | queued | 4/13 | `platforms/linux/process.rs` (real, 102) |
| 10 | window | queued | 10/36 | `platforms/linux/window.rs` (real, 412) |
| 11 | package | queued | 19 managers | `platforms/linux/package/` (apt…zypper) |
| 12 | compositor | queued | 9 backends | `platforms/linux/compositor/` (sway…xfwm) |
| · | 21 more | archive-body-exists | real bodies | display 246, filesystem 103, web 124, app 106, … |
| · | 12 more | archive-body-exists | placeholders | `available() = false` bodies |

**The order's rationale** (the recon's): the domains whose old bodies were
real end-to-end go first (clipboard 3/3, queue 8/8, service 8/8 — a whole
body is a whole wave); then the partial bodies by real-method ratio
(media 8/13, database 8/14, files 8/17); then the thin ones (audio 6/7,
power 4/7, process 4/13); then the multi-backend domains, each of which is
its own wave (window 10/36 across four WMs, package's 19 managers,
compositor's 9 backends). Value order, one dispatch per domain.

## 5. Bug 1: root cause and fix

**Symptom (the live verification's):** `get_status` typed `wlp0s20f3` as
`ethernet` while `get_interfaces` typed it `wifi`.

**Root cause — the two paths disagreed on the type's SOURCE, one level down
than it looked:** the kernel reports a wlan interface's `ARPHRD_*` link type
as `ether` — the wireless-ness lives in the phy, which rtnetlink does not
carry — so `RealNetlinkReader::snapshot()` upgrades a link's kind from the
kernel's own sysfs marker (`/sys/class/net/<name>/wireless`, via
`enrich_link`). The EVENT path (`parse_event_datagram`) parsed the wire only.
An `RTM_NEWLINK` event for the wifi interface therefore carried
`ARPHRD_ETHER`, and the store applied it **in place**, replacing the
enriched `wifi` record with an un-enriched `ethernet` one. A read served
from the snapshot said `wifi`; a read served from the event-applied record —
`getStatus`, `pullStatus`, and my `watch` surface — said `ethernet`. The
committed probe `live_snapshot_and_event_agree_on_a_links_kind` names the
two surfaces; it had passed only because the drain found no queued events at
that moment.

**Fix** (`src/platforms/linux/netlink.rs`): `RealNetlinkReader::parse_events`
— the one place the reader turns datagrams into link events — now applies
the same `enrich_link` the snapshot applies. One source for the type on both
surfaces: the kernel's link type, plus the kernel's own sysfs marker.

**Proof:** hermetic — `event_links_report_the_same_kind_the_snapshot_would`
(a temp sysfs tree; the wlan event link enriches to `wifi`, the wired one
stays `ethernet`) and `status_and_enumeration_agree_on_the_type` (every
enumerated interface's `getStatus` kind equals its `getInterfaces` kind);
live — `live_reads_networkmanager` now asserts per-interface agreement and
prints both surfaces, and §6's `smart network` / `watch network` show
`wlp0s20f3 wifi` on both paths.

**The @78 wire fix (same file, same slice — the surface depends on it):**
the hand-rolled D-Bus unmarshaller counted the array's *pre-element
padding* inside the length field and parsed the payload's *tail padding* as
a phantom dict entry, so the live `a{sa{sv}}` GetSettings reply
(`listKnownWifiNetworks`) desynced off the end of the message. Fixed per the
spec (the length excludes the padding after the length field; the payload is
padded to the element alignment, so the trailing padding is not an element),
plus STRUCT 8-alignment, plus the writer side made symmetric. Locked by
`dbus_settings_reply_in_the_daemons_convention_parses` (a dbus-daemon-shaped
reply built byte-for-byte, including the empty-trailing-setting shape that
failed live) and proven live: the known-networks read parses (this host: 10
NM profiles, 0 remembered wifi networks — an honest empty).

## 6. The live proof (read-only; nothing on this host was changed)

```
$ cargo build -p shell -j6 && ./target/debug/smart network
  🌐 Network
  ├─ State    ◉ Online
  └─ Interfaces
  ┌───────────┬──────────┬──────┬───────────┬─────────┬───────┐
  │ NAME      │ TYPE     │ LINK │ CONNECTED │ MANAGED │ MTU   │
  ├───────────┼──────────┼──────┼───────────┼─────────┼───────┤
  │ enp0s31f6 │ ethernet │ ↑↓   │ ○         │ ◉       │ 1500  │
  │ lo        │ loopback │ ↑↓   │ ○         │ ◉       │ 65536 │
  │ wlp0s20f3 │ wifi     │ ↑↓   │ ◉         │ ◉       │ 1500  │      ← bug 1 fixed
  └───────────┴──────────┴──────┴───────────┴─────────┴───────┘

$ ./target/debug/smart network status
  🌐 Network Status
  ├─ Networking  ◉ enabled
  ├─ Radios
  │  ├─ wifi      ◉ enabled hard=no soft=no
  │  └─ cellular  ◉ enabled hard=no soft=no
  └─ Interfaces
     ├─ enp0s31f6        ethernet  ○ hostState carrier-down (kernel)
     ├─ lo               loopback  ○ hostState managed (kernel)
     └─ wlp0s20f3        wifi      ◉ hostState managed (kernel)     ← bug 1 fixed

$ ./target/debug/smart network interface wlp0s20f3
  🔌 wlp0s20f3
  ├─ Type          wifi
  ├─ Connected     yes
  ├─ Host state    managed (kernel)
  ├─ Admin up      yes
  ├─ MTU           1500
  ├─ Managed       yes
  └─ MAC           98:59:7A:E4:A0:F2

$ ./target/debug/smart network wifi list
  📡 Known Networks
  └─ ○ — the engine enumerated no remembered networks (an honest empty)
      (live probe: 10 NM profiles, 0 remembered wifi networks)

$ ./target/debug/smart network wifi scan
  smart: notSupported: no-backend: active WiFi scan orchestration
  (NM RequestScan + AccessPoint enumeration) not implemented    ← the engine's honest answer

$ ./target/debug/smart network wifi off
  smart: refusing to run `wifi off`: it CHANGES host state. In the REPL confirm
  the prompt, or pass --force … (the live bring-back proof is founder-run).

$ printf '@network/interfaces\nnetwork | where type == wifi\n…' | ./target/debug/smart
  ┌───────────┬──────────┬──────┬───────────┬───────┬─────────┬───────────────────┐
  │ name      │ type     │ up   │ connected │ mtu   │ managed │ mac               │
  ├───────────┼──────────┼──────┼───────────┼───────┼─────────┼───────────────────┤
  │ enp0s31f6 │ ethernet │ true │ false     │ 1500  │ true    │ 08:8F:C3:E1:21:71 │
  │ lo        │ loopback │ true │ false     │ 65536 │ true    │ 00:00:00:00:00:00 │
  │ wlp0s20f3 │ wifi     │ true │ true      │ 1500  │ true    │ 98:59:7A:E4:A0:F2 │   ← @network/interfaces
  └───────────┴──────────┴──────┴───────────┴───────┴─────────┴───────────────────┘
  ┌───────────┬──────┬──────┬───────────┬──────┬─────────┬───────────────────┐
  │ name      │ type │ up   │ connected │ mtu  │ managed │ mac               │
  │ wlp0s20f3 │ wifi │ true │ true      │ 1500 │ true    │ 98:59:7A:E4:A0:F2 │   ← network | where type == wifi
  └───────────┴──────┴──────┴───────────┴──────┴─────────┴───────────────────┘

$ printf 'watch network --count 3\n…' | ./target/debug/smart
  ⟳ watching network — 3 interface(s), 3 event(s) bounded (Ctrl+C to stop)
  ⟳ event 1 (0 so far)
     ├─ enp0s31f6        ethernet  ○ hostState carrier-down (kernel)
     ├─ lo               loopback  ○ hostState managed (kernel)
     └─ wlp0s20f3        wifi      ◉ hostState managed (kernel)      ← bug 1 fixed on the
  ⟳ event 2 (3 so far)                                                                 subscription path
     … (same three, real store pulls)
  ⟳ event 3 (6 so far)
     …

$ printf 'apply /tmp/kilo/network.toml\n' | ./target/debug/smart
  ⚙ Applying /tmp/kilo/network.toml [network]...
  ├─ networking  ◉ on (engine read — satisfied)
  ├─ radio.wifi  ◉ on (engine read — satisfied)
  └─ ✓ nothing to write — the engine reads satisfy every entry

$ printf 'export /tmp/kilo/proof-export.toml\n' | ./target/debug/smart
  ✓ exported network state → /tmp/kilo/proof-export.toml
  # [network] networking = true; radios wifi/cellular with block states

$ ./target/debug/smart migrate           # the queue: 46 domains — 1 real, 12 queued, 33 archive-only
  🧭 Migration Queue … (the table of §4) …

$ ./target/debug/smart -c 'migrate | where status == migrated-pending | count'
  12

$ ./target/debug/smart -c '@clipboard/content'
  ✗ migration pending — archive body exists at platforms/linux/clipboard.rs
    (real body, 295 lines) (3/3 methods); see `smart migrate`
```

Engine live probes (read-only), all four green after the fixes:

```
$ cargo test --lib -p smart-lib --features domain-network -j6 -- --ignored --nocapture
live snapshot links: ["lo(loopback)", "enp0s31f6(ethernet)", "wlp0s20f3(wifi)"]
live known networks: [] · live profiles enumerated: 10 · live proxy: enabled=false
live: netlink links = ["enp0s31f6(ethernet)", "lo(loopback)", "wlp0s20f3(wifi)"],
      status kinds = ["enp0s31f6(ethernet)", "lo(loopback)", "wlp0s20f3(wifi)"]   ← the two surfaces agree
test result: ok. 4 passed; 0 failed
```

The rustyline REPL was additionally proven under a pty (`script -qec
./target/debug/smart /dev/null`): the banner, the prompt, a command
evaluation (`network interface lo`), and `exit` → `Goodbye!`.

## 7. Test totals

| suite | before | after |
|---|---|---|
| engine lib (`smart-lib`, `domain-network`) | 314 (+4 ignored live) | **318** (+4 ignored live, all passing) |
| shell (new) | — | **54** |
| — lib units (fuzzy 4, completer 4) | | 8 |
| — shell_language (parser/eval/pipes/selectors + gating + honesty) | | 17 |
| — network_cmd (surface, gated control verbs, JSON) | | 13 |
| — selectors (grammar, paths, filters, pending) | | 6 |
| — migrate (queue invariants) | | 6 |
| — pipes (all 20 ops + errors) | | 4 |

New engine tests: `event_links_report_the_same_kind_the_snapshot_would`,
`status_and_enumeration_agree_on_the_type`,
`dbus_settings_reply_in_the_daemons_convention_parses`,
`dbus_settings_value_round_trips_through_the_codec`.

## 8. The next domain to migrate, and why

**Clipboard** (queue position 1). Its archive body is the only *complete*
one (3/3 methods, 295 lines), it has no multi-backend fan-out, and it is the
smallest surface that still exercises the whole C2 pattern end-to-end —
archive body → hub trait → engine-owned types → shell renderer → live proof
in `smart`. Migrating it first validates the pattern (and the shell's
pending-answer removal path) before the waves that need their own judgement:
window's four WM backends, package's 19 managers, compositor's 9 backends.
One caveat for the dispatch that takes it: the old clipboard body is
`wl-paste`/`xclip` **shellouts**, and the C2 rule is native over subprocess —
so clipboard is also the first test of "upgrade the mechanism", not just
"port the body" (the Wayland data-control protocol natively, or an honest
`Unavailable`-with-reason where no native path exists).

## 9. For the founder to rule on

1. **Scope growth, flagged:** the dispatch's engine scope was bug 1; the
   `wifi list` proof needed the @78 D-Bus wire bug fixed too, and both live
   in the files I own this slice, so I fixed it. The fix is spec-locked and
   live-proven; nothing outside `crates/shell/**`, the engine's network
   files, the one workspace line, and this report was touched.
2. **The host-write gate's product shape:** one-shot mutating verbs need
   `--force`; the REPL prompts; a pipe never confirms. Confirm this is the
   intended UX (it is the `network_probe` precedent applied to the shell).
3. **`watch`'s default** is unbounded (Ctrl+C), with `--count N` for the
   bounded form — the old shell's behaviour. Confirm.
4. **`queue`** is the one domain with no old shell command module (new
   grammar at position 2). Confirm it stays.
5. **smart-cli-ng**: retired per the plan, left in place (not deleted); the
   `net` grammar is absorbed here (full names only). The orbit is recorded,
   not executed.
6. **The banner version** reads `Smart Shell v0.2.0` (the shell comes home).
