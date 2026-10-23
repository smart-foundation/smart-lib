#![allow(missing_docs)]
//! The manifest domain, served through the contract's generated `Server` trait.
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
use crate::domains::manifest::availability;
use crate::domains::manifest::backend::ManifestBackend;
use crate::schema::manifest_capnp::manifest_service as mf;
use crate::schema::types_capnp;
use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;
use super::availability::matrix_reader;
matrix_reader!(availability);

pub struct ManifestService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for ManifestService<B> {
    fn clone(&self) -> Self {
        Self { backend: Arc::clone(&self.backend), class: self.class }
    }
}

impl<B> ManifestService<B>
where
    B: ManifestBackend + 'static,
{
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }
}

fn gate(class: CapabilityClass, verb: &str) -> Option<capnp::Error> {
    if class.is_control() { return None; }
    Some(errors::decline(
        ErrorCode::PermissionDenied,
        &format!("permissionDenied: the {verb} verb changes this host's manifest state, and this connection holds the read-only class"),
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
            &format!("invalidArgument: {name} is not a ManifestService contract method"),
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

impl<B> mf::Server for ManifestService<B>
where
    B: ManifestBackend + 'static,
{
}

pub struct ManifestDomain<B> {
    backend: Arc<B>,
}

impl<B> ManifestDomain<B>
where
    B: ManifestBackend + 'static,
{
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for ManifestDomain<B>
where
    B: ManifestBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        mf::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::MANIFEST_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "manifest"
    }

    fn category(&self) -> &'static str {
        "manifest"
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
                "manifest resolve",
                "ManifestService",
                "resolve",
                "resolve",
                0,
                true,
            )
            .with_row(matrix_row("resolve")),            DomainCommand::new(
                "manifest list",
                "ManifestService",
                "list",
                "list",
                1,
                true,
            )
            .with_row(matrix_row("list")),            DomainCommand::new(
                "manifest subscribe",
                "ManifestService",
                "subscribe",
                "subscribe",
                2,
                true,
            )
            .with_row(matrix_row("subscribe")),            DomainCommand::new(
                "manifest pull",
                "ManifestService",
                "pull",
                "pull",
                3,
                true,
            )
            .with_row(matrix_row("pull")),            DomainCommand::new(
                "manifest compose",
                "ManifestService",
                "compose",
                "compose",
                4,
                true,
            )
            .with_row(matrix_row("compose")),            DomainCommand::new(
                "manifest diff",
                "ManifestService",
                "diff",
                "diff",
                5,
                true,
            )
            .with_row(matrix_row("diff")),            DomainCommand::new(
                "manifest validate",
                "ManifestService",
                "validate",
                "validate",
                6,
                true,
            )
            .with_row(matrix_row("validate")),            DomainCommand::new(
                "manifest sign",
                "ManifestService",
                "sign",
                "sign",
                7,
                true,
            )
            .with_row(matrix_row("sign")),            DomainCommand::new(
                "manifest verify",
                "ManifestService",
                "verify",
                "verify",
                8,
                true,
            )
            .with_row(matrix_row("verify")),            DomainCommand::new(
                "manifest migrate",
                "ManifestService",
                "migrate",
                "migrate",
                9,
                true,
            )
            .with_row(matrix_row("migrate")),            DomainCommand::new(
                "manifest create-template",
                "ManifestService",
                "createTemplate",
                "createTemplate",
                10,
                true,
            )
            .with_row(matrix_row("createTemplate")),            DomainCommand::new(
                "manifest compose-with-config",
                "ManifestService",
                "composeWithConfig",
                "composeWithConfig",
                11,
                true,
            )
            .with_row(matrix_row("composeWithConfig")),            DomainCommand::new(
                "manifest validate-dependencies",
                "ManifestService",
                "validateDependencies",
                "validateDependencies",
                12,
                true,
            )
            .with_row(matrix_row("validateDependencies")),            DomainCommand::new(
                "manifest resolve-by-name",
                "ManifestService",
                "resolveByName",
                "resolveByName",
                13,
                true,
            )
            .with_row(matrix_row("resolveByName")),            DomainCommand::new(
                "manifest pull",
                "ManifestService",
                "pull",
                "pull",
                14,
                true,
            )
            .with_row(matrix_row("pull")),            DomainCommand::new(
                "manifest cancel",
                "ManifestService",
                "cancel",
                "cancel",
                15,
                true,
            )
            .with_row(matrix_row("cancel")),        ]
    }

    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/schemas/manifest.schema.bin"
        )))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = ManifestService::new(Arc::clone(&self.backend), class);
        let client: mf::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
