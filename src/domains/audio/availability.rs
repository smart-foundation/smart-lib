//! audio availability matrix.

pub const AUDIO_SCHEMA_ID: u64 = 11651590505119481872;

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

pub const AUDIO_AVAILABILITY: &[MethodAvailability] = &[
    MethodAvailability::real("getState", 0, "l3_linux_audio::get_state_reports_the_hosts_own_default_sink"),
    MethodAvailability::real("listDevices", 1, "l3_linux_audio::list_devices_enumerates_the_hosts_own_sinks_and_sources"),
    MethodAvailability::real("listStreams", 2, "l3_linux_audio::list_streams_enumerates_the_hosts_own_sink_inputs"),
    MethodAvailability::degraded("setVolume", 3, "l3_linux_audio::set_volume_proves_the_write_path_and_leaves_the_host_as_it_found_it", "native PulseAudio (libpulse): pa_context_set_sink_volume_by_name/mute; the host-mutating verb is proven by a test that reads the host's own value, writes another, reads the host back and restores the host exactly as it found it"),
    MethodAvailability::degraded("setMute", 4, "l3_linux_audio::set_mute_proves_the_write_path_and_leaves_the_host_as_it_found_it", "native PulseAudio (libpulse): pa_context_set_sink_volume_by_name/mute; the host-mutating verb is proven by a test that reads the host's own value, writes another, reads the host back and restores the host exactly as it found it"),
    MethodAvailability::degraded("setStreamVolume", 5, "l3_linux_audio::set_stream_volume_proves_the_write_path_and_leaves_the_host_as_it_found_it", "native PulseAudio (libpulse): pa_context_set_sink_volume_by_name/mute; the host-mutating verb is proven by a test that reads the host's own value, writes another, reads the host back and restores the host exactly as it found it"),
    MethodAvailability::degraded("setStreamMute", 6, "l3_linux_audio::set_stream_mute_proves_the_write_path_and_leaves_the_host_as_it_found_it", "native PulseAudio (libpulse): pa_context_set_sink_volume_by_name/mute; the host-mutating verb is proven by a test that reads the host's own value, writes another, reads the host back and restores the host exactly as it found it"),
    MethodAvailability::unavailable("enableDevice", 7, ReasonClass::NoBackend, "PulseAudio does not expose device enable/disable; driver-level (udev/ALSA) surface not wired"),
    MethodAvailability::unavailable("disableDevice", 8, ReasonClass::NoBackend, "PulseAudio does not expose device enable/disable; driver-level (udev/ALSA) surface not wired"),
    MethodAvailability::real("subscribeState", 9, "l3_linux_audio::subscribe_state_mints_a_subscription"),
    MethodAvailability::real("pullState", 10, "l3_linux_audio::pull_state_answers_with_the_hosts_own_state"),
    MethodAvailability::real("cancelState", 11, "l3_linux_audio::cancel_state_retires_the_subscription"),
    MethodAvailability::real("subscribeDevices", 12, "l3_linux_audio::subscribe_devices_mints_a_subscription"),
    MethodAvailability::real("pullDevices", 13, "l3_linux_audio::pull_devices_answers_with_the_hosts_own_devices"),
    MethodAvailability::real("cancelDevices", 14, "l3_linux_audio::cancel_devices_retires_the_subscription"),
    MethodAvailability::real("subscribeStreams", 15, "l3_linux_audio::subscribe_streams_mints_a_subscription"),
    MethodAvailability::real("pullStreams", 16, "l3_linux_audio::pull_streams_answers_with_the_hosts_own_streams"),
    MethodAvailability::real("cancelStreams", 17, "l3_linux_audio::cancel_streams_retires_the_subscription"),
    MethodAvailability::unavailable("requestFocus", 18, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("releaseFocus", 19, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("subscribeFocus", 20, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("pullFocus", 21, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("cancelFocus", 22, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setStreamPosition", 23, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setListenerPosition", 24, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setStreamSpatialProfile", 25, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("recordConsent", 26, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("declareRecordingPolicy", 27, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("listCodecs", 28, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("negotiateCodec", 29, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("subscribeCodecs", 30, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("pullCodecs", 31, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("cancelCodecs", 32, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setRoute", 33, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("duckOnCall", 34, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("listEffects", 35, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("addEffect", 36, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("removeEffect", 37, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setEffectEnabled", 38, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setEffectParameter", 39, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("listPresets", 40, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("applyPreset", 41, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("saveCustomPreset", 42, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("deleteCustomPreset", 43, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setGlobalDuckPolicy", 44, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getGlobalDuckPolicy", 45, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getStreamMetering", 46, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getDeviceMetering", 47, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("createSession", 48, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("joinSession", 49, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("leaveSession", 50, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getSession", 51, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("listSessions", 52, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setSessionRouting", 53, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getAudioTransport", 54, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("listChannelLayouts", 55, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setPerRoleDuckLevels", 56, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getPerRoleDuckLevels", 57, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setHrtfData", 58, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getHrtfData", 59, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("playSoundEvent", 60, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("stopSoundEvent", 61, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setSoundEventVolume", 62, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getSoundEventVolume", 63, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("listSoundEvents", 64, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("loadSoundTheme", 65, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getActiveSoundTheme", 66, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getStreamAnalysis", 67, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getDeviceAnalysis", 68, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("subscribeAnalysis", 69, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("pullAnalysis", 70, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("cancelAnalysis", 71, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getMicAccess", 72, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("requestMicAccess", 73, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("revokeMicAccess", 74, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("listMicAccess", 75, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setMicPolicy", 76, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getMicPolicy", 77, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setLoudnessLimit", 78, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getLoudnessLimit", 79, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getLoudnessHistory", 80, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setSafetyMode", 81, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getSafetyMode", 82, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("suggestRoute", 83, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getRoutingRules", 84, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setRoutingRules", 85, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getActiveRoutes", 86, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("listTransports", 87, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getTransportInfo", 88, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setTransportPriorities", 89, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("getDeviceHealth", 90, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("subscribeDeviceHealth", 91, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("pullDeviceHealth", 92, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("cancelDeviceHealth", 93, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("createAudioZone", 94, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("removeAudioZone", 95, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("listAudioZones", 96, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setZoneRoute", 97, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("setZoneVolume", 98, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
    MethodAvailability::unavailable("openDriver", 99, ReasonClass::NoBackend, "no native host API on PipeWire/PulseAudio for this surface"),
];

pub const PLATFORM: &str = "linux";
pub const TRANSPORTS: &[&str] = &["InProcess"];
pub const CAPS: &[&str] = &[];

pub fn status_counts() -> (usize, usize, usize) {
    let mut r=0; let mut d=0; let mut u=0; for row in AUDIO_AVAILABILITY { match row.status { AvailStatus::Real=>r+=1, AvailStatus::Degraded=>d+=1, AvailStatus::Unavailable=>u+=1 } } (r,d,u)
}

pub fn claimed_l3_tests() -> impl Iterator<Item = (&'static str, &'static str)> {
    AUDIO_AVAILABILITY.iter().filter(|r| r.l3_test.is_some()).map(|r| (r.name, r.l3_test.unwrap()))
}

pub fn method_availability_for(name: &str) -> Option<&'static MethodAvailability> {
    AUDIO_AVAILABILITY.iter().find(|r| r.name == name)
}
