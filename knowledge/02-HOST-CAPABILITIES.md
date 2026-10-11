# 02 — HOST CAPABILITIES: the map the backend implementers build on

**Wave 1, Agent B.** Exclusive output: this file (+ `lessons/02-host-research.md`). No code was written.
Requirements source: `/home/lufuno/smart/smart-protocol-official/schema/{files,network,audio,system,capability,types}.capnp`. Frozen reference: `/home/lufuno/smart/smart-protocol/crates/smart-lib/src/` (read-only, never edited).

## Claim marking (mandatory per brief)

| Mark | Meaning |
|---|---|
| **[V]** | VERIFIED — measured on this machine, command/probe cited |
| **[U]** | UNVERIFIED — from knowledge; this box is Linux-only, no macOS/Windows host available (no cross toolchain either: `rustup target list --installed` shows only linux + bare-metal targets **[V]**) |

This box **[V]**: Void Linux, kernel `6.18.40_1` x86_64, glibc 2.41, `capnp` **1.4.0**, rustc 1.99.0-nightly. PipeWire **1.6.9** + WirePlumber + pulse-compat running; NetworkManager running (wpa_supplicant, no iwd); **no systemd** (runit `runsv`/`sv`); no `polkitd` (only `pkexec` present); ext4 on `/`; tmpfs `/tmp` and `/dev/shm` (7.7 G); user `lufuno` is in group `audio`; Yama `ptrace_scope=1`; no SELinux (`SO_PEERSEC` → ENOPROTOOPT); cgroup v2.

## 0. Host primitives verified on this box (the substrate everything below rests on)

| Primitive | Result |
|---|---|
| `memfd_create` (syscall 319) | fd returned **[V]**; with `MFD_ALLOW_SEALING` `fcntl(F_ADD_SEALS, F_SEAL_WRITE)` returned 0 **[V]** |
| `shm_open` + `ftruncate` + `mmap` | write/read OK **[V]**; 64 MB map OK **[V]** |
| `AF_UNIX` `SCM_RIGHTS` fd passing | verified: kernel hands back a *new* fd number; **253 fds accepted in one message** (kernel `SCM_MAX_FD`) **[V]** |
| `AF_UNIX` abstract namespace, `SOCK_DGRAM` + ancillary | working **[V]**; filesystem path >105 bytes → `EINVAL`/too-long (the 108-byte `sun_path` limit, verified `AF_UNIX path too long` at 106 chars) **[V]** |
| `SO_PEERCRED` on `AF_UNIX` | returns peer `(pid, uid, gid)` **[V]** |
| `io_uring_setup` | 4-entry setup returned fd **[V]** (usable; no `io_uring_disabled` sysctl observed) |
| `statx` (332) | `ret=0` on `/tmp` **[V]** — btime/statx attributes available (mask `STATX_BTIME` 0x800) |
| `copy_file_range` (326) | 4096 bytes copied same-fs **[V]** |
| `sendfile64` (40), `splice` (275) | 4096 bytes each **[V]** |
| `openat` with `__O_TMPFILE|O_DIRECTORY` | **EINVAL on this box** **[V]** — do not rely on O_TMPFILE; use memfd |
| `openat2` (437) | dispatched, works with a real `open_how` **[V]** |
| `pidfd_open` + `pidfd_getfd` (438) | both returned fds for a child process **[V]** (Yama=1 allows descendants only) |
| `landlock_create_ruleset` (444) | ABI version **7** returned **[V]** — unprivileged filesystem sandboxing exists |
| `fanotify_init` (300) | 7 flag combos probed: only bare `FAN_REPORT_FID` succeeded; `FAN_CLASS_NOTIF` returned **EPERM**, `FAN_CLASS_CONTENT`/`PRE_CONTENT` EPERM **[V]** — treat as unavailable to an unprivileged backend |
| `inotify` | works; `max_user_watches=126713`, `max_user_instances=128`, `max_queued_events=16384` **[V]** |
| `xattr user.*` | set/get OK **[V]** |
| `ALSA` direct | `aplay -l` lists card 0 PCH; `/dev/snd/*` is `root:audio` 0660 **[V]** |
| `PipeWire` | `wpctl status`: server 1.6.9, session bus socket per-user; `pactl info` → protocol 35, `/run/user/1000/pulse/native` **[V]** |
| `nl80211` | `iw dev` shows phy#0, driver `iwlwifi`; `ethtool -i` reports driver + `supports-statistics: yes` **[V]** |
| `ping` | `/proc/sys/net/ipv4/ping_group_range` = `1 0` (all groups) **[V]** — unprivileged ICMP possible |
| `SO_PEERSEC` | ENOPROTOOPT **[V]** (no SELinux label to read) |
| `cgroup` | v2 unified **[V]** |

## 1. Per (domain × OS) capability tables

Method ordinals refer to the canonical interfaces. Tier vocabulary is §5's. "Host mechanism" is the layer a backend targets; the zero-copy story is where `T.DataHandle`'s `buffer` arm (`types.capnp:502`) and `BufferId` actually land on that OS.

### 1.1 Files (74 methods, `files.capnp` @0–@73)

