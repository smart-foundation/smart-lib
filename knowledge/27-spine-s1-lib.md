# 27 — Spine S1 (lib): files, process, settings, logging, clock, time served through the contract

Six domains, served through their **generated** `Server` traits, published
through the contract's own command registry, with the engine's own availability
in the registry's unavailable list. The network domain was the template; this
is the batch that makes the surface whole.

The rule that shaped every row: **no method of any of the six domains falls
through to the generated default.** The capnp default answers
`method <schema>::Server::<method> not implemented` — a lie about the
surface. Every one of the **197 contract methods** of the six domains has a
real arm: the backend's answer where the engine can act, and the contract's
own `notSupported` carrying the availability matrix's reason where it cannot.
Never a fabricated success; never a silent zero.

---

## 0. What was built

| file | lines | what it is |
|---|---|---|
| `src/serve/files.rs` | 3 733 | the `Files` interface (74), `FileService` (5), `FileEventSubscription` (2), `FileAuditSubscription` (2) |
| `src/serve/process.rs` | 2 319 | the `Process` interface (42), `ProcessSubscription` (2), `ExitNotificationSubscription` (2) |
| `src/serve/clock.rs` | 1 401 | the `ClockService` interface (20), `TimerSubscription` (3), `AlarmService` (8) |
| `src/serve/settings.rs` | 912 | the `Settings` interface (18), `SettingsSubscription` (2) |
| `src/serve/logging.rs` | 598 | the `Logging` interface (9), `LogSubscription` (2) |
| `src/serve/time.rs` | 438 | the `Time` interface (6) |
| `src/serve/availability.rs` | 127 | the registry's availability vocabulary (NEW) |

**197 contract methods served** (83 + 46 + 31 + 20 + 11 + 6), every one of
them a typed arm over the domain's own engine trait.

The domain's engine trait is the ONLY thing the serve layer talks to: no
ordinals, no method table, no envelope of this layer's own anywhere on the
path. The dispatch is the generated `ServerDispatch`.

---

## 1. The availability, per domain (the engine's own matrix)

The split is the domain's own `availability.rs` — read as data, never
re-derived. `REAL` + `DEGRADED` rows are served by the backend; `UNAVAILABLE`
rows are served by the matrix's answer, and published in the registry's
`listUnavailable` with the reason.

| domain | contract methods | matrix rows | REAL | DEGRADED | UNAVAILABLE | discovery state |
|---|---|---|---|---|---|---|
| files | 83 | 83 | 48 | 31 | 4 | `degraded` |
| process | 46 | 46 | 16 | 17 | 13 | `degraded` |
| settings | 20 | 20 | 11 | 7 | 2 | `degraded` |
| logging | 11 | 11 | 6 | 5 | 0 | `online` |
| clock | 31 | 30 | 25 | 5 | 0 | `online` |
| time | 6 | 8 | 6 | 1 | 1 | `degraded` |
| **total** | **197** | **198** | **112** | **66** | **21** | — |

Two matrix/contract mismatches, both understood and both honest:

* **clock**: the matrix has 30 rows, the contract has 31 methods. The extra
  one is `TimerSubscription.pullPaginated @2` — the contract's paginated pull,
  which the engine has no separate row for. It is served from the engine's
  `pull_timers` drain, sliced to the caller's window, with `totalCount` set to
  the drain's own count (the number the caller may page through, not a count
  of a list that is gone).
* **time**: the matrix has 8 rows, the contract has 6 methods. The two extra
  rows are `convertToTai` / `convertFromTai` — this domain's **mirror** of the
  contract's TAI converters, which live in `timezone.capnp`'s
  `TimezoneService`, a schema this batch does not serve. A mirror is not a
  contract method, so the registry publishes **no** TAI command for this
  domain: a registration naming a method the `Time` interface does not
  declare would be a command no client could address.

### The unavailable rows, and the reason each one carries

| domain | command | contract reason code | the matrix's class |
|---|---|---|---|
| files | `files encrypted`, `files decrypt`, `files encryption-info` | `unsupportedPlatform` | `not-applicable:` |
| files | `files analyze` | `unsupportedPlatform` | `not-applicable:` |
| process | `process attach`, `process write-memory`, `process group create`, `process group add`, `process group remove`, `process group foreground`, `process set-limit`, `process set-context`, `process supervise`, `process respawn`, `process exit`, `process step`, `process continue` (13) | `capabilityDenied` | `no-privilege:` |
| settings | `settings set-app-override`, `settings clear-app-override` | `capabilityDenied` | `no-privilege:` |
| time | `time verify-hsm` | `unsupportedPlatform` | `not-applicable:` |

