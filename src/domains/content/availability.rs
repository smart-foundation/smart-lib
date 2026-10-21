//! The **content** availability matrix: what this platform really serves.
//!
//! The store this domain keeps is the engine's own memory — the contract
//! defines content as the engine's own corpus, and this host publishes no
//! content corpus of its own — so the store's verbs are real and the rest of
//! the contract's surface (the graph, the ledger, the keyring, the
//! translations, the change feed) is not.

/// The canonical schema id of `content.capnp` (`@0xa1b2c3d4e5f60055`).
pub const CONTENT_SCHEMA_ID: u64 = 11651590505119481941;

/// Why a method is not served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonClass {
    /// The platform has no surface for this method at all.
    NoBackend,
    /// The surface exists but this uid cannot reach it.
    NoPrivilege,
    /// The method does not apply to this platform.
    NotApplicable,
    /// A policy turned it off.
    DisabledByPolicy,
    /// The reason is not one of the named ones.
    Unknown,
}

impl ReasonClass {
    /// The reason class as the contract's spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            ReasonClass::NoBackend => "no-backend",
            ReasonClass::NoPrivilege => "no-privilege",
            ReasonClass::NotApplicable => "not-applicable",
            ReasonClass::DisabledByPolicy => "disabled-by-policy",
            ReasonClass::Unknown => "unknown",
        }
    }
}

/// How well a method is served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvailStatus {
    /// Served, completely, by this platform's own mechanism.
    Real,
    /// Served, but with a stated limit.
    Degraded,
    /// Not served; the reason says why.
    Unavailable,
}

/// One row of the matrix.
#[derive(Clone, Copy, Debug)]
pub struct MethodAvailability {
    /// The contract's method name.
    pub name: &'static str,
    /// The method's ordinal in the contract.
    pub ordinal: u16,
    /// How well it is served.
    pub status: AvailStatus,
    /// Why it is not `Real` (empty when it is).
    pub reason_class: Option<ReasonClass>,
    /// The stated limit (empty when there is none).
    pub reason: &'static str,
    /// The `L3` test that proves the row.
    pub l3_test: Option<&'static str>,
}

impl MethodAvailability {
    /// A method this platform really serves.
    const fn real(name: &'static str, ordinal: u16, l3: &'static str) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Real,
            reason_class: None,
            reason: "",
            l3_test: Some(l3),
        }
    }

    /// A method this platform does not serve, and why.
    const fn unavailable(
        name: &'static str,
        ordinal: u16,
        class: ReasonClass,
        reason: &'static str,
    ) -> Self {
        MethodAvailability {
            name,
            ordinal,
            status: AvailStatus::Unavailable,
            reason_class: Some(class),
            reason,
            l3_test: None,
        }
    }
}