| Group | Methods | Linux mechanism | macOS mechanism | Windows mechanism |
|---|---|---|---|---|
| A. POSIX core | openFile…changeDirectory, walkTree, getFileType (22) | thin: `openat`/`statx`/`renameat`/`linkat` **[V]** | thin: BSD syscalls **[U]** | thin: `CreateFileW`/`NtCreateFile`, `FindFirstFileExW` **[U]** |
| B. Special paths | getSpecialPath @13 | **partial**: XDG vars + `$HOME`; `templates`/`public` often absent → must answer honestly | partial: `NSSearchPathForDirectoriesInDomains`; no "templates" **[U]** | partial: `SHGetKnownFolderPath`; no "templates" **[U]** |
| C. Watch | watchDirectory @14 | thin: inotify (per-dir fd, cookie-paired rename) **[V]**; fanotify unusable **[V]** | partial: FSEvents (rename oldPath needs pairing) **[U]** | partial: `ReadDirectoryChangesW` (same pairing problem) **[U]** |
| D. Name search | searchFiles @15 | partial: walk + glob; **no index daemon on this box** (`plocate`/`tracker3` absent) **[V]** | partial: Spotlight `mdfind` **[U]** | partial: Search indexer query **[U]** |
| E. Disk usage | getDiskUsage @24 | thin: `statvfs` **[V]** | thin **[U]** | thin: `GetDiskFreeSpaceExW` **[U]** |
| F. Metadata | getFileMetadata @25 | partial: `statx` + xattrs **[V]**; `owner` is a *label* (schema comment) — never authority | partial: mdls/xattr **[U]** | partial: ADS + property store **[U]** |
| G. Compare | compareFiles @26 | thin: userspace diff through one memfd | thin | thin |
| H. Locks | lock/unlock/getLocks @27–29 | thin: `fcntl` OFD locks + `flock` **[V] (kernel-side; flocks visible in /proc/locks)** | thin **[U]** | thin: `LockFileEx` **[U]** |
| I. Versioning | @30–33 | **subsystem**: no fs versioning on ext4 **[V]** (btrfs/zfs unmounted) — build a content store or refuse | subsystem: APFS snapshots exist but no public per-file API **[U]** | subsystem: VSS is volume-level **[U]** |
| J. Branches | @60–62 | subsystem (userspace VCS semantics) | subsystem | subsystem |
| K. Content-addressed | @34–36 | subsystem (CAS store over any fs) | subsystem | subsystem |
| L. Ephemeral | createEphemeral @37 | **thin**: `memfd_create` **[V]** (O_TMPFILE EINVAL **[V]** — do not use) | partial (O_TMPFILE exists on APFS) **[U]** | thin: `FileAttributeTemporary` + delete-on-close **[U]** |
| M. Encryption | @38–40 | **partial**: fscrypt exists for ext4 **[U]** but key material is out-of-band → `T.KeyId` maps to a keyring id | **permanent**: no per-file encryption API (FileVault is volume-level) | partial: EFS per-file **[U]** |
| N. Compression | @41–42 | **permanent** on ext4 **[V]** (only ext4 mounted here) | permanent | partial: NTFS `FSCTL_SET_COMPRESSION` **[U]** |
| O. Dedup stats | getDeduplicationStats @43 | **permanent** (no dedup on ext4 **[V]**) | permanent | permanent (server-only feature) |
| P. Batch | @44–46 | thin | thin | thin |
| Q. Dir stats | @47–49 | thin (walk + statx, paginated) | thin | thin |
| R. Activity | getFileActivity @50 | subsystem: auditd/LSM history | subsystem | partial: USN change journal **[U]** |
| S. Hash | @52–53 | thin | thin | thin |
| T. Lifecycle | @54–56 | subsystem (userspace policy engine; `systemd-tmpfiles` is systemd-only and this box has none **[V]**) | subsystem | subsystem (FSRM, server) |
| U. Audit sub | subscribeFileAudit @57 | subsystem | subsystem | subsystem (SACL + event log) |
| V. Analyze | analyzeFileContent @58 | subsystem (external OCR/AI) | subsystem | subsystem |
| W. Content search | searchFileContent @59 | partial (grep-style) | partial (Spotlight) | partial (indexer) |
| X. Trash | @63–65 | subsystem: freedesktop trash spec / `gio trash` present **[V]** | partial: `NSFileManager` trash + Put Back **[U]** | partial: `IFileOperation` Recycle Bin **[U]** |
| Y. Recent | @66–67 | subsystem (GTK-recent/tracker; absent here **[V]**) | partial: `LSSharedFileList` **[U]** | partial: Recent folder + `SHAddToRecentDocs` **[U]** |
| Z. Default app | getDefaultApp @68 | partial: xdg-mime + `.desktop` parsing | partial: `LSCopyDefaultRoleHandlerForContentType` **[U]** | partial: `AssocQueryString` **[U]** |
| AA. Health/quota | @69–71 | subsystem (quota tools; `smartctl` absent **[V]**) | subsystem | partial: NTFS quotas **[U]** |
| AB. Snapshot | createSnapshot @72 | subsystem (LVM/btrfs; ext4 needs a userspace copy) | subsystem | subsystem |
| AC. Delegated | acquireFile @73 | thin (pure capability binding, no host call) | thin | thin |

**Zero-copy story (files).** `readFile`/`writeFile` return/consume `T.DataHandle`. Linux: the `buffer` arm is a `BufferId` in a memfd/shm arena the endpoint registered; `read` can be served `mmap`'d (page faults, no copy), `copyFile` via `copy_file_range` **[V]**, `writeFile` to a socket transport via `sendfile`/`splice` **[V]**, `O_DIRECT` verified openable but **silently ignored on tmpfs** **[V]** — only meaningful on ext4/block-backed targets; `createEphemeral` → memfd **[V]**. macOS: `clonefile`/`copyfile` with clone, `mmap` **[U]**. Windows: memory-mapped sections, `TransmitFile`, `ReadFileScatter` **[U]**. `Deduplicate`/`getByHash` are the only paths where a copy is semantically required.

