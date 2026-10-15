//! The confirmation machinery — the founder's standard, made real.
//!
//! ## The standard
//!
//! A destructive verb asks **`delete <path>? (y/N)` with the CONTEXT shown** —
//! the type/size/mtime of the target, or the old→new of a move — and the
//! operator answers `y` or `n`. It NEVER asks the operator to type the name of
//! the thing being destroyed: "yes or no prompt not type the file ur deleting"
//! is the founder's wording, and the type-the-name variant (`confirm_typed`)
//! that used to sit here as dead code earns nothing against it — so it is
//! deleted, not revived.
//!
//! ## The three rules
//!
//! 1. **An empty answer is NO.** A bare Enter never means yes: the old
//!    `confirm` returned true for an empty line, which made every prompt a
//!    tripwire. Only an explicit `y` / `yes` (any case) proceeds.
//! 2. **Script mode never assumes yes.** A non-interactive caller with no
//!    piped answer line gets [`Answer::NoInput`], which the shell turns into
//!    the deterministic refusal (`not_supported`, a JSON answer under
//!    `--json`). A caller that pipes `y` gets that line as the answer — so
//!    `echo y | smart files rm …` is legal, and `smart … < /dev/null` is a
//!    refusal, never a silent success.
//! 3. **`--force` is the explicit acceptance.** It is documented on the flag
//!    and in every refusal message: it skips the prompt because the operator
//!    already said yes on the command line.

use std::io::{self, BufRead, IsTerminal, Write};

/// What an operator (or a piped line) answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// An explicit `y` / `yes`.
    Yes,
    /// An explicit `n`, a bare Enter, or anything that is not a yes.
    No,
    /// There was nobody to ask: stdin offered no line at all (EOF, /dev/null,
    /// a closed pipe). The caller's answer is a deterministic refusal, never
    /// a write.
    NoInput,
}

/// Whether an answer line means yes. Only an explicit `y`/`yes` does; an
/// empty line is a NO (the safe default), as is everything else.
pub fn answer_is_yes(line: &str) -> bool {
    matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Ask the question with the context beside it and read one line of answer.
/// The prompt is written to stdout and flushed before the read — but only when
/// there IS somebody to show it to: a non-interactive stdin (a pipe, a
/// redirection, `/dev/null`) gets no prompt, so a script's output stays exactly
/// what it asked for and a `-o json` answer is never prefixed with a question.
pub fn ask(question: &str) -> Answer {
    if io::stdin().is_terminal() {
        print!("  ⚠ {question} (y/N): ");
        let _ = io::stdout().flush();
    }
    let mut line = String::new();
    match io::stdin().lock().read_line(&mut line) {
        // EOF: no answer was offered — nobody to ask.
        Ok(0) => Answer::NoInput,
        Ok(_) => {
            if answer_is_yes(&line) {
                Answer::Yes
            } else {
                Answer::No
            }
        }
        Err(_) => Answer::NoInput,
    }
}

/// The destructive verb's question, answered as a plain bool: yes only when
/// the operator explicitly said yes (a piped `y` line counts; EOF does not).
pub fn confirm_destructive(question: &str) -> bool {
    matches!(ask(question), Answer::Yes)
}

/// The law-#8 host-write gate the `apply`/`export` reconciliation uses: a
/// mutating verb proceeds only when the operator confirms at a terminal.
/// Non-interactive input (a pipe, a script) never confirms — the answer is a
/// refusal, never a write.
pub fn confirm_host_write(verb: &str, target: &str) -> bool {
    let what = if target.is_empty() {
        verb.to_string()
    } else {
        format!("{verb} ({target})")
    };
    confirm_destructive(&format!("{what} changes host state. Continue"))
}
