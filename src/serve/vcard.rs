#![allow(missing_docs)]
//! The vcard domain, served through the contract's generated `Server` trait.
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
use crate::domains::vcard::availability;
use crate::domains::vcard::backend::VcardBackend;
use crate::schema::vcard_capnp::v_card_service as vc;
use crate::schema::types_capnp;
use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;
use super::availability::matrix_reader;
matrix_reader!(availability);

pub struct VcardService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for VcardService<B> {
    fn clone(&self) -> Self {
        Self { backend: Arc::clone(&self.backend), class: self.class }
    }
}

impl<B> VcardService<B>
where
    B: VcardBackend + 'static,
{
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }
}

fn gate(class: CapabilityClass, verb: &str) -> Option<capnp::Error> {
    if class.is_control() { return None; }
    Some(errors::decline(
        ErrorCode::PermissionDenied,
        &format!("permissionDenied: the {verb} verb changes this host's vcard state, and this connection holds the read-only class"),
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
            &format!("invalidArgument: {name} is not a VCardService contract method"),
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

impl<B> vc::Server for VcardService<B>
where
    B: VcardBackend + 'static,
{
    fn import_v_card(
        self: Rc<Self>,
        _params: vc::ImportVCardParams,
        _results: vc::ImportVCardResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("importVCard") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn export_v_card(
        self: Rc<Self>,
        _params: vc::ExportVCardParams,
        _results: vc::ExportVCardResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("exportVCard") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn parse_v_card(
        self: Rc<Self>,
        _params: vc::ParseVCardParams,
        _results: vc::ParseVCardResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("parseVCard") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn format_v_card(
        self: Rc<Self>,
        _params: vc::FormatVCardParams,
        _results: vc::FormatVCardResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("formatVCard") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_supported_versions(
        self: Rc<Self>,
        _params: vc::GetSupportedVersionsParams,
        _results: vc::GetSupportedVersionsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getSupportedVersions") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_sync_token(
        self: Rc<Self>,
        _params: vc::GetSyncTokenParams,
        _results: vc::GetSyncTokenResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getSyncToken") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn sync_changes(
        self: Rc<Self>,
        _params: vc::SyncChangesParams,
        _results: vc::SyncChangesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("syncChanges") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn resolve_conflict(
        self: Rc<Self>,
        _params: vc::ResolveConflictParams,
        _results: vc::ResolveConflictResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("resolveConflict") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }
}

pub struct VcardDomain<B> {
    backend: Arc<B>,
}

impl<B> VcardDomain<B>
where
    B: VcardBackend + 'static,
{
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for VcardDomain<B>
where
    B: VcardBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        vc::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::VCARD_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "vcard"
    }

    fn category(&self) -> &'static str {
        "vcard"
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
                "vcard import",
                "VCardService",
                "importVCard",
                "importVCard",
                0,
                true,
            )
            .with_row(matrix_row("importVCard")),            DomainCommand::new(
                "vcard export",
                "VCardService",
                "exportVCard",
                "exportVCard",
                1,
                true,
            )
            .with_row(matrix_row("exportVCard")),            DomainCommand::new(
                "vcard parse-v-card",
                "VCardService",
                "parseVCard",
                "parseVCard",
                2,
                true,
            )
            .with_row(matrix_row("parseVCard")),            DomainCommand::new(
                "vcard new",
                "VCardService",
                "formatVCard",
                "formatVCard",
                3,
                true,
            )
            .with_row(matrix_row("formatVCard")),            DomainCommand::new(
                "vcard versions",
                "VCardService",
                "getSupportedVersions",
                "getSupportedVersions",
                4,
                true,
            )
            .with_row(matrix_row("getSupportedVersions")),            DomainCommand::new(
                "vcard sync-token",
                "VCardService",
                "getSyncToken",
                "getSyncToken",
                5,
                true,
            )
            .with_row(matrix_row("getSyncToken")),            DomainCommand::new(
                "vcard sync",
                "VCardService",
                "syncChanges",
                "syncChanges",
                6,
                true,
            )
            .with_row(matrix_row("syncChanges")),            DomainCommand::new(
                "vcard resolve-conflict",
                "VCardService",
                "resolveConflict",
                "resolveConflict",
                7,
                true,
            )
            .with_row(matrix_row("resolveConflict")),        ]
    }

    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/schemas/vcard.schema.bin"
        )))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = VcardService::new(Arc::clone(&self.backend), class);
        let client: vc::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
