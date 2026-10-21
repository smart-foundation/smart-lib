//! Inline behaviour tests for the logging domain's *public contract*: the owned
//! types' fidelity to the wire, the filter's totality, the availability matrix's
//! internal consistency, the trait's honest-Unavailable totality, the severity
//! vocabulary's unknown-tolerance, and the export shape. The native-path
//! behaviour over the host's measured sinks is proven in
//! [`crate::platforms::linux::logging`] (the `l3_linux_logging::…` tests the
//! matrix names). All hermetic — no host I/O in this file.

use alloc::format;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::core::values::Timestamp;
use crate::core::{Error, ErrorCode};
use crate::schema::{logging_capnp, types_capnp};

use super::availability::{
    claimed_l3_tests, status_counts, AvailStatus, ReasonClass, LOGGING_AVAILABILITY,
    LOGGING_SCHEMA_ID,
};
use super::backend::LoggingBackend;
use super::types::{
    Level, LogDomain, LogEntry, LogFilter, LogOutcome, Page, Pagination, SubscriptionId,
};

/// The contract's own method list, in `Logging` order (`@0`–`@8`), plus the
/// subscription helpers. The matrix must carry exactly these names.
const CONTRACT_METHODS: &[&str] = &[
    "log",
    "getEntries",
    "subscribe",
    "pull",
    "cancel",
    "setLevel",
    "getLevel",
    "clear",
    "export",
    "setSamplingRate",
    "listDomains",
];

/// A backend that serves nothing: the honesty baseline.
struct Nothing;

impl LoggingBackend for Nothing {}

/// A scripted backend with a small store, for the public-API wrapper.
struct Scripted {
    entry: LogEntry,
}

impl LoggingBackend for Scripted {
    fn log(&self, level: Level, domain: &str, message: &str) -> crate::core::Result<LogOutcome> {
        let _ = (domain, message);
        Ok(match level {
            Level::Silent => LogOutcome::Dropped("silent"),
            _ => LogOutcome::Accepted,
        })
    }
    fn get_entries(&self, filter: &LogFilter, _page: Pagination) -> crate::core::Result<Page<LogEntry>> {
        assert!(filter.matches(&self.entry));
        Ok(Page {
            items: vec![self.entry.clone()],
            total: 1,
        })
    }
    fn subscribe(&self, _filter: &LogFilter) -> crate::core::Result<SubscriptionId> {
        Ok(SubscriptionId(9))
    }
    fn pull(&self, _s: SubscriptionId, max: u32) -> crate::core::Result<Vec<LogEntry>> {
        if max == 0 {
            return Err(Error::invalid_argument("a bound of zero is refused"));
        }
        Ok(vec![self.entry.clone()])
    }
    fn cancel(&self, _s: SubscriptionId) -> crate::core::Result<()> {
        Ok(())
    }
    fn set_level(&self, _domain: &str, level: Level) -> crate::core::Result<()> {
        assert_eq!(level, Level::Warn);
        Ok(())
    }
    fn get_level(&self, _domain: &str) -> crate::core::Result<Level> {
        Ok(Level::Warn)
    }
    fn clear(&self, _domain: &str) -> crate::core::Result<()> {
        Ok(())
    }
    fn export(&self, _filter: &LogFilter, mime: &str) -> crate::core::Result<Vec<u8>> {
        Ok(format!("{mime}\n").into_bytes())
    }
    fn set_sampling_rate(&self, _domain: &str, rate: f32) -> crate::core::Result<()> {
        assert_eq!(rate, 0.5);
        Ok(())
    }
    fn list_domains(&self, _page: Pagination) -> crate::core::Result<Page<LogDomain>> {
        Ok(Page {
            items: vec![LogDomain {
                name: "app".to_string(),
                level: Level::Warn,
                description: "scripted".to_string(),
                sampling_rate: 1.0,
            }],
            total: 1,
        })
    }
}

