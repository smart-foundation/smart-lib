# 15 — Batch 0, the CLI half: the width fix, the aesthetic pass, the finished surface

**Scope.** `crates/shell/**` only. Reads live; writes gate-refused and never performed (charter
law #8). No git commits.

**Where the aborted run left it.** The tree was clean at `c581437`; the only shell-side residue was
the three files that arrived *inside* the engine commits — `domains/service/mod.rs`
(`can_read_exact_state`), `migrate.rs` (two count strings), `tests/common/mod.rs` (the mock's new
caps field). The renderer, the two domains' command surface and the board's clipboard row were
untouched: nothing to salvage, everything to build. `cargo check` green before anything was written.

---

## 1. The width fix

`crates/shell/src/widgets.rs` — one measure, used by every table, every pad and every cut.

`display_width(&str) -> usize` walks the string and:

| rule | why |
|---|---|
| ANSI escapes count **zero** | a cell is `"\x1b[32m◉\x1b[0m active"`; the colour codes are not columns |
| combining marks, joiners, variation selectors count zero | they ride on their base |
| East-Asian Wide/Fullwidth (the CJK blocks) count **two** | every terminal draws them two |
| emoji-presentation marks (the U+1F3xx…U+1FAFF blocks, `⛔`) count **two** | same |
| the shell's meaning glyphs count **two** | `◉ ○ ● ◌ ◐` (the state circle family), `— …` (null, cut), `↑ ↓ ↔ ← →` (link) |
| the structural box-drawing glyphs (`─ │ ├ ┬ ┼ ┴ ┌ ┐ └ ┘`) count **one** | a border segment is one `─` per column, so the top rule is exactly as wide as the cells it closes — the beauty book's pixel contract (`03-beauty-book` §3.3) draws them one cell wide |

`pad_to` and `truncate_to` are built on the same measure, so padding and ellipsising cannot
disagree with it. `Table::widths`/`pad_row` (the only table builder — service, network, clipboard,
migrate and the pipe engine's `render_pretty` all render through it) now measure display width.

**A bug the new test caught in the new code:** `truncate_to` reserved *one* column for a
*two-column* `…`, so every ellipsised cell was one column too wide. Fixed to subtract the
ellipsis's own width; locked by `display_width(&truncate_to("abcdefghij", 8)) == 8`.

## 2. Alignment evidence — measured, not eyeballed

The same table, rendered live, measured two ways (ANSI stripped, East-Asian width per the table
above — an independent Python oracle using `wcwidth` + `unicodedata`, not the shell's own code):

```
  old rule (count chars, escapes included): widths 77..101   spread = 24 columns of drift
  new rule (display width):                widths 77..77    spread = 0
```

`smart service`, live, measured line by line:

```
service table lines: 62
  distinct widths: [77]
  distinct border layouts: 1
  border columns: (2, 26, 39, 45, 55, 68, 76)   ← identical on all 62 lines
```

```
  ⚙ Services (58) — the supervisor's own state, read natively
  ┌───────────────────────┬────────────┬─────┬─────────┬────────────┬───────┐
  │ NAME                  │ STATE      │ PID │ ENABLED │ SUPERVISED │ SINCE │
  ├───────────────────────┼────────────┼─────┼─────────┼────────────┼───────┤
  │ NetworkManager        │ ◉ active  │ 715 │ ◉      │ ◉         │ 1d    │
  │ acpid                 │ ◉ active  │ 706 │ ◉      │ ◉         │ 1d    │
  │ agetty-console        │ ○ unknown │ 0   │ ○      │ ○         │ —    │
  │ agetty-generic        │ ○ unknown │ 0   │ ○      │ ○         │ —    │
  │ agetty-tty1           │ ◉ active  │ 710 │ ◉      │ ◉         │ 1d    │
  │ alsa                  │ ◉ active  │ 708 │ ◉      │ ◉         │ 1d    │
  │ bluetoothd            │ ○ unknown │ 0   │ ○      │ ○         │ —    │
  │ chronyd               │ ◉ active  │ 711 │ ◉      │ ◉         │ 1d    │
  │ dbus                  │ ◉ active  │ 723 │ ◉      │ ◉         │ 1d    │
  └───────────────────────┴────────────┴─────┴─────────┴────────────┴───────┘
```

Every other boxed surface measures the same way: `network` (interfaces, radios, known networks),
`clipboard history`, and `smart migrate` — 50 lines, one width (175 columns), one layout. Before the
pass the migrate table's ARCHIVE BODY column was unbounded (its longest cell measured 263 display
columns — a 354-column table); it is now 72 with the full sentence one door away in
`smart migrate -o json`.

## 3. The aesthetic pass

Small, disciplined, information-first — no redesign, no new decoration:

* **State cells are glyph + word** (`◉ active`, `○ unknown`, `✗ failed`) instead of the glued
  `◉active` — colour never carries meaning alone, and the word is what a reader sees.
* **Every domain header is one treatment**: `  <glyph> <Title> (<count>)` in the domain's colour,
  bold. `⚙ Services (58)`, `❤ Health (58)`, `📋 Clipboard`, `📋 Copy History`, `🏷️ Offered MIME types
  (2)`, `🌐 Network`, `📡 Radios`, `🔌 wlp0s20f3 — interface`, `🧭 Migration Queue`. The ad-hoc red
  `❤ Health` and the cyan `🔌`/`📶` are gone; the state glyphs keep their own colours.
* **A measured summary line** under the service table: `16 active · 16 supervised · 0 failed — 42
  row(s) not supervised on this host` — every state counted, nothing hidden behind one number.
* **Bounded columns**: the clipboard ledger's TEXT column stops at 40 display columns with a marked
  `…` (one long copy used to blow the table open); the migrate ARCHIVE BODY column stops at 72 with
  the full sentence one door away (`smart migrate -o json`).
* **The clipboard content preview is one line, always** — a payload the seat holds with newlines in
  it used to break the tree it was rendered into.
* **The Wayland arm is rendered from the engine's own availability matrix** — the row's status token
  and its measured reason travel in the value tree and print in the status view, so the shell
  asserts nothing about this host that the engine did not measure.

## 4. The two stale lines, fixed

`domains/service/render.rs` — the trailer that said "the supervisor's run state is not readable by
this caller" (now false: every uid reads real /proc-derived states) now **separates the two measured
reasons** a row can answer `unknown` with, by the engine's own reason class:

* `no-privilege:` → named as a credential wall, the old sentence kept for the case it was true in;
* `not-found:` → "real states for every uid — no row above is blocked by privilege. These services
  are not supervised on this host … the reason each one did not find is measured, per row, below."

Both branches then name the one physically-privileged half and its reader:

```
     The one physically-privileged half is the supervisor's own exact stat words
     (its supervise tree is root-only for this uid):
       sudo ./target/debug/examples/service-state-probe --state-root /run/runit
```

`domains/service/mod.rs` — the module doc's "a root-only supervise tree answers `○ unknown` with the
measured reason naming the denied path" now describes the measured post-finish truth: real states
for every uid, the declared-but-not-linked rows named by their stat path and both supervision facts,
the registry-config surface answering with the contract's own reason class, and the privileged half
named with its command.

## 5. Per-command live verification (reads only)

**service** — the read path is the engine's native supervision read, no subprocess anywhere:

```
$ smart service status NetworkManager
  ⚙ NetworkManager
  ├─ State        ◉ active
  ├─ Main PID     715
  ├─ Enabled      ◉
  └─ Health       ◉ online  (probe not run by a read-only backend)

$ smart service logs NetworkManager
  📜 NetworkManager logs
  └─ ○ unavailable: service.capnp OBSERVES and defines no log surface — reading a service's log is
     logging.capnp's Logging.tail, which is a later wave. On this host the log store is the
     supervisor's own (runit keeps it in the service dir's ./log subdirectory; journald on a systemd
     host). No log line was read and none was invented (20 lines were requested for NetworkManager).
```

Also live: `inspect` (same shape as `status <name>`), `health` (58 rows, blue-headed table,
`◉ online`/`○ unknown`), `health --quarantined` (the honest empty: this supervisor has no quarantine
concept), `deps NetworkManager` (`◉ required dbus`), `resolve NetworkManager`
(`◉ id 464403115136593964` — exact, never an f64 rounding), `metrics`/`allocation`/`check`
(`✗ not-applicable: …` with the contract's own reason class), `watch --count 3` (real transitions),
`controls` (the seven gated `ServiceControl` verbs with their reason classes).

Selectors and pipes:

```
$ smart -c '@service/supervisor'
  supervisor            runit
  service_root          /var/service
  declared_root         /etc/sv
  state_root            /run/runit
  can_read_state        true          ← ungated for every uid (post-finish)
  can_read_exact_state  false         ← the one physically-privileged half
  can_control           false

$ smart -c 'service | where state == active | count'          16
$ smart -c '@service/services | where supervised == true | count'   16
$ smart -c '@service/services | where state == unknown | count'     42
```

Writes are gate refusals, demonstrated and never performed:

```
$ smart service start NetworkManager
smart: refusing to run `service start NetworkManager`: it CHANGES host state. In the REPL confirm
the prompt, or pass --force once you accept the write (the live bring-back proof is founder-run).
```

**clipboard** — the X11 arm, over `x11rb`, no `wl-paste`/`xclip` anywhere:

```
$ smart clipboard
  📋 Clipboard
  ├─ Transport   ◉ x11
  ├─ Wayland     ◐ degraded · wlr-data-control
  │  └─ the wlr-data-control Wayland arm is protocol-complete and seat-proven (the r…
  ├─ Readable    ◉ yes
  └─ Content     ◉ present
     mime text/plain
     4393 bytes
     text OTHER 0 20 '  🧭 Migration Queue'    OTHER 1 54 '  46 domains…
```

The content is whatever the seat holds — it changed mid-session under the read (154 bytes of the
founder's dispatch text at first, then a 4393-byte measurement dump), which is itself the proof the
read is the live selection and not a cache.

```
$ smart clipboard get            → mime text/plain · 4393 bytes · one-line preview
$ smart clipboard get --mime UTF8_STRING  → the same 4393 bytes, served on its second target
$ smart clipboard types          → text/plain, UTF8_STRING           (🏷️ Offered MIME types (2))
$ smart clipboard history        → ○ this process has made no copy yet (the ledger is per-process)
$ smart clipboard limits         → max item 16777216 · max history 67108864 · max count 64
$ smart clipboard preview 1 / validate 1 / convert 1 text/html → the ledger's honest not-found
$ smart -c '@clipboard/transport'   x11
$ smart -c '@clipboard/mimes | count'   2
$ smart -c '@clipboard/content'     mime_type / size_bytes / text
```

Writes refused: `clipboard set`, `clipboard clear`, `clipboard history pin|unpin|delete`.

**network** (already real; re-verified end to end after the width change): `network`,
`network interfaces`, `network status` (networking + radios + the four-state hostState),
`network interface wlp0s20f3`, `network radios`, `network wifi list` (the honest empty: 10 NM
profiles, 0 remembered), `network wifi scan` (the engine's own `no-backend` answer), `wifi off`
refused.

## 6. The board, before → after

| domain | before | after |
|---|---|---|
| clipboard | `native · 8 REAL / 10 DEGRADED` | `native · 5 REAL / 15 DEGRADED / 0 UNAVAILABLE` |
| service | `native · 7 REAL / 12 DEGRADED / 10 UNAVAILABLE` (count) with the supervise-tree archive line | same count, and the archive line now names the **/proc-derived supervision truth for every uid** plus the one gated half: `sudo ./target/debug/examples/service-state-probe --state-root /run/runit` |

Both numbers are tallied from the engine's own matrices (`CLIPBOARD_AVAILABILITY`,
`SERVICE_AVAILABILITY`) and **locked by a test**, so the sentence `smart migrate` prints cannot
drift from the matrix the engine ships.

```
$ smart migrate   (the real-today rows)
  │ ·   │ clipboard     │ ◉ real-today │ native · 5 REAL / 15 DEGRADED / 0 UNAVAILABLE │ …
  │ ·   │ network       │ ◉ real-today │ real engine                                    │ …
  │ ·   │ service       │ ◉ real-today │ native · 7 REAL / 12 DEGRADED / 10 UNAVAILABLE │ …
```

## 7. Tests

**75 → 85 green** (`cargo test -p shell -j6`), zero failures.

New file `tests/render_width.rs` (8 tests) — the alignment lock:

* `display_width_counts_the_glyphs_the_shell_draws` — the width model itself, including a coloured
  cell and the ellipsis arithmetic;
* `a_table_with_double_width_cells_is_border_aligned` — **the lock**: a hand-built table with `◉`,
  `—`, CJK and a coloured cell must render with every border on the same display column, asserted
  against the test's own width oracle (hand-written, so the assertion is not circular) with the
  expected padding spelled out per cell;
* `the_service_table_holds_its_borders`, `the_health_table_holds_its_borders`,
  `the_migration_board_holds_its_borders`, `the_clipboard_ledger_text_column_is_bounded` — the real
  renderings, measured;
* `the_trailer_names_the_privileged_half_instead_of_denying_the_read` — both measured classes of
  `unknown`, the probe command present, the stale sentence absent;
* `the_boards_counts_are_the_engines_own_availability_tallies` — the board vs. the engine's tables.

`tests/clipboard_cmd.rs` (+2): `the_wayland_arm_is_the_engines_own_availability_row` (the row's
status, reason and physical limit travel into the value tree and the human view) and
`the_content_preview_is_one_line_whatever_the_seat_holds`.

`cargo clippy -p shell` adds no new warnings — the single `manual_clamp` at `widgets.rs:30` is the
salvaged `make_bar`'s own, left exactly as it was found.

## 8. What is still true, stated plainly

* `service logs` answers with the contract split — log reading is logging.capnp's surface, a later
  wave; no log line was read and none was invented.
* The supervisor's exact stat words are root-only for uid 1000; the engine's helper binary is the
  ready command for that half.
* The clipboard's history ledger is this process's own copies, published DEGRADED-with-reason; the
  seat keeps no history across clients.
* No host-mutating verb was performed in this batch.