### 1.2 Network (72 methods, `network.capnp` @0–@71)

| Group | Methods | Linux | macOS | Windows |
|---|---|---|---|---|
| N1 Enumerate/status | getInterfaces @0, getStatus @1 | **thin**: rtnetlink directly — no daemon needed **[V]** (`ip` is just a netlink client) | thin: `getifaddrs` + SystemConfiguration **[U]** | thin: `GetAdaptersAddresses`/`GetIfEntry2` **[U]** |
| N2 Status subscription | @2–3 | thin: `RTMGRP_LINK`/`RTMGRP_IPV4_IFADDR` | partial: SCDynamicStore **[U]** | partial: `NotifyIpInterfaceChange` **[U]** |
| N3 Connect/disconnect | @4–5 | **subsystem**: netlink cannot do WPA auth — must go through `wpa_supplicant` or NM (both present **[V]**) | subsystem: CoreWLAN **[U]** | subsystem: WLAN API **[U]** |
| N4 WiFi | scan @6, connect @7, disconnect @8, autoConnect @9, macRandom @10 | scan/disconnect/auto/macRandom **partial** (nl80211 visible **[V]**; privileges/NM mediate); connect subsystem | partial/subsystem **[U]** | partial/subsystem **[U]** |
| N5 Cellular | @11–14 | thin (NM settings; ModemManager absent **[V]**) | partial **[U]** | partial **[U]** |
| N6 VPN | @15–20 (6) | subsystem (NM VPN plugin, strongswan) | subsystem: NetworkExtension **[U]** | subsystem **[U]** |
| N7 Hotspot | @21–25 (5) | partial (NM hotspot; `hostapd` absent **[V]**) | partial **[U]** | partial **[U]** |
| N8 DHCP | @26–30 (5) | partial: reading the assignment via netlink is thin; *requesting* needs `dhcpcd`/NM (`dhcpcd` present **[V]**) | subsystem **[U]** | subsystem **[U]** |
| N9 DNS | @31–32 | get thin (netlink/resolv.conf); set **subsystem** (needs polkit/root — **no polkitd here** **[V]**) | same split | same split |
| N10 QoS | @33–34 | subsystem: `tc`/netem (CAP_NET_ADMIN; `nft` absent, iptables present **[V]**) | subsystem | subsystem (WFP) |
| N11 Counters | @35–38 (4) | get/subscribe/pull **thin** (`ethtool` statistics on the live NIC **[V]**); reset partial | thin/partial | thin/partial |
| N12 Captive portal | @39–41 | partial: NM connectivity check (`nmcli` reports `full` **[V]**) or userland probe | partial | partial: NCSI |
| N13 Up/down | @42–43 | partial (netlink needs CAP_NET_ADMIN; NM+polkit path) | subsystem | subsystem |
| N14 Topology | getTopology @44 | partial (ARP/LLDP synthesis) | partial | partial |
| N15 Power profile | @45–46 | partial (`iw power save` via nl80211) | partial | partial |
| N16 Handoff | @47 | subsystem | subsystem | subsystem |
| N17 Slicing | @48–50 (3) | subsystem (netns/VLAN are real primitives) | subsystem | subsystem |
| N18 Telemetry | @51–53 (3) | thin (sampled netlink/ethtool counters) | partial | partial |
| N19 Diagnostics | ping @54, traceroute @55 | partial: ICMP unprivileged (`ping_group_range=1 0` **[V]**); traceroute needs raw/UDP tricks | partial | partial |
| N20 Firewall | @56–57 | subsystem (nftables/iptables **[V]** present) | subsystem (pf) | subsystem (WFP) |
| N21 Time sync | getTimeSync @58 | thin (query NTP; chrony status is a file/socket) | thin | thin |
| N22 Satellite | getSatelliteConfig @59 | partial (mostly reported) | partial | partial |
| N23 Hotspot extras | @60–62 (3) | partial | partial | partial |
| N24 DTN | @63–64 | subsystem (userspace BP agent) | subsystem | subsystem |
| N25 Cellular cfg | @65–66 | get partial, set subsystem | same | same |
| N26 IPv6 cfg | @67–68 | get thin (netlink), set partial (netlink/sysctl) | same | get thin, set partial (Netsh) |
| N27 NAN | @69–70 | subsystem (no public NAN API; internal to wpa_supplicant) | subsystem | subsystem |
| N28 NIC driver | nicProvider @71 | subsystem (netlink/ethtool/DPDK backend) | subsystem | subsystem |

**Zero-copy story (network).** `NicFrameDesc` carries `bufferSlot` + `T.BufferId` (`network.capnp:908`) — the frame path is the protocol's highest-volume path. Linux: an AF_PACKET mmap ring or `io_uring` registered buffers **[V]** map naturally onto `BufferId`; macOS: BPF device mmap **[U]**; Windows: NDIS/Npcap buffers **[U]**. On any transport without a shared buffer namespace (Tcp), the `BufferId` arm cannot be honoured — that is an **honest gap**, not a fallback to inline.

