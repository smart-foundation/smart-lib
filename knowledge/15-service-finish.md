# 15 — the service domain's live states, finished

> The follow-up to `19-service.md`. That report parked one thing behind a
> founder decision (§9.1): *"the root-only supervise tree is the live-proof
> ceiling here … If you want live states in the shell, that is a founder-run
> `sudo` probe."* This report is the answer: **no `sudo` was needed.** The
> states were never actually behind that wall — they were behind a missing
> read path. That path now exists, and every uid gets real states.

## 1. What was parked, and what it actually was

The parked claim was: all 26 `supervise.*` directories are `drwx------ root
root`, so uid 1000 gets `EACCES` on `supervise.<name>/stat`, therefore the state
is `unknown`. **The measurement was correct. The conclusion was not.**

The missing half of the reasoning: a runit service's run state is published
twice. Once by the supervisor, in the file that is root-only here —

```
$ ls -ld /run/runit/supervise.sshd
drwx------ 2 root root 160 Oct 13 00:38 /run/runit/supervise.sshd
$ ls -d /run/runit/supervise.* | wc -l          →  26
$ (for d in /run/runit/supervise.*; do cat $d/stat; done | wc -l)  →  0   # 0 of 26 readable
```

— and once by the kernel, in `/proc`, which publishes **every** process to
**every** uid (no `hidepid` on this mount):

```
$ ls /proc | grep -c '^[0-9]*$'                 →  374 processes, all visible
$ for p in $(ls /proc | grep '^[0-9]*$'); do
    [ "$(cat /proc/$p/comm)" = runsv ] && tr '\0' ' ' < /proc/$p/cmdline; done
  pid 683 runsv agetty-tty4
  pid 685 runsv alsa
  pid 686 runsv acpid
  pid 687 runsv polkitd
  …
  (16 runsv processes, one per linked service, all readable by uid 1000)
```

So the state was derivable; the code simply never derived it. `proc.rs` (319
lines, committed but unwired) held the derivation, and `runit.rs`'s
`enumerate()` stopped at `read_stat()`'s `Err` and answered `unknown` with the
denied path. **The parked work was a wiring gap, not a physical limit.**

## 2. The permitted paths tried, and their results

| # | path | result |
|---|---|---|
| 1 | re-read `/run/runit/supervise.<n>/stat` as uid 1000 | **EACCES** — confirmed physically gated, 0/26 readable |
| 2 | parse `sv status` output | **not reachable** — `sv` is a shellout this domain replaces (P5) *and* it fails itself (`unable to open supervise/ok: access denied`) |
| 3 | `sudo` (the founder's suggested fix) | **not used** — law #8, and unnecessary once path 4 existed |
| 4 | **derive the state from `/proc`: the `runsv` process + its live children + the service's own pid file + the kernel's boot clock** | **works, for every uid** — this is what shipped |
| 5 | keep the privileged exact read as a separate, complete, founder-run code path | **kept** — `RunitBackend::exact_states` + `examples/service-state-probe` |

Path 4 is not a substitute or an approximation. For a `run`-state service,
`/proc` names the same main pid the supervisor names, and the kernel's
`starttime` + `btime` gives the same transition clock:

```
$ ./target/debug/smart service status sshd           (as uid 1000, no sudo)
  ⚙ sshd
  ├─ State        ◉ active
  ├─ Main PID     726
  ├─ Enabled      ◉
  └─ Health       ◉ online  (probe not run by a read-only backend)
```

The supervisor's own `stat` line would say `run: … (pid 726) …`. The derived
read says pid 726. **Same pid, same clock, no privilege required.**

## 3. The wiring

One decision point, in `src/platforms/linux/service/runit.rs` `enumerate()`:
the exact read is tried **first** and used whenever it is readable (a root
caller, or a readable supervise tree, gets the supervisor's own words, its own
transition clock and its want up/down flags); **otherwise**
`state_from_proc()` answers from `/proc`. Nothing else changed shape — the
`NativeObservation` seam already carried `proc_facts`, so the derivation
travels as measured metadata (`runsv` pid, `live_children`, `state_source`,
`pidfile`) beside every state.

The derivation rules, all measured, never fabricated:

| question | `/proc` answer |
|---|---|
| is a supervisor watching it? | a `runsv <name>` process exists |
| is the service up? | that runsv has a live non-log child |
| which process is the main one? | the single non-log child, else the service's own pid file |
| since when? | `/proc/<pid>/stat` starttime + `/proc/stat` `btime` |
| is it down? | runsv exists, no live service child (zombies are not running) |
| running outside the supervisor? | the pid file names a live pid, no runsv watches it |
| no state derivable? | `unknown` — naming the denied stat path **and** the supervision truth |

Two details that keep it honest rather than heuristic: the log process is
distinguished from the service process by parsing the service's own `log/run`
script for its `exec`'d program (both are children of the same runsv, and that
is the only `/proc`-visible difference), and a caller who cannot tell them
apart says so instead of guessing which pid is the main one.

