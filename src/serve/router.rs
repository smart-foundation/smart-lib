//! The bootstrap capability: the call names the contract interface it wants,
//! and the router hands it to the interface that owns it.
//!
//! # Why a router and not one interface
//!
//! A capnp RPC connection has exactly one *bootstrap* capability. The peer
//! reaches it, and every call it makes on it carries the **interface id**
//! the contract's own generated code assigned to the interface it means
//! (`network_capnp`'s `Network` interface, `discovery_capnp`'s `Discovery`,
//! and so on) plus the method id inside it.
//!
//! The RPC runtime hands both to the bootstrap's hook — that is the whole
//! point of the interface id on the wire. So the bootstrap must be able to
//! answer for *several* contract interfaces, and the honest way to do that
//! is to route on the interface id the caller named.
//!
//! # What this file is NOT
//!
//! It is not a method table. There is no ordinal here, no method name, no
//! per-method arm: the *methods* are dispatched by the generated
//! `ServerDispatch` of each interface, from the schema's own `interface`
//! blocks. The router only decides **which contract interface** a call
//! belongs to — the one thing the schema itself defines and the one thing
//! capnp RPC asks the transport to carry.

use alloc::boxed::Box;
use alloc::format;
use alloc::string::ToString;
use alloc::rc::Rc;
use alloc::vec::Vec;

use capnp::any_pointer;
use capnp::capability::{Client, Promise, Request};
use capnp::MessageSize;
use capnp::private::capability::ClientHook;

/// One contract interface this service serves.
pub struct Route {
    /// The interface id the contract's generated code gave this interface.
    /// Never a hand-written constant: it comes from the generated
    /// `<interface>::_private::TYPE_ID`.
    pub interface_id: u64,
    /// The hook that dispatches to the generated server for that interface.
    pub hook: Box<dyn ClientHook>,
}

/// The bootstrap capability: it routes a call to the contract interface the
/// caller named.
#[derive(Clone)]
pub struct InterfaceRouter {
    routes: Rc<Vec<Route>>,
    broken: Client,
}

impl InterfaceRouter {
    /// Build a router over the given interfaces.
    pub fn new(routes: Vec<Route>) -> Self {
        Self {
            routes: Rc::new(routes),
            broken: capnp_rpc::new_broken_cap(capnp::Error::unimplemented(
                "no contract interface is served on this connection".to_string(),
            )),
        }
    }

    /// The route for an interface id, if this service serves it.
    pub fn route(&self, interface_id: u64) -> Option<&Route> {
        self.routes
            .iter()
            .find(|route| route.interface_id == interface_id)
    }

    /// The interface ids this router serves (diagnostics).
    pub fn interface_ids(&self) -> Vec<u64> {
        self.routes.iter().map(|route| route.interface_id).collect()
    }

    /// The router as the bootstrap capability the RPC system exports.
    pub fn as_client(&self) -> Client {
        Client::new(Box::new(self.clone()))
    }
}

impl ClientHook for InterfaceRouter {
    fn add_ref(&self) -> Box<dyn ClientHook> {
        Box::new(self.clone())
    }

    fn new_call(
        &self,
        interface_id: u64,
        method_id: u16,
        size_hint: Option<MessageSize>,
    ) -> Request<any_pointer::Owned, any_pointer::Owned> {
        match self.route(interface_id) {
            Some(route) => route.hook.new_call(interface_id, method_id, size_hint),
            None => self.broken.hook.new_call(interface_id, method_id, size_hint),
        }
    }

    fn call(
        &self,
        interface_id: u64,
        method_id: u16,
        params: Box<dyn capnp::private::capability::ParamsHook>,
        results: Box<dyn capnp::private::capability::ResultsHook>,
    ) -> Promise<(), capnp::Error> {
        match self.route(interface_id) {
            Some(route) => route.hook.call(interface_id, method_id, params, results),
            None => Promise::err(capnp::Error::unimplemented(format!(
                "no contract interface with id {interface_id} is served on this connection"
            ))),
        }
    }

    fn get_brand(&self) -> usize {
        0
    }

    fn get_ptr(&self) -> usize {
        Rc::as_ptr(&self.routes) as *const u8 as usize
    }

    fn get_resolved(&self) -> Option<Box<dyn ClientHook>> {
        None
    }

    fn when_more_resolved(&self) -> Option<Promise<Box<dyn ClientHook>, capnp::Error>> {
        None
    }

    fn when_resolved(&self) -> Promise<(), capnp::Error> {
        Promise::ok(())
    }
}
