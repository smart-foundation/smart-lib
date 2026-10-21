# 28 — Honest proofs: the matrix stops publishing proofs that do not exist

The founder's words (2026-10-20):

> *"there is a volume command which reduces volume, why doesnt the test read the current
> volume and change and return — actually the test which reduces the volume should go."*

and the doctrine (knowledge/00-CHARTER.md, law #8): a build or a test **never** changes host
state; when a test must exercise a mutating verb it **reads the current value, proves the write
path, and leaves the host exactly as it found it**.

This file records: the host's audio state as it is **now** against the value the tree recorded;
every host-mutating test that was found and what it does now; the per-domain count of the claims
that were made real, withdrawn or renamed; and the shape of the gate that makes the rot unable to
come back.

---

## 0. The host's audio state: measured now, and the drift

Measured on this box with the library's own backend (PulseAudio client library over
PipeWire-pulse), reading only — `cargo test -j6 --lib --features domain-audio` with a temporary
probe that printed the state, then the probe was removed.

| | recorded (knowledge/25-batch5-cli.md) | measured now | drift |
|---|---|---|---|
| default sink | index 61 | index **60** (`alsa_output.pci-0000_00_1f.3.analog-stereo`) | the sink's own index moved |
| default sink **volume** | **1.49** | **1.2489777** | **−0.241 (≈16 % lower)** |
| default sink **mute** | false | **false** | none |
| default output | 61 | 60 | moved |
| default input | 62 | 61 | moved |

**The volume has drifted from the recorded 1.49 to 1.2489777.** The mute has not (still false).
The default sink's index moved too, so the *device numbering* on this host is not the numbering the
tree recorded — that is what a device appearing or disappearing looks like, not a test writing a
volume.

The old test was `src/platforms/linux/audio.rs:749-757`:

```rust
let _ = b.set_volume(AudioDeviceId(d.id), 0.5);   // OVERWRITES the host's sink volume
let _ = b.set_mute(AudioDeviceId(d.id), false);    // UNMUTES the host's sink
```

It wrote 0.5, swallowed the errors, and never restored. It would have left the founder's sink at
0.5 — not at 1.49, and not at 1.2489777 either, so the drift on this box was **not** written by
that test alone; something else on the host moved the sink between the recorded run and now
(measured: the value also moved 1.4173737 → 1.2489777 *during* this work, while the suite was
provably not the cause — see §2).

**What the suite does now, measured:** running the audio suite leaves the sink at
`volume 1.2489777, mute false` — byte-identical before and after, and identical after running
the two mutating tests alone:

```
$ cargo test -j6 --lib --features domain-audio zz_final_host_audio_measurement -- --nocapture
MEASURE state = Ok(AudioState { volume: 1.2489777, mute: false, default_output: 60, default_input: 61 })
$ cargo test -j6 --lib --features domain-audio set_mute_proves -- --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
$ cargo test -j6 --lib --features domain-audio zz_final_host_audio_measurement -- --nocapture
MEASURE state = Ok(AudioState { volume: 1.2489777, mute: false, default_output: 60, default_input: 61 })
$ cargo test -j6 --lib --features domain-audio set_volume_proves -- --nocapture
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.02s
$ cargo test -j6 --lib --features domain-audio zz_final_host_audio_measurement -- --nocapture
MEASURE state = Ok(AudioState { volume: 1.2489777, mute: false, default_output: 60, default_input: 61 })
```

(The probe was a temporary test, added for the measurement and removed afterwards; the tree does
not carry it.)

---

## 1. The host-mutating tests: every one found, and what it is now