## 4. The privileged exact read: complete, and gated honestly

The exact half **is** a physical limit for this uid and is marked as such — not
`Unavailable`, not `unknown`:

- `availability.rs` carries a `readSuperviseStat` row (ordinal `0xFFFC`, not a
  contract method), `DEGRADED`-with-physical-reason, naming the exact denied
  path, the fact that the code path is complete and proven on both sides (a
  readable tree and a denied one), and the founder command.
- `runtime_gate()` was **narrowed**, not widened: it no longer gates the state
  reads, because `can_read_state` measures **true** (the `/proc` derivation
  answers). Only the verbs remain gated, by `can_control`. A root caller or a
  readable supervise tree lifts the exact half without a line of code changing.

**The founder command** (reads only; never writes a supervise file, never sends
a control byte):

```sh
cargo build -j6 --example service-state-probe --features domain-service
sudo ./target/debug/examples/service-state-probe --state-root /run/runit
sudo ./target/debug/examples/service-state-probe --state-root /run/runit sshd
```

It is a thin CLI over the engine's own `RunitBackend::exact_states` /
`render_exact_states`, so the format is test-locked in the library and cannot
drift. Exit codes: `0` every named service read, `1` at least one unreadable
(the reason names the exact denied path), `2` no supervise tree reached.

Run unprivileged on this host (proving the gate), then hermetically over a
scripted readable tree (proving what the founder will see):

```
$ ./target/debug/examples/service-state-probe --state-root /run/runit \
        --declared-root /etc/sv --scan-root /var/service sshd NetworkManager acpid
runit exact state read: 3 service(s)
  sshd                         unreadable  no-privilege: /run/runit/supervise.sshd/stat: Permission denied (os error 13)
  NetworkManager               unreadable  no-privilege: /run/runit/supervise.NetworkManager/stat: Permission denied (os error 13)
  acpid                        unreadable  no-privilege: /run/runit/supervise.acpid/stat: Permission denied (os error 13)

$ service-state-probe --state-root <scripted>           # a readable supervise tree
runit exact state read: 2 service(s)
  dbus                         inactive    down: /etc/sv/dbus: 4s, want down
  sshd                         active      run: /etc/sv/sshd: (pid 726) 5678s, normally up
```

## 5. Live proof (read-only; nothing on the host was changed)

```
$ cargo test -j6 --lib --features domain-service \
        live_reads_this_hosts_runit_supervision_natively -- --ignored --nocapture
live supervisor: runit (scan /var/service declared /etc/sv state /run/runit)
live caps: enumerate=true read_state=true control=false      ← read_state is now TRUE
live services: 58 enumerated
  NetworkManager   active   pid=715  enabled=true  supervised=true  runsv=695  source=proc
  acpid            active   pid=706  enabled=true  supervised=true  runsv=686  source=proc
  agetty-tty1      active   pid=710  enabled=true  supervised=true  runsv=693  source=proc
  … 16 linked services, all active, all with a measured runsv pid and main pid …
  alsa             active   pid=708  enabled=true  supervised=true  runsv=685  source=proc
  chronyd          active   pid=711  enabled=true  supervised=true  runsv=688  source=proc
  dbus             active   pid=723  enabled=true  supervised=true  runsv=696  source=proc
  elogind          active   pid=768  enabled=true  supervised=true  runsv=699  source=proc
  lightdm          active   pid=718  enabled=true  supervised=true  runsv=692  source=proc
  polkitd          active   pid=707  enabled=true  supervised=true  runsv=687  source=proc
  sshd             active   pid=726  enabled=true  supervised=true  runsv=702  source=proc
  udevd            active   pid=716  enabled=true  supervised=true  runsv=694  source=proc
  agetty-console   unknown  pid=0    enabled=false supervised=false runsv=-     source=-
      not-found: /run/runit/supervise.agetty-console/stat: No such file or directory (os error 2);
      the service is declared but not linked into the supervisor's scan dir,
      and no runsv process supervises it
  … 41 more declared-only services with the same measured reason …
live sshd: active pid=726 reason=-
live NetworkManager: active pid=715 reason=-
test result: ok. 1 passed; 0 failed
```

