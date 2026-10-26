//! The selection **watcher**: the thread that makes the clipboard domain notice
//! a copy somebody else made.
//!
//! # Why the backend cannot do this by itself
//!
//! Every read in [`super::clipboard`] is *pull*: a caller asks, the backend
//! introspects the owner and answers. Nothing in that path fires when the user
//! presses Ctrl-C in another application — the contract has no host callback to
//! give us, because neither ICCCM nor `wlr-data-control` has one. A history that
//! only records copies this process performed is not a clipboard history; it is a
//! log of the app's own writes.
//!
//! So the daemon runs one watcher thread. It is the smallest honest mechanism
//! that produces the contract's `Copied` event for copies nobody told us about:
//!
//! ```text
//!   every POLL_INTERVAL
//!     owner_token()  ── changed? ──▶ targets() ──▶ read(preferred targets)
//!          │                                            │
//!          │ unchanged                                  ▼
//!          ▼                                     backend.ingest(offers)
//!        (sleep)                                 ├─ dedupe by fingerprint
//!                                                ├─ ledger + change event
//!                                                └─ durable archive
//! ```
//!
//! # The cost model, stated
//!
//! `owner_token` is one `GetSelectionOwner` round trip and answers the question
//! "did anything happen?" for every X11 copy, because the copying client becomes
//! the new owner. Only when it changes do we pay for `targets()` plus at most
//! three payload transfers. A transport that has no owner notion (Wayland's
//! data-control device) returns `None`, and the watcher degrades to reading the
//! preferred target each tick and letting the backend's content fingerprint drop
//! the duplicate — more expensive, still correct, and never a silent gap.
//!
//! # What it deliberately does not do
//!
//! It never *writes* the selection, never claims ownership, never injects input
//! and never spawns a process. It is a reader on its own connection: the watch
//! thread owns its own [`SelectionHost`], because an X connection driven from two
//! threads is a protocol error waiting to happen, and because the served
//! connection is busy answering callers.

extern crate std;

use alloc::string::String;
use alloc::vec::Vec;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use super::clipboard::{
    LinuxClipboardBackend, SelectionHost, SelectionRead, WaylandSelectionHost, X11SelectionHost,
};
use crate::domains::clipboard::types::ClipboardContent;

/// How long the watcher sleeps between owner checks. Fast enough that a copy
/// shows up in the app while the user is still reaching for the palette, slow
/// enough that an idle daemon costs one round trip per tick.
pub const POLL_INTERVAL: Duration = Duration::from_millis(700);

/// How many representations one observed copy captures. A clipboard entry can
/// advertise a dozen targets; reading all of them per copy would multiply the
/// cost of every Ctrl-C, and the three that matter for this contract are the
/// plain text, the rich text a paste target wants, and the image.
const MAX_REPRESENTATIONS: usize = 3;

/// One running watcher. Dropping it stops the thread: a daemon that exits must
/// not leave a reader spinning.
pub struct ClipboardWatcher {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl ClipboardWatcher {
    /// Start watching the seat, feeding `backend`.
    ///
    /// The host is connected **here**, on the watch thread's own connection, so a
    /// display that is unreachable is an error the caller can report once at
    /// startup rather than a thread that silently does nothing.
    pub fn spawn(backend: Arc<Mutex<LinuxClipboardBackend>>) -> Result<Self, String> {
        Self::spawn_with(POLL_INTERVAL, native_host, backend)
    }

    /// The general form: a poll interval and a host factory (tests drive this
    /// with a scripted host, so the polling protocol is proven without a seat).
    pub fn spawn_with<F>(
        interval: Duration,
        host_factory: F,
        backend: Arc<Mutex<LinuxClipboardBackend>>,
    ) -> Result<Self, String>
    where
        F: FnOnce() -> Result<Box<dyn SelectionHost>, String> + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("smart-clipboard-watch".into())
            .spawn(move || {
                let host = match host_factory() {
                    Ok(host) => host,
                    Err(_reason) => return,
                };
                run_loop(host.as_ref(), backend.as_ref(), interval, &stop_for_thread);
            })
            .map_err(|error| alloc::format!("cannot start the clipboard watcher: {error}"))?;
        Ok(ClipboardWatcher { stop, thread: Some(thread) })
    }

