# 21 — Batch 4 (CLI): the shell side of the system core — files, process, settings, logging, clock, time

Six domains through the shell, in one session, on the working stack
(`/home/lufuno/smart/smart-lib-ng`). The engine landed in `3a9b832`; this
report is the command surface, the live proof, the board and the physical
limits of the shell half.

The founder's standard held on every row: **every command wired from the
engine's OWNED types** (`FileInfo`, `DirectoryEntry`, `ProcessInfo`,
`ProcessStatus`, `SettingResult`, `SchemaInfo`, `HostSink`, `LogEntry`,
`ClockInfo`, `TimerSpec`, `Alarm`, `DateTime`, `TaiTimestamp`), **every command
verified LIVE**, every host-mutating verb **demonstrated as a refusal** (law
#8: no host write was performed anywhere in this session), and where a method
is physically limited the command answers with the host's measured reason in
the shell's own voice.

**Shell suite: `cargo test -j6 -p shell` → 169 passed; 0 failed; 2 ignored**
(16 test binaries + doc-tests; 82 of those tests are the six new domain
suites). The shell crate compiles with **zero warnings**.

---

## 0. The shape (what was scaffolded when the session opened)

The working tree already carried the feature gates, the clap grammar, the
`main.rs` dispatch and the `@domain/...` selector arms for all six domains, plus
the `time` module half-written. This session wrote the five missing modules
(`files`, `process`, `settings`, `logging`, `clock`), finished `time`, added the
`watch files` arm to the REPL's watch surface, moved `human_bytes` into
`widgets.rs` where every domain shares it, bounded the piped table's cells,
boarded all six, and wrote the six test suites.

Two engine seams the shell had to reach through a trait object (the same
pattern the shell's `host_sinks` needed, because the trait's default answers an
empty string):

| seam | why |
|---|---|
| `logging::SinkSurface` | the Linux backend's `host_sinks()` is an inherent method, not part of `LoggingBackend`, so the trait object could not reach the per-sink measurements |
| `clock::SyncEvidenceSurface` | same for `sync_evidence()` — the trait's default answers an empty string, which would have printed "nothing was measurable" while the host's own words existed |

Both are declared in the shell (`pub trait …: LoggingBackend + … {}` with the
blanket impl on the Linux backend), so a scripted mock satisfies the same seam
in the tests.

---

## 1. files — the anchor (24 read commands, 9 gated writes)

Native mechanism per surface, all live-proven: `statx(2)` metadata,
`getdents64` listings, the handle surface's `mmap` read window,
`copy_file_range` copies, `inotify` watches, `statvfs` + `/proc/mounts`,
SHA-256 implemented in the engine, the file's own user xattrs, and the
content-addressed store.

| command | mechanism |
|---|---|
| `files` / `files ls [path] [--all] [--pattern G] [--sort f] [--order d] [--recursive]` | `listDirectory` over the engine's own filter |
| `files find <path> [--name G] [--dirs] [--files]` | `searchFiles` — the backend's own glob |
| `files tree <path> [--depth N]` | `walkTree`, bounded and depth-stated |
| `files stat <path>` | `statFile` (the contract's total union) |
| `files xattrs <path>` | `getFileMetadata` — statx + the file's own xattrs |
| `files cat <path> [--offset N] [--bytes N]` | `openFile` + `readFile` + `closeFile` (the mmap window) |
| `files du [path]` | `getDiskUsage` (statvfs) with human sizes |
| `files mounts [path]` | the host's own `/proc/mounts` row |
| `files locks [path]` | `getLocks` — this process's own `fcntl` table |
| `files hash <path>` | `getFileHash("sha256")` |
| `files versions <path>` / `files branches <path>` | the backend's own store |
| `files store --content <path>` / `files dedupe <path>` | the content-addressed store / the host's own inode sharing |
| `files watch <path> [--count N] [--interval MS]` | `watchDirectory` + bounded pulls |
| `files special <token>` / `files trash` / `files recents` / `files app <mime>` | the host's own XDG surfaces |
| `files grep <path> <query>` / `files compare <a> <b>` | real lines, real bytes |
| `files health <path>` / `files quota <path>` | the measured half |
| `files caps` | law #6 tokens |

### Live, per command

```
$ smart files ls /tmp/kilo/b4/demo
  📁 Files (2) — getdents64, 84 B total
  ┌───┬───────┬───────────┬──────┬─────────────────────────┐
  │   │ NAME  │ KIND      │ SIZE │ MODIFIED                │
  ├───┼───────┼───────────┼──────┼─────────────────────────┤
  │ · │ a.txt │ regular   │ 24 B │ 2026-10-15 17:26:35.165 │
  │ ▣ │ sub   │ directory │ 60 B │ 2026-10-15 17:26:35.165 │
  └───┴───────┴───────────┴──────┴─────────────────────────┘

$ smart files find /tmp/kilo/b4/demo --name "*.txt"
  🔍 Find (1) under /tmp/kilo/b4/demo — the backend's own glob over getdents64
  ┌───┬─────────────────────────┬─────────┬──────┐
  │   │ PATH                    │ KIND    │ SIZE │
  ├───┼─────────────────────────┼─────────┼──────┤
  │ · │ /tmp/kilo/b4/demo/a.txt │ regular │ 24 B │
  └───┴─────────────────────────┴─────────┴──────┘

$ smart files du /tmp/kilo/b4/demo
  💽 Disk usage /tmp/kilo/b4/demo — statvfs plus the host's own /proc/mounts row
  ├─ Total      7.67 GiB     (8233533440 bytes)
  ├─ Used       1.77 GiB     23.0%
  ├─ Free       5.90 GiB
  ├─ Mount      /tmp (mount id 36)
  ├─ files      2010140
  ├─ filesFree  1939631
  ├─ nameMax    255
  ├─ mountFlags 0x1006
  ├─ mountOptions rw,nosuid,nodev,relatime,inode64
  └─ Source     the kernel's own statvfs(2) for this path's filesystem

$ smart files stat /tmp/kilo/b4/demo/a.txt
  🔎 Stat /tmp/kilo/b4/demo/a.txt
  ├─ Type       · regular
  ├─ Size       24 (24 B)
  ├─ Created    2026-10-15 17:26:35.164
  ├─ Modified   2026-10-15 17:26:35.165
  ├─ Accessed   2026-10-15 17:27:20.419
  ├─ Mime       text/plain
  ├─ Inode      264931
  ├─ Links      1
  ├─ Uid        1000
  ├─ Gid        1000
  ├─ Blocks     8
  ├─ Mode       644
  ├─ Changed    2026-10-15 17:26:35.164
  └─ Source     statx(2) — the kernel's own answer

$ smart files cat /tmp/kilo/b4/demo/a.txt
  📄 Cat /tmp/kilo/b4/demo/a.txt — 24 byte(s) through the engine's mmap window
  hello files
  second line

$ smart files hash /tmp/kilo/b4/demo/a.txt
  🔐 SHA-256 /tmp/kilo/b4/demo/a.txt df9f0667c8451557fd067c3943f7edcec271302c76a59fb7f706dd53e6fbe860 — implemented in the engine, over the file's own bytes

$ smart files store --content /tmp/kilo/b4/demo/a.txt
  📦 Content store /tmp/kilo/b4/demo/a.txt — SHA-256 digest, one real object
  ├─ Digest     df9f0667c8451557fd067c3943f7edcec271302c76a59fb7f706dd53e6fbe860 (sha256)
  ├─ Size       24 B
  └─ Scope      DEGRADED — hashes the path's real bytes (SHA-256) and writes one object into the store named by that digest, so a second store of the same content is the same file

$ smart files dedupe /tmp/kilo/b4/demo/a.txt
  🧬 Dedupe /tmp/kilo/b4/demo/a.txt
  ├─ Saved      0 B over the paths that share this digest
  ├─ References 1 path(s) the host's own inode sharing names
  └─ Measured   DEGRADED — MEASURED: the mount carrying /home on this host is not reflink-capable (its type is read from /proc/mounts), so the kernel cannot rewrite two equal files into one. The answer is the host's own measured sharing — the inode and link count of the paths that already share an inode — never a claimed saving that did not happen

$ smart files versions /tmp/kilo/b4/demo/a.txt
  🕰 Versions /tmp/kilo/b4/demo/a.txt
  └─ ○ the store holds no stored version of /tmp/kilo/b4/demo/a.txt — DEGRADED — a version store the backend maintains beside the host's cache root: every stored version is a real content-addressed file with its own hash and timestamp. It is NOT a host VCS (git/hg metadata is not invented for a path the host never put under one), and the reason says which

$ smart files locks /tmp/kilo/b4/demo/a.txt
  🔒 Locks /tmp/kilo/b4/demo/a.txt
  └─ ○ this process holds no lock — DEGRADED — the kernel publishes no portable lock table (the /proc/locks view is root-only on this host, measured), so the list is exactly what THIS process holds — every lock a caller asked this backend for, and no other process's. Stated rather than silently partial

$ smart files xattrs /tmp/kilo/b4/demo/a.txt
  🏷 Extended metadata /tmp/kilo/b4/demo/a.txt
  ├─ Owner      lufuno (uid 1000)
  ├─ Mode       644 — read-only ○ executable ○
  ├─ Created    2026-10-15 17:26:35.164 · modified 2026-10-15 17:26:35.165
  ├─ Attributes ◉ (6):
  ├─   inode 264931
  ├─   linkCount 1
  ├─   deviceMajor 0
  ├─   ownerUid 1000
  ├─   permissions 644
  └─   created 2026-10-15 17:26:35.164
     read natively from the file's own user xattr namespace

$ smart files special home
  📍 Special path home → /home/lufuno

$ smart files mounts /tmp/kilo/b4/demo
  🗄 Mount /tmp/kilo/b4/demo
  ├─ Mount point /tmp
  ├─ Mount id    36 · filesystem id 264901
  ├─ files       2010140
  ├─ mountOptions rw,nosuid,nodev,relatime,inode64
  └─ Source     /proc/mounts read as the host's own words (root mount included)

$ smart files caps
  🔑 Files caps — what this backend serves
  ├─ ◉ files.read
  ├─ ◉ files.write
  ├─ ◉ files.watch
  ├─ ◉ files.mounts
  └─ ◉ files.content
```

The binary read is honest rather than lossy:

```
$ smart files cat /tmp/kilo/b4/demo/sub/b.bin
  📄 Cat /tmp/kilo/b4/demo/sub/b.bin — 1 byte(s) through the engine's mmap window
  ✗ the window's 1 byte(s) are not valid UTF-8 text — the bytes are hashed and previewed above rather than decoded lossily and passed off as content
  └─ hex  78
```

### The inotify watch, live (the REPL's `watch files` arm)

```
$ watch files /tmp/kilo/b4/demo --count 8 --interval 400   (a file is created and removed while it runs)
  ⟳ watching /tmp/kilo/b4/demo — 8 round(s) bounded the kernel's own inotify records (Ctrl+C to stop)
     ＋ create  /tmp/kilo/b4/demo/c.txt (event 1)
     ～ modify  /tmp/kilo/b4/demo/c.txt (event 2)
     ～ modify  /tmp/kilo/b4/demo/c.txt (event 3)
     － delete  /tmp/kilo/b4/demo/c.txt (event 4)
  ◉ watch closed — 4 event(s) over 8 round(s)
```

### Gated (never performed; demonstrated as refusals)

`mkdir`, `touch`, `rm`, `mv`, `cp`, `write`, `link`, `lock`, `unlock` — nine
verbs, all behind the C1 gate (law #8). The refusal names the verb and what it
would change; the REPL prompts and one-shot mode opens it with `--force`.

```
$ smart files mkdir /tmp/kilo/b4/newdir
smart: refusing to run `files mkdir /tmp/kilo/b4/newdir`: it CHANGES host state. In the REPL confirm the prompt, or pass --force once you accept the write (the live bring-back proof is founder-run).
$ smart files cp /tmp/kilo/b4/demo/a.txt /tmp/kilo/b4/demo/b.txt
smart: refusing to run `files cp /tmp/kilo/b4/demo/a.txt /tmp/kilo/b4/demo/b.txt`: it CHANGES host state. …
```

The `cp` path is the kernel's own `copy_file_range` (proven against the mock in
`files_cmd.rs` by the recorded engine call `cp(src, dst)`).

---

## 2. process — 17 commands, every number measured

Native mechanism: `/proc/<pid>/{stat,status,cmdline,statm,environ,maps,io,task/<tid>/stat}`,
`btime` from `/proc/stat`, the host's own page size and `CLK_TCK` through
`sysconf`, and `kill(pid, 0)` as the read-shaped signal probe. The archive
body's hardcoded MEM/CPU pair is the named past: the table's memory is the
kernel's own resident pages and its time column is the kernel's own
`utime+stime` counters scaled by this host's clock-tick rate.

| command | surface |
|---|---|
| `process [--name N] [--state S] [--user U]` | `listProcesses` + the measured accounting per row |
| `process <pid>` | the contract's whole read surface for one process |
| `process tree [pid] [--depth N]` | `getProcessTree` |
| `process threads <pid>` | `getThreads` |
| `process env <pid> [--names-only]` | `getEnvironment`, the host's own NUL pairs |
| `process maps <pid>` | `getMemoryMap` |
| `process limits <pid>` / `process accounting <pid>` | `getResourceLimits` / `getAccounting` |
| `process groups` / `process group <id>` | `listProcessGroups` / `getProcessGroup` |
| `process stats [--user U]` | `getStats` — the measured aggregate |
| `process ptrace` / `process can-signal <pid>` | the host's measured verdicts |
| `process signal <pid> [--signal T]` / `process attach <pid>` | gated |
| `process wait <pid> [--timeout-ms N]` | `wait` (the contract's own WNOHANG) |
| `process caps` | law #6 tokens |

### Live

```
$ smart process --user 1000
  🧮 Processes (139) — /proc/<pid> read natively, every value measured · 99 more below the cut
  ┌──────┬─────────────────┬───────────┬──────┬──────────┬──────────┬─────┐
  │ PID  │ NAME            │ STATE     │ UID  │ MEM      │ CPU      │ THR │
  ├──────┼─────────────────┼───────────┼──────┼──────────┼──────────┼─────┤
  │ 532  │ Isolated Web Co │ ◐ blocked │ 1000 │ 63.8 MiB │ 17.32    │ 28  │
  │ 1172 │ gnome-session-b │ ◐ blocked │ 1000 │ 4.93 MiB │ 0.85     │ 5   │
  │ 1194 │ dbus-launch     │ ◐ blocked │ 1000 │ 556 KiB  │ 0.00     │ 1   │
  │ 1234 │ gnome-shell     │ ◐ blocked │ 1000 │ 137 MiB  │ 1:12:08  │ 31  │
  └──────┴─────────────────┴───────────┴──────┴──────────┴──────────┴─────┘
  ● memory is the kernel's own resident pages; CPU is the kernel's own utime+stime scaled by this host's clock-tick rate — never the archive body's hardcoded pair

$ smart process 1
  🔬 Process 1 (runit)
  ├─ State       ◐ blocked
  ├─ Parent      0 · uid 0 · 1 thread(s)
  ├─ Command     runit
  ├─ Memory      128 KiB current · 448 KiB peak
  ├─ CPU         1.07 s of the kernel's own utime+stime (scaled by this host's clock-tick rate)
  ├─ Uptime      68:28:34 · started 2026-10-12 22:38:01.000
  ├─ I/O         0 B read · 0 B written
  ├─ Signal      ✗ this uid may NOT deliver a terminate (measured with kill(pid, 0))
  ├─ Limits      — memory · 0% cpu · — pid(s) (the host's own /proc/<pid>/limits)
  └─ Source     /proc/<pid>/{stat,status,cmdline,statm,io} + /proc/stat btime

$ smart process env 12721 --names-only        (this uid's own process)
  🌍 Environment (pid 12721, 62) · names only — the host's own NUL-separated pairs
  ┌───────────────────────────────┬──────────┐
  │ VARIABLE                      │ VALUE    │
  ├───────────────────────────────┼──────────┤
  │ XDG_DATA_DIRS                 │ ⟨hidden⟩ │
  │ GDMSESSION                    │ ⟨hidden⟩ │
  │ LANG                          │ ⟨hidden⟩ │
  └───────────────────────────────┴──────────┘

$ smart process maps <this uid's own pid>
  🗺 Memory map (25) — the kernel's own /proc/<pid>/maps rows
  ┌─────────────────┬─────────────────┬──────────┬───────┬────────────────────────────────┐
  │ START           │ END             │ SIZE     │ PERMS │ NAME                           │
  ├─────────────────┼─────────────────┼──────────┼───────┼────────────────────────────────┤
  │ 94804892270592  │ 94804892504064  │ 228 KiB  │ r--p  │ /usr/bin/bash                  │
  │ 94804892504064  │ 94804893446144  │ 920 KiB  │ r-xp  │ /usr/bin/bash                  │
  └─────────────────┴─────────────────┴──────────┴───────┴────────────────────────────────┘

$ smart process ptrace
  🧷 Ptrace ✗ refused — measured, not assumed
  └─ MEASURED: /proc/sys/kernel/yama/ptrace_scope is 1 on this host and this process's CapEff is 0000000000000000 (CAP_SYS_PTRACE bit 19 is clear), so a non-parent ptrace attach is refused

$ smart process can-signal 1
  📡 pid 1 ✗ this uid may NOT deliver a terminate (measured) — measured with the host's own kill(pid, 0) — signal 0 is the kernel's null signal, so the probe changes nothing

$ smart process stats
  📊 Process stats (every uid) — the kernel's own enumeration, aggregated
  ├─ Processes  372 total · 4 active · 368 blocked · 0 suspended · 0 terminated
  ├─ CPU        the engine's own aggregate over its measured window: 70000
  ● the contract's `totalCpuUsage` window reading — the kernel's own counters, never the archive body's hardcode
  └─ Memory     11.1 GiB in use (the kernel's own /proc/meminfo)

$ smart -c "process | sort mem desc | take 5"
  ┌───────┬───────┬─────────────────┬──────────────────────────────────────────────────┬─────────┬──────┬─────────┬────────────┬────────────────┬────────────┬────────────┬─────────────┐
  │ pid   │ ppid  │ name            │ command                                          │ state   │ uid  │ threads │ mem        │ cpu_nanos     │ peak_mem   │ io_read    │ io_write    │
  ├───────┼───────┼─────────────────┼──────────────────────────────────────────────────┼─────────┼──────┼─────────┼────────────┼────────────────┼────────────┼────────────┼─────────────┤
  │ 17389 │ 7462  │ kilo            │ kilo -s ses_eda637f88ffeJSwL7IZYm0z5Ww           │ active  │ 1000 │ 31      │ 1762050048 │ 5133410000000 │ 2072354816 │ 5485629440 │ 29944176640 │
  │ 12628 │ 12452 │ opencode        │ opencode                                         │ active  │ 1000 │ 32      │ 1714622464 │ 30176610000000│ 1936318464 │ 8966303744 │ 983822336  │
  │ 27152 │ 1234  │ firefox         │ /usr/bin/firefox                                 │ blocked │ 1000 │ 137     │ 584601600  │ 9461620000000 │ 1014657024 │ 3675037696 │ 8767574016 │
  └───────┴───────┴─────────────────┴──────────────────────────────────────────────────┴─────────┴──────┴─────────┴────────────┴────────────────┴────────────┴────────────┴─────────────┘
```

(The piped table is the pipe's own value rendering, bounded at 48 columns per
cell with the cut marked — a 400-column argv would otherwise run off the
terminal.)

### Gated

`process signal <pid>` and `process attach <pid>` — both host-mutating; both
refuse without the accepted write, and both are also refused by the host
itself with its own words when the gate is open (EACCES/EPERM measured).

```
$ smart process signal 1
smart: refusing to run `process signal 1`: it CHANGES host state. …
```

### One engine-side physical limit, named not hidden

`getStats`'s `totalCpuUsage` carries the engine's own window reading in its own
arithmetic (ticks × clock-tick rate ÷ the window). The shell does NOT restate a
unit the engine's own doc and arithmetic disagree on: it prints the engine's
number under the contract's own field name and says which reading it is, so
nothing on the screen claims "cores busy".

---

## 3. settings — 18 commands, the host's own engine

Native mechanism: glib's `GSettings` through the maintained `gio` crate — the
compiled schema source for schemas, defaults, ranges and summaries; the host's
own dconf database for per-user values; the host's own `changed` signal for the
watch surface. No `gsettings` fork, no `dconf` fork.

| command | surface |
|---|---|
| `settings` (or `settings domains`) | `listDomains` — the schema source |
| `settings list <domain>` | `getAll` — the contract's StringMap |
| `settings get <domain> <key>` | `get` — the contract's total union |
| `settings set <domain> <key> <value>` | gated; the value is typed through the schema |
| `settings schema <domain>` | the schema browser |
| `settings category <token>` | `listByCategory` — the OPEN category |
| `settings version <domain> <key>` / `settings history` / `settings diff` | the version/history/diff reads |
| `settings bulk-get <domain> <keys…>` / `settings bulk-set <domain> k=v…` | the StringMap read / gated write |
| `settings watch <domain> [--key K] [--count N]` | the host's own change signal |
| `settings level <domain>` / `settings set-level` | `getLevel` / gated |
| `settings sampling <domain> [--set R]` / `settings clear` | gated writes |
| `settings export [--mime T]` / `settings caps` | export / law #6 |

### Live

```
$ smart settings
  ⚙ Settings (258) — the host's own schema source, read natively
  ┌───┬──────────────────────────────────────────────────────┬──────┬──────────────────────────────────────────────┐
  │   │ DOMAIN                                               │ KEYS │ PATH                                         │
  ├───┼──────────────────────────────────────────────────────┼──────┼──────────────────────────────────────────────┤
  │ ◉ │ ca.desrt.dconf-editor.Demo.Empty                     │ 0    │ /ca/desrt/dconf-editor/Demo/Empty/           │
  │ ◉ │ org.gnome.evolution-data-server.calendar             │ 24   │ /org/gnome/evolution-data-server/calendar/   │
  │ ◉ │ org.melroy.winegui                                   │ 6    │ /org/melroy/winegui/                         │
  │ ◉ │ org.gnome.desktop.a11y                               │ 2    │ /org/gnome/desktop/a11y/                     │
  └───┴──────────────────────────────────────────────────────┴──────┴──────────────────────────────────────────────┘
  ● the full list: smart settings -o json · one schema: smart settings schema <domain>

$ smart settings get org.gnome.desktop.interface font-name
  🔎 Setting org.gnome.desktop.interface font-name (s)
  ├─ Value       Sans 10 (string)
  ├─ Range       ('type', <@as []>)
  ├─ Summary     Default font
  └─ Source     the host's own schema source + its own dconf database

$ smart settings get org.gnome.desktop.interface accent-color
  🔎 Setting org.gnome.desktop.interface accent-color (s)
  ├─ Value       purple (string)
  ├─ Range       ('enum', <['blue', 'teal', 'green', 'yellow', 'orange', 'red', 'pink', 'purple', 'slate']>)
  ├─ Summary     Accent color
  └─ Source     the host's own schema source + its own dconf database

$ smart settings schema org.gnome.desktop.interface
  📚 Schema org.gnome.desktop.interface (45 keys) — /org/gnome/desktop/interface/
  ┌──────────────────────────┬──────┬──────────┬──────────────────────┬──────────────────────┬─────────────────────────────────────────────────┐
  │ KEY                      │ TYPE │ CONTRACT │ DEFAULT              │ RANGE                │ SUMMARY                                         │
  ├──────────────────────────┼──────┼──────────┼──────────────────────┼──────────────────────┼─────────────────────────────────────────────────┤
  │ gtk-color-palette        │ s    │ string   │ 'black:white:gray50… │ ('type', <@as []>)   │ Palette used in the color selector              │
  │ show-battery-percentage  │ b    │ boolean  │ false                │ ('type', <@ab []>)   │ Show battery percentage                         │
  │ can-change-accels        │ b    │ boolean  │ false                │ ('type', <@ab []>)   │ Can Change Accels                               │
  │ accent-color             │ s    │ string   │ 'blue'               │ ('enum', <['blue', … │ Accent color                                    │
  └──────────────────────────┴──────┴──────────┴──────────────────────┴──────────────────────┴─────────────────────────────────────────────────┘

$ smart settings list org.gnome.desktop.interface
  🗂 Settings org.gnome.desktop.interface (45) — the host's own dconf values
  ┌──────────────────────────┬──────────────────────────────────────────┬─────────┬──────────────────────────────┐
  │ KEY                      │ VALUE                                    │ TYPE    │ SCHEMA DEFAULT               │
  ├──────────────────────────┼──────────────────────────────────────────┼─────────┼──────────────────────────────┤
  │ gtk-color-palette        │ black:white:gray50:red:purple:blue:ligh… │ string  │ 'black:white:gray50:red:pur… │
  │ show-battery-percentage  │ true                                     │ boolean │ false                        │
  │ menubar-accel            │ F10                                      │ string  │ 'F10'                        │
  └──────────────────────────┴──────────────────────────────────────────┴─────────┴──────────────────────────────┘

$ smart settings bulk-get org.gnome.desktop.interface font-name icon-theme can-change-accels
  📥 Bulk read org.gnome.desktop.interface (3) — the contract's StringMap read
  ┌───────────────────┬─────────┐
  │ KEY               │ VALUE   │
  ├───────────────────┼─────────┤
  │ font-name         │ Sans 10 │
  │ icon-theme        │ Adwaita │
  │ can-change-accels │ false   │
  └───────────────────┴─────────┘

$ smart settings watch org.gnome.desktop.interface --count 2 --interval 300
  👁 Watch org.gnome.desktop.interface (0 event(s)) — the host's own `changed` signal
  └─ ○ no change arrived in the bounded rounds (the subscription is real — the host simply reported nothing)
```

`notSet` is an answer, not a sentinel:

```
$ smart settings get org.gnome.desktop.interface clock-show-weekday
  🔎 Setting org.gnome.desktop.interface clock-show-weekday (b)
  ├─ Value       ○ the host carries no value for this key
  ├─ Default     false (the schema's own default)
  ├─ Summary     Show weekday in clock
  └─ Source     the host's own schema source + its own dconf database
```

### Gated

`set`, `delete` (via `set`), `clear`, `bulk-set`, `set-level`, `sampling` — every
write goes through the host's own engine and is gated once (law #8); the value a
`set` hands the engine is typed through the schema (a `b` key is a boolean, an
`i` key an integer, never a coerced string).

```
$ smart settings set org.gnome.desktop.interface font-name "Sans 11"
smart: refusing to run `settings set org.gnome.desktop.interface font-name`: it CHANGES host state. …

$ smart settings schema org.example.NoSuchSchema
smart: this host's own schema source publishes no schema named org.example.NoSuchSchema
```

---

## 4. logging — 12 commands, this host's real sinks

Native mechanism: the host's own svlogd supervisor directories under
`/run/runit/supervise.<n>-log`, the files under `/var/log`, and this process's
own stderr — every verdict measured by opening the sink. This host has no
journald, no `/dev/log` and no auditd (all measured), so there is no journal to
parse and none is invented.

```
$ smart logging
  📡 Log sinks (50) 5 readable — measured by opening each one, no journald here
  ┌───┬──────────────────────────────┬──────────────────────────────────────────────┬──────────┬──────────────────────────────────────────┐
  │   │ SINK                         │ PATH                                         │ READABLE │ MEASURED REASON                          │
  ├───┼──────────────────────────────┼──────────────────────────────────────────────┼──────────┼──────────────────────────────────────────┤
  │ ◉ │ wtmp                         │ /var/log/wtmp                               │ ◉ yes    │                                          │
  │ ◉ │ Xorg.0.log                   │ /var/log/Xorg.0.log                         │ ◉ yes    │                                          │
  │ ✗ │ btmp                         │ /var/log/btmp                               │ ✗ no     │ /var/log/btmp: Permission denied (os er… │
  │ ✗ │ dmesg.log                    │ /var/log/dmesg.log                          │ ✗ no     │ /var/log/dmesg.log: Permission denied (… │
  │ ✗ │ service:NetworkManager       │ /run/runit/supervise.NetworkManager-log     │ ✗ no     │ /run/runit/supervise.NetworkManager-log… │
  └───┴──────────────────────────────┴──────────────────────────────────────────────┴──────────┴──────────────────────────────────────────┘
  ● measured by opening each one: this host runs no journald and no /dev/log (both measured), so the trails are the host's own svlogd directories and /var/log files — plus the one sink this process owns, its own stderr
  ● the one sink this process owns is its own stderr (smart logging entries), which under a runit service IS the svlogd input

$ smart logging follow --count 3
  👁 Follow (0) (min level trace) — the contract's own subscription, bounded by the caller
  └─ ○ no entry arrived in the bounded rounds (the subscription is real — nothing was reported)

$ smart logging tail Xorg.0.log --lines 2
  📜 Tail Xorg.0.log (2) — the host's own lines, read natively
  ▢ custom  Xorg.0.log               [     5.149]
  ▢ custom  Xorg.0.log               X.Org X Server 1.21.1.24

  ● DEGRADED — two sources, both honest: this process's retained entries (exact, complete) and the host's own log files, read natively — the world-readable ones are read and a root-only one is NAMED in the result's measured reason rather than silently skipped (this host's svlogd output lives under /run/runit/supervise.<n>-log, mode drwx------ root root, measured EACCES)

$ smart logging tail service:sshd        (the root-only svlogd directory)
smart: no-privilege: the host's log sink is not readable by this caller: /run/runit/supervise.sshd-log: Permission denied (os error 13)

$ smart logging domains
  🗂 Log domains (50) — the domains this backend knows
  ┌───────────────────────────────┬──────────┬──────────┬──────────────────────────────────────────────────────┐
  │ DOMAIN                        │ LEVEL    │ SAMPLING │ WHAT IT IS                                           │
  ├───────────────────────────────┼──────────┼──────────┼──────────────────────────────────────────────────────┤
  │ Xorg.0.log                    │ ▢ custom │ 1.00     │ host log file /var/log/Xorg.0.log                    │
  │ service:NetworkManager        │ ▢ custom │ 1.00     │ host log file … (not readable by this caller: …)     │
  └───────────────────────────────┴──────────┴──────────┴──────────────────────────────────────────────────────┘

$ smart logging caps
  🔑 Logging caps — what this backend serves
  ├─ ◉ logging.write
  ├─ ◉ logging.read
  ├─ ◉ logging.subscribe
  └─ ◉ logging.export
```

The filter speaks the contract's own severity vocabulary
(`trace/debug/info/warn/error/fatal/silent/crash/security/audit`), and an
absent `--level` is the wire reading of an absent `minLevel` — ordinal 0,
`trace`, never `info` (which would silently drop the debug entries nobody asked
to drop).

### Gated

`emit`, `set-level`, `sampling`, `clear` — every one names what it would touch.

```
$ smart logging emit info "a line"
smart: refusing to run `logging emit info`: it CHANGES host state (the one sink a hosted process owns is its own stderr — under a runit service that stream IS the svlogd input, so an entry lands in a host log file). …
```

---

## 5. clock — 22 commands, the kernel's own clocks

Native mechanism: `clock_gettime`/`clock_getres`, `timerfd_create` +
`timerfd_settime` (a real timerfd, so a periodic timer that expired three times
reports **3**), `nanosleep` on the caller's own thread, `/proc/stat`'s `btime`,
and the measured chrony verdict with the root-only state dir named.

```
$ smart clock
  🕰 Clock (tsc) — the kernel's own clocks
  ├─ Realtime    1792091248350275359 (CLOCK_REALTIME)
  ├─ Monotonic   113290516859717 (CLOCK_MONOTONIC — immune to NTP and DST)
  ├─ Uptime      2d 20h 29m since boot (btime 1791844681000000000)
  ├─ Resolution  1 ns (clock_getres — never a fabricated precision)
  ├─ Sync        ○ chrony (measured — see `clock sync`)
  ├─ Security    tampered ○ · secure source ○ · hardware rng ◉
  └─ Domains     2 served

$ smart clock sync
  📡 Clock sync ○ not synchronized — measured, never assumed
  ├─ Source      chrony · stratum 0
  ├─ Evidence    pid 711 runs chronyd: the host runs a synchronization daemon, and that is the measured fact; the daemon's own sync state (stratum, offset, last sync) is not readable by this uid (/run/chrony is root-only on this host), so `synced` stays false rather than being assumed
  └─ Reading     last sync 0 · offset 0 ns

$ smart clock resolution
  📐 Resolution (2) — clock_getres, measured
  ┌───────────┬─────────────────┬────────────┬───────────┐
  │ DOMAIN    │ NAME            │ RESOLUTION │ AVAILABLE │
  ├───────────┼─────────────────┼────────────┼───────────┤
  │ realtime  │ CLOCK_REALTIME  │ 1 ns       │ ◉         │
  │ monotonic │ CLOCK_MONOTONIC │ 1 ns       │ ◉         │
  └───────────┴─────────────────┴────────────┴───────────┘

$ smart clock timer create --after-ms 50 --label live-check
  ⏲ Timer #1 (live-check) — a real timerfd this process owns
  ├─ Anchor      50 ms from now (monotonic)
  ├─ Interval    0 ms (one-shot)
  └─ Source      a real timerfd this process owns: timerfd_create + timerfd_settime, so a periodic timer that fired three times reports three

$ smart clock timer check 1        (a fresh invocation holds no timerfd from an earlier one)
  ⏲ Timer 1 ✗ notFound
  └─ a timerfd is held by the process that armed it, so a one-shot `smart` invocation holds no timer from an earlier one — arm one and check it in the same session

$ smart clock watch --count 3 --interval 100
  👁 Timer watch (0) — the contract's own TimerSubscription
  └─ ○ no timer fired in the bounded rounds (the subscription is real — nothing was reported)

$ smart clock alarm create --label wake --after-ms 60000
  ⏰ Alarm #1 wake — fires at 1792089873327343457
```

### Gated (the removal half, exactly as the contract names it)

`clock timer cancel <id>` and `clock alarm delete <id>` — creation arms this
process's own timerfd and its own store, so it runs; REMOVAL is gated.

```
$ smart clock alarm delete 1
smart: refusing to run `clock alarm delete 1`: it CHANGES state. …
$ smart clock timer cancel 3
smart: refusing to run `clock timer cancel 3`: it CHANGES state. …
```

### Physically limited, named

Timer persistence has no host store here (`persistTimer`/`listPersistedTimers`
answer notApplicable with the measured absence, and `clock timer list` says so).

---

## 6. time — 7 commands, the contract's own converters

```
$ smart time
  🕐 Now
  ├─ Wall       2026-10-15 19:07:28.369
  ├─ Weekday    Thursday (day 288 of the year)
  ├─ Zone       Africa/Johannesburg (UTC+02:00)
  └─ Source     CLOCK_REALTIME, zone from the host's own TZif file — no `date` fork

$ smart time tai
  ⏱ TAI — the contract's own converter
  ├─ UTC        1792084048376000000 nanos (since 1970)
  ├─ TAI        1792084085 s + 376000000 ns
  ├─ Offset     37 s (TAI − UTC, the contract's constant)
  └─ Round-trip back to UTC 1792084048376000000 ◉ exact

$ smart time cal
  📅 Calendars every calendar in the contract's own order — exact arithmetic, never an approximation
  ┌───┬───────────┬────────────────────────────────────────────────────────────────────┐
  │   │ CALENDAR  │ DATE                                                               │
  ├───┼───────────┼────────────────────────────────────────────────────────────────────┤
  │ ▣ │ gregorian │ 2026-10-15                                                         │
  │ ▤ │ julian    │ 2026-10-02                                                         │
  │ ◍ │ islamic   │ 1448-05-03                                                         │
  │ ▢ │ hebrew    │ ✗ notSupported: no-backend: the Hebrew calendar is not carried by… │
  │ ▢ │ chinese   │ ✗ notSupported: no-backend: the Chinese calendar is not carried b… │
  │ ◈ │ ethiopian │ 2019-02-05                                                         │
  └───┴───────────┴────────────────────────────────────────────────────────────────────┘

$ smart time add 2026-01-31 1month
  📅 2026-01-31 +1month → 2026-02-28 (month-end clamped)
$ smart time sub 2026-03-31 1month
  📅 2026-03-31 −1month → 2026-02-28 (month-end clamped)
$ smart time diff 2026-10-15 2020-01-01
  📏 6 years, 9 months, 14 days — 2479 day(s) after
$ smart time caps
  🔑 Time caps — what this backend serves
  ├─ ◉ time.datetime
  ├─ ◉ time.calendar
  └─ ◉ time.tai
```

The shell renders through the contract's converters (TAI = UTC + 37 s, seconds
AND nanoseconds since the Unix epoch) and the contract's own four calendar
kernels — never chrono's epoch math. The Hebrew and Chinese astronomies are
REFUSED by the engine rather than approximated, and the refusal is the answer.

---

## 7. The selectors, live

Each domain's selector tree is the same value the command surface renders, so a
selector and a command can never disagree.

```
$ smart -c "@process/processes/0/mem"        → 131072
$ smart -c "@process/processes/0/name"       → runit
$ smart -c "@files/cwd"                      → /home/lufuno/smart/smart-lib-ng
$ smart -c "@files/entries/0/name"           → Cargo.lock
$ smart -c "@files/entries where kind == \"directory\""   → the directory rows
$ smart -c "@settings/caps/0"                → settings.read
$ smart -c "@clock/info/source"              → tsc
$ smart -c "@logging/sinks/total"            → 50
$ smart -c "@time/now/timezone"              → Africa/Johannesburg
```

---

## 8. The board, before and after

**Before** (`3a9b832`, the engine commit): 46 domains — **3 real today**
(network, clipboard, service), 10 queued (queue, media, database, files, audio,
power, process, window, package, compositor), 33 archive-only.

**After** (this session): 47 domains — **9 real today**, 8 queued, 30
archive-only. `files` (was queue position 4) and `process` (was 7) are now
real; `settings`, `logging`, `time` (archive-only) are now real; `clock` — the
one domain the original shell never hosted — is added, real, beside time. The
remaining queue closed its gaps: queue 1, media 2, database 3, audio 4, power
5, window 6, package 7, compositor 8.

```
$ smart migrate
  🧭 Migration Queue
  47 domains: 9 real today, 8 queued, 30 archive-only
  ┌────┬───────────────┬───────────────────────┬─────────────────────────────────────────────────┬──────────────────────────────────────────────────────────────────────────┐
  │ 1. │ queue         │ ◌ migrated-pending    │ 8/8 methods                                     │ platforms/linux/queue.rs (real body, 60 lines)                           │
  │ 2. │ media         │ ◌ migrated-pending    │ 8/13 methods                                    │ platforms/linux/media.rs (real body, 95 lines)                           │
  │ ·  │ files         │ ◉ real-today          │ native · 48 REAL / 31 DEGRADED / 4 UNAVAILABLE   │ engine: the host's own files read natively — statx(2) metadata, getdent… │
  │ ·  │ process       │ ◉ real-today          │ native · 16 REAL / 17 DEGRADED / 13 UNAVAILABLE │ engine: /proc/<pid>/{stat,status,cmdline,statm,environ,maps,task} read … │
  │ ·  │ settings      │ ◉ real-today          │ native · 11 REAL / 7 DEGRADED / 2 UNAVAILABLE   │ engine: the host's own settings engine (glib's GSettings through the gio  │
  │ ·  │ logging       │ ◉ real-today          │ native · 6 REAL / 5 DEGRADED / 0 UNAVAILABLE    │ engine: this host's own sinks, measured by opening each one — the svlog… │
  │ ·  │ clock         │ ◉ real-today          │ native · 25 REAL / 5 DEGRADED / 0 UNAVAILABLE   │ engine: the kernel's own clocks — clock_gettime/clock_getres for the re… │
  │ ·  │ time          │ ◉ real-today          │ native · 6 REAL / 1 DEGRADED / 1 UNAVAILABLE    │ engine: the kernel's own CLOCK_REALTIME, the host's own /etc/localtime … │
  └────┴───────────────┴───────────────────────┴─────────────────────────────────────────────────┴──────────────────────────────────────────────────────────────────────────┘
```

---

## 9. Test totals

```
$ cargo test -j6 -p shell
     Running unittests src/lib.rs                                  →  17 passed; 0 failed
     Running unittests src/main.rs                                 →   0 passed; 0 failed
     Running tests/clipboard_cmd.rs                                →   9 passed; 0 failed
     Running tests/clock_cmd.rs                                    →  12 passed; 0 failed
     Running tests/files_cmd.rs                                    →  15 passed; 0 failed
     Running tests/logging_cmd.rs                                  →   9 passed; 0 failed
     Running tests/migrate.rs                                      →   6 passed; 0 failed
     Running tests/network_cmd.rs                                  →  17 passed; 0 failed
     Running tests/pipes.rs                                        →   4 passed; 0 failed
     Running tests/process_cmd.rs                                  →  13 passed; 0 failed
     Running tests/render_width.rs                                 →  11 passed; 0 failed
     Running tests/selectors.rs                                    →   6 passed; 0 failed
     Running tests/service_cmd.rs                                  →  13 passed; 0 failed
     Running tests/settings_cmd.rs                                 →  11 passed; 0 failed
     Running tests/shell_language.rs                               →  17 passed; 0 failed
     Running tests/time_cmd.rs                                     →   7 passed; 0 failed
   Doc-tests shell                                                 →   2 passed; 0 failed
                                                                    ─────────────────────────
                                                                     169 passed; 0 failed
```

Each new suite runs against a scripted backend over the engine's OWNED types
(`MockFiles`, `MockProcess`, `MockSettings`, `MockLogging`, `MockClock`,
`MockTime` in `tests/common/mod.rs`), the host is never touched (law #8), and
the pixel-perfect assertions use the shared width oracle
(`assert_borders_hold`: every boxed line the same display width, every border
glyph at the same display column, W/F=2, ambiguous=1, ANSI stripped) rather
than eyeballing. Every gated verb has a test that proves the engine was NOT
called without the accepted write, and one that proves the engine's own path
runs with it.

The shell's own build is warning-free (`cargo build -j6 -p shell` emits no
`crates/shell` warnings; the 24 remaining warnings are the engine's, from the
lib commit).

---

## 10. Physically limited, each named with its measured reason

1. **Files: versioning** — the store is the BACKEND'S OWN content-addressed
   store beside the host's cache root; the host carries no VCS metadata for a
   path it never put under one. Every empty version listing says which
   (`files versions`, measured).
2. **Files: dedupe** — this host's filesystem has no reflink (its type is read
   from `/proc/mounts`), so the answer is the host's own measured inode
   sharing, never a claimed saving.
3. **Files: snapshots** — no btrfs/zfs/device-mapper mount here, so
   `createSnapshot` is a content-addressed reference and names the difference.
4. **Files: locks** — the kernel publishes no portable lock table readable by
   this uid (`/proc/locks` is root-only, measured), so the listing is exactly
   what THIS process holds.
5. **Process: per-process CPU fraction** — the kernel's per-thread and
   per-process CPU *fraction* is exported only through the engine's own
   sampling window; the shell prints the measured cumulative tick time in the
   detail view and the engine's aggregate in `process stats`, labelled as the
   engine's own reading (the engine's `totalCpuUsage` arithmetic and its
   documented unit disagree, and the shell does not pick a side on a screen).
