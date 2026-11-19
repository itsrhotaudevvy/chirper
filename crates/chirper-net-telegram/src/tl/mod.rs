//! TL: the schema layer of MTProto. The generated types live in [`types`], [`enums`] and
//! [`functions`]; they are produced by `build.rs` from the schema files in `tl/`.
//!
//! Every read is bounds-checked through [`Cursor`], and every length or count that comes from the
//! network is checked against the bytes that remain before anything is allocated.

#[cfg(test)]
mod codegen_tests;
mod generated;

pub use generated::{enums, functions, types};

/// The TL layer this schema describes.
pub const LAYER: i32 = 233;

/// Constructor id of the boxed `Vector` type.
pub const VECTOR_ID: u32 = 0x1cb5_c415;
const BOOL_TRUE_ID: u32 = 0x9972_75b5;
const BOOL_FALSE_ID: u32 = 0xbc79_9737;

/// Largest string or byte string this codec will encode (the TL long form has a 24-bit length).
const MAX_BYTES_LEN: usize = (1 << 24) - 1;

/// Nesting limit for deserialised objects.
const MAX_DEPTH: u32 = 64;

/// A type that can be written in TL binary form.
pub trait Serializable {
    fn serialize(&self, buf: &mut Vec<u8>);
}

/// A type that can be read from TL binary form.
pub trait Deserializable: Sized {
    fn deserialize(buf: &mut Cursor<'_>) -> Result<Self, DeserializeError>;
}

/// A constructor with a fixed 32-bit id.
pub trait Identifiable {
    const CONSTRUCTOR_ID: u32;
}

/// A function that can be sent as a request, with the type its answer deserialises to.
pub trait RemoteCall: Serializable {
    type Return: Deserializable;
}

/// Why a TL object could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeserializeError {
    /// The data ended before the object did.
    UnexpectedEnd,
    /// A constructor id that the schema does not define here.
    UnknownConstructor(u32),
    /// A different constructor than the one the caller expected.
    UnexpectedConstructor { expected: u32, found: u32 },
    /// A length or count that is negative, too large for the data, or otherwise impossible.
    InvalidLength,
    /// A `Bool` value that is neither `boolTrue` nor `boolFalse`.
    InvalidBool(u32),
    /// A string that is not UTF-8.
    InvalidUtf8,
    /// Objects nested deeper than the limit.
    TooDeep,
}

impl std::fmt::Display for DeserializeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedEnd => write!(f, "unexpected end of data"),
            Self::UnknownConstructor(id) => write!(f, "unknown constructor {id:#010x}"),
            Self::UnexpectedConstructor { expected, found } => {
                write!(
                    f,
                    "expected constructor {expected:#010x}, found {found:#010x}"
                )
            }
            Self::InvalidLength => write!(f, "invalid length"),
            Self::InvalidBool(id) => write!(f, "invalid bool {id:#010x}"),
            Self::InvalidUtf8 => write!(f, "string is not UTF-8"),
            Self::TooDeep => write!(f, "objects nested too deeply"),
        }
    }
}

impl std::error::Error for DeserializeError {}

