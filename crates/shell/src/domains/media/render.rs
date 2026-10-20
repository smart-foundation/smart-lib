use crate::selector::Value;
use crate::OutputFormat;
use super::MediaCmd;
pub fn render_action(_cmd: &MediaCmd, value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => format!("  🎬 Media — {}", value.display()),
    }
}
