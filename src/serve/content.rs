#![allow(missing_docs)]
//! The content domain, served through the contract's generated `Server` trait.
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
use crate::domains::content::availability;
use crate::domains::content::backend::ContentBackend;
use crate::schema::content_capnp::content_service as ct;
use crate::schema::types_capnp;
use super::errors;
use super::service::{DomainCommand, ServedDomain};
use super::CapabilityClass;
use super::availability::matrix_reader;
matrix_reader!(availability);

pub struct ContentService<B> {
    backend: Arc<B>,
    class: CapabilityClass,
}

impl<B> Clone for ContentService<B> {
    fn clone(&self) -> Self {
        Self { backend: Arc::clone(&self.backend), class: self.class }
    }
}

impl<B> ContentService<B>
where
    B: ContentBackend + 'static,
{
    pub fn new(backend: Arc<B>, class: CapabilityClass) -> Self {
        Self { backend, class }
    }
}

fn gate(class: CapabilityClass, verb: &str) -> Option<capnp::Error> {
    if class.is_control() { return None; }
    Some(errors::decline(
        ErrorCode::PermissionDenied,
        &format!("permissionDenied: the {verb} verb changes this host's content state, and this connection holds the read-only class"),
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
            &format!("invalidArgument: {name} is not a ContentService contract method"),
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

impl<B> ct::Server for ContentService<B>
where
    B: ContentBackend + 'static,
{
}

pub struct ContentDomain<B> {
    backend: Arc<B>,
}

impl<B> ContentDomain<B>
where
    B: ContentBackend + 'static,
{
    pub fn new(backend: Arc<B>) -> Self {
        Self { backend }
    }
}

impl<B> ServedDomain for ContentDomain<B>
where
    B: ContentBackend + 'static,
{
    fn interface_id(&self) -> u64 {
        ct::_private::TYPE_ID
    }

    fn service_id(&self) -> u64 {
        availability::CONTENT_SCHEMA_ID
    }

    fn name(&self) -> &'static str {
        "content"
    }

    fn category(&self) -> &'static str {
        "content"
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
                "content publish",
                "ContentService",
                "publish",
                "publish",
                0,
                true,
            )
            .with_row(matrix_row("publish")),            DomainCommand::new(
                "content publish-many",
                "ContentService",
                "publishMany",
                "publishMany",
                1,
                true,
            )
            .with_row(matrix_row("publishMany")),            DomainCommand::new(
                "content update",
                "ContentService",
                "update",
                "update",
                2,
                true,
            )
            .with_row(matrix_row("update")),            DomainCommand::new(
                "content unpublish",
                "ContentService",
                "unpublish",
                "unpublish",
                3,
                true,
            )
            .with_row(matrix_row("unpublish")),            DomainCommand::new(
                "content delete",
                "ContentService",
                "delete",
                "delete",
                4,
                true,
            )
            .with_row(matrix_row("delete")),            DomainCommand::new(
                "content trash",
                "ContentService",
                "trash",
                "trash",
                5,
                true,
            )
            .with_row(matrix_row("trash")),            DomainCommand::new(
                "content restore",
                "ContentService",
                "restore",
                "restore",
                6,
                true,
            )
            .with_row(matrix_row("restore")),            DomainCommand::new(
                "content empty-trash",
                "ContentService",
                "emptyTrash",
                "emptyTrash",
                7,
                true,
            )
            .with_row(matrix_row("emptyTrash")),            DomainCommand::new(
                "content query",
                "ContentService",
                "query",
                "query",
                8,
                true,
            )
            .with_row(matrix_row("query")),            DomainCommand::new(
                "content update",
                "ContentService",
                "get",
                "get",
                9,
                true,
            )
            .with_row(matrix_row("get")),            DomainCommand::new(
                "content subscribe",
                "ContentService",
                "subscribe",
                "subscribe",
                10,
                true,
            )
            .with_row(matrix_row("subscribe")),            DomainCommand::new(
                "content add-relationship",
                "ContentService",
                "addRelationship",
                "addRelationship",
                11,
                true,
            )
            .with_row(matrix_row("addRelationship")),            DomainCommand::new(
                "content remove-relationship",
                "ContentService",
                "removeRelationship",
                "removeRelationship",
                12,
                true,
            )
            .with_row(matrix_row("removeRelationship")),            DomainCommand::new(
                "content get-relationships",
                "ContentService",
                "getRelationships",
                "getRelationships",
                13,
                true,
            )
            .with_row(matrix_row("getRelationships")),            DomainCommand::new(
                "content semantic-search",
                "ContentService",
                "semanticSearch",
                "semanticSearch",
                14,
                true,
            )
            .with_row(matrix_row("semanticSearch")),            DomainCommand::new(
                "content get-content-graph",
                "ContentService",
                "getContentGraph",
                "getContentGraph",
                15,
                true,
            )
            .with_row(matrix_row("getContentGraph")),            DomainCommand::new(
                "content react-to-content",
                "ContentService",
                "reactToContent",
                "reactToContent",
                16,
                true,
            )
            .with_row(matrix_row("reactToContent")),            DomainCommand::new(
                "content get-reactions",
                "ContentService",
                "getReactions",
                "getReactions",
                17,
                true,
            )
            .with_row(matrix_row("getReactions")),            DomainCommand::new(
                "content add-provenance",
                "ContentService",
                "addProvenance",
                "addProvenance",
                18,
                true,
            )
            .with_row(matrix_row("addProvenance")),            DomainCommand::new(
                "content get-provenance",
                "ContentService",
                "getProvenance",
                "getProvenance",
                19,
                true,
            )
            .with_row(matrix_row("getProvenance")),            DomainCommand::new(
                "content erase-content",
                "ContentService",
                "eraseContent",
                "eraseContent",
                20,
                true,
            )
            .with_row(matrix_row("eraseContent")),            DomainCommand::new(
                "content get-undo-history",
                "ContentService",
                "getUndoHistory",
                "getUndoHistory",
                21,
                true,
            )
            .with_row(matrix_row("getUndoHistory")),            DomainCommand::new(
                "content undo-content",
                "ContentService",
                "undoContent",
                "undoContent",
                22,
                true,
            )
            .with_row(matrix_row("undoContent")),            DomainCommand::new(
                "content encrypt-content",
                "ContentService",
                "encryptContent",
                "encryptContent",
                23,
                true,
            )
            .with_row(matrix_row("encryptContent")),            DomainCommand::new(
                "content decrypt-content",
                "ContentService",
                "decryptContent",
                "decryptContent",
                24,
                true,
            )
            .with_row(matrix_row("decryptContent")),            DomainCommand::new(
                "content rotate-encryption-key",
                "ContentService",
                "rotateEncryptionKey",
                "rotateEncryptionKey",
                25,
                true,
            )
            .with_row(matrix_row("rotateEncryptionKey")),            DomainCommand::new(
                "content geotag-content",
                "ContentService",
                "geotagContent",
                "geotagContent",
                26,
                true,
            )
            .with_row(matrix_row("geotagContent")),            DomainCommand::new(
                "content get-by-geo-location",
                "ContentService",
                "getByGeoLocation",
                "getByGeoLocation",
                27,
                true,
            )
            .with_row(matrix_row("getByGeoLocation")),            DomainCommand::new(
                "content ai-tag-content",
                "ContentService",
                "aiTagContent",
                "aiTagContent",
                28,
                true,
            )
            .with_row(matrix_row("aiTagContent")),            DomainCommand::new(
                "content get-accessibility-report",
                "ContentService",
                "getAccessibilityReport",
                "getAccessibilityReport",
                29,
                true,
            )
            .with_row(matrix_row("getAccessibilityReport")),            DomainCommand::new(
                "content batch-publish",
                "ContentService",
                "batchPublish",
                "batchPublish",
                30,
                true,
            )
            .with_row(matrix_row("batchPublish")),            DomainCommand::new(
                "content create-branch",
                "ContentService",
                "createBranch",
                "createBranch",
                31,
                true,
            )
            .with_row(matrix_row("createBranch")),            DomainCommand::new(
                "content list-branches",
                "ContentService",
                "listBranches",
                "listBranches",
                32,
                true,
            )
            .with_row(matrix_row("listBranches")),            DomainCommand::new(
                "content merge-branch",
                "ContentService",
                "mergeBranch",
                "mergeBranch",
                33,
                true,
            )
            .with_row(matrix_row("mergeBranch")),            DomainCommand::new(
                "content create-composite",
                "ContentService",
                "createComposite",
                "createComposite",
                34,
                true,
            )
            .with_row(matrix_row("createComposite")),            DomainCommand::new(
                "content get-composite",
                "ContentService",
                "getComposite",
                "getComposite",
                35,
                true,
            )
            .with_row(matrix_row("getComposite")),            DomainCommand::new(
                "content update-composite",
                "ContentService",
                "updateComposite",
                "updateComposite",
                36,
                true,
            )
            .with_row(matrix_row("updateComposite")),            DomainCommand::new(
                "content add-lineage",
                "ContentService",
                "addLineage",
                "addLineage",
                37,
                true,
            )
            .with_row(matrix_row("addLineage")),            DomainCommand::new(
                "content get-lineage",
                "ContentService",
                "getLineage",
                "getLineage",
                38,
                true,
            )
            .with_row(matrix_row("getLineage")),            DomainCommand::new(
                "content get-audit-trail",
                "ContentService",
                "getAuditTrail",
                "getAuditTrail",
                39,
                true,
            )
            .with_row(matrix_row("getAuditTrail")),            DomainCommand::new(
                "content set-rate-limit",
                "ContentService",
                "setRateLimit",
                "setRateLimit",
                40,
                true,
            )
            .with_row(matrix_row("setRateLimit")),            DomainCommand::new(
                "content get-rate-limit",
                "ContentService",
                "getRateLimit",
                "getRateLimit",
                41,
                true,
            )
            .with_row(matrix_row("getRateLimit")),            DomainCommand::new(
                "content generate-preview",
                "ContentService",
                "generatePreview",
                "generatePreview",
                42,
                true,
            )
            .with_row(matrix_row("generatePreview")),            DomainCommand::new(
                "content get-preview",
                "ContentService",
                "getPreview",
                "getPreview",
                43,
                true,
            )
            .with_row(matrix_row("getPreview")),            DomainCommand::new(
                "content get-dependencies",
                "ContentService",
                "getDependencies",
                "getDependencies",
                44,
                true,
            )
            .with_row(matrix_row("getDependencies")),            DomainCommand::new(
                "content add-dependency",
                "ContentService",
                "addDependency",
                "addDependency",
                45,
                true,
            )
            .with_row(matrix_row("addDependency")),            DomainCommand::new(
                "content remove-dependency",
                "ContentService",
                "removeDependency",
                "removeDependency",
                46,
                true,
            )
            .with_row(matrix_row("removeDependency")),            DomainCommand::new(
                "content schedule-expiration",
                "ContentService",
                "scheduleExpiration",
                "scheduleExpiration",
                47,
                true,
            )
            .with_row(matrix_row("scheduleExpiration")),            DomainCommand::new(
                "content cancel-expiration",
                "ContentService",
                "cancelExpiration",
                "cancelExpiration",
                48,
                true,
            )
            .with_row(matrix_row("cancelExpiration")),            DomainCommand::new(
                "content get-replication-status",
                "ContentService",
                "getReplicationStatus",
                "getReplicationStatus",
                49,
                true,
            )
            .with_row(matrix_row("getReplicationStatus")),            DomainCommand::new(
                "content get-quality-score",
                "ContentService",
                "getQualityScore",
                "getQualityScore",
                50,
                true,
            )
            .with_row(matrix_row("getQualityScore")),            DomainCommand::new(
                "content get-recommendations",
                "ContentService",
                "getRecommendations",
                "getRecommendations",
                51,
                true,
            )
            .with_row(matrix_row("getRecommendations")),            DomainCommand::new(
                "content add-translation",
                "ContentService",
                "addTranslation",
                "addTranslation",
                52,
                true,
            )
            .with_row(matrix_row("addTranslation")),            DomainCommand::new(
                "content get-translations",
                "ContentService",
                "getTranslations",
                "getTranslations",
                53,
                true,
            )
            .with_row(matrix_row("getTranslations")),            DomainCommand::new(
                "content get-translated-content",
                "ContentService",
                "getTranslatedContent",
                "getTranslatedContent",
                54,
                true,
            )
            .with_row(matrix_row("getTranslatedContent")),            DomainCommand::new(
                "content pull",
                "ContentService",
                "pull",
                "pull",
                55,
                true,
            )
            .with_row(matrix_row("pull")),            DomainCommand::new(
                "content cancel",
                "ContentService",
                "cancel",
                "cancel",
                56,
                true,
            )
            .with_row(matrix_row("cancel")),        ]
    }

    fn schema_tree(&self) -> Option<&'static [u8]> {
        Some(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/schemas/content.schema.bin"
        )))
    }

    fn build(&self, class: CapabilityClass) -> Box<dyn ClientHook> {
        let server = ContentService::new(Arc::clone(&self.backend), class);
        let client: ct::Client = capnp_rpc::new_client(server);
        client.into_client_hook()
    }
}
