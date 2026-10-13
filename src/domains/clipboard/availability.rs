//! The clipboard domain availability matrix: one row per `Clipboard` contract
//! method, published as data exactly the way `knowledge/03-CONFORMANCE.md` §3.2
//! prescribes. A `REAL`/`DEGRADED` claim names the `l3_test` that backs it; an
//! `UNAVAILABLE` row carries a nameable `reasonClass` and a reason that the
//! live behaviour repeats at runtime.
//!
//! This table is the contract with the availability generator and `cli.capnp`'s
//! `listUnavailable`: every non-`REAL` row is answered at runtime by the method
//! returning the matching `notSupported`/`unavailable` whose message begins
//! with the row's `reasonClass` (see [`super::backend`]).
//!
//! ## The native decision, and how it splits the matrix
//!
//! The archive body subprocessed out to `wl-paste`/`xclip` per call. The current
//! backend speaks the selection protocol natively — an X client over `x11rb`
//! here, a Wayland data-control client on a Wayland seat — so the *live
//! selection* read (`getContent`) and the write (`setContent`/`setOffer`/
//! `clearContent`) are REAL/DEGRADED on a host with a seat, and `UNAVAILABLE`
//! with `reasonClass = no-backend` when the concrete backend has no seat. For
//! this workspace the Linux backend is built without such a seat (a headless
//! build box), so its live rows report `UNAVAILABLE` at *runtime* while the
//! deterministic engine-half rows (the copy ledger, the change subscription,
//! the preview/validation/conversion/limits) stay REAL/DEGRADED — proven by the
//! mock column in the l3 tests, never by live host access.
//!
//! ## Why history is one honest DEGRADED, not a kao
//!
//! Neither X11's CLIPBOARD manager nor wlr-data-control keeps a history across
//! clients. A real host history does not exist, so it is never faked as REAL.
//! The backend serves `getHistory`/`clearHistory`/`searchHistory`/`pin`/
//! `unpin`/`delete` from a bounded **engine-side ledger of this process's own
//! copies**, published as DEGRADED-with-reason: honest that the ledger is
//! per-process, not the seat's, and bounded by the size limits so a pull never
//! returns an unbounded payload.

/// The compiled schema file ID, read from `clipboard.capnp`'s `@0x…` line. The
/// matrix can never describe a contract the build did not compile.
pub const CLIPBOARD_SCHEMA_ID: u64 = 0xa1b2c3d4e5f60012;

/// The CLI's availability reasons (`cli.capnp` `AvailabilityReason`), re-used
/// rather than a second grammar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonClass {
    /// No implementation exists for this (domain, platform).
    NoBackend,
    /// The host primitive exists but the caller's credentials do not reach it.
    NoPrivilege,
    /// The concept does not exist on this platform.
    NotApplicable,
    /// Exists, held back deliberately.
    DisabledByPolicy,
    /// Reserved and flagged: a distinguishable gap. Never used in this table.
    Unknown,
}

impl ReasonClass {
    /// The exact token the matrix, the tool reason and the CLI agree on.
    pub const fn as_str(self) -> &'static str {
        match self {
            ReasonClass::NoBackend => "no-backend",
            ReasonClass::NoPrivilege => "no-privilege",
            ReasonClass::NotApplicable => "not-applicable",
            ReasonClass::DisabledByPolicy => "disabled-by-policy",
            ReasonClass::Unknown => "unknown",
        }
    }
}

/// Per-method status.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvailStatus {
    /// Passes its full behaviour on this platform; an `l3Test` proves it.
    Real,
    /// Answers, but not completely; `reason` names the degraded capability and
    /// an `l3Test` still backs it.
    Degraded,
    /// Answers `notSupported`/`unavailable` with a `reasonClass`-prefixed reason.
    Unavailable,
}

/// One availability row.
#[derive(Clone, Copy, Debug)]
pub struct MethodAvailability {
    /// The contract method name (`clipboard.capnp`).
    pub name: &'static str,
    /// The contract ordinal (the value that never moves).
    pub ordinal: u16,
    pub status: AvailStatus,
    /// The reason class, exactly when `status == UNAVAILABLE`.
    pub reason_class: Option<ReasonClass>,
    /// For `UNAVAILABLE`: why. For `DEGRADED`: the named degraded capability.
    /// Empty for `REAL`.
    pub reason: &'static str,
    /// The L3 test that ran and passed, required when `status != UNAVAILABLE`.
    pub l3_test: Option<&'static str>,
}

