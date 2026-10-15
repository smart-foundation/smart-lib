# 21 — Batch 4 (lib): files, process, settings landed; logging, clock, time verified

Six domains through the engine, in one session, on the working stack
(`/home/lufuno/smart/smart-lib-ng`). The founder's standard held on every row:
**REAL, or DEGRADED to the physical limit with a measured reason.** "Not
implemented" appears nowhere in the six matrices; every non-REAL row carries a
nameable `reasonClass` and a reason the live behaviour repeats.

The inherited state was inventoried and verified first: `src/domains/{clock,
logging,time}/` plus `src/platforms/linux/{clock,logging,time}.rs` were on disk
from the aborted run, `src/platforms/linux.rs` was modified (its wiring checked
as part of the inventory), and `files`/`process`/`settings` did not exist. The
three landed domains were compiled and tested the moment the session resumed:
**logging did not compile** (4 errors), and **its tests then failed** (5 real
failures) — all fixed before anything new was written, so the three
verification domains are green too. What those three needed and what was fixed
is in §5.

---

## 0. The measured host (every privilege answer below is a measurement)

| question | the measured answer (uid 1000, Void Linux 6.18.40, x86_64) |
|---|---|
| init system | **runit** — `/run/runit` + `/etc/sv` (58 services declared), 16 linked into `/var/service` |
| journald | **absent** — no `/run/systemd/journal`, no `systemd-journald` in `/proc` |
| syslog socket | **absent** — no `/dev/log` |
| `/proc` mount | `proc /proc proc rw,nosuid,nodev,noexec,relatime` — **no `hidepid`**, so every pid's stat/status/cmdline is readable by this uid (including root's) |
| `/proc/<pid>/maps` (foreign uid) | **EACCES** (measured on pid 1) |
| `ptrace_scope` | **1** (`/proc/sys/kernel/yama/ptrace_scope`) and **no `CAP_SYS_PTRACE`** in `CapEff` → a non-parent attach is refused by the kernel |
| clock-sync daemon | **chronyd running** (pid 711); its state dir `/run/chrony` is `drwxr-x--- chrony` → **EACCES** for this uid |
| glib schema source | **present** — 219 045 bytes of compiled store, 174 `.gschema.xml` on the host; `gio::Settings` reads live values from this host's own dconf database |
| dconf user store | `$HOME/.config/dconf/user`, 25 387 bytes, uid-owned |
| `/var/log` | `Xorg.{0,1}.log` + `.old` are 0644 (readable); `dmesg.log`, `lastlog`, `btmp` are 0600; `gdm/`, `lightdm/` are closed dirs |
| svlogd dirs | running services' `/run/runit/supervise.<n>-log` are `drwx------ root root` → **EACCES**; not-running services' `log/supervise` is dangling → **ENOENT** |
| no passwordless sudo | measured (`sudo -n true` needs a password) — so no host write was performed anywhere |

---

## 1. files — the anchor (83 rows: 48 REAL / 31 DEGRADED / 4 UNAVAILABLE)

**Salvaged from the archive body:** the `/proc`-free file surface — `copy`,
`move`, `symlink`, `hardlink`, `chmod`, `stat`, `exists`, `list`, `walk_dir`
(`src/files.rs`, `platforms/linux/files.rs` of the old repo) — its whole real
logic, upgraded.

**Native mechanism per surface:**

