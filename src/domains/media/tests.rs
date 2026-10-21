use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::core::{Error, ErrorCode};
use crate::schema::media_capnp;

use super::availability::{status_counts, AvailStatus, MEDIA_AVAILABILITY, MEDIA_SCHEMA_ID};
use super::backend::MediaBackend;
use super::types::{MediaMetadata, MediaPlayer};

const CONTRACT_METHODS: &[&str] = &[
    "listPlayers","getPlayer","play","pause","stop","next","previous","seek","getMetadata","getPosition",
    "setVolume","setPosition","openUri","subscribePlayer","getSupportedFormats","transcode",
    "getRecommendations","getAnalytics","shareMedia","downloadMedia","setCrossfade","setSyncGroup",
    "createPlaylist","addToPlaylist","removeFromPlaylist","reorderPlaylist","getRecommendationsWithScores",
    "getSharePreview","getDownloadStatus","listDownloads","pauseDownload","resumeDownload","cancelDownload",
    "subscribeDownload","setCrossfadeConfig","getCrossfadeConfig","sendLiveInteraction",
    "subscribeLiveInteractions","subscribeTranscode",
];

struct HonestBackend;
impl MediaBackend for HonestBackend {}

#[test]
fn availability_matrix_matches_contract() {
    assert_eq!(MEDIA_AVAILABILITY.len(), CONTRACT_METHODS.len());
    for (row, name) in MEDIA_AVAILABILITY.iter().zip(CONTRACT_METHODS.iter()) {
        assert_eq!(row.name, *name);
    }
}

#[test]
fn media_metadata_round_trips() {
    let m = MediaMetadata { title: "t".to_string(), artist: "a".to_string(), album: "b".to_string(), duration: crate::core::values::Duration::from_nanos(10), art_uri: "u".to_string() };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<media_capnp::media_metadata::Builder<'_>>();
        m.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new()).expect("deserialize");
    let wire = reader.get_root::<media_capnp::media_metadata::Reader<'_>>().expect("root");
    assert_eq!(MediaMetadata::from_reader(&wire), m);
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

/// The media cell's own source, read at compile time.
const PLATFORM_SOURCE_0: &str = include_str!("../../platforms/linux/media.rs");

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
            "{method} claims {test}, which is not a test function in ../../platforms/linux/media.rs"
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
