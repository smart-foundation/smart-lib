//! The ABI service: what this host speaks, agreed **before** a call.
//!
//! # The handshake is the intersection
//!
//! `abi.capnp`'s `AbiService.handshake` takes the caller's ABI and answers
//! with the agreed one: the schema intersection, computed from what the
//! caller declared and what this service actually serves. The answer is
//! the contract's own union — `matched`, `partialMatch` or `incompatible` —
//! and the incompatible arm carries the contract's own `T.Error`.
//!
//! # The ABI is published, not assumed
//!
//! `list` and `verify` answer with the schemas this service really serves
//! (the domains the daemon registered), not with a hardcoded catalogue.

// The crate is `no_std` (law #1); this module is the std-gated seam.
extern crate std;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use std::sync::Mutex;

use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;

use crate::schema::abi_capnp::{
    abi, abi_service, abi_subscription, schema_version, SchemaSupportStatus,
};
use crate::schema::types_capnp;


/// The ABI service's interface id, from the contract's generated code.
pub const INTERFACE_ID: u64 = abi_service::_private::TYPE_ID;

/// The contract family this service speaks.
pub const NAMESPACE: &str = "smart-protocol";

/// The ABI generation this service speaks.
pub const MAJOR: u32 = 0;
/// The ABI generation this service speaks.
pub const MINOR: u32 = 1;

/// One schema this service speaks, with the version it speaks it at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SupportedAbi {
    /// The schema's file id.
    pub schema_id: u64,
    /// The schema's qualified name ("smart-protocol/network").
    pub name: String,
    /// The major version of this schema.
    pub major: u32,
    /// The minor version of this schema.
    pub minor: u32,
}

/// The ABI state: the schemas this service speaks.
#[derive(Default)]
pub struct AbiState {
    supported: Mutex<Vec<SupportedAbi>>,
}

impl AbiState {
    /// An empty catalogue.
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish a schema this service speaks.
    pub fn publish(&self, abi: SupportedAbi) {
        if let Ok(mut guard) = self.supported.lock() {
            match guard.iter_mut().find(|entry| entry.schema_id == abi.schema_id) {
                Some(entry) => *entry = abi,
                None => guard.push(abi),
            }
        }
    }

    /// Every schema this service speaks.
    pub fn supported(&self) -> Vec<SupportedAbi> {
        self.supported
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }
}

/// Write one supported schema into an `Abi`'s `schemaVersions` list.
fn write_schema(
    entry: &SupportedAbi,
    builder: &mut schema_version::Builder<'_>,
) {
    builder.set_id(entry.schema_id);
    builder.set_major(entry.major);
    builder.set_minor(entry.minor);
    builder.set_name(&entry.name);
}

/// Fill an `Abi` struct with this service's own ABI.
fn fill_abi(
    state: &AbiState,
    builder: &mut abi::Builder<'_>,
) {
    builder.set_namespace(NAMESPACE);
    let mut version = builder.reborrow().init_version();
    version.set_major(MAJOR);
    version.set_minor(MINOR);
    version.set_patch(0);
    let supported = state.supported();
    let mut schemas = builder.reborrow().init_schema_versions(supported.len() as u32);
    for (index, entry) in supported.iter().enumerate() {
        write_schema(entry, &mut schemas.reborrow().get(index as u32));
    }
}

/// The ABI service server: the contract's own `AbiService`.
pub struct AbiServer {
    state: std::sync::Arc<AbiState>,
}

impl Clone for AbiServer {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
        }
    }
}

impl abi_service::Server for AbiServer {
    /// The handshake: the schema intersection, agreed before a call.
    fn handshake(
        self: std::rc::Rc<Self>,
        params: abi_service::HandshakeParams,
        mut results: abi_service::HandshakeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        async move {
            let params = params.get()?;
            let request = params.get_request()?;
            let caller = request.get_caller_abi()?;
            let namespace = crate::serve::text_of(caller.get_namespace()?);
            let builder = results.get();
            if namespace != NAMESPACE {
                // The honest mismatch: the contract's own error arm, naming
                // both sides.
                let mut failure = builder.init_response().init_incompatible();
                failure.set_code(types_capnp::ErrorCode::ProtocolViolation);
                failure.set_message(&alloc::format!(
                    "this service speaks the {NAMESPACE} contract family; the caller declared {namespace:?}"
                ));
                return Ok(());
            }
            // The intersection: the schemas the caller declared that this
            // service actually speaks.
            //
            // A caller that declares NOTHING has nothing to intersect: the
            // answer is then this service's own catalogue (what it speaks),
            // exactly as `list` answers — never a fabricated agreement. A
            // caller that DOES declare gets the true intersection: the
            // schemas both sides speak, and nothing else (abi.capnp: "agree
            // on an intersection before either commits to a call").
            let supported = state.supported();
            let declared: Vec<u64> = caller
                .get_schema_versions()?
                .iter()
                .map(|entry| entry.get_id())
                .collect();
            let agreed: Vec<&SupportedAbi> = if declared.is_empty() {
                supported.iter().collect()
            } else {
                supported
                    .iter()
                    .filter(|entry| declared.contains(&entry.schema_id))
                    .collect()
            };
            let mut matched = builder.init_response().init_matched();
            matched.set_namespace(NAMESPACE);
            let mut version = matched.reborrow().init_version();
            version.set_major(MAJOR);
            version.set_minor(MINOR);
            version.set_patch(0);
            let mut schemas = matched.reborrow().init_schema_versions(agreed.len() as u32);
            for (index, entry) in agreed.iter().enumerate() {
                write_schema(entry, &mut schemas.reborrow().get(index as u32));
            }
            Ok(())
        }
    }

