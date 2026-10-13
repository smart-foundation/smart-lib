# 18 — C2: clipboard, native over shellout (the innovation mandate, first test)

> The convergence's second slice, and the first full test of "UPGRADE the
> mechanism" (the plan's C2 rule; the founder's *"innovation like how we added
> netlink"*). Network (C1, `e21e296`) proved the pattern — salvage map, hub
> trait, engine-owned types, shell renderer, migration queue. Clipboard applies
> it **and** replaces the archive body's condemned `wl-paste`/`xclip`
> subprocess shellout with a real selection-protocol client. Live-proven on this
> host, read-only; the one mutating verb (`clipboard set`) is code-complete and
> gate-routed, its live bring-back is the founder's.

## 1. The native path chosen — 2 lines + why

- **X11 → native via `x11rb` 0.13 (pure-Rust X11 protocol).** This host *is* an
  X11 session (`DISPLAY=:0`, socket `/tmp/.X11-unix/X0`, `XDG_SESSION_TYPE=x11`),
  and neither `wl-paste`/`wl-copy`/`xclip` is installed — so it is the one path
  that can be — and is — live-proven end-to-end here with **zero subprocess**.
- **Wayland → a documented honest decision, not faked.** This box runs no
  compositor (no `wayland-*` socket, `WAYLAND_DISPLAY` empty), so the
  wlr-data-control read/write cannot be live-proven; per law #8 + the C1
  hand-rolled-D-Bus lesson (no untested wire protocol), it is **not shipped**
  here. `native_connect()` detects a Wayland-only seat and returns a precise
  `no-backend`-with-reason naming the deferred arm; on the X11 box the native
  read path is the proven one. No shellout anywhere (grep-proven, §6).