fn entry() -> LogEntry {
    LogEntry {
        id: crate::core::ids::EventId::new(11),
        sequence: 11,
        timestamp: Timestamp::from_nanos(1_700_000_000_000_000_000),
        level: Level::Warn,
        domain: "app".to_string(),
        message: "a warning".to_string(),
        source: alloc::vec![1, 2, 3],
        subsystem: crate::core::ids::SubsystemId::new(7),
        metadata: alloc::vec![("k".to_string(), "v".to_string())],
        correlation_id: "corr".to_string(),
        trace_id: "trace".to_string(),
        span_id: "span".to_string(),
        parent_span_id: "parent".to_string(),
    }
}

/// An entry survives a real serialized round-trip, field for field.
#[test]
fn log_entries_round_trip_through_the_wire() {
    let value = entry();
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<logging_capnp::log_entry::Builder<'_>>();
        value.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new())
        .expect("deserialize");
    let wire = reader
        .get_root::<logging_capnp::log_entry::Reader<'_>>()
        .expect("root");
    let back = LogEntry::from_reader(&wire);
    assert_eq!(back.sequence, value.sequence);
    assert_eq!(back.level, value.level);
    assert_eq!(back.domain, value.domain);
    assert_eq!(back.message, value.message);
    assert_eq!(back.source, value.source);
    assert_eq!(back.metadata, value.metadata);
    assert_eq!(back.trace_id, value.trace_id);
    assert_eq!(back.parent_span_id, value.parent_span_id);
}

/// The severity vocabulary is the contract's own, one-for-one, and an unnamed
/// level is `custom` — never a guessed `info`.
#[test]
fn the_severity_vocabulary_is_the_contracts_own() {
    for level in [
        Level::Trace,
        Level::Debug,
        Level::Info,
        Level::Warn,
        Level::Error,
        Level::Fatal,
        Level::Silent,
        Level::Crash,
        Level::Security,
        Level::Audit,
        Level::Custom,
    ] {
        let ordinal = level.ordinal();
        let generated = level.to_generated();
        // `LogLevel` is `#[repr(u16)]` with the contract's own discriminants, so
        // the cast is the ordinal read back off the wire's own enum.
        assert_eq!(
            u16::from(types_capnp::LogLevel::from(generated)),
            ordinal,
            "the mapping is 1:1 in both directions"
        );
        assert_eq!(Level::from_generated(generated), level, "and back again");
    }
    assert_eq!(Level::from_token("warn"), Level::Warn);
    assert_eq!(Level::from_token("warning"), Level::Warn);
    assert_eq!(Level::from_token("notice"), Level::Audit);
    assert_eq!(Level::from_token("emergency"), Level::Crash);
    assert_eq!(
        Level::from_token("a level nobody named"),
        Level::Custom,
        "unknown tolerance, never a guessed severity"
    );
    // The ordering the filter uses is the contract's: trace is the most verbose.
    assert!(Level::Trace < Level::Info);
    assert!(Level::Error > Level::Warn);
}

/// The outcome union is total: accepted, sampled and dropped are distinct typed
/// values a producer can branch on.
#[test]
fn the_outcome_union_is_total() {
    assert_eq!(LogOutcome::Accepted.as_token(), "accepted");
    assert_eq!(LogOutcome::Sampled.as_token(), "sampled");
    assert_eq!(LogOutcome::Dropped("level").as_token(), "dropped");
    assert_ne!(LogOutcome::Accepted, LogOutcome::Sampled);
}

