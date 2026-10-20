//! The **audio** domain: volume, devices, streams, subscriptions — over one
//! object-safe [`AudioBackend`] trait. Native mechanism: PulseAudio client
//! library (PipeWire-pulse) for volume/mute/devices/streams; advanced
//! spatial/codec/effect surfaces have no native host API on this host.

pub mod api;
pub mod availability;
pub mod backend;
#[cfg(test)]
pub mod tests;
pub mod types;

pub use api::Audio;
pub use availability::{
    claimed_l3_tests, method_availability_for, status_counts, AvailStatus, AUDIO_AVAILABILITY,
    AUDIO_SCHEMA_ID, MethodAvailability, ReasonClass,
};
pub use backend::AudioBackend;
pub use types::{
    AudioDevice, AudioDeviceType, AudioState, AudioStream, Page, Pagination, SubscriptionId,
};
