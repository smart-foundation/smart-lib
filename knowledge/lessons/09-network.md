# 09-lessons — network domain (appended to `knowledge/lessons/`)

- **The D-Bus string length was the deepest trap.** The `u32` EXCLUDES the trailing NUL (the NUL is a
  separate byte). Getting it backwards marshals a *plausible* message the daemon silently drops and
  closes on. The only reliable way to trust a hand-rolled wire codec is to diff your bytes against a
  real client (`dbus-send` through a logging proxy) and to read the reply stream — my three deepest
  bugs (string length, missing `Hello`, unpadded last header field) were invisible to reasoning and
  obvious the moment I compared bytes.
- **A D-Bus client is a state machine, not a request/response pipe.** After `Auth`+`BEGIN` the
  reference daemon requires `Hello` (else it closes on the first routed call) and then emits a
  `NameAcquired` signal that must be skipped before the first real reply. Any hermetic mock hides
  all of this; only a read-only live probe (law #8-legal) surfaced it.
- **Mockability is a law #8 hygiene tool, not just a testing nicety.** Splitting the NM surface into
  a seam (`NmClient`) with the mutators on the wrong side of it is what let me prove
  `Enable(true)`/`Enable(false)` map correctly and inject all 20 error codes — without ever having a
  code path that could turn the founder's wifi off.
- **DEGRADED is the honest status for a correct-but-ungrantable write.** A mutating method whose map
  is implemented and mock-verified, but whose live authority is polkit-gated and polkitd is absent,
  is DEGRADED (naming the gated write in `detail`), not REAL. Claiming REAL would need a live write
  test — which is exactly the founder's, not mine.
- **The four-state function's branch order is the whole contract.** `!NetworkingEnabled` before
  everything else is why a switched-off host reads `networking-disabled` even though a device also
  looks unmanaged. One reordering collapses two failure modes into one and the domain is pointless.