/// A bounds-checked reader over TL data.
#[derive(Debug)]
pub struct Cursor<'a> {
    data: &'a [u8],
    pos: usize,
    depth: u32,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            pos: 0,
            depth: 0,
        }
    }

    /// Bytes not read yet.
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    /// The next `n` bytes, or `UnexpectedEnd`.
    pub fn take(&mut self, n: usize) -> Result<&'a [u8], DeserializeError> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.data.len())
            .ok_or(DeserializeError::UnexpectedEnd)?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or(DeserializeError::UnexpectedEnd)?;
        self.pos = end;
        Ok(slice)
    }

    /// Bytes read so far.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// Everything not read yet.
    pub fn take_rest(&mut self) -> &'a [u8] {
        let rest = self.data.get(self.pos..).unwrap_or(&[]);
        self.pos = self.data.len();
        rest
    }

    fn take_array<const N: usize>(&mut self) -> Result<[u8; N], DeserializeError> {
        self.take(N)?
            .try_into()
            .map_err(|_| DeserializeError::UnexpectedEnd)
    }

    pub fn read_u32(&mut self) -> Result<u32, DeserializeError> {
        Ok(u32::from_le_bytes(self.take_array()?))
    }

    pub fn read_i32(&mut self) -> Result<i32, DeserializeError> {
        Ok(i32::from_le_bytes(self.take_array()?))
    }

    /// The next constructor id, without consuming it.
    pub fn peek_u32(&self) -> Result<u32, DeserializeError> {
        let bytes: [u8; 4] = self
            .data
            .get(self.pos..self.pos.saturating_add(4))
            .and_then(|s| s.try_into().ok())
            .ok_or(DeserializeError::UnexpectedEnd)?;
        Ok(u32::from_le_bytes(bytes))
    }

    /// Read a constructor id and require it to be `expected`.
    pub fn expect_constructor(&mut self, expected: u32) -> Result<(), DeserializeError> {
        let found = self.read_u32()?;
        if found == expected {
            Ok(())
        } else {
            Err(DeserializeError::UnexpectedConstructor { expected, found })
        }
    }

    /// Enter one level of nesting. Pair with [`Cursor::leave`] on success.
    pub fn enter(&mut self) -> Result<(), DeserializeError> {
        if self.depth >= MAX_DEPTH {
            return Err(DeserializeError::TooDeep);
        }
        self.depth += 1;
        Ok(())
    }

    pub fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }
}

impl Serializable for i32 {
    fn serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.to_le_bytes());
    }
}

impl Deserializable for i32 {
    fn deserialize(buf: &mut Cursor<'_>) -> Result<Self, DeserializeError> {
        buf.read_i32()
    }
}

impl Serializable for u32 {
    fn serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.to_le_bytes());
    }
}

impl Deserializable for u32 {
    fn deserialize(buf: &mut Cursor<'_>) -> Result<Self, DeserializeError> {
        buf.read_u32()
    }
}

impl Serializable for i64 {
    fn serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.to_le_bytes());
    }
}

impl Deserializable for i64 {
    fn deserialize(buf: &mut Cursor<'_>) -> Result<Self, DeserializeError> {
        Ok(i64::from_le_bytes(buf.take_array()?))
    }
}

impl Serializable for f64 {
    fn serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.to_le_bytes());
    }
}

impl Deserializable for f64 {
    fn deserialize(buf: &mut Cursor<'_>) -> Result<Self, DeserializeError> {
        Ok(f64::from_le_bytes(buf.take_array()?))
    }
}

impl Serializable for [u8; 16] {
    fn serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(self);
    }
}

impl Deserializable for [u8; 16] {
    fn deserialize(buf: &mut Cursor<'_>) -> Result<Self, DeserializeError> {
        buf.take_array()
    }
}

impl Serializable for [u8; 32] {
    fn serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(self);
    }
}

impl Deserializable for [u8; 32] {
    fn deserialize(buf: &mut Cursor<'_>) -> Result<Self, DeserializeError> {
        buf.take_array()
    }
}

/// `Bool` is `boolTrue` or `boolFalse`, each a constructor id with no fields.
impl Serializable for bool {
    fn serialize(&self, buf: &mut Vec<u8>) {
        let id = if *self { BOOL_TRUE_ID } else { BOOL_FALSE_ID };
        buf.extend_from_slice(&id.to_le_bytes());
    }
}

impl Deserializable for bool {
    fn deserialize(buf: &mut Cursor<'_>) -> Result<Self, DeserializeError> {
        match buf.read_u32()? {
            BOOL_TRUE_ID => Ok(true),
            BOOL_FALSE_ID => Ok(false),
            other => Err(DeserializeError::InvalidBool(other)),
        }
    }
}

/// A boxed `Vector<T>` answer is read as its constructor id, count and items.
impl<T: Deserializable> Deserializable for Vec<T> {
    fn deserialize(buf: &mut Cursor<'_>) -> Result<Self, DeserializeError> {
        read_vector(buf, T::deserialize)
    }
}

/// An object whose body is kept as its raw serialised bytes (the schema's `Object`).
#[derive(Clone, PartialEq, Eq, Default)]
pub struct RawObject {
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for RawObject {
    /// Only the length: the bytes may carry message text.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RawObject({} bytes)", self.bytes.len())
    }
}

