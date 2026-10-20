use alloc::string::String;
use alloc::vec::Vec;

use crate::core::values::Duration;
use crate::core::{Error, Result};

use super::types::{MediaMetadata, MediaPlayer, MediaPlayerId, Page, Pagination, SubscriptionId};

pub trait MediaBackend {
    fn list_players(&self, _page: Pagination) -> Result<Vec<MediaPlayer>> {
        Err(Error::not_supported("no-backend: media players are not served on this backend"))
    }
    fn get_player(&self, _player_id: MediaPlayerId) -> Result<MediaPlayer> {
        Err(Error::not_supported("no-backend: media player is not served on this backend"))
    }
    fn play(&self, _player_id: MediaPlayerId) -> Result<()> {
        Err(Error::not_supported("no-backend: media play is not served on this backend"))
    }
    fn pause(&self, _player_id: MediaPlayerId) -> Result<()> {
        Err(Error::not_supported("no-backend: media pause is not served on this backend"))
    }
    fn stop(&self, _player_id: MediaPlayerId) -> Result<()> {
        Err(Error::not_supported("no-backend: media stop is not served on this backend"))
    }
    fn next(&self, _player_id: MediaPlayerId) -> Result<()> {
        Err(Error::not_supported("no-backend: media next is not served on this backend"))
    }
    fn previous(&self, _player_id: MediaPlayerId) -> Result<()> {
        Err(Error::not_supported("no-backend: media previous is not served on this backend"))
    }
    fn seek(&self, _player_id: MediaPlayerId, _position: Duration) -> Result<()> {
        Err(Error::not_supported("no-backend: media seek is not served on this backend"))
    }
    fn get_metadata(&self, _player_id: MediaPlayerId) -> Result<MediaMetadata> {
        Err(Error::not_supported("no-backend: media metadata is not served on this backend"))
    }
    fn get_position(&self, _player_id: MediaPlayerId) -> Result<(Duration, Duration)> {
        Err(Error::not_supported("no-backend: media position is not served on this backend"))
    }
    fn set_volume(&self, _player_id: MediaPlayerId, _volume: f32) -> Result<()> {
        Err(Error::not_supported("no-backend: media volume is not served on this backend"))
    }
    fn set_position(&self, _player_id: MediaPlayerId, _position: Duration) -> Result<()> {
        Err(Error::not_supported("no-backend: media set position is not served on this backend"))
    }
    fn open_uri(&self, _player_id: MediaPlayerId, _uri: String) -> Result<()> {
        Err(Error::not_supported("no-backend: media open uri is not served on this backend"))
    }
    fn subscribe_player(&self, _player_id: MediaPlayerId) -> Result<SubscriptionId> {
        Err(Error::not_supported("no-backend: media subscribe is not served on this backend"))
    }
    fn get_supported_formats(&self, _player_id: MediaPlayerId) -> Result<Vec<String>> {
        Err(Error::not_supported("no-backend: media formats are not served on this backend"))
    }
}