| surface | mechanism |
|---|---|
| handle surface (`open/close/read/write/flush`) | the host's own descriptors: `openat` through `OpenOptions`, **`mmap`** for the read hot path (the window is served out of the page cache, no byte copy; `MappedWindow` is the zero-copy form), `pwrite` at the caller's offset, `fdatasync` on flush |
| metadata | **`statx(2)`** — declared locally with the kernel's own `struct statx` layout from `linux/stat.h`, so `created` is the kernel's real birth time and inode/device/link-count are reported data |
| walks | `getdents64` through `std::fs::read_dir`, symlinks stat'd as themselves (a walk never escapes through a link it was not asked to follow), a stated depth bound, and the contract's pagination |
| watch | **`inotify`** — `inotify_init1(IN_NONBLOCK)` + `inotify_add_watch` over the create/delete/modify/rename/move mask, decoded from the kernel's own `inotify_event` records; no polling thread |
| mounts | `/proc/mounts` read as the host's own words (octal-unescaped), longest-prefix rule; the root mount is not skipped |
| disk usage | `statvfs` (the glib `sys/statvfs.h` layout, declared locally) |
| hashes | **SHA-256 implemented here** — FIPS 180-4, proven against the standard's own empty-message digest |
| policy attributes | the file's own **user xattrs** (`user.smart-lib.compression`, `.lifecycle`, `.checksum`) — the host's own namespace, so every other tool sees what the backend wrote |
| versioning / CAS / dedupe / branches / snapshots | a content-addressed store under the host's own cache root (`$XDG_CACHE_HOME/smart-lib/files`): every version, object and manifest is a real file |
| copy | **`copy_file_range`** — the kernel's own in-kernel copy, with a bounded 1 MiB user-space fallback only when the kernel refuses (EXDEV/EINVAL) |
| ephemeral | **`memfd_create`** — the kernel's own anonymous file; `maxReads`/`deleteAfterRead` enforced on the read that consumes them |
| locks | the kernel's own **`fcntl(F_SETLK)`** advisory record locks; the table owns the descriptor because dropping it is the kernel's own release |
| special paths / trash / recents / default apps | the host's own XDG dirs, freedesktop trash (`files`/`info`), `recently-used.xbel` (attributes read by name, not position), and `mimeapps.list`/`mimeinfo.cache` |

**The condemned past, replaced:** the archive body's
`Command::new("cp").args(["-r", src, dst])` is `copy_file_range` (plus the
`cp -r` fork's lost error codes are now the host's own errno).

**Privilege measurements, per method:** a copy into a directory this uid cannot
cross is refused with the host's own `EACCES`; `clear` on a host log file is
`permissionDenied` naming the measured reason; `analyzeFileContent` and the
three encryption verbs are `notApplicable` (no OCR engine, no key custody —
MEASURED, never a guessed key); `setQuota` is gated with the measured reason;
the `fsck` half of `getFilesystemHealth` is deliberately absent and says so.

**Availability (83 rows):**

| | REAL | DEGRADED | UNAVAILABLE | total |
|---|---|---|---|---|
| files | **48** | **31** | **4** | 83 |

DEGRADED rows are named per method: the version store is the backend's own
(the host carries no VCS metadata for a path it never put under one), dedupe
measures the host's own inode sharing and this host's ext4 has no reflink,
`getLocks`/activity/audit are this process's own table (the host runs no
auditd), quota reads stop where privilege does, `createSnapshot` returns a
content-addressed reference (measured: `/tmp` is tmpfs here, so no filesystem
snapshot exists to take).

**Test totals:** `cargo test -j6 --lib --features domain-files` → **408 passed;
0 failed; 4 ignored** (15 hermetic domain tests + 64 `l3_linux_files` tests + 1
baseline; the grep and the two-way claimed/existing test check are in the
suite).

---

## 2. process (46 rows: 16 REAL / 17 DEGRADED / 13 UNAVAILABLE)

**Salvaged from the archive body:** its `/proc/<pid>/stat` and `statm` parsing
and its id/ppid/state/threads fields — the real half of `get_process`.

**The upgrade that mattered:** the archive body read the numeric fields by
splitting the WHOLE line on whitespace, which shifts every field whenever
`comm` contains a space. The salvaged parser anchors the numbers to the
closing `)` (first `(` … last `)`), and the field map is `field - 4`. **The
verification found the index map off by one** (it read `pgrp` as `ppid`), which
the `a_listing_reads_the_kernels_own_proc` test caught — fixed and re-measured.