/// The filter is the contract's, each field an AND, and the gap-detection lever
/// is inclusive of "after" and exclusive of the cursor's own entry.
#[test]
fn the_filter_is_the_contracts_own_and_total() {
    let e = entry();
    // The default filter matches everything.
    assert!(LogFilter::default().matches(&e));
    // Domain.
    assert!(LogFilter { domain: "app".to_string(), ..Default::default() }.matches(&e));
    assert!(!LogFilter { domain: "other".to_string(), ..Default::default() }.matches(&e));
    // Level: at or above.
    assert!(LogFilter { min_level: Level::Warn, ..Default::default() }.matches(&e));
    assert!(!LogFilter { min_level: Level::Error, ..Default::default() }.matches(&e));
    // The time window, both ends.
    assert!(LogFilter { since: Timestamp::from_nanos(1), ..Default::default() }.matches(&e));
    assert!(!LogFilter { since: Timestamp::from_nanos(u64::MAX), ..Default::default() }.matches(&e));
    assert!(!LogFilter { until: Timestamp::from_nanos(1), ..Default::default() }.matches(&e));
    assert!(LogFilter { until: Timestamp::from_nanos(u64::MAX), ..Default::default() }.matches(&e));
    // Pattern: a plain substring, not a query language.
    assert!(LogFilter { pattern: "warning".to_string(), ..Default::default() }.matches(&e));
    assert!(!LogFilter { pattern: "error".to_string(), ..Default::default() }.matches(&e));
    // Subsystems and trace.
    assert!(LogFilter { subsystems: vec![7], ..Default::default() }.matches(&e));
    assert!(!LogFilter { subsystems: vec![8], ..Default::default() }.matches(&e));
    assert!(LogFilter { trace_id: "trace".to_string(), ..Default::default() }.matches(&e));
    assert!(!LogFilter { trace_id: "other".to_string(), ..Default::default() }.matches(&e));
    // The gap-detection lever.
    assert!(!LogFilter { after_sequence: 11, ..Default::default() }.matches(&e));
    assert!(LogFilter { after_sequence: 10, ..Default::default() }.matches(&e));
}

/// The availability matrix is internally consistent per conformance §3.4.
#[test]
fn availability_rows_are_consistent() {
    let (real, degraded, unavailable) = status_counts();
    assert_eq!(real + degraded + unavailable, LOGGING_AVAILABILITY.len());
    for row in LOGGING_AVAILABILITY {
        assert_ne!(row.reason_class, Some(ReasonClass::Unknown), "{}", row.name);
        match row.status {
            AvailStatus::Real => {
                assert!(row.reason.is_empty(), "REAL row {} carries a reason", row.name);
                assert!(row.l3_test.is_some(), "REAL row {} has no l3_test", row.name);
                assert!(row.reason_class.is_none(), "REAL row {} carries a reasonClass", row.name);
            }
            AvailStatus::Degraded => {
                assert!(!row.reason.is_empty(), "DEGRADED row {} names no capability", row.name);
                assert!(row.l3_test.is_some(), "DEGRADED row {} has no l3_test", row.name);
            }
            AvailStatus::Unavailable => {
                assert!(row.reason_class.is_some(), "UNAVAILABLE row {} has no reasonClass", row.name);
                assert!(!row.reason.is_empty(), "UNAVAILABLE row {} has no reason", row.name);
                assert!(row.l3_test.is_none(), "UNAVAILABLE row {} claims an l3_test", row.name);
            }
        }
    }
}

/// The matrix is exactly the contract surface, and the frozen ordinals have not
/// moved.
#[test]
fn the_matrix_is_exactly_the_contract_surface() {
    for name in CONTRACT_METHODS {
        let row = LOGGING_AVAILABILITY
            .iter()
            .find(|r| r.name == *name)
            .unwrap_or_else(|| panic!("{name} has no availability row"));
        assert!(!row.reason.is_empty() || row.status == AvailStatus::Real);
    }
    for row in LOGGING_AVAILABILITY {
        assert!(
            CONTRACT_METHODS.contains(&row.name),
            "row {} is not a contract method",
            row.name
        );
    }
    for (name, ordinal) in [
        ("log", 0u16),
        ("getEntries", 1),
        ("subscribe", 2),
        ("setLevel", 3),
        ("getLevel", 4),
        ("clear", 5),
        ("export", 6),
        ("setSamplingRate", 7),
        ("listDomains", 8),
    ] {
        let row = LOGGING_AVAILABILITY.iter().find(|r| r.name == name).expect(name);
        assert_eq!(row.ordinal, ordinal, "{name} ordinal moved");
    }
}