| # | where | what it did | what it does now |
|---|---|---|---|
| 1 | `src/platforms/linux/audio.rs:749` `set_volume_round_trips_through_pulse` | wrote 0.5 to the host's **default sink** and unmuted it, swallowed both errors, restored nothing | **gone.** The verb is now proven by four tests that read → prove → restore (§2) |
| 2 | `src/platforms/linux/files.rs` `the_cwd_is_the_processes_own` | `chdir`'d the **process** into a sandbox and restored it only on the happy path — a failed assertion left the test binary in the wrong directory | split into `get_current_directory_is_the_processes_own` (a read) and `change_directory_is_real_and_is_restored`, which restores the process's own directory through a `CwdGuard` whose `Drop` runs while a failed assertion unwinds |
| 3 | `src/platforms/linux/files.rs` `sandbox()` | every files test built a private tree under `/tmp` and **left it there** (65 trees per run) | the root is registered in a thread-local and removed when the test's own thread ends — on every path out of the test, including a failed assertion |
| 4 | `src/platforms/linux/{audio,appindex,content,database,manifest,media,power,queue,vcard}.rs` `fn core_works` | nine placeholder tests that called a few getters and **discarded the answers**; five of them were published as the proof of 71 method rows | all nine are gone. Each of the 71 rows now names a test that exercises that method and asserts a real outcome |
| 5 | `src/platforms/linux/power.rs` `setProfile` | the row claimed `core_works` for the **cpufreq governor write** — a proof that never touched the governor | `set_profile_proves_the_write_path_and_leaves_the_host_as_it_found_it`: reads the active profile, writes another, reads the host back, restores the original through a `Drop` guard; on a host where the write is not permitted the refusal is the proof |
| 6 | `src/platforms/linux/media.rs` | the nine MPRIS verbs (`play`, `pause`, `stop`, `next`, `previous`, `seek`, `setVolume`, `setPosition`, `openUri`) were published as proven by `core_works` | **no automated proof exists** for a verb that moves the founder's own player, so the claims were **withdrawn** and the rows now say so (`degraded_unproven`): the write path is code-complete and gated, the live proof is the founder's (§4) |

### Assessed and deliberately unchanged

| where | what it does | why it stays |
|---|---|---|
| `src/platforms/linux/process.rs` `owned_child()` | spawns `/bin/true` as the test's own child | the process domain cannot measure `/proc/<pid>/environ` or a real exit notification without a child of its own; the child is short-lived and the test reaps it. No process the test did not start is touched, and the `spawn` **verb** is gated (the test proves the refusal) |
| `src/platforms/linux/settings.rs` (the write verbs) | `set`/`delete`/`clear`/`bulkSet` are gated by the backend and answer `no-privilege:` | a gated write that never happens changes nothing; the per-method proofs assert the refusal *and* that the host's value is exactly as it was found |
| `src/platforms/linux/clipboard.rs` | the selection is exercised against the engine's scripted seat, never a live X selection | the live test **reads** the host's selection and writes none |

---

## 2. The mutating verbs, in the founder's own form

`src/platforms/linux/audio.rs` — the four verbs that touch the host, each with the same shape:

