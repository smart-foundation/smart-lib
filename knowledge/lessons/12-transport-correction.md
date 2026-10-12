# 12 lessons — the network transport correction (law #9)

Learned finishing the P6 correction in `smart-lib-ng` (report 12).

1. **A maintained crate can still encode a wrong assumption — about the wire, not about
   itself.** `netlink-packet-core`'s `NetlinkMessage::deserialize` parses exactly ONE message
   per buffer and silently ignores whatever follows it. Netlink packs several messages into one
   datagram (a dump's replies). Result: this host's `enp0s31f6` vanished from the link table
   while `lo` and `wlp0s20f3` survived — a partial answer that looks complete. The lesson is not
   "avoid the crate" (it does the marshalling correctly) but "know the framing rules the crate
   leaves to you". A test with a two-message datagram would have caught it; a one-message test
   never will.

2. **When you swap transports, grep for the assumption that leaks across the seam.** The kernel
   answers `RTM_GETLINK`/`GETADDR`/`GETROUTE` with `RTM_NEWLINK`/`NEWADDR`/`NEWROUTE` — not with
   the request's kind. Folding dump replies by request kind produced an empty link table on a
   three-interface host. The unit test that "covered" it serialized only the request kind.

3. **A read-only live probe is the cheapest test you own.** Two of the three defects above were
   invisible to every hermetic test and obvious in one `#[ignore]`d probe run. The netlink probe
   in `netlink.rs` is now the first thing to run after touching the read path — it is read-only
   (law #8) and it answers in milliseconds.

4. **`ARPHRD_*` says `ether` for a normal wlan interface.** The wireless-ness lives in the phy,
   not the link type, so a kernel-only link-kind derivation reports `ethernet` for the wifi — the
   exact interface class the incident was about. The kernel's own marker is
   `/sys/class/net/<name>/wireless` (a sysfs read: still kernel-first, no daemon, no guess). Any
   "kind" derived from ARPHRD alone is wrong by default.

5. **Count a fallback at the SEAM, not per wire call.** `RealNmClient::policy()` performs six
   D-Bus round trips; counting those would make "zero NmClient calls on a warm store"
   unassertable and would make the acceptance test a wire-level measurement. One seam call =
   one counted fallback, and the count lives on the backend (`nm_fallback_calls()`), not in a
   log.

6. **Invalidate caches on EVENTS, not on a clock.** The NM policy snapshot is cached against the
   store's topology revision (a link/address arriving or going away, or a link's administrative
   flag flipping). A TTL would have made the acceptance assertions time-dependent and flaky;
   event-driven invalidation is deterministic, cheaper in the common case, and re-sweeps exactly
   when something actually changed. `refresh_policy()` covers the "policy moved and the topology
   did not" case.

7. **`IFLA_CARRIER` means different things on a wire and on a radio.** On ethernet it is "the
   cable is plugged"; on wifi it is "associated". Reading it the same way turns a healthy,
   unassociated wifi into `carrier-down`, and a `stalled()` marker (admin up, operstate DOWN, no
   carrier) is indistinguishable from "not associated yet" — so NM's device view must be allowed
   to REFUTE the kernel marker even when the kernel is primary. Kernel-first does not mean
   kernel-only.

8. **A `#[cfg(test)]` mock with no constructor is a TODO in disguise.** The dead-code warning
   ("struct `MockNetlink` is never constructed") was the only build signal that the whole
   integration was missing. Treat "written but never constructed" as unfinished work, not as
   scaffolding.
