//! The lightspeed step interpreter: runs the `step` tree of an `/ls_resp` payload and records every
//! stored-procedure call the response makes, with its arguments already evaluated.
//!
//! Ported from `pkg/messagix/lightspeed/decode.go` (AGPL-3.0, mautrix-meta). Steps are JSON arrays
//! whose first element is the step type. Integers are never raw JSON numbers in a payload: they are
//! `[19, "123"]` steps that parse a string, which is why [`Value::Int`] is only produced there.

use std::collections::{BTreeMap, HashMap, HashSet};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use log::debug;
use serde_json::Value as Json;

/// Step type numbers of the wire format (the subset the decoder interprets).
pub mod step {
    pub const BLOCK: i64 = 1;
    pub const LOAD: i64 = 2;
    pub const STORE: i64 = 3;
    pub const STORE_ARRAY: i64 = 4;
    pub const CALL_STORED_PROCEDURE: i64 = 5;
    pub const UNDEFINED: i64 = 9;
    pub const TO_BLOB: i64 = 16;
    pub const I64_FROM_STRING: i64 = 19;
    pub const IF: i64 = 23;
    pub const NOT: i64 = 26;
    pub const I64_EQUAL: i64 = 30;
    pub const NATIVE_OP_MAP_CREATE: i64 = 53;
    pub const NATIVE_OP_MAP_SET: i64 = 55;
    pub const NATIVE_OP_ARRAY_CREATE: i64 = 50;
    pub const NATIVE_OP_ARRAY_APPEND: i64 = 51;
    pub const NATIVE_OP_ARRAY_GET_SIZE: i64 = 52;
    pub const NATIVE_OP_CURRENT_TIME: i64 = 60;
    pub const LOGGER_LOG: i64 = 49;
    pub const I64_ADD: i64 = 69;
}

/// A decoded argument of a stored-procedure call.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// A JSON number. Integers are not sent this way, so these are only ever seen by accident.
    Float(f64),
    Int(i64),
    Str(String),
    Blob(Vec<u8>),
    List(Vec<Value>),
    Map(BTreeMap<String, Value>),
}

impl Value {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_blob(&self) -> Option<&[u8]> {
        match self {
            Self::Blob(value) => Some(value),
            _ => None,
        }
    }

    /// The value as an integer or, for the "unset" cases the server uses, zero.
    pub fn int_or_zero(&self) -> i64 {
        self.as_i64().unwrap_or(0)
    }

    /// The value as text, or the empty string.
    pub fn str_or_empty(&self) -> &str {
        self.as_str().unwrap_or("")
    }

    fn from_json(value: &Json) -> Self {
        match value {
            Json::Null => Self::Null,
            Json::Bool(flag) => Self::Bool(*flag),
            Json::Number(number) => Self::Float(number.as_f64().unwrap_or(0.0)),
            Json::String(text) => Self::Str(text.clone()),
            Json::Array(items) => Self::List(items.iter().map(Self::from_json).collect()),
            Json::Object(map) => Self::Map(
                map.iter()
                    .map(|(key, value)| (key.clone(), Self::from_json(value)))
                    .collect(),
            ),
        }
    }
}

/// One stored-procedure call of a response, in the order the server made it.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    /// The dependency name, such as `deleteThenInsertThread`.
    pub name: String,
    pub args: Vec<Value>,
}

/// Interprets a step tree. Only calls whose name is in `wanted` are recorded; `None` records all.
pub struct Decoder {
    wanted: Option<HashSet<String>>,
    references: HashMap<i64, Value>,
    calls: Vec<Call>,
}

impl Decoder {
    pub fn new(wanted: Option<HashSet<String>>) -> Self {
        Self {
            wanted,
            references: HashMap::new(),
            calls: Vec::new(),
        }
    }

    /// Interpret a whole `step` tree and return the recorded calls.
    pub fn run(mut self, steps: &Json) -> Vec<Call> {
        self.decode(steps);
        self.calls
    }

    fn decode(&mut self, node: &Json) -> Value {
        match node.as_array() {
            Some(items) => self.decode_items(items),
            None => Value::from_json(node),
        }
    }

