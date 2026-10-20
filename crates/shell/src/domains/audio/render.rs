use crate::error::{SmartError, SmartResult};
use crate::selector::Value;
use crate::OutputFormat;

use smart_lib::domains::audio::AudioBackend;
use smart_lib::domains::audio::types::{AudioDevice, AudioState, AudioStream};

use super::AudioCmd;

pub fn state_value(backend: &dyn AudioBackend) -> SmartResult<Value> {
    let s = backend.get_state().map_err(SmartError::from)?;
    Ok(state_value_from(&s))
}

pub fn state_value_from(s: &AudioState) -> Value {
    Value::Object(vec![
        ("volume".into(), Value::Number(s.volume as f64)),
        ("mute".into(), Value::Bool(s.mute)),
        ("default_output".into(), Value::Number(s.default_output as f64)),
        ("default_input".into(), Value::Number(s.default_input as f64)),
    ])
}

pub fn devices_value(backend: &dyn AudioBackend) -> SmartResult<Value> {
    let page = backend.list_devices(super::all()).map_err(SmartError::from)?;
    Ok(Value::List(page.items.iter().map(device_value).collect()))
}

pub fn device_value(d: &AudioDevice) -> Value {
    Value::Object(vec![
        ("id".into(), Value::Number(d.id as f64)),
        ("name".into(), Value::String(d.name.clone())),
        ("description".into(), Value::String(d.description.clone())),
        ("type".into(), Value::String(format!("{:?}", d.device_type))),
        ("volume".into(), Value::Number(d.volume as f64)),
        ("mute".into(), Value::Bool(d.mute)),
        ("default".into(), Value::Bool(d.is_default)),
        ("plugged_in".into(), Value::Bool(d.plugged_in)),
    ])
}

pub fn streams_value(backend: &dyn AudioBackend) -> SmartResult<Value> {
    let page = backend.list_streams(super::all()).map_err(SmartError::from)?;
    Ok(Value::List(page.items.iter().map(stream_value).collect()))
}

pub fn stream_value(s: &AudioStream) -> Value {
    Value::Object(vec![
        ("id".into(), Value::Number(s.id as f64)),
        ("device".into(), Value::Number(s.device as f64)),
        ("name".into(), Value::String(s.name.clone())),
        ("volume".into(), Value::Number(s.volume as f64)),
        ("mute".into(), Value::Bool(s.mute)),
    ])
}

pub fn caps_value(backend: &dyn AudioBackend) -> Value {
    let _ = backend;
    Value::List(vec![Value::String("audio.read".into()), Value::String("audio.write".into())])
}

pub fn render_action(_cmd: &AudioCmd, value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => {
            let mut out = String::new();
            out.push_str(&format!("  🎵 Audio — {}\n", value.display()));
            out
        }
    }
}