    /// Ask the thread to stop and wait for it. Idempotent.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for ClipboardWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The host this session should watch: the same native decision the backend
/// makes (Wayland first when the seat advertises data-control, X11 otherwise).
fn native_host() -> Result<Box<dyn SelectionHost>, String> {
    let display = std::env::var_os("DISPLAY").is_some();
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    if wayland {
        match WaylandSelectionHost::connect() {
            Ok(host) => return Ok(Box::new(host)),
            Err(reason) => {
                if !display {
                    return Err(reason);
                }
            }
        }
    }
    if display {
        return X11SelectionHost::connect().map(|host| Box::new(host) as Box<dyn SelectionHost>);
    }
    Err("no-backend: this host has no seat to watch (no WAYLAND_DISPLAY, no DISPLAY)".to_string())
}

/// The polling loop, written once so the tests drive exactly what the daemon
/// drives.
fn run_loop(
    host: &dyn SelectionHost,
    backend: &Mutex<LinuxClipboardBackend>,
    interval: Duration,
    stop: &AtomicBool,
) {
    // 0 means "nothing observed yet, so the current owner is new to us": the
    // first tick records whatever is on the clipboard at startup, which is what
    // a user expects when they start the service with something already copied.
    let mut last_owner: Option<u64> = None;
    while !stop.load(Ordering::SeqCst) {
        let owner = host.owner_token();
        // Only X11-style transports can answer this. `None` means "read every
        // tick"; the backend's fingerprint makes the repeat a no-op.
        let changed = match (owner, last_owner) {
            (Some(current), Some(previous)) => current != previous,
            (Some(_), None) => true,
            (None, _) => true,
        };
        last_owner = owner;
        if changed {
            let targets = host.targets();
            if !targets.is_empty() {
                let mut offers: Vec<ClipboardContent> = Vec::new();
                for mime_type in preferred_targets(&targets) {
                    match host.read(&mime_type) {
                        Ok(SelectionRead::Data(bytes)) if !bytes.is_empty() => {
                            offers.push(ClipboardContent { mime_type, data: bytes });
                        }
                        // An owner that advertises a target it will not serve is
                        // common (a lazy image, a TARGETS-only advertiser). It is
                        // not a copy and not an error: try the next one.
                        _ => {}
                    }
                }
                if !offers.is_empty() {
                    // One short lock: the served connection answers callers from
                    // the same backend, so the ingest must not span a transfer.
                    let backend = backend.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    backend.ingest(offers);
                }
            }
        }
        // Sleep in slices so `stop` is honoured promptly rather than after a
        // full interval.
        let mut slept = Duration::ZERO;
        while slept < interval && !stop.load(Ordering::SeqCst) {
            let slice = Duration::from_millis(50).min(interval - slept);
            std::thread::sleep(slice);
            slept += slice;
        }
    }
}

/// Which of an owner's advertised targets to read, in preference order.
///
/// The order is a *paste-quality* order, not an alphabetic one: `text/plain`
/// first because it is what every history row, every search and every text
/// preflight reads; `text/html` next because a rich-text target wants it and
/// losing it is the classic "my formatting is gone" bug; then an image. Anything
/// else the owner advertises is recorded as a MIME the item offers (the item's
/// target list is the owner's own), but its bytes are not pulled — a watched copy
/// is captured for use, not mirrored in full.
///
/// The X11 spellings (`UTF8_STRING`, `STRING`) are included because an ICCCM
/// owner advertises atoms, and the backend maps them to the contract's MIME names
/// on read.
pub fn preferred_targets(targets: &[String]) -> Vec<String> {
    const ORDER: [&str; 9] = [
        "text/plain",
        "text/plain;charset=utf-8",
        "UTF8_STRING",
        "text/html",
        "text/rtf",
        "image/png",
        "image/jpeg",
        "image/jpg",
        "image/webp",
    ];
    let mut chosen: Vec<String> = Vec::new();
    for wanted in ORDER {
        if chosen.len() >= MAX_REPRESENTATIONS {
            break;
        }
        if let Some(offered) = targets.iter().find(|target| target.as_str() == wanted) {
            if !chosen.iter().any(|have| have == offered) {
                chosen.push(offered.clone());
            }
        }
    }
    // Nothing from the preference list: take the owner's own first target, so a
    // custom MIME type (a colour, a file list, an app-specific blob) is still
    // captured rather than dropped on the floor.
    if chosen.is_empty() {
        if let Some(first) = targets
            .iter()
            .find(|target| !is_administrative(target))
            .cloned()
        {
            chosen.push(first);
        }
    }
    chosen
}

/// Targets that describe the selection rather than being it. Reading them would
/// record the protocol's bookkeeping as if it were the user's content.
fn is_administrative(target: &str) -> bool {
    matches!(
        target,
        "TARGETS" | "MULTIPLE" | "TIMESTAMP" | "DELETE" | "INSERT_SELECTION" | "SAVE_TARGETS"
            | "MIME_TYPES" | "COMPOUND_TEXT" | "LOST_SELECTION"
    )
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use crate::domains::clipboard::backend::ClipboardBackend;
    use crate::domains::clipboard::types::Pagination;
    use alloc::string::ToString;
    use alloc::vec;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;
    use std::time::Duration;

    fn targets(list: &[&str]) -> Vec<String> {
        list.iter().map(|item| item.to_string()).collect()
    }

    /// Text first, then rich text, then an image — and never a protocol atom.
    #[test]
    fn the_preference_order_is_text_then_html_then_image() {
        let offered = targets(&["application/placeholder", "image/png", "text/html", "TARGETS", "text/plain"]);
        assert_eq!(
            preferred_targets(&offered),
            vec!["text/plain".to_string(), "text/html".to_string(), "image/png".to_string()]
        );
    }

    /// An ICCCM owner advertising atoms still yields the text target, because the
    /// backend maps `UTF8_STRING` onto `text/plain` on read.
    #[test]
    fn x11_atom_spellings_are_understood() {
        let offered = targets(&["TARGETS", "STRING", "UTF8_STRING"]);
        assert_eq!(preferred_targets(&offered), vec!["UTF8_STRING".to_string()]);
    }

    /// An owner advertising only custom types is still captured, by its first
    /// non-administrative target.
    #[test]
    fn a_custom_target_is_still_captured() {
        let offered = targets(&["TARGETS", "application/x-smart-colour"]);
        assert_eq!(
            preferred_targets(&offered),
            vec!["application/x-smart-colour".to_string()]
        );
        // Nothing to take at all is an empty answer, not a panic.
        assert!(preferred_targets(&targets(&["TARGETS", "MULTIPLE"])).is_empty());
    }

    /// No more than three representations are pulled per observed copy.
    #[test]
    fn the_representation_budget_is_bounded() {
        let offered = targets(&["text/plain", "text/html", "text/rtf", "image/png", "image/jpeg"]);
        assert_eq!(preferred_targets(&offered).len(), MAX_REPRESENTATIONS);
    }

    /// A scripted seat: a fixed owner script, one target set, one payload. It
    /// counts what the loop asks it, so the loop's *cost* is assertable too.
    struct ScriptedHost {
        owners: Mutex<Vec<u64>>,
        targets: Vec<String>,
        payload: Vec<u8>,
        reads: AtomicUsize,
        polls: AtomicUsize,
        stop: Arc<AtomicBool>,
        stop_after_polls: usize,
    }

    impl SelectionHost for ScriptedHost {
        fn targets(&self) -> Vec<String> {
            self.targets.clone()
        }

        fn read(&self, mime_type: &str) -> std::result::Result<SelectionRead, String> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if self.targets.iter().any(|target| target == mime_type) {
                Ok(SelectionRead::Data(self.payload.clone()))
            } else {
                Ok(SelectionRead::NotOffered("not offered here".to_string()))
            }
        }

        fn claim(&self, _offers: Vec<(String, Vec<u8>)>) -> std::result::Result<(), String> {
            Err("the watcher never writes".to_string())
        }

        fn relinquish(&self) -> std::result::Result<(), String> {
            Err("the watcher never writes".to_string())
        }

        fn owner_token(&self) -> Option<u64> {
            let index = self.polls.fetch_add(1, Ordering::SeqCst);
            let owners = self.owners.lock().unwrap();
            if index + 1 >= self.stop_after_polls {
                // The loop honours this on its next check, which is what a
                // daemon's SIGTERM path does.
                self.stop.store(true, Ordering::SeqCst);
            }
            Some(owners[index.min(owners.len() - 1)])
        }
    }