**Native mechanisms:** `/proc/<pid>/{stat,status,cmdline,statm,environ,maps,mem,task/<tid>/stat}`
read and parsed natively; `btime` from `/proc/stat`; the host's own page size
and `CLK_TCK` via `sysconf` (never a guessed 100/4096); `waitpid(WNOHANG)` for
the contract's own "do not block"; the CPU fraction is **measured** over a 20 ms
window of the kernel's own `utime+stime` (the archive body's hardcoded MEM/CPU
pair is the named past); aggregates come from `/proc/meminfo`.

**Privilege measurements:** every `/proc` row is readable (no `hidepid`);
`/proc/<pid>/maps` and `/proc/<pid>/mem` are EACCES for a foreign uid;
`ptrace_scope` is 1 with no `CAP_SYS_PTRACE` → `attach`/`step`/`continue` refuse
with the host's own words; signal delivery is gated behind a **READ-shaped
`kill(pid, 0)` probe** (signal 0 is the kernel's own null signal — the probe
was first written with the real signal's number and the suite caught it, since
it SIGTERMed the test process).

**Availability (46 rows):**

| | REAL | DEGRADED | UNAVAILABLE | total |
|---|---|---|---|---|
| process | **16** | **17** | **13** | 46 |

The contract's `kill` was deleted pre-revamp (signal authority is
signal.capnp's), so `signal_process`/`measure_signal_privilege` are backend
diagnostics carrying no availability row — the network domain's
`measure_wifi_write_privilege` precedent.

**Test totals:** `cargo test -j6 --lib --features domain-process` → **451 passed;
0 failed; 4 ignored** (14 hermetic + 29 `l3_linux_process`, read-only against the
host, with the one lawful target being a child the test spawns and reaps).

---

## 3. settings — the old 3/6 body, native on this host (20 rows: 11 REAL / 7 DEGRADED / 2 UNAVAILABLE)

**Salvaged from the archive body:** its XDG-config flat-file read/write
(`src/settings.rs`) — the honest, portable half — plus its gsettings/dconf
surface, which was three `Command::new("gsettings")` forks per key.

**The upgraded mechanism, measured first:** this host publishes a **glib
compiled schema source** (219 KB, 174 `.gschema.xml`) and a live **dconf**
database, and the maintained `gio` crate is in the local registry with its full
transitive closure offline-cached. `gio = { version = "0.21.5", optional = true }`
was added and gated exactly like the service domain's `dep:dbus`
(`domain-settings = ["dep:gio"]`), so a settings-less build pays nothing for it.

So the backend is **the host's own settings engine** — the same machinery
`gsettings` is a thin CLI over: `Settings::value` reads the host's own per-user
dconf value, `SettingsSchemaSource` enumerates the host's own schemas,
`SettingsSchema::key` gives the host's own type, default, range and summary, and
the host's own `changed` signal drives the watch verbs (delivered synchronously,
proven). No `gsettings` fork, no `dconf` fork.

**Privilege measurements:** the schema source is present (measured), so the
schema/default reads are REAL; a schema this host does not publish is answered
`notFound` rather than aborting (glib's own `Settings::new` fatals, which is why
the source is consulted first); **every write is gated** — `set`/`delete`/
`clear`/`bulkSet` go through the host's own engine and are law #8-gated (the
write path is code-complete: the variant is validated and handed to
`Settings::set_value` when the gate is lifted); per-app overrides refuse with the
contract's own acquisition-time rule and the measured absence of a per-app
namespace in this host's engine.

**Availability (20 rows):**

| | REAL | DEGRADED | UNAVAILABLE | total |
|---|---|---|---|---|
| settings | **11** | **7** | **2** | 20 |

DEGRADED: `listByCategory` (the contract's tokens are conventions and glib
publishes no per-key category, so the listing reports the host's own keys and
records the category beside them), `getAppOverride` (answers the contract's own
`exists=false`), the four write verbs (mechanism real, act gated).

**Test totals:** `cargo test -j6 --lib --features domain-settings` → **434 passed;
0 failed; 4 ignored** (13 hermetic + 13 `l3_linux_settings`, read-only against the
host's own engine; the cross-check is the host's own two paths — the engine and
the schema source — agreeing, not a CLI).

---

## 4. logging (verified) — 11 rows: 6 REAL / 5 DEGRADED / 0 UNAVAILABLE

