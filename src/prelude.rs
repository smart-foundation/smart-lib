//! Re-export point for downstream code (law #1: one API at every altitude).
//!
//! Deliberately small, and only what **every** domain author touches: the
//! generated foundation schemas, the capnp runtime surface, and core's one
//! error model plus the authority handle (charter laws #3, #5, #6). Anything a
//! domain author uses on *some* paths belongs in its own module and is imported
//! by name — `core::ids::*`, `core::values::*`, `transport::*` — never here.
//!
//! Nothing here is generated: hand-written, additive-only. The foundation
//! schemas are re-exported unconditionally because foundation compiles in every
//! feature configuration, so these paths always exist.
//!
//! ```
//! use smart_lib::prelude::*;
//!
//! // The prelude is what a domain author's file starts with: the error model
//! // rides along without any further import (law #6: honest degradation).
//! fn open_file() -> Result<()> {
//!     Err(Error::not_supported("not-applicable: no backend on this platform"))
//! }
//! # let answer = open_file();
//! # let err = answer.err().expect("not supported");
//! # assert_eq!(err.code, ErrorCode::NotSupported);
//! # assert!(!err.message.is_empty());
//! ```
//! ```
//! use smart_lib::prelude::*;
//!
//! // A capability handle wraps a generated interface client and derefs to it,
//! // so the handle behaves like the client it holds (law #5: authority is a
//! // handle, never a permission string).
//! fn read_through(cap: &Cap<capability_capnp::capability_service::Client>) {
//!     let _client: &capability_capnp::capability_service::Client = cap;
//! }
//! # fn caller(cap: &Cap<capability_capnp::capability_service::Client>) {
//! #     read_through(cap);
//! # }
//! ```

/// The foundation schemas — always compiled, in every feature configuration.
pub use crate::schema::{abi_capnp, capability_capnp, types_capnp};

/// Core's one error model: `Result<T>`, `Error` and the contract's exact
/// 20-code `ErrorCode` vocabulary. Every domain method returns these.
pub use crate::core::{Error, ErrorCode, Result};

/// The authority handle (law #5): `Cap<T>` wraps a generated interface client
/// and is what an implemented `Acquire` hands back. `Deref` reaches the
/// wrapped client's methods.
pub use crate::core::capability::Cap;

/// capnp runtime surface every domain backend needs.
pub use capnp::{
    capability, constant, data, data_list, enum_list, message, primitive_list, serialize,
    struct_list, text, text_list,
};
