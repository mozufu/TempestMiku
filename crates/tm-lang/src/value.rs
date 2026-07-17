use std::{collections::BTreeMap, rc::Rc};

use serde_json::{Map, Number, Value as JsonValue};

use crate::{Expr, Pattern};

pub type Environment = BTreeMap<String, Value>;

#[derive(Clone)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Decimal(f64),
    String(String),
    Uri(String),
    List(Vec<Value>),
    Record(BTreeMap<String, Value>),
    Tagged {
        name: String,
        payload: Option<Box<Value>>,
    },
    Callable(Rc<Callable>),
}

impl std::fmt::Debug for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Callable(callable) => f.debug_tuple("Callable").field(callable).finish(),
            _ => f.debug_tuple("Value").field(&self.to_json()).finish(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Callable {
    Builtin {
        name: String,
        args: Vec<Value>,
        arity: usize,
    },
    BuiltinDataLast {
        name: String,
        args: Vec<Value>,
        data: Value,
        arity: usize,
    },
    User {
        params: Vec<Pattern>,
        body: Expr,
        captured: Rc<Environment>,
        args: Vec<Value>,
    },
    Row {
        body: Expr,
        captured: Rc<Environment>,
    },
    Capability {
        name: String,
    },
    Constructor {
        name: String,
    },
}

impl Value {
    pub fn to_json(&self) -> JsonValue {
        match self {
            Self::Null => JsonValue::Null,
            Self::Bool(value) => JsonValue::Bool(*value),
            Self::Int(value) => JsonValue::Number((*value).into()),
            Self::Decimal(value) => {
                Number::from_f64(*value).map_or(JsonValue::Null, JsonValue::Number)
            }
            Self::String(value) | Self::Uri(value) => JsonValue::String(value.clone()),
            Self::List(values) => JsonValue::Array(values.iter().map(Self::to_json).collect()),
            Self::Record(fields) => JsonValue::Object(
                fields
                    .iter()
                    .map(|(name, value)| (name.clone(), value.to_json()))
                    .collect::<Map<_, _>>(),
            ),
            Self::Tagged { name, payload } => {
                let mut map = Map::new();
                map.insert("tag".into(), JsonValue::String(name.clone()));
                if let Some(payload) = payload {
                    map.insert("value".into(), payload.to_json());
                }
                JsonValue::Object(map)
            }
            Self::Callable(_) => JsonValue::String("<function>".into()),
        }
    }

    pub fn from_json(value: JsonValue) -> Self {
        match value {
            JsonValue::Null => Self::Null,
            JsonValue::Bool(value) => Self::Bool(value),
            JsonValue::Number(value) => value.as_i64().map_or_else(
                || Self::Decimal(value.as_f64().unwrap_or_default()),
                Self::Int,
            ),
            JsonValue::String(value) => Self::String(value),
            JsonValue::Array(values) => {
                Self::List(values.into_iter().map(Self::from_json).collect())
            }
            JsonValue::Object(fields) => Self::Record(
                fields
                    .into_iter()
                    .map(|(name, value)| (name, Self::from_json(value)))
                    .collect(),
            ),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Bool(_) => "bool",
            Self::Int(_) => "int",
            Self::Decimal(_) => "decimal",
            Self::String(_) => "string",
            Self::Uri(_) => "uri",
            Self::List(_) => "list",
            Self::Record(_) => "record",
            Self::Tagged { .. } => "constructor",
            Self::Callable(_) => "function",
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        json_semantic_eq_bounded(self, other, usize::MAX)
            .map(|(equal, _)| equal)
            .unwrap_or(false)
    }
}

pub(crate) fn json_semantic_eq_bounded(
    left: &Value,
    right: &Value,
    max_nodes: usize,
) -> Option<(bool, usize)> {
    fn string_view(value: &Value) -> Option<&str> {
        match value {
            Value::String(value) | Value::Uri(value) => Some(value),
            Value::Callable(_) => Some("<function>"),
            _ => None,
        }
    }

    fn visit(visits: &mut usize, max_nodes: usize) -> Option<()> {
        *visits = visits.checked_add(1)?;
        if *visits > max_nodes {
            return None;
        }
        Some(())
    }

    let mut visits = 0usize;
    let mut pending = vec![(left, right)];
    while let Some((left, right)) = pending.pop() {
        visit(&mut visits, max_nodes)?;
        if let (Some(left), Some(right)) = (string_view(left), string_view(right)) {
            if left != right {
                return Some((false, visits));
            }
            continue;
        }
        match (left, right) {
            (Value::Null, Value::Null) => {}
            (Value::Null, Value::Decimal(value)) | (Value::Decimal(value), Value::Null)
                if !value.is_finite() => {}
            (Value::Bool(left), Value::Bool(right)) if left == right => {}
            (Value::Int(left), Value::Int(right)) if left == right => {}
            (Value::Decimal(left), Value::Decimal(right))
                if Number::from_f64(*left) == Number::from_f64(*right) => {}
            (Value::Int(left), Value::Decimal(right))
            | (Value::Decimal(right), Value::Int(left))
                if Some(Number::from(*left)) == Number::from_f64(*right) => {}
            (Value::List(left), Value::List(right)) => {
                if left.len() != right.len() {
                    return Some((false, visits));
                }
                pending.extend(left.iter().zip(right).rev());
            }
            (Value::Record(left), Value::Record(right)) => {
                if left.len() != right.len() {
                    return Some((false, visits));
                }
                for ((left_name, left), (right_name, right)) in left.iter().zip(right).rev() {
                    if left_name != right_name {
                        return Some((false, visits));
                    }
                    pending.push((left, right));
                }
            }
            (
                Value::Tagged {
                    name: left_name,
                    payload: left_payload,
                },
                Value::Tagged {
                    name: right_name,
                    payload: right_payload,
                },
            ) => {
                if left_name != right_name {
                    return Some((false, visits));
                }
                match (left_payload.as_deref(), right_payload.as_deref()) {
                    (Some(left), Some(right)) => pending.push((left, right)),
                    (None, None) => {}
                    _ => return Some((false, visits)),
                }
            }
            (Value::Tagged { name, payload }, Value::Record(record))
            | (Value::Record(record), Value::Tagged { name, payload }) => {
                let expected_len = usize::from(payload.is_some()).saturating_add(1);
                if record.len() != expected_len {
                    return Some((false, visits));
                }
                let Some(tag) = record.get("tag") else {
                    return Some((false, visits));
                };
                visit(&mut visits, max_nodes)?;
                if string_view(tag).is_none_or(|value| value != name) {
                    return Some((false, visits));
                }
                match (payload.as_deref(), record.get("value")) {
                    (Some(payload), Some(value)) => pending.push((payload, value)),
                    (None, None) => {}
                    _ => return Some((false, visits)),
                }
            }
            _ => return Some((false, visits)),
        }
    }
    Some((true, visits))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equality_is_iterative_json_equivalent_and_node_bounded() {
        assert_eq!(
            Value::String("workspace:x".into()),
            Value::Uri("workspace:x".into())
        );
        let callable = Value::Callable(Rc::new(Callable::Constructor { name: "X".into() }));
        assert_eq!(callable, Value::String("<function>".into()));
        let tagged = Value::Tagged {
            name: "Some".into(),
            payload: Some(Box::new(Value::Int(1))),
        };
        let record = Value::Record(BTreeMap::from([
            ("tag".into(), Value::String("Some".into())),
            ("value".into(), Value::Int(1)),
        ]));
        assert_eq!(tagged, record);

        let large = Value::List(vec![Value::Null; 64]);
        assert!(json_semantic_eq_bounded(&large, &large, 8).is_none());

        let deep = (0..256).fold(Value::Null, |value, _| Value::List(vec![value]));
        let (equal, visits) = json_semantic_eq_bounded(&deep, &deep, 512).unwrap();
        assert!(equal);
        assert_eq!(visits, 257);
    }
}
