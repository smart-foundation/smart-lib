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
    fn list_apps(
        self: Rc<Self>,
        _params: ai::ListAppsParams,
        _results: ai::ListAppsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("listApps") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_app(
        self: Rc<Self>,
        _params: ai::GetAppParams,
        _results: ai::GetAppResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getApp") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn launch(
        self: Rc<Self>,
        _params: ai::LaunchParams,
        _results: ai::LaunchResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("launch") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn launch_url(
        self: Rc<Self>,
        _params: ai::LaunchUrlParams,
        _results: ai::LaunchUrlResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("launchUrl") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_pinned(
        self: Rc<Self>,
        _params: ai::GetPinnedParams,
        _results: ai::GetPinnedResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getPinned") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn pin_app(
        self: Rc<Self>,
        _params: ai::PinAppParams,
        _results: ai::PinAppResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("pinApp") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn unpin_app(
        self: Rc<Self>,
        _params: ai::UnpinAppParams,
        _results: ai::UnpinAppResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("unpinApp") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn search_apps(
        self: Rc<Self>,
        _params: ai::SearchAppsParams,
        _results: ai::SearchAppsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("searchApps") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_recent_apps(
        self: Rc<Self>,
        _params: ai::GetRecentAppsParams,
        _results: ai::GetRecentAppsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getRecentApps") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn clear_recent_apps(
        self: Rc<Self>,
        _params: ai::ClearRecentAppsParams,
        _results: ai::ClearRecentAppsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("clearRecentApps") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn subscribe(
        self: Rc<Self>,
        _params: ai::SubscribeParams,
        _results: ai::SubscribeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("subscribe") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn pull(
        self: Rc<Self>,
        _params: ai::PullParams,
        _results: ai::PullResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("pull") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn cancel(
        self: Rc<Self>,
        _params: ai::CancelParams,
        _results: ai::CancelResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("cancel") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_stats(
        self: Rc<Self>,
        _params: ai::GetStatsParams,
        _results: ai::GetStatsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getStats") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn list_capabilities(
        self: Rc<Self>,
        _params: ai::ListCapabilitiesParams,
        _results: ai::ListCapabilitiesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("listCapabilities") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_dependents(
        self: Rc<Self>,
        _params: ai::GetDependentsParams,
        _results: ai::GetDependentsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getDependents") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn batch_get(
        self: Rc<Self>,
        _params: ai::BatchGetParams,
        _results: ai::BatchGetResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("batchGet") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }
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
