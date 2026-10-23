#![allow(missing_docs)]
//! The appindex domain, served through the contract's generated `Server` trait.
extern crate std;
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use std::rc::Rc;
use std::sync::Arc;
use capnp::capability::FromClientHook;
use capnp::private::capability::ClientHook;
use crate::core::ErrorCode;
use crate::domains::appindex::availability;
use crate::domains::appindex::backend::AppindexBackend;
use crate::schema::appindex_capnp::app_index_service as ai;
use crate::schema::types_capnp;
use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;
use super::availability::matrix_reader;
matrix_reader!(availability);

pub struct AppindexService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for AppindexService<B> {
    fn clone(&self) -> Self {
        Self { backend: Arc::clone(&self.backend), class: self.class }
    }
}

impl<B> AppindexService<B>
where
    B: AppindexBackend + 'static,
{
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }
}

fn gate(class: CapabilityClass, verb: &str) -> Option<capnp::Error> {
    if class.is_control() { return None; }
    Some(errors::decline(
        ErrorCode::PermissionDenied,
        &format!("permissionDenied: the {verb} verb changes this host's appindex state, and this connection holds the read-only class"),
    ))
}

fn unavailable(name: &str) -> Option<capnp::Error> {
    match matrix_row(name) {
        Some(row) => match row.unavailable_class() {
            Some((class, reason)) => Some(errors::decline(
                ErrorCode::NotSupported,
                &format!("{class}: {reason}"),
            )),
            None => None,
        },
        None => Some(errors::decline(
            ErrorCode::InvalidArgument,
            &format!("invalidArgument: {name} is not a AppIndexService contract method"),
        )),
    }
}

fn void_result(
    result: crate::core::Result<()>,
    builder: &mut types_capnp::void_result::Builder<'_>,
) {
    match result {
        Ok(()) => builder.set_success(()),
        Err(error) => {
            let mut failure = builder.reborrow().init_error();
            errors::write_t_error(&error, &mut failure);
        }
    }
}

impl<B> ai::Server for AppindexService<B>
where
    B: AppindexBackend + 'static,
{
}

pub struct AppindexDomain<B> {
    backend: Arc<B>,
}

impl<B> AppindexDomain<B>
where
    B: AppindexBackend + 'static,
{
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for AppindexDomain<B>
where
    B: AppindexBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        ai::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::APPINDEX_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "appindex"
    }

    fn category(&self) -> &'static str {
        "appindex"
    }

    fn capability_tokens(&self) -> Vec<&'static str> {
        availability::CAPS.to_vec()
    }

    fn state(&self) -> super::discovery::ServiceState {
        let (real, degraded, unavailable) = availability::status_counts();
        if unavailable == 0 {
            super::discovery::ServiceState::Online
        } else if real + degraded == 0 {
            super::discovery::ServiceState::Offline
        } else {
            super::discovery::ServiceState::Degraded
        }
    }

    fn commands(&self) -> Vec<DomainCommand> {
        vec![
            DomainCommand::new(
                "appindex list-apps",
                "AppIndexService",
                "listApps",
                "listApps",
                0,
                true,
            )
            .with_row(matrix_row("listApps")),            DomainCommand::new(
                "appindex get",
                "AppIndexService",
                "getApp",
                "getApp",
                1,
                true,
            )
            .with_row(matrix_row("getApp")),            DomainCommand::new(
                "appindex launch",
                "AppIndexService",
                "launch",
                "launch",
                2,
                true,
            )
            .with_row(matrix_row("launch")),            DomainCommand::new(
                "appindex launch-url",
                "AppIndexService",
                "launchUrl",
                "launchUrl",
                3,
                true,
            )
            .with_row(matrix_row("launchUrl")),            DomainCommand::new(
                "appindex pinned",
                "AppIndexService",
                "getPinned",
                "getPinned",
                4,
                true,
            )
            .with_row(matrix_row("getPinned")),            DomainCommand::new(
                "appindex pin",
                "AppIndexService",
                "pinApp",
                "pinApp",
                5,
                true,
            )
            .with_row(matrix_row("pinApp")),            DomainCommand::new(
                "appindex unpin",
                "AppIndexService",
                "unpinApp",
                "unpinApp",
                6,
                true,
            )
            .with_row(matrix_row("unpinApp")),            DomainCommand::new(
                "appindex search-apps",
                "AppIndexService",
                "searchApps",
                "searchApps",
                7,
                true,
            )
            .with_row(matrix_row("searchApps")),            DomainCommand::new(
                "appindex get-recent-apps",
                "AppIndexService",
                "getRecentApps",
                "getRecentApps",
                8,
                true,
            )
            .with_row(matrix_row("getRecentApps")),            DomainCommand::new(
                "appindex clear-recent-apps",
                "AppIndexService",
                "clearRecentApps",
                "clearRecentApps",
                9,
                true,
            )
            .with_row(matrix_row("clearRecentApps")),            DomainCommand::new(
                "appindex subscribe",
                "AppIndexService",
                "subscribe",
                "subscribe",
                10,
                true,
            )
            .with_row(matrix_row("subscribe")),            DomainCommand::new(
                "appindex pull",
                "AppIndexService",
                "pull",
                "pull",
                11,
                true,
            )
            .with_row(matrix_row("pull")),            DomainCommand::new(
                "appindex cancel",
                "AppIndexService",
                "cancel",
                "cancel",
                12,
                true,
            )
            .with_row(matrix_row("cancel")),            DomainCommand::new(
                "appindex stats",
                "AppIndexService",
                "getStats",
                "getStats",
                13,
                true,
            )
            .with_row(matrix_row("getStats")),            DomainCommand::new(
                "appindex list-capabilities",
                "AppIndexService",
                "listCapabilities",
                "listCapabilities",
                14,
                true,
            )
            .with_row(matrix_row("listCapabilities")),            DomainCommand::new(
                "appindex get-dependents",
                "AppIndexService",
                "getDependents",
                "getDependents",
                15,
                true,
            )
            .with_row(matrix_row("getDependents")),            DomainCommand::new(
                "appindex batch",
                "AppIndexService",
                "batchGet",
                "batchGet",
                16,
                true,
            )
            .with_row(matrix_row("batchGet")),            DomainCommand::new(
                "appindex pull",
                "AppIndexService",
                "pull",
                "pull",
                17,
                true,
            )
            .with_row(matrix_row("pull")),            DomainCommand::new(
                "appindex cancel",
                "AppIndexService",
                "cancel",
                "cancel",
                18,
                true,
            )
            .with_row(matrix_row("cancel")),        ]
    }

    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/schemas/appindex.schema.bin"
        )))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = AppindexService::new(Arc::clone(&self.backend), class);
        let client: ai::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
