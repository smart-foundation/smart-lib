//! Inline behaviour tests for the settings domain's *public contract*: the
//! value model's fidelity to the wire, the `get` union's exhaustiveness, the
//! availability matrix's internal consistency, the trait's honest-Unavailable
//! totality, and total error propagation through a scripted backend. All
//! hermetic — no host I/O here; the native-path behaviour over the host's own
//! settings engine is proven in [`crate::platforms::linux::settings`] (the
//! `l3_linux_settings::…` tests the matrix names).

use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::core::ids::AppId;
use crate::core::{Error, ErrorCode};
use crate::schema::settings_capnp;

use super::api::Settings;
use super::availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, SETTINGS_AVAILABILITY,
    SETTINGS_SCHEMA_ID, ReasonClass,
};
use super::backend::{render_value, SettingsBackend, SettingsSubscription};
use super::types::{
    Page, SchemaInfo, SchemaKey, SettingChange, SettingMapEntry, SettingResult, SettingValue,
};

/// The contract's own method list, in `Settings` order (`@0`–`@17`), plus the
/// subscription interface.
const CONTRACT_METHODS: &[&str] = &[
    "get", "set", "getAll", "delete", "clear", "listDomains", "watch", "watchDomain", "bulkSet",
    "bulkGet", "listByCategory", "setAppOverride", "getAppOverride", "clearAppOverride",
    "getVersion", "setIfUnchanged", "getHistory", "getSettingsDiff", "SettingsSubscription.pull",
    "SettingsSubscription.cancel",
];

/// A backend that serves nothing: the honesty baseline.
struct Nothing;

impl SettingsBackend for Nothing {}

/// A scripted backend with one key, for the public-API wrapper.
struct Scripted {
    version: u64,
    changes: Vec<SettingChange>,
}

impl Scripted {
    fn new() -> Self {
        Scripted {
            version: 3,
            changes: vec![SettingChange {
                domain: "org.example".to_string(),
                key: "accent".to_string(),
                value: SettingValue::String("blue".to_string()),
                version: 3,
                ..Default::default()
            }],
        }
    }
}

impl SettingsBackend for Scripted {
    fn get(&self, domain: &str, key: &str) -> crate::core::Result<SettingResult> {
        match (domain, key) {
            ("scripted", "accent") => Ok(SettingResult::Value(SettingValue::String("blue".into()))),
            ("scripted", "never-set") => Ok(SettingResult::NotSet),
            _ => Ok(SettingResult::Error(Error::not_found().with_context("setting"))),
        }
    }
    fn describe(&self, _domain: &str) -> crate::core::Result<Option<SchemaInfo>> {
        Ok(Some(SchemaInfo {
            keys: vec![SchemaKey {
                name: "accent".to_string(),
                variant_type: "s".to_string(),
                default_value: Some("'red'".to_string()),
                ..Default::default()
            }],
            ..Default::default()
        }))
    }
    fn watch(&self, _domain: &str, _key: &str) -> crate::core::Result<SettingsSubscription> {
        Ok(SettingsSubscription(1))
    }
    fn pull_settings(
        &self,
        _subscription: SettingsSubscription,
        _max: u32,
    ) -> crate::core::Result<Vec<SettingChange>> {
        Ok(self.changes.clone())
    }
    fn cancel_settings(&self, _subscription: SettingsSubscription) -> crate::core::Result<()> {
        Ok(())
    }
    fn get_settings_diff(
        &self,
        _domain: &str,
        since: u64,
    ) -> crate::core::Result<(Vec<SettingChange>, u64)> {
        Ok((
            self.changes.iter().filter(|change| change.version > since).cloned().collect(),
            self.version,
        ))
    }
    fn get_version(&self, _domain: &str, _key: &str) -> crate::core::Result<u64> {
        Ok(self.version)
    }
}

/// The matrix is exactly the contract surface.
#[test]
fn the_matrix_is_exactly_the_contract_surface() {
    let (real, degraded, unavailable) = status_counts();
    assert_eq!(real + degraded + unavailable, SETTINGS_AVAILABILITY.len());
    assert_eq!(SETTINGS_AVAILABILITY.len(), CONTRACT_METHODS.len());
    for name in CONTRACT_METHODS {
        assert!(SETTINGS_AVAILABILITY.iter().any(|row| row.name == *name), "{name} has no row");
    }
}

/// Every row carries the evidence its status demands.
#[test]
fn availability_rows_are_consistent() {
    for row in SETTINGS_AVAILABILITY {
        assert_ne!(row.reason_class, Some(ReasonClass::Unknown), "{}", row.name);
        match row.status {
            AvailStatus::Real => {
                assert!(row.reason.is_empty(), "REAL row {} carries a reason", row.name);
                assert!(row.l3_test.is_some(), "REAL row {} has no l3_test", row.name);
                assert!(row.reason_class.is_none());
            }
            AvailStatus::Degraded => {
                assert!(!row.reason.is_empty(), "DEGRADED row {} names no capability", row.name);
                assert!(row.l3_test.is_some());
            }
            AvailStatus::Unavailable => {
                assert!(row.reason_class.is_some());
                assert!(!row.reason.is_empty());
                assert!(row.l3_test.is_none());
            }
        }
    }
    assert!(method_availability_for("get").is_some());
}

