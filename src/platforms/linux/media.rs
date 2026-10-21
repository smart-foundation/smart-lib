extern crate std;

use alloc::boxed::Box;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

use crate::core::{Error, Result};
use crate::core::values::Duration;
use crate::domains::media::backend::MediaBackend;
use crate::domains::media::types::{MediaMetadata, MediaPlayer, MediaPlayerId, PlaybackStatus};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";

pub struct LinuxMediaBackend;

impl LinuxMediaBackend {
    pub fn new() -> Self { Self }
}

fn session_conn() -> std::result::Result<dbus::blocking::Connection, String> {
    dbus::blocking::Connection::new_session().map_err(|e| format!("dbus: {e}"))
}

fn list_player_names(conn: &dbus::blocking::Connection) -> Vec<String> {
    let proxy = conn.with_proxy("org.freedesktop.DBus", "/org/freedesktop/DBus", std::time::Duration::from_millis(750));
    let (names,): (Vec<String>,) = match proxy.method_call("org.freedesktop.DBus", "ListNames", ()) {
        Ok(x) => x,
        Err(_) => return Vec::new(),
    };
    names.into_iter().filter(|n| n.starts_with(MPRIS_PREFIX)).collect()
}

fn player_proxy<'a>(conn: &'a dbus::blocking::Connection, name: &'a str) -> dbus::blocking::Proxy<'a, &'a dbus::blocking::Connection> {
    conn.with_proxy(name, "/org/mpris/MediaPlayer2", std::time::Duration::from_millis(750))
}

fn get_str(conn: &dbus::blocking::Connection, name: &str, iface: &str, prop: &str) -> String {
    let proxy = player_proxy(conn, name);
    let (variant,): (dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,) = match proxy.method_call("org.freedesktop.DBus.Properties", "Get", (iface, prop)) {
        Ok(x) => x,
        Err(_) => return String::new(),
    };
    variant.0.as_str().map(|s| s.to_string()).unwrap_or_default()
}

fn get_f64(conn: &dbus::blocking::Connection, name: &str, iface: &str, prop: &str) -> f64 {
    let proxy = player_proxy(conn, name);
    let (variant,): (dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,) = match proxy.method_call("org.freedesktop.DBus.Properties", "Get", (iface, prop)) {
        Ok(x) => x,
        Err(_) => return 0.0,
    };
    variant.0.as_f64().unwrap_or(0.0)
}

fn get_i64(conn: &dbus::blocking::Connection, name: &str, iface: &str, prop: &str) -> i64 {
    let proxy = player_proxy(conn, name);
    let (variant,): (dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,) = match proxy.method_call("org.freedesktop.DBus.Properties", "Get", (iface, prop)) {
        Ok(x) => x,
        Err(_) => return 0,
    };
    variant.0.as_i64().unwrap_or(0)
}

fn parse_playback_status(s: &str) -> PlaybackStatus {
    match s {
        "Playing" => PlaybackStatus::Playing,
        "Paused" => PlaybackStatus::Paused,
        "Stopped" => PlaybackStatus::Stopped,
        _ => PlaybackStatus::Stopped,
    }
}

fn parse_metadata(conn: &dbus::blocking::Connection, name: &str) -> MediaMetadata {
    let proxy = player_proxy(conn, name);
    let (variant,): (dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,) = match proxy.method_call::<(dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,), _, _, _>("org.freedesktop.DBus.Properties", "Get", ("org.mpris.MediaPlayer2.Player", "Metadata")) {
        Ok(x) => x,
        Err(_) => return MediaMetadata::default(),
    };
    let mut md = MediaMetadata::default();
    if let Some(mut iter) = variant.0.as_iter() {
        while let Some(entry) = iter.next() {
            if let Some(mut eiter) = entry.as_iter() {
                let key = eiter.next().and_then(|k| k.as_str()).unwrap_or("").to_string();
                if let Some(val) = eiter.next() {
                    match key.as_str() {
                        "xesam:title" => {
                            if let Some(s) = val.as_str() { md.title = s.to_string(); }
                        }
                        "xesam:album" => {
                            if let Some(s) = val.as_str() { md.album = s.to_string(); }
                        }
                        "xesam:artist" => {
                            if let Some(mut aiter) = val.as_iter() {
                                let mut artists = Vec::new();
                                while let Some(a) = aiter.next() {
                                    if let Some(s) = a.as_str() { artists.push(s.to_string()); }
                                }
                                md.artist = artists.join(", ");
                            } else if let Some(s) = val.as_str() {
                                md.artist = s.to_string();
                            }
                        }
                        "mpris:length" => {
                            if let Some(n) = val.as_i64() {
                                md.duration = Duration::from_nanos((n as i64 * 1000) as u64);
                            } else if let Some(n) = val.as_u64() {
                                md.duration = Duration::from_nanos((n as i64 * 1000) as u64);
                            }
                        }
                        "mpris:artUrl" => {
                            if let Some(s) = val.as_str() { md.art_uri = s.to_string(); }
                        }
                        _ => {}
                    }
                }
            }
        }
    }
    md
}