    /// Interpret one step: `items[0]` is its type, the rest are its arguments.
    fn decode_items(&mut self, items: &[Json]) -> Value {
        let Some(kind) = items.first().and_then(as_integer) else {
            return Value::Null;
        };
        let args = items.get(1..).unwrap_or_default();
        match kind {
            step::BLOCK => {
                for block in args {
                    if block.is_array() {
                        self.decode(block);
                    } else {
                        debug!("lightspeed: block entry is not a step");
                    }
                }
                Value::Null
            }
            step::LOAD => {
                let key = args.first().and_then(as_integer);
                key.and_then(|key| self.references.get(&key).cloned())
                    .unwrap_or(Value::Null)
            }
            step::STORE => {
                let key = args.first().and_then(as_integer);
                let value = args.get(1).map(|node| self.decode(node)).unwrap_or(Value::Null);
                if let Some(key) = key {
                    self.references.insert(key, value);
                }
                Value::Null
            }
            step::STORE_ARRAY => self.store_array(items, args),
            step::CALL_STORED_PROCEDURE => {
                self.call_procedure(args);
                Value::Null
            }
            step::UNDEFINED => Value::Null,
            step::I64_FROM_STRING => args
                .first()
                .and_then(Json::as_str)
                .and_then(|text| text.parse::<i64>().ok())
                .map_or(Value::Null, Value::Int),
            step::TO_BLOB => args
                .first()
                .and_then(Json::as_str)
                .and_then(|text| STANDARD.decode(text).ok())
                .map_or(Value::Null, Value::Blob),
            step::IF => self.branch(args),
            step::NOT => match args.first().map(|node| self.decode(node)) {
                Some(value @ Value::Int(_)) => value,
                _ => Value::Null,
            },
            step::I64_EQUAL => {
                let first = args.first().map(|node| self.decode(node));
                let second = args.get(1).map(|node| self.decode(node));
                match (first.and_then(|v| v.as_i64()), second.and_then(|v| v.as_i64())) {
                    (Some(a), Some(b)) => Value::Bool(a == b),
                    _ => Value::Null,
                }
            }
            step::I64_ADD => {
                let first = args.first().map(|node| self.decode(node));
                let second = args.get(1).map(|node| self.decode(node));
                match (first.and_then(|v| v.as_i64()), second.and_then(|v| v.as_i64())) {
                    (Some(a), Some(b)) => Value::Int(a.wrapping_add(b)),
                    _ => Value::Null,
                }
            }
            step::NATIVE_OP_CURRENT_TIME => Value::Int(now_millis()),
            step::NATIVE_OP_MAP_CREATE => Value::Map(BTreeMap::new()),
            step::NATIVE_OP_MAP_SET => self.map_set(args),
            step::NATIVE_OP_ARRAY_CREATE => Value::List(Vec::new()),
            step::NATIVE_OP_ARRAY_APPEND => {
                let list = args.first().map(|node| self.decode(node));
                let item = args.get(1).map(|node| self.decode(node)).unwrap_or(Value::Null);
                match list {
                    Some(Value::List(mut items)) => {
                        items.push(item);
                        Value::List(items)
                    }
                    _ => Value::Null,
                }
            }
            step::NATIVE_OP_ARRAY_GET_SIZE => match args.first().map(|node| self.decode(node)) {
                Some(Value::List(items)) => Value::Int(items.len() as i64),
                _ => Value::Null,
            },
            step::LOGGER_LOG => Value::Null,
            other => {
                debug!("lightspeed: unhandled step type {other}");
                Value::Null
            }
        }
    }

    /// `STORE_ARRAY` remembers a count and then decodes the rest of the step as a block whose type
    /// is that count, exactly as the reference does with `ls.Decode(s[2:])`.
    fn store_array(&mut self, items: &[Json], args: &[Json]) -> Value {
        let key = args.first().and_then(Json::as_f64).map(|value| value as i64);
        let count = args.get(1).and_then(Json::as_f64).map(|value| value as i64);
        let (Some(key), Some(count)) = (key, count) else {
            return Value::Null;
        };
        if args.len() == 3 && is_poll_attachment_probe(&args[2]) {
            self.references.insert(key, Value::Int(0));
            return Value::Null;
        }
        self.references.insert(key, Value::Int(count));
        match items.get(2..) {
            Some(rest) if !rest.is_empty() => self.decode_items(rest),
            _ => Value::Null,
        }
    }

    fn call_procedure(&mut self, args: &[Json]) {
        let Some(name) = args.first().and_then(Json::as_str) else {
            debug!("lightspeed: procedure call without a name");
            return;
        };
        if let Some(wanted) = &self.wanted {
            if !wanted.contains(name) {
                debug!("lightspeed: skipping procedure without a dependency");
                return;
            }
        }
        let decoded = args[1..].iter().map(|node| self.decode(node)).collect();
        self.calls.push(Call {
            name: name.to_owned(),
            args: decoded,
        });
    }

    fn branch(&mut self, args: &[Json]) -> Value {
        let Some(condition) = args.first() else {
            return Value::Null;
        };
        match self.decode(condition) {
            Value::Int(value) if value > 0 => {
                if let Some(then) = args.get(1) {
                    self.decode(then);
                }
            }
            _ => {
                if let Some(other) = args.get(2) {
                    if !other.is_null() {
                        self.decode(other);
                    }
                }
            }
        }
        Value::Null
    }