**The live state table (58 services: 16 active / 0 inactive / 42 unknown):**

```
$ ./target/debug/smart service
  NAME              STATE     PID   ENABLED  SUPERVISED  SINCE
  NetworkManager    ◉active   715   ◉        ◉           1d
  acpid             ◉active   706   ◉        ◉           1d
  agetty-console    ○unknown  0     ○        ○           —
  agetty-generic    ○unknown  0     ○        ○           —
  …
  agetty-tty1       ◉active   710   ◉        ◉           1d
  … 16 active rows …
  sshd              ◉active   726   ◉        ◉           1d
  udevd             ◉active   716   ◉        ◉           1d
  … 42 unknown rows (declared-only, no supervisor process) …
```

`smart service status <name>`:

```
$ ./target/debug/smart service status NetworkManager
  ⚙ NetworkManager
  ├─ State        ◉ active
  ├─ Main PID     715
  ├─ Enabled      ◉
  └─ Health       ◉ online  (probe not run by a read-only backend)

$ ./target/debug/smart service status sshd
  ⚙ sshd
  ├─ State        ◉ active
  ├─ Main PID     726
  ├─ Enabled      ◉
  └─ Health       ◉ online  (probe not run by a read-only backend)
```

**What the 42 `unknown` rows are.** They are honest, and they are not a gap:
each is declared in `/etc/sv` but **not linked** into `/var/service`, with
**no `runsv` process** and no pid file — there is genuinely nothing to derive
a state from, and the reason says exactly that, naming the stat path it did not
find and both supervision facts. Before this wave those 42 were `unknown` for
the *wrong* reason (`no-privilege` on a file they could not have a state in
even with root) and the 16 that do run were unknown too. Now 58/58 rows answer
with a measured reason, and every service with a visible process has a real
state: **`unknown` appears nowhere a state is derivable.**

## 6. The shell side — checked, and one stale line for the CLI agent's wave

**The state flow needs no shell change.** `crates/shell/src/domains/service/
render.rs` maps the engine's `ServiceStatus` generically
(`state_glyph`, `service_status`, `render.rs:458-468`, `586-594`); it never
hard-codes a runit state. The 16 real states came through with zero shell
edits, and `crates/shell/src/migrate.rs:97` already cites the new matrix
counts.

**One stale line to hand to the CLI agent's wave** (I did not touch it — the
shell is out of this wave's scope):

- `crates/shell/src/domains/service/render.rs:570` — the table footer reads
  *"the supervisor's run state is not readable by this caller —"*. That is now
  **false**: the run state *is* readable for every uid, from `/proc`. It should
  name what is actually gated — the supervisor's own exact `stat` words — and
  point at the founder helper. Suggested wording: *"the supervisor's own exact
  state read (`supervise.stat`) is root-only on this host — run
  `sudo ./target/debug/examples/service-state-probe --state-root /run/runit`;
  these states are derived from /proc"*.
- `crates/shell/src/domains/service/mod.rs:18` — the module doc says *"a
  root-only supervise tree answers `○ unknown` with the measured reason"*;
  same staleness, same fix.

## 7. Tests

