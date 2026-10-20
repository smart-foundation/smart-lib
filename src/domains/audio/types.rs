use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::values::{Duration, Timestamp};
use crate::schema::{audio_capnp, types_capnp};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pagination {
    pub offset: u32,
    pub limit: u32,
}

impl Pagination {
    pub const fn effective_limit(self) -> u32 {
        if self.limit == 0 { 1 } else { self.limit }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct SubscriptionId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AudioDeviceId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct AudioStreamId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioDeviceType {
    Sink,
    Source,
    Server,
    Custom,
}
impl Default for AudioDeviceType { fn default() -> Self { AudioDeviceType::Sink } }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioState {
    pub volume: f32,
    pub mute: bool,
    pub default_output: u64,
    pub default_input: u64,
}

impl AudioState {
    pub fn write_to(&self, b: &mut audio_capnp::audio_state::Builder<'_>) {
        b.set_volume(self.volume);
        b.set_mute(self.mute);
        b.reborrow().init_default_output().set_id(self.default_output);
        b.reborrow().init_default_input().set_id(self.default_input);
    }
    pub fn from_reader(r: &audio_capnp::audio_state::Reader<'_>) -> Self {
        Self {
            volume: r.get_volume(),
            mute: r.get_mute(),
            default_output: r.get_default_output().map(|x| x.get_id()).unwrap_or(0),
            default_input: r.get_default_input().map(|x| x.get_id()).unwrap_or(0),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioDevice {
    pub id: u64,
    pub name: String,
    pub description: String,
    pub device_type: AudioDeviceType,
    pub volume: f32,
    pub mute: bool,
    pub is_default: bool,
    pub plugged_in: bool,
}

impl AudioDevice {
    pub fn write_to(&self, b: &mut audio_capnp::audio_device::Builder<'_>) {
        b.reborrow().init_id().set_id(self.id);
        b.set_name(&self.name);
        b.set_description(&self.description);
        b.set_volume(self.volume);
        b.set_mute(self.mute);
        b.set_is_default(self.is_default);
        b.set_plugged_in(self.plugged_in);
    }
    pub fn from_reader(r: &audio_capnp::audio_device::Reader<'_>) -> Self {
        Self {
            id: r.get_id().map(|x| x.get_id()).unwrap_or(0),
            name: r.get_name().map(|t| core::str::from_utf8(t.as_bytes()).unwrap_or("")).unwrap_or("").to_string(),
            description: r.get_description().map(|t| core::str::from_utf8(t.as_bytes()).unwrap_or("")).unwrap_or("").to_string(),
            device_type: AudioDeviceType::Sink,
            volume: r.get_volume(),
            mute: r.get_mute(),
            is_default: r.get_is_default(),
            plugged_in: r.get_plugged_in(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct AudioStream {
    pub id: u64,
    pub device: u64,
    pub name: String,
    pub volume: f32,
    pub mute: bool,
}

impl AudioStream {
    pub fn write_to(&self, b: &mut audio_capnp::audio_stream::Builder<'_>) {
        b.reborrow().init_id().set_id(self.id);
        b.reborrow().init_device().set_id(self.device);
        b.set_name(&self.name);
        b.set_volume(self.volume);
        b.set_mute(self.mute);
    }
    pub fn from_reader(r: &audio_capnp::audio_stream::Reader<'_>) -> Self {
        Self {
            id: r.get_id().map(|x| x.get_id()).unwrap_or(0),
            device: r.get_device().map(|x| x.get_id()).unwrap_or(0),
            name: r.get_name().map(|t| core::str::from_utf8(t.as_bytes()).unwrap_or("")).unwrap_or("").to_string(),
            volume: r.get_volume(),
            mute: r.get_mute(),
        }
    }
}