    /// `NATIVE_OP_MAP_SET` writes through a reference when its target is a `LOAD`, which is how the
    /// reference's shared maps behave in practice.
    fn map_set(&mut self, args: &[Json]) -> Value {
        let (Some(target), Some(key_node), Some(value_node)) = (args.first(), args.get(1), args.get(2))
        else {
            return Value::Null;
        };
        let key = match self.decode(key_node) {
            Value::Str(text) => text,
            Value::Int(number) => number.to_string(),
            _ => return Value::Null,
        };
        let value = self.decode(value_node);
        let mut map = match self.decode(target) {
            Value::Map(map) => map,
            _ => return Value::Null,
        };
        map.insert(key, value);
        let result = Value::Map(map);
        if let Some(reference) = load_key(target) {
            self.references.insert(reference, result.clone());
        }
        result
    }
}

fn as_integer(node: &Json) -> Option<i64> {
    let number = node.as_f64()?;
    if number.fract() == 0.0 && number.is_finite() {
        Some(number as i64)
    } else {
        None
    }
}

fn load_key(node: &Json) -> Option<i64> {
    let items = node.as_array()?;
    if as_integer(items.first()?)? != step::LOAD {
        return None;
    }
    as_integer(items.get(1)?)
}

fn is_poll_attachment_probe(node: &Json) -> bool {
    let Some(items) = node.as_array() else {
        return false;
    };
    items.len() == 4
        && items.first().and_then(as_integer) == Some(step::CALL_STORED_PROCEDURE)
        && items.get(1).and_then(Json::as_str) == Some("hasPollAttachmentNewerThan")
}

/// Milliseconds since the Unix epoch, for the `NATIVE_OP_CURRENT_TIME` step.
fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn calls(steps: Json) -> Vec<Call> {
        Decoder::new(None).run(&steps)
    }

    #[test]
    fn a_procedure_call_collects_its_evaluated_arguments() {
        let recorded = calls(json!([
            1,
            [5, "upsertMessage", "hello", [19, "42"], true, [9]]
        ]));
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].name, "upsertMessage");
        assert_eq!(
            recorded[0].args,
            vec![
                Value::Str("hello".into()),
                Value::Int(42),
                Value::Bool(true),
                Value::Null
            ]
        );
    }

    #[test]
    fn integers_from_strings_that_do_not_parse_become_null() {
        let recorded = calls(json!([1, [5, "x", [19, "not a number"]]]));
        assert_eq!(recorded[0].args, vec![Value::Null]);
    }

    #[test]
    fn stored_values_are_loaded_by_key() {
        let recorded = calls(json!([
            1,
            [3, 7, [19, "9"]],
            [5, "deleteThread", [2, 7], [2, 99]]
        ]));
        assert_eq!(recorded[0].args, vec![Value::Int(9), Value::Null]);
    }

    #[test]
    fn an_if_takes_the_branch_its_condition_selects() {
        let steps = json!([
            1,
            [23, [19, "1"], [5, "yes", [19, "1"]], [5, "no", [19, "2"]]],
            [23, [19, "0"], [5, "yes2", [19, "1"]], [5, "no2", [19, "2"]]]
        ]);
        let names: Vec<String> = calls(steps).into_iter().map(|call| call.name).collect();
        assert_eq!(names, vec!["yes", "no2"]);
    }

    #[test]
    fn only_wanted_procedures_are_recorded() {
        let wanted: HashSet<String> = ["keep".to_owned()].into_iter().collect();
        let recorded = Decoder::new(Some(wanted)).run(&json!([
            1,
            [5, "drop", [19, "1"]],
            [5, "keep", [19, "2"]]
        ]));
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].name, "keep");
    }

    #[test]
    fn blobs_are_base64_decoded() {
        let recorded = calls(json!([1, [5, "p", [16, "aGk="]]]));
        assert_eq!(recorded[0].args, vec![Value::Blob(b"hi".to_vec())]);
    }

    #[test]
    fn store_array_decodes_the_remaining_steps_as_a_block() {
        let recorded = calls(json!([
            1,
            [4, 3, 1, [5, "a", [19, "1"]], [5, "b", [19, "2"]]]
        ]));
        let names: Vec<String> = recorded.into_iter().map(|call| call.name).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn unknown_steps_and_malformed_input_never_panic() {
        assert!(calls(json!([1, [999, 1, 2], [5], [19], [23], [2, "x"]])).is_empty());
        assert!(calls(json!("not a step")).is_empty());
        assert!(calls(json!([1, [5, 7]])).is_empty());
    }

    #[test]
    fn a_map_collects_string_keys_from_the_steps() {
        let mut decoder = Decoder::new(None);
        let value = decoder.decode(&json!([55, [53], [19, "1"], [19, "2"]]));
        assert_eq!(
            value,
            Value::Map(BTreeMap::from([("1".to_owned(), Value::Int(2))]))
        );
    }
}
