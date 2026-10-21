use alloc::collections::BTreeMap;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::core::{Error, ErrorCode};
use crate::schema::audio_capnp;

use super::availability::{
    claimed_l3_tests, status_counts, AvailStatus, AUDIO_AVAILABILITY, AUDIO_SCHEMA_ID,
    ReasonClass,
};
use super::backend::AudioBackend;
use super::types::{AudioDevice, AudioState, Pagination};

const CONTRACT_METHODS: &[&str] = &[
    "getState","listDevices","listStreams","setVolume","setMute","setStreamVolume","setStreamMute",
    "enableDevice","disableDevice","subscribeState","pullState","cancelState","subscribeDevices",
    "pullDevices","cancelDevices","subscribeStreams","pullStreams","cancelStreams","requestFocus",
    "releaseFocus","subscribeFocus","pullFocus","cancelFocus","setStreamPosition","setListenerPosition",
    "setStreamSpatialProfile","recordConsent","declareRecordingPolicy","listCodecs","negotiateCodec",
    "subscribeCodecs","pullCodecs","cancelCodecs","setRoute","duckOnCall","listEffects","addEffect",
    "removeEffect","setEffectEnabled","setEffectParameter","listPresets","applyPreset","saveCustomPreset",
    "deleteCustomPreset","setGlobalDuckPolicy","getGlobalDuckPolicy","getStreamMetering","getDeviceMetering",
    "createSession","joinSession","leaveSession","getSession","listSessions","setSessionRouting",
    "getAudioTransport","listChannelLayouts","setPerRoleDuckLevels","getPerRoleDuckLevels","setHrtfData",
    "getHrtfData","playSoundEvent","stopSoundEvent","setSoundEventVolume","getSoundEventVolume",
    "listSoundEvents","loadSoundTheme","getActiveSoundTheme","getStreamAnalysis","getDeviceAnalysis",
    "subscribeAnalysis","pullAnalysis","cancelAnalysis","getMicAccess","requestMicAccess","revokeMicAccess",
    "listMicAccess","setMicPolicy","getMicPolicy","setLoudnessLimit","getLoudnessLimit","getLoudnessHistory",
    "setSafetyMode","getSafetyMode","suggestRoute","getRoutingRules","setRoutingRules","getActiveRoutes",
    "listTransports","getTransportInfo","setTransportPriorities","getDeviceHealth","subscribeDeviceHealth",
    "pullDeviceHealth","cancelDeviceHealth","createAudioZone","removeAudioZone","listAudioZones",
    "setZoneRoute","setZoneVolume","openDriver",
];

struct HonestBackend;
impl AudioBackend for HonestBackend {}

#[test]
fn availability_matrix_matches_contract() {
    assert_eq!(AUDIO_AVAILABILITY.len(), CONTRACT_METHODS.len());
    for (row, name) in AUDIO_AVAILABILITY.iter().zip(CONTRACT_METHODS.iter()) {
        assert_eq!(row.name, *name);
    }
}

#[test]
fn claimed_l3_tests_exist() {
    for (_name, test) in super::availability::claimed_l3_tests() {
        assert!(test.starts_with("l3_linux_audio::"));
    }
}

// ── The proof gate ──────────────────────────────────────────────────────
//
// The matrix PUBLISHES a proof name per method. A published name is a claim
// about the tree: "this test exists, and it proves this method". Two ways to
// lie about that are now impossible to ship:
//
//   1. a name that resolves to nothing — the test was renamed or deleted;
//   2. ONE name standing for MANY methods — the placeholder pattern, the
//      `core_works` rot, where a test that proves nothing is published as the
//      proof of a hundred verbs.
//
// The gate is mechanical: the platform module's own source is read at
// COMPILE time, so the check is against the real `fn` definitions, not
// against a hand-kept list that can drift.

/// The audio platform cell's own source, read at compile time.
const PLATFORM_SOURCE: &str = include_str!("../../platforms/linux/audio.rs");

/// Does the platform module really define this test function?
fn the_platform_defines(test: &str) -> bool {
    let name = test.rsplit("::").next().unwrap_or(test);
    PLATFORM_SOURCE.contains(&alloc::format!("fn {name}("))
}

/// Every published proof name resolves to a real test function in the
/// platform module. A name that resolves to nothing is a FAILURE.
#[test]
fn every_claimed_proof_resolves_to_a_real_test() {
    for (method, test) in claimed_l3_tests() {
        assert!(
            the_platform_defines(test),
            "{method} claims {test}, which is not a test function in \
             src/platforms/linux/audio.rs"
        );
    }
}

/// One proof names ONE method. A name published for many methods is the
/// placeholder pattern: the matrix would be publishing one test as if it
/// proved a whole surface.
#[test]
fn one_proof_never_stands_for_many_methods() {
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for (_method, test) in claimed_l3_tests() {
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

/// A served row names a proof; an unserved row names none. A row that claims
/// a test it does not have is a FAILURE, and a row that claims nothing while
/// claiming to be served is one too.
#[test]
fn every_served_row_names_its_own_proof() {
    for row in AUDIO_AVAILABILITY {
        match row.status {
            AvailStatus::Real => {
                assert!(row.l3_test.is_some(), "{} names its backing test", row.name);
                assert!(row.reason.is_empty(), "{} states no limit", row.name);
            }
            AvailStatus::Degraded => {
                assert!(row.l3_test.is_some(), "{} names its backing test", row.name);
                assert!(!row.reason.is_empty(), "{} states the limit", row.name);
            }
            AvailStatus::Unavailable => {
                assert!(row.l3_test.is_none(), "{} claims no test it does not have", row.name);
                assert!(row.reason_class.is_some(), "{} names why", row.name);
                assert!(!row.reason.is_empty(), "{} states the reason", row.name);
            }
        }
    }
}

#[test]
fn audio_state_round_trips() {
    let s = AudioState { volume: 0.5, mute: true, default_output: 1, default_input: 2 };
    let mut message = capnp::message::Builder::new_default();
    {
        let mut builder = message.init_root::<audio_capnp::audio_state::Builder<'_>>();
        s.write_to(&mut builder);
    }
    let mut buf = Vec::new();
    capnp::serialize::write_message(&mut buf, &message).expect("serialize");
    let reader = capnp::serialize::read_message(&mut &buf[..], capnp::message::ReaderOptions::new()).expect("deserialize");
    let wire = reader.get_root::<audio_capnp::audio_state::Reader<'_>>().expect("root");
    assert_eq!(AudioState::from_reader(&wire), s);
}
