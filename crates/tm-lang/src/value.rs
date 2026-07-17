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
        captured: Environment,
        args: Vec<Value>,
    },
    Row {
        body: Expr,
        captured: Environment,
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
        self.to_json() == other.to_json()
    }
}
