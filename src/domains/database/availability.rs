pub const DATABASE_SCHEMA_ID: u64 = 11651590505119481901;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReasonClass { NoBackend, NoPrivilege, NotApplicable, DisabledByPolicy, Unknown }
impl ReasonClass { pub const fn as_str(self) -> &'static str { match self { ReasonClass::NoBackend => "no-backend", ReasonClass::NoPrivilege => "no-privilege", ReasonClass::NotApplicable => "not-applicable", ReasonClass::DisabledByPolicy => "disabled-by-policy", ReasonClass::Unknown => "unknown" } } }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvailStatus { Real, Degraded, Unavailable }

#[derive(Clone, Copy, Debug)]
pub struct MethodAvailability { pub name: &'static str, pub ordinal: u16, pub status: AvailStatus, pub reason_class: Option<ReasonClass>, pub reason: &'static str, pub l3_test: Option<&'static str> }
impl MethodAvailability {
  const fn real(name: &'static str, ordinal: u16, l3: &'static str) -> Self { MethodAvailability { name, ordinal, status: AvailStatus::Real, reason_class: None, reason: "", l3_test: Some(l3) } }
  const fn degraded(name: &'static str, ordinal: u16, l3: &'static str, detail: &'static str) -> Self { MethodAvailability { name, ordinal, status: AvailStatus::Degraded, reason_class: None, reason: detail, l3_test: Some(l3) } }
  const fn unavailable(name: &'static str, ordinal: u16, class: ReasonClass, reason: &'static str) -> Self { MethodAvailability { name, ordinal, status: AvailStatus::Unavailable, reason_class: Some(class), reason, l3_test: None } }
}

pub const DATABASE_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real("get", 0, "l3_linux_database::get_answers_with_the_stored_bytes"),
    MethodAvailability::real("has", 1, "l3_linux_database::has_answers_with_the_stores_own_yes_or_no"),
    MethodAvailability::real("listKeys", 2, "l3_linux_database::list_keys_answers_with_the_namespaces_own_keys"),
    MethodAvailability::real("count", 3, "l3_linux_database::count_answers_with_the_namespaces_own_key_count"),
    MethodAvailability::real("set", 4, "l3_linux_database::set_stores_the_bytes_the_store_gives_back"),
    MethodAvailability::real("delete", 5, "l3_linux_database::delete_removes_the_key_and_the_store_says_so"),
    MethodAvailability::real("clear", 6, "l3_linux_database::clear_empties_the_namespace_and_only_the_namespace"),
    MethodAvailability::unavailable("setBatch", 7, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getBatch", 8, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("deleteBatch", 9, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::real("listNamespaces", 10, "l3_linux_database::list_namespaces_answers_with_the_stores_own_namespaces"),
    MethodAvailability::unavailable("deleteNamespace", 11, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("exportNamespace", 12, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("importNamespace", 13, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("subscribeChanges", 14, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("pullChanges", 15, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getTyped", 16, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("setTyped", 17, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("setWithTtl", 18, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getTtl", 19, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("setBatchWithTtl", 20, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getMetadata", 21, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getBatchMetadata", 22, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("beginTransaction", 23, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("commitTransaction", 24, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("rollbackTransaction", 25, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getTransactionStatus", 26, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("listKeysByTimeRange", 27, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getTimeSeries", 28, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("applySchema", 29, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getSchema", 30, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getQueryLanguages", 31, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getNamespace", 32, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("watchKey", 33, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("compactNamespace", 34, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getNamespaceStats", 35, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("backupNamespace", 36, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("restoreNamespace", 37, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getKeyHistory", 38, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("encryptKey", 39, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("decryptKey", 40, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("subscribeChangeStream", 41, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("encryptNamespace", 42, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getNamespaceEncryptionStatus", 43, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("subscribeAuditLog", 44, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getResourceMonitor", 45, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getConnectionPoolStatus", 46, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("query", 47, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("createIndex", 48, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("listIndexes", 49, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("addGraphEdge", 50, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("shortestPath", 51, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("detectCommunities", 52, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getNeighbors", 53, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("insertVector", 54, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("searchVector", 55, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("deleteVector", 56, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("setCrdt", 57, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getCrdt", 58, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("mergeCrdt", 59, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("startReplication", 60, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getReplicationStatus", 61, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("stopReplication", 62, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("appendEvent", 63, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("replayEvents", 64, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("createSnapshot", 65, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("readFromSnapshot", 66, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("deleteSnapshot", 67, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("configureSharding", 68, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("getShardInfo", 69, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("pull", 70, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("cancel", 71, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("pull", 72, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("cancel", 73, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("pull", 74, ReasonClass::NoBackend, "no native database surface for this method on this host"),
    MethodAvailability::unavailable("cancel", 75, ReasonClass::NoBackend, "no native database surface for this method on this host"),
];

pub const PLATFORM: &str = "linux";
pub const TRANSPORTS: &[&str] = &["InProcess"];
pub const CAPS: &[&str] = &[];

pub fn status_counts() -> (usize, usize, usize) { let mut r=0;let mut d=0;let mut u=0; for row in DATABASE_AVAILABILITY { match row.status { AvailStatus::Real=>r+=1, AvailStatus::Degraded=>d+=1, AvailStatus::Unavailable=>u+=1 } } (r,d,u) }
pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> { DATABASE_AVAILABILITY.iter().filter(|r| r.l3_test.is_some()).map(|r| (r.name, r.l3_test.unwrap())) }
pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> { DATABASE_AVAILABILITY.iter().find(|r| r.name == name) }
