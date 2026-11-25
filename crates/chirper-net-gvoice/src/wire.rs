//! Protobuf messages on the wire, in the two encodings Google Voice speaks: binary protobuf
//! (`alt=proto` responses) and pblite (JSON arrays, used for every request and for the realtime
//! channel).
//!
//! One [`Schema`] per message describes its fields; the same schema reads both encodings. The
//! schemas are hand-written from `pkg/libgv/gvproto/*.proto` of mautrix-gvoice (AGPL-3.0), and the
//! pblite rules follow `go.mau.fi/util/pblite`: element `n - 1` of the array holds field `n`,
//! repeated fields and nested messages are arrays, bytes are base64 strings, and integers may be
//! strings. Binary decoding goes through `prost`'s low-level varint and key functions.

use std::collections::BTreeMap;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use prost::encoding::{DecodeContext, WireType, decode_key, decode_varint, skip_field};
use serde_json::Value;

use chirper_bridge::{Error, Result};

/// The type of one field.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    /// `string`.
    Str,
    /// `bytes`; base64 text in pblite.
    Bytes,
    /// `int32`, `int64`, `uint32`, `uint64` or an enum: one varint.
    Int,
    /// `bool`: one varint in binary, a JSON boolean in pblite.
    Bool,
    /// `float`: fixed 32-bit in binary, a JSON number in pblite.
    Float,
    /// A nested message.
    Message(&'static Schema),
}

/// One field of a message.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub number: u32,
    pub kind: Kind,
    pub repeated: bool,
}

/// Field-number-keyed description of a message.
#[derive(Debug)]
pub struct Schema {
    pub fields: &'static [Field],
}

impl Schema {
    fn field(&self, number: u32) -> Option<&Field> {
        self.fields.iter().find(|field| field.number == number)
    }
}

/// A singular field.
pub const fn one(number: u32, kind: Kind) -> Field {
    Field {
        number,
        kind,
        repeated: false,
    }
}

/// A repeated field.
pub const fn many(number: u32, kind: Kind) -> Field {
    Field {
        number,
        kind,
        repeated: true,
    }
}

/// One value of a decoded field.
#[derive(Clone, Debug, PartialEq)]
pub enum Val {
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    Msg(Msg),
}

/// A decoded message: field number to its values, in wire order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Msg {
    fields: BTreeMap<u32, Vec<Val>>,
}

static EMPTY: Msg = Msg {
    fields: BTreeMap::new(),
};

impl Msg {
    /// An empty message, for building requests.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append one value to field `number`.
    pub fn add(mut self, number: u32, value: Val) -> Self {
        self.push(number, value);
        self
    }

    /// Append one value to field `number` in place.
    pub fn push(&mut self, number: u32, value: Val) {
        self.fields.entry(number).or_default().push(value);
    }

    /// Every value of field `number`.
    pub fn values(&self, number: u32) -> &[Val] {
        self.fields.get(&number).map_or(&[], Vec::as_slice)
    }

