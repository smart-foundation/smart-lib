use clap::{Parser, Subcommand};

use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::domains::audio::AudioBackend;
use smart_lib::domains::audio::types::{AudioDeviceId, AudioStreamId, Pagination};

pub type Backend = dyn AudioBackend;

fn all() -> Pagination {
    Pagination { offset: 0, limit: u32::MAX }
}

#[derive(Parser, Debug)]
pub struct AudioCmd {
    #[command(subcommand)]
    pub action: Option<AudioAction>,
}

#[derive(Subcommand, Debug)]
pub enum AudioAction {
    /// The default sink's volume/mute and default devices.
    State,
    /// Every sink and source the host reports.
    Devices,
    /// The active playback streams (sink inputs).
    Streams,
    /// Set a device's volume (host-mutating, gated).
    Volume {
        device: u64,
        value: f32,
    },
    /// Set a device's mute (host-mutating, gated).
    Mute {
        device: u64,
        value: bool,
    },
    /// Set a stream's volume (host-mutating, gated).
    StreamVolume {
        stream: u64,
        value: f32,
    },
    /// Set a stream's mute (host-mutating, gated).
    StreamMute {
        stream: u64,
        value: bool,
    },
    /// This backend's honest capability tokens.
    Caps,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteGuard {
    pub allowed: bool,
}

pub fn evaluate(cmd: &AudioCmd, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    match &cmd.action {
        None | Some(AudioAction::State) => render::state_value(backend),
        Some(AudioAction::Devices) => render::devices_value(backend),
        Some(AudioAction::Streams) => render::streams_value(backend),
        Some(AudioAction::Volume { device, value }) => {
            guard_host_write(guard, &format!("audio volume {device}"))?;
            backend.set_volume(AudioDeviceId(*device), *value).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("volume".into())),
                ("device".into(), Value::Number(*device as f64)),
                ("value".into(), Value::Number(*value as f64)),
            ]))
        }
        Some(AudioAction::Mute { device, value }) => {
            guard_host_write(guard, &format!("audio mute {device}"))?;
            backend.set_mute(AudioDeviceId(*device), *value).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("mute".into())),
                ("device".into(), Value::Number(*device as f64)),
                ("value".into(), Value::Bool(*value)),
            ]))
        }
        Some(AudioAction::StreamVolume { stream, value }) => {
            guard_host_write(guard, &format!("audio stream-volume {stream}"))?;
            backend.set_stream_volume(AudioStreamId(*stream), *value).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("stream-volume".into())),
                ("stream".into(), Value::Number(*stream as f64)),
                ("value".into(), Value::Number(*value as f64)),
            ]))
        }
        Some(AudioAction::StreamMute { stream, value }) => {
            guard_host_write(guard, &format!("audio stream-mute {stream}"))?;
            backend.set_stream_mute(AudioStreamId(*stream), *value).map_err(SmartError::from)?;
            Ok(Value::Object(vec![
                ("verb".into(), Value::String("stream-mute".into())),
                ("stream".into(), Value::Number(*stream as f64)),
                ("value".into(), Value::Bool(*value)),
            ]))
        }
        Some(AudioAction::Caps) => Ok(render::caps_value(backend)),
    }
}

pub fn run(cmd: &AudioCmd, format: &OutputFormat, backend: &Backend, guard: WriteGuard) -> SmartResult<Value> {
    let value = evaluate(cmd, backend, guard)?;
    if *format != OutputFormat::Raw {
        println!("{}", render::render_action(cmd, &value, *format));
    }
    Ok(value)
}

pub fn write_intent(cmd: &AudioCmd) -> Option<(String, String)> {
    match &cmd.action {
        Some(AudioAction::Volume { device, value }) => Some(("set volume of".into(), format!("device {device} to {value}"))),
        Some(AudioAction::Mute { device, value }) => Some(("set mute of".into(), format!("device {device} to {value}"))),
        Some(AudioAction::StreamVolume { stream, value }) => Some(("set volume of".into(), format!("stream {stream} to {value}"))),
        Some(AudioAction::StreamMute { stream, value }) => Some(("set mute of".into(), format!("stream {stream} to {value}"))),
        _ => None,
    }
}

pub mod render;

fn guard_host_write(guard: WriteGuard, verb: &str) -> SmartResult<()> {
    if guard.allowed { return Ok(()); }
    Err(SmartError::not_supported(format!(
        "refusing to run `{verb}`: it CHANGES host state. In the REPL confirm the prompt, or pass --force once you accept the write (the live bring-back proof is founder-run)."
    )))
}

pub fn selector_root(backend: &Backend) -> SmartResult<Value> {
    let state = backend.get_state().map_err(SmartError::from)?;
    let devices = backend.list_devices(all()).map_err(SmartError::from)?;
    let streams = backend.list_streams(all()).map_err(SmartError::from)?;
    Ok(Value::Object(vec![
        ("state".into(), render::state_value_from(&state)),
        ("devices".into(), Value::List(devices.items.iter().map(render::device_value).collect())),
        ("streams".into(), Value::List(streams.items.iter().map(render::stream_value).collect())),
        ("caps".into(), render::caps_value(backend)),
    ]))
}
