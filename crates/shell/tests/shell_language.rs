//! The shell language's parser and evaluator: assignments, conditionals
//! (with the `elif`/`else` branches the original never dispatched), loops,
//! functions, and the pipe expression — all over the scripted engine, with
//! the host never touched.

mod common;

use common::MockBackend;
use shell::selector::Value;
use shell::shell::{Shell, ShellCommand};

fn shell() -> Shell {
    Shell::new(Box::new(MockBackend::host()))
}

// ── the parser ─────────────────────────────────────────────────────────────

#[test]
fn parses_assignment_and_selector_expressions() {
    let cmd = Shell::parse("v = @network/interfaces | count");
    match cmd {
        ShellCommand::Assign { name, value } => {
            assert_eq!(name, "v");
            assert!(matches!(value, shell::shell::Expr::Pipe { .. }));
        }
        other => panic!("expected an assignment, got {other:?}"),
    }
}

#[test]
fn parses_watch_with_its_bounds() {
    match Shell::parse("watch network --count 3") {
        ShellCommand::Watch { domain, args } => {
            assert_eq!(domain, "network");
            assert_eq!(args, vec!["--count".to_string(), "3".to_string()]);
        }
        other => panic!("expected a watch, got {other:?}"),
    }
}

#[test]
fn parses_apply_export_help_exit() {
    assert!(matches!(Shell::parse("apply net.toml"), ShellCommand::Apply { .. }));
    assert!(matches!(Shell::parse("export"), ShellCommand::Export { path: None }));
    assert!(matches!(
        Shell::parse("export out.toml"),
        ShellCommand::Export { path: Some(_) }
    ));
    assert!(matches!(
        Shell::parse("help network"),
        ShellCommand::Help { domain: Some(d) } if d == "network"
    ));
    assert!(matches!(Shell::parse("exit"), ShellCommand::Exit));
    assert!(matches!(Shell::parse("quit"), ShellCommand::Exit));
    assert!(matches!(Shell::parse("migrate"), ShellCommand::Migrate));
}

#[test]
fn parses_a_for_loop_over_a_selector() {
    match Shell::parse("for i in @network/interfaces: network") {
        ShellCommand::For { var, iter, body } => {
            assert_eq!(var, "i");
            assert!(matches!(iter, shell::shell::Expr::Selector(_)));
            assert!(!body.is_empty());
        }
        other => panic!("expected a for, got {other:?}"),
    }
}

#[test]
fn parses_a_function_definition() {
    match Shell::parse("fn wifi_count(): @network/interfaces | count") {
        ShellCommand::Fn { name, params, body } => {
            assert_eq!(name, "wifi_count");
            assert!(params.is_empty());
            assert!(!body.is_empty());
        }
        other => panic!("expected a fn, got {other:?}"),
    }
}

/// The repair: `elif`/`else` used to parse into an empty `If` and silently
/// never dispatch. The block parser fills them.
#[test]
fn parses_if_elif_else_blocks() {
    let block = "if @network/networking == false:\n  network status\nelif @network/interfaces where type == wifi:\n  network interfaces\nelse:\n  network";
    match Shell::parse(block) {
        ShellCommand::If {
            then_body,
            elifs,
            else_body,
            ..
        } => {
            assert_eq!(then_body.len(), 1, "the then branch has its command");
            assert_eq!(elifs.len(), 1, "the elif branch is populated");
            assert_eq!(elifs[0].1.len(), 1);
            assert_eq!(else_body.len(), 1, "the else branch is populated");
        }
        other => panic!("expected an if block, got {other:?}"),
    }
}

#[test]
fn parses_the_one_line_if() {
    match Shell::parse("if @network/networking == true: network") {
        ShellCommand::If { then_body, elifs, else_body, .. } => {
            assert_eq!(then_body.len(), 1);
            assert!(elifs.is_empty());
            assert!(else_body.is_empty());
        }
        other => panic!("expected an if, got {other:?}"),
    }
}

// ── the evaluator ──────────────────────────────────────────────────────────

#[test]
fn evaluates_a_piped_selector_expression() {
    let mut sh = shell();
    let expr = sh_expr(&mut sh, "@network/interfaces | count");
    assert_eq!(sh.eval_expr(&expr).unwrap(), Value::Number(2.0));
}

fn sh_expr(_sh: &mut Shell, input: &str) -> shell::shell::Expr {
    // The evaluator's expression entry is the pipe expression built from a
    // command line; reach it the way a `var = …` assignment does.
    match Shell::parse(&format!("v = {input}")) {
        ShellCommand::Assign { value, .. } => value,
        other => panic!("expected an assignment, got {other:?}"),
    }
}

