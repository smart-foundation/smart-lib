//! The migration queue, as data.
//!
//! The convergence's honesty rule: every domain the original shell hosted is
//! present here with its status from the recon — REAL-today, queued
//! (migrated-pending), or archive-only — and nothing appears as a fake
//! command. Two consumers read this one table:
//!
//! 1. `smart migrate` renders it (the queue, its order and its rationale).
//! 2. the shell's dispatcher answers an unmigrated domain from it, so the
//!    honest answer ("migration pending — archive body exists at …") is
//!    generated from the same data, never hand-written per domain.
//!
//! The archive is the old repo read-only: `smart-protocol/crates/smart-lib/
//! src/platforms/linux/<domain>.rs`.

/// Where a domain stands on the migration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    /// The command is real on the new engine today.
    RealToday,
    /// In the migration queue with an order position.
    MigratedPending,
    /// Not queued yet; the archive body exists (real or placeholder).
    ArchiveBodyExists,
}

impl Status {
    pub const fn token(self) -> &'static str {
        match self {
            Status::RealToday => "real-today",
            Status::MigratedPending => "migrated-pending",
            Status::ArchiveBodyExists => "archive-body-exists",
        }
    }
    pub const fn glyph(self) -> &'static str {
        match self {
            Status::RealToday => "◉",
            Status::MigratedPending => "◌",
            Status::ArchiveBodyExists => "○",
        }
    }
}

/// One domain in the queue. The counts are the recon's: for queued domains,
/// the real-method count over the contract's method total (the value order);
/// for the rest, what the archive holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Domain {
    /// The shell grammar name — the domain's full name, no abbreviations.
    pub name: &'static str,
    pub status: Status,
    /// The recon position (1-based); `None` when not queued.
    pub order: Option<u16>,
    /// What the archive holds, e.g. "platforms/linux/clipboard.rs (real body, 295 lines)".
    pub archive: &'static str,
    /// The recon count for queued domains, e.g. "3/3 methods".
    pub count: &'static str,
}

