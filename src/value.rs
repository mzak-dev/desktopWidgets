//! The dynamic value bindings evaluate to.

use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Value {
    #[default]
    Nil,
    Num(f64),
    Bool(bool),
    Str(String),
    List(Vec<Value>),
    Obj(BTreeMap<String, Value>),
}

impl Value {
    pub fn truthy(&self) -> bool {
        match self {
            Value::Nil => false,
            Value::Bool(b) => *b,
            Value::Num(n) => *n != 0.0,
            Value::Str(s) => !s.is_empty(),
            Value::List(l) => !l.is_empty(),
            Value::Obj(_) => true,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Bool(b) => Some(*b as u8 as f64),
            Value::Str(s) => s.trim().parse().ok(),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(m) => m.get(key),
            Value::List(l) => key.parse::<usize>().ok().and_then(|i| l.get(i)),
            _ => None,
        }
    }

    pub fn obj<const N: usize>(pairs: [(&str, Value); N]) -> Value {
        Value::Obj(pairs.into_iter().map(|(k, v)| (k.to_string(), v)).collect())
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Num(n)
    }
}
impl From<i32> for Value {
    fn from(n: i32) -> Self {
        Value::Num(n as f64)
    }
}
impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}
impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::Str(s.to_string())
    }
}
impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::Str(s)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Nil => Ok(()),
            Value::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => write!(f, "{}", *n as i64),
            Value::Num(n) => write!(f, "{n}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Str(s) => f.write_str(s),
            Value::List(l) => write!(f, "[{} items]", l.len()),
            Value::Obj(_) => f.write_str("{…}"),
        }
    }
}

impl From<&toml::Value> for Value {
    fn from(v: &toml::Value) -> Self {
        match v {
            toml::Value::String(s) => Value::Str(s.clone()),
            toml::Value::Integer(i) => Value::Num(*i as f64),
            toml::Value::Float(x) => Value::Num(*x),
            toml::Value::Boolean(b) => Value::Bool(*b),
            toml::Value::Array(a) => Value::List(a.iter().map(Value::from).collect()),
            toml::Value::Table(t) => {
                Value::Obj(t.iter().map(|(k, v)| (k.clone(), Value::from(v))).collect())
            }
            toml::Value::Datetime(d) => Value::Str(d.to_string()),
        }
    }
}

impl From<&serde_json::Value> for Value {
    fn from(v: &serde_json::Value) -> Self {
        match v {
            serde_json::Value::Null => Value::Nil,
            serde_json::Value::Bool(b) => Value::Bool(*b),
            serde_json::Value::Number(n) => Value::Num(n.as_f64().unwrap_or(0.0)),
            serde_json::Value::String(s) => Value::Str(s.clone()),
            serde_json::Value::Array(a) => Value::List(a.iter().map(Value::from).collect()),
            serde_json::Value::Object(o) => {
                Value::Obj(o.iter().map(|(k, v)| (k.clone(), Value::from(v))).collect())
            }
        }
    }
}

impl From<&Value> for serde_json::Value {
    fn from(v: &Value) -> Self {
        match v {
            Value::Nil => serde_json::Value::Null,
            Value::Bool(b) => (*b).into(),
            Value::Num(n) if n.fract() == 0.0 && n.abs() < 1e15 => (*n as i64).into(),
            Value::Num(n) => serde_json::Number::from_f64(*n).map_or(serde_json::Value::Null, Into::into),
            Value::Str(s) => s.clone().into(),
            Value::List(l) => serde_json::Value::Array(l.iter().map(Into::into).collect()),
            Value::Obj(o) => serde_json::Value::Object(o.iter().map(|(k, v)| (k.clone(), v.into())).collect()),
        }
    }
}