### 1.3 Audio (100 methods, `audio.capnp` @0–@99)

Layer choice is the whole game — see §2. Numbers below assume the **PipeWire session service** backend on Linux, **CoreAudio** on macOS, **WASAPI + session manager** on Windows.

| Group | Methods | Linux | macOS | Windows |
|---|---|---|---|---|
| A1 State/devices/streams | @0–2 (3) | thin: `wpctl`/native PipeWire; ALSA as fallback **[V]** | thin **[U]** | thin: `IMMDeviceEnumerator` + `IAudioSessionManager2` **[U]** |
| A2 Volume/mute | @3–6 (4) | thin (PipeWire/ALSA mixer) **[V]** | thin **[U]** | thin: `ISimpleAudioVolume` **[U]** |
| A3 Enable/disable | @7–8 | partial: is a profile switch, not a power rail | partial | partial |
| A4 Subscriptions | @9–17, @30–32 (12) | thin (PipeWire change events; ALSA via udev/inotify fallback) | thin: property listeners | thin: session notifications |
| A5 Focus | @18–22 (5) | **subsystem**: no OS focus arbiter — PipeWire/WirePlumber has *no* per-app focus theft API; a policy daemon must be written | partial: session categories do ducking **[U]** | partial: session ducking **[U]** |
| A6 Spatial | @23–25 (3) | subsystem (PipeWire core has no HRTF positional mixer; custom module) | partial | partial: Spatial Sound platform **[U]** |
| A7 Recording consent | @26–27 | subsystem: only via desktop portal (`xdg-desktop-portal` present **[V]**); **no OS-native mic consent on Linux** | partial: TCC | partial: per-app mic setting |
| A8 Codecs | @28–29 | thin | thin | thin |
| A9 Routing/duck | @33–34 | partial | partial | partial |
| A10 Effects | @35–39 (5) | partial (filter-chain/LADSPA; PipeWire modules) | partial: AudioUnits | partial: APO |
| A11 Presets | @40–43 (4) | thin (backend-local store) | thin | thin |
| A12 Global duck | @44–45 | subsystem | partial | partial |
| A13 Metering | @46–47 | thin (peak monitor) | thin | thin: peak meter |
| A14 Sessions | @48–53 (6) | subsystem (cross-app session registry must be built) | subsystem | subsystem (session groups exist, joining does not) |
| A15 Transport info | @54, @87–89 (4) | thin — backend-declared open token per the schema header | thin | thin |
| A16 Layouts | listChannelLayouts @55 | thin | thin | thin |
| A17 Per-role duck | @56–57 | subsystem | partial | partial |
| A18 HRTF | @58–59 | subsystem | partial | partial |
| A19 Sound events | @60–66 (7) | subsystem (freedesktop theme + libcanberra; no theme dirs here **[V]**) | thin: `NSSound` **[U]** | thin: `PlaySound` + registry schemes |
| A20 Analysis | @67–71 (5) | subsystem (DSP over captured stream) | subsystem | subsystem |
| A21 Mic privacy | @72–77 (6) | subsystem (portal) | partial: TCC | partial: Settings privacy |
| A22 Safety limits | @78–82 (5) | **permanent**: no loudness-limiter OS API exists | partial (Reduce Loud Sounds) | partial (APO loudness EQ) |
| A23 Routing rules | @83–86 (4) | subsystem | partial | partial |
| A24 Device health | @90–93 (4) | partial | partial | partial |
| A25 Zones | @94–98 (5) | subsystem | partial | partial |
| A26 Driver | openDriver @99 | subsystem (the driver backend itself) | subsystem | subsystem |

**Zero-copy story (audio).** `AudioDriverStream.submit` takes `T.DataHandle` — the hottest path in the file (48 kHz stereo s16 = 192 KB/s per stream, schema comment). Linux: the `buffer` arm is a PipeWire/ALSA ring buffer registered as a `BufferId` **[V]**; inline arm covers sub-1 KB frames. macOS: AudioUnit/HAL buffer exchange **[U]**. Windows: WASAPI render endpoint **[U]**. A backend that copies every submit has failed the product standard; a backend that *pretends* a BufferId exists on Tcp has failed honesty.

### 1.4 System (8 methods, `system.capnp` @0–@7)

| Method | Linux | macOS | Windows |
|---|---|---|---|
| handshake @0 | thin: session id from an internal counter; capabilities = the *measured* token set | thin | thin |
| getCapabilities @1 | thin: read from the caps registry — never a hardcoded bitfield (see pitfall P1) | thin | thin |
| getInfo @2 | thin: `uname` (**[V]** `uname -a`), `gethostname` (**[V]** exists as `think`), arch token | thin | thin |
| subscribeCapabilities @3 | thin (cheap recomputation, bounded pull) | thin | thin |
| getExtension @4 / listExtensions @5 | partial (plugin discovery: `.so` scan; extensions report `capabilityIds` honestly) | partial | partial |
| ping @6 | thin (monotonic timestamp, ungated) | thin | thin |
| shutdown @7 | thin (service-level; OS power operations belong to the power domain) | thin | thin |

**Capabilities struct discipline** (`system.capnp:90`): the 47-bool bitmap was replaced by an open token set with `partial` + `note`. A backend that cannot *enumerate* its own set MUST set `partial=true` with the reason — an unrecognised token is ignored by consumers, never rejected.

## 2. Fragmentation map (why smart-lib exists)