impl MethodAvailability {
    const fn real(name: &'static str, ordinal: u16, l3_test: &'static str) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Real,
            reason_class: None,
            reason: "",
            l3_test: Some(l3_test),
        }
    }

    const fn degraded(
        name: &'static str,
        ordinal: u16,
        l3_test: &'static str,
        detail: &'static str,
    ) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Degraded,
            reason_class: None,
            reason: detail,
            l3_test: Some(l3_test),
        }
    }

    /// An `UNAVAILABLE` row. The native-backend target serves every contract
    /// method (each genuinely REAL or DEGRADED), so this constructor is unused by
    /// this matrix — it is kept so the grammar stays total and a future host that
    /// cannot serve a method marks it rather than faking a row.
    #[allow(dead_code)]
    const fn unavailable(
        name: &'static str,
        ordinal: u16,
        class: ReasonClass,
        reason: &'static str,
    ) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Unavailable,
            reason_class: Some(class),
            reason,
            l3_test: None,
        }
    }
}

/// The published availability matrix for the clipboard domain on the
/// native-backend (X11/Wayland) build, plus the domain's own subscription
/// `cancel` helper.
///
/// Every REAL row names an always-on test that drives a **scripted**
/// `SelectionHost` (the mock column); the host-touching live probe is the
/// separate `#[ignore]`d founder-run test. Every DEGRADED row names the mock
/// that proves its mapping, exactly as the network matrix names its NM/netlink
/// mocks. On a concrete host with a seat, `getContent`/`setContent`/
/// `setOffer`/`clearContent`/`getSupportedMimeTypes` are served by the real
/// selection transport; the reason strings below describe that resident
/// transport, and the live probe is where it is exercised.
pub const CLIPBOARD_AVAILABILITY: &[MethodAvailability] = &[
    // ── The live selection — native, no subprocess ──────────────────────────
    MethodAvailability::real(
        "getContent",
        0,
        "l3_linux_clipboard::get_content_speaks_the_selection_natively",
    ),
    MethodAvailability::degraded(
        "setContent",
        1,
        "l3_linux_clipboard::set_content_claims_the_selection_natively",
        "host-mutating: the process becomes the selection owner over the native transport (X11 \
         CLIPBOARD / wlr-data-control); the mapping is mock-proven and the live bring-back is \
         founder-run; the read after a write is served from the engine's own offer, not a \
         re-shellout to wl-copy/xclip",
    ),
    MethodAvailability::degraded(
        "clearContent",
        2,
        "l3_linux_clipboard::clear_content_relinquishes_the_selection",
        "host-mutating: the selection owner disowns the selection; the mapping is mock-proven and \
         the live bring-back is founder-run",
    ),
    // ── The engine-side copy ledger (a real, bounded, per-process history) ───
    MethodAvailability::degraded(
        "getHistory",
        3,
        "l3_linux_clipboard::history_is_a_bounded_ledger_of_this_backends_copies",
        "a bounded engine-side ledger of THIS process's copies, capped by the size limits — not a \
         host history: neither an X11 CLIPBOARD manager nor wlr-data-control keeps a history across \
         clients, so none is faked as REAL",
    ),
    MethodAvailability::degraded(
        "clearHistory",
        4,
        "l3_linux_clipboard::clear_history_empties_the_ledger_only",
        "clears the engine-side ledger only; there is no host history to clear, and the current \
         selection owner's content is left to clearContent @2",
    ),
    MethodAvailability::degraded(
        "searchHistory",
        7,
        "l3_linux_clipboard::search_history_matches_the_ledger",
        "matches the engine-side ledger's item metadata and previews; there is no host history to \
         search, so the scope is this process's copies",
    ),
    // ── The change subscription (native events, engine-merged) ──────────────
    MethodAvailability::real(
        "subscribeChanges",
        5,
        "l3_linux_clipboard::subscription_pulls_are_bounded_and_merge_native_events",
    ),
    MethodAvailability::degraded(
        "pullChanges",
        0x1005,
        "l3_linux_clipboard::subscription_pulls_are_bounded_and_merge_native_events",
        "pulls up to the caller's maxEvents; a pull reports how many engine-side events it \
         collapsed, and the native selection-worker's events are merged into the same ledger — the \
         pull is bounded even when the worker lags behind file activity",
    ),
    MethodAvailability::real(
        "cancelSubscription",
        0xFFFF,
        "l3_linux_clipboard::cancel_retires_the_subscription_token",
    ),
    MethodAvailability::real(
        "getSupportedMimeTypes",
        6,
        "l3_linux_clipboard::supported_mime_types_are_the_native_targets",
    ),
    // ── Ledger mutations (host-mutating; mapping mock-proven) ───────────────
    MethodAvailability::degraded(
        "pinItem",
        8,
        "l3_linux_clipboard::pin_and_unpin_flip_the_ledger_flag",
        "host-mutating: pins an item in the engine-side ledger against auto-expiry; the mapping is \
         mock-proven and the live bring-back is founder-run; the ledger is this process's",
    ),
    MethodAvailability::degraded(
        "unpinItem",
        9,
        "l3_linux_clipboard::pin_and_unpin_flip_the_ledger_flag",
        "host-mutating: unpins an item in the engine-side ledger; the mapping is mock-proven and \
         the live bring-back is founder-run; the ledger is this process's",
    ),
    MethodAvailability::degraded(
        "deleteItem",
        10,
        "l3_linux_clipboard::delete_item_removes_from_the_ledger",
        "host-mutating: removes one item from the engine-side ledger; the mapping is mock-proven \
         and the live bring-back is founder-run; the ledger is this process's",
    ),
    // ── Payload-shaped work over the ledger ─────────────────────────────────
    MethodAvailability::degraded(
        "getPreview",
        11,
        "l3_linux_clipboard::preview_is_a_truncation_of_the_ledger_payload",
        "a truncation of the ledger item's own payload (the same payload bytes the native transport \
         carried), never a fabricated placeholder; a binary payload has no text preview",
    ),
    MethodAvailability::real(
        "validateContent",
        12,
        "l3_linux_clipboard::validate_content_matches_the_ledger_claim",
    ),
    MethodAvailability::degraded(
        "convertContent",
        13,
        "l3_linux_clipboard::convert_content_wins_with_the_offered_representation",
        "conversion wins when this backend already offers the target mime (the native multi-offer / \
         the utf-8 text view); any other source-target conversion is a converter this backend is \
         not, so it is `unsupported`, not a guessed re-encoding",
    ),
    // ── Size limits — this backend's own policy ─────────────────────────────
    MethodAvailability::degraded(
        "getSizeLimits",
        14,
        "l3_linux_clipboard::size_limits_are_the_backends_own_policy",
        "the size limits are this backend's declared policy, not a host fact: neither X11 nor \
         wlr-data-control imposes or reports copy limits, so the values are the engine's own",
    ),
    MethodAvailability::degraded(
        "setSizeLimits",
        15,
        "l3_linux_clipboard::set_size_limits_changes_the_backend_policy",
        "host-mutating: sets this backend's policy; the mapping is mock-proven and the live \
         bring-back is founder-run",
    ),
    MethodAvailability::degraded(
        "setOffer",
        16,
        "l3_linux_clipboard::set_offer_claims_all_representations",
        "host-mutating: the process becomes the selection owner offering every representation at \
         once over the native transport; the mapping is mock-proven and the live bring-back is \
         founder-run",
    ),
];

