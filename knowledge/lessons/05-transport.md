# 05 — transport lessons (Wave 2, Agent E)

Appended by the owner of `knowledge/lessons/05-transport.md`; the brief allows this file.

* **Declare, don't fake, and let a test prove the declaration.** Every honest-degradation claim
  in this layer (Tcp's `InBandToken`, Shmem's `BootstrapOnly`, the non-Linux `None`) exists as a
  declared value *and* an assertion in the same test file. The declaration is the API; the test
  is what stops it from drifting into a lie.
* **Shared memory needs its publication protocol spelled out.** Writing a payload into a slot
  through `&self` is only sound with: disjoint slots, one writer (the odd/even sequence), an
  `Acquire`/`Release` pair, and 8-byte-aligned atomics in the region. `std` has no portable API
  for this, so the invariant is the documentation — write it next to the `unsafe`, not in a
  design doc nobody reads.
* **Constants are load-bearing data.** One wrong seal bit (0x0010 for `F_SEAL_SEAL`) silently
  blocked the exact peer transfer the layer exists for, and nothing but the two-mappings test
  could see it. Compile-time asserts on struct layouts and a probe that reads back what the
  kernel enforced are the cheap fix.
* **A blocking mock makes protocols untestable.** `recv -> Empty` + a bounded wait for a
  mandatory verdict turned three racy tests into deterministic ones without weakening the
  protocol.
* **Frame length prefixes: the off-by-four that looks like a deadlock.** Counting the length
  bytes themselves in the length is a classic, and the symptom (a receiver waiting for bytes
  that were never sent) looks exactly like a hang. Write the reader's arithmetic *next to* the
  writer's, and test them as a pair.
* **`dyn`-seam design is easier when the capability is data.** Carrying `Box<dyn Any + Send>`
  for live handles and bytes for descriptors kept this layer independent of a concurrently
  built `src/core`, at the cost of one downcast at the receiving peer. The dependency direction
  (transport never imports core) is what let both waves land in parallel.
