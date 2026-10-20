use clap::{Parser, Subcommand};
use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;
use smart_lib::domains::media::MediaBackend;
use smart_lib::domains::media::types::{MediaPlayerId, Pagination};

pub type Backend = dyn MediaBackend;

fn all() -> Pagination { Pagination { offset: 0, limit: u32::MAX } }

#[derive(Parser, Debug)]
pub struct MediaCmd {
    #[command(subcommand)]
    pub action: Option<MediaAction>,
}
#[derive(Subcommand, Debug)]
pub enum MediaAction {
    Players,
    Player { id: u64 },
    Play { id: u64 },
    Pause { id: u64 },
    Stop { id: u64 },
    Next { id: u64 },
    Previous { id: u64 },
    Seek { id: u64, position_ms: u64 },
    Metadata { id: u64 },
    Position { id: u64 },
    Volume { id: u64, value: f32 },
    SetPosition { id: u64, position_ms: u64 },
    OpenUri { id: u64, uri: String },
    Formats { id: u64 },
    Caps,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteGuard { pub allowed: bool }

pub fn evaluate(cmd: &MediaCmd, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    use smart_lib::core::values::Duration;
    match &cmd.action {
        None | Some(MediaAction::Players) => {
            let v = backend.list_players(all()).map_err(SmartError::from)?;
            Ok(Value::List(v.into_iter().map(|p| Value::Object(vec![
                ("id".into(), Value::Number(p.id as f64)),
                ("name".into(), Value::String(p.name)),
                ("identity".into(), Value::String(p.identity)),
                ("status".into(), Value::String(format!("{:?}", p.status))),
                ("volume".into(), Value::Number(p.volume as f64)),
            ])).collect()))
        }
        Some(MediaAction::Player { id }) => {
            let p = backend.get_player(MediaPlayerId(*id)).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("id".into(), Value::Number(p.id as f64)),
                ("name".into(), Value::String(p.name)),
                ("identity".into(), Value::String(p.identity)),
                ("status".into(), Value::String(format!("{:?}", p.status))),
                ("volume".into(), Value::Number(p.volume as f64)),
            ]))
        }
        Some(MediaAction::Play { id }) => { guard_host_write(guard, &format!("media play {id}"))?; backend.play(MediaPlayerId(*id)).map_err(SmartError::from)?; Ok(Value::Object(vec![("verb".into(),Value::String("play".into())),("id".into(),Value::Number(*id as f64))])) }
        Some(MediaAction::Pause { id }) => { guard_host_write(guard, &format!("media pause {id}"))?; backend.pause(MediaPlayerId(*id)).map_err(SmartError::from)?; Ok(Value::Object(vec![("verb".into(),Value::String("pause".into())),("id".into(),Value::Number(*id as f64))])) }
        Some(MediaAction::Stop { id }) => { guard_host_write(guard, &format!("media stop {id}"))?; backend.stop(MediaPlayerId(*id)).map_err(SmartError::from)?; Ok(Value::Object(vec![("verb".into(),Value::String("stop".into())),("id".into(),Value::Number(*id as f64))])) }
        Some(MediaAction::Next { id }) => { guard_host_write(guard, &format!("media next {id}"))?; backend.next(MediaPlayerId(*id)).map_err(SmartError::from)?; Ok(Value::Object(vec![("verb".into(),Value::String("next".into())),("id".into(),Value::Number(*id as f64))])) }
        Some(MediaAction::Previous { id }) => { guard_host_write(guard, &format!("media previous {id}"))?; backend.previous(MediaPlayerId(*id)).map_err(SmartError::from)?; Ok(Value::Object(vec![("verb".into(),Value::String("previous".into())),("id".into(),Value::Number(*id as f64))])) }
        Some(MediaAction::Seek { id, position_ms }) => { guard_host_write(guard, &format!("media seek {id}"))?; backend.seek(MediaPlayerId(*id), Duration::from_nanos(*position_ms as u64 * 1_000_000)).map_err(SmartError::from)?; Ok(Value::Object(vec![("verb".into(),Value::String("seek".into())),("id".into(),Value::Number(*id as f64))])) }
        Some(MediaAction::Metadata { id }) => {
            let m = backend.get_metadata(MediaPlayerId(*id)).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("title".into(), Value::String(m.title)),
                ("artist".into(), Value::String(m.artist)),
                ("album".into(), Value::String(m.album)),
                ("art_uri".into(), Value::String(m.art_uri)),
            ]))
        }
        Some(MediaAction::Position { id }) => {
            let (pos, dur) = backend.get_position(MediaPlayerId(*id)).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("position_ns".into(), Value::Number(pos.as_nanos() as f64)),
                ("duration_ns".into(), Value::Number(dur.as_nanos() as f64)),
            ]))
        }
        Some(MediaAction::Volume { id, value }) => {
            guard_host_write(guard, &format!("media volume {id}"))?;
            backend.set_volume(MediaPlayerId(*id), *value).map_err(SmartError::from)?;
            Ok(Value::Object(vec![("verb".into(),Value::String("volume".into())),("id".into(),Value::Number(*id as f64)),("value".into(),Value::Number(*value as f64))]))
        }
        Some(MediaAction::SetPosition { id, position_ms }) => {
            guard_host_write(guard, &format!("media set-position {id}"))?;
            backend.set_position(MediaPlayerId(*id), Duration::from_nanos(*position_ms as u64 * 1_000_000)).map_err(SmartError::from)?;
            Ok(Value::Object(vec![("verb".into(),Value::String("set-position".into())),("id".into(),Value::Number(*id as f64))]))
        }
        Some(MediaAction::OpenUri { id, uri }) => {
            guard_host_write(guard, &format!("media open-uri {id}"))?;
            backend.open_uri(MediaPlayerId(*id), uri.clone()).map_err(SmartError::from)?;
            Ok(Value::Object(vec![("verb".into(),Value::String("open-uri".into())),("id".into(),Value::Number(*id as f64)),("uri".into(),Value::String(uri.clone()))]))
        }
        Some(MediaAction::Formats { id }) => {
            let f = backend.get_supported_formats(MediaPlayerId(*id)).map_err(SmartError::from)?;
            Ok(Value::List(f.into_iter().map(Value::String).collect()))
        }
        Some(MediaAction::Caps) => Ok(Value::List(vec![Value::String("media.read".into()), Value::String("media.write".into())])),
    }
}

