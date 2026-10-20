use alloc::vec::Vec;

use crate::core::{Error, Result};

use super::types::{ContentFilter, ContentItem, Page, Pagination};

pub trait ContentBackend {
    fn publish(&self, _item: &ContentItem) -> Result<u64> {
        Err(Error::not_supported(
            "no-backend: content publish is not served on this backend",
        ))
    }
    fn publish_many(&self, _items: &[ContentItem]) -> Result<Vec<u64>> {
        Err(Error::not_supported(
            "no-backend: content publishMany is not served on this backend",
        ))
    }
    fn update(&self, _content_id: u64, _item: &ContentItem, _expected_version: u64) -> Result<u64> {
        Err(Error::not_supported(
            "no-backend: content update is not served on this backend",
        ))
    }
    fn unpublish(&self, _content_id: u64) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: content unpublish is not served on this backend",
        ))
    }
    fn delete(&self, _content_id: u64) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: content delete is not served on this backend",
        ))
    }
    fn trash(&self, _content_id: u64) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: content trash is not served on this backend",
        ))
    }
    fn restore(&self, _content_id: u64) -> Result<()> {
        Err(Error::not_supported(
            "no-backend: content restore is not served on this backend",
        ))
    }
    fn empty_trash(&self) -> Result<u32> {
        Err(Error::not_supported(
            "no-backend: content emptyTrash is not served on this backend",
        ))
    }
    fn query(&self, _filter: &ContentFilter, _page: Pagination) -> Result<Page<ContentItem>> {
        Err(Error::not_supported(
            "no-backend: content query is not served on this backend",
        ))
    }
    fn get(&self, _content_id: u64) -> Result<ContentItem> {
        Err(Error::not_supported(
            "no-backend: content get is not served on this backend",
        ))
    }
    fn subscribe(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content subscribe is not served on this backend"))
    }
    fn add_relationship(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content addRelationship is not served on this backend"))
    }
    fn remove_relationship(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content removeRelationship is not served on this backend"))
    }
    fn get_relationships(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getRelationships is not served on this backend"))
    }
    fn semantic_search(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content semanticSearch is not served on this backend"))
    }
    fn get_content_graph(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getContentGraph is not served on this backend"))
    }
    fn react_to_content(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content reactToContent is not served on this backend"))
    }
    fn get_reactions(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getReactions is not served on this backend"))
    }
    fn add_provenance(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content addProvenance is not served on this backend"))
    }
    fn get_provenance(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getProvenance is not served on this backend"))
    }
    fn erase_content(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content eraseContent is not served on this backend"))
    }
    fn get_undo_history(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getUndoHistory is not served on this backend"))
    }
    fn undo_content(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content undoContent is not served on this backend"))
    }
    fn rotate_key(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content rotateKey is not served on this backend"))
    }
    fn encrypt_content(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content encryptContent is not served on this backend"))
    }
    fn decrypt_content(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content decryptContent is not served on this backend"))
    }
    fn rotate_encryption_key(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content rotateEncryptionKey is not served on this backend"))
    }
    fn geotag_content(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content geotagContent is not served on this backend"))
    }
    fn get_by_geo_location(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getByGeoLocation is not served on this backend"))
    }
    fn ai_tag_content(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content aiTagContent is not served on this backend"))
    }
    fn get_accessibility_report(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getAccessibilityReport is not served on this backend"))
    }
    fn batch_publish(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content batchPublish is not served on this backend"))
    }
    fn create_branch(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content createBranch is not served on this backend"))
    }
    fn list_branches(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content listBranches is not served on this backend"))
    }
    fn merge_branch(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content mergeBranch is not served on this backend"))
    }
    fn create_composite(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content createComposite is not served on this backend"))
    }
    fn get_composite(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getComposite is not served on this backend"))
    }
    fn update_composite(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content updateComposite is not served on this backend"))
    }
    fn add_lineage(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content addLineage is not served on this backend"))
    }
    fn get_lineage(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getLineage is not served on this backend"))
    }
    fn get_audit_trail(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getAuditTrail is not served on this backend"))
    }
    fn set_rate_limit(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content setRateLimit is not served on this backend"))
    }
    fn get_rate_limit(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getRateLimit is not served on this backend"))
    }
    fn generate_preview(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content generatePreview is not served on this backend"))
    }
    fn get_preview(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getPreview is not served on this backend"))
    }
    fn get_dependencies(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getDependencies is not served on this backend"))
    }
    fn add_dependency(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content addDependency is not served on this backend"))
    }
    fn remove_dependency(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content removeDependency is not served on this backend"))
    }
    fn schedule_expiration(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content scheduleExpiration is not served on this backend"))
    }
    fn cancel_expiration(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content cancelExpiration is not served on this backend"))
    }
    fn get_replication_status(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getReplicationStatus is not served on this backend"))
    }
    fn get_quality_score(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getQualityScore is not served on this backend"))
    }
    fn get_recommendations(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getRecommendations is not served on this backend"))
    }
    fn add_translation(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content addTranslation is not served on this backend"))
    }
    fn get_translations(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getTranslations is not served on this backend"))
    }
    fn get_translated_content(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content getTranslatedContent is not served on this backend"))
    }
    fn pull(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content pull is not served on this backend"))
    }
    fn cancel(&self) -> Result<()> {
        Err(Error::not_supported("no-backend: content cancel is not served on this backend"))
    }

    fn content_caps(&self) -> Vec<&'static str> { alloc::vec!["content.inproc"] }
}