    fn scripted(owners: Vec<u64>, offered: &[&str], payload: &[u8], polls: usize) -> (ScriptedHost, Arc<AtomicBool>) {
        let stop = Arc::new(AtomicBool::new(false));
        (
            ScriptedHost {
                owners: Mutex::new(owners),
                targets: targets(offered),
                payload: payload.to_vec(),
                reads: AtomicUsize::new(0),
                polls: AtomicUsize::new(0),
                stop: Arc::clone(&stop),
                stop_after_polls: polls,
            },
            stop,
        )
    }

    /// The loop records one item per **new owner** and never twice for the same
    /// one: four polls across two owners is two history rows, and only those two
    /// reads were paid for.
    #[test]
    fn the_loop_ingests_once_per_new_owner() {
        let backend = Arc::new(Mutex::new(LinuxClipboardBackend::solid()));
        let (host, stop) = scripted(vec![7, 7, 7, 9, 9], &["text/plain"], b"copied elsewhere", 5);
        run_loop(&host, &backend, Duration::from_millis(1), &stop);
        assert!(stop.load(Ordering::SeqCst), "the script stopped the loop");
        let page = backend
            .lock()
            .unwrap()
            .get_history(Pagination { offset: 0, limit: 10 })
            .expect("history");
        assert_eq!(page.total, 2, "two owners, two copies, no duplicates");
        assert_eq!(host.reads.load(Ordering::SeqCst), 2, "one read per observed change");
        assert_eq!(host.polls.load(Ordering::SeqCst), 5);
    }