impl RawObject {
    /// An object whose length is given by a preceding `bytes:int` field.
    pub fn read_sized(buf: &mut Cursor<'_>, len: i32) -> Result<Self, DeserializeError> {
        let len = usize::try_from(len).map_err(|_| DeserializeError::InvalidLength)?;
        Ok(Self {
            bytes: buf.take(len)?.to_vec(),
        })
    }
}

impl Serializable for RawObject {
    fn serialize(&self, buf: &mut Vec<u8>) {
        buf.extend_from_slice(&self.bytes);
    }
}

impl Deserializable for RawObject {
    /// An object without a length prefix runs to the end of the enclosing data.
    fn deserialize(buf: &mut Cursor<'_>) -> Result<Self, DeserializeError> {
        Ok(Self {
            bytes: buf.take_rest().to_vec(),
        })
    }
}

/// TL `string` and `bytes`: one length byte below 254, otherwise `0xFE` and a 3-byte little-endian
/// length; the value is then padded with zeros to a multiple of four bytes.
pub fn write_bytes(buf: &mut Vec<u8>, value: &[u8]) {
    let len = value.len();
    assert!(len <= MAX_BYTES_LEN, "TL byte string too long to encode");
    let header = if len < 254 {
        buf.push(len as u8);
        1
    } else {
        buf.push(254);
        buf.extend_from_slice(&(len as u32).to_le_bytes()[..3]);
        4
    };
    buf.extend_from_slice(value);
    let pad = (4 - (header + len) % 4) % 4;
    buf.extend(std::iter::repeat_n(0u8, pad));
}

/// Read a TL `bytes` value.
pub fn read_bytes(buf: &mut Cursor<'_>) -> Result<Vec<u8>, DeserializeError> {
    let [first] = buf.take_array::<1>()?;
    let (len, header) = match first {
        0..=253 => (usize::from(first), 1),
        254 => {
            let [b0, b1, b2] = buf.take_array::<3>()?;
            let len = usize::from(b0) | (usize::from(b1) << 8) | (usize::from(b2) << 16);
            (len, 4)
        }
        255 => return Err(DeserializeError::InvalidLength),
    };
    let value = buf.take(len)?.to_vec();
    let pad = (4 - (header + len) % 4) % 4;
    buf.take(pad)?;
    Ok(value)
}

/// Read a TL `string`, which must be UTF-8.
pub fn read_string(buf: &mut Cursor<'_>) -> Result<String, DeserializeError> {
    String::from_utf8(read_bytes(buf)?).map_err(|_| DeserializeError::InvalidUtf8)
}

/// Write the id and count of a boxed `Vector`.
pub fn write_vector_header(buf: &mut Vec<u8>, count: usize) {
    buf.extend_from_slice(&VECTOR_ID.to_le_bytes());
    write_len(buf, count);
}

/// Write a bare count (used by bare vectors).
pub fn write_len(buf: &mut Vec<u8>, count: usize) {
    let count = i32::try_from(count).unwrap_or(i32::MAX);
    buf.extend_from_slice(&count.to_le_bytes());
}

/// Read a boxed `Vector`: id, count, then items read by `item`.
pub fn read_vector<'a, T>(
    buf: &mut Cursor<'a>,
    item: impl FnMut(&mut Cursor<'a>) -> Result<T, DeserializeError>,
) -> Result<Vec<T>, DeserializeError> {
    buf.expect_constructor(VECTOR_ID)?;
    read_bare_vector(buf, item)
}

