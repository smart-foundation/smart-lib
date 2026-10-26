//! # smart-clipboard-proto — a clipboard product that owns nothing
//!
//! The protocol half of **smart-clipboard**. It is a *client*, in the strict
//! sense the smart-protocol charter uses: it holds capability handles and it
//! calls interfaces. It does not read the X11 selection, it does not speak
//! `wlr-data-control`, it does not keep a history ledger, and it opens no
//! database file. Those are the backend's job, and the backend is whatever
//! service the discovery interface points at.
//!
//! ```text
//!   smart-clipboard (gpui UI)
//!            │  commands / events (no gpui types cross this line)
//!            ▼
//!     this crate: worker thread
//!            │  capnp RPC over the unix carrier
//!            ▼
//!   smart-service  ── discovery.capnp ──▶ "clipboard is served here"
//!                  ── clipboard.capnp ──▶ getHistory / searchHistory /
//!                                          getPreview / convertContent /
//!                                          setContent / setOffer / pin / …
//!                  ── capability.capnp ─▶ "this connection is ReadOnly"
//!            │
//!            ▼
//!     the backend (engine + its platform cells + whatever store it owns)
//! ```
//!
//! ## The three rules this crate is built to keep
//!
//! 1. **Nothing is invented.** A preview the service does not serve is
//!    rendered as "not served, with a reason", never as a guess. The schema
//!    id the service publishes is compared with the one this binary compiled,
//!    and a mismatch is *shown*, not swallowed.
//! 2. **Nothing is stored here.** No cache on disk, no database, no ledger.
//!    The list you see is a window over the service's history, fetched per
//!    query. Close the app and the app has forgotten everything — because it
//!    never knew anything to begin with.
//! 3. **The UI never blocks on the wire.** One worker thread owns the RPC
//!    session; the UI sends commands and receives events. A hung service is a
//!    banner, not a frozen window.
//!
//! ## Where to look
//!
//! * [`config`] — how the service is found, and every candidate that was tried;
//! * [`error`] — the honest error vocabulary (a refusal *class* survives the
//!   wire, because the engine prefixes it);
//! * [`model`] — the owned types the UI renders (no capnp reader escapes);
//! * [`session`] — connect + the three interfaces + the handshake checks;
//! * [`client`] — the typed calls, one per contract method the UI uses;
//! * [`worker`] — the actor that keeps all of the above off the UI thread.

pub mod client;
pub mod config;
pub mod contract;
pub mod error;
pub mod model;
pub mod session;
pub mod worker;

/// The contract's generated modules, straight from `build.rs`.
///
/// The module names are capnpc's (`<schema>_capnp`), so a reader can map any
/// name here back to the schema file it came from without a lookup table.
pub mod schema {
    #![allow(clippy::all)]
    #![allow(missing_docs)]
    #![allow(dead_code)]
    #![allow(unused_imports)]

    pub mod types_capnp {
        include!(concat!(env!("OUT_DIR"), "/types_capnp.rs"));
    }
    pub mod capability_capnp {
        include!(concat!(env!("OUT_DIR"), "/capability_capnp.rs"));
    }
    pub mod discovery_capnp {
        include!(concat!(env!("OUT_DIR"), "/discovery_capnp.rs"));
    }
    pub mod clipboard_capnp {
        include!(concat!(env!("OUT_DIR"), "/clipboard_capnp.rs"));
    }
}

pub use client::{
    ClipboardClient, CopyReport, CopyTarget, LiveContent, Page, Query, Representation,
};
pub use config::{Candidate, EndpointSource, ServiceLocation};
pub use error::{ClientError, Refusal, RefusalClass};
pub use model::{
    Authority, CapabilityRow, ChangeEvent, ChangeKind, ContentState, Item, ItemId, MimeFamily,
    Observation, PreviewState, ServedMethod, ServiceInfo,
};
pub use worker::{BackendEvent, Command, Worker, WorkerHandle, PULL_BATCH, PULL_INTERVAL};
