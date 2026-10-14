# 19 — C3: service, the supervisor's own state read natively (the innovation mandate, third test)

> The convergence's third slice. Network (C1, `e21e296`) and clipboard (C2,
> `f1bf51b`) proved the pattern — hub trait, owned types, availability, the
> shell's rendering, the host-write gate, the migrate queue. Service applies it
> to the multi-supervisor domain and, per the founder's *"innovation like how we
> added netlink"*, replaces the archive body's fork-per-query `sv`/`systemctl`/
> `rc-service` shellout with the supervisor's own native read on every path.
> Live-proven on this runit host, read-only; the three lifecycle verbs are
> code-complete, gate-routed twice over, and demonstrated only as refusals.

## 1. The native read path per backend — 2 lines + why

- **runit → the supervisor's own files, read directly (primary, live-proven
  here).** This box runs runit (16 services linked in `/var/service`, 58 declared
  in `/etc/sv`, 26 `supervise.*` state dirs under `/run/runit` — all measured
  below). The backend does `read_dir(/etc/sv)` (declared), `read_dir(/var/service)`
  (linked = enabled), `metadata(/run/runit/supervise.<name>)` (supervised),
  `read(/run/runit/supervise.<name>/stat)` (state + the supervisor's own
  transition clock), and `read(/etc/sv/<name>/{run,check,finish})` (the `sv check`
  / `sv up` invocations a runit service's dependency actually is). Control is one
  byte to `supervise.<name>/control` — exactly what `sv` writes, without the fork.
  **Zero subprocesses on every path.**
- **systemd → sd-bus/D-Bus, spoken natively.** The maintained Rust `dbus` crate
  0.9 (a from-scratch reimplementation of libdbus's wire format, no C library, so
  it links on this box — the same standard network met with its hand-rolled NM
  marshaller): `org.freedesktop.systemd1.Manager.ListUnits` `a(ssssssouso)`,
  `ListUnitFiles` `a(ss)`, `Properties.Get(MainPID / ActiveEnterTimestamp /
  Wants / Requires / ActiveState)` on the unit's own published object path, and
  `StartUnit`/`StopUnit`/`RestartUnit` for the (founder-run) control verbs.
  **No `systemctl` anywhere.**
- **openrc / sysvinit → an honest decision, not a forgotten one.** openrc
  publishes its state as files, so the read is native (`/run/openrc/state/*`,
  `/etc/init.d`, `/etc/runlevels/*`, the init script's own `description=`,
  `rc_depend_in`/`rc_use`); its *control* is an honest `Unavailable`-with-reason
  because the only surface that exists is executing `rc-service` — the very
  shellout this domain replaces. sysvinit has **no portable state at all** (the
  pidfile convention is per-script), so a run state would be a guess: enumeration
  is native, state answers `Unavailable` naming that.

**The host fact that shapes the primary target, and what it honestly costs.**
All 26 `supervise.*` directories are `drwx------ root root`, so an unprivileged
reader (uid 1000) cannot read a single `stat` file — `sv status sshd` itself
reports `unable to open supervise/ok: access denied`. The backend therefore
reads natively and **reports honestly what it could not read**: every service
answers `status = unknown` with the measured reason and the exact denied path,
and the three control verbs are refused with `no-privilege` before any host
state is touched. Every *parse* of the status grammar is proven over a scripted
supervision tree built in a temp dir, so the native read path is proven without
needing root (§6).

## 2. The engine side (`domain-service`)

`domain-service = ["dep:dbus"]` in the engine manifest — one feature line
(law #2), `dbus` `optional` so a build without the domain never pays for it;
`libc` is a dev-dependency only (the runit control FIFO test makes a real FIFO,
`std`'s `mkfifo` being unstable on this toolchain). Feature = compiles
`service.capnp` + its transitive closure only, proven by the codegen inventory:

```
default features        : compiled 7 schema file(s) [… domain-network, domain-system …]
+ domain-service        : compiled 11 schema file(s) [… domain-service …]
```

- **`backend.rs` — the `ServiceBackend` trait**: one object-safe method per
  `Service` contract method (`listServices @0` … `control @13`), the two
  subscriptions' `pull`/`cancel`, plus `ServiceControl` (the gated half) and the
  `ServiceLifecycle` bridge. No permission string on any method (law #5): the
  contract's split is mirrored — observation open, every mutation behind
  `control`, whose returned reference **is** the authority. Every method carries
  an honest default (`Unavailable`/`notSupported` **with** its reason).
- **`types.rs`** — owned, engine-shaped: `ServiceInstanceInfo`,
  `ServiceInstanceState`, `ServiceHealth` (the canonical `D.ServiceHealth`,
  round-tripped through `discovery.capnp`), `ServiceDependency`, `Quota`,
  `ServiceResourceAllocation`, `ServiceHealthCheck`, `ServiceMetrics`,
  `ServiceEvent`/`ServiceEventPayload`/`ServiceEventType`, `LogLevel`,
  `Pagination`/`Page`, `SupervisorIdentity`, `SupervisionCapabilities`.
  `T.DataHandle` never appears: nothing in this surface allocates a shared
  buffer, so a fabricated `BufferId` reference cannot be produced. capnp
  `write_to`/`from_reader` round-trip every state struct (test-locked).
- **`availability.rs`** — one row per contract method **plus** the additions the
  domain documents separately: the two subscription pulls and two cancels (with
  the documented `interface << 8 | method` ordinals so they cannot collide with a
  contract ordinal), the seven `ServiceControl` verbs, and the three lifecycle
  bridge verbs (`0xFFFF`–`0xFFFD`, labelled "not a service.capnp method"). **7
  REAL / 11 DEGRADED / 10 UNAVAILABLE = 28 rows**, schema id
  `0xa1b2c3d4e5f60021`. Every REAL/DEGRADED row names its `l3_test`; a test that
  has vanished is a FAIL (`every_claimed_test_exists` asserts both directions).
  `runtime_gate(caps, method)` is computed from the **measured** caps, so the
  runtime answer and the table can never disagree.
- **`platforms/linux/service/`** — the hub (`LinuxServiceBackend`, one cell per
  supervisor, chosen by measurement at `native_connect()`), the four cells, and
  the `NativeSupervision` seam each cell implements (what the supervisor's own
  state says, with no contract shape in the signature).

### The mock seams (nothing touches a supervisor in CI)

- **runit/openrc**: injectable roots — the tests build a real supervision tree
  in a temp dir (`runit_tree()`: declared, linked, supervised, one service with a
  `sv check dbus` in its own `run` script, one cold, one wanted-up-and-down) and
  prove enumeration, the status grammar, pagination, dependencies, the bounded
  pulls, the capability scope rules and the honest absences.
- **systemd**: a scripted `SystemdBusHost` answering in the daemon's own
  convention (5 units, 3 unit files, MainPID/ActiveEnterTimestamp/Wants/
  Requires/ActiveState properties, recorded control verbs).
- **the control path**: a **real FIFO** in a temp dir with a reader thread on the
  other end — the supervisor's own protocol, exercised hermetically.
- The two host-touching probes are `#[ignore]`d and read-only (§6).

## 3. The shell side (`domain-service`, default)

`smart service` renders the engine's owned types in the shell's aesthetic:
`status [name]` (the table / one service), `inspect`, `health [--quarantined]`,
`deps`, `resolve`, `watch [name] [-c N]`, `controls [verb]`, `metrics`,
`allocation`, `check`, `logs`, and `start|stop|restart` (host-mutating). Selectors
`@service/services` and `@service/supervisor` resolve the same tree the command
renders, so a selector and a command can never disagree. The native-path
decisions and the honest absences are in the aesthetic: the table's footer names
the reason any state could not be read, `service controls` lists the seven gated
verbs with the contract's own reason class, `service logs` answers that log
reading is logging.capnp's surface, and `metrics`/`allocation`/`check` answer
`✗ notSupported: not-applicable: …` naming the supervisor that was reached.
`migrate.rs`: service is **real-today**; the queue is 3 real / 10 queued / 33
archive-only, and `pending_reason` is status-aware so a real domain is never told
it is pending.

## 4. Availability counts

- **The matrix**: **7 REAL / 11 DEGRADED / 10 UNAVAILABLE** (28 rows).
  REAL: `listServices`, `getService`, `getState`, `resolve`, `cancelState`,
  `cancelServiceEvents`, `control`. DEGRADED-with-reason: `getHealth`,
  `listHealth`, `listQuarantined` (the honest empty — a supervisor has no
  quarantine concept), `getDependencies` (the supervisor's own declaration),
  the four subscription rows (engine-side observation, bounded pull), and the
  three lifecycle bridge verbs (mock-proven native control path, founder-run
  live). UNAVAILABLE-with-reason: `getResourceAllocation`, `getHealthCheck`,
  `getMetrics` (`not-applicable`) and all seven `ServiceControl` verbs
  (`no-backend`).
- **Runtime** on top of that: this host's runit supervise tree is root-only, so
  `can_read_state` measures **false** and the state reads answer
  `no-privilege` (never a fabricated state, never a fake empty), while the verbs
  answer `no-privilege` from `can_control` measuring false. A root or a readable
  supervise tree would lift the same gate without a line of code changing.

## 5. Live proof (read-only; no write was performed — the founder's call)

```
$ ./target/debug/smart service          # this host's runit, read natively
  ⚙ Services
  ┌───────────────────────┬──────────┬─────┬────────────┬────────────┬───────┐
  │ NAME                  │ STATE    │ PID │ ENABLED    │ SUPERVISED │ SINCE │
  ├───────────────────────┼──────────┼─────┼────────────┼────────────┼───────┤
  │ NetworkManager        │ ○unknown │ 0   │ ◉ │ ◉ │ —     │
  │ acpid                 │ ○unknown │ 0   │ ◉ │ ◉ │ —     │
  │ agetty-tty1           │ ○unknown │ 0   │ ◉ │ ◉ │ —     │
  │ … 58 rows (16 linked / 58 declared / 26 supervised) …
  └───────────────────────┴──────────┴─────┴────────────┴────────────┴───────┘

  ● the supervisor's run state is not readable by this caller —
    NetworkManager: no-privilege: /run/runit/supervise.NetworkManager/stat: Permission denied (os error 13)
    acpid: no-privilege: /run/runit/supervise.acpid/stat: Permission denied (os error 13)
    agetty-console: not-found: /run/runit/supervise.agetty-console/stat: No such file or directory (os error 2)
    … and 55 more service(s) with the same measured reason

$ ./target/debug/smart service status sshd
  ⚙ sshd
  ├─ State        ○ unknown
  ├─ Reason       no-privilege: /run/runit/supervise.sshd/stat: Permission denied (os error 13)
  ├─ Main PID     ○ none measured
  ├─ Enabled      ◉
  └─ Health       ○ unknown  (probe not run by a read-only backend)

$ ./target/debug/smart -c '@service/supervisor/supervisor'   →  runit
$ ./target/debug/smart -c '@service/services | count'        →  58
$ ./target/debug/smart -c '@service/services | where enabled == true | count'  → 16
$ ./target/debug/smart -c '@service/services | where state == unknown | count' → 58

$ ./target/debug/smart service resolve NetworkManager
  🔎 resolve NetworkManager       ◉ id 464403115136593964      ← the engine's exact id

$ ./target/debug/smart service deps NetworkManager            ← read natively from
  🔗 Dependencies                                              /etc/sv/NetworkManager's own script
  ├─ ○ required  dbus

$ ./target/debug/smart service controls                        ← the one gated surface
  🔒 ServiceControl — the one gated surface, and what it honestly answers here
  ├─ setDependencies          ✗ no-backend — a registry-config operation a host supervisor does not own
  ├─ … (all seven, with the contract's own reason class)

$ ./target/debug/smart service logs sshd
  📜 sshd logs
  └─ ○ unavailable: service.capnp OBSERVES and defines no log surface — reading a service's log
     is logging.capnp's Logging.tail, which is a later wave … No log line was read and none was invented.

$ ./target/debug/smart migrate
  🧭 Migration Queue
  46 domains: 3 real today, 10 queued, 33 archive-only
  │ network │ ◉ real-today │ real engine │ netlink + NM (live-proven, 358 tests) │
  │ service │ ◉ real-today │ native · 7 REAL / 11 DEGRADED / 10 UNAVAILABLE │ engine: the
  │         │              │            │ supervisor's own state read natively — runit's supervise
  │         │              │            │ tree (live-proven here), systemd's system bus over the
  │         │              │            │ dbus crate, openrc's own state files — no shellout │
$ ./target/debug/smart -c 'migrate | where status == real-today | count'      → 3
$ ./target/debug/smart -c 'migrate | where status == migrated-pending | count' → 10

# ── the write verbs: demonstrated only as refusals ──
$ ./target/debug/smart service start sshd
smart: refusing to run `service start sshd`: it CHANGES host state. In the REPL confirm the
prompt, or pass --force once you accept the write (the live bring-back proof is founder-run).

$ ./target/debug/smart --force service start sshd     # the shell gate opens, the engine's
smart: permissionDenied: no-privilege: the supervisor's control path is not writable by this
caller, so the verb is refused before any host state is touched
                                                     ← MEASURED gate: 26/26 supervise dirs are
$ ls /var/service | wc -l                             drwx------ root; host unchanged (16, sshd
16                                                    still supervised, dir mtime Oct 13 00:38)
```

Engine live probes (read-only, `#[ignore]`d, run here):

```
$ cargo test -j6 --lib --features domain-service live_reads_this_hosts_runit_supervision_natively \
        -- --ignored --nocapture
  live supervisor: runit (scan /var/service declared /etc/sv state /run/runit)
  live caps: enumerate=true read_state=false control=false        ← the measured gate
  live services: 58 enumerated
  live sshd: unknown pid=0 reason=no-privilege: /run/runit/supervise.sshd/stat: Permission denied (os error 13)
  live NetworkManager: unknown pid=0 reason=no-privilege: /run/runit/supervise.NetworkManager/stat: Permission denied (os error 13)
  test result: ok. 1 passed

$ cargo test -j6 --lib --features domain-service live_speaks_the_system_bus_natively \
        -- --ignored --nocapture
  live systemd ListUnits: dbus: ListUnits: The name org.freedesktop.systemd1 was not
  provided by any .service files
  test result: ok. 1 passed
```

The second probe is the native-path evidence for the systemd cell: the D-Bus
client **reaches this host's real system bus and speaks the protocol correctly** —
the bus itself answers, naming the absent manager (`dbus` is one of `/var/service`'s
16; there is no systemd here). What is absent is the manager, and the answer says so.

## 6. The shellout-free grep

```
$ grep -rn "Command::new\|process::Command\|\.spawn()\|\.output()" \
      src/domains/service/ src/platforms/linux/service/ \
      crates/shell/src/domains/service/ src/platforms/linux.rs
```

| pattern | matches | where |
|---|---|---|
| `Command::new` | **3** | `runit.rs:331`, `mod.rs:9`, shell `mod.rs:8` — all doc comments naming the condemned archive pattern |
| `process::Command` | **0** | — |
| `.spawn()` | **0** | — |
| `.output()` | **0** | — |

There is no `std::process::Command` in executable code anywhere in the ported
surface: the archive body's `Command::new("sv")`, `Command::new("ls")`,
`Command::new("ln")`, `Command::new("rm")`, `Command::new("systemctl")`,
`Command::new("rc-service")`, `Command::new("openrc")`, `Command::new("rc-update")`
(verified in the read-only archive) are all gone. The remaining textual mentions
of those tools are doc comments and reason strings that name what was replaced —
the same convention clipboard's grep proof used.

## 7. Test totals (before → after)

| suite | before | after |
|---|---|---|
| engine lib (default features) | 318 (+4 ignored) | **318** (+4 ignored) — unchanged; service is feature-gated |
| engine lib (`+ domain-service`) | — | **355 passed** / 6 ignored / 0 failed |
| engine lib (`+ domain-service + domain-clipboard`) | — | **378 passed** / 7 ignored / 0 failed |
| engine service domain (its own tests) | — | **39** (37 running + 2 `#[ignore]`d live probes): 11 domain contract, 18 Linux hub, 6 systemd cell, 4 openrc cell |
| shell (all suites) | 61 | **75** (+14) |
| — lib units (completer) | 8 | **9** (+1: service subcommand completion) |
| — `service_cmd` (new) | — | **13** |
| — network / pipes / selectors / clipboard / shell_language / migrate | unchanged in count | unchanged |

Coverage: error totality over every honest default
(`every_method_degrades_with_a_reason_on_a_backend_that_serves_nothing` covers
every method and every `ServiceControl` verb), the 20 error codes' defaults, the
status grammar parsed for every state the supervisor writes (and an unknown word
staying unknown with the line verbatim), bounded pulls (`maxStates`/`maxEvents`
honoured, an empty batch when nothing changed, an unknown subscription is
not-found), the availability matrix's internal consistency (one row per contract
method on both interfaces, unique ordinals, every claim's test exists and every
test is claimed), the presented-capability scope rules, the runtime gate, the
capnp round-trips, the FIFO write-through, and the seat-less-style absence for a
host with no supervisor.

Feature matrix (law #2) green in every configuration: `-p shell` default,
`--no-default-features`, and each single domain on its own.

## 8. The next domain, and why

**`queue`** (queue position 1). Its archive body is complete (8/8 methods, 60
lines, `platforms/linux/queue.rs`), it has no multi-backend fan-out, and it is
the third wave that re-uses the pattern now proven three times (archive body →
hub trait → owned types → shell renderer → live proof). It is also the one queued
domain the original shell never hosted a command for, so it validates net-new
grammar on the working engine a second time. Next, unchanged: `media 8/13`,
`database 8/14`, `files 8/17`, then the thin ones (`audio 6/7`, `power 4/7`,
`process 4/13`), then the multi-backend domains (window's four WMs, package's 19
managers, compositor's 9 backends) — each its own wave.

## 9. For the founder to rule on

1. **The root-only supervise tree is the live-proof ceiling here.** The native
   read path (declaration, enablement, supervision, dependency, the status
   grammar) is fully proven — hermetic over scripted trees *and* live against
   this host's `/etc/sv`, `/var/service`, `/run/runit`. What this host cannot
   serve unprivileged is the *run state*: all 26 `supervise.*` dirs are
   `drwx------`, so `smart service` honestly answers `unknown` + the denied path.
   If you want live states in the shell, that is a founder-run `sudo` probe (or a
   runit layout with a readable supervise dir); I did not use `sudo` (law #8).
2. **`dbus` (0.9) is a new engine dependency**, optional and gated behind
   `domain-service`; `libc` is dev-only. Both are declared with their reasons in
   the manifest. The systemd cell is built and mock-proven but **live-unproven**
   (this box has no systemd) — the same honesty clipboard's Wayland arm has, and
   its live probe proves the client against the real bus rather than inventing a
   manager.
3. **Two defects found and fixed in the incoming work** (both mine to fix, both
   now test-locked): the queue cited the availability split as `5 REAL / 13
   DEGRADED`, while the matrix is **7 REAL / 11 DEGRADED / 10 UNAVAILABLE** — the
   count is now the table's own; and the shell rendered `T.ServiceId` as an
   `f64`, which **rounds a 64-bit hash into a number that is not the id**
   (`9495150527980360000` vs the engine's `9495150527980359471`). Opaque
   identities and nanosecond timestamps now travel as exact decimal text
   (`464403115136593964`), locked by
   `an_opaque_identity_is_rendered_exactly_never_rounded`.
4. **A latent variant of that rounding in the clipboard wave's file** (not mine
   to edit): `clipboard/render.rs:119` renders a history `timestamp` as
   `f64` — nanoseconds exceed 2^53, so a populated history would print a rounded
   time. The clipboard ledger is empty on this host, so nothing renders wrongly
   today. Recommend the clipboard owner switch it to exact text the same way.
5. **Shell surface I widened beyond the module** (all in files this wave's
   pattern already edits): `help service` and `help clipboard` arms, the real
   domains in the top-level command list, and tab completion of the service
   subcommands (the mutating verbs are completable — they are gated at execution,
   not hidden). `watch service` is a verb (`service watch`), not the REPL's
   `watch` binding, which is still network-only — same as clipboard's C2 decision.
6. **The host-write gate now has two layers**: the shell's `--force`/REPL prompt,
   and the engine's *measured* capability gate, which refused this host's
   `--force service start` before any open. Confirmed as the intended UX (it is
   the C1/C2 precedent, applied verbatim).