    /// The first string of field `number`, or empty.
    pub fn string(&self, number: u32) -> String {
        self.values(number)
            .iter()
            .find_map(|value| match value {
                Val::Str(text) => Some(text.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// Every string of a repeated field.
    pub fn strings(&self, number: u32) -> Vec<String> {
        self.values(number)
            .iter()
            .filter_map(|value| match value {
                Val::Str(text) => Some(text.clone()),
                _ => None,
            })
            .collect()
    }

    /// The first integer of field `number`, or zero.
    pub fn int(&self, number: u32) -> i64 {
        self.values(number)
            .iter()
            .find_map(|value| match value {
                Val::Int(int) => Some(*int),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// The first boolean of field `number`, false when absent.
    pub fn flag(&self, number: u32) -> bool {
        self.int(number) != 0
    }

    /// The first float of field `number`, or zero.
    pub fn float(&self, number: u32) -> f64 {
        self.values(number)
            .iter()
            .find_map(|value| match value {
                Val::Float(float) => Some(*float),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// The first bytes of field `number`, or empty.
    pub fn bytes(&self, number: u32) -> Vec<u8> {
        self.values(number)
            .iter()
            .find_map(|value| match value {
                Val::Bytes(bytes) => Some(bytes.clone()),
                _ => None,
            })
            .unwrap_or_default()
    }

    /// The first nested message of field `number`, or an empty message.
    pub fn message(&self, number: u32) -> &Msg {
        self.messages(number).next().unwrap_or(&EMPTY)
    }

    /// Every nested message of a repeated field.
    pub fn messages(&self, number: u32) -> impl Iterator<Item = &Msg> {
        self.values(number).iter().filter_map(|value| match value {
            Val::Msg(message) => Some(message),
            _ => None,
        })
    }
}

/// Decode a binary protobuf message. Fields the schema does not know are skipped.
pub fn decode_binary(schema: &Schema, bytes: &[u8]) -> Result<Msg> {
    let mut buf = bytes;
    decode_fields(schema, &mut buf, DecodeContext::default())
}

fn decode_fields(schema: &Schema, buf: &mut &[u8], ctx: DecodeContext) -> Result<Msg> {
    let mut msg = Msg::new();
    while !buf.is_empty() {
        let (tag, wire) = decode_key(buf).map_err(protocol)?;
        let Some(field) = schema.field(tag) else {
            skip_field(wire, tag, buf, ctx.clone()).map_err(protocol)?;
            continue;
        };
        for value in read_field(field, wire, buf, &ctx)? {
            msg.push(tag, value);
        }
    }
    Ok(msg)
}

fn read_field(
    field: &Field,
    wire: WireType,
    buf: &mut &[u8],
    ctx: &DecodeContext,
) -> Result<Vec<Val>> {
    let number = field.number;
    let value = match (field.kind, wire) {
        (Kind::Message(sub), WireType::LengthDelimited) => {
            // Schemas are finite trees, so nesting depth is bounded by the schema itself.
            let mut body = take(buf)?;
            vec![Val::Msg(decode_fields(sub, &mut body, ctx.clone())?)]
        }
        (Kind::Str, WireType::LengthDelimited) => {
            vec![Val::Str(String::from_utf8_lossy(take(buf)?).into_owned())]
        }
        (Kind::Bytes, WireType::LengthDelimited) => vec![Val::Bytes(take(buf)?.to_vec())],
        (Kind::Int | Kind::Bool, WireType::Varint) => {
            vec![Val::Int(decode_varint(buf).map_err(protocol)? as i64)]
        }
        (Kind::Int | Kind::Bool, WireType::LengthDelimited) if field.repeated => {
            let mut body = take(buf)?;
            let mut out = Vec::new();
            while !body.is_empty() {
                out.push(Val::Int(decode_varint(&mut body).map_err(protocol)? as i64));
            }
            out
        }
        (Kind::Float, WireType::ThirtyTwoBit) => vec![Val::Float(take_f32(buf)?)],
        (Kind::Float, WireType::LengthDelimited) if field.repeated => {
            let mut body = take(buf)?;
            let mut out = Vec::new();
            while !body.is_empty() {
                out.push(Val::Float(take_f32(&mut body)?));
            }
            out
        }
        _ => {
            return Err(protocol(format!(
                "field {number} has an unexpected wire type"
            )));
        }
    };
    Ok(value)
}

/// Split one length-delimited body off the front of `buf`.
fn take<'a>(buf: &mut &'a [u8]) -> Result<&'a [u8]> {
    let len = usize::try_from(decode_varint(buf).map_err(protocol)?)
        .map_err(|_| protocol("a length does not fit in memory"))?;
    if len > buf.len() {
        return Err(protocol("a length runs past the end of the message"));
    }
    let (body, rest) = buf.split_at(len);
    *buf = rest;
    Ok(body)
}

fn take_f32(buf: &mut &[u8]) -> Result<f64> {
    if buf.len() < 4 {
        return Err(protocol("a fixed 32-bit field is truncated"));
    }
    let (bytes, rest) = buf.split_at(4);
    *buf = rest;
    let array: [u8; 4] = bytes
        .try_into()
        .map_err(|_| protocol("a fixed 32-bit field is truncated"))?;
    Ok(f64::from(f32::from_le_bytes(array)))
}

/// Decode a pblite message (the JSON array of the message, as parsed).
pub fn decode_pblite(schema: &Schema, value: &Value) -> Result<Msg> {
    let mut items = value
        .as_array()
        .ok_or_else(|| protocol("expected a pblite array"))?
        .as_slice();
    // A trailing object holds fields by their number as a string.
    let extra = match items.last().and_then(Value::as_object) {
        Some(map) => {
            items = &items[..items.len() - 1];
            Some(map)
        }
        None => None,
    };
    let mut msg = Msg::new();
    for field in schema.fields {
        let key = field.number.to_string();
        let index = usize::try_from(field.number.saturating_sub(1)).unwrap_or(usize::MAX);
        let element = extra
            .and_then(|map| map.get(&key))
            .or_else(|| items.get(index));
        let Some(element) = element.filter(|element| !element.is_null()) else {
            continue;
        };
        if field.repeated {
            let list = element
                .as_array()
                .ok_or_else(|| protocol(format!("field {} is not a list", field.number)))?;
            for item in list {
                msg.push(field.number, pblite_value(field.kind, item)?);
            }
        } else {
            msg.push(field.number, pblite_value(field.kind, element)?);
        }
    }
    Ok(msg)
}

fn pblite_value(kind: Kind, value: &Value) -> Result<Val> {
    let decoded = match kind {
        Kind::Str => value.as_str().map(|text| Val::Str(text.to_owned())),
        Kind::Bytes => value
            .as_str()
            .and_then(|text| STANDARD.decode(text).ok())
            .map(Val::Bytes),
        Kind::Int => json_int(value).map(Val::Int),
        Kind::Bool => value
            .as_bool()
            .or_else(|| value.as_f64().map(|number| number != 0.0))
            .map(|flag| Val::Int(i64::from(flag))),
        Kind::Float => value.as_f64().map(Val::Float),
        Kind::Message(sub) => return decode_pblite(sub, value).map(Val::Msg),
    };
    decoded.ok_or_else(|| protocol("a pblite value has the wrong type"))
}

fn json_int(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|number| number as i64))
        .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
}

/// Encode a message as pblite. The array is as long as the highest field number in the schema,
/// like the reference encoder, so trailing positions are present as `null`.
pub fn encode_pblite(schema: &Schema, msg: &Msg) -> Value {
    let width = schema
        .fields
        .iter()
        .map(|field| usize::try_from(field.number).unwrap_or(0))
        .max()
        .unwrap_or(0);
    let mut items = vec![Value::Null; width];
    for field in schema.fields {
        let values = msg.values(field.number);
        let Some(first) = values.first() else {
            continue;
        };
        let value = if field.repeated {
            Value::Array(
                values
                    .iter()
                    .map(|item| pblite_json(field.kind, item))
                    .collect(),
            )
        } else {
            pblite_json(field.kind, first)
        };
        if let Some(slot) = usize::try_from(field.number)
            .ok()
            .and_then(|number| number.checked_sub(1))
            .and_then(|index| items.get_mut(index))
        {
            *slot = value;
        }
    }
    Value::Array(items)
}

fn pblite_json(kind: Kind, value: &Val) -> Value {
    match (kind, value) {
        (Kind::Str, Val::Str(text)) => Value::String(text.clone()),
        (Kind::Bytes, Val::Bytes(bytes)) => Value::String(STANDARD.encode(bytes)),
        (Kind::Int, Val::Int(int)) => Value::from(*int),
        (Kind::Bool, Val::Int(int)) => Value::Bool(*int != 0),
        (Kind::Float, Val::Float(float)) => {
            serde_json::Number::from_f64(*float).map_or(Value::Null, Value::Number)
        }
        (Kind::Message(sub), Val::Msg(msg)) => encode_pblite(sub, msg),
        _ => Value::Null,
    }
}

/// A protocol error for input the driver cannot use.
pub fn protocol(detail: impl std::fmt::Display) -> Error {
    Error::Protocol(format!("Google Voice sent an unusable message: {detail}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    static LEAF: Schema = Schema {
        fields: &[one(1, Kind::Str)],
    };
    static SAMPLE: Schema = Schema {
        fields: &[
            one(1, Kind::Str),
            one(2, Kind::Int),
            one(3, Kind::Message(&LEAF)),
            many(4, Kind::Str),
            one(6, Kind::Bool),
            one(7, Kind::Bytes),
            many(8, Kind::Int),
            one(9, Kind::Float),
        ],
    };

    #[test]
    fn pblite_encodes_at_field_number_minus_one() {
        let request = Msg::new().add(2, Val::Int(1));
        static REQ: Schema = Schema {
            fields: &[one(1, Kind::Int), one(2, Kind::Int)],
        };
        assert_eq!(encode_pblite(&REQ, &request), json!([null, 1]));
    }

    #[test]
    fn pblite_pads_to_the_highest_schema_field() {
        let msg = Msg::new().add(1, Val::Str("a".into()));
        assert_eq!(
            encode_pblite(&SAMPLE, &msg),
            json!(["a", null, null, null, null, null, null, null, null])
        );
    }

    #[test]
    fn pblite_round_trips_every_kind() {
        let msg = Msg::new()
            .add(1, Val::Str("hi".into()))
            .add(2, Val::Int(-7))
            .add(3, Val::Msg(Msg::new().add(1, Val::Str("leaf".into()))))
            .add(4, Val::Str("x".into()))
            .add(4, Val::Str("y".into()))
            .add(6, Val::Int(1))
            .add(7, Val::Bytes(vec![0, 255, 16]))
            .add(8, Val::Int(3))
            .add(8, Val::Int(4))
            .add(9, Val::Float(0.5));
        let encoded = encode_pblite(&SAMPLE, &msg);
        assert_eq!(decode_pblite(&SAMPLE, &encoded).expect("decodes"), msg);
    }

    #[test]
    fn pblite_accepts_string_integers_and_bare_booleans() {
        let value = json!(["a", "12", null, null, null, true, null, [3, 4], 1.5]);
        let msg = decode_pblite(&SAMPLE, &value).expect("decodes");
        assert_eq!(msg.int(2), 12);
        assert!(msg.flag(6));
        assert_eq!(msg.values(8), &[Val::Int(3), Val::Int(4)]);
        assert!((msg.float(9) - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn pblite_reads_a_trailing_object_as_extra_fields() {
        let value = json!(["a", "12", null, null, null, null, null, null, null, {"2": 9}]);
        let msg = decode_pblite(&SAMPLE, &value).expect("decodes");
        assert_eq!(msg.int(2), 9);
        assert_eq!(msg.string(1), "a");
    }

    #[test]
    fn pblite_rejects_wrong_shapes() {
        assert!(matches!(
            decode_pblite(&SAMPLE, &json!({"a": 1})),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(
            decode_pblite(&SAMPLE, &json!([1])),
            Err(Error::Protocol(_))
        ));
        assert!(matches!(
            decode_pblite(&SAMPLE, &json!([null, null, "not a list"])),
            Err(Error::Protocol(_))
        ));
    }

    #[test]
    fn binary_reads_known_fields_and_skips_unknown_ones() {
        // field 1 "ab", field 2 varint 300, field 3 {1: "x"}, field 50 varint 1 (unknown).
        let bytes = [
            0x0a, 0x02, b'a', b'b', 0x10, 0xac, 0x02, 0x1a, 0x03, 0x0a, 0x01, b'x', 0x80, 0x03,
            0x01,
        ];
        let msg = decode_binary(&SAMPLE, &bytes).expect("decodes");
        assert_eq!(msg.string(1), "ab");
        assert_eq!(msg.int(2), 300);
        assert_eq!(msg.message(3).string(1), "x");
    }

    #[test]
    fn binary_reads_packed_repeated_scalars() {
        // field 8 packed {3, 4}, then field 4 "y" as a plain repeated string.
        let bytes = [0x42, 0x02, 0x03, 0x04, 0x22, 0x01, b'y'];
        let msg = decode_binary(&SAMPLE, &bytes).expect("decodes");
        assert_eq!(msg.values(8), &[Val::Int(3), Val::Int(4)]);
        assert_eq!(msg.strings(4), vec!["y".to_owned()]);
    }

    #[test]
    fn binary_reads_fixed32_floats() {
        // field 9 as fixed32 1.5.
        let bytes = [0x4d, 0x00, 0x00, 0xc0, 0x3f];
        let msg = decode_binary(&SAMPLE, &bytes).expect("decodes");
        assert!((msg.float(9) - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn binary_refuses_truncated_or_mistyped_input() {
        // A string whose length runs past the end.
        assert!(matches!(
            decode_binary(&SAMPLE, &[0x0a, 0x05, b'a']),
            Err(Error::Protocol(_))
        ));
        // Field 1 sent as a varint instead of a string.
        assert!(matches!(
            decode_binary(&SAMPLE, &[0x08, 0x01]),
            Err(Error::Protocol(_))
        ));
    }
}
