use std::io::{self, BufRead, Write};

pub fn confirm(prompt: &str) -> bool {
    print!("{} [y/N]: ", prompt);
    if let Err(_) = io::stdout().flush() {
        return false;
    }

    let mut input = String::new();
    let stdin = io::stdin();

    if stdin.lock().read_line(&mut input).is_err() {
        return false;
    }

    let input = input.trim().to_lowercase();
    input.is_empty() || input == "y" || input == "yes"
}

pub fn confirm_typed(prompt: &str) -> bool {
    print!("{} (type 'yes' to confirm): ", prompt);
    if let Err(_) = io::stdout().flush() {
        return false;
    }

    let mut input = String::new();
    let stdin = io::stdin();

    if stdin.lock().read_line(&mut input).is_err() {
        return false;
    }

    input.trim() == "yes"
}

pub fn confirm_or_cancel(prompt: &str) -> Result<(), &'static str> {
    if confirm(prompt) {
        Ok(())
    } else {
        Err("cancelled")
    }
}

pub fn confirm_typed_or_cancel(prompt: &str) -> Result<(), &'static str> {
    if confirm_typed(prompt) {
        Ok(())
    } else {
        Err("cancelled")
    }
}
/// The host-write confirmation (charter law #8): a mutating verb proceeds
/// only when the operator confirms at a terminal. Non-interactive input (a
/// pipe, a script) never confirms — the verb refuses instead.
pub fn confirm_host_write(verb: &str, target: &str) -> bool {
    if !crate::color::is_tty() {
        return false;
    }
    let what = if target.is_empty() {
        verb.to_string()
    } else {
        format!("{verb} ({target})")
    };
    confirm(&format!(
        "  ⚠ {what} changes host state. Continue?"
    ))
}