    /// A seat with no owner notion (`owner_token` is `None` for Wayland) still
    /// records the copy — and the backend's fingerprint keeps the repeats out.
    #[test]
    fn a_transport_without_an_owner_token_falls_back_to_content_dedupe() {
        let backend = Arc::new(Mutex::new(LinuxClipboardBackend::solid()));
        let (host, stop) = scripted(vec![1, 1, 1, 1, 1, 1], &["text/plain"], b"same bytes", 6);
        // Simulate the Wayland arm: no token, so every tick is a candidate.
        struct NoToken(ScriptedHost);
        impl SelectionHost for NoToken {
            fn targets(&self) -> Vec<String> {
                self.0.targets()
            }
            fn read(&self, mime_type: &str) -> std::result::Result<SelectionRead, String> {
                self.0.read(mime_type)
            }
            fn claim(&self, _offers: Vec<(String, Vec<u8>)>) -> std::result::Result<(), String> {
                Err("the watcher never writes".to_string())
            }
            fn relinquish(&self) -> std::result::Result<(), String> {
                Err("the watcher never writes".to_string())
            }
            fn owner_token(&self) -> Option<u64> {
                let _ = self.0.owner_token();
                None
            }
        }
        let wrapper = NoToken(host);
        run_loop(&wrapper, &backend, Duration::from_millis(1), &stop);
        let page = backend
            .lock()
            .unwrap()
            .get_history(Pagination { offset: 0, limit: 10 })
            .expect("history");
        assert_eq!(page.total, 1, "six identical reads are one history row");
    }
}