* **read** the host's own value first (the sink's volume and mute, the stream's volume and mute);
* **prove** the write path reached the host — write a value the host can tell apart from its own
  (a 0.1 delta, so the founder's ear is not jolted for the length of the write), then read the
  host back and assert it reports the new value;
* **restore** the original through a `Drop` guard, so the restore runs **while a failed assertion
  unwinds** — on every path, not only the happy one;
* assert the host is **exactly** as it was found, after the restore.

The four mutating audio tests take one lock, so two tests never write the same sink at once and
no test ever reads a host that another test is halfway through changing.

**The one honest caveat:** the restore is a `Drop`, so a test binary **killed** between the write
and the restore would leave the sink at the probe value. The window is milliseconds; the doctrine's
own form (read → prove → restore) has this window by construction.

---

## 3. The per-domain claims: made real, withdrawn, renamed

Measured on the tree at `HEAD` before this work and after it (`git archive HEAD` → audited, the
same script over the working tree):

| domain | claims before | claims after | renamed to a new proof | withdrawn | distinct proofs now |
|---|---|---|---|---|---|
| appindex | 8 | 8 | 3 | 0 | 8 |
| audio | 16 | 16 | **16** (all were `core_works`) | 0 | 16 |
| clipboard | 20 | 20 | 4 (+5 rows that named a test which did not exist — now it does) | 0 | 20 |
| clock | 30 | 30 | 8 | 0 | 30 |
| content | 10 | 10 | 4 (+3 rows that named a test which did not exist — now it does) | 0 | 10 |
| database | 27 | 8 | 8 (all were `core_works`) | **19** | 8 |
| files | 79 | 79 | 27 | 0 | 79 |
| logging | 11 | 11 | 4 | 0 | 11 |
| manifest | 5 | 5 | 1 | 0 | 5 |
| media | 15 | 6 | 6 (all were `core_works`) | **9** | 6 |
| network | 23 | 23 | 1 | 0 | 23 |
| power | 6 | 6 | **6** (all were `core_works`) | 0 | 6 |
| process | 33 | 33 | 13 | 0 | 33 |
| queue | 7 | 7 | **7** (all were `core_works`) | 0 | 7 |
| service | 19 | 19 | 10 | 0 | 19 |
| settings | 18 | 18 | 5 | 0 | 18 |
| time | 7 | 7 | 3 | 0 | 7 |
| vcard | 7 | 7 | 4 | 0 | 7 |
| **total** | **341** | **313** | **111** (+10 rows whose published name now resolves) | **28** | **313** |

* **111 claims were renamed** — each now names a test that exists and exercises that method. 46 of
  them were shared proofs split into per-method proofs. **10 more rows kept the name the matrix had
  published all along and now resolve to a test written under that very name** (clipboard's five —
  `clear_content_relinquishes_the_selection`, `clear_history_empties_the_ledger_only`,
  `delete_item_removes_from_the_ledger`, `size_limits_are_the_backends_own_policy`,
  `set_size_limits_changes_the_backend_policy` — and content's three —
  `a_batch_publish_answers_with_every_id`, `a_deleted_item_is_gone`,
  `an_unpublished_item_is_no_longer_published`). Those ten were claims of a test that **did not
  exist**; now it does.
* **28 claims were withdrawn** — the database's 19 methods the backend does not serve (no
  `setBatch`/`getBatch`/`exportNamespace`/`watchKey`/… arm exists, so the row is `unavailable` with
  the `no-backend` class, exactly like the 49 rows that already said so) and the media domain's 9
  MPRIS verbs, whose live proof would move the founder's own player. **Nothing was invented to
  cover the gap.**
* The remaining **202 claims already named a real per-method test** and were left alone — they are
  now protected by the gate, so they cannot rot either.
* The five domains that published one `core_works` for every method (audio 16, database 27,
  media 15, power 6, queue 7 — **71 rows**) now publish 71 distinct proofs.
* **The tree's test count: 578 → 728 `#[test]` functions in `src/`** (+150): 193 test functions
  added (146 per-method proofs, 36 new gate tests — two per domain, 11 hand-kept-list gates
  replaced by source scans) and 44 removed (the nine placeholders, the shared proofs that had
  been split, and the old hand-kept lists).

---

## 4. The gate: the law, made mechanical

Every domain carries the same gate in its own L1 test file
(`src/domains/<domain>/tests.rs`), and the domains that carried a hand-kept list of test names
(clock, files, logging, time, process, service, settings) now read the list **from the cell's own
source at compile time** instead of keeping it by hand.

```rust
/// The audio platform cell's own source, read at compile time.
const PLATFORM_SOURCE: &str = include_str!("../../platforms/linux/audio.rs");

/// Every published proof name resolves to a real test function in the
/// platform cell. A name that resolves to nothing is a FAILURE.
#[test]
fn every_claimed_proof_resolves_to_a_real_test() {
    for (method, test) in claimed_l3_tests() {
        assert!(the_platform_defines(test), "{method} claims {test}, which is not a test function …");
    }
}

/// One proof names ONE method. A name published for many methods is the
/// placeholder pattern: the matrix would be publishing one test as if it
/// proved a whole surface.
#[test]
fn one_proof_never_stands_for_many_methods() {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (_method, test) in claimed_l3_tests() { *counts.entry(test).or_insert(0) += 1; }
    for (test, count) in counts {
        assert_eq!(count, 1, "{test} is published as the proof for {count} methods: a proof names ONE method");
    }
}
```

What it catches:

1. **a name that resolves to nothing** — a test renamed or deleted in the cell breaks the matrix
   the moment it is published. The check is against the real `fn` definitions in the source
   (`include_str!` at compile time), not against a hand-kept list that can drift.