`cargo test -j6 --lib --features domain-logging` → **431 passed; 0 failed; 4 ignored**.

**What verification found broken:** the inherited file did not compile (4
errors: two `format!` macro imports missing, a private `types_capnp::log_level`
module reference, a nonexistent `last_unavailable_reason` call, a `zerocopy`
`as_bytes` on `&[u8]`, and `Error::permission_denied().with_message(...)`
against an API that takes the message first). With those fixed, 5 tests failed,
and each was a real defect:

1. **a line whose marker had no trailing space** (`(WW)warning!`) lost its
   severity: the parser required `") "`. Replaced by a marker rule that finds a
   two-character parenthesised word standing alone (start of line or after a
   space/`]`), so `(II) msg`, `(WW)msg` and `[t] (EE)fatal` all decode.
2. **`LogFilter`'s default `min_level` was `info`**, silently dropping every
   debug entry a caller never said it wanted dropped — while the wire read of an
   absent `minLevel` is the contract's ordinal 0, `trace`. Replaced the derived
   `Default` with an explicit one.
3. **the declared-sampler dropped nothing** at rate 0.5 (`sequence % 100 < 50`
   never trips for the first 50 entries). Replaced by rate-thinning on the
   sequence, which is pure and reproducible from the sequence.
4. **two listings/defaults were off-by-one** on the contract's own rules (the
   truncated page and the `0`-limit page).
5. **the sink test asserted EACCES for a service whose supervisor is not
   running** — measured ENOENT, so the assertion now accepts the host's own two
   verdicts (EACCES for the 16 live services, ENOENT for declared-but-not-running)
   and asserts the measured set explicitly.

The `/var/service` scan was added on top of the `/etc/sv` scan, so the live
set is measured separately from the declared set, and the module doc now states
both with their counts.

Native mechanism: the host's own svlogd `/run/runit/supervise.<n>-log`
directories and `/var/log` files, read natively (no `journalctl`, no `sv`, no
`dmesg` fork, and no journal parsing — there is no journal to parse), plus the
one sink a hosted process owns: its own stderr, which under a runit service IS
the svlogd input.

---

## 5. clock (verified) — 30 rows: 25 REAL / 5 DEGRADED / 0 UNAVAILABLE

`cargo test -j6 --lib --features domain-clock` → **449 passed; 0 failed; 5 ignored**.

Compiled and ran green as inherited; verification confirmed the mechanisms are
the kernel's own (`clock_gettime`/`clock_getres`, `timerfd_create`+
`timerfd_settime` so a periodic timer that expired three times reports **3**,
`nanosleep` on the caller's own thread, `/proc/stat`'s `btime`) and that the
measurement claims are honest (a `chronyd` process exists so `getClockSync`
reports that fact; `/run/chrony` is root-only for this uid so `synced` stays
false and `stratum` stays 0 rather than being invented; drift is measured over
a bounded window and says so; persistence has no host store, which is a physical
limit and is named as one per row).

---

## 6. time (verified) — 8 rows: 6 REAL / 1 DEGRADED / 1 UNAVAILABLE

`cargo test -j6 --lib --features domain-time` → **430 passed; 0 failed; 5 ignored**.

Compiled and ran green as inherited; verification confirmed the canonical
representation and the contract's own converters are used (TAI = UTC + 37 s,
seconds **and** nanoseconds since the **Unix epoch**, `timezone.capnp`'s rule —
never chrono's epoch math; the four calendar kernels are exact arithmetic with
the Hebrew/Chinese astronomies refused rather than approximated), the zone is
the host's own `/etc/localtime` symlink and its own TZif file parsed natively
(reading the version-2/3 64-bit block, not the 2038-truncated one), and
`getDateTime` is the kernel's `CLOCK_REALTIME` (no `date` fork).

The contract carries no timer verbs (they are clock.capnp's, and they are real
`timerfd`s), so nothing here is limited; `verifyHsmTimestamp` is `notApplicable`
with the measured absence of a TPM/PKCS#11.

