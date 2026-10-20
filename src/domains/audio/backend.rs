use alloc::string::String;
use alloc::vec::Vec;

use crate::core::{Error, Result};

use super::types::{AudioDevice, AudioDeviceId, AudioState, AudioStream, AudioStreamId, Page, Pagination, SubscriptionId};

pub trait AudioBackend {
    fn get_state(&self) -> Result<AudioState> {
        Err(Error::not_supported("no-backend: audio state is not served on this backend"))
    }
    fn list_devices(&self, _page: Pagination) -> Result<Page<AudioDevice>> {
        Err(Error::not_supported("no-backend: audio devices are not served on this backend"))
    }
    fn list_streams(&self, _page: Pagination) -> Result<Page<AudioStream>> {
        Err(Error::not_supported("no-backend: audio streams are not served on this backend"))
    }
    fn set_volume(&self, _device: AudioDeviceId, _volume: f32) -> Result<()> {
        Err(Error::not_supported("no-backend: audio volume is not served on this backend"))
    }
    fn set_mute(&self, _device: AudioDeviceId, _mute: bool) -> Result<()> {
        Err(Error::not_supported("no-backend: audio mute is not served on this backend"))
    }
    fn set_stream_volume(&self, _stream: AudioStreamId, _volume: f32) -> Result<()> {
        Err(Error::not_supported("no-backend: stream volume is not served on this backend"))
    }
    fn set_stream_mute(&self, _stream: AudioStreamId, _mute: bool) -> Result<()> {
        Err(Error::not_supported("no-backend: stream mute is not served on this backend"))
    }
    fn subscribe_state(&self) -> Result<SubscriptionId> {
        Err(Error::not_supported("no-backend: audio state subscription is not served on this backend"))
    }
    fn pull_state(&self, _subscription: SubscriptionId, _max_events: u32) -> Result<AudioState> {
        Err(Error::not_supported("no-backend: audio state pull is not served on this backend"))
    }
    fn cancel_state(&self, _subscription: SubscriptionId) -> Result<()> {
        Err(Error::not_supported("no-backend: audio state cancel is not served on this backend"))
    }
    fn subscribe_devices(&self) -> Result<SubscriptionId> {
        Err(Error::not_supported("no-backend: audio devices subscription is not served on this backend"))
    }
    fn pull_devices(&self, _subscription: SubscriptionId, _max_events: u32) -> Result<Vec<AudioDevice>> {
        Err(Error::not_supported("no-backend: audio devices pull is not served on this backend"))
    }
    fn cancel_devices(&self, _subscription: SubscriptionId) -> Result<()> {
        Err(Error::not_supported("no-backend: audio devices cancel is not served on this backend"))
    }
    fn subscribe_streams(&self) -> Result<SubscriptionId> {
        Err(Error::not_supported("no-backend: audio streams subscription is not served on this backend"))
    }
    fn pull_streams(&self, _subscription: SubscriptionId, _max_events: u32) -> Result<Vec<AudioStream>> {
        Err(Error::not_supported("no-backend: audio streams pull is not served on this backend"))
    }
    fn cancel_streams(&self, _subscription: SubscriptionId) -> Result<()> {
        Err(Error::not_supported("no-backend: audio streams cancel is not served on this backend"))
    }
}
