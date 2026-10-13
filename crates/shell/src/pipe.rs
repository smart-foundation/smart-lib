use crate::selector::Value;
use crate::error::SmartError;

#[derive(Debug, Clone)]
pub enum PipeOp {
    Where { field: String, op: String, value: Value },
    Map { field: String },
    Sort { field: String, desc: bool },
    Take(usize),
    Skip(usize),
    Count,
    Reverse,
    Unique,
    Head(usize),
    Tail(usize),
    Pluck { field: String },
    GroupBy { field: String },
    Flatten,
    Len,
    First,
    Last,
    Sum { field: String },
    Avg { field: String },
    Min { field: String },
    Max { field: String },
}

pub fn parse_pipe(input: &str) -> Result<PipeOp, SmartError> {
    let tokens: Vec<&str> = input.split_whitespace().collect();
    match tokens.first() {
        Some(&"where") => {
            let field = tokens.get(1)
                .ok_or_else(|| SmartError::Failed("where: missing field".into()))?
                .to_string();
            let op = tokens.get(2).unwrap_or(&"==").to_string();
            let value = tokens.get(3)
                .map(|v| parse_pipe_value(v))
                .unwrap_or(Value::Bool(true));
            Ok(PipeOp::Where { field, op, value })
        }
        Some(&"map") => Ok(PipeOp::Map {
            field: tokens.get(1)
                .ok_or_else(|| SmartError::Failed("map: missing field".into()))?
                .to_string(),
        }),
        Some(&"sort") => Ok(PipeOp::Sort {
            field: tokens.get(1)
                .ok_or_else(|| SmartError::Failed("sort: missing field".into()))?
                .to_string(),
            desc: tokens.get(2) == Some(&"desc"),
        }),
        Some(&"take") => Ok(PipeOp::Take(
            tokens.get(1)
                .ok_or_else(|| SmartError::Failed("take: missing count".into()))?
                .parse()
                .map_err(|_| SmartError::Failed("take: invalid number".into()))?
        )),
        Some(&"skip") => Ok(PipeOp::Skip(
            tokens.get(1)
                .ok_or_else(|| SmartError::Failed("skip: missing count".into()))?
                .parse()
                .map_err(|_| SmartError::Failed("skip: invalid number".into()))?
        )),
        Some(&"count") => Ok(PipeOp::Count),
        Some(&"reverse") => Ok(PipeOp::Reverse),
        Some(&"unique") => Ok(PipeOp::Unique),
        Some(&"head") => Ok(PipeOp::Head(
            tokens.get(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(10)
        )),
        Some(&"tail") => Ok(PipeOp::Tail(
            tokens.get(1)
                .and_then(|s| s.parse().ok())
                .unwrap_or(10)
        )),
        Some(&"pluck") => Ok(PipeOp::Pluck {
            field: tokens.get(1)
                .ok_or_else(|| SmartError::Failed("pluck: missing field".into()))?
                .to_string(),
        }),
        Some(&"group") => Ok(PipeOp::GroupBy {
            field: tokens.get(1)
                .ok_or_else(|| SmartError::Failed("group: missing field".into()))?
                .to_string(),
        }),
        Some(&"flatten") => Ok(PipeOp::Flatten),
        Some(&"len") => Ok(PipeOp::Len),
        Some(&"first") => Ok(PipeOp::First),
        Some(&"last") => Ok(PipeOp::Last),
        Some(&"sum") => Ok(PipeOp::Sum {
            field: tokens.get(1)
                .ok_or_else(|| SmartError::Failed("sum: missing field".into()))?
                .to_string(),
        }),
        Some(&"avg") => Ok(PipeOp::Avg {
            field: tokens.get(1)
                .ok_or_else(|| SmartError::Failed("avg: missing field".into()))?
                .to_string(),
        }),
        Some(&"min") => Ok(PipeOp::Min {
            field: tokens.get(1)
                .ok_or_else(|| SmartError::Failed("min: missing field".into()))?
                .to_string(),
        }),
        Some(&"max") => Ok(PipeOp::Max {
            field: tokens.get(1)
                .ok_or_else(|| SmartError::Failed("max: missing field".into()))?
                .to_string(),
        }),
        Some(other) => Err(SmartError::Failed(format!("Unknown pipe operation: {other}"))),
        None => Err(SmartError::Failed("Empty pipe".into())),
    }
}

fn parse_pipe_value(s: &str) -> Value {
    if s == "true" { Value::Bool(true) }
    else if s == "false" { Value::Bool(false) }
    else if s == "null" || s == "none" { Value::Null }
    else if let Ok(n) = s.parse::<f64>() { Value::Number(n) }
    else { Value::String(s.trim_matches('"').to_string()) }
}

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

pub fn apply_pipe(input: Value, op: &PipeOp) -> Result<Value, SmartError> {
    match op {
        PipeOp::Where { field, op, value } => {
            let items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("where: input must be a list".into())),
            };
            let filtered: Vec<Value> = items.into_iter().filter(|item| {
                let field_val = get_field(item, field);
                match op.as_str() {
                    "==" | "=" => field_val == *value,
                    "!=" => field_val != *value,
                    ">" => field_val.as_f64().unwrap_or(0.0) > value.as_f64().unwrap_or(0.0),
                    "<" => field_val.as_f64().unwrap_or(0.0) < value.as_f64().unwrap_or(0.0),
                    ">=" => field_val.as_f64().unwrap_or(0.0) >= value.as_f64().unwrap_or(0.0),
                    "<=" => field_val.as_f64().unwrap_or(0.0) <= value.as_f64().unwrap_or(0.0),
                    "~=" => {
                        match (&field_val, value.as_str()) {
                            (Value::String(haystack), Some(needle)) => haystack.contains(needle),
                            _ => false,
                        }
                    }
                    _ => false,
                }
            }).collect();
            Ok(Value::List(filtered))
        }
        PipeOp::Map { field } => {
            let items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("map: input must be a list".into())),
            };
            let mapped: Vec<Value> = items.into_iter()
                .map(|item| get_field(&item, field))
                .collect();
            Ok(Value::List(mapped))
        }
        PipeOp::Sort { field, desc } => {
            let mut items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("sort: input must be a list".into())),
            };
            items.sort_by(|a, b| {
                let a_val = get_field(a, field);
                let b_val = get_field(b, field);
                match (&a_val, &b_val) {
                    (Value::Number(a), Value::Number(b)) => {
                        if *desc { b.partial_cmp(a).unwrap() }
                        else { a.partial_cmp(b).unwrap() }
                    }
                    (Value::String(a), Value::String(b)) => {
                        if *desc { b.cmp(a) }
                        else { a.cmp(b) }
                    }
                    _ => std::cmp::Ordering::Equal,
                }
            });
            Ok(Value::List(items))
        }
        PipeOp::Take(n) => {
            let items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("take: input must be a list".into())),
            };
            Ok(Value::List(items.into_iter().take(*n).collect()))
        }
        PipeOp::Skip(n) => {
            let items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("skip: input must be a list".into())),
            };
            Ok(Value::List(items.into_iter().skip(*n).collect()))
        }
        PipeOp::Count => {
            match input {
                Value::List(l) => Ok(Value::Number(l.len() as f64)),
                _ => Ok(Value::Number(1.0)),
            }
        }
        PipeOp::Reverse => {
            let mut items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("reverse: input must be a list".into())),
            };
            items.reverse();
            Ok(Value::List(items))
        }
        PipeOp::Unique => {
            let items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("unique: input must be a list".into())),
            };
            let mut seen = Vec::new();
            let unique: Vec<Value> = items.into_iter().filter(|item| {
                let display = item.display();
                if seen.contains(&display) {
                    false
                } else {
                    seen.push(display);
                    true
                }
            }).collect();
            Ok(Value::List(unique))
        }
        PipeOp::Head(n) => {
            let items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("head: input must be a list".into())),
            };
            Ok(Value::List(items.into_iter().take(*n).collect()))
        }
        PipeOp::Tail(n) => {
            let items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("tail: input must be a list".into())),
            };
            let skip = items.len().saturating_sub(*n);
            Ok(Value::List(items.into_iter().skip(skip).collect()))
        }
        PipeOp::Pluck { field } => {
            let items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("pluck: input must be a list".into())),
            };
            let plucked: Vec<Value> = items.into_iter()
                .map(|item| get_field(&item, field))
                .collect();
            Ok(Value::List(plucked))
        }
        PipeOp::GroupBy { field } => {
            let items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("group: input must be a list".into())),
            };
            // First-seen order, stable: a HashMap's arbitrary iteration made
            // the same input group differently between runs.
            let mut keys: Vec<String> = Vec::new();
            let mut groups: std::collections::HashMap<String, Vec<Value>> = std::collections::HashMap::new();
            for item in items {
                let key = get_field(&item, field).display();
                let entry = groups.entry(key.clone()).or_default();
                if entry.is_empty() {
                    keys.push(key);
                }
                entry.push(item);
            }
            let grouped: Vec<Value> = keys
                .into_iter()
                .filter_map(|k| groups.remove(&k).map(|v| {
                    Value::Object(vec![
                        ("key".into(), Value::String(k)),
                        ("items".into(), Value::List(v)),
                    ])
                }))
                .collect();
            Ok(Value::List(grouped))
        }
        PipeOp::Flatten => {
            let items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("flatten: input must be a list".into())),
            };
            let mut flat = Vec::new();
            for item in items {
                if let Value::List(sub) = item {
                    flat.extend(sub);
                } else {
                    flat.push(item);
                }
            }
            Ok(Value::List(flat))
        }
        PipeOp::Len => {
            match input {
                Value::List(l) => Ok(Value::Number(l.len() as f64)),
                Value::String(s) => Ok(Value::Number(s.len() as f64)),
                _ => Ok(Value::Number(0.0)),
            }
        }
        PipeOp::First => {
            match input {
                Value::List(l) => Ok(l.into_iter().next().unwrap_or(Value::Null)),
                _ => Err(SmartError::Failed("first: input must be a list".into())),
            }
        }
        PipeOp::Last => {
            match input {
                Value::List(l) => Ok(l.into_iter().last().unwrap_or(Value::Null)),
                _ => Err(SmartError::Failed("last: input must be a list".into())),
            }
        }
        PipeOp::Sum { field } => {
            let items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("sum: input must be a list".into())),
            };
            let total: f64 = items.iter()
                .map(|item| get_field(item, field).as_f64().unwrap_or(0.0))
                .sum();
            Ok(Value::Number(total))
        }
        PipeOp::Avg { field } => {
            let items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("avg: input must be a list".into())),
            };
            if items.is_empty() {
                return Ok(Value::Number(0.0));
            }
            let total: f64 = items.iter()
                .map(|item| get_field(item, field).as_f64().unwrap_or(0.0))
                .sum();
            Ok(Value::Number(total / items.len() as f64))
        }
        PipeOp::Min { field } => {
            let items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("min: input must be a list".into())),
            };
            let min = items.iter()
                .map(|item| get_field(item, field).as_f64().unwrap_or(f64::MAX))
                .fold(f64::MAX, f64::min);
            Ok(Value::Number(min))
        }
        PipeOp::Max { field } => {
            let items = match input {
                Value::List(l) => l,
                _ => return Err(SmartError::Failed("max: input must be a list".into())),
            };
            let max = items.iter()
                .map(|item| get_field(item, field).as_f64().unwrap_or(f64::MIN))
                .fold(f64::MIN, f64::max);
            Ok(Value::Number(max))
        }
    }
}

pub fn parse_pipe_chain(input: &str) -> Result<Vec<PipeOp>, SmartError> {
    let parts: Vec<&str> = input.split('|').collect();
    let mut ops = Vec::new();
    for part in &parts[1..] {
        let trimmed = part.trim();
        if !trimmed.is_empty() {
            ops.push(parse_pipe(trimmed)?);
        }
    }
    Ok(ops)
}

pub fn apply_pipe_chain(input: Value, ops: &[PipeOp]) -> Result<Value, SmartError> {
    let mut current = input;
    for op in ops {
        current = apply_pipe(current, op)?;
    }
    Ok(current)
}