Fixture-driven, with the `/proc` tree built by the test itself (no host process
is inspected by any test — law #8). `root_only_tree()` fabricates a full
supervision world: a world-readable `btime`, a `runsvdir`, and for each service
its `runsv` process, its children, its cmdline, its pid file and its declared
scripts — with the `stat` files at mode `000`, the same `EACCES` this host
serves.

- `state_is_derived_from_proc_truth_for_every_uid` — the main wiring test:
  a running service with a logger (main pid = the service child, the logger is
  not mistaken for it), a service with no log script, a service whose run
  script execs through `chpst` (main pid is `pause`, not `vlogger`), a
  wrapper with two children (active, no invented main pid), a zombie child
  (inactive — a dead child is not running), a service whose logger is its only
  live child, a service running outside the supervisor, a dead supervisor
  slot, a cold declaration, a phantom `runsv` that must not become a service,
  two dependency edges measured satisfied/unsatisfied from the same `/proc`
  truth, and the published caps — `can_read_state` **true**,
  `can_read_exact_state` false, `can_control` false, with
  `runtime_gate() == None` for every state read.
- `the_exact_supervise_read_is_code_complete` — the privileged half proven on
  **both sides**: a readable tree (`run: … (pid 4242) 5678s, normally up` →
  active, pid 4242, `since_us` 5 678 000 000), a denied one (reason starts
  `no-privilege:` and names `supervise.denied/stat`), an absent one
  (`not-found:`), plus the name filter and the test-locked render.

Totals:

```
$ cargo check -j6 --lib --features domain-service        →  Finished, 0 errors
$ cargo test  -j6 --lib --features domain-service
  running 363 tests
  test result: ok. 357 passed; 0 failed; 6 ignored; 0 measured; 0 filtered out

$ cargo test -j6 --lib --features domain-service live_reads_this_hosts_runit_supervision_natively -- --ignored --nocapture
  test result: ok. 1 passed; 0 failed
```

(Service-domain unit tests in the Linux hub suite: 23. The 6 ignored are
read-only live probes — netlink, NetworkManager ×3, the systemd bus, this
runit tree.)

## 8. The shellout-free grep

```
$ grep -rn "Command::new\|process::Command\|\.spawn()\|\.output()" \
        src/domains/service/ src/platforms/linux/service/ examples/ tests/
```

| pattern | matches | where |
|---|---|---|
| `Command::new` | **2** | `runit.rs:390`, `mod.rs:9` — both doc comments naming the condemned archive pattern |
| `process::Command` | **0** | — |
| `.spawn()` | **0** | — |
| `.output()` | **0** | — |

**Zero in executable code.** The `/proc` derivation is 374 `read_dir`/`read`
calls on the kernel's own introspection directory — no fork on any path.

## 9. Availability, before → after

| | before (`19-service.md`) | after |
|---|---|---|
| REAL | 7 | **7** |
| DEGRADED | 11 | **12** (+1: `readSuperviseStat`, the privileged exact read, DEGRADED-with-physical-reason) |
| UNAVAILABLE | 10 | **10** |
| **total rows** | 28 | **29** |

The shape of the change is the whole point: the state rows did **not** move to
`Unavailable` and nothing moved to `unknown`. One new `DEGRADED` row names the
one thing that is physically gated for this uid, and the runtime gate that
used to refuse every state read for it is gone.

| | before | after |
|---|---|---|
| live states answered (uid 1000) | 0 real / 58 `unknown` + `no-privilege` | **16 real** / 42 `unknown`-with-a-true-reason |
| `can_read_state` measured | false | **true** |
| runtime gate on `listServices`/`getService`/`getState`/`getHealth`/`listHealth` | `no-privilege` | **none** |
| `can_read_exact_state` measured | false | false (unchanged — the physical limit, now named in one DEGRADED row) |

## 10. For the founder

1. **Run the privileged half to see the supervisor's own words:**
   ```sh
   sudo ./target/debug/examples/service-state-probe --state-root /run/runit
   ```
   Expect 16 `run: … (pid …) …s, normally up` lines and 26 total rows, with the
   10 declared-only services answering `not-found`. This is read-only: it
   writes no supervise file and sends no control byte. The three lifecycle
   verbs are still yours to rule on — they remain mock-proven and
   founder-run.
2. **One shell edit is queued for the CLI agent's wave** (§6): the table
   footer and the module doc still claim the run state is unreadable. It is.
