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
    fn publish(
        self: Rc<Self>,
        _params: ct::PublishParams,
        _results: ct::PublishResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("publish") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn publish_many(
        self: Rc<Self>,
        _params: ct::PublishManyParams,
        _results: ct::PublishManyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("publishMany") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn update(
        self: Rc<Self>,
        _params: ct::UpdateParams,
        _results: ct::UpdateResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("update") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn unpublish(
        self: Rc<Self>,
        _params: ct::UnpublishParams,
        _results: ct::UnpublishResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("unpublish") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn delete(
        self: Rc<Self>,
        _params: ct::DeleteParams,
        _results: ct::DeleteResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("delete") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn trash(
        self: Rc<Self>,
        _params: ct::TrashParams,
        _results: ct::TrashResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("trash") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn restore(
        self: Rc<Self>,
        _params: ct::RestoreParams,
        _results: ct::RestoreResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("restore") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn empty_trash(
        self: Rc<Self>,
        _params: ct::EmptyTrashParams,
        _results: ct::EmptyTrashResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("emptyTrash") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn query(
        self: Rc<Self>,
        _params: ct::QueryParams,
        _results: ct::QueryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("query") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get(
        self: Rc<Self>,
        _params: ct::GetParams,
        _results: ct::GetResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("get") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn subscribe(
        self: Rc<Self>,
        _params: ct::SubscribeParams,
        _results: ct::SubscribeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("subscribe") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn add_relationship(
        self: Rc<Self>,
        _params: ct::AddRelationshipParams,
        _results: ct::AddRelationshipResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("addRelationship") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn remove_relationship(
        self: Rc<Self>,
        _params: ct::RemoveRelationshipParams,
        _results: ct::RemoveRelationshipResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("removeRelationship") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_relationships(
        self: Rc<Self>,
        _params: ct::GetRelationshipsParams,
        _results: ct::GetRelationshipsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getRelationships") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn semantic_search(
        self: Rc<Self>,
        _params: ct::SemanticSearchParams,
        _results: ct::SemanticSearchResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("semanticSearch") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_content_graph(
        self: Rc<Self>,
        _params: ct::GetContentGraphParams,
        _results: ct::GetContentGraphResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getContentGraph") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn react_to_content(
        self: Rc<Self>,
        _params: ct::ReactToContentParams,
        _results: ct::ReactToContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("reactToContent") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_reactions(
        self: Rc<Self>,
        _params: ct::GetReactionsParams,
        _results: ct::GetReactionsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getReactions") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn add_provenance(
        self: Rc<Self>,
        _params: ct::AddProvenanceParams,
        _results: ct::AddProvenanceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("addProvenance") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_provenance(
        self: Rc<Self>,
        _params: ct::GetProvenanceParams,
        _results: ct::GetProvenanceResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getProvenance") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn erase_content(
        self: Rc<Self>,
        _params: ct::EraseContentParams,
        _results: ct::EraseContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("eraseContent") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_undo_history(
        self: Rc<Self>,
        _params: ct::GetUndoHistoryParams,
        _results: ct::GetUndoHistoryResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getUndoHistory") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn undo_content(
        self: Rc<Self>,
        _params: ct::UndoContentParams,
        _results: ct::UndoContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("undoContent") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn encrypt_content(
        self: Rc<Self>,
        _params: ct::EncryptContentParams,
        _results: ct::EncryptContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("encryptContent") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn decrypt_content(
        self: Rc<Self>,
        _params: ct::DecryptContentParams,
        _results: ct::DecryptContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("decryptContent") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn rotate_encryption_key(
        self: Rc<Self>,
        _params: ct::RotateEncryptionKeyParams,
        _results: ct::RotateEncryptionKeyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("rotateEncryptionKey") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn geotag_content(
        self: Rc<Self>,
        _params: ct::GeotagContentParams,
        _results: ct::GeotagContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("geotagContent") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_by_geo_location(
        self: Rc<Self>,
        _params: ct::GetByGeoLocationParams,
        _results: ct::GetByGeoLocationResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getByGeoLocation") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn ai_tag_content(
        self: Rc<Self>,
        _params: ct::AiTagContentParams,
        _results: ct::AiTagContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("aiTagContent") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_accessibility_report(
        self: Rc<Self>,
        _params: ct::GetAccessibilityReportParams,
        _results: ct::GetAccessibilityReportResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getAccessibilityReport") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn batch_publish(
        self: Rc<Self>,
        _params: ct::BatchPublishParams,
        _results: ct::BatchPublishResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("batchPublish") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn create_branch(
        self: Rc<Self>,
        _params: ct::CreateBranchParams,
        _results: ct::CreateBranchResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("createBranch") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn list_branches(
        self: Rc<Self>,
        _params: ct::ListBranchesParams,
        _results: ct::ListBranchesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("listBranches") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn merge_branch(
        self: Rc<Self>,
        _params: ct::MergeBranchParams,
        _results: ct::MergeBranchResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("mergeBranch") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn create_composite(
        self: Rc<Self>,
        _params: ct::CreateCompositeParams,
        _results: ct::CreateCompositeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("createComposite") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_composite(
        self: Rc<Self>,
        _params: ct::GetCompositeParams,
        _results: ct::GetCompositeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getComposite") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn update_composite(
        self: Rc<Self>,
        _params: ct::UpdateCompositeParams,
        _results: ct::UpdateCompositeResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("updateComposite") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn add_lineage(
        self: Rc<Self>,
        _params: ct::AddLineageParams,
        _results: ct::AddLineageResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("addLineage") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_lineage(
        self: Rc<Self>,
        _params: ct::GetLineageParams,
        _results: ct::GetLineageResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getLineage") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_audit_trail(
        self: Rc<Self>,
        _params: ct::GetAuditTrailParams,
        _results: ct::GetAuditTrailResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getAuditTrail") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn set_rate_limit(
        self: Rc<Self>,
        _params: ct::SetRateLimitParams,
        _results: ct::SetRateLimitResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("setRateLimit") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_rate_limit(
        self: Rc<Self>,
        _params: ct::GetRateLimitParams,
        _results: ct::GetRateLimitResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getRateLimit") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn generate_preview(
        self: Rc<Self>,
        _params: ct::GeneratePreviewParams,
        _results: ct::GeneratePreviewResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("generatePreview") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_preview(
        self: Rc<Self>,
        _params: ct::GetPreviewParams,
        _results: ct::GetPreviewResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getPreview") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_dependencies(
        self: Rc<Self>,
        _params: ct::GetDependenciesParams,
        _results: ct::GetDependenciesResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getDependencies") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn add_dependency(
        self: Rc<Self>,
        _params: ct::AddDependencyParams,
        _results: ct::AddDependencyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("addDependency") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn remove_dependency(
        self: Rc<Self>,
        _params: ct::RemoveDependencyParams,
        _results: ct::RemoveDependencyResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("removeDependency") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn schedule_expiration(
        self: Rc<Self>,
        _params: ct::ScheduleExpirationParams,
        _results: ct::ScheduleExpirationResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("scheduleExpiration") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn cancel_expiration(
        self: Rc<Self>,
        _params: ct::CancelExpirationParams,
        _results: ct::CancelExpirationResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("cancelExpiration") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_replication_status(
        self: Rc<Self>,
        _params: ct::GetReplicationStatusParams,
        _results: ct::GetReplicationStatusResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getReplicationStatus") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_quality_score(
        self: Rc<Self>,
        _params: ct::GetQualityScoreParams,
        _results: ct::GetQualityScoreResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getQualityScore") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_recommendations(
        self: Rc<Self>,
        _params: ct::GetRecommendationsParams,
        _results: ct::GetRecommendationsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getRecommendations") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn add_translation(
        self: Rc<Self>,
        _params: ct::AddTranslationParams,
        _results: ct::AddTranslationResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("addTranslation") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_translations(
        self: Rc<Self>,
        _params: ct::GetTranslationsParams,
        _results: ct::GetTranslationsResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getTranslations") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }

    fn get_translated_content(
        self: Rc<Self>,
        _params: ct::GetTranslatedContentParams,
        _results: ct::GetTranslatedContentResults,
    ) -> impl core::future::Future<Output = Result<(), capnp::Error>> {
        core::future::ready(match unavailable("getTranslatedContent") {
            Some(e) => Err(e),
            None => Ok(()),
        })
    }
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
