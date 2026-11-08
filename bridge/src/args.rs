//! Positional arguments of a bridge request, read leniently (a missing value is empty/false/0).

use serde_json::Value;

/// The `args` array of a request.
pub struct Args {
    items: Vec<Value>,
}

impl Args {
    /// Wrap the request's `args` value (anything but an array is treated as no arguments).
    pub fn new(v: Option<&Value>) -> Self {
        let items = match v {
            Some(Value::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        Self { items }
    }

    /// Argument `i` as text ("" when missing or not text).
    pub fn str(&self, i: usize) -> String {
        match self.items.get(i) {
            Some(Value::String(s)) => s.clone(),
            _ => String::new(),
        }
    }

    /// Argument `i` as a flag (false when missing).
    pub fn bool(&self, i: usize) -> bool {
        match self.items.get(i) {
            Some(Value::Bool(b)) => *b,
            _ => false,
        }
    }

    /// Argument `i` as an integer (0 when missing).
    pub fn i64(&self, i: usize) -> i64 {
        match self.items.get(i) {
            Some(v) => v.as_i64().unwrap_or(0),
            None => 0,
        }
    }

    /// Argument `i` as a list of text. The app passes these as a JSON string like `["/a","/b"]`;
    /// a real array is accepted too.
    pub fn list(&self, i: usize) -> Vec<String> {
        let v = match self.items.get(i) {
            Some(Value::String(s)) => serde_json::from_str::<Value>(s).unwrap_or(Value::Null),
            Some(other) => other.clone(),
            None => Value::Null,
        };
        match v {
            Value::Array(a) => a
                .iter()
                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                .collect(),
            _ => Vec::new(),
        }
    }
}