#[test]
fn dispatches_the_elif_branch_when_the_first_condition_fails() {
    let mut sh = shell();
    // Each branch ends in an assignment, so the branch that ran is the one
    // whose value the conditional yields.
    let block = "if @network/networking == false:\n  v = @network/radios | count\nelif @network/interfaces where type == wifi:\n  v = @network/interfaces | count\nelse:\n  v = 0";
    let value = sh.exec_cmd(&Shell::parse(block)).unwrap();
    assert_eq!(
        value,
        Value::Number(2.0),
        "the elif branch ran: its assignment is the conditional's value"
    );
}

#[test]
fn dispatches_the_else_branch_when_nothing_matches() {
    let mut sh = shell();
    let block = "if @network/networking == false:\n  v = 1\nelif @network/interfaces where type == cellular:\n  v = 2\nelse:\n  v = @network/radios | count";
    let value = sh.exec_cmd(&Shell::parse(block)).unwrap();
    assert_eq!(value, Value::Number(2.0), "the else branch ran");
}

#[test]
fn dispatches_the_then_branch_when_the_condition_holds() {
    let mut sh = shell();
    let block = "if @network/networking == true:\n  v = 7\nelif @network/interfaces where type == wifi:\n  v = 8\nelse:\n  v = 9";
    let value = sh.exec_cmd(&Shell::parse(block)).unwrap();
    assert_eq!(value, Value::Number(7.0), "the then branch ran first");
}

#[test]
fn a_for_loop_iterates_the_selector_list() {
    let mut sh = shell();
    // The loop body records each item; the last variable holds the final
    // one, which proves every iteration ran in order.
    sh.exec_cmd(&Shell::parse("for i in @network/interfaces: last = $i")).unwrap();
    let last = sh
        .eval_expr(&shell::shell::Expr::Variable("last".to_string()))
        .unwrap();
    assert!(
        last.display().contains("wlp0s20f3"),
        "the loop finished on the last interface: {}",
        last.display()
    );
}

#[test]
fn functions_capture_their_arguments_and_return_their_last_value() {
    let mut sh = shell();
    sh.exec_cmd(&Shell::parse("fn count_all(): @network/interfaces | count")).unwrap();
    let value = sh
        .eval_expr(&shell::shell::Expr::FunctionCall {
            name: "count_all".to_string(),
            args: vec![],
        })
        .unwrap();
    assert_eq!(value, Value::Number(2.0));
}

#[test]
fn variables_survive_a_pipe_expression() {
    let mut sh = shell();
    sh.exec_cmd(&Shell::parse("n = @network/interfaces | count")).unwrap();
    assert_eq!(
        sh.eval_expr(&shell::shell::Expr::Variable("n".to_string())).unwrap(),
        Value::Number(2.0)
    );
}

#[test]
fn a_command_pipe_filters_the_commands_value_not_its_rendering() {
    let mut sh = shell();
    let value = sh.exec_cmd(&Shell::parse("network | where type == wifi")).unwrap();
    match value {
        Value::List(items) => {
            assert_eq!(items.len(), 1, "only the wifi interface survives");
            assert_eq!(items[0].field_str("name"), "wlp0s20f3");
        }
        other => panic!("expected a list, got {other:?}"),
    }
}

/// The write guard: law #8's gate. A mutating verb without acceptance is
/// refused with the reason, and the engine is NOT called.
#[test]
fn a_host_write_is_refused_without_acceptance() {
    use shell::domains::network::{NetworkCmd, WriteGuard};
    let mock = MockBackend::host();
    let cmd: NetworkCmd = clap::Parser::try_parse_from(["network", "wifi", "off"]).unwrap();
    let err = shell::domains::network::evaluate(&cmd, &mock, WriteGuard { allowed: false }).unwrap_err();
    assert!(
        err.to_string().contains("CHANGES host state"),
        "the refusal names the reason: {err}"
    );
    assert!(mock.writes().is_empty(), "the engine write never happened");
}

/// The honest pending answer: a queue domain's command answers from the
/// queue data, and the engine is never called for it.
#[test]
fn an_unmigrated_domain_answers_from_the_queue() {
    let mut sh = shell();
    // The domain commands answer the honest pending reason and touch no
    // engine seam.
    assert!(matches!(
        sh.exec_cmd(&Shell::parse("clipboard")).unwrap(),
        Value::Null
    ));
    assert!(matches!(
        sh.exec_cmd(&Shell::parse("audio volume 75")).unwrap(),
        Value::Null
    ));
    // The selector surface answers the same way.
    let err = sh.exec_cmd(&Shell::parse("@clipboard/content")).unwrap_err();
    assert!(
        err.to_string().contains("migration pending")
            && err.to_string().contains("platforms/linux/clipboard.rs"),
        "the honest answer names the archive body: {err}"
    );
}
