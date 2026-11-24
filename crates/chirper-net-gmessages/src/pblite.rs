//! The `pblite` JSON encoding of protobuf messages that the Messages web endpoints use for
//! request and response bodies (`application/json+protobuf`).
//!
//! A message is a JSON array whose element `n - 1` holds field `n`; absent fields are `null`, a
//! nested message is an array, a repeated field is an array, bytes are standard base64, and 64-bit
//! integers may arrive as strings. Fields marked `pblite_binary` in the schema are base64 of their
//! binary protobuf (or, for strings, of their bytes). This module only has the positional helpers;
//! each message's layout lives beside its type in [`crate::wire`].

use base64::Engine;
use serde_json::Value;

use chirper_bridge::{Error, Result};

/// Builds one pblite array, leaving `null` where a field is absent.
#[derive(Debug, Default)]
pub struct Array {
    items: Vec<Value>,
}

impl Array {
    /// An empty message.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set field `field` (1-based). Setting the same field twice keeps the last value.
    pub fn set(&mut self, field: usize, value: Value) -> &mut Self {
        if self.items.len() < field {
            self.items.resize(field, Value::Null);
        }
        self.items[field - 1] = value;
        self
    }

    /// A string field.
    pub fn string(&mut self, field: usize, value: &str) -> &mut Self {
        self.set(field, Value::String(value.to_owned()))
    }

    /// An integer field, written as a JSON number.
    pub fn int(&mut self, field: usize, value: i64) -> &mut Self {
        self.set(field, Value::from(value))
    }

    /// A boolean field.
    pub fn boolean(&mut self, field: usize, value: bool) -> &mut Self {
        self.set(field, Value::Bool(value))
    }

    /// A bytes field, base64 encoded.
    pub fn bytes(&mut self, field: usize, value: &[u8]) -> &mut Self {
        self.set(field, Value::String(base64_encode(value)))
    }

    /// A nested message.
    pub fn message(&mut self, field: usize, value: Array) -> &mut Self {
        self.set(field, value.into_value())
    }

    /// A repeated field of nested messages.
    pub fn list(&mut self, field: usize, values: Vec<Array>) -> &mut Self {
        let items = values.into_iter().map(Array::into_value).collect();
        self.set(field, Value::Array(items))
    }

    /// The JSON value.
    pub fn into_value(self) -> Value {
        Value::Array(self.items)
    }
}

/// Standard base64, as pblite writes bytes.
pub fn base64_encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Decode standard base64 from a field.
pub fn base64_decode(text: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .map_err(|_| Error::Protocol("a pblite field is not valid base64".to_owned()))
}

/// Read-side view of a pblite array. Out-of-range and `null` fields read as absent.
#[derive(Clone, Copy, Debug)]
pub struct Fields<'a> {
    items: &'a [Value],
}

impl<'a> Fields<'a> {
    /// Wrap a JSON value that must be an array.
    pub fn of(value: &'a Value) -> Result<Self> {
        match value.as_array() {
            Some(items) => Ok(Self { items }),
            None => Err(Error::Protocol("expected a pblite array".to_owned())),
        }
    }

    /// The raw value of field `field` (1-based), when it is present.
    pub fn get(&self, field: usize) -> Option<&'a Value> {
        self.items
            .get(field.checked_sub(1)?)
            .filter(|value| !value.is_null())
    }

    /// A string field; an empty string when absent.
    pub fn string(&self, field: usize) -> Result<String> {
        match self.get(field) {
            None => Ok(String::new()),
            Some(Value::String(text)) => Ok(text.clone()),
            Some(_) => Err(type_error(field, "a string")),
        }
    }

    /// An optional string field.
    pub fn opt_string(&self, field: usize) -> Result<Option<String>> {
        match self.get(field) {
            None => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(type_error(field, "a string")),
        }
    }

    /// An integer field, read from a number or a numeric string; 0 when absent.
    pub fn int(&self, field: usize) -> Result<i64> {
        match self.get(field) {
            None => Ok(0),
            Some(Value::Number(number)) => number
                .as_i64()
                .or_else(|| number.as_f64().map(|f| f as i64))
                .ok_or_else(|| type_error(field, "an integer")),
            Some(Value::String(text)) => text
                .parse::<i64>()
                .map_err(|_| type_error(field, "an integer")),
            Some(_) => Err(type_error(field, "an integer")),
        }
    }

    /// A boolean field, read from a boolean or a number; false when absent.
    pub fn boolean(&self, field: usize) -> Result<bool> {
        match self.get(field) {
            None => Ok(false),
            Some(Value::Bool(flag)) => Ok(*flag),
            Some(Value::Number(number)) => Ok(number.as_f64().unwrap_or(0.0) != 0.0),
            Some(_) => Err(type_error(field, "a boolean")),
        }
    }

    /// A bytes field (base64); empty when absent.
    pub fn bytes(&self, field: usize) -> Result<Vec<u8>> {
        match self.get(field) {
            None => Ok(Vec::new()),
            Some(Value::String(text)) => base64_decode(text),
            Some(_) => Err(type_error(field, "base64 bytes")),
        }
    }

    /// A nested message, when present.
    pub fn message(&self, field: usize) -> Result<Option<Fields<'a>>> {
        match self.get(field) {
            None => Ok(None),
            Some(value) => Fields::of(value).map(Some),
        }
    }

    /// A repeated field, empty when absent.
    pub fn list(&self, field: usize) -> Result<Vec<Fields<'a>>> {
        match self.get(field) {
            None => Ok(Vec::new()),
            Some(Value::Array(items)) => items
                .iter()
                .map(Fields::of)
                .collect::<Result<Vec<_>>>(),
            Some(_) => Err(type_error(field, "a list")),
        }
    }
}

fn type_error(field: usize, expected: &str) -> Error {
    Error::Protocol(format!("pblite field {field} is not {expected}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn fields_are_positional_and_nulls_are_absent() {
        let value = json!([null, "x", 7, null]);
        let fields = Fields::of(&value).expect("array");
        assert_eq!(fields.string(1).expect("absent"), "");
        assert_eq!(fields.string(2).expect("string"), "x");
        assert_eq!(fields.int(3).expect("number"), 7);
        assert!(fields.get(4).is_none());
        assert!(fields.get(9).is_none());
    }

    #[test]
    fn int64_accepts_a_numeric_string() {
        let value = json!([null, "1234567890123"]);
        assert_eq!(Fields::of(&value).expect("array").int(2).expect("int"), 1_234_567_890_123);
    }

    #[test]
    fn writer_leaves_gaps_as_null_and_trims_nothing_after_the_last_set_field() {
        let mut arr = Array::new();
        arr.string(3, "c").int(1, 5);
        assert_eq!(arr.into_value(), json!([5, null, "c"]));
    }

    #[test]
    fn bytes_round_trip_as_base64() {
        let mut arr = Array::new();
        arr.bytes(2, &[0xde, 0xad, 0xbe, 0xef]);
        let value = arr.into_value();
        assert_eq!(value, json!([null, "3q2+7w=="]));
        assert_eq!(
            Fields::of(&value).expect("array").bytes(2).expect("bytes"),
            vec![0xde, 0xad, 0xbe, 0xef]
        );
    }

    #[test]
    fn wrong_types_are_protocol_errors() {
        let value = json!([{"not": "a string"}, "!!"]);
        let fields = Fields::of(&value).expect("array");
        assert!(matches!(fields.string(1), Err(Error::Protocol(_))));
        assert!(matches!(fields.bytes(2), Err(Error::Protocol(_))));
        assert!(Fields::of(&json!("nope")).is_err());
    }
}
