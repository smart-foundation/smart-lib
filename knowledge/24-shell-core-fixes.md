# 24 — The shell-core fixes (what was found, what was finished, what is proven)

**Scope:** `cd /home/lufuno/smart/smart-lib-ng`, one session, continuing the killed wave
(commit `860b553` = the shallowness audit is the spec). Only `crates/shell/**` was touched and
this report was written; no engine file, no commit, no host write. Charter law #8: reads live,
writes never performed — the two writes that *were* performed are named below with their
evidence (both were scratch files under `/tmp`, and the shell's own state files my proofs
created were removed again).

Test discipline observed: never `cargo test -p shell`. One binary per invocation, every one
pasted below. `cargo check -j6 -p shell` and `cargo build -j6 -p shell` were clean (the only
remaining warnings are the pre-existing engine ones in `src/**`, untouched by this wave).

---

## PART 1 — INVENTORY: what the killed run had done

`git status --short` → 21 modified files + 1 untracked (`crates/shell/src/session.rs`), 1399
insertions / 382 deletions. `cargo check -j6 -p shell` **compiled** (one warning: the unused
`after_ms` in `clock/mod.rs:486`), so the killed run's tree was sound but incomplete.

| mandate fix | state the killed run left it in | evidence |
|---|---|---|
| **1. session** | **~70% done, one fatal bug** | `session.rs` existed (new, untracked): per-domain read counts, memoized per-domain backends, `ClockSession` with a shared `timers.tsv` ledger. But `exec_clock` did `self.clock_backend.take().expect(...)` **before** the backend was ever built → every `clock` verb in the REPL panicked (`the session's clock backend`, shell.rs:884). The renders also printed twice (the command rendered itself *and* the REPL printed the value). |
| **2. confirmation** | **~50% done, backwards in one place** | `confirm.rs` had been rewritten to the founder's standard (Answer/NoInput, empty = NO, `confirm_typed` deleted as dead). `write_intent` existed in 8 domains, `write_context` in files only. But `accept_write` **dropped the target**: the prompt read `delete regular, 12 B, modified …?` with no path. The one-shot binary still passed `allowed: cli.force` and never prompted. The prompt also printed on a non-TTY, so `-o json` refusals were prefixed with a question. |
| **3. render** | **done** | `widgets.rs`: `CellShape::{Text,Path}`, `truncate_middle` (both ends kept), `truncate_path` (basename survives), `DEFAULT_CELL_CAP=48`, `Table::cap`, the row-count peepholes gone. `value.rs::to_raw` + `main.rs::emit` made `-o raw` print real text. |
| **4. the REPL stops lying** | **NOT TOUCHED** | `help` still listed 4 of 9 live domains and closed with "every other domain is the migration queue"; `help files`/`process`/`settings`/`logging`/`clock`/`time` all fell to `◌ migration pending`. No board-driven list existed. |