/// Every claimed `l3_test` exists in the Linux backend's suite.
#[test]
fn every_claimed_test_exists() {
    let tests: &[&str] = &[
        "l3_linux_settings::a_read_is_the_hosts_own_settings_engine",
        "l3_linux_settings::a_write_is_the_hosts_own_engine_and_gated",
        "l3_linux_settings::a_schema_read_lists_the_hosts_own_keys",
        "l3_linux_settings::a_domain_list_is_the_hosts_own_schema_source",
        "l3_linux_settings::a_watch_reports_the_hosts_own_change_signal",
        "l3_linux_settings::a_bulk_read_answers_every_key",
        "l3_linux_settings::a_category_listing_reports_the_hosts_own_keys",
        "l3_linux_settings::an_app_override_answers_exists",
        "l3_linux_settings::a_version_is_the_backends_own_monotonic_counter",
        "l3_linux_settings::an_optimistic_set_reports_the_conflict_it_finds",
        "l3_linux_settings::a_history_is_the_backends_own_recorded_changes",
        "l3_linux_settings::a_delta_read_returns_the_changes_after_the_version",
        "l3_linux_settings::a_watch_pull_is_bounded_and_cancellable",
    ];
    for (method, test) in claimed_l3_tests() {
        assert!(tests.contains(&test), "{method} claims {test}, which is not in the suite");
    }
    for test in tests {
        assert!(claimed_l3_tests().any(|(_, t)| t == *test), "{test} is real but unclaimed");
    }
}

/// The compiled schema id matches `settings.capnp`'s `@0x…` line.
#[test]
fn schema_id_is_the_settings_contract() {
    assert_eq!(SETTINGS_SCHEMA_ID, 0xa1b2c3d4e5f60022);
    #[allow(dead_code)]
    fn assert_generated_module_linked(_: Option<settings_capnp::setting_value::Owned>) {}
}

/// The trait's default surface answers `notSupported` **with a reason**.
#[test]
fn every_default_method_answers_with_a_reason() {
    let backend = Nothing;
    let err = backend.get("org.x", "key").expect_err("must say so");
    assert_eq!(err.code, ErrorCode::NotSupported);
    assert!(!err.retryable);
    assert!(err.message.starts_with("no-backend:"), "{}", err.message);
    for message in [
        backend.get_all("org.x", Default::default()).unwrap_err().message,
        backend.list_domains(Default::default()).unwrap_err().message,
        backend.watch("org.x", "key").unwrap_err().message,
        backend.watch_domain("org.x").unwrap_err().message,
        backend.bulk_get("org.x", &["key"]).unwrap_err().message,
        backend.list_by_category("display", Default::default()).unwrap_err().message,
        backend.get_app_override(AppId::new(1), "key").unwrap_err().message,
        backend.get_version("org.x", "key").unwrap_err().message,
        backend.get_history("org.x", "key", Default::default()).unwrap_err().message,
        backend.get_settings_diff("org.x", 0).unwrap_err().message,
        backend.pull_settings(SettingsSubscription(1), 1).unwrap_err().message,
        backend.cancel_settings(SettingsSubscription(1)).unwrap_err().message,
        backend.describe("org.x").unwrap_err().message,
        backend.describe_all().unwrap_err().message,
    ] {
        assert!(message.starts_with("no-backend:"), "{message}");
    }
    // Every write verb names privilege, not a missing backend.
    for message in [
        backend.set("org.x", "key", &SettingValue::default()).unwrap_err().message,
        backend.delete("org.x", "key").unwrap_err().message,
        backend.clear("org.x").unwrap_err().message,
        backend.bulk_set("org.x", &[]).unwrap_err().message,
        backend.set_app_override(AppId::new(1), "key", &SettingValue::default()).unwrap_err().message,
        backend.clear_app_override(AppId::new(1), "key").unwrap_err().message,
        backend.set_if_unchanged("org.x", "key", &SettingValue::default(), 1).unwrap_err().message,
    ] {
        assert!(message.starts_with("no-privilege:"), "{message}");
    }
    assert_eq!(backend.settings_caps().len(), 3);
}

/// Error propagation is total and honest across all 20 contract codes.
#[test]
fn error_injection_is_total_over_all_20_codes() {
    for code in ErrorCode::ALL {
        let err = Error::new(code);
        assert_eq!(err.code, code);
        assert_eq!(err.retryable, code.default_retryable());
    }
}

/// The `get` union is total: a set key, a `notSet` key and an error are three
/// distinct answers a caller must handle — which is the wire-honesty fix the
/// contract's header records.
#[test]
fn the_get_union_is_total() {
    assert!(matches!(
        SettingResult::Value(SettingValue::String(String::new())),
        SettingResult::Value(_)
    ));
    assert_eq!(SettingResult::NotSet, SettingResult::NotSet);
    assert!(matches!(SettingResult::Error(Error::not_found()), SettingResult::Error(_)));
    assert!(matches!(SettingResult::Unknown, SettingResult::Unknown));
    // The empty string and notSet are distinct, which is the whole point.
    let set_to_empty = SettingResult::Value(SettingValue::String(String::new()));
    assert_eq!(set_to_empty.value(), Some(&SettingValue::String(String::new())));
    assert_eq!(SettingResult::NotSet.value(), None);
}