Per OS, which layer to target and what that choice costs.

| Layer | Linux | macOS | Windows |
|---|---|---|---|
| **Audio** | Three stacks coexist: **ALSA** (kernel `/dev/snd`, group-gated **[V]**), **PulseAudio**, **PipeWire** — and on this box PipeWire is the server with pulse protocol 35 compat **[V]**; the old impl shelled `pactl` and assumed all three. **Target: PipeWire session API** (focus/route/metering live there), with ALSA as the `AudioProvider` backend. Cost: a second, ALSA-only backend for no-sound-server machines; WatchPlumber policy is the only focus arbiter and it has no focus API → the focus subsystem daemon (A5) is unavoidable work, not optional. | Single first-party stack (CoreAudio/HAL) — **target it**, everything above (spatial, ducking) is either an AudioUnit or absent **[U]**. Cost: no fragmentation cost, but a *capability* cost — TCC and entitlements gate what the API may do. | WASAPI + desktop app runtime session manager. **[U]** Cost: WinRT vs classic desktop split — the same operation has two APIs. |
| **Network** | Split three ways: **netlink** (raw, CAP_NET_ADMIN-gated, always present), **wpa_supplicant** (present **[V]**), **NetworkManager** (running **[V]**), iwd (absent). **Target: netlink for read/passive ops; NM D-Bus for privileged/wifi connect ops.** Cost: a D-Bus dependency for half the surface, and NM's own polkit policy decides — note polkitd is **absent** on this box **[V]**, so privileged NM calls fail for unprivileged callers there; the backend must translate that to `permissionDenied`, not fabricate. | `networksetup`/CoreWLAN/SystemConfiguration **[U]**. Cost: sandbox can't change system config. | WLAN API + `netsh`/WMI **[U]**. Cost: admin elevation for most writes. |
| **Storage/files** | fstab/mount namespace (thin), udisks2 (`udisksctl` present **[V]**), ext4-only here **[V]**. **Target: syscalls for the file surface; udisks2 only if a mount/admin domain appears later.** Cost: none for the four pilot domains — mount is a different schema (`mount.capnp`). | DiskArbitration **[U]** | VDS **[U]** |
| **Services/processes** | systemd (absent **[V]**), OpenRC, **runit (`sv` present [V])**, sysvinit — the old code enumerated all four via `Command::new("sv"/"systemctl")` text parsing. **Target for scope: none of them** — service/process are not pilot domains; when they arrive, go through `/run` + a dedicated service trait. Cost: text-scraping supervision layers is a trap (see P5). | launchd **[U]** | SCM **[U]** |
| **Power** | UPower present **[V]** (CLI only; D-Bus API), no systemd-logind (runit). **Target: none** — power is not a pilot domain. Cost: battery data available only via upower CLI here. | IOKit **[U]** | power scheme API **[U]** |

## 3. Authority mapping design (the critical section)

**Law 5**: no permission strings; calls carry capability references; host credentials map into handles **at acquisition**. The acquisition points in the contract are exactly three: `Files.acquireFile @73 (path, authority)`, `Network.nicProvider @71 (authority)`, `Audio.openDriver @99 (authority)` — plus the host-injected root references (the `Audio`, `Files`, `Network`, `System`, `CapabilityService` roots themselves).

### 3.1 The boundary adapter (OS-independent shape)

```
acquire(authority: Capability, target: Text) -> Result<Handle>
  1. identify the PEER (who is calling), once per session:
        UnixSocket/Shmem transports: SO_PEERCRED → (pid, uid, gid)   [V: Linux]
        InProcess: same-process marker (no peer)
        Tcp: no peer identity exists → the backend may only mint
              attenuated, rate/period-boxed handles or refuse.
  2. capture the peer's host authority snapshot at session start:
        uid, gid, supplementary groups, capability sets, sandbox state
  3. on acquire(): match the PRESENTED Capability (id, target, operations,
     constraints, expiry, chain) against the target and the snapshot.
     - match  -> mint/return the bound handle; the handle IS the authority
     - cannot evaluate a constraint -> Error(code != permissionDenied),
       per capability.capnp's verify contract; never answer false-ish
  4. refuse (permissionDenied + reason) when:
        - target outside the presented capability's scope
        - the host mechanism itself gates the target (§3.2/3.3/3.4 checks fail)
```

Non-negotiables: the mapping is the **only** place OS identity enters the library; the handle the caller receives carries no permission name, no path, no mode; the backend must re-check revocation per call (or via a bounded cache whose staleness is declared); a `CapabilityHandle` (`types.capnp:705`) is an opaque u64 minted per acquisition, never derived from a uid.

### 3.2 Linux — uid/gid, capabilities(7), ambient caps, seccomp, Landlock

| Host fact the backend asks | How | Status |
|---|---|---|
| peer pid/uid/gid | `getsockopt(SO_PEERCRED)` on the transport socket | **[V]** returned `(pid, uid, gid)` for AF_UNIX |
| supplementary groups | `/proc/<pid>/status` `Groups:` line | **[V]** (`audio` = 12 present for this user) |
| permitted/effective/bounding/ambient caps | `/proc/<pid>/status` `CapPrm/CapEff/CapBnd/CapAmb` | **[V]** — this shell: `Eff=0`, `Bnd=0x1ffffffffff` (41 caps), `Amb=0` |
| is the peer sandboxed by seccomp? | `/proc/<pid>/status` `Seccomp` field (0/1/2) | **[V]** (`Seccomp: 0`, `Seccomp_filters: 0`) — the mode is visible, the **program is not**: you cannot guess what a filtered peer may not do |
| Landlock restrictions of a peer | **not introspectable** — Landlock rules are per-process private | knowledge, consistent with ABI 7 **[V]**; treat as unknowable: enforce on your own syscalls, trust nothing about the peer's |
| can I move an fd to/from the peer? | `pidfd_getfd(438)` + `task_for_pid`-like ptrace check | **[V]** worked for a child; Yama=1 **[V]** blocks non-descendants |