**Why `x11rb` and not `x11`/`x11-dl`/`xcb`:** those bind C `libX11`/`libxcb`,
and neither library is installed on the build box, so a C-backed client fails to
link. `x11rb` is a from-scratch **Rust** implementation of the X11 wire protocol;
it links nothing. Same standard as the network domain's `netlink-sys` /
`netlink-packet-route`: a maintained crate ecosystem, native over subprocess,
behind a mockable seam (charter law #9).

What the read path actually does, with no fork in any step:
`GetSelectionOwner(CLIPBOARD)` → `ConvertSelection(CLIPBOARD, target,
property)` → wait for `SelectionNotify` → `GetProperty` (follow `bytes_after`
until the whole payload, bounded at 64 MiB). `text/plain` maps to the `UTF8_STRING`
target the way X11 clipboard managers expose it. The write path (`setContent`
/`setOffer`) is `SetSelectionOwner` + on-demand `SelectionRequest` serving from
the stored offers — the standard ICCCM mechanism, host-mutating, founder-run.

## 2. The engine side (`domain-clipboard`)

`domain-clipboard = ["dep:x11rb"]` in the engine manifest — one feature line
(law #2); `x11rb` is `optional` so a build without the domain never pays for it.
Feature = compiles `clipboard.capnp` + foundation only (transitive closure), and
auto-wires `src/domains/clipboard/mod.rs` (no shared-file edit); the platform
cell is `src/platforms/linux.rs`'s computed `#[path="linux/clipboard.rs"]`.

- **`backend.rs` — the `ClipboardBackend` trait**: one object-safe method per
  contract method (`getContent @0` … `setOffer @16`) plus the subscription's
  `pull`/`cancel`. No permission string on any method (law #5). Every method
  carries an honest default (`Unavailable`/`notSupported` **with** a
  `reasonClass`-prefixed reason — an `Unavailable` without a reason is the
  fabrication law #6 exists to stop). `clipboard_caps() -> (transport, can_read,
  can_write)`.
- **`types.rs`** — owned, engine-shaped: `ClipboardContent { mime_type, data }`,
  `ClipboardItem`, `ClipboardChange`, `ClipboardEventType`, `ClipboardPreview`,
  `ClipboardSizeLimits`, `ContentOutcome { Content, Empty }`,
  `ConversionOutcome { Content, Unsupported }`, `ValidationOutcome`,
  `Pagination`/`Page`. `data` is `Vec<u8>` on purpose: the contract's
  `T.DataHandle` `buffer :BufferId` arm names shared memory this in-process
  backend never allocated, so a fabricated buffer reference can never be
  produced. capnp `write_to`/`from_reader` round-trip the state structs (payload
  rides `DataHandle.inline`; `mimeType`→`T.MimeType`), so the wire half is
  faithful.
- **`availability.rs`** — one row per contract method. **8 REAL / 9 DEGRADED / 0
  UNAVAILABLE** as the native-backend *target* (schema id `0xa1b2c3d4e5f60012`).
  The honest axis is not "unavailable" but **DEGRADED-with-reason**: `getHistory`
  /`clearHistory`/`searchHistory`/`pin`/`unpin`/`delete` are a bounded
  **engine-side ledger of this process's own copies**, because neither X11's
  CLIPBOARD manager nor wlr-data-control keeps a history across clients — a real
  host history does not exist, so it is never faked as REAL. Host-mutating verbs
  are DEGRADED (mock-proven mapping; founder-run live). The seat-less runtime
  gate is a per-method `Unavailable`-with-reason (§5), the envelope around this.
- **`platforms/linux/clipboard.rs`** — the `SelectionHost` seam (+ `NoSelectionHost`
  seat-less stub + the x11rb `X11SelectionHost`) and the `LinuxClipboardBackend`
  (ledger + subscriptions + limits, deterministic and host-independent).
- **`api.rs`** — the borrowing `Clipboard` wrapper.

### The mock seam (nothing touches a display in CI)

Every host touch goes through `SelectionHost`. Tests drive `LinuxClipboardBackend`
over a **scripted** seat that models a real one (`claim` makes the copy readable,
`relinquish` empties it), so error totality, bounded pulls and the mapping are
proven with no display. The one live probe is `#[ignore]`d and read-only (§6).

## 3. The shell side (`domain-clipboard`, default)

`smart clipboard` renders the engine's owned types in the shell's aesthetic;
every mutating verb routes through the C1 host-write gate (`--force` one-shot,
REPL confirm; law #8). Surface: `status`, `get [--mime]`, `set`, `clear`,
`history [search|pin|unpin|delete]`, `types`, `preview`, `validate`, `convert`,
`limits`. Selectors: `@clipboard/content`, `@clipboard/transport`,
`@clipboard/mimes`, `@clipboard/history` — the same tree the command renders,
so a selector and a command can never disagree (the C1 rule). `migrate.rs`:
clipboard is now **real-today**; the queue renumbers 2→1 … 12→11 (11 queued);
`pending_reason` is status-aware so a real domain is never told it is pending.

## 4. Availability counts

- **Native-backend target matrix**: **8 REAL / 9 DEGRADED / 0 UNAVAILABLE**
  (18 rows: the 17 contract methods `@0`–`@16` + `cancel`).
- **Runtime** is honest on top of that: a seat-less host reads `getContent` as
  `unavailable`-with-reason (never an empty clipboard fabricated); a Wayland seat
  gets the precise deferred-arm reason; an X11 seat reads natively.
- History family is DEGRADED-with-reason on every host (per-process ledger).

## 5. Live proof (read-only; a write was NOT performed — founder's call)

```
$ smart clipboard
  📋 Clipboard
  ├─ Transport   ◉ x11
  ├─ Readable    ◉ yes
  └─ Content     ◉ present
     mime text/plain · 549 bytes · text " git log --oneline -2 ..."

$ smart clipboard get
  📋 Clipboard · mime text/plain · 549 bytes · <the real clipboard text>

$ smart clipboard get --mime image/png
  smart: unavailable: the current selection owner does not offer image/png

$ smart -o json clipboard get
  {"mime_type":"text/plain","size_bytes":549,"text":" git log --oneline -2\n🌐 Network ..."}

$ smart -c '@clipboard/transport'   →  x11
$ smart -c '@clipboard/mimes'       →  text/plain, UTF8_STRING
$ smart -c '@clipboard/mimes | count'        →  2
$ smart -c 'clipboard history | count'       →  0   (per-process ledger; honest empty)

$ smart clipboard types / limits   → live targets / this backend's own policy

$ smart clipboard set "PROBE"      →  refused (gate): "it CHANGES host state ...
                                     pass --force ..."   ← nothing was mutated
$ smart migrate | where name == clipboard
  │ clipboard │ real-today │ null │ engine: native X11 CLIPBOARD selection over
  │ x11rb (live-proven) + wlr-data-control on Wayland — no wl-paste/xclip shellout │
  │ native · 8 REAL / 9 DEGRADED │
$ smart -c 'migrate | where status == real-today | count'   → 2
$ smart -c 'migrate | where status == migrated-pending | count' → 11
```

The `clipboard set` live bring-back is founder-run (gate-routed, code-complete);
it was not performed here.

Engine live probe (read-only, `#[ignore]`d, run here):
```
$ cargo test --lib --no-default-features --features domain-clipboard,std \
      live_reads_the_x_clipboard_natively -- --ignored --nocapture
  live clipboard transport: x11 read=true write=true
  live clipboard targets: ["text/plain", "UTF8_STRING"]
  live clipboard text/plain (549 bytes): " git log --oneline -2\n🌐 Network ..."
  test result: ok. 1 passed
```

## 6. The shellout-free grep

```
$ grep -rn "Command::new\|wl-paste\|wl-copy\|xclip" \
      crates/shell/src/domains/clipboard src/platforms/linux/clipboard.rs \
      src/domains/clipboard src/platforms/linux.rs
```
Every match is a **doc-comment or a reason string that names the condemned
pattern we replaced** (`mod.rs:7,11,22`, `backend.rs:64,74,219`,
`availability.rs:14,174`, `linux/clipboard.rs:3,155,1415`, `linux.rs:24`).
`Command::new` matches **0** times. There is no `std::process::Command` anywhere
in the ported code — the archive body's fork-per-op shellout is gone.

## 7. Test totals (before → after)

| suite | before | after |
|---|---|---|
| engine lib (`smart-lib`, default features) | 318 (+4 ignored) | 318 (+4 ignored) — unchanged; clipboard is feature-gated |
| engine lib (`+ domain-clipboard`, TCP flake skipped) | — | **340 passed** / 5 ignored / 0 failed |
| **engine clipboard domain** (its own tests) | — | **25 passed** (+1 ignored live probe): 19 native-path over the scripted seam + 6 contract/totality |
| shell (all suites) | 54 | **61** (+7 `clipboard_cmd`; network/pipes/selectors/migrate unchanged in count) |

Coverage: error totality over the honest defaults (`every_method_degrades_with_a_reason`
covers every method), bounded pulls (`subscription_pulls_are_bounded…`),
empty-vs-not-offered, the write gate, capnp round-trips (verified payload via
`DataHandle.inline`; the `buffer :BufferId` half is intentionally inapplicable
here), availability-matrix consistency, and the seat-less runtime gate.

Feature matrix (law #2) green in every configuration: `-p shell` default,
`--no-default-features`, `--no-default-features --features domain-clipboard`,
`--no-default-features --features domain-network`.

## 8. The next domain, and why

**`queue`** (queue position 1). Its archive body is complete (8/8 methods, 60
lines, `platforms/linux/queue.rs`), it has no multi-backend fan-out, and it is
the second wave that re-uses the exact C1→C2 pattern now proven twice
(archive body → hub trait → engine-owned types → shell renderer → live proof),
including the pending-answer removal path and the host-write gate. It is also
the one queued domain the original shell never hosted a command for, so it
validates net-new grammar on the working engine. Next, unchanged: `service 8/8`,
`media 8/13`, `database 8/14`, `files 8/17`, then the thin and multi-backend
ones.

## 9. For the founder to rule on

1. **Wayland arm deferred (honest decision, documented):** the wlr-data-control
   read/write is not shipped because this X11-only box cannot prove it; the X11
   read path is the proven native one. If you want Wayland code before a
   compositor exists here, that is a scope call — I built the honest alternative
   instead of untestable wire code (law #8 + the C1 D-Bus lesson).
2. **History is per-process, not the seat's** (DEGRADED-with-reason). A real
   cross-client history is a daemon's job (a clipboard manager); if `smart`
   should grow one, that is a service wave, not a backend method.
3. **Host-write gate UX** (unchanged from C1, re-used verbatim for clipboard):
   mutating verbs need `--force` or a REPL confirm; a pipe never confirms. The
   `clipboard set` live bring-back is yours.
4. **`watch clipboard`** is not wired this slice (the engine has a real change
   subscription `subscribeChanges`/`pull`/`cancel`, mock-proven; the REPL `watch`
   binding is network-only today). Wire it if you want the streaming surface.
