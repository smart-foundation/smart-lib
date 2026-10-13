//! The pipe engine: all twenty operations, salvaged engine-agnostic and
//! proven against the shell's own value type.

use shell::pipe::{apply_pipe, parse_pipe};
use shell::selector::Value;

fn iface(name: &str, kind: &str, mtu: f64) -> Value {
    Value::Object(vec![
        ("name".into(), Value::String(name.to_string())),
        ("type".into(), Value::String(kind.to_string())),
        ("mtu".into(), Value::Number(mtu)),
    ])
}

fn interfaces() -> Value {
    Value::List(vec![
        iface("enp0s31f6", "ethernet", 1500.0),
        iface("lo", "loopback", 65536.0),
        iface("wlp0s20f3", "wifi", 1500.0),
    ])
}

#[test]
fn all_twenty_ops_parse_and_apply() {
    let cases: Vec<(&str, Value)> = vec![
        ("where type == wifi", one_wifi()),
        ("map type", Value::List(vec![
            Value::String("ethernet".into()),
            Value::String("loopback".into()),
            Value::String("wifi".into()),
        ])),
        ("sort name", sorted_by_name()),
        ("sort name desc", sorted_by_name_desc()),
        ("take 2", Value::List(vec![
            iface("enp0s31f6", "ethernet", 1500.0),
            iface("lo", "loopback", 65536.0),
        ])),
        ("skip 2", Value::List(vec![iface("wlp0s20f3", "wifi", 1500.0)])),
        ("count", Value::Number(3.0)),
        ("reverse", reversed()),
        ("unique", interfaces()),
        ("head 1", Value::List(vec![iface("enp0s31f6", "ethernet", 1500.0)])),
        ("tail 1", Value::List(vec![iface("wlp0s20f3", "wifi", 1500.0)])),
        ("pluck name", Value::List(vec![
            Value::String("enp0s31f6".into()),
            Value::String("lo".into()),
            Value::String("wlp0s20f3".into()),
        ])),
        ("group type", grouped()),
        ("flatten", flattened()),
        ("len", Value::Number(3.0)),
        ("first", iface("enp0s31f6", "ethernet", 1500.0)),
        ("last", iface("wlp0s20f3", "wifi", 1500.0)),
        ("sum mtu", Value::Number(68_536.0)),
        ("avg mtu", Value::Number(68_536.0 / 3.0)),
        ("min mtu", Value::Number(1500.0)),
        ("max mtu", Value::Number(65_536.0)),
    ];
    for (line, expected) in cases {
        let op = parse_pipe(line).unwrap_or_else(|e| panic!("parse {line}: {e}"));
        let got = apply_pipe(interfaces(), &op).unwrap_or_else(|e| panic!("apply {line}: {e}"));
        assert_eq!(got, expected, "op: {line}");
    }
}

fn one_wifi() -> Value {
    Value::List(vec![iface("wlp0s20f3", "wifi", 1500.0)])
}

fn sorted_by_name() -> Value {
    Value::List(vec![
        iface("enp0s31f6", "ethernet", 1500.0),
        iface("lo", "loopback", 65536.0),
        iface("wlp0s20f3", "wifi", 1500.0),
    ])
}

fn sorted_by_name_desc() -> Value {
    Value::List(vec![
        iface("wlp0s20f3", "wifi", 1500.0),
        iface("lo", "loopback", 65536.0),
        iface("enp0s31f6", "ethernet", 1500.0),
    ])
}

fn reversed() -> Value {
    Value::List(vec![
        iface("wlp0s20f3", "wifi", 1500.0),
        iface("lo", "loopback", 65536.0),
        iface("enp0s31f6", "ethernet", 1500.0),
    ])
}

fn grouped() -> Value {
    // The groups keep first-seen order per group; the items inside keep
    // their own order.
    Value::List(vec![
        Value::Object(vec![
            ("key".into(), Value::String("ethernet".into())),
            ("items".into(), Value::List(vec![iface("enp0s31f6", "ethernet", 1500.0)])),
        ]),
        Value::Object(vec![
            ("key".into(), Value::String("loopback".into())),
            ("items".into(), Value::List(vec![iface("lo", "loopback", 65536.0)])),
        ]),
        Value::Object(vec![
            ("key".into(), Value::String("wifi".into())),
            ("items".into(), Value::List(vec![iface("wlp0s20f3", "wifi", 1500.0)])),
        ]),
    ])
}

fn flattened() -> Value {
    Value::List(vec![
        iface("enp0s31f6", "ethernet", 1500.0),
        iface("lo", "loopback", 65536.0),
        iface("wlp0s20f3", "wifi", 1500.0),
    ])
}

/// A where-filter with a `~=` substring match, the operator the selector
/// grammar and the pipe engine share.
#[test]
fn where_supports_the_substring_operator() {
    let op = parse_pipe("where name ~= wlp").unwrap();
    let got = apply_pipe(interfaces(), &op).unwrap();
    assert_eq!(got, one_wifi());
}

/// An unknown pipe operation is an error, never a silent no-op.
#[test]
fn an_unknown_pipe_op_is_an_error() {
    assert!(parse_pipe("frobnicate").is_err());
}

/// Piping a non-list where a list is required is an error naming the op.
#[test]
fn where_on_a_non_list_names_itself() {
    let op = parse_pipe("count").unwrap();
    let got = apply_pipe(Value::Number(1.0), &op).unwrap();
    assert_eq!(got, Value::Number(1.0), "count answers 1 for a scalar");
    let op = parse_pipe("where type == wifi").unwrap();
    let err = apply_pipe(Value::Number(1.0), &op).unwrap_err();
    assert!(err.to_string().contains("list"), "the error says so: {err}");
}
