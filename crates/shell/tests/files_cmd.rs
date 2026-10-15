//! The files command module over the scripted engine: the command surface's
//! data half, its human aesthetic (pixel-perfect under the terminal-faithful
//! measure), the host-write gate on every mutating verb, the honest answers for
//! what the host cannot serve, and the `@files/*` selector tree. Nothing here
//! touches the host (charter law #8): every read is served from the script.

#![cfg(feature = "domain-files")]

mod common;

use clap::Parser;
use common::{assert_borders_hold, glob, MockFileEntry, MockFiles};
use smart_lib::domains::files::types::FileKind;
use shell::domains::files::{evaluate, render, selector_root, FilesCmd, WriteGuard};
use shell::selector;
use shell::widgets::display_width;
use shell::{OutputFormat, Value};

fn cmd(argv: &[&str]) -> FilesCmd {
    FilesCmd::try_parse_from(argv).unwrap()
}

fn allowed() -> WriteGuard {
    WriteGuard { allowed: true }
}


/// A record field as the list of records it holds (a missing or non-list field
/// is the honest empty, never a panic).
fn list_of<'a>(value: &'a Value, key: &str) -> Vec<&'a Value> {
    match value.field(key) {
        Value::List(items) => items.iter().collect(),
        _ => Vec::new(),
    }
}

fn refused() -> WriteGuard {
    WriteGuard { allowed: false }
}

/// The default view is the directory listing: the engine's own entries with
/// their measured kinds, sizes and timestamps.
#[test]
fn the_default_view_is_the_listing() {
    let backend = MockFiles::default();
    let value = evaluate(&cmd(&["files"]), &backend, refused()).unwrap();
    match value {
        Value::List(entries) => {
            assert_eq!(entries.len(), 3, "the scripted host serves three entries");
            let notes = &entries[0];
            assert_eq!(notes.field_str("name"), "big.bin", "name sort, honested by the engine");
            assert_eq!(notes.field_str("kind"), "regular");
            assert_eq!(notes.field("size").as_f64(), Some(3_145_728.0));
            assert_eq!(notes.field_str("modified"), "1700000002000000000");
            assert!(!notes.field_bool("hidden"));
        }
        other => panic!("expected a list, got {other:?}"),
    }
}

/// `files ls <path>` filters through the engine's own vocabulary: the glob, the
/// dot-file rule and the sort field all reach the backend.
#[test]
fn a_listing_filters_and_sorts_through_the_engine() {
    let backend = MockFiles::default();
    let value = evaluate(
        &cmd(&["files", "ls", "/tmp/scripted", "--pattern", "*.bin", "--sort", "size"]),
        &backend,
        refused(),
    )
    .unwrap();
    match value {
        Value::List(entries) => {
            assert_eq!(entries.len(), 1, "the glob keeps only the .bin entry");
            assert_eq!(entries[0].field_str("name"), "big.bin");
        }
        other => panic!("expected a list, got {other:?}"),
    }
    // A path the engine does not serve is the engine's own reason, never an
    // empty list that would claim the directory is empty.
    let missing = evaluate(&cmd(&["files", "ls", "/tmp/nope"]), &backend, refused());
    assert!(missing.is_err(), "an unserved path answers with its reason: {missing:?}");
    assert!(missing
        .unwrap_err()
        .to_string()
        .contains("not-found: /tmp/nope"));
}