2. **one name standing for many methods** — the `core_works` pattern. 71 rows were published that
   way; the gate now fails on the first one that comes back.
3. **dead evidence** — in the five domains whose cell is a single file
   (clock, files, logging, time, process), every test in the cell must either be claimed by the
   matrix or **say it is a unit test**, in its own doc comment (`/// UNIT: …`). A test that proves
   nothing can no longer sit in a cell unclaimed, and a placeholder cannot hide there either.
   (The service cell holds three supervisor back-ends' tests, and the settings cell holds tests
   for verbs the matrix does not publish, so those two carry the forward check only.)
4. the domain whose platform cell is a **directory** (service: `tests.rs`, `openrc.rs`,
   `systemd.rs`) reads all three of its sources; the settings domain reads the cell **and** its
   own L1 file, because the settings cell is another agent's file and the domain's L1 file
   carries the five per-method proofs for `delete`, `clear`, `bulkSet`, `watchDomain` and
   `SettingsSubscription.cancel` in the meantime.

**The settings cell is another agent's file** (`src/platforms/linux/settings.rs`), so its five
per-method proofs live in `src/domains/settings/tests.rs` for now, and the settings gate reads
both sources. When that cell is handed back, the five proofs move into it and the gate follows.

---

## 5. The commands that produced every claim in this file

```
# the per-domain lib test binary, one domain at a time
cargo test -j6 --lib --features domain-audio
cargo test -j6 --lib --features domain-database
cargo test -j6 --lib --features domain-media
cargo test -j6 --lib --features domain-power
cargo test -j6 --lib --features domain-queue
cargo test -j6 --lib --features domain-clipboard
cargo test -j6 --lib --features domain-content
cargo test -j6 --lib --features domain-appindex
cargo test -j6 --lib --features domain-clock
cargo test -j6 --lib --features domain-files
cargo test -j6 --lib --features domain-logging
cargo test -j6 --lib --features domain-manifest
cargo test -j6 --lib --features domain-network
cargo test -j6 --lib --features domain-process
cargo test -j6 --lib --features domain-service
cargo test -j6 --lib --features domain-settings
cargo test -j6 --lib --features domain-time
cargo test -j6 --lib --features domain-vcard

# the integration binaries, one at a time, with the feature set
FEATURES=domain-settings,domain-files,domain-process,domain-logging,domain-clock,domain-time,rpc,std
cargo test -j6 --test smoke        --features $FEATURES
cargo test -j6 --test codegen      --features $FEATURES
cargo test -j6 --test serve_rpc    --features $FEATURES
cargo test -j6 --test serve_clock  --features $FEATURES
cargo test -j6 --test serve_files  --features $FEATURES
cargo test -j6 --test serve_logging --features $FEATURES
cargo test -j6 --test serve_process --features $FEATURES
cargo test -j6 --test serve_settings --features $FEATURES
cargo test -j6 --test serve_time   --features $FEATURES
```

(the `--features` list above is the one the run used; each binary was built and run on its own)

**Results:** every domain's lib binary green (655–702 tests each, 6–8 ignored), every integration
binary green (5, 23, 28, 7, 8, 6, 8, 9, 7 tests).

Two flakes were found and **one was fixed**:

* `platforms::linux::process::tests::stats_count_the_kernels_own_enumeration` compared the host's
  used memory with a **second** reading of `/proc/meminfo` taken a moment later — a live number
  that moves, so the published proof failed whenever the host allocated in between. **Fixed**: the
  two readings are now compared as what they are, the same live number sampled twice, within 1 %.
* `platforms::linux::process::tests::the_environment_is_the_hosts_own_proc_read` reads a child's
  `/proc/<pid>/environ` in the window where the child may already be gone. **Left**: it is the
  process domain's own test of the host's `/proc` and the read is the point of it.
* `transport::tests::bootstrap::symmetric_identity_checks_on_both_sides` and
  `transport::tests::shmem::attach_is_refused_with_a_typed_reason` are pre-existing flakes in
  `src/transport/` — files this work did not touch.