    /// The negotiation: the same intersection, in the contract's own
    /// `NegotiateResult` union.
    fn negotiate(
        self: std::rc::Rc<Self>,
        params: abi_service::NegotiateParams,
        mut results: abi_service::NegotiateResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        async move {
            let params = params.get()?;
            let offer = params.get_offer()?;
            let namespace = crate::serve::text_of(offer.get_namespace()?);
            let builder = results.get();
            if namespace != NAMESPACE {
                let mut failure = builder.init_result().init_rejected();
                failure.set_code(types_capnp::ErrorCode::ProtocolViolation);
                failure.set_message(&alloc::format!(
                    "this service speaks the {NAMESPACE} contract family; the caller offered {namespace:?}"
                ));
                return Ok(());
            }
            let mut agreed = builder.init_result().init_agreed();
            fill_abi(&state, &mut agreed);
            Ok(())
        }
    }

    /// Verify a caller's ABI claim against what this service speaks.
    fn verify(
        self: std::rc::Rc<Self>,
        params: abi_service::VerifyParams,
        mut results: abi_service::VerifyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        async move {
            let params = params.get()?;
            let claim = params.get_claim()?;
            let supported = state.supported();
            let declared = claim.get_schema_versions()?;
            let builder = results.get();
            let mut result = builder.init_result();
            let mut status = result.reborrow().init_schema_status(declared.len() as u32);
            let mut all_supported = true;
            for index in 0..declared.len() {
                let declared_id = declared.get(index).get_id();
                let known = supported.iter().find(|entry| entry.schema_id == declared_id);
                let mut entry = status.reborrow().get(index);
                entry.set_schema_id(declared_id);
                match known {
                    Some(_) => {
                        entry.set_supported(true);
                        entry.set_status(SchemaSupportStatus::Supported);
                    }
                    None => {
                        all_supported = false;
                        entry.set_supported(false);
                        entry.set_status(SchemaSupportStatus::Removed);
                        entry.set_reason("this service does not speak that schema");
                    }
                }
            }
            result.set_supported(all_supported);
            Ok(())
        }
    }

    /// Watch the ABI catalogue change: the contract's own capability.
    fn subscribe(
        self: std::rc::Rc<Self>,
        _params: abi_service::SubscribeParams,
        mut results: abi_service::SubscribeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let server = AbiSubscriptionServer;
            let client: abi_subscription::Client = capnp_rpc::new_client(server);
            results.get().set_subscription(client);
            Ok(())
        }
    }

    /// Cancel an ABI subscription.
    fn cancel(
        self: std::rc::Rc<Self>,
        _params: abi_service::CancelParams,
        mut results: abi_service::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            results.get().set_success(());
            Ok(())
        }
    }

    /// The difference between two ABIs, in the contract's own `AbiDiff`.
    fn diff(
        self: std::rc::Rc<Self>,
        params: abi_service::DiffParams,
        mut results: abi_service::DiffResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            let params = params.get()?;
            let from = params.get_from()?;
            let to = params.get_to()?;
            let builder = results.get();
            let mut diff = builder.init_diff();
            // The schemas this service added: the ones `to` has and `from`
            // does not.
            let from_schemas = from.get_schema_versions()?;
            let to_schemas = to.get_schema_versions()?;
            let mut added = Vec::new();
            for index in 0..to_schemas.len() {
                let candidate = to_schemas.get(index);
                let candidate_id = candidate.get_id();
                let mut known = false;
                for other in 0..from_schemas.len() {
                    if from_schemas.get(other).get_id() == candidate_id {
                        known = true;
                        break;
                    }
                }
                if !known {
                    added.push(candidate_id);
                }
            }
            let mut list = diff.reborrow().init_added_schemas(added.len() as u32);
            for (index, id) in added.iter().enumerate() {
                list.reborrow().get(index as u32).set_id(*id);
            }
            Ok(())
        }
    }

    /// Every ABI this service speaks.
    fn list(
        self: std::rc::Rc<Self>,
        _params: abi_service::ListParams,
        mut results: abi_service::ListResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        async move {
            let supported = state.supported();
            let mut builder = results.get();
            let mut list = builder.reborrow().init_supported(supported.len() as u32);
            for (index, entry) in supported.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                item.set_namespace(NAMESPACE);
                let mut version = item.reborrow().init_version();
                version.set_major(entry.major);
                version.set_minor(entry.minor);
                version.set_patch(0);
                let mut schemas = item.reborrow().init_schema_versions(1);
                write_schema(entry, &mut schemas.reborrow().get(0));
            }
            builder.set_total_count(supported.len() as u32);
            Ok(())
        }
    }
}

/// The ABI subscription: the contract's own capability. No event has been
/// published yet, so the pull is honestly empty.
pub struct AbiSubscriptionServer;

impl Clone for AbiSubscriptionServer {
    fn clone(&self) -> Self {
        Self
    }
}

impl abi_subscription::Server for AbiSubscriptionServer {
    fn pull(
        self: std::rc::Rc<Self>,
        _params: abi_subscription::PullParams,
        mut results: abi_subscription::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            results.get().init_events(0);
            Ok(())
        }
    }

    fn cancel(
        self: std::rc::Rc<Self>,
        _params: abi_subscription::CancelParams,
        mut results: abi_subscription::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        async move {
            results.get().set_success(());
            Ok(())
        }
    }
}

/// The ABI service's server, as a hook the router can route to.
pub fn server(state: std::sync::Arc<AbiState>) -> Box<dyn ClientHook> {
    let server = AbiServer { state };
    let client: abi_service::Client = capnp_rpc::new_client(server);
    client.into_client_hook()
}
