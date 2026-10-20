//! The capability service: what this host **holds**, published so a client
//! can ask instead of assuming.
//!
//! # Publication, not minting
//!
//! This backend does not implement a capability authority: it does not issue,
//! attenuate, transfer or revoke capabilities. What it *does* know is the
//! authority the host already granted the connection — the
//! [`CapabilityClass`] the peer's identity earned — and that is published
//! through `capability.capnp`'s own `CapabilityService`, in the contract's
//! own `Capability` struct.
//!
//! The methods this backend does not implement answer honestly
//! (`notSupported`), never with a fabricated success: a client that asks
//! "can I revoke?" gets the truth.

// The crate is `no_std` (law #1); this module is the std-gated seam.
extern crate std;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use std::sync::Mutex;

use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;

use crate::schema::capability_capnp::{
    capability, capability_service, Operation as ContractOperation,
};

use super::errors;
use super::CapabilityClass;

/// The capability service's interface id, from the contract's generated code.
pub const INTERFACE_ID: u64 = capability_service::_private::TYPE_ID;

/// One published capability: what the host holds, and for what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedCapability {
    /// The opaque identifier of the capability.
    pub id: Vec<u8>,
    /// The target it covers ("service/network").
    pub target: String,
    /// The operations it grants, in the contract's own vocabulary.
    pub operations: Vec<Operation>,
    /// Who issued it.
    pub issuer: String,
}

/// The operations the contract declares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    /// `read @0`.
    Read,
    /// `write @1`.
    Write,
    /// `execute @2`.
    Execute,
    /// `subscribe @3`.
    Subscribe,
    /// `publish @4`.
    Publish,
    /// `create @5`.
    Create,
    /// `destroy @6`.
    Destroy,
    /// `attenuate @7`.
    Attenuate,
    /// `transfer @8`.
    Transfer,
    /// `revoke @9`.
    Revoke,
    /// `custom @10`.
    Custom,
}

impl Operation {
    /// The contract's own spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Operation::Read => "read",
            Operation::Write => "write",
            Operation::Execute => "execute",
            Operation::Subscribe => "subscribe",
            Operation::Publish => "publish",
            Operation::Create => "create",
            Operation::Destroy => "destroy",
            Operation::Attenuate => "attenuate",
            Operation::Transfer => "transfer",
            Operation::Revoke => "revoke",
            Operation::Custom => "custom",
        }
    }
}

/// The capability state: what this host holds.
#[derive(Default)]
pub struct CapabilityState {
    published: Mutex<Vec<PublishedCapability>>,
}

impl CapabilityState {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Publish a capability.
    pub fn publish(&self, capability: PublishedCapability) {
        if let Ok(mut guard) = self.published.lock() {
            match guard.iter_mut().find(|entry| entry.id == capability.id) {
                Some(entry) => *entry = capability,
                None => guard.push(capability),
            }
        }
    }

    /// Every published capability.
    pub fn published(&self) -> Vec<PublishedCapability> {
        self.published
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// The capability the connection's class earns. This is the class
    /// *published*: the host's own authority, in the contract's own
    /// `Capability` struct — visible to a client that asks, hidden from
    /// nobody.
    pub fn class_capability(class: CapabilityClass, target: &str) -> PublishedCapability {
        let (id, operations) = match class {
            CapabilityClass::Control => (
                b"smart-lib:control".to_vec(),
                vec![Operation::Read, Operation::Execute, Operation::Subscribe],
            ),
            CapabilityClass::ReadOnly => (
                b"smart-lib:readonly".to_vec(),
                vec![Operation::Read, Operation::Subscribe],
            ),
        };
        PublishedCapability {
            id,
            target: target.to_string(),
            operations,
            issuer: "smart-lib".to_string(),
        }
    }
}

/// Write a published capability into the contract's `Capability` struct.
fn write_capability(
    published: &PublishedCapability,
    builder: &mut capability::Builder<'_>,
) -> capnp::Result<()> {
    builder.set_id(&published.id[..]);
    builder.set_target(&published.target);
    let mut operations = builder.reborrow().init_operations(published.operations.len() as u32);
    for (index, operation) in published.operations.iter().enumerate() {
        operations.reborrow().set(index as u32, match operation {
            Operation::Read => ContractOperation::Read,
            Operation::Write => ContractOperation::Write,
            Operation::Execute => ContractOperation::Execute,
            Operation::Subscribe => ContractOperation::Subscribe,
            Operation::Publish => ContractOperation::Publish,
            Operation::Create => ContractOperation::Create,
            Operation::Destroy => ContractOperation::Destroy,
            Operation::Attenuate => ContractOperation::Attenuate,
            Operation::Transfer => ContractOperation::Transfer,
            Operation::Revoke => ContractOperation::Revoke,
            Operation::Custom => ContractOperation::Custom,
        });
    }
    let mut audit = builder.reborrow().init_audit();
    audit.set_issuer(&published.issuer);
    Ok(())
}

/// The capability service server: the contract's own `CapabilityService`.
pub struct CapabilityServer {
    state: std::sync::Arc<CapabilityState>,
    class: CapabilityClass,
}

impl Clone for CapabilityServer {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            class: self.class,
        }
    }
}