Concrete gates that must be enforced at acquisition:
- **audio authority** (`openDriver`): `/dev/snd/*` is `root:audio` 0660 **[V]** and the PipeWire/Pulse sockets live in the per-user runtime dir **[V]**. Check peer group membership (gid 12) or runtime-dir ownership before handing an ALSA-backed `AudioProvider`; otherwise the honest answer is `permissionDenied`, not an `AudioProvider` that fails on first `openStream`.
- **network authority** (`nicProvider`): privileged ops need `CAP_NET_ADMIN` in the peer's effective set **[V: mechanism]** (or an NM/polkit grant — polkitd absent here **[V]**, so expect denial and report it).
- **file authority** (`acquireFile`): the presented capability's `target`/`pathPrefix` must be honoured; a peer's Landlock/seccomp state is invisible, so a handle that would force the *backend* to open a path the peer itself cannot reach is still legitimate — the backend, not the peer, performs the syscall. Say so explicitly in the trait docs.
- **must refuse**: minting a handle the host didn't grant; passing a peer's fd through `pidfd_getfd` without the ptrace check; claiming a capability set the backend did not measure.

### 3.3 macOS — UNVERIFIED (no macOS host)

Sandbox profiles (Seatbelt), code-signature **entitlements** (`com.apple.security.device.audio-input`, etc.), and **TCC** (the prompt layer whose consent database `TCC.db` is root/tccd-only readable). Adapter shape: peer identity from the mach audit token (`audit_token_to_au32`), authority snapshot from entitlement lookup + TCC category status, refuse on absent entitlement. Marked **[U]** in full; the shape above is the design to verify on first macOS hardware.

### 3.4 Windows — UNVERIFIED (no Windows host)

Access tokens (elevation, integrity level, groups), **privileges** (`SeShutdownPrivilege` etc. via `AdjustTokenPrivileges`), **AppContainer** + package identity capabilities, UIPI. Adapter shape: peer identity from the named-pipe/ALPC peer token (`ImpersonateNamedPipeClient`), authority snapshot from token groups/privileges/AppContainer SID set, refuse when the AppContainer lacks the device capability. **[U]** in full.

### 3.5 Where authority is *not* the OS

`MicPolicy`, `recordConsent`, mic allow/deny lists: the canonical schema already deleted `blockedApps`/`trustedApps` as permission strings (`audio.capnp:1079-1091`). Authority is expressed once, at acquisition — `requestMicAccess` returns the entry holding the granted reference; `revokeMicAccess` revokes it. A backend must **not** reintroduce app-name lists.

## 4. Transport primitives and per-transport capability-transfer semantics

### 4.1 Host primitive table

| Primitive | Linux | macOS | Windows |
|---|---|---|---|
| Anonymous shared memory + sealing | `memfd_create` + seals **[V]** | `shm_open` (no seals) **[U]** | named sections + ACLs **[U]** |
| Named shared memory | `shm_open`/`/dev/shm` (7.7 G tmpfs **[V]**) | `shm_open` **[U]** | `CreateFileMapping(INVALID_HANDLE_VALUE)` + security descriptor **[U]** |
| Passing an fd/handle to another process | `AF_UNIX`+`SCM_RIGHTS` — **253 per message [V]**; plus `pidfd_getfd` **[V]** (ptrace-gated) | **[U]** `SCM_RIGHTS` over `AF_UNIX` — **correction to the brief**: XNU does implement SCM_RIGHTS (this is POSIX ancillary data, not Linux-only); what macOS lacks is a `DuplicateHandle`-into-arbitrary-process primitive (needs `task_for_pid`, a restricted entitlement) — the substitute is mach port rights transfer between tasks. Confidence: knowledge, high; not verifiable here | `DuplicateHandle`/`DuplicateHandle`-into-peer via `OpenProcess(PROCESS_DUP_HANDLE)`; `AF_UNIX` exists (Win10+) and Win32 documents SCM_RIGHTS support over it **[U — verify on hardware]** |
| Peer identification | `SO_PEERCRED` **[V]** | `LOCAL_PEERCRED`/`getpeereid` **[U]** | named pipe peer token **[U]** |
| Zero-copy I/O offload | `io_uring` **[V]**, `sendfile`/`splice` **[V]**, `copy_file_range` **[V]** | kqueue + F_NOCACHE **[U]** | `TransmitFile`, registered I/O **[U]** |
| Notable absences | fanotify unusable unprivileged **[V]**; O_TMPFILE EINVAL **[V]** | — | — |

### 4.2 Per-transport semantics (what each of the four transports can honestly promise)