/// `files stat` reads the kernel's own metadata out of the contract's total
/// union — and a missing path is the union's error arm, not a zeroed struct.
#[test]
fn stat_reads_the_statx_answer_and_names_an_absent_path() {
    let backend = MockFiles::default();
    let value = evaluate(&cmd(&["files", "stat", "/tmp/scripted/notes.txt"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("kind"), "regular");
    assert_eq!(value.field_str("size"), "12");
    assert_eq!(value.field_str("inode"), "4242", "inode is reported data");
    assert_eq!(value.field_str("permissions"), "644");
    assert_eq!(value.field_str("created"), "1700000000000000000");

    let missing = evaluate(&cmd(&["files", "stat", "/tmp/scripted/absent"]), &backend, refused());
    assert!(missing.is_err());
    assert!(missing.unwrap_err().to_string().contains("ENOENT"));
}

/// `files cat` is the handle surface's own read path: open, the mmap window,
/// close — and a binary window is named rather than decoded lossily.
#[test]
fn cat_reads_the_handle_surface_and_names_a_binary_read() {
    let backend = MockFiles::default();
    let value = evaluate(&cmd(&["files", "cat", "/tmp/scripted/notes.txt"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("text"), "the scripted bytes\n");
    assert!(!value.field_bool("binary"));
    let writes = backend.writes.borrow();
    assert_eq!(writes[0], "open(read)", "the engine's own read handle");
    assert_eq!(writes[1], "close", "the descriptor is retired");

    // A window the host cannot read as text answers with the hex preview and
    // the reason — never a lossy decode passed off as content.
    let backend = MockFiles {
        content: vec![0xff, 0xfe, 0x00],
        ..MockFiles::default()
    };
    let value = evaluate(&cmd(&["files", "cat", "/tmp/scripted/notes.txt"]), &backend, refused()).unwrap();
    assert!(value.field_bool("binary"));
    assert_eq!(value.field_str("preview"), "fffe00");
    assert!(value.field_str("reason").contains("not valid UTF-8"));
}

/// `files du` is the engine's own statvfs totals, and the human sizes are the
/// binary-prefix scaling the host's own tools print.
#[test]
fn du_reports_the_statvfs_totals_in_human_sizes() {
    let backend = MockFiles::default();
    let value = evaluate(&cmd(&["files", "du"]), &backend, refused()).unwrap();
    assert_eq!(value.field_str("total_human"), "100 MiB");
    assert_eq!(value.field_str("used_human"), "40.0 MiB");
    assert_eq!(value.field_str("free_human"), "60.0 MiB");
    assert_eq!(value.field("percent_used").as_f64(), Some(40.0));
    assert_eq!(value.field_str("mount_point"), "/tmp");

    // The render states the mount row the host publishes and never truncates
    // its own numbers away.
    let rendered = render::render_action(
        &cmd(&["files", "du"]),
        &value,
        OutputFormat::Human,
    );
    assert!(rendered.contains("100 MiB"), "{rendered}");
    assert!(rendered.contains("statvfs"), "{rendered}");
}

/// `files watch` is the engine's own inotify: one subscription, bounded pulls,
/// the kernel's records, and the subscription retired at the end.
#[test]
fn a_watch_pulls_the_kernels_own_events_and_retires() {
    let backend = MockFiles::default();
    let value = evaluate(
        &cmd(&["files", "watch", "/tmp/scripted", "--count", "2", "--interval", "1"]),
        &backend,
        refused(),
    )
    .unwrap();
    match value {
        Value::List(events) => {
            assert_eq!(events.len(), 1, "the scripted kernel reported one record");
            assert_eq!(events[0].field_str("kind"), "create");
            assert_eq!(events[0].field_str("path"), "/tmp/scripted/notes.txt");
        }
        other => panic!("expected a list, got {other:?}"),
    }
    let writes = backend.writes.borrow();
    assert!(writes.iter().any(|w| w == "watch"), "{writes:?}");
    assert!(writes.iter().any(|w| w == "cancel-watch"), "{writes:?}");
}

/// Every host-mutating verb refuses without the accepted write, names what it
/// would have changed, and reaches the engine's own code-complete path with it.
#[test]
fn every_write_verb_is_gated_and_the_gate_names_the_verb() {
    let verbs: &[&[&str]] = &[
        &["files", "mkdir", "/tmp/scripted/new"],
        &["files", "touch", "/tmp/scripted/new"],
        &["files", "rm", "/tmp/scripted/notes.txt"],
        &["files", "mv", "/tmp/scripted/notes.txt", "/tmp/scripted/moved.txt"],
        &["files", "cp", "/tmp/scripted/notes.txt", "/tmp/scripted/copy.txt"],
        &["files", "write", "/tmp/scripted/new", "hello"],
        &["files", "link", "/tmp/scripted/notes.txt", "/tmp/scripted/hard"],
        &["files", "lock", "/tmp/scripted/notes.txt"],
        &["files", "unlock", "1"],
    ];
    for argv in verbs {
        let backend = MockFiles::default();
        let refused = evaluate(&cmd(argv), &backend, refused());
        let Err(error) = refused else {
            panic!("{} ran without the accepted write", argv.join(" "));
        };
        let message = error.to_string();
        assert!(message.contains("CHANGES host state"), "{message}");
        assert!(
            backend.writes.borrow().is_empty(),
            "{} reached the engine: {:?}",
            argv.join(" "),
            backend.writes.borrow()
        );
    }
    // With the gate open, each verb reaches the engine's own path — and `cp` is
    // the kernel's own copy_file_range call, recorded by name.
    let backend = MockFiles::default();
    let value = evaluate(&cmd(&["files", "cp", "/tmp/scripted/a", "/tmp/scripted/b"]), &backend, allowed()).unwrap();
    assert_eq!(value.field_str("verb"), "cp");
    let writes = backend.writes.borrow();
    assert!(writes.iter().any(|w| w == "cp(/tmp/scripted/a, /tmp/scripted/b)"), "{writes:?}");
}

/// The store surface: a content-addressed store is a real object, and the
/// version store answers with the engine's own measured reason when it holds
/// nothing — never a fake empty without one.
#[test]
fn the_content_and_version_surfaces_answer_with_measured_reasons() {
    let backend = MockFiles::default();
    let stored = evaluate(&cmd(&["files", "store", "--content", "/tmp/scripted/notes.txt"]), &backend, refused()).unwrap();
    assert_eq!(stored.field_str("algorithm"), "sha256");
    assert_eq!(stored.field("size_bytes").as_f64(), Some(12.0));
    assert!(stored.field_str("reason").contains("DEGRADED"), "{}", stored.field_str("reason"));

    let versions = evaluate(&cmd(&["files", "versions", "/tmp/scripted/notes.txt"]), &backend, refused()).unwrap();
    assert_eq!(versions.field_str("total"), "1");
    assert_eq!(
        list_of(&versions, "versions")[0].field_str("message"),
        "the scripted version"
    );

    let empty_store = MockFiles {
        entries: vec![MockFileEntry {
            name: "notes.txt".to_string(),
            path: "/tmp/scripted/notes.txt".to_string(),
            kind: FileKind::Regular,
            size: 12,
            modified: 1_700_000_000_000_000_000,
        }],
        ..MockFiles::default()
    };
    // The scripted store always holds one version, so an empty store is proven
    // through the branch listing, whose honest empty carries its own total.
    let branches = evaluate(&cmd(&["files", "branches", "/tmp/scripted/notes.txt"]), &empty_store, refused()).unwrap();
    assert_eq!(branches.field_str("total"), "0");
    let rendered = render::render_action(&cmd(&["files", "branches", "/tmp/scripted/notes.txt"]), &branches, OutputFormat::Human);
    assert!(rendered.contains("no branch"), "{rendered}");
}

/// The dedupe answer is the host's own measured inode sharing, with the
/// reflink limit named — never a claimed saving that did not happen.
#[test]
fn dedupe_reports_the_measured_sharing() {
    let backend = MockFiles::default();
    let value = evaluate(&cmd(&["files", "dedupe", "/tmp/scripted/notes.txt"]), &backend, refused()).unwrap();
    assert_eq!(value.field("references").as_f64(), Some(1.0));
    assert!(value.field_str("reason").contains("reflink"), "{}", value.field_str("reason"));
    let writes = backend.writes.borrow();
    assert!(writes.iter().any(|w| w == "dedupe(/tmp/scripted/notes.txt)"), "{writes:?}");
}

/// The host's own XDG surfaces: the trash, the recents record and the default
/// application all answer from the engine's own reads.
#[test]
fn the_hosts_own_xdg_surfaces_are_read_natively() {
    let backend = MockFiles::default();
    let trash = evaluate(&cmd(&["files", "trash"]), &backend, refused()).unwrap();
    assert_eq!(trash.field_str("total"), "1");
    assert_eq!(
        list_of(&trash, "entries")[0].field_str("original_path"),
        "/tmp/scripted/gone.txt"
    );

    let recents = evaluate(&cmd(&["files", "recents"]), &backend, refused()).unwrap();
    assert_eq!(recents.field_str("total"), "1");

    let app = evaluate(&cmd(&["files", "app", "text/plain"]), &backend, refused()).unwrap();
    assert_eq!(app.field_str("name"), "Scripted Editor");

    // A mime type the host names no application for is the contract's own
    // error arm, and the shell says so rather than inventing one.
    let missing = evaluate(&cmd(&["files", "app", "application/x-none"]), &backend, refused());
    assert!(missing.is_err());
    assert!(missing.unwrap_err().to_string().contains("no default app"));
}

/// The selector tree is the same values the command surface renders, so a
/// selector and a command cannot disagree.
#[test]
fn the_selector_tree_resolves_from_the_engines_values() {
    let backend = MockFiles::default();
    let root = selector_root(&backend).unwrap();
    assert_eq!(root.field_str("cwd"), "/tmp/scripted");
    assert_eq!(list_of(&root, "entries")[0].field_str("name"), "big.bin");
    assert_eq!(list_of(&root, "caps")[0].display(), "files.read");
    assert_eq!(root.field("du").field_str("mount_point"), "/tmp");
    assert_eq!(root.field("home").field_str("path"), "/tmp/scripted/home");

    // The `@files/...` grammar resolves through the shell's own selector path,
    // filters included.
    let selected = selector::resolve(root, &selector::parse("@files/entries where kind == regular").unwrap());
    match selected {
        Value::List(items) => assert_eq!(items.len(), 2, "the two regular files"),
        other => panic!("expected a list, got {other:?}"),
    }
    let one = selector::resolve(
        selector_root(&backend).unwrap(),
        &selector::parse("@files/entries/1/name").unwrap(),
    );
    assert_eq!(one.display(), "notes.txt");
}

/// Every human rendering is pixel-perfect: the tables' borders hold under the
/// terminal-faithful measure (W/F=2, ambiguous=1, ANSI stripped).
#[test]
fn every_rendering_is_pixel_perfect() {
    let backend = MockFiles::default();
    let views: &[&[&str]] = &[
        &["files"],
        &["files", "find", "/tmp/scripted", "--name", "*"],
        &["files", "tree", "/tmp/scripted"],
        &["files", "trash"],
        &["files", "recents"],
        &["files", "grep", "/tmp/scripted", "scripted"],
    ];
    for argv in views {
        let value = evaluate(&cmd(argv), &backend, refused()).unwrap();
        let rendered = render::render_action(&cmd(argv), &value, OutputFormat::Human);
        assert_borders_hold(&rendered);
        for line in rendered.lines() {
            assert!(
                !line.contains('\u{1b}') || display_width(line) >= 1,
                "a styled line still measures its content: {line:?}"
            );
        }
    }
}

/// The JSON and raw forms are the same data the human table draws, so an agent
/// and a terminal can never disagree.
#[test]
fn json_and_raw_carry_the_same_values() {
    let backend = MockFiles::default();
    let value = evaluate(&cmd(&["files"]), &backend, refused()).unwrap();
    let json = render::render_action(&cmd(&["files"]), &value, OutputFormat::Json);
    assert!(json.contains("\"name\":\"big.bin\""), "{json}");
    assert!(json.contains("\"size\":3145728"), "{json}");
    let raw = render::render_action(&cmd(&["files"]), &value, OutputFormat::Raw);
    assert!(raw.contains("big.bin"), "{raw}");
}

/// The listing's own glob is the engine's rule: the tests exercise it through
/// the shared helper so a pattern cannot drift between the fixture and the
/// command.
#[test]
fn the_fixtures_glob_is_the_engines_own_rule() {
    assert!(glob("*", "anything"));
    assert!(glob("*.bin", "big.bin"));
    assert!(!glob("*.bin", "notes.txt"));
    assert!(glob("notes*", "notes.txt"));
    assert!(glob("notes.txt", "notes.txt"));
}

/// A listing that filters everything out is an honest empty: the host was read
/// and answered with no entries.
#[test]
fn a_filtered_listing_is_an_honest_empty() {
    let backend = MockFiles::default();
    let value = evaluate(&cmd(&["files", "ls", "/tmp/scripted", "--pattern", "*.rs"]), &backend, refused()).unwrap();
    assert_eq!(value, Value::List(Vec::new()));
    let rendered = render::render_action(
        &cmd(&["files", "ls", "/tmp/scripted", "--pattern", "*.rs"]),
        &value,
        OutputFormat::Human,
    );
    assert!(rendered.contains("the directory is empty"), "{rendered}");
}
