use crate::selector::Value;
use crate::OutputFormat;
use super::PowerCmd;
pub fn render_action(_cmd: &PowerCmd, value: &Value, format: OutputFormat) -> String {
    match format {
        OutputFormat::Json => crate::value::to_json(value),
        OutputFormat::Raw => value.display(),
        OutputFormat::Human => format!("  🔋 Power — {}", value.display()),
    }
}