/// The Linux platform token for this matrix.
pub const PLATFORM: &str = "linux";
/// The transports the availability generator records. Clipboard read/write are
/// transport-independent; the change subscription rides the same transports.
pub const TRANSPORTS: &[&str] = &["InProcess", "Shmem", "UnixSocket"];
/// Published clipboard capability tokens (open vocabulary).
pub const CAPS: &[&str] = &[
    "clipboard.content.read",
    "clipboard.content.write",
    "clipboard.history.ledger",
    "clipboard.change.subscription",
];

/// Count rows by status — the numbers the report cites.
pub fn status_counts() -> (usize, usize, usize) {
    let mut real = 0;
    let mut degraded = 0;
    let mut unavailable = 0;
    for row in CLIPBOARD_AVAILABILITY {
        match row.status {
            AvailStatus::Real => real += 1,
            AvailStatus::Degraded => degraded += 1,
            AvailStatus::Unavailable => unavailable += 1,
        }
    }
    (real, degraded, unavailable)
}

/// Every `l3_test` id the matrix claims — surfaced so the test suite can assert
/// each one exists (a REAL/DEGRADED claim with a vanished test is a FAIL).
pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> {
    CLIPBOARD_AVAILABILITY
        .iter()
        .filter(|r| r.l3_test.is_some())
        .map(|r| (r.name, r.l3_test.unwrap()))
}

/// The row for one contract method name, or `None` when the matrix has no such
/// method. The service harness and the CLI both resolve rows by name.
pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    CLIPBOARD_AVAILABILITY.iter().find(|r| r.name == name)
}