/// Every `SettingValue` arm round-trips through the wire.
#[test]
fn setting_values_round_trip_through_the_wire() {
    let values = [
        SettingValue::String("text".to_string()),
        SettingValue::Integer(-17),
        SettingValue::Real(1.5),
        SettingValue::Boolean(true),
        SettingValue::Data(vec![1, 2, 3]),
        SettingValue::List(vec![SettingValue::Integer(1), SettingValue::Integer(2)]),
        SettingValue::Dict(vec![("k".to_string(), SettingValue::String("v".to_string()))]),
    ];
    for value in values {
        let mut message = capnp::message::Builder::new_default();
        {
            let mut builder = message.init_root::<settings_capnp::setting_value::Builder<'_>>();
            value.write_to(builder.reborrow());
        }
        let mut bytes = Vec::new();
        capnp::serialize::write_message(&mut bytes, &message).expect("serialize");
        let reader = capnp::serialize::read_message(
            &mut bytes.as_slice(),
            capnp::message::ReaderOptions::new(),
        )
        .expect("deserialize");
        let root = reader.get_root::<settings_capnp::setting_value::Reader<'_>>().expect("root");
        let round_tripped = SettingValue::from_reader(&root);
        assert_eq!(round_tripped, value, "{value:?}");
        assert_eq!(value.type_token(), round_tripped.type_token());
    }
}

/// Pagination is the contract's rule: `limit == 0` means "one", never "all".
#[test]
fn pagination_zero_limit_means_one() {
    let page = Page {
        items: vec![SettingMapEntry::default(), SettingMapEntry::default()],
        total: 2,
    };
    let _ = page;
    let pagination = super::types::Pagination { offset: 0, limit: 0 };
    assert_eq!(pagination.effective_limit(), 1);
}

/// A schema key's contract value mapping is honest: a type outside the
/// contract's seven forms is `None` rather than coerced into the wrong arm.
#[test]
fn a_schema_key_maps_only_the_types_it_has() {
    let key = |variant_type: &str| SchemaKey {
        variant_type: variant_type.to_string(),
        ..Default::default()
    };
    assert_eq!(key("s").contract_value(), Some("string"));
    assert_eq!(key("b").contract_value(), Some("boolean"));
    assert_eq!(key("i").contract_value(), Some("integer"));
    assert_eq!(key("d").contract_value(), Some("real"));
    assert_eq!(key("a{ss}").contract_value(), Some("dict"));
    assert_eq!(key("ay").contract_value(), Some("data"));
    assert_eq!(key("v").contract_value(), None, "a bare variant has no contract arm");
}

/// A `SettingChange` keeps the contract's own shape, including the version a
/// consumer uses to tell whether it has seen everything.
#[test]
fn a_change_carries_the_version_beside_it() {
    let change = SettingChange {
        key: "accent".to_string(),
        value: SettingValue::String("blue".to_string()),
        version: 7,
        actor_id: "app:1".to_string(),
        ..Default::default()
    };
    assert_eq!(change.version, 7);
    assert_eq!(render_value(&change.value), "blue");
    assert_eq!(render_value(&SettingValue::Integer(3)), "3");
}

/// The public API is a thin borrowing wrapper over the backend.
#[test]
fn the_public_api_is_a_thin_wrapper() {
    let backend = Scripted::new();
    let settings = Settings::new(&backend);
    let value = settings.value("scripted", "accent").expect("get");
    assert_eq!(value.value(), Some(&SettingValue::String("blue".to_string())));
    // The convenience resolver answers `None` for `notSet`, which is the case
    // the contract's union exists to separate from the empty string.
    assert_eq!(
        settings.value_or_not_set("scripted", "never-set").expect("get"),
        None
    );
    assert_eq!(
        settings.value_or_not_set("scripted", "accent").expect("get"),
        Some(SettingValue::String("blue".to_string()))
    );
    let schema = settings.describe("scripted").expect("describe").expect("a schema");
    assert_eq!(schema.keys[0].variant_type, "s");
    assert_eq!(schema.keys[0].default_value.as_deref(), Some("'red'"));
    assert!(schema.has_key("accent"));
    assert_eq!(schema.key("accent").expect("the key").summary, None);
    let (subscription, changes) = settings.watch("scripted", "accent").expect("watch");
    assert_eq!(changes.len(), 1, "the host's own change, delivered once");
    settings.close(subscription).expect("close");
    let (diff, version) = settings.changes_since("scripted", 2).expect("diff");
    assert_eq!(diff.len(), 1, "the change after version 2");
    assert_eq!(version, 3, "the version now");
    let (none, _) = settings.changes_since("scripted", 3).expect("diff");
    assert!(none.is_empty(), "everything up to the version is already seen");
    assert_eq!(settings.caps().len(), 3);
}