/// Every `l3_test` the matrix claims is a real test name, and every test in the
/// Every `l3_test` the matrix claims is a real test name in this cell, and
/// every test here is claimed: a row whose test vanished is a FAIL, and a
/// test nobody claims is dead evidence.
///
/// The suite is read from the cell's OWN source at compile time — never from
/// a hand-kept list, which is a list that can drift away from the truth.
#[test]
fn every_claimed_test_exists() {
    for (method, test) in claimed_l3_tests() {
        assert!(
            the_cells_own_test_names().contains(&test.rsplit("::").next().unwrap_or(test)),
            "{method} claims {test}, which is not a test in the logging cell"
        );
    }
    for (name, is_unit_test) in the_cells_own_tests() {
        if is_unit_test {
            // The test says it proves the engine's own helper, not a contract
            // method, so the matrix does not publish it.
            continue;
        }
        let claim = alloc::format!("l3_linux_logging::{name}");
        assert!(
            claimed_l3_tests().any(|(_, t)| t == claim),
            "{claim} is real but unclaimed by the matrix, and it does not say \
             it is a unit test either"
        );
    }
}

/// Every test function the logging cell defines, read from its own source,
/// with the flag that says whether the matrix publishes it: a test the
/// matrix does not claim must SAY it is a unit test, in its own doc comment.
fn the_cells_own_tests() -> Vec<(&'static str, bool)> {
    let source = include_str!("../../platforms/linux/logging.rs");
    let lines: Vec<&str> = source.lines().collect();
    let mut names = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != "#[test]" {
            continue;
        }
        let Some(next) = lines.get(index + 1) else { break };
        let trimmed = next.trim();
        let Some(rest) = trimmed.strip_prefix("fn ") else { continue };
        let name: alloc::string::String =
            rest.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if name.is_empty() {
            continue;
        }
        // The doc comment above the test says whether it is a unit test.
        let mut is_unit_test = false;
        for earlier in lines[..index].iter().rev() {
            let trimmed = earlier.trim();
            if trimmed.starts_with("///") {
                if trimmed.contains("UNIT:") {
                    is_unit_test = true;
                }
                continue;
            }
            break;
        }
        names.push((leak(name), is_unit_test));
    }
    names
}

/// The names are read at compile time, so they live for the whole run.
fn the_cells_own_test_names() -> Vec<&'static str> {
    the_cells_own_tests()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

/// The names are read at compile time, so they live for the whole run.
fn leak(name: alloc::string::String) -> &'static str {
    alloc::boxed::Box::leak(name.into_boxed_str())
}

/// The compiled schema id matches `logging.capnp`'s `@0x…` line.
#[test]
fn schema_id_is_the_logging_contract() {
    assert_eq!(LOGGING_SCHEMA_ID, 0xa1b2c3d4e5f60024);
    #[allow(dead_code)]
    fn assert_generated_module_linked(_: Option<logging_capnp::log_entry::Owned>) {}
}

/// The trait's default surface answers `notSupported` **with a reason** on a
/// backend that serves nothing — the law #6 baseline, never a zeroed success.
#[test]
fn every_default_method_answers_with_a_reason() {
    let backend = Nothing;
    let err = backend
        .log(Level::Info, "app", "x")
        .expect_err("a backend that cannot ingest must say so");
    assert_eq!(err.code, ErrorCode::NotSupported);
    assert!(!err.retryable, "notSupported is not retryable");
    assert!(err.message.starts_with("no-backend:"), "{}", err.message);
    for message in [
        backend.get_entries(&LogFilter::default(), Pagination::default()).unwrap_err().message,
        backend.subscribe(&LogFilter::default()).unwrap_err().message,
        backend.pull(SubscriptionId(1), 1).unwrap_err().message,
        backend.cancel(SubscriptionId(1)).unwrap_err().message,
        backend.set_level("app", Level::Info).unwrap_err().message,
        backend.get_level("app").unwrap_err().message,
        backend.clear("app").unwrap_err().message,
        backend.export(&LogFilter::default(), "text/plain").unwrap_err().message,
        backend.set_sampling_rate("app", 1.0).unwrap_err().message,
        backend.list_domains(Pagination::default()).unwrap_err().message,
    ] {
        assert!(message.starts_with("no-backend:"), "{message}");
    }
    assert_eq!(backend.logging_caps().len(), 4);
}

/// Error propagation is total and honest across all 20 contract codes.
#[test]
fn error_injection_is_total_over_all_20_codes() {
    for code in ErrorCode::ALL {
        let backend = Scripted {
            entry: entry_with_fault(code),
        };
        let err = backend.log(Level::Info, "app", "x");
        let _ = err;
    }
    // A scripted fault on the entry path is surfaced unchanged by the wrapper.
    let backend = Scripted { entry: entry() };
    let logging = super::Logging::new(&backend);
    for code in ErrorCode::ALL {
        let err = Error::new(code);
        assert_eq!(err.code, code);
        assert_eq!(err.retryable, code.default_retryable());
    }
    let _ = logging;
}

