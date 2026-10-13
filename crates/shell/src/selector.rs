use crate::error::SmartError;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    String(String),
    Number(f64),
    Bool(bool),
    List(Vec<Value>),
    Object(Vec<(String, Value)>),
    Null,
}

impl Value {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            Value::String(s) => s.parse::<f64>().ok(),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Number(n) => Some(*n != 0.0),
            Value::String(s) => match s.as_str() {
                "true" | "1" | "yes" => Some(true),
                "false" | "0" | "no" => Some(false),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn is_truthy(&self) -> bool {
        match self {
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0,
            Value::String(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::Object(_) => true,
            Value::Null => false,
        }
    }

    /// A record field as a bool (a missing field is false, never a guess).
    pub fn field_bool(&self, key: &str) -> bool {
        match self {
            Value::Object(pairs) => pairs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| matches!(v, Value::Bool(true)))
                .unwrap_or(false),
            _ => false,
        }
    }

    /// A record field, borrowed (a missing field is the Null value, which
    /// never panics and never fabricates).
    pub fn field(&self, key: &str) -> &Value {
        match self {
            Value::Object(pairs) => pairs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v)
                .unwrap_or(&Value::Null),
            _ => &Value::Null,
        }
    }

    /// A record field's display text (a missing field is empty).
    pub fn field_str(&self, key: &str) -> String {
        match self {
            Value::Object(pairs) => pairs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.display())
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    pub fn display(&self) -> String {
        match self {
            Value::String(s) => s.clone(),
            Value::Number(n) => {
                if *n == (*n as i64) as f64 {
                    format!("{}", *n as i64)
                } else {
                    format!("{:.2}", n)
                }
            }
            Value::Bool(b) => if *b { "true".to_string() } else { "false".to_string() },
            Value::List(l) => {
                let items: Vec<String> = l.iter().map(|v| v.display()).collect();
                format!("[{}]", items.join(", "))
            }
            Value::Object(pairs) => {
                let items: Vec<String> = pairs.iter()
                    .map(|(k, v)| format!("{}: {}", k, v.display()))
                    .collect();
                format!("{{{}}}", items.join(", "))
            }
            Value::Null => "null".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum SelectorSegment {
    Field(String),
    Index(usize),
    Wildcard,
}

#[derive(Debug, Clone)]
pub struct Filter {
    pub field: String,
    pub op: FilterOp,
    pub value: Value,
}

#[derive(Debug, Clone)]
pub enum FilterOp {
    Eq,
    Ne,
    Gt,
    Lt,
    Gte,
    Lte,
    Contains,
}

#[derive(Debug, Clone)]
pub struct Selector {
    pub domain: String,
    pub path: Vec<SelectorSegment>,
    pub filter: Option<Filter>,
}

pub fn parse(input: &str) -> Result<Selector, SmartError> {
    let input = input.strip_prefix('@')
        .ok_or_else(|| SmartError::Failed("Selector must start with @".into()))?;

    let parts: Vec<&str> = input.split('/').collect();
    if parts.is_empty() {
        return Err(SmartError::Failed("Empty selector".into()));
    }

    let domain = parts[0].to_string();
    let mut path = Vec::new();
    let mut filter = None;

    for part in &parts[1..] {
        if part.contains(" where ") {
            let (field_part, filter_part) = part.split_once(" where ").unwrap();
            if !field_part.is_empty() {
                path.push(SelectorSegment::Field(field_part.to_string()));
            }
            filter = Some(parse_filter(filter_part)?);
        } else if *part == "*" {
            path.push(SelectorSegment::Wildcard);
        } else if let Ok(idx) = part.parse::<usize>() {
            path.push(SelectorSegment::Index(idx));
        } else {
            path.push(SelectorSegment::Field(part.to_string()));
        }
    }

    Ok(Selector { domain, path, filter })
}

fn parse_filter(input: &str) -> Result<Filter, SmartError> {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    if tokens.len() == 1 {
        return Ok(Filter {
            field: tokens[0].to_string(),
            op: FilterOp::Eq,
            value: Value::Bool(true),
        });
    }
    if tokens.len() == 3 {
        let op = match tokens[1] {
            "==" | "=" => FilterOp::Eq,
            "!=" => FilterOp::Ne,
            ">" => FilterOp::Gt,
            "<" => FilterOp::Lt,
            ">=" => FilterOp::Gte,
            "<=" => FilterOp::Lte,
            "~=" => FilterOp::Contains,
            _ => return Err(SmartError::Failed(format!("Unknown operator: {}", tokens[1]))),
        };
        let value = parse_value(tokens[2]);
        return Ok(Filter {
            field: tokens[0].to_string(),
            op,
            value,
        });
    }
    Err(SmartError::Failed(format!("Invalid filter: {input}")))
}

fn parse_value(s: &str) -> Value {
    if s == "true" { Value::Bool(true) }
    else if s == "false" { Value::Bool(false) }
    else if s == "null" || s == "none" { Value::Null }
    else if let Ok(n) = s.parse::<f64>() { Value::Number(n) }
    else { Value::String(s.trim_matches('"').to_string()) }
}


/// Evaluate a selector against the engine, in-process. The one real domain
/// renders its tree from the engine's OWNED types; every other domain answers
/// honestly from the migration queue — never a fake.
#[cfg(feature = "domain-network")]
pub fn evaluate(
    sel: &Selector,
    backend: &dyn smart_lib::domains::network::NetworkBackend,
) -> Result<Value, SmartError> {
    match sel.domain.as_str() {
        #[cfg(feature = "domain-network")]
        "network" => {
            let root = network_root(backend)?;
            let value = resolve_path(root, &sel.path);
            if let Some(filter) = &sel.filter {
                let Value::List(items) = value else {
                    return Err(SmartError::invalid_argument(format!(
                        "the `where` filter applies to a list; {}/… does not resolve to one",
                        sel.domain
                    )));
                };
                return Ok(Value::List(apply_filter(&items, filter)));
            }
            Ok(value)
        }
        // Every other domain answers honestly from the migration queue —
        // the selector surface is never faked either.
        other => Err(SmartError::Failed(crate::migrate::pending_reason(other))),
    }
}

/// The network domain's selector tree, rendered from the engine's owned types
/// (`InterfaceSnapshot`, `RadioState`, `KnownWifiNetwork`, `ConnStatus`) — the
/// same values the command surface renders, so a selector and a command can
/// never disagree.
#[cfg(feature = "domain-network")]
fn network_root(
    backend: &dyn smart_lib::domains::network::NetworkBackend,
) -> Result<Value, SmartError> {
    use smart_lib::domains::network::types::Pagination;

    let all = Pagination { offset: 0, limit: u32::MAX };
    let interfaces = backend.get_interfaces(all)?;
    let radios = backend.list_radio_states(all)?;
    let networking = backend.is_networking_enabled()?;
    let known = backend.list_known_wifi_networks(all)?;

    let mut statuses = Vec::with_capacity(interfaces.items.len());
    for iface in &interfaces.items {
        statuses.push((iface, backend.get_status(iface.id)?));
    }

    Ok(Value::Object(vec![
        ("networking".into(), Value::Bool(networking)),
        (
            "interfaces".into(),
            Value::List(
                interfaces
                    .items
                    .iter()
                    .map(|i| {
                        Value::Object(vec![
                            ("name".into(), Value::String(i.name.clone())),
                            ("type".into(), Value::String(i.kind.clone())),
                            ("up".into(), Value::Bool(i.up)),
                            ("connected".into(), Value::Bool(i.connected)),
                            ("mtu".into(), Value::Number(i.mtu as f64)),
                            ("managed".into(), Value::Bool(i.managed)),
                            ("mac".into(), Value::String(i.hardware_address.0.clone())),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "radios".into(),
            Value::List(
                radios
                    .items
                    .iter()
                    .map(|r| {
                        Value::Object(vec![
                            ("name".into(), Value::String(r.name.clone())),
                            ("enabled".into(), Value::Bool(r.enabled)),
                            ("hard_blocked".into(), Value::Bool(r.hard_blocked)),
                            ("soft_blocked".into(), Value::Bool(r.soft_blocked)),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "known".into(),
            Value::List(
                known
                    .items
                    .iter()
                    .map(|n| {
                        Value::Object(vec![
                            ("ssid".into(), Value::String(n.ssid.clone())),
                            ("auto_connect".into(), Value::Bool(n.auto_connect)),
                            ("security".into(), Value::String(n.security.clone())),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "status".into(),
            Value::List(
                statuses
                    .iter()
                    .map(|(i, s)| {
                        Value::Object(vec![
                            ("name".into(), Value::String(i.name.clone())),
                            ("type".into(), Value::String(s.kind.clone())),
                            ("connected".into(), Value::Bool(s.connected)),
                            ("host_state".into(), Value::String(s.host_state.as_token().to_string())),
                            (
                                "host_state_source".into(),
                                Value::String(s.host_state_source.as_token().to_string()),
                            ),
                            ("ssid".into(), Value::String(s.ssid.clone())),
                        ])
                    })
                    .collect(),
            ),
        ),
    ]))
}

/// Read one field out of a record value (a non-record has no fields).
fn get_field(obj: &Value, field: &str) -> Value {
    match obj {
        Value::Object(pairs) => {
            for (k, v) in pairs {
                if k == field {
                    return v.clone();
                }
            }
            Value::Null
        }
        _ => Value::Null,
    }
}

/// Apply a selector filter to a list of records.
fn apply_filter(items: &[Value], filter: &Filter) -> Vec<Value> {
    items
        .iter()
        .filter(|item| {
            let field_val = get_field(item, &filter.field);
            match &filter.op {
                FilterOp::Eq => field_val == filter.value,
                FilterOp::Ne => field_val != filter.value,
                FilterOp::Gt => {
                    field_val.as_f64().unwrap_or(0.0) > filter.value.as_f64().unwrap_or(0.0)
                }
                FilterOp::Lt => {
                    field_val.as_f64().unwrap_or(0.0) < filter.value.as_f64().unwrap_or(0.0)
                }
                FilterOp::Gte => {
                    field_val.as_f64().unwrap_or(0.0) >= filter.value.as_f64().unwrap_or(0.0)
                }
                FilterOp::Lte => {
                    field_val.as_f64().unwrap_or(0.0) <= filter.value.as_f64().unwrap_or(0.0)
                }
                FilterOp::Contains => match (&field_val, &filter.value) {
                    (Value::String(haystack), Value::String(needle)) => haystack.contains(needle),
                    _ => false,
                },
            }
        })
        .cloned()
        .collect()
}

/// Walk a path (fields, indexes, wildcards) into a value.
fn resolve_path(mut obj: Value, path: &[SelectorSegment]) -> Value {
    for seg in path {
        match seg {
            SelectorSegment::Field(f) => obj = get_field(&obj, f),
            SelectorSegment::Index(i) => {
                if let Value::List(list) = obj {
                    obj = list.get(*i).cloned().unwrap_or(Value::Null);
                } else {
                    return Value::Null;
                }
            }
            SelectorSegment::Wildcard => {
                if let Value::Object(pairs) = obj {
                    let values: Vec<Value> = pairs.into_iter().map(|(_, v)| v).collect();
                    return Value::List(values);
                }
                return obj;
            }
        }
    }
    obj
}

/// The feature-off answer: the engine's network domain is not compiled, so no
/// selector can be served — said once, honestly, rather than faked.
#[cfg(not(feature = "domain-network"))]
pub fn evaluate(_sel: &Selector) -> Result<Value, SmartError> {
    Err(SmartError::unavailable(
        "the shell was built without the domain-network feature — the engine's network \
         domain is not compiled; rebuild with default features",
    ))
}