| Transport | Capability transfer | Framing | Limits / measured | Honest gaps |
|---|---|---|---|---|
| **InProcess** | free: the handle IS a Rust object; moving a `&mut`/owned handle is the whole mechanism | direct call; no bytes cross | none | none — but it must not be the *only* implemented mode |
| **Shmem** | **none by itself** — a shared arena carries data, not authority. Bootstrap rule: capability handles are exchanged **once** over an authenticated channel (UnixSocket SCM_RIGHTS or a bootstrap token), then handles live in the ring. A `BufferId` is only valid inside an established arena — both sides must register the mapping at open | fixed slot or length-prefixed capnp flat messages; a slot ring avoids framing entirely | `/dev/shm` 7.7 G **[V]**; 64 MB map **[V]**; abstract name ≤107 B **[V]**, fs path ≤108 B **[V]**; memfd sealable **[V]** | no peer identity → must be paired with a Unix socket for `SO_PEERCRED`; no revocation *channel* — handle validity must be checked by the owner per call; on Windows the named section's ACL is the entire authority story **[U]** |
| **UnixSocket** | **full**: `SCM_RIGHTS` passes fds **[V]** (253/batch **[V]**); a passed fd is a genuine capability — the kernel re-opened it; transfer of a memfd *is* transfer of a BufferId namespace | `SOCK_SEQPACKET` preserves message boundaries; `SOCK_STREAM` needs capnp segment-table framing (self-delimiting flat messages handle it) | path/abstract limits **[V]**; default `SO_SNDBUF`/`RCVBUF` are autotuning-dependent (not measured) | ancillary data over `SOCK_DGRAM` works **[V]**; no SCM_RIGHTS on Tcp (below); on Windows/macOS the semantics are **[U]**; an fd passed once cannot be un-passed — revocation must be modelled above the OS (capability.capnp's job) |
| **Tcp** | **none** — there is no fd transfer over TCP. The honest design: capability tokens are carried *in-band* as `Capability` structs (`capability.capnp:56`), verified per call against a registered issuer; `T.DataHandle`'s `buffer` arm has **no BufferId namespace** to reference — either degrade to `inline` (a declared copy) or run a side Shmem/UnixSocket channel and pass its handle id as an opaque token | length-prefixed capnp flat messages; must be self-delimiting (segment table is) | LAN/WAN latency; no OS meddling | `partial=true` capability reporting is mandatory here: a backend that answers as if it could do zero-copy has fabricated. Peer identity: none — handle minting must be attenuated+time-boxed or refused (§3.1 step 1) |

### 4.3 Cross-transport invariants (rules, not advice)

1. One `BufferId` namespace per shared arena; `DataHandle.buffer` is invalid across arenas and always invalid on Tcp. The producer must decide inline-vs-buffer at call time (schema rule, `types.capnp:276-286`).
2. `T.UnknownEvent.payload` is the protocol's **one** inline payload — never a second one appears because "the transport was awkward" (`types.capnp:258-294`).
3. Pull-based subscriptions everywhere with caller-stated bounds — no transport may turn an event stream into an unbounded push.

## 5. Cost pyramid

Tier definitions (per brief): **thin** ≤1 day behind a host primitive; **partial** structure exists but with real mismatch work; **subsystem** genuine integration (daemon, driver, service); **honest-permanent** no mechanism exists on that OS — the method must return `Unavailable`+reason (ErrorCode `unavailable @4`/`notSupported @11` in `types.capnp:324`) forever.

Counting rule: one row per contract method on the four root interfaces (74 + 72 + 100 + 8 = **254**), classified by the *host mechanism available on that OS*.

### Per domain × OS

| Domain | Linux thin/partial/subsystem/permanent | macOS thin/partial/subsystem/permanent | Windows thin/partial/subsystem/permanent |
|---|---|---|---|
| Files | 37 / 9 / 25 / 3 | 37 / 11 / 20 / 6 | 37 / 20 / 16 / 1 |
| Network | 16 / 32 / 24 / 0 | 13 / 33 / 26 / 0 | 13 / 35 / 24 / 0 |
| Audio | 32 / 13 / 50 / 5 | 39 / 47 / 14 / 0 | 39 / 47 / 14 / 0 |
| System | 6 / 2 / 0 / 0 | 6 / 2 / 0 / 0 | 6 / 2 / 0 / 0 |
| **Total (254)** | **91 / 56 / 99 / 8** | **95 / 93 / 60 / 6** | **95 / 102 / 54 / 1** |

### Reading of the pyramid

- **Linux is the cheapest and the least honest-able**: 91 thin methods (most syscall-level surface of any OS), but 99 subsystem methods — nearly 40% of the contract needs real services on Linux that macOS/Windows get from the first-party audio/WiFi/session stacks. The Linux focus daemon (A5), the session registry (A14), and the netlink-backed network surface are the three big-ticket items.
- **Permanent counts are small and they matter**: Linux has 8 (compression/dedup on ext4 **[V]** + loudness limits), macOS 6 (per-file encryption/compression/dedup), Windows 1 (per-file dedup stats). These are not gaps to fill with emulation — they are the contract's `Unavailable` surface, and they are where the new backends must not regress into fabrication.
- Wave scope driver: a first honest Linux milestone is the 91 thin + the 56 partial surface with the 99 subsystem methods answered `Unavailable` (reason: "subsystem not yet implemented on this backend") — that is a *valid* Wave-3/4 deliverable under law 6, not a failure.

## 6. Pitfalls from the old implementation (frozen reference, read only)

All of these were observed directly in `smart-protocol/crates/smart-lib/src/` **[V: file:line citations below]**.

1. **P1 — 47 hardcoded capability bools all set true.** `backend_impl.rs:151-207`: `system_get_capabilities` sets ~47 `set_*(true)` flags with no measurement at all. (The canonical schema has since replaced this bitmap with the open token set + `partial`/`note` — `system.capnp:90`.) A backend must publish **measured** tokens only.
2. **P2 — invented system facts.** `system_get_info` (backend_impl.rs:208-220) reports `os_version("unknown")` while hardcoding `hostname("localhost")` — the hostname was never queried (`gethostname` exists; this box's is `think` **[V]**). Fabricated values are the unforgivable defect.
3. **P3 — `Box::leak` per call.** `build_capnp!` (backend_impl.rs:104-111) leaks every message and reader to make `'static` readers. Every `list_directory`, `list_devices`, `stat_file` call leaks. The new core must return owned/pooled readers with a declared arena lifetime.
4. **P4 — one value for 17 special paths.** `files_get_special_path` (backend_impl.rs:2016-2022) returns `$HOME` for every `SpecialPathType` — documents, downloads, trash and config all answered "home". Wrong-by-fabrication; per-type resolution or `Unavailable`, never a default.
5. **P5 — CLI scraping as the platform layer.** `pactl` (audio.rs:49), `ip` (network.rs:29), `cp -r` (platforms/linux/files.rs:26), `sv`/`systemctl` (service/*). Consequences: fork/exec per file operation, locale-dependent text parsing, no error codes, no pagination, and a fabricated `0.5` volume fallback when `pactl` output doesn't parse (audio.rs:44). Note this box runs **PipeWire, not PulseAudio** **[V]** — the old code's whole audio surface worked only through a compat layer.
6. **P6 — mute that ignores its argument.** `audio_set_mute(device, mute)` calls `toggle_output_mute()` (backend_impl.rs:309; audio.rs:83) — it ignores the requested state and flips the current one. A retry double-toggles. State-destroying bug class: never derive an action from current state when the caller stated the target.
7. **P7 — fabricated audio stream facts.** `audio_list_streams` sets `sample_rate(44100)`, `channels(2)`, `device.id(0)` regardless of reality (backend_impl.rs:280-303); `parse_volume`'s 0.5 fallback (audio.rs:44). Report what the server says or `Unavailable`.
8. **P8 — `Unavailable` as both catch-all and placeholder.** macOS and Windows platform modules are **two-line stubs** (platforms/macos.rs, platforms/windows.rs) compiled in only when targeting those OSes, while the Linux path answers `SmartError::Unavailable` with no reason string. Over-conservative fake unavailability (e.g. `symlink`/`hardlink` refused on non-unix even where the host offers them, files.rs:31-52) is the *opposite* sin of fabrication and equally dishonest.
9. **P9 — no transport layer at all.** No `Channel`/InProcess/Shmem/UnixSocket/Tcp abstraction, no `BufferId` registry: `T.DataHandle`'s buffer arm was unimplementable and zero-copy impossible. The new design's entire transport seam (§4) exists because of this.
10. **P10 — the god-trait.** `backend.rs` imported three deleted schemas and merged every domain into one trait (charter §anti-pattern; confirmed by the single 4658-line `backend_impl.rs` covering ~48 domains). Per-domain object-safe traits only.
11. **P11 — compile-time platform fiction.** `Platform::current()` + `#[cfg(target_os)]` module gating meant "macOS support" existed only as a target triple. Platform capability must be **runtime-detected and published** via `caps()`, not compiled-in.

Audit-corpus notes consulted: `audit/audit_analysis.md` (timelessness: "no hardcoded limits"), `audit/capability.md` (closed operation lists), `audit-9/audio.capnp.md` (the transport-enum-to-open-token history that P7-era code re-froze), `audit/files.md` (POSIX-shaped permissions — the reason files must carry *no* permission model).

## 7. Corrections to the brief (per charter: a wrong brief reported, not followed)

1. **"macOS … no SCM_RIGHTS equivalent"** — incorrect. XNU implements `SCM_RIGHTS` over `AF_UNIX` (POSIX ancillary data). The genuine macOS gaps are the absence of a `DuplicateHandle`-into-arbitrary-process primitive (`task_for_pid` is entitlement-restricted) and of anything like `pidfd_getfd`. The substitute is mach port rights transfer. **[U: knowledge, high confidence — verify on first macOS host.]**
2. **"io_uring availability"** as a *question* — answered: `io_uring_setup` returns a usable fd on this box **[V]**; treat as present on Linux ≥5.1.
3. Cost-pyramid "e.g. spatial audio, VPN" listed as subsystem — confirmed for spatial (Linux subsystem, macOS/Windows partial) and VPN (subsystem on all three).

## 8. Open items for other waves (not mine to build)

- **Wave-2 core**: `BufferId` registry keyed by (arena id, slot), plus the acquisition adapter interface in §3.1 as a trait hole each platform module fills.
- **Wave-3 transport**: §4.2/§4.3 is the contract to implement; `SO_PEERCRED`-based peer identity must land in the UnixSocket transport, not in each backend.
- **Wave-4 backends**: the permanent-method list per OS (§5) is the *only* set of methods allowed to answer `Unavailable` permanently; everything else must be thin/partial/subsystem or explicitly staged.
- **macOS/Windows verification task**: one hardware pass to convert every **[U]** into **[V]**/**[F]** (false) before Wave-4 claims those platforms in `caps()`.