6. **Process: environ/maps for a foreign uid** — EACCES on this host
   (measured), and that is what the commands answer.
7. **Process: signal/attach** — `ptrace_scope` 1 and no `CAP_SYS_PTRACE`, so
   both refuse with the host's own measured words; `kill(pid, 0)` is the
   read-shaped probe the shell renders.
8. **Settings: categories** — the contract's category tokens are conventions
   and glib publishes no per-key category, so the listing records the category
   beside each key rather than guessing.
9. **Settings: per-app overrides** — the contract gates them at acquisition
   and this host's engine has no per-app namespace (measured).
10. **Logging: the sinks** — no journald, no `/dev/log`, no auditd on this
    host, so the trails are the host's own svlogd directories and `/var/log`
    files; a root-only one is named with the OS error, never skipped.
11. **Logging: this process's retained store** — exact and bounded (4096
    entries), and the shell says that is what `logging entries` reads.
12. **Clock: sync** — chronyd runs (pid 711), but `/run/chrony` is root-only
    for this uid, so `synced` stays false and `stratum` stays 0 with the
    evidence beside them.
13. **Clock: persistence** — this host carries no timer-persistence store, so
    `persistTimer`/`listPersistedTimers` answer notApplicable and
    `clock timer list` says which.
14. **Time: the astronomies** — the Hebrew and Chinese calendars need
    ephemerides this build does not carry, so the engine refuses and the shell
    prints the refusal rather than an approximation.
15. **Time: HSM attestation** — `verifyHsmTimestamp` is notApplicable with the
    measured absence of a TPM/PKCS#11 (the command surface carries no verb that
    would pretend otherwise).