pub fn run(cmd: &MediaCmd, format: &OutputFormat, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    let value = evaluate(cmd, backend, guard)?;
    if *format != OutputFormat::Raw { println!("{}", render::render_action(cmd, &value, *format)); }
    Ok(value)
}

pub fn write_intent(cmd: &MediaCmd) -> Option<(String, String)> {
    match &cmd.action {
        Some(MediaAction::Play { id }) => Some(("play".into(), format!("player {id}"))),
        Some(MediaAction::Pause { id }) => Some(("pause".into(), format!("player {id}"))),
        Some(MediaAction::Stop { id }) => Some(("stop".into(), format!("player {id}"))),
        Some(MediaAction::Next { id }) => Some(("next".into(), format!("player {id}"))),
        Some(MediaAction::Previous { id }) => Some(("previous".into(), format!("player {id}"))),
        Some(MediaAction::Seek { id, .. }) => Some(("seek".into(), format!("player {id}"))),
        Some(MediaAction::Volume { id, value }) => Some(("set volume of".into(), format!("player {id} to {value}"))),
        Some(MediaAction::SetPosition { id, .. }) => Some(("set position of".into(), format!("player {id}"))),
        Some(MediaAction::OpenUri { id, uri }) => Some(("open uri on".into(), format!("player {id}: {uri}"))),
        _ => None,
    }
}

pub mod render;
fn guard_host_write(guard: WriteGuard, verb: &str) -> SmartResult<()> {
    if guard.allowed { return Ok(()); }
    Err(SmartError::not_supported(format!("refusing to run `{verb}`: it CHANGES host state. In the REPL confirm the prompt, or pass --force once you accept the write (the live bring-back proof is founder-run).")))
}
pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    let players = backend.list_players(all()).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("players".into(), Value::List(players.into_iter().map(|p| Value::Object(vec![
            ("id".into(), Value::Number(p.id as f64)),
            ("name".into(), Value::String(p.name)),
            ("identity".into(), Value::String(p.identity)),
        ])).collect())),
        ("caps".into(), Value::List(vec![Value::String("media.read".into()), Value::String("media.write".into())])),
    ]))
}