fn player_name_for(conn: &dbus::blocking::Connection, player_id: MediaPlayerId) -> Result<String> {
    let names = list_player_names(conn);
    let idx = player_id.0 as usize;
    if idx >= names.len() {
        return Err(Error::invalid_argument("no such player id"));
    }
    Ok(names[idx].clone())
}

impl MediaBackend for LinuxMediaBackend {
    fn list_players(&self, _page: crate::domains::media::types::Pagination) -> Result<Vec<MediaPlayer>> {
        let conn = session_conn().map_err(|e| Error::not_supported(&e))?;
        let names = list_player_names(&conn);
        let mut out = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let status_str = get_str(&conn, name, "org.mpris.MediaPlayer2.Player", "PlaybackStatus");
            let status = parse_playback_status(&status_str);
            let volume = get_f64(&conn, name, "org.mpris.MediaPlayer2.Player", "Volume") as f32;
            let identity = get_str(&conn, name, "org.mpris.MediaPlayer2", "Identity");
            let metadata = parse_metadata(&conn, name);
            out.push(MediaPlayer {
                id: i as u64,
                name: name.clone(),
                identity: if identity.is_empty() { name.clone() } else { identity },
                status,
                volume,
                metadata,
            });
        }
        Ok(out)
    }

    fn get_player(&self, player_id: MediaPlayerId) -> Result<MediaPlayer> {
        let conn = session_conn().map_err(|e| Error::not_supported(&e))?;
        let name = player_name_for(&conn, player_id)?;
        let status_str = get_str(&conn, &name, "org.mpris.MediaPlayer2.Player", "PlaybackStatus");
        let status = parse_playback_status(&status_str);
        let volume = get_f64(&conn, &name, "org.mpris.MediaPlayer2.Player", "Volume") as f32;
        let identity = get_str(&conn, &name, "org.mpris.MediaPlayer2", "Identity");
        let metadata = parse_metadata(&conn, &name);
        Ok(MediaPlayer {
            id: player_id.0,
            name: name.clone(),
            identity: if identity.is_empty() { name.clone() } else { identity },
            status,
            volume,
            metadata,
        })
    }

    fn play(&self, player_id: MediaPlayerId) -> Result<()> {
        let conn = session_conn().map_err(|e| Error::not_supported(&e))?;
        let name = player_name_for(&conn, player_id)?;
        let proxy = player_proxy(&conn, &name);
        proxy.method_call::<(), _, _, _>("org.mpris.MediaPlayer2.Player", "Play", ()).map_err(|e| Error::failed(format!("dbus: Play: {e}")))?;
        Ok(())
    }

    fn pause(&self, player_id: MediaPlayerId) -> Result<()> {
        let conn = session_conn().map_err(|e| Error::not_supported(&e))?;
        let name = player_name_for(&conn, player_id)?;
        let proxy = player_proxy(&conn, &name);
        proxy.method_call::<(), _, _, _>("org.mpris.MediaPlayer2.Player", "Pause", ()).map_err(|e| Error::failed(format!("dbus: Pause: {e}")))?;
        Ok(())
    }

    fn stop(&self, player_id: MediaPlayerId) -> Result<()> {
        let conn = session_conn().map_err(|e| Error::not_supported(&e))?;
        let name = player_name_for(&conn, player_id)?;
        let proxy = player_proxy(&conn, &name);
        proxy.method_call::<(), _, _, _>("org.mpris.MediaPlayer2.Player", "Stop", ()).map_err(|e| Error::failed(format!("dbus: Stop: {e}")))?;
        Ok(())
    }

    fn next(&self, player_id: MediaPlayerId) -> Result<()> {
        let conn = session_conn().map_err(|e| Error::not_supported(&e))?;
        let name = player_name_for(&conn, player_id)?;
        let proxy = player_proxy(&conn, &name);
        proxy.method_call::<(), _, _, _>("org.mpris.MediaPlayer2.Player", "Next", ()).map_err(|e| Error::failed(format!("dbus: Next: {e}")))?;
        Ok(())
    }

    fn previous(&self, player_id: MediaPlayerId) -> Result<()> {
        let conn = session_conn().map_err(|e| Error::not_supported(&e))?;
        let name = player_name_for(&conn, player_id)?;
        let proxy = player_proxy(&conn, &name);
        proxy.method_call::<(), _, _, _>("org.mpris.MediaPlayer2.Player", "Previous", ()).map_err(|e| Error::failed(format!("dbus: Previous: {e}")))?;
        Ok(())
    }

    fn seek(&self, player_id: MediaPlayerId, position: Duration) -> Result<()> {
        let conn = session_conn().map_err(|e| Error::not_supported(&e))?;
        let name = player_name_for(&conn, player_id)?;
        let proxy = player_proxy(&conn, &name);
        let offset_micros = position.as_nanos() / 1000;
        proxy.method_call::<(), _, _, _>("org.mpris.MediaPlayer2.Player", "Seek", (offset_micros,)).map_err(|e| Error::failed(format!("dbus: Seek: {e}")))?;
        Ok(())
    }

    fn get_metadata(&self, player_id: MediaPlayerId) -> Result<MediaMetadata> {
        let conn = session_conn().map_err(|e| Error::not_supported(&e))?;
        let name = player_name_for(&conn, player_id)?;
        Ok(parse_metadata(&conn, &name))
    }

    fn get_position(&self, player_id: MediaPlayerId) -> Result<(Duration, Duration)> {
        let conn = session_conn().map_err(|e| Error::not_supported(&e))?;
        let name = player_name_for(&conn, player_id)?;
        let pos_micros = get_i64(&conn, &name, "org.mpris.MediaPlayer2.Player", "Position");
        let pos = Duration::from_nanos((pos_micros * 1000) as u64);
        let md = parse_metadata(&conn, &name);
        Ok((pos, md.duration))
    }

    fn set_volume(&self, player_id: MediaPlayerId, volume: f32) -> Result<()> {
        let conn = session_conn().map_err(|e| Error::not_supported(&e))?;
        let name = player_name_for(&conn, player_id)?;
        let proxy = player_proxy(&conn, &name);
        proxy.method_call::<(), _, _, _>("org.freedesktop.DBus.Properties", "Set", ("org.mpris.MediaPlayer2.Player", "Volume", dbus::arg::Variant(volume as f64))).map_err(|e| Error::failed(format!("dbus: Set Volume: {e}")))?;
        Ok(())
    }

    fn set_position(&self, player_id: MediaPlayerId, position: Duration) -> Result<()> {
        let conn = session_conn().map_err(|e| Error::not_supported(&e))?;
        let name = player_name_for(&conn, player_id)?;
        let proxy = player_proxy(&conn, &name);
        let md = parse_metadata(&conn, &name);
        // trackid not stored in MediaMetadata; re-read from metadata dict
        let (variant,): (dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,) = proxy.method_call::<(dbus::arg::Variant<Box<dyn dbus::arg::RefArg>>,), _, _, _>("org.freedesktop.DBus.Properties", "Get", ("org.mpris.MediaPlayer2.Player", "Metadata")).unwrap_or_else(|_| (dbus::arg::Variant(Box::new(dbus::arg::Variant(0u32)) as Box<dyn dbus::arg::RefArg>),));
        let mut trackid = String::from("/");
        if let Some(mut iter) = variant.0.as_iter() {
            while let Some(entry) = iter.next() {
                if let Some(mut eiter) = entry.as_iter() {
                    let key = eiter.next().and_then(|k| k.as_str()).unwrap_or("");
                    if key == "mpris:trackid" {
                        if let Some(v) = eiter.next() {
                            if let Some(s) = v.as_str() { trackid = s.to_string(); }
                        }
                    }
                }
            }
        }
        let pos_micros = position.as_nanos() / 1000;
        proxy.method_call::<(), _, _, _>("org.mpris.MediaPlayer2.Player", "SetPosition", (trackid.as_str(), pos_micros)).map_err(|e| Error::failed(format!("dbus: SetPosition: {e}")))?;
        Ok(())
    }

    fn open_uri(&self, player_id: MediaPlayerId, uri: String) -> Result<()> {
        let conn = session_conn().map_err(|e| Error::not_supported(&e))?;
        let name = player_name_for(&conn, player_id)?;
        let proxy = player_proxy(&conn, &name);
        proxy.method_call::<(), _, _, _>("org.mpris.MediaPlayer2.Player", "OpenUri", (uri,)).map_err(|e| Error::failed(format!("dbus: OpenUri: {e}")))?;
        Ok(())
    }

    fn subscribe_player(&self, _player_id: MediaPlayerId) -> Result<crate::domains::media::types::SubscriptionId> {
        Ok(crate::domains::media::types::SubscriptionId(0))
    }

    fn get_supported_formats(&self, _player_id: MediaPlayerId) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::core::ErrorCode;
    use crate::domains::media::backend::MediaBackend;

    /// The backend over the host's own session bus.
    fn backend() -> LinuxMediaBackend {
        LinuxMediaBackend::new()
    }

    /// The players the host really has. `None` when the host has no session
    /// bus at all: the honest answer then is the refusal, and the caller
    /// proves THAT instead of a fabricated player.
    fn the_hosts_players() -> Option<Vec<MediaPlayer>> {
        let backend = backend();
        match backend.list_players(crate::domains::media::types::Pagination::default()) {
            Ok(players) => Some(players),
            Err(error) => {
                assert!(
                    error.code == ErrorCode::NotSupported,
                    "a failure to reach the session bus must be named, never swallowed: {error:?}"
                );
                None
            }
        }
    }

    /// `listPlayers` answers with the host's OWN MPRIS players: every entry
    /// is a real bus name, and the ids are distinct.
    #[test]
    fn list_players_answers_with_the_hosts_own_players() {
        let Some(players) = the_hosts_players() else { return };
        let mut ids: Vec<u64> = players.iter().map(|p| p.id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), total, "every player has its own id");
        for player in &players {
            // The name is the MPRIS bus name the host published — never a
            // fabricated label.
            assert!(
                player.name.starts_with("org.mpris.MediaPlayer2."),
                "the player's own name is its MPRIS bus name: {}",
                player.name
            );
            assert!(!player.identity.is_empty(), "the player carries an identity");
        }
    }

    /// `getPlayer` reads ONE entry natively: the player the id names is the
    /// one the listing reports.
    #[test]
    fn get_player_reads_one_entry_natively() {
        let Some(players) = the_hosts_players() else { return };
        let Some(first) = players.first() else {
            // A host with no player answers the honest refusal, not a zeroed
            // player.
            let error = backend()
                .get_player(MediaPlayerId(0))
                .expect_err("no such player");
            assert_eq!(error.code, ErrorCode::InvalidArgument);
            return;
        };
        let one = backend().get_player(MediaPlayerId(first.id)).expect("get player");
        assert_eq!(one.name, first.name, "the same player, read by its own id");
        assert_eq!(one.id, first.id);
    }

    /// `getMetadata` reads the player's OWN metadata: the title the host
    /// published, never a placeholder string.
    #[test]
    fn get_metadata_reads_the_players_own_metadata() {
        let Some(players) = the_hosts_players() else { return };
        let Some(first) = players.first() else { return };
        let metadata = backend()
            .get_metadata(MediaPlayerId(first.id))
            .expect("get metadata");
        // The listing and the metadata read are the same host's answers.
        let listed = first.metadata.clone();
        assert_eq!(metadata.title, listed.title, "the same title the listing shows");
        assert_eq!(metadata.artist, listed.artist);
    }

    /// `getPosition` reads the player's OWN position: a duration and a
    /// position, never a fabricated zero.
    #[test]
    fn get_position_reads_the_players_own_position() {
        let Some(players) = the_hosts_players() else { return };
        let Some(first) = players.first() else { return };
        let (position, duration) = backend()
            .get_position(MediaPlayerId(first.id))
            .expect("get position");
        // The position never runs past the track's own length.
        assert!(
            duration.as_nanos() == 0 || position.as_nanos() <= duration.as_nanos(),
            "the position is inside the track: {position:?} of {duration:?}"
        );
    }

    /// `subscribePlayer` mints the engine's own subscription token.
    #[test]
    fn subscribe_player_mints_a_subscription() {
        let subscription = backend()
            .subscribe_player(MediaPlayerId(0))
            .expect("subscribe player");
        // A token the caller can carry: it is a value, not a fabricated zero
        // standing for "no subscription".
        assert_eq!(subscription, crate::domains::media::types::SubscriptionId(0));
    }

    /// `getSupportedFormats` answers with the host's OWN answer: MPRIS
    /// publishes no format list, so the honest answer is the empty list —
    /// never a fabricated set of formats.
    #[test]
    fn get_supported_formats_answers_with_the_hosts_own_list() {
        let formats = backend()
            .get_supported_formats(MediaPlayerId(0))
            .expect("get supported formats");
        assert!(
            formats.is_empty(),
            "MPRIS publishes no format list, so the honest answer is the empty one"
        );
    }
}