/// Read a bare vector: count, then items read by `item`.
pub fn read_bare_vector<'a, T>(
    buf: &mut Cursor<'a>,
    mut item: impl FnMut(&mut Cursor<'a>) -> Result<T, DeserializeError>,
) -> Result<Vec<T>, DeserializeError> {
    let count = buf.read_i32()?;
    // Every item takes at least four bytes, so a larger count cannot be real.
    let count = usize::try_from(count)
        .ok()
        .filter(|count| *count <= buf.remaining() / 4)
        .ok_or(DeserializeError::InvalidLength)?;
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        items.push(item(buf)?);
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(value: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        write_bytes(&mut buf, value);
        buf
    }

    #[test]
    fn string_of_253_bytes_has_one_byte_header_and_two_pad_bytes() {
        let value = vec![b'a'; 253];
        let encoded = encode(&value);
        assert_eq!(encoded.len(), 256);
        assert_eq!(encoded[0], 253);
        assert_eq!(&encoded[1..254], value.as_slice());
        assert_eq!(&encoded[254..], &[0, 0]);
    }

    #[test]
    fn string_of_254_bytes_uses_the_long_header() {
        let value = vec![b'b'; 254];
        let encoded = encode(&value);
        assert_eq!(encoded.len(), 260);
        assert_eq!(&encoded[..4], &[254, 254, 0, 0]);
        assert_eq!(&encoded[4..258], value.as_slice());
        assert_eq!(&encoded[258..], &[0, 0]);
    }

    #[test]
    fn string_of_1000_bytes_needs_no_padding() {
        let value = vec![7u8; 1000];
        let encoded = encode(&value);
        assert_eq!(encoded.len(), 1004);
        assert_eq!(&encoded[..4], &[254, 232, 3, 0]);
        assert_eq!(&encoded[4..], value.as_slice());
    }

    #[test]
    fn byte_strings_round_trip() {
        for len in [0usize, 1, 3, 4, 252, 253, 254, 255, 1000, 70_000] {
            let value: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let encoded = encode(&value);
            assert_eq!(encoded.len() % 4, 0, "length {len} is not padded to 4");
            let mut cursor = Cursor::new(&encoded);
            assert_eq!(read_bytes(&mut cursor).unwrap(), value);
            assert_eq!(cursor.remaining(), 0);
        }
    }

    #[test]
    fn oversized_string_length_is_rejected_without_allocating() {
        // Long form claiming 16 MiB of data in a 8-byte buffer.
        let data = [254u8, 0xff, 0xff, 0xff, 1, 2, 3, 4];
        let mut cursor = Cursor::new(&data);
        assert_eq!(
            read_bytes(&mut cursor),
            Err(DeserializeError::UnexpectedEnd)
        );
    }

    #[test]
    fn vector_of_long_round_trips_with_its_boxed_header() {
        let values: Vec<i64> = vec![1, -2, i64::MAX];
        let mut buf = Vec::new();
        write_vector_header(&mut buf, values.len());
        for v in &values {
            v.serialize(&mut buf);
        }
        assert_eq!(&buf[..4], &VECTOR_ID.to_le_bytes());
        assert_eq!(buf.len(), 4 + 4 + 3 * 8);
        let mut cursor = Cursor::new(&buf);
        let decoded: Vec<i64> = Deserializable::deserialize(&mut cursor).unwrap();
        assert_eq!(decoded, values);
    }

    #[test]
    fn vector_count_larger_than_the_data_is_rejected() {
        let mut buf = Vec::new();
        write_vector_header(&mut buf, 1_000_000);
        let mut cursor = Cursor::new(&buf);
        let result: Result<Vec<i64>, _> = Deserializable::deserialize(&mut cursor);
        assert_eq!(result, Err(DeserializeError::InvalidLength));
    }

    #[test]
    fn negative_count_is_rejected() {
        let mut buf = VECTOR_ID.to_le_bytes().to_vec();
        buf.extend_from_slice(&(-1i32).to_le_bytes());
        let mut cursor = Cursor::new(&buf);
        let result: Result<Vec<i32>, _> = Deserializable::deserialize(&mut cursor);
        assert_eq!(result, Err(DeserializeError::InvalidLength));
    }

    #[test]
    fn bool_uses_its_constructor_ids() {
        let mut buf = Vec::new();
        true.serialize(&mut buf);
        false.serialize(&mut buf);
        assert_eq!(&buf[..4], &0x9972_75b5u32.to_le_bytes());
        assert_eq!(&buf[4..], &0xbc79_9737u32.to_le_bytes());
        let mut cursor = Cursor::new(&buf);
        assert!(bool::deserialize(&mut cursor).unwrap());
        assert!(!bool::deserialize(&mut cursor).unwrap());
        let mut bad = Cursor::new(&[1u8, 2, 3, 4]);
        assert_eq!(
            bool::deserialize(&mut bad),
            Err(DeserializeError::InvalidBool(0x0403_0201))
        );
    }

    #[test]
    fn nesting_limit_is_enforced() {
        let data = [];
        let mut cursor = Cursor::new(&data);
        for _ in 0..MAX_DEPTH {
            cursor.enter().unwrap();
        }
        assert_eq!(cursor.enter(), Err(DeserializeError::TooDeep));
    }
}