/// The queue. The order is the recon's value order, past the real-today domains
/// — network, clipboard, service, and the batch-4 six the engine landed in one
/// commit (files, process, settings, logging, clock, time, each native and
/// live-proven with the contract's own rows answered by measured reasons), the
/// batch-5 three (audio, media, power) and the batch-6 six (database, queue,
/// appindex, manifest, vcard, content). The positions below are contiguous: a
/// landed domain leaves the queue and the queue closes the gap. What remains:
/// the multi-backend domains (window 10/36 across four WMs, package's 19
/// managers, compositor's 9 backends) — each of those is its own wave with its
/// own live proof in the shell.
pub const QUEUE: &[Domain] = &[
    Domain {
        name: "network",
        status: Status::RealToday,
        order: None,
        archive: "engine: netlink reads + NM policy/control (live-proven, 358 tests)",
        count: "real engine",
    },
    Domain {
        name: "clipboard",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: native X11 CLIPBOARD selection over x11rb (live-proven) + wlr-data-control on Wayland (seat-proven over a real wayland-server peer) — no wl-paste/xclip shellout",
        count: "native · 5 REAL / 15 DEGRADED / 0 UNAVAILABLE",
    },
    // ── the ordered queue (C2: one dispatch per domain, in this order) ──
    Domain {
        name: "queue",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: the engine's own in-process FIFO — named queues of byte payloads behind one mutex, with the contract's own enqueue/dequeue/peek/clearQueue/deleteQueue surface (no broker shellout; the store lives in the engine's process memory)",
        count: "native · 7 REAL / 0 DEGRADED / 26 UNAVAILABLE",
    },
    Domain {
        name: "service",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: the supervisor's own state read natively — runit's supervise tree plus the /proc-derived supervision truth (live states with pids and clocks for EVERY uid on this host), systemd's system bus over the dbus crate, openrc's own state files — no sv/systemctl/rc-service shellout. The one physically-privileged half is the supervisor's exact stat words: sudo ./target/debug/examples/service-state-probe --state-root /run/runit",
        count: "native · 7 REAL / 12 DEGRADED / 10 UNAVAILABLE",
    },
    Domain {
        name: "media",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: MPRIS over the session bus (dbus crate) — listPlayers/getPlayer/play/pause/stop/next/previous/seek/getMetadata/getPosition/setVolume/setPosition/openUri/subscribePlayer/getSupportedFormats (live-proven)",
        count: "native · 15 REAL / 0 DEGRADED / 24 UNAVAILABLE",
    },
    Domain {
        name: "database",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: the engine's own in-process namespaced key/value store — namespaces of keys to byte payloads behind one mutex, with the contract's own listKeys/listNamespaces paging (no SQL engine shellout; the store lives in the engine's process memory)",
        count: "native · 27 REAL / 0 DEGRADED / 49 UNAVAILABLE",
    },
    Domain {
        name: "files",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: the host's own files read natively — statx(2) metadata, getdents64 walks,              mmap + pread on the handle surface, copy_file_range copies, inotify watches,              statvfs usage, SHA-256 implemented in the engine, the file's own user xattrs, and a              content-addressed store beside the host's own cache root (live-proven)",
        count: "native · 48 REAL / 31 DEGRADED / 4 UNAVAILABLE",
    },
    Domain {
        name: "audio",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: PulseAudio client library (libpulse) via FFI threaded mainloop — getState/listDevices/listStreams/setVolume/setMute/setStreamVolume/setStreamMute + subscriptions (live-proven, no shellout)",
        count: "native · 12 REAL / 4 DEGRADED / 84 UNAVAILABLE",
    },
    Domain {
        name: "power",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: UPower D-Bus (system bus) for battery + cpufreq sysfs for profiles; /sys/class/power_supply fallback (live-proven, no shellout)",
        count: "native · 5 REAL / 1 DEGRADED / 16 UNAVAILABLE",
    },
    Domain {
        name: "process",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: /proc/<pid>/{stat,status,cmdline,statm,environ,maps,task} read natively,              btime from /proc/stat, the host's own page size and CLK_TCK through sysconf, a              measured 20 ms CPU window, and kill(pid,0) as the read-shaped signal probe              (live-proven)",
        count: "native · 16 REAL / 17 DEGRADED / 13 UNAVAILABLE",
    },
    Domain {
        name: "window",
        status: Status::MigratedPending,
        order: Some(1),
        archive: "platforms/linux/window.rs (real body, 412 lines)",
        count: "10/36 methods",
    },
    Domain {
        name: "package",
        status: Status::MigratedPending,
        order: Some(2),
        archive: "platforms/linux/package/ (19 managers: apt, dnf, pacman, …)",
        count: "19 managers",
    },
    Domain {
        name: "compositor",
        status: Status::MigratedPending,
        order: Some(3),
        archive: "platforms/linux/compositor/ (9 backends: sway, hyprland, i3, …)",
        count: "9 backends",
    },
    // ── archive-only: a real body exists, migration not yet scheduled ──
    Domain {
        name: "display",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/display.rs (real body, 246 lines)",
        count: "archive body",
    },
    Domain {
        name: "filesystem",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/fs.rs (real body, 103 lines)",
        count: "archive body",
    },
    Domain {
        name: "web",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/web.rs (real body, 124 lines)",
        count: "archive body",
    },
    Domain {
        name: "app",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/app.rs (real body, 106 lines)",
        count: "archive body",
    },
    Domain {
        name: "appindex",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: the host's OWN freedesktop desktop-entry corpus — the .desktop files under the XDG data directories, read with read_dir and the entry format's own INI grammar (177 entries on this host). No gtk-launch, no xdg-open shellout",
        count: "native · 5 REAL / 3 DEGRADED / 11 UNAVAILABLE",
    },
    Domain {
        name: "calendar",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/calendar.rs (real body, 96 lines)",
        count: "archive body",
    },
    Domain {
        name: "contacts",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/contacts.rs (real body, 83 lines)",
        count: "archive body",
    },
    Domain {
        name: "print",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/print.rs (real body, 81 lines)",
        count: "archive body",
    },
    Domain {
        name: "search",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/search.rs (real body, 78 lines)",
        count: "archive body",
    },
    Domain {
        name: "notification",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/notification.rs (real body, 75 lines)",
        count: "archive body",
    },
    Domain {
        name: "secret",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/secret.rs (real body, 71 lines)",
        count: "archive body",
    },
    Domain {
        name: "time",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: the kernel's own CLOCK_REALTIME, the host's own /etc/localtime and its own              TZif file (the version-2/3 64-bit block), and the contract's own TAI/Unix              converters and arithmetic calendar kernels — never chrono's epoch math (live-proven)",
        count: "native · 6 REAL / 1 DEGRADED / 1 UNAVAILABLE",
    },
    Domain {
        name: "usb",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/usb.rs (real body, 65 lines)",
        count: "archive body",
    },
    Domain {
        name: "speech",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/speech.rs (real body, 65 lines)",
        count: "archive body",
    },
    Domain {
        name: "update",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/update.rs (real body, 64 lines)",
        count: "archive body",
    },
    Domain {
        name: "settings",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: the host's own settings engine (glib's GSettings through the gio crate) —              the compiled schema source for schemas/defaults/ranges, the host's own dconf              database for values, and the host's own `changed` signal for the watch surface. No              gsettings fork, no dconf fork (live-proven)",
        count: "native · 11 REAL / 7 DEGRADED / 2 UNAVAILABLE",
    },
    Domain {
        name: "camera",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/camera.rs (real body, 61 lines)",
        count: "archive body",
    },
    Domain {
        name: "backup",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/backup.rs (real body, 60 lines)",
        count: "archive body",
    },
    Domain {
        name: "logging",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: this host's own sinks, measured by opening each one — the svlogd              supervisor directories under /run/runit, the files under /var/log, and this              process's own stderr (which under a runit service IS the svlogd input). No              journalctl fork, no sv fork, no dmesg fork: this host has no journal to parse              (live-proven)",
        count: "native · 6 REAL / 5 DEGRADED / 0 UNAVAILABLE",
    },
    Domain {
        name: "font",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/font.rs (real body, 58 lines)",
        count: "archive body",
    },
    Domain {
        name: "translation",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/translation.rs (real body, 37 lines)",
        count: "archive body",
    },
    Domain {
        name: "clock",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: the kernel's own clocks — clock_gettime/clock_getres for the readings,              timerfd_create + timerfd_settime for a real timerfd (a periodic timer that expired              three times reports 3), nanosleep on the caller's own thread, /proc/stat's btime,              and the measured chrony verdict with the root-only state dir named (live-proven)",
        count: "native · 25 REAL / 5 DEGRADED / 0 UNAVAILABLE",
    },
    // ── the Batch 6 domains the ORIGINAL shell never hosted ──────────────
    // The engine has a real body for each and the shell's command surface
    // answers them today, so they join the board as real-today — not as a
    // queue position, and not as an archive body to salvage.
    Domain {
        name: "manifest",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: the engine's own in-process manifest store — real manifests with real names, versions, entry points and dependencies, and a REAL validation (a required dependency no manifest in the store offers is an error the caller is told about)",
        count: "native · 5 REAL / 0 DEGRADED / 11 UNAVAILABLE",
    },
    Domain {
        name: "vcard",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: the engine's OWN in-process RFC 6350 parser and formatter — groups, the open parameter list, line unfolding, the backslash escapes, structured values, base64 binary values, URI values and dates; the contract's normative mapping table row by row, and the two rows the canonical tree cannot carry reported as warnings",
        count: "native · 7 REAL / 0 DEGRADED / 1 UNAVAILABLE",
    },
    Domain {
        name: "content",
        status: Status::RealToday,
        order: None,
        archive:
            "engine: the engine's own in-process content store — real items with real ids, versions, timestamps, payloads and tags, and the contract's own optimistic-concurrency check (a write presenting a stale version is refused with a conflict, never a silent overwrite)",
        count: "native · 10 REAL / 0 DEGRADED / 47 UNAVAILABLE",
    },
    // ── archive-only: the archive holds a placeholder (available() = false) ──
    Domain {
        name: "accessibility",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/accessibility.rs (placeholder: available() = false)",
        count: "placeholder",
    },
    Domain {
        name: "auth",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/auth.rs (placeholder: available() = false)",
        count: "placeholder",
    },
    Domain {
        name: "buffer",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/buffer.rs (placeholder: available() = false)",
        count: "placeholder",
    },
    Domain {
        name: "capture",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/capture.rs (placeholder: available() = false)",
        count: "placeholder",
    },
    Domain {
        name: "extension",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/extension.rs (placeholder: available() = false)",
        count: "placeholder",
    },
    Domain {
        name: "input",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/input.rs (placeholder: available() = false)",
        count: "placeholder",
    },
    Domain {
        name: "location",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/location.rs (placeholder: available() = false)",
        count: "placeholder",
    },
    Domain {
        name: "session",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/session.rs (placeholder: available() = false)",
        count: "placeholder",
    },
    Domain {
        name: "surface",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/surface.rs (placeholder: available() = false)",
        count: "placeholder",
    },
    Domain {
        name: "system",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/system.rs (placeholder: available() = false)",
        count: "placeholder",
    },
    Domain {
        name: "textinput",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/textinput.rs (placeholder: available() = false)",
        count: "placeholder",
    },
    Domain {
        name: "theme",
        status: Status::ArchiveBodyExists,
        order: None,
        archive: "platforms/linux/theme.rs (placeholder: available() = false)",
        count: "placeholder",
    },
];

/// Look a domain up by its shell grammar name (the full name, no abbreviations).
pub fn find(domain: &str) -> Option<&'static Domain> {
    QUEUE.iter().find(|d| d.name == domain)
}

