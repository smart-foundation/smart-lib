use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use crate::core::{Error, ErrorCode};
use crate::schema::audio_capnp;

use super::availability::{
    status_counts, AvailStatus, AUDIO_AVAILABILITY, AUDIO_SCHEMA_ID, ReasonClass,
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