/// The matrix: one row per method `content.capnp`'s `ContentService` declares
/// (and the subscription's own `pull`/`cancel`), in ordinal order.
pub const CONTENT_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real(
        "publish",
        0,
        "l3_linux_content::a_published_item_is_really_stored",
    ),
    MethodAvailability::real(
        "publishMany",
        1,
        "l3_linux_content::a_batch_publish_answers_with_every_id",
    ),
    MethodAvailability::real(
        "update",
        2,
        "l3_linux_content::an_update_is_checked_against_the_version",
    ),
    MethodAvailability::real(
        "unpublish",
        3,
        "l3_linux_content::an_unpublished_item_is_no_longer_published",
    ),
    MethodAvailability::real(
        "delete",
        4,
        "l3_linux_content::a_deleted_item_is_gone",
    ),
    MethodAvailability::real(
        "trash",
        5,
        "l3_linux_content::trash_moves_the_item_into_the_trash",
    ),
    MethodAvailability::real(
        "restore",
        6,
        "l3_linux_content::restore_takes_the_item_out_of_the_trash",
    ),
    MethodAvailability::real(
        "emptyTrash",
        7,
        "l3_linux_content::empty_trash_empties_the_trash_and_says_how_many",
    ),
    MethodAvailability::real(
        "query",
        8,
        "l3_linux_content::a_query_answers_from_the_store",
    ),
    MethodAvailability::real(
        "get",
        9,
        "l3_linux_content::get_reads_the_stored_item_back",
    ),
    MethodAvailability::unavailable(
        "subscribe",
        10,
        ReasonClass::NoBackend,
        "no change feed: the store keeps no subscriber list and runs no publisher-side stream, so there is nothing to subscribe to",
    ),
    MethodAvailability::unavailable(
        "addRelationship",
        11,
        ReasonClass::NoBackend,
        "no relationship model: the store holds documents, not the graph between them",
    ),
    MethodAvailability::unavailable(
        "removeRelationship",
        12,
        ReasonClass::NoBackend,
        "no relationship model: the store holds documents, not the graph between them",
    ),
    MethodAvailability::unavailable(
        "getRelationships",
        13,
        ReasonClass::NoBackend,
        "no relationship model: the store holds documents, not the graph between them",
    ),
    MethodAvailability::unavailable(
        "semanticSearch",
        14,
        ReasonClass::NoBackend,
        "no embedding model: the store keeps no vectors, and this host publishes none for it",
    ),
    MethodAvailability::unavailable(
        "getContentGraph",
        15,
        ReasonClass::NoBackend,
        "no content graph: the same limit as @addRelationship",
    ),
    MethodAvailability::unavailable(
        "reactToContent",
        16,
        ReasonClass::NoBackend,
        "no reaction model: the store keeps no reactions",
    ),
    MethodAvailability::unavailable(
        "getReactions",
        17,
        ReasonClass::NoBackend,
        "no reaction model: the store keeps no reactions",
    ),
    MethodAvailability::unavailable(
        "addProvenance",
        18,
        ReasonClass::NoBackend,
        "no provenance ledger: the store keeps no provenance records",
    ),
    MethodAvailability::unavailable(
        "getProvenance",
        19,
        ReasonClass::NoBackend,
        "no provenance ledger: the store keeps no provenance records",
    ),
    MethodAvailability::unavailable(
        "eraseContent",
        20,
        ReasonClass::NoBackend,
        "no erasure ledger: the store's delete is a removal, not the auditable erasure this verb names",
    ),
    MethodAvailability::unavailable(
        "getUndoHistory",
        21,
        ReasonClass::NoBackend,
        "no edit history: the store keeps the current version, not the ones before it",
    ),
    MethodAvailability::unavailable(
        "undoContent",
        22,
        ReasonClass::NoBackend,
        "no edit history: the store keeps the current version, not the ones before it",
    ),
    MethodAvailability::unavailable(
        "encryptContent",
        23,
        ReasonClass::NoBackend,
        "no key store: the contract's key id names a keyring this host does not publish to the engine",
    ),
    MethodAvailability::unavailable(
        "decryptContent",
        24,
        ReasonClass::NoBackend,
        "no key store: the contract's key id names a keyring this host does not publish to the engine",
    ),
    MethodAvailability::unavailable(
        "rotateEncryptionKey",
        25,
        ReasonClass::NoBackend,
        "no key store: the contract's key id names a keyring this host does not publish to the engine",
    ),
    MethodAvailability::unavailable(
        "geotagContent",
        26,
        ReasonClass::NoBackend,
        "no geo index: the store holds no coordinates",
    ),
    MethodAvailability::unavailable(
        "getByGeoLocation",
        27,
        ReasonClass::NoBackend,
        "no geo index: the store holds no coordinates",
    ),
    MethodAvailability::unavailable(
        "aiTagContent",
        28,
        ReasonClass::NoBackend,
        "no tagging model: the store's tags are the ones the caller set",
    ),
    MethodAvailability::unavailable(
        "getAccessibilityReport",
        29,
        ReasonClass::NoBackend,
        "no accessibility analysis: the store holds the payload, not a rendering of it",
    ),
    MethodAvailability::unavailable(
        "batchPublish",
        30,
        ReasonClass::NoBackend,
        "the same store as @publishMany, but this verb also carries a provenance record the store does not model",
    ),
    MethodAvailability::unavailable(
        "createBranch",
        31,
        ReasonClass::NoBackend,
        "no branch model in the store",
    ),
    MethodAvailability::unavailable(
        "listBranches",
        32,
        ReasonClass::NoBackend,
        "no branch model in the store",
    ),
    MethodAvailability::unavailable(
        "mergeBranch",
        33,
        ReasonClass::NoBackend,
        "no branch model in the store",
    ),
    MethodAvailability::unavailable(
        "createComposite",
        34,
        ReasonClass::NoBackend,
        "no composite model in the store",
    ),
    MethodAvailability::unavailable(
        "getComposite",
        35,
        ReasonClass::NoBackend,
        "no composite model in the store",
    ),
    MethodAvailability::unavailable(
        "updateComposite",
        36,
        ReasonClass::NoBackend,
        "no composite model in the store",
    ),
    MethodAvailability::unavailable(
        "addLineage",
        37,
        ReasonClass::NoBackend,
        "no lineage ledger in the store",
    ),
    MethodAvailability::unavailable(
        "getLineage",
        38,
        ReasonClass::NoBackend,
        "no lineage ledger in the store",
    ),
    MethodAvailability::unavailable(
        "getAuditTrail",
        39,
        ReasonClass::NoBackend,
        "no audit trail: the store keeps the current version, not the writes that produced it",
    ),
    MethodAvailability::unavailable(
        "setRateLimit",
        40,
        ReasonClass::NoBackend,
        "no rate limiter in the store",
    ),
    MethodAvailability::unavailable(
        "getRateLimit",
        41,
        ReasonClass::NoBackend,
        "no rate limiter in the store",
    ),
    MethodAvailability::unavailable(
        "generatePreview",
        42,
        ReasonClass::NoBackend,
        "no preview model: the store holds the payload, not a rendering of it",
    ),
    MethodAvailability::unavailable(
        "getPreview",
        43,
        ReasonClass::NoBackend,
        "no preview model: the store holds the payload, not a rendering of it",
    ),
    MethodAvailability::unavailable(
        "getDependencies",
        44,
        ReasonClass::NoBackend,
        "no dependency graph in the store",
    ),
    MethodAvailability::unavailable(
        "addDependency",
        45,
        ReasonClass::NoBackend,
        "no dependency graph in the store",
    ),
    MethodAvailability::unavailable(
        "removeDependency",
        46,
        ReasonClass::NoBackend,
        "no dependency graph in the store",
    ),
    MethodAvailability::unavailable(
        "scheduleExpiration",
        47,
        ReasonClass::NoBackend,
        "no expiration scheduler: the store has no clock that runs on its own",
    ),
    MethodAvailability::unavailable(
        "cancelExpiration",
        48,
        ReasonClass::NoBackend,
        "no expiration scheduler: the store has no clock that runs on its own",
    ),
    MethodAvailability::unavailable(
        "getReplicationStatus",
        49,
        ReasonClass::NoBackend,
        "no replication: the store has no peer",
    ),
    MethodAvailability::unavailable(
        "getQualityScore",
        50,
        ReasonClass::NoBackend,
        "no scoring model: a quality score would be a number this engine cannot measure",
    ),
    MethodAvailability::unavailable(
        "getRecommendations",
        51,
        ReasonClass::NoBackend,
        "no recommendation model: this engine has no ranker over the corpus",
    ),
    MethodAvailability::unavailable(
        "addTranslation",
        52,
        ReasonClass::NoBackend,
        "no translation model, and no translation engine on this host",
    ),
    MethodAvailability::unavailable(
        "getTranslations",
        53,
        ReasonClass::NoBackend,
        "no translation model, and no translation engine on this host",
    ),
    MethodAvailability::unavailable(
        "getTranslatedContent",
        54,
        ReasonClass::NoBackend,
        "no translation model, and no translation engine on this host",
    ),
    MethodAvailability::unavailable(
        "pull",
        55,
        ReasonClass::NoBackend,
        "no change feed to pull from — the same limit as @subscribe",
    ),
    MethodAvailability::unavailable(
        "cancel",
        56,
        ReasonClass::NoBackend,
        "no change feed to cancel — the same limit as @subscribe",
    ),
];

/// The platform this matrix describes.
pub const PLATFORM: &str = "linux";

/// The transports the domain is served over.
pub const TRANSPORTS: &[&str] = &["InProcess"];

/// The contract caps this domain publishes.
pub const CAPS: &[&str] = &[];

/// How many rows are `Real`, `Degraded` and `Unavailable`.
pub fn status_counts() -> (usize, usize, usize) {
    let mut r = 0;
    let mut d = 0;
    let mut u = 0;
    for row in CONTENT_AVAILABILITY {
        match row.status {
            AvailStatus::Real => r += 1,
            AvailStatus::Degraded => d += 1,
            AvailStatus::Unavailable => u += 1,
        }
    }
    (r, d, u)
}

/// Every `Real`/`Degraded` row's backing test, paired with the method it
/// proves.
pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> {
    CONTENT_AVAILABILITY
        .iter()
        .filter(|r| r.l3_test.is_some())
        .map(|r| (r.name, r.l3_test.unwrap()))
}

/// The row for a method name.
pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    CONTENT_AVAILABILITY.iter().find(|r| r.name == name)
}
