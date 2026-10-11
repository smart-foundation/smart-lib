# BRIEF-02 — Host capability research (Wave 1, Agent B)

## Mission
Produce the definitive host-capability map for the four pilot domains (`files`, `network`, `audio`, `system`) on **Linux, macOS, Windows**, plus the transport primitives, so the backend implementers (Waves 3–4) never have to guess what a host offers, what it costs, or how to stay honest. You write NO code. You write the document the whole build trusts.

## Your exclusive output
- `/home/lufuno/smart/smart-lib-ng/knowledge/02-HOST-CAPABILITIES.md`
- (optional append) `/home/lufuno/smart/smart-lib-ng/knowledge/lessons/02-host-research.md`
Create no other files. Edit nothing else. No git commits.

## Inputs to read (read-only)
- Canonical contract: `/home/lufuno/smart/smart-protocol-official/schema/{files,network,audio,system,capability,types}.capnp` (+ `docs/` if useful) — the contract is the requirements source.
- Old implementation (frozen reference, for what was learned the hard way): `/home/lufuno/smart/smart-protocol/crates/smart-lib/src/{files,network,audio,system}.rs` and `src/platforms/`.
- Your own knowledge of host APIs (verify with `--help`/man pages/local docs where possible; this box has Linux; do not fetch the web unless a fact is unverifiable otherwise — mark anything unverified as unverified).

## The checklist

- [ ] **Per (domain × os) capability table**: for each contract method in the four domains: the host mechanism(s) that can serve it (e.g. files→`statx`/`copy_file_range`/`inotify` on Linux; FSEvents on macOS; ReadDirectoryChangesW on Windows), the zero-copy story (where `T.DataHandle` maps to `O_DIRECT`/`sendfile`/`mmap`/`TransmitFile`), and the honest `Unavailable` reason when nothing serves it.
- [ ] **Fragmentation map** (the reason smart-lib exists): audio (ALSA/PulseAudio/PipeWire; CoreAudio; WASAPI), network (NetworkManager/iwd/wpa_supplicant; `networksetup`/CoreWLAN; WLAN API), storage/files (fstab/udisks2; DiskArbitration; VDS), services/processes (systemd/openrc; launchd; SCM), power (UPower; IOKit; power scheme API) — which layer a backend should target per OS, and what that choice costs.
- [ ] **Authority mapping (critical)**: for each OS, how native credentials become smart-protocol capability handles at acquisition: Linux (uid/gid, capabilities(7), ambient caps, seccomp), macOS (sandbox profiles, entitlements, TCC prompts), Windows (tokens, privileges, AppContainer). Specify the boundary adapter design: what the backend asks the OS, what it hands the caller, and what it must refuse.
- [ ] **Transport primitives**: Linux (`memfd_create`, `shm_open`, `AF_UNIX` + SCM_RIGHTS fd passing, `io_uring` availability), macOS (`shm_open`, unix sockets, no SCM_RIGHTS equivalent — document the capability-transfer substitute), Windows (named sections, `AF_UNIX` in Win10+, handle duplication via `DuplicateHandle`). For each transport (InProcess/Shmem/UnixSocket/Tcp): capability-transfer semantics, framing, limits, and the honest gaps.
- [ ] **Cost pyramid**: per domain per OS — the "thin wrapper" methods (≤1 day), the "partial" (structure exists), the "subsystem" (real integration work, e.g. spatial audio, VPN), the "honest-permanent" (cannot exist on this OS). This sizing drives wave scope.
- [ ] **Pitfalls from the old implementation**: read the old modules + `/home/lufuno/smart/smart-protocol/audit*/` audit notes and `complaints_and_protests/` if present; list what the old code got wrong or fake (e.g. invented values, dbus coupling) so the new backend never repeats it.

## Report
`02-HOST-CAPABILITIES.md` with: the per-(domain×OS) tables, the fragmentation map, the authority-mapping design, the transport primitive table, the cost pyramid, and the pitfalls list. Every claim marked VERIFIED (checked on this machine/locally) or UNVERIFIED (from knowledge — flagged as such).

## Return to orchestrator (final message, ≤25 lines)
The four cost-pyramid totals (thin/partial/subsystem/permanent per OS), the authority-mapping verdict per OS, the transport gaps, and your top-3 warnings for the backend implementers.