Tests at the start: **3 failures** — `files_cmd::every_write_verb_is_gated_and_the_gate_names_the_verb`
(locked the old `CHANGES host state` shout while the killed run had lowercased it),
`process_cmd::the_table_is_pixel_perfect` (fixture name was 40 chars — under the new 48 cap, so
nothing was cut), `clock_cmd::the_physical_limits_are_named_not_faked` (the ledger's reason no
longer named the host's lack of a persistence store).

---

## PART 2 — THE FOUR FIXES, finished

### Fix 1 — the session

**The change.** The backend is built once per domain per *process* and every later line drives it;
`session.rs` carries the counts (`ReadCount { domain, reads, builds }`) and the ledgers.

* `shell.rs`: `Shell { files_backend, process_backend, settings_backend, logging_backend,
  clock_backend, time_backend }` each `Option<Box<…>>`, built on first use through
  `self.files()` / `self.clock()` … which is also where `session.note_read(domain, built)` is
  counted (`builds` is 1 on the command that built it, 0 afterwards). `run_shell_with` builds the
  shell; the REPL's every line, a `-c` line and a piped script line share them.
* `session reads` is a verb (`ShellCommand::Session`) so the claim is measurable, not asserted.
* The clock's `exec_clock`/`exec_files` move the backend out for the call and put it back, so
  the session ledger and the engine are never aliased.
* **The panics and double-prints are gone** (`let _ = self.clock();` before the take; a rendered
  command answers `Value::Null`, never the value twice).

**The persistence decision, with reasoning.** A `timerfd` is a kernel object owned by the process
that armed it; no other process can read its expiration counter and it cannot be handed over. The
engine agrees — `persistTimer` / `listPersistedTimers` are DEGRADED with "this host carries no
timer-persistence store". So the shell does not fake a firing:

* **within a session** timers are live and checkable through the engine (create → list → check);
* **across processes** what survives is the *fact*: a small state file
  (`$SMART_STATE_DIR` else `~/.local/state/smart`) recording `pid / id / label / after_ms /
  interval_ms / clock / armed_at / session`, and each row of a fresh process is measured against
  `/proc` — `live` (this session), `owned` (a live sibling holds the fd), `gone` (the owner exited
  and the fd died with it). A firing is never claimed;
* the same shape was implemented for **watches** (`WatchSession`, `watches.tsv`, the new
  `files watches` verb): an inotify queue belongs to the fd that armed it, so the ledger records
  who armed what over which path and measures ownership the same way. `files watch` records on
  subscribe and retires on cancel — the row's lifetime is exactly the command's fd.

Hermetic by construction: `SessionState::hermetic()` (the library, and every test) resolves no
state file and writes nothing; only the binary calls `enable_state_persistence()`.

**Evidence.** `process | sort mem desc | take 5` → **1 read, 1 backend built** (the whole pipe
reads once and pipes the VALUE through `sort`/`take`).

```
  📈 Session reads (1) — one backend per domain per session; pipes read once and pipe the VALUE
  ┌─────────┬───────┬────────────────┐
  │ DOMAIN  │ READS │ BACKENDS BUILT │
  ├─────────┼───────┼────────────────┤
  │ process │ 1     │ 1              │
  └─────────┴───────┴────────────────┘
```

One-shot commands build one backend per process and do not promise cross-invocation state; where
they do share one (the timers), they say what kind of row it is.

### Fix 2 — confirmation (the founder's standard)

`accept_write(intent, context, gate)` is now ONE function both doors call — the REPL's dispatcher
and the one-shot binary's domain arms — and `WriteGate { force, dry_run, script_mode }` is the
whole answer surface.

* The question is the founder's wording **with the path always named**:
  `delete /tmp/smart-p/f.txt (regular, 2 B, modified 2026-10-15 21:39:16.305)? (y/N)`.
* **type/size/mtime** for a removal (plus `N entries below it` for `--recursive`), **old → new**
  for a move/copy (`/tmp/a (…) → /tmp/b (…)`) and for `settings set` — the host's current value is
  read live and shown beside the new one (`clock-format: 24h → 12h`).
* **Empty answer = NO.** Only an explicit `y`/`yes` proceeds; `n` and a bare Enter both refuse.
* **Script mode refuses, deterministically.** No operator and no piped line → `not_supported`,
  which `-o json` renders as `{"error":{"kind":"not_supported",…}}` and exits 1. A piped `y` line
  to a one-shot is a genuine answer.
* **`--dry-run` prints the plan and performs no write** — it prints the question with the same
  context and says so.
* `confirm_typed` is gone (deleted, not revived): the founder's standard is the yes/no prompt
  with context, and a "type the name" variant earns nothing against it.
* The prompt only prints when **stdin is a terminal**, so a script's output is never prefixed
  with a question. `script_mode` exists because a piped REPL's stdin is the PROGRAM: prompting
  there deadlocked the shell against its own reader (found live: exit 124), and now refuses
  instead of reading the next command as an answer.

### Fix 3 — the render layer

Mostly finished by the killed run; this wave hardened it:

* **width-aware middle truncation** in `widgets.rs`'s `Cell`: `truncate_middle` keeps both ends
  (`pool_workqueue_rele…worker`), `truncate_path` keeps the head and the **whole basename**; both
  are width-exact under the terminal-faithful rule (W/F=2, ambiguous=1, ANSI skipped with a
  correct CSI skipper `\x1b\[[0-9;?]*[ -/]*[@-~]`). The full value survives in `-o json` (locked
  by an assertion in `process_cmd.rs`.
* **the process table got a NAME cap** (`.cap(1, 32)`) so a long name is cut and the row stays
  bounded (a 7-column table at 48/column ran to 110 columns).
* **no renderer silently caps rows**: `settings` renders **259** rows, `logging sinks` **51**,
  `process` **373** — the 40/24-row peepholes and "N more below the cut" are gone; `| take` is
  the answer, and the one remaining sub-second rounding ("once, in 0s") is fixed by
  `duration_label` falling to ms/ns below a second.
* **counts become values**: `settings get … application-children` renders its 8 real items,
  not `[1 items]`; the settings header shows the type word (`list`), not the raw GVariant token
  (`as`) — the token still rides `-o json`.
* **`-o raw` stops printing 0 bytes** (all nine domains, measured below).

### Fix 4 — the REPL stops lying

* `migrate::live_domains()` is the board: `help` lists **what the board says is `RealToday`** —
  all nine, in the board's order, with a gloss table that can never shorten the list (a domain
  missing from the gloss still appears, with the board's default).
* `help <domain>` answers with the domain's **real verbs** for all nine (six new arms written
  from each domain's clap grammar), and the closing lie is replaced by the honest
  "8 queued and 30 archive-only domains are NOT live here — each answers honestly from the board".
* Every refusal message now names the real path — the interactive `(y/N)` prompt, a piped `y`
  line, or `--force` — in all nine domains (six still said only "In the REPL confirm the prompt").
* `clock timer list`'s reason names the physical limit it cannot beat ("this host does not carry
  a timer-persistence store … so the ledger records the FACT of a row and never claims a firing").
* `help`'s language section says what the grammar actually does (one command per line; an
  `if:`/`for:`/`fn` block is entered as a single `-c` line), instead of implying interactive
  blocks work.

---

## PART 3 — PROVE

### Per-binary test totals (one binary per invocation, machine law)

```
--lib                      test result: ok. 25 passed; 0 failed
--doc                      test result: ok.  3 passed; 0 failed
--test files_cmd           test result: ok. 15 passed; 0 failed
--test process_cmd         test result: ok. 13 passed; 0 failed
--test settings_cmd        test result: ok. 11 passed; 0 failed
--test logging_cmd         test result: ok.  9 passed; 0 failed
--test clock_cmd           test result: ok. 12 passed; 0 failed
--test time_cmd            test result: ok.  7 passed; 0 failed
--test network_cmd         test result: ok. 17 passed; 0 failed
--test clipboard_cmd       test result: ok.  9 passed; 0 failed
--test service_cmd         test result: ok. 13 passed; 0 failed
--test render_width        test result: ok. 11 passed; 0 failed
--test selectors           test result: ok.  6 passed; 0 failed
--test pipes               test result: ok.  4 passed; 0 failed
--test shell_language      test result: ok. 17 passed; 0 failed
--test migrate             test result: ok.  6 passed; 0 failed
```
Total: **145 passing, 0 failed** across 16 binaries. The engine (`src/**`) was never compiled as a
test target this wave because no engine file was touched (`git status` shows only
`crates/shell/**`); the three failures at the start are now green, each with its fixture or
assertion *strengthened* rather than deleted (the process name fixture is longer than the cap and
the JSON door is asserted to carry the uncut value; the files refusal now asserts the prompt AND
`--force` are named; the duration label carries its new sub-second cases).

### Live output, pasted

**(1) One session — `clock timer create → sleep → list → check` MEET, and the timer FIRED:**

```
  ⏲ Timer #1 (proof) — a real timerfd this process owns
  ├─ Anchor      10 ms from now (monotonic)
  ├─ Interval    0 ms (one-shot)
  └─ Source      a real timerfd this process owns: timerfd_create + timerfd_settime, …
  😴 Sleep 250 ms  — nanosleep on the caller's own thread
  ⏱ Timers (1 live · 0 owned · 0 gone) — the session's ledger; a firing cannot outlive the process that owns the timer
  ┌────┬───────┬────────────────┬───────────┬───────┬────────────────────────────────────┐
  │ ID │ LABEL │ EVERY          │ CLOCK     │ OWNER │ STATE                              │
  ├────┼───────┼────────────────┼───────────┼───────┼────────────────────────────────────┤
  │ 1  │ proof │ once, in 10 ms │ monotonic │ 11562 │ ◉ live — checkable in this session │
  └────┴───────┴────────────────┴───────────┴───────┴────────────────────────────────────┘
  ⏲ Timer 1 ◉ fired  — the kernel's own expiration count
```

And across processes, in a **second** invocation, the row the first one left is measured, not
invented: `(1 live · 0 owned · 1 gone)` → `│ 1 │ session-proof │ … │ 21771 │ ○ gone — pid 21771
exited, the fd died with it │`.

**(2) `files watch` receives a real kernel event mid-session** (`touch /tmp/smart-proof/hello.txt`
2 s into a 4-round watch):

```
  👁 Watch /tmp/smart-proof (3 event(s)) — the kernel's own inotify records, no polling thread
  ┌───────────┬────────────────────────────┬─────────────────────────┬──────┐
  │           │ EVENT                      │ PATH                    │ WHEN │
  ├───────────┼────────────────────────────┼─────────────────────────┼──────┤
  │ ＋ create │ /tmp/smart-proof/hello.txt │ 2026-10-15 21:15:23.758 │
  │ ～ modify │ /tmp/smart-proof/hello.txt │ 2026-10-15 21:15:23.758 │
  │ ～ modify │ /tmp/smart-proof/hello.txt │ 2026-10-15 21:15:23.758 │
  └───────────┴────────────────────────────┴─────────────────────────┴──────┘
```

And the watch ledger, from a fresh process while a live sibling holds its fd:

```
  ┌───┬──────────────┬───────┬────────────────────────────┐
  │   │ PATH         │ PID   │ OWNERSHIP                  │
  ├───┼──────────────┼───────┼────────────────────────────┤
  │ ◐ │ /tmp/smart-p │ 11742 │ owned by pid 11742 (alive) │
  └───┴──────────────┴───────┴────────────────────────────┘
```

**(3) The rm prompt rendered, and its script-mode refusal as JSON.** Interactive (a pty), the
answer `n`:

```
smart> files rm /tmp/smart-proof/hello.txt
  ⚠ delete /tmp/smart-proof/hello.txt (regular, 0 B, modified 2026-10-15 21:15:23.354) (y/N): n
  ✗ delete /tmp/smart-proof/hello.txt (regular, 0 B, modified 2026-10-15 21:15:23.354) — the operator answered no; nothing was written
```
`files ls` immediately after showed `hello.txt` still present. A bare Enter answers NO the same
way. Script mode (`< /dev/null`, `-o json`), exit 1 and no prompt prefix:

```json
{"error":{"kind":"not_supported","message":"refusing `delete /tmp/smart-p/f.txt (regular, 2 B, modified 2026-10-15 21:39:16.305)`: it changes host state and there is no operator to ask (non-interactive). Answer the interactive `(y/N)` prompt when running interactively, pipe a `y` line, or pass `--force` to accept the write explicitly (the live bring-back proof is founder-run)."}}
```

**(4) `files ls` on long names — truncated, and every row measured EQUAL** (ANSI stripped):

```
  ┌───┬──────────────────────────────────────────────────┬─────────┬──────┬─────────────────────────┐
  │   │ NAME                                             │ KIND    │ SIZE │ MODIFIED                │
  ├───┼──────────────────────────────────────────────────┼─────────┼──────┼─────────────────────────┤
  │ · │ a-very-long-directory-na…er-ends-and-keeps-going │ regular │ 0 B  │ 2026-10-15 21:39:21.885 │
  │ · │ f.txt                                            │ regular │ 2 B  │ 2026-10-15 21:39:16.305 │
  │ · │ x.txt                                            │ regular │ 0 B  │ 2026-10-15 21:39:21.885 │
  └───┴──────────────────────────────────────────────────┴─────────┴──────┴─────────────────────────┘
per-row display width (ANSI stripped):
  row 1: 101 cols   row 2: 101 cols   row 3: 101 cols
  row 4: 101 cols   row 5: 101 cols   row 6: 101 cols   row 7: 101 cols
```
The cut is the middle with both ends kept (`…na…er-ends…`), the basename survives.

**(5) `process | sort mem desc | take 5` with the read counts** (past the top of the table; the
fold is for width only):

```
  │ 12628 │ opencode        │ mem 1862066176 │   │ 2537 │ kilo   │ mem 1448226816 │
  │ 27152 │ firefox         │ mem  710918144 │   │ 30258 │ Isolated Web Co …    │
  📈 Session reads (1)
  ┌─────────┬───────┬────────────────┐
  │ DOMAIN  │ READS │ BACKENDS BUILT │
  ├─────────┼───────┼────────────────┤
  │ process │ 1     │ 1              │
  └─────────┴───────┴────────────────┘
```
One read for the whole pipe. (`sort mem desc` is the descending form the pipe grammar takes.)

**(6) `-o raw` is non-empty for all nine domains** (bytes, `wc -c`):

```
  network status  737    clipboard      1303   service status  11493
  files ls        154    process stats   358   settings        28483
  logging sinks  8402    clock now        48   time now          199
```

**(7) `help` lists what is LIVE**, from the board:

```
  ● Smart Shell — commands: 9 domains LIVE on the engine
    network      interfaces, wifi, radios — netlink + NetworkManager
    clipboard    the native selection: get/set/history/convert (writes gate-routed)
    service      the supervisor's services: state, health, deps, watch (writes gated)
    files        the host's files: ls/find/tree/stat/cat/du/watch (writes gated)
    process      /proc: every process, inspect, tree, groups, caps (writes gated)
    time         wall time, TAI, the contract's calendars and their arithmetic
    settings     the host's schemas, keys and values, live (writes gated)
    logging      the host's sinks and logs: sinks/domains/tail/entries/follow
    clock        clocks, timerfds and alarms — the session's ledger keeps them live
  ◌ 8 queued domain(s) and 30 archive-only domain(s) are NOT live here —
```

### Writes performed

Exactly two, both my own scratch files under `/tmp`, both the object of the confirmation proof:
`/tmp/smart-proof/target.txt` (removed, by piping `y` to the one-shot — the proof that a piped
`y` is a real answer) and the `/tmp/smart-long` + `/tmp/smart-proof` fixtures. The shell's own
state files my proofs created (`~/.local/state/smart/timers.tsv`, `watches.tsv`) were removed
again, so the host is as it was found. No host state (settings, wifi, services, signals,
clipboard) was ever written — every one of those was proven refused, or answered `n` /
`--dry-run`.

---

## PART 4 — WHAT REMAINS

1. **`files watch` cannot be observed live from another process.** The row is `owned`, but its
   *events* belong to the fd; handing an inotify fd across processes (SCM_RIGHTS over a unix
   socket) is the only way and is its own wave. The ledger is honest about it.
2. **A timer's FIRING cannot outlive its process** — the ledger names the fact and the physical
   limit, and `clock timer check` reads the kernel's counter only while the owner lives.
3. **Confirmation contexts live in two domains** (files: type/size/mtime/old→new; settings:
   old→new). `network`, `logging`, `service`, `process`, `clipboard` and `clock` name verb +
   target only — each needs its own live read (radio state, log level, service state, pid state)
   to earn the context beside it.
4. **One prompt per process.** A `-c 'files rm a' && -c 'files rm b'` answers once per
   invocation; there is no answer cache (deliberately: caching "yes" across commands is how a
   prompt becomes a trap).
5. **`--dry-run` is global, not per-verb**: it prints the plan for any mutating verb, but a dry
   run of a *pipe* over a write (`files ls | map … | files rm`) has no plan to show — pipes carry
   no write verb yet, by design.
6. The audit's runner-ups still stand outside this wave's four fixes: `network ip/dns/routes/
   counters/proxy`, `process spawn/read/watch/attach-as-verb`, `files chmod/append/cd/pwd/open`,
   `settings delete/reset/search`, `logging tail --pattern/--since`, `time --epoch/--format/
   zones`, the shell language's quoting/`$VAR`/stdin/redirection, and tab completion for the six
   domains that had none.