The class → code map lives in one place (`src/serve/availability.rs`):
`no-backend` → `serviceUnavailable`, `no-privilege` → `capabilityDenied`,
`not-applicable` → `unsupportedPlatform`, `disabled-by-policy` →
`disabledByPolicy`, anything else → `unknown` (explicitly distinct, so a gap
is visible).

---

## 2. The registry — the CLI's surface, generated

`cli.capnp`'s `CommandRegistration` is filled in fully: `commandPath`,
`schemaId`, `interfaceName`, `methodName`, `presentation` (domain, summary,
**order**, **noArgsValid**), `unavailableReason`, and the open `metadata` map.

The metadata keys this batch publishes:

| key | meaning |
|---|---|
| `methodId` | the method's capnp ordinal, decimal — resolved from the generated contract table, never written by hand |
| `capnpSchema` | the schema's node tree, hex |
| `reason` | why the command cannot run: the engine's own reason class and words (`no-privilege: …`) |
| `degraded` | the named degraded capability, when the command answers but not completely |
| **`composes`** | the other contract methods one verb needs, comma separated, in the order a presenter should call them |

`composes` is the stable key the CLI's presenter composes from. The examples:
`files cat` → `openFile,closeFile`; `process inspect` →
`getProcessStatus,getAccounting`; `settings watch` → `watchDomain,pull,cancel`;
`clock watch` → `pull,cancel,pullPaginated`; `logging follow` → `pull,cancel`.

### The command paths, per domain (the product's own words)

The paths are what `smart <domain> …` types, taken from the coupled shell's
own command surface (`crates/shell/src/domains/{files,process,settings,logging,clock,time}/mod.rs`,
read READ-ONLY; the CLI agent retires those modules next). Where a contract
method has no shell verb, it is published anyway under a sensible path — the
surface must not hide a method the engine serves.

**files (83 paths — 79 available, 4 unavailable)**

```
files ls, files find, files tree, files stat, files xattrs, files cat,
files write, files du, files locks, files hash, files versions,
files branches, files store, files dedupe, files watch, files special,
files trash, files recents, files app, files grep, files compare,
files health, files quota, files mkdir, files touch, files rm, files mv,
files cp, files link, files lock, files unlock,
files open, files close, files flush, files exists, files create, files rmdir,
files hardlink, files realpath, files pwd, files cd, files version,
files version restore, files version compare, files by-hash, files ephemeral,
files encrypted*, files decrypt*, files encryption-info*,
files compression set, files compression, files dedupe-stats,
files batch copy, files batch move, files batch delete, files size, files stats,
files compare-dirs, files activity, files type, files checksum,
files lifecycle set, files lifecycle, files lifecycle apply, files audit,
files analyze*, files branch, files branch merge, files trash restore,
files trash empty, files recents clear, files quota set, files snapshot,
files acquire, files service open, files service read, files service write,
files service stat, files service close, files watch pull, files watch cancel,
files audit pull, files audit cancel
```

`*` = published in `listUnavailable` with the matrix's reason.

**process (46 paths — 33 available, 13 unavailable)**

```
process list, process inspect, process status, process tree, process threads,
process env, process maps, process limits, process accounting,
process resource-accounting, process groups, process group,
process group create*, process group add*, process group remove*,
process group foreground*, process stats, process context, process set-context*,
process set-limit*, process spawn, process wait, process set-env,
process attach*, process detach, process read-memory, process write-memory*,
process watch, process watch-exit, process supervise*, process report-failure,
process respawn*, process respawn-status, process exit*, process breakpoint set,
process breakpoint remove, process breakpoint list, process watchpoint set,
process watchpoint remove, process watchpoint list, process step*,
process continue*, process pull, process subscription cancel, process exit pull,
process exit cancel
```

**settings (20 paths — 18 available, 2 unavailable)**

```
settings domains, settings list, settings get, settings set, settings delete,
settings clear, settings watch, settings watch-domain, settings bulk-set,
settings bulk-get, settings category, settings set-app-override*,
settings get-app-override, settings clear-app-override*, settings version,
settings set-if-unchanged, settings history, settings diff, settings pull,
settings cancel
```

