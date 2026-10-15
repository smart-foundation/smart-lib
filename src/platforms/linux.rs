//! The Linux platform layer. Each domain cell lives at
//! `crate::platforms::linux::<domain>`; this file declares them.
//!
//! (Charter note: the per-OS aggregator file is unassigned in the ownership
//! table; the network domain agent created it because it is the first Linux
//! (os, domain) cell and the module tree needs it to wire the cell. A later
//! Linux cell agent adds its own `#[cfg(feature = …)] pub mod <domain>;`.)

// The network backend is only compiled when its feature is on, so the platform
// layer stays green in every feature configuration (charter law #2). The
// explicit `#[path]` is required because build.rs loads this file with its own
// `#[path]`, which changes how the compiler resolves sibling submodules.
#[cfg(feature = "domain-network")]
#[path = "linux/network.rs"]
pub mod network;
// The kernel-first read seam for the network domain (law #9): netlink behind a
// mockable `NetlinkReader`, feeding the backend's state store.
#[cfg(feature = "domain-network")]
#[path = "linux/netlink.rs"]
pub mod netlink;

// The clipboard domain's native selection backend (law #9): the CLIPBOARD
// selection protocol spoken by a real X client (`x11rb`) — no wl-paste/xclip
// subprocess — behind the mockable `SelectionHost` seam.
#[cfg(feature = "domain-clipboard")]
#[path = "linux/clipboard.rs"]
pub mod clipboard;

// The logging domain's native sink surface (law #9): the host's own log files
// and per-service svlogd supervisor directories, measured (readable or
// root-only with the OS's own words), plus this process's own stderr — the one
// sink a hosted process owns on a host with no /dev/log and no journald.
#[cfg(feature = "domain-logging")]
#[path = "linux/logging.rs"]
pub mod logging;

// The time domain's native wall-clock and calendar surface (law #9): the
// kernel's CLOCK_REALTIME, the host's own zone read from /etc/localtime's
// symlink and TZif file, and the contract's own civil-date arithmetic.
#[cfg(feature = "domain-time")]
#[path = "linux/time.rs"]
pub mod time;

// The clock domain's native time surfaces (law #9): the kernel's own clocks
// (`clock_gettime`/`clock_getres`), the kernel's own timers (`timerfd`), and
// the boot epoch from /proc/stat — behind no seam at all, because a clock read
// is the host's own kernel answering and a scripted clock would be the mock
// proving the mapping.
#[cfg(feature = "domain-clock")]
#[path = "linux/clock.rs"]
pub mod clock;

// The service domain's supervision backend (law #9): the supervisor's own
// state read natively — runit's supervise tree, systemd's system bus over the
// `dbus` crate, openrc's state files — behind the `NativeSupervision` seam, so
// no path forks `sv`/`systemctl`/`rc-service` (the archive body's P5 shellout).
#[cfg(feature = "domain-service")]
#[path = "linux/service/mod.rs"]
pub mod service;

// The files domain's native file surface (law #9): the host's own descriptors
// (`openat`/`mmap`/`pwrite`/`fsync`), `statx(2)` for metadata, `getdents64`
// for the walks, `inotify` for the watches, `/proc/mounts` for the mount rows,
// user xattrs for the policy attributes, and a content-addressed store under
// the host's own cache root for versioning and content addressing. It replaces
// the archive body's `Command::new("cp") -r` with the kernel's own
// `copy_file_range` — no fork per operation, no locale-dependent text, no lost
// error codes.
#[cfg(feature = "domain-files")]
#[path = "linux/files.rs"]
pub mod files;

// The process domain's native process surface (law #9): `/proc/<pid>/stat`,
// `/proc/<pid>/status`, `/proc/<pid>/cmdline` and `/proc/<pid>/statm` read
// natively (the archive body's /proc parsing, salvaged and made real), plus
// `kill(2)` for the signal verbs instead of `Command::new("kill")`.
#[cfg(feature = "domain-process")]
#[path = "linux/process.rs"]
pub mod process;

// The settings domain's native configuration surface (law #9): the host's own
// compiled gschema and dconf databases read out of their binary gvdb format,
// and the XDG config files as they are — no `Command::new("gsettings")` (the
// archive body's condemned fork-per-key path).
#[cfg(feature = "domain-settings")]
#[path = "linux/settings.rs"]
pub mod settings;