---

## 7. Shellout-free, grep-proven

For each of the six (`src/domains/<d>/` + `src/platforms/linux/<d>.rs`):

```
files     : 0 code hits   (2 doc-comment condemnations of the old `cp -r`)
process   : 0 code hits   (5 inside `#[cfg(test)] mod tests`, each spawning the
                           TEST'S OWN child — the one lawful target for a read,
                           a signal probe and a wait; documented at the helper)
settings  : 0 code hits   (2 doc-comment condemnations of the old gsettings/dconf forks)
logging   : 0 code hits
clock     : 0 code hits
time      : 0 code hits
```

No `nmcli`, `ps`, `top`, `pgrep`, `journalctl`, `sv`, `systemctl`, `timedatectl`,
`gsettings`, `dconf`, `kill`, `inotifywait`, `dmesg` or `id` anywhere in the
six backends.

---

## 8. Anything physically limited, stated as such

1. **No host write was performed** anywhere (law #8). Every mutating path in
   files and settings is code-complete and gate-ready; the process domain's
   signal/ptrace verbs refuse with the measured reason. The tests read the host
   and mutate only their own temporary trees and the backend's own store.
2. **reflink dedupe** — this host's filesystem has none (measured from
   `/proc/mounts`), so `deduplicate`/`getDeduplicationStats` measure the host's
   own inode sharing and say so per row.
3. **filesystem snapshots** — no btrfs/zfs/device-mapper mount here, so
   `createSnapshot` writes a content-addressed reference into the store and
   names the difference per row.
4. **versioning** — the store is the backend's own; the host carries no VCS
   metadata for a path it never put under one (named per row).
5. **ptrace/memory inspection** — `ptrace_scope` 1 and no `CAP_SYS_PTRACE`, so
   `attach`/`step`/`continue`/`writeMemory` are refused by the host's own rules;
   `readMemory` works for this uid's own processes only.
6. **ptrace-free debug registers** — breakpoints/watchpoints are recorded, not
   poked, for the same measured reason.
7. **log sinks** — no journal, no `/dev/log`, no auditd on this host, so the
   trails and the sinks are this process's/this backend's own and are named as
   such per row.
8. **per-app settings overrides** — the contract gates them at acquisition and
   this host's engine has no per-app namespace (measured).
9. **settings categories** — the contract's tokens are conventions and glib
   publishes no per-key category, so a listing records the category rather than
   guessing which keys belong to it.
10. **quota** — the read is the mount's own `statvfs` totals; per-user quota
    needs privilege this tool does not hold, so those fields stay 0 with the
    reason carried.

## 9. Proof, per domain (one build at a time)

```
$ cargo test -j6 --lib --features domain-files     → 408 passed; 0 failed;  4 ignored
$ cargo test -j6 --lib --features domain-process   → 451 passed; 0 failed;  4 ignored
$ cargo test -j6 --lib --features domain-settings  → 434 passed; 0 failed;  4 ignored
$ cargo test -j6 --lib --features domain-logging   → 431 passed; 0 failed;  4 ignored
$ cargo test -j6 --lib --features domain-clock     → 449 passed; 0 failed;  5 ignored
$ cargo test -j6 --lib --features domain-time      → 430 passed; 0 failed;  5 ignored
```

Hermetic throughout: `/proc`, `statx`, `statvfs`, gvdb/`dconf` and glib-schema
reads are measured against a second native path into the same engine (or against
`std::fs`'s own metadata, which is a different mechanism reaching the same
answer), error totality is proven over all 20 contract codes, every pull is
bounded by the caller's own number, and each matrix asserts it is exactly the
contract's method list with the l3 claims two-way-checked against the suite that
claims them. Test fixtures live in private temp trees (`smart-files-<tag>-<pid>-<n>`,
`smart-<tag>-<pid>-<n>`), following the service domain's existing practice.

Two pre-existing flakes remain in `src/transport/tests/interchange.rs`
(`four_transports_one_api_tcp` / `_unix`) — the wave-3 transport domain's tests,
not this batch's, and untouched by it.