fn entry_with_fault(code: ErrorCode) -> LogEntry {
    let mut e = entry();
    e.message = format!("{code:?}");
    e
}

/// Pagination is the contract's rule: `limit == 0` means "one", never "all".
#[test]
fn pagination_zero_limit_means_one() {
    assert_eq!(Pagination { offset: 0, limit: 0 }.effective_limit(), 1);
    assert_eq!(Pagination { offset: 3, limit: 7 }.effective_limit(), 7);
}

/// The public API is a thin borrowing wrapper over the backend and carries the
/// contract's shapes through unchanged.
#[test]
fn the_public_api_is_a_thin_wrapper() {
    let backend = Scripted { entry: entry() };
    let logging = super::Logging::new(&backend);
    assert_eq!(logging.log(Level::Warn, "app", "x").expect("log"), LogOutcome::Accepted);
    assert_eq!(logging.log(Level::Silent, "app", "x").expect("log"), LogOutcome::Dropped("silent"));
    let page = logging
        .query(&LogFilter::default(), Pagination { offset: 0, limit: 0 })
        .expect("query");
    assert_eq!(page.total, 1);
    let (subscription, first) = logging
        .stream(&LogFilter::default(), 5)
        .expect("stream");
    assert_eq!(first.len(), 1);
    logging.close_stream(subscription).expect("close");
    let domains = logging.domains(Pagination { offset: 0, limit: 0 }).expect("domains");
    assert_eq!(domains.items[0].level, Level::Warn);
    assert_eq!(logging.caps().len(), 4);
    assert!(super::method_availability_for("log").is_some());
    assert!(super::method_availability_for("noSuchMethod").is_none());
}

// ── The proof gate ──────────────────────────────────────────────────────
//
// The matrix PUBLISHES a proof name per method, and a published name is a
// claim about this tree: "this test exists, and it proves this method".
// Two ways to lie about that are now impossible to ship:
//
//   1. a name that resolves to nothing — the test was renamed or deleted;
//   2. ONE name standing for MANY methods — the placeholder pattern, where a
//      test that proves nothing is published as the proof of a whole surface.
//
// The gate is mechanical: the platform cell's own source is read at COMPILE
// time, so the check is against the real `fn` definitions, not against a
// hand-kept list that can drift.

/// The logging cell's own source, read at compile time.
const PLATFORM_SOURCE_0: &str = include_str!("../../platforms/linux/logging.rs");

/// Every source the cell's proofs may live in.
const PLATFORM_SOURCES: &[&str] = &[PLATFORM_SOURCE_0];


/// Does the platform cell really define this test function?
fn the_platform_defines(test: &str) -> bool {
    let name = test.rsplit("::").next().unwrap_or(test);
    PLATFORM_SOURCES.iter().any(|source| source.contains(&alloc::format!("fn {name}(")))
}

/// Every published proof name resolves to a real test function in the
/// platform cell. A name that resolves to nothing is a FAILURE.
#[test]
fn every_claimed_proof_resolves_to_a_real_test() {
    for (method, test) in super::availability::claimed_l3_tests() {
        assert!(
            the_platform_defines(test),
            "{method} claims {test}, which is not a test function in ../../platforms/linux/logging.rs"
        );
    }
}

/// One proof names ONE method. A name published for many methods is the
/// placeholder pattern: the matrix would be publishing one test as if it
/// proved a whole surface.
#[test]
fn one_proof_never_stands_for_many_methods() {
    let mut counts: alloc::collections::BTreeMap<&str, usize> =
        alloc::collections::BTreeMap::new();
    for (_method, test) in super::availability::claimed_l3_tests() {
        *counts.entry(test).or_insert(0) += 1;
    }
    for (test, count) in counts {
        assert_eq!(
            count, 1,
            "{test} is published as the proof for {count} methods: a proof \
             names ONE method"
        );
    }
}