**logging (13 paths — all available)**

```
logging sinks, logging domains, logging tail, logging entries, logging follow,
logging emit, logging level, logging set-level, logging sampling,
logging clear, logging export, logging pull, logging cancel
```

**clock (34 paths — all available)**

```
clock info, clock now, clock monotonic, clock uptime, clock resolution,
clock sync, clock security, clock domains, clock quality, clock drift,
clock sleep, clock sleep-cancellable, clock timer create, clock timer check,
clock timer reset, clock timer cancel, clock timers, clock persist-timer,
clock set-timer-dependency, clock watch, clock timer pull,
clock subscription cancel, clock timer pull-paginated, clock virtual create,
clock virtual advance, clock alarms, clock alarm list, clock alarm next,
clock alarm get, clock alarm create, clock alarm update, clock alarm snooze,
clock alarm dismiss, clock alarm delete
```

**time (6 paths — 5 available, 1 unavailable)**

```
time now, time cal, time add, time sub, time diff, time verify-hsm*
```

### The coverage rule, proved

Every domain's registry names **every** contract method of that domain: the
six tests assert the distinct method-name set (81 for files — `pull` and
`cancel` are each named by two interfaces; 44 for process, likewise; 20
settings, 11 logging, 31 clock, 6 time).

---

## 3. The discovery descriptor

Each domain publishes its own descriptor: the service id (the schema's
canonical id), the name, the category, the endpoints, the **state** (the
domain's own — `online` when every row is served, `degraded` when some are
not, `offline` when none are), and its capability tokens.

`requiredCapabilityIds` is the contract's list of opaque `T.CapabilityHandle`
**numbers**, and the domains' capabilities are an open vocabulary of **words**
(`files.read`, `time.datetime`, …). The numbers therefore stay empty (the
authority the engine really holds is the connection's class, published through
the capability service), and the words travel in the descriptor's **open
metadata** under the key `capabilities`, where an open vocabulary belongs.
**This is a schema observation, not a change** — see §6.

---

## 4. The tests

Six new binaries, one per domain, plus the shared harness
(`tests/serve_common/mod.rs`). Each test drives the service the way a
*contract-only* client would: through the generated `Client` of the contract's
own schema, over a unix socket, with the service served on the test's own
thread.

| binary | tests | what it proves |
|---|---|---|
| `tests/serve_time.rs` | 7 | every method answers or refuses with a reason; the matrix's reason on the HSM verb; the registry's paths; the unavailable list; the descriptor; the matrix's own split; the coverage rule |
| `tests/serve_logging.rs` | 6 | the same, plus the subscription is the contract's own capability (a reference, no token) |
| `tests/serve_settings.rs` | 8 | the same, against a schema the HOST actually publishes (read through `listDomains` + `getAll`) |
| `tests/serve_clock.rs` | 8 | the same, plus the timer subscription and the alarm service as the contract's own capabilities |
| `tests/serve_process.rs` | 8 | the same, plus the process subscription and the exit-notification subscription |
| `tests/serve_files.rs` | 7 | the same, plus the bound file service (`acquireFile` → a reference bound to ONE path) and the file-event subscription |
| **total** | **44** | |

The central assertion (`assert_honest`): a refusal must NOT be the generated
`not implemented` default, and must carry a reason. That is the property the
whole batch exists for.

The commands that produced the evidence:

```
cargo test -j 6 -p smart-lib --lib                                  # 568 passed; 0 failed
cargo test -j 6 -p smart-lib --test codegen                         # 23 passed
cargo test -j 6 -p smart-lib --test smoke                           # 5 passed
cargo test -j 6 -p smart-lib --test serve_rpc                       # 28 passed
cargo test -j 6 -p smart-lib --test serve_time                      # 7 passed
cargo test -j 6 -p smart-lib --test serve_logging                   # 6 passed
cargo test -j 6 -p smart-lib --test serve_settings                  # 8 passed
cargo test -j 6 -p smart-lib --test serve_clock                     # 8 passed
cargo test -j 6 -p smart-lib --test serve_process                   # 8 passed
cargo test -j 6 -p smart-lib --test serve_files                     # 7 passed
cargo build  -j 6 -p smart-lib --example smart-service              # the daemon, all seven domains
```

The daemon's own words, after the batch:

```
smart-service: serving the contract on unix at /tmp/smart-lib-s1-smoke.sock
smart-service:   domain network  (interface id 0xd589365b57b982b8, service id 0xa1b2c3d4e5f6000f)
smart-service:   domain files     (interface id 0xa4cf61ba6c5f84e2, service id 0xa1b2c3d4e5f60025)
smart-service:   domain process   (interface id 0x85b5e006df9e5714, service id 0xa1b2c3d4e5f60018)
smart-service:   domain settings  (interface id 0xbc898a5763ddc512, service id 0xa1b2c3d4e5f60022)
smart-service:   domain logging   (interface id 0xdc9c434baafae246, service id 0xa1b2c3d4e5f60024)
smart-service:   domain clock     (interface id 0xd7f7f8a813f757c7, service id 0xa1b2c3d4e5f60040)
smart-service:   domain time      (interface id 0xb7467f2f404d6ab6, service id 0xa1b2c3d4e5f6001e)
```

---

## 5. What the engine was allowed to do, and what it was not

* **The class gate.** A mutating verb on a read-only connection is refused
  **before** the backend is reached (law #5) — a change that is refused
  afterwards has already happened. The gate is per domain, over the verbs that
  change the host.
* **The matrix's answer comes first.** What the host cannot do, no class can
  do: an `UNAVAILABLE` row answers `notSupported` with the matrix's reason
  before the class is consulted.
* **The subscriptions are capabilities, not ids.** `subscribeProcess`,
  `subscribeDeath`, `subscribeTimers`, `subscribeFileAudit`, `watchDirectory`,
  `subscribe`, `watch` and `watchDomain` all hand back an **interface
  reference**; holding it is the authority to pull. There is no subscription
  id to present, and no per-method token anywhere in the six files.
* **`acquireFile` mints a bound capability.** The `FileService` it hands back
  is bound to ONE path: every method goes through the engine's `bound_*`
  family, which refuses a path that is not the one the capability was
  acquired for. The contract's `Capability.id` is opaque bytes; the engine's
  authority is an opaque `u64`, so the bytes are folded into it with FNV-1a
  (the backend treats the token as opaque and never interprets it).

---

## 6. The schema tree — observations, NOT changes

The schema tree was **not** edited. Four observations for its owners:

1. **`logging.capnp`'s `LogOutcome.dropped` is a `Void` arm**, although its
   own comment says "`reason` is open; the caller decides what to do". The
   drop reason therefore cannot cross the contract boundary — the engine
   keeps it (`LogOutcome::Dropped(&str)`), the contract's arm is set, and the
   reason is lost to the caller. A `dropped @2 :Text;` (or a `reason @5 :Text`
   field) would close the gap the comment already promises.
2. **`T.HashOutput.algorithm` is an ordinal with no declared vocabulary.**
   No schema in the tree declares a `HashAlgorithm` enum (the field's comment
   points at `signature.capnp`/`crypto.capnp`, which have none). The serve
   layer writes `0` and carries the algorithm's **name** in the surrounding
   metadata wherever the struct has one; `T.HashOutput` on its own
   (`FileHashResult.hash`, `FileInfo.contentHash`) has no metadata, so the
   ordinal stands alone.
3. **`discovery.capnp`'s `requiredCapabilityIds` is a list of opaque
   `T.CapabilityHandle` numbers**, while a domain's capabilities are an open
   vocabulary of words. There is no home in that field for a token like
   `files.read`; the words travel in the descriptor's open metadata under
   `capabilities` (§3).
4. **`clock.capnp`'s `TimerSubscription.pullPaginated @2` has no engine
   counterpart** — the engine's `pull_timers` is a drain with no window. The
   contract's paginated pull is served from that drain (§1), and the total is
   the drain's count, because a count of a list that is gone would be a lie.

---

## 7. The engine bug this batch found (NOT mine to fix)

The Linux **settings** backend calls GIO's `g_settings_get_value` for a key
the schema does not carry (`src/platforms/linux/settings.rs:439` in
`bulk_get`, and the `connect_changed` path in `watch`/`watch_domain`). GIO
**aborts the process** on an unknown key — it is a `g_error`, not an error
return. The engine's `get` guards itself with `schema.has_key(key)`; the
other readers do not.

This is why the settings test reads a domain and a key the host actually
publishes (through `listDomains` + `getAll`) instead of inventing one: the
invented key would abort the test process, not fail an assertion. The
backend's other readers need the same guard. **Reported, not fixed** — the
settings domain is not this batch's engine.
