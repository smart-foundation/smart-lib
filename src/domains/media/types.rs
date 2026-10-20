use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::core::values::Duration;
use crate::schema::{media_capnp, types_capnp};

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
pub struct MediaPlayerId(pub u64);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaybackStatus {
    Stopped,
    Playing,
    Paused,
    Forward,
    Rewind,
    Custom,
}
impl Default for PlaybackStatus { fn default() -> Self { PlaybackStatus::Stopped } }

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaMetadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration: Duration,
    pub art_uri: String,
}

impl MediaMetadata {
    pub fn write_to(&self, b: &mut media_capnp::media_metadata::Builder<'_>) {
        b.set_title(&self.title);
        b.set_artist(&self.artist);
        b.set_album(&self.album);
        b.reborrow().init_duration().set_nanoseconds(self.duration.as_nanos());
        b.set_art_uri(&self.art_uri);
    }
    pub fn from_reader(r: &media_capnp::media_metadata::Reader<'_>) -> Self {
        Self {
            title: r.get_title().map(|t| core::str::from_utf8(t.as_bytes()).unwrap_or("")).unwrap_or("").to_string(),
            artist: r.get_artist().map(|t| core::str::from_utf8(t.as_bytes()).unwrap_or("")).unwrap_or("").to_string(),
            album: r.get_album().map(|t| core::str::from_utf8(t.as_bytes()).unwrap_or("")).unwrap_or("").to_string(),
            duration: Duration::from_nanos(r.get_duration().map(|d| d.get_nanoseconds()).unwrap_or(0)),
            art_uri: r.get_art_uri().map(|t| core::str::from_utf8(t.as_bytes()).unwrap_or("")).unwrap_or("").to_string(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaPlayer {
    pub id: u64,
    pub name: String,
    pub identity: String,
    pub status: PlaybackStatus,
    pub volume: f32,
    pub metadata: MediaMetadata,
}

impl MediaPlayer {
    pub fn write_to(&self, b: &mut media_capnp::media_player::Builder<'_>) {
        b.reborrow().init_id().set_id(self.id);
        b.set_name(&self.name);
        b.set_identity(&self.identity);
        let mut md = b.reborrow().init_metadata();
        self.metadata.write_to(&mut md);
    }
    pub fn from_reader(r: &media_capnp::media_player::Reader<'_>) -> Self {
        Self {
            id: r.get_id().map(|x| x.get_id()).unwrap_or(0),
            name: r.get_name().map(|t| core::str::from_utf8(t.as_bytes()).unwrap_or("")).unwrap_or("").to_string(),
            identity: r.get_identity().map(|t| core::str::from_utf8(t.as_bytes()).unwrap_or("")).unwrap_or("").to_string(),
            status: PlaybackStatus::Stopped,
            volume: 0.0,
            metadata: r.get_metadata().map(|x| MediaMetadata::from_reader(&x)).unwrap_or_default(),
        }
    }
}