/// The honest answer for a domain that is not migrated yet: the queue's own
/// data says where the archive body is and what it holds. A domain that is real
/// today is never told it is "pending" — honesty cuts both ways.
pub fn pending_reason(domain: &str) -> String {
    match find(domain) {
        Some(d) if d.status == Status::RealToday => format!(
            "{domain} is real today on the engine — run `smart {domain}` (see `smart migrate`)"
        ),
        Some(d) => format!(
            "migration pending — archive body exists at {} ({}); see `smart migrate`",
            d.archive, d.count
        ),
        None => format!(
            "unknown domain: {domain} — not in the original shell's command map; \
             `smart migrate` lists the queue"
        ),
    }
}

/// The queue's sizes, for the summary line and the tests.
pub fn counts() -> (usize, usize, usize) {
    let real = QUEUE.iter().filter(|d| d.status == Status::RealToday).count();
    let pending = QUEUE.iter().filter(|d| d.status == Status::MigratedPending).count();
    let archive = QUEUE.iter().filter(|d| d.status == Status::ArchiveBodyExists).count();
    (real, pending, archive)
}

/// The domains that are LIVE on the engine today, in the board's own order.
///
/// This is the single source the REPL's `help` lists from: the list is the
/// board's (a domain appears because its status is `RealToday`, not because a
/// help string remembered to mention it), so `help` can never again claim 3 of
/// 9 live domains while six answer real commands in the same session.
pub fn live_domains() -> Vec<&'static str> {
    QUEUE
        .iter()
        .filter(|d| d.status == Status::RealToday)
        .map(|d| d.name)
        .collect()
}