impl capability_service::Server for CapabilityServer {
    /// List the capabilities this host holds.
    fn list(
        self: std::rc::Rc<Self>,
        _params: capability_service::ListParams,
        mut results: capability_service::ListResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        let class = self.class;
        async move {
            let mut published = state.published();
            published.push(CapabilityState::class_capability(class, "service"));
            let mut builder = results.get();
            let mut list = builder.reborrow().init_caps(published.len() as u32);
            for (index, entry) in published.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_capability(entry, &mut item)?;
            }
            builder.set_total_count(published.len() as u32);
            Ok(())
        }
    }

    /// A point-in-time snapshot of the same set.
    fn snapshot(
        self: std::rc::Rc<Self>,
        _params: capability_service::SnapshotParams,
        mut results: capability_service::SnapshotResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        let state = self.state.clone();
        let class = self.class;
        async move {
            let mut published = state.published();
            published.push(CapabilityState::class_capability(class, "service"));
            let mut builder = results.get();
            let mut list = builder.reborrow().init_capabilities(published.len() as u32);
            for (index, entry) in published.iter().enumerate() {
                let mut item = list.reborrow().get(index as u32);
                write_capability(entry, &mut item)?;
            }
            Ok(())
        }
    }

    /// Issuing a capability is not this backend's authority to claim.
    fn issue(
        self: std::rc::Rc<Self>,
        _params: capability_service::IssueParams,
        _results: capability_service::IssueResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service publishes the authority the host \
             granted it; it does not issue capabilities of its own",
        )))
    }

    /// Verification of a capability is the host's authority, not this
    /// service's.
    fn verify(
        self: std::rc::Rc<Self>,
        _params: capability_service::VerifyParams,
        _results: capability_service::VerifyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service publishes capabilities; it does not \
             verify them",
        )))
    }

    /// Attenuation is the capability authority's job.
    fn attenuate(
        self: std::rc::Rc<Self>,
        _params: capability_service::AttenuateParams,
        _results: capability_service::AttenuateResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service publishes capabilities; it does not \
             attenuate them",
        )))
    }

    /// Transfer is the capability authority's job.
    fn transfer(
        self: std::rc::Rc<Self>,
        _params: capability_service::TransferParams,
        _results: capability_service::TransferResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service publishes capabilities; it does not \
             transfer them",
        )))
    }

    /// Revocation is the capability authority's job.
    fn revoke(
        self: std::rc::Rc<Self>,
        _params: capability_service::RevokeParams,
        _results: capability_service::RevokeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(Err(errors::decline(
            crate::core::ErrorCode::NotSupported,
            "notSupported: no-backend: this service publishes capabilities; it does not \
             revoke them",
        )))
    }
}

/// The capability service's server, as a hook the router can route to.
pub fn server(
    state: std::sync::Arc<CapabilityState>,
    class: CapabilityClass,
) -> Box<dyn ClientHook> {
    let server = CapabilityServer { state, class };
    let client: capability_service::Client = capnp_rpc::new_client(server);
    client.into_client_hook()
}