/// The queue as a value, so the shell language can pipe it
/// (`migrate | where status == migrated-pending | count`).
pub fn queue_value() -> crate::selector::Value {
    use crate::selector::Value;
    Value::List(
        QUEUE
            .iter()
            .map(|d| {
                Value::Object(vec![
                    ("name".into(), Value::String(d.name.to_string())),
                    ("status".into(), Value::String(d.status.token().to_string())),
                    (
                        "order".into(),
                        match d.order {
                            Some(n) => Value::Number(n as f64),
                            None => Value::Null,
                        },
                    ),
                    ("archive".into(), Value::String(d.archive.to_string())),
                    ("count".into(), Value::String(d.count.to_string())),
                ])
            })
            .collect(),
    )
}

/// Render the queue: the order, each domain's status, and where its archive
/// body lives. Human is the aesthetic; Json is the same data for agents.
pub fn render_queue(format: &crate::OutputFormat) -> String {
    use crate::color::ColorExt;
    use crate::widgets::{truncate_to, Table};

    /// The widest the ARCHIVE BODY column may grow before the cut is marked.
    const ARCHIVE_COLUMN: usize = 72;

    match format {
        crate::OutputFormat::Json => crate::value::to_json(&queue_value()),
        crate::OutputFormat::Raw => {
            QUEUE
                .iter()
                .map(|d| format!("{}\t{}\t{}", d.name, d.status.token(), d.archive))
                .collect::<Vec<_>>()
                .join("\n")
        }
        crate::OutputFormat::Human => {
            let (real, pending, archive) = counts();
            let mut out = String::new();
            out.push_str(&format!(
                "  {} {}\n",
                "🧭".blue().bold(),
                "Migration Queue".blue().bold()
            ));
            out.push_str(&format!(
                "  {} domains: {} real today, {} queued, {} archive-only\n\n",
                QUEUE.len(),
                real,
                pending,
                archive
            ));

            let mut ordered: Vec<&Domain> = QUEUE.iter().collect();
            ordered.sort_by_key(|d| (d.order.unwrap_or(u16::MAX), d.name));

            let mut table = Table::new(vec![
                "".into(),
                "DOMAIN".into(),
                "STATUS".into(),
                "COUNT".into(),
                "ARCHIVE BODY".into(),
            ]);
            for d in &ordered {
                let position = match d.order {
                    Some(n) => format!("{n}."),
                    None => "·".to_string(),
                };
                table = table.row(vec![
                    position,
                    d.name.to_string(),
                    format!("{} {}", d.status.glyph(), d.status.token()),
                    d.count.to_string(),
                    // The archive body is bounded to a column (never wrapped, the
                    // cut is marked) — the full sentence is one door away in the
                    // JSON and raw forms.
                    truncate_to(d.archive, ARCHIVE_COLUMN),
                ]);
            }
            out.push_str(&table.render());
            out.push('\n');
            out.push_str(&format!(
                "\n  {} the full archive body of any row: {}\n",
                "●".blue().bold(),
                "smart migrate -o json".dimmed()
            ));
            out.push_str(&format!(
                "\n  {} The order is the recon's value order, past the batch-4 six (files,\n",
                "●".blue().bold()
            ));
            out.push_str(
                "    process, settings, logging, clock, time — all native, all live-proven), the\n",
            );
            out.push_str(
                "    batch-5 three (audio, media, power) and the batch-6 six (database, queue,\n",
            );
            out.push_str(
                "    appindex, manifest, vcard, content). What remains is the multi-backend\n",
            );
            out.push_str(
                "    domains (window 10/36 across four WMs, package's 19 managers, compositor's\n",
            );
            out.push_str(
                "    9 backends). Each migration is its own wave with its own live proof in the\n",
            );
            out.push_str("    shell.\n");
            out.push_str(&format!(
                "\n  {} A domain not yet migrated answers honestly from this table:\n",
                "●".blue().bold()
            ));
            out.push_str("    its command says where the archive body is, never a fake.\n");
            out
        }
    }
}
