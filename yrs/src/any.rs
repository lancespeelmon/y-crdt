use crate::encoding::read::{Error, Read};
use crate::encoding::write::Write;
use serde::de::Visitor;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cmp::PartialEq;
use std::collections::HashMap;
use std::convert::TryFrom;
use std::fmt::Formatter;
use std::sync::Arc;

/// The largest integer a JavaScript number holds exactly, `2^53 - 1` (`Number.MAX_SAFE_INTEGER`).
pub const F64_MAX_SAFE_INTEGER: f64 = (i64::pow(2, 53) - 1) as f64;
/// The smallest integer a JavaScript number holds exactly, `-(2^53 - 1)` (`Number.MIN_SAFE_INTEGER`).
pub const F64_MIN_SAFE_INTEGER: f64 = -F64_MAX_SAFE_INTEGER;

/// Any is an enum with a potentially associated value that is used to represent JSON values
/// and supports efficient encoding of those values.
#[derive(Debug, Clone, PartialEq)]
pub enum Any {
    /// JavaScript `null`.
    Null,
    /// JavaScript `undefined`.
    Undefined,
    /// A boolean.
    Bool(bool),
    /// A [Number]: an integer, a float or a big integer.
    Number(Number),
    /// A string.
    String(Arc<str>),
    /// A binary buffer.
    Buffer(Arc<[u8]>),
    /// A JS array.
    Array(Arc<[Any]>),
    /// A JS object. Entries keep the order they were inserted (decoded) in, as JS object keys
    /// do, so a decoded map re-encodes to the same bytes.
    Map(Arc<AnyMap>),
}

/// The entries of an [Any::Map], in insertion order.
pub type AnyMap = indexmap::IndexMap<String, Any>;

impl Any {
    /// The deepest nesting of arrays and maps that [Any::decode] accepts. Deeper input fails with
    /// [Error::UnexpectedValue].
    ///
    /// Decoding, encoding, comparing and dropping an [Any] all recurse once per level, so a value
    /// nested too deeply overflows the stack, which aborts the process. Accepting it and running
    /// the decoder on a bigger stack would not help, as the value would still have to be dropped
    /// on an ordinary one.
    pub const MAX_DECODE_DEPTH: usize = 256;

    /// Converts this value into `T`. Returns the value back as the error if it can't be converted.
    #[inline]
    pub fn cast<T>(self) -> Result<T, Self>
    where
        T: TryFrom<Any, Error = Any>,
    {
        // we create dedicated cast, so that we can parametrize it in a fluent fashion
        // ie. `any.cast::<u32>().unwrap_or_default()`.
        T::try_from(self)
    }

    /// Reads a value from a `decoder`, in the lib0 "any" encoding. Fails if the data is malformed or nested deeper than [Any::MAX_DECODE_DEPTH].
    pub fn decode<R: Read>(decoder: &mut R) -> Result<Self, Error> {
        Self::decode_nested(decoder, 0)
    }

    // Nested values recurse through `decode_nested` and `decode_map`/`decode_array`, so those
    // frames, times [Any::MAX_DECODE_DEPTH], are what decoding needs of the stack. The scalar
    // cases live in `decode_scalar` to keep their locals out of the recursive frames.
    fn decode_nested<R: Read>(decoder: &mut R, depth: usize) -> Result<Self, Error> {
        let tag = decoder.read_u8()?;
        match tag {
            117 | 118 if depth >= Self::MAX_DECODE_DEPTH => Err(Error::UnexpectedValue),
            // CASE 118: Map<string,Any>
            118 => Self::decode_map(decoder, depth),
            // CASE 117: Array<Any>
            117 => Self::decode_array(decoder, depth),
            _ => Self::decode_scalar(decoder, tag),
        }
    }

    #[inline(never)]
    fn decode_map<R: Read>(decoder: &mut R, depth: usize) -> Result<Self, Error> {
        let len: usize = decoder.read_var()?;
        let mut map = AnyMap::new();
        map.try_reserve(Self::reservation(decoder, len, depth))
            .map_err(|_| Error::UnexpectedValue)?;
        for _ in 0..len {
            let key = decoder.read_string()?.to_owned();
            let value = Self::decode_nested(decoder, depth + 1)?;
            map.insert(key, value);
        }
        Ok(Any::Map(Arc::new(map)))
    }

    #[inline(never)]
    fn decode_array<R: Read>(decoder: &mut R, depth: usize) -> Result<Self, Error> {
        let len: usize = decoder.read_var()?;
        let mut arr = Vec::new();
        arr.try_reserve(Self::reservation(decoder, len, depth))?;
        for _ in 0..len {
            arr.push(Self::decode_nested(decoder, depth + 1)?);
        }
        Ok(Any::Array(Arc::from(arr)))
    }

    #[inline(never)]
    fn decode_scalar<R: Read>(decoder: &mut R, tag: u8) -> Result<Self, Error> {
        Ok(match tag {
            // CASE 127: undefined
            127 => Any::Undefined,
            // CASE 126: null
            126 => Any::Null,
            // CASE 125: integer. lib0 keeps the sign of -0, which only a float can hold.
            125 => {
                let n = decoder.read_var_signed::<i64>()?;
                if n.value() == 0 && n.is_negative() {
                    Any::Number(Number::Float(-0.0))
                } else {
                    Any::Number(Number::Int(n.value()))
                }
            }
            // CASE 124: float32
            124 => Any::Number(Number::Float(decoder.read_f32()? as f64)),
            // CASE 123: float64
            123 => Any::Number(Number::Float(decoder.read_f64()?)),
            // CASE 122: bigint
            122 => Any::Number(Number::BigInt(decoder.read_i64()?)),
            // CASE 121: boolean (false)
            121 => Any::Bool(false),
            // CASE 120: boolean (true)
            120 => Any::Bool(true),
            // CASE 119: string
            119 => {
                let str = decoder.read_string()?;
                Any::String(Arc::from(str))
            }
            // CASE 116: buffer
            116 => Any::Buffer(Arc::from(decoder.read_buf()?)),
            _ => return Err(Error::UnexpectedValue),
        })
    }

    /// How many elements to reserve up front for a container claiming `len` of them.
    ///
    /// `len` comes from the input and every element costs at least one more byte, so the unread
    /// input is an upper bound. Only the outermost container reserves: the containers open on the
    /// way down to a nested one all claim the same unread bytes, so reserving at every level
    /// would add up to [Any::MAX_DECODE_DEPTH] times the input. Inner containers grow as their
    /// elements are decoded.
    fn reservation<R: Read>(decoder: &R, len: usize, depth: usize) -> usize {
        if depth == 0 {
            len.min(decoder.remaining())
        } else {
            0
        }
    }

    // Encode data with efficient binary format.
    //
    // Differences to JSON:
    // • Transforms data to a binary format (not to a string)
    // • Encodes undefined, NaN, and ArrayBuffer (these can't be represented in JSON)
    // • Numbers are efficiently encoded either as a variable length integer, as a
    //   32 bit float, as a 64 bit float, or as a 64 bit bigint.
    //
    // Encoding table:
    //
    // | Data Type           | Prefix   | Encoding Method    | Comment |
    // | ------------------- | -------- | ------------------ | ------- |
    // | undefined           | 127      |                    | Functions, symbol, and everything that cannot be identified is encoded as undefined |
    // | null                | 126      |                    | |
    // | integer             | 125      | writeVarInt        | Only encodes 32 bit signed integers |
    // | float32             | 124      | writeFloat32       | |
    // | float64             | 123      | writeFloat64       | |
    // | bigint              | 122      | writeBigInt64      | |
    // | boolean (false)     | 121      |                    | True and false are different data types so we save the following byte |
    // | boolean (true)      | 120      |                    | - 0b01111000 so the last bit determines whether true or false |
    // | string              | 119      | writeVarString     | |
    // | object<string,any>  | 118      | custom             | Writes {length} then {length} key-value pairs |
    // | array<any>          | 117      | custom             | Writes {length} then {length} json values |
    // | Uint8Array          | 116      | writeVarUint8Array | We use Uint8Array for any kind of binary data |
    //
    // Reasons for the decreasing prefix:
    // We need the first bit for extendability (later we may want to encode the
    // prefix with writeVarUint). The remaining 7 bits are divided as follows:
    // [0-30]   the beginning of the data range is used for custom purposes
    //          (defined by the function that uses this library)
    // [31-127] the end of the data range is used for data encoding by
    //          lib0/encoding.js
    /// Writes this value into an `encoder`, in the lib0 "any" encoding.
    pub fn encode<W: Write>(&self, encoder: &mut W) {
        match self {
            Any::Undefined => {
                // TYPE 127: undefined
                encoder.write_u8(127)
            }
            Any::Null => {
                // TYPE 126: null
                encoder.write_u8(126)
            }
            Any::Bool(bool) => {
                // TYPE 120/121: boolean (true/false)
                encoder.write_u8(if *bool { 120 } else { 121 })
            }
            Any::String(str) => {
                // TYPE 119: String
                encoder.write_u8(119);
                encoder.write_string(&str)
            }
            Any::Number(num) => {
                match *num {
                    Number::Int(n) => {
                        // ensure max compatibility with yjs lib0 number encoding
                        if n >= -0x7FFFFFFF && n <= 0x7FFFFFFF {
                            // TYPE 125: INTEGER
                            encoder.write_u8(125);
                            encoder.write_var(n)
                        } else if let Some(f) = exact_f64(n) {
                            // A JS number, as lib0 writes one, when it holds `n` exactly.
                            if (f as f32) as f64 == f {
                                // TYPE 124: FLOAT32
                                encoder.write_u8(124);
                                encoder.write_f32(f as f32)
                            } else {
                                // TYPE 123: FLOAT64
                                encoder.write_u8(123);
                                encoder.write_f64(f)
                            }
                        } else {
                            // TYPE 122: BigInt. Rounding to a JS number would leave the peer
                            // that wrote `n` with a different value from all the others.
                            encoder.write_u8(122);
                            encoder.write_i64(n)
                        }
                    }
                    Number::BigInt(n) => {
                        // TYPE 122: BigInt
                        encoder.write_u8(122);
                        encoder.write_i64(n)
                    }
                    Number::Float(n) => {
                        if (n as f32) as f64 == n {
                            // TYPE 124: FLOAT32
                            encoder.write_u8(124);
                            encoder.write_f32(n as f32)
                        } else {
                            // TYPE 123: FLOAT64
                            encoder.write_u8(123);
                            encoder.write_f64(n)
                        }
                    }
                }
            }
            Any::Array(arr) => {
                // TYPE 117: Array
                encoder.write_u8(117);
                encoder.write_var(arr.len() as u64);
                for el in arr.iter() {
                    el.encode(encoder);
                }
            }
            Any::Map(map) => {
                // TYPE 118: Map
                encoder.write_u8(118);
                encoder.write_var(map.len() as u64);
                for (key, value) in map.as_ref() {
                    encoder.write_string(&key);
                    value.encode(encoder);
                }
            }
            Any::Buffer(buf) => {
                // TYPE 116: Buffer
                encoder.write_u8(116);
                encoder.write_buf(&buf)
            }
        }
    }

    /// Parses a value from a JSON string.
    pub fn from_json(src: &str) -> Result<Self, Error> {
        Ok(serde_json::from_str(src)?)
    }

    /// Appends this value, serialized as JSON, to `buf`.
    pub fn to_json(&self, buf: &mut String) {
        use serde::Serialize;
        use serde_json::Serializer;

        let buf = unsafe { buf.as_mut_vec() };
        let cursor = std::io::Cursor::new(buf);

        let mut s = Serializer::new(cursor);
        self.serialize(&mut s).unwrap();
    }

    /// Returns an iterator over an inner values of an array or a map, current reference represents.
    pub fn try_iter(&self) -> Option<AnyIter<'_>> {
        match self {
            Any::Array(values) => Some(AnyIter::Array(values.iter())),
            Any::Map(entries) => Some(AnyIter::Map(entries.iter())),
            _ => None,
        }
    }

    /// Consumes this value and returns an iterator over the inner values of an array or a map. Returns `None` for any other value.
    pub fn try_into_iter(self) -> Option<AnyIntoIter> {
        match self {
            Any::Array(values) => Some(AnyIntoIter::from(values.clone())),
            Any::Map(entries) => Some(AnyIntoIter::from(entries.clone())),
            _ => None,
        }
    }
}

impl std::fmt::Display for Any {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Any::Null => f.write_str("null"),
            Any::Undefined => f.write_str("undefined"),
            Any::Bool(value) => write!(f, "{}", value),
            Any::Number(value) => write!(f, "{}", value),
            Any::String(value) => f.write_str(value.as_ref()),
            Any::Array(values) => {
                write!(f, "[")?;
                let mut i = values.iter();
                if let Some(value) = i.next() {
                    value.fmt(f)?;
                }
                while let Some(value) = i.next() {
                    write!(f, ", ")?;
                    value.fmt(f)?;
                }
                write!(f, "]")
            }
            Any::Map(entries) => {
                write!(f, "{{")?;
                let mut i = entries.iter();
                if let Some((key, value)) = i.next() {
                    write!(f, "{}: {}", key, value)?;
                }
                while let Some((key, value)) = i.next() {
                    write!(f, ", {}: {}", key, value)?;
                }
                write!(f, "}}")
            }
            Any::Buffer(value) => {
                f.write_str("0x")?;
                for &byte in value.iter() {
                    write!(f, "{:02x}", byte)?;
                }
                Ok(())
            }
        }
    }
}

/// `n` as an `f64`, if that is exact. `n as f64 as i64` alone saturates: `i64::MAX` rounds to
/// 2^63, which converts back to `i64::MAX`.
fn exact_f64(n: i64) -> Option<f64> {
    let f = n as f64;
    (f < 9_223_372_036_854_775_808.0 && f as i64 == n).then_some(f)
}

/// A number in an [Any]: what a JavaScript number holds, plus `BigInt`.
#[derive(Debug, Clone, Copy)]
pub enum Number {
    /// An integer. On the wire it is a JS number when one holds it exactly (always within
    /// ±2^53, see [Number::I64_MAX_SAFE_INTEGER]), and a `BigInt` otherwise, so every peer
    /// reads the same value. Peers decode those as [Number::BigInt], which compares equal.
    Int(i64),
    /// A float. `-0.0` keeps its sign.
    Float(f64),
    /// A JavaScript `BigInt`. It is encoded as one (lib0 tag 122), so Yjs reads a `bigint` and
    /// every 64-bit value survives the round trip.
    BigInt(i64),
}

impl Number {
    /// The largest integer that a JavaScript number holds exactly, `2^53 - 1`.
    pub const I64_MAX_SAFE_INTEGER: i64 = i64::pow(2, 53) - 1;
    /// The smallest integer that a JavaScript number holds exactly, `-(2^53 - 1)`.
    pub const I64_MIN_SAFE_INTEGER: i64 = -Self::I64_MAX_SAFE_INTEGER;
    /// [Number::I64_MAX_SAFE_INTEGER] as a float.
    pub const F64_MAX_SAFE_INTEGER: f64 = Self::I64_MAX_SAFE_INTEGER as f64;
    /// [Number::I64_MIN_SAFE_INTEGER] as a float.
    pub const F64_MIN_SAFE_INTEGER: f64 = -Self::F64_MAX_SAFE_INTEGER;

    /// `value` as an [Number::Int] if it is a safe integer (but not `-0.0`), otherwise as a
    /// [Number::Float].
    pub fn try_i64(value: f64) -> Self {
        if value.trunc() == value
            && !(value == 0.0 && value.is_sign_negative())
            && value >= Self::F64_MIN_SAFE_INTEGER
            && value <= Self::F64_MAX_SAFE_INTEGER
        {
            Number::Int(value as i64)
        } else {
            Number::Float(value)
        }
    }

    /// Returns this number as an `i64`, or `None` if it's a float that isn't a whole number within the `i64` range.
    pub fn as_i64(self) -> Option<i64> {
        match self {
            Number::Int(value) | Number::BigInt(value) => Some(value),
            Number::Float(value) => {
                // Lossless only for whole numbers in i64's range: `as` saturates, so 2^63 would
                // come out as i64::MAX.
                if value.trunc() == value
                    && value >= -9_223_372_036_854_775_808.0
                    && value < 9_223_372_036_854_775_808.0
                {
                    Some(value as i64)
                } else {
                    None
                }
            }
        }
    }

    /// Returns this number as an `f64`, or `None` if an integer can't be represented exactly as one.
    pub fn as_f64(self) -> Option<f64> {
        match self {
            Number::Int(value) | Number::BigInt(value) => exact_f64(value),
            Number::Float(value) => Some(value),
        }
    }
}

impl PartialEq for Number {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Number::Float(a), Number::Float(b)) => a == b,
            (Number::Float(_), _) | (_, Number::Float(_)) => {
                match (self.as_f64(), other.as_f64()) {
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                }
            }
            // Int and BigInt compare by value.
            _ => self.as_i64() == other.as_i64(),
        }
    }
}

impl From<f64> for Number {
    #[inline]
    fn from(value: f64) -> Self {
        Number::Float(value)
    }
}

impl From<i64> for Number {
    #[inline]
    fn from(value: i64) -> Self {
        Number::Int(value)
    }
}

impl TryFrom<u64> for Number {
    type Error = u64;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value <= i64::MAX as u64 {
            Ok(Number::Int(value as i64))
        } else {
            Err(value)
        }
    }
}

impl Serialize for Number {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::Error;
        if let Number::BigInt(v) = self {
            return serializer.serialize_i64(*v);
        }
        match self.as_f64() {
            Some(v) => serializer.serialize_f64(v),
            None => match self.as_i64() {
                Some(v) => serializer.serialize_i64(v),
                None => Err(S::Error::custom("cannot serialize number")),
            },
        }
    }
}

impl<'de> Deserialize<'de> for Number {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct NumberVisitor;
        impl<'de> Visitor<'de> for NumberVisitor {
            type Value = Number;

            fn expecting(&self, formatter: &mut Formatter) -> std::fmt::Result {
                write!(formatter, "number")
            }

            fn visit_u64<E>(self, v: u64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v > (i64::MAX as u64) {
                    return Err(E::custom("integer outside of bounds of i64"));
                }
                Ok(Number::Int(v as i64))
            }

            fn visit_i64<E>(self, value: i64) -> Result<Number, E> {
                Ok(Number::Int(value))
            }

            fn visit_f64<E>(self, v: f64) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                if v <= Number::F64_MAX_SAFE_INTEGER
                    && v > Number::F64_MIN_SAFE_INTEGER
                    && v.trunc() == v
                    && !(v == 0.0 && v.is_sign_negative())
                {
                    Ok(Number::Int(v as i64))
                } else {
                    Ok(Number::Float(v))
                }
            }
        }

        deserializer.deserialize_any(NumberVisitor)
    }
}

impl std::fmt::Display for Number {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Number::Int(value) | Number::BigInt(value) => write!(f, "{}", value),
            Number::Float(value) => write!(f, "{}", value),
        }
    }
}

/// Iterator over the inner values of an [Any] array or map, created by [Any::try_iter]. Items are the entry's key (`None` for arrays) and its value.
pub enum AnyIter<'a> {
    /// Iterates over the elements of an array.
    Array(std::slice::Iter<'a, Any>),
    /// Iterates over the entries of a map.
    Map(indexmap::map::Iter<'a, String, Any>),
}

impl<'a> Iterator for AnyIter<'a> {
    type Item = (Option<&'a str>, &'a Any);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            AnyIter::Array(iter) => {
                let value = iter.next()?;
                Some((None, value))
            }
            AnyIter::Map(iter) => {
                let (key, value) = iter.next()?;
                Some((Some(key), value))
            }
        }
    }
}

/// Iterator over the elements of an [Any] array that owns a handle to it. Items are clones of the elements.
pub struct AnyArrayIter {
    source: Arc<[Any]>,
    index: usize,
}

impl AnyArrayIter {
    /// Creates an iterator over the elements of `source`.
    pub fn new(source: Arc<[Any]>) -> Self {
        Self { source, index: 0 }
    }
}

impl Iterator for AnyArrayIter {
    type Item = Any;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.source.len() {
            let value = self.source[self.index].clone();
            self.index += 1;
            Some(value)
        } else {
            None
        }
    }
}

/// Iterator created by [Any::try_into_iter], over the inner values of an [Any] array or map.
pub enum AnyIntoIter {
    /// Iterates over the elements of an array.
    Array(AnyArrayIter),
    //TODO: we need to clone the map, because we need to consume its iterator
    // try to figure out something better
    /// Iterates over the entries of a map.
    Map(indexmap::map::IntoIter<String, Any>),
}

impl From<Arc<[Any]>> for AnyIntoIter {
    fn from(value: Arc<[Any]>) -> Self {
        Self::Array(AnyArrayIter::new(value))
    }
}

impl From<Arc<AnyMap>> for AnyIntoIter {
    fn from(value: Arc<AnyMap>) -> Self {
        Self::Map((&*value).clone().into_iter())
    }
}

impl Iterator for AnyIntoIter {
    type Item = (Option<String>, Any);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            AnyIntoIter::Array(iter) => {
                let n = iter.next()?;
                Some((None, n))
            }
            AnyIntoIter::Map(iter) => {
                let (key, value) = iter.next()?;
                Some((Some(key.clone()), value.clone()))
            }
        }
    }
}

macro_rules! impl_from_float {
    ($t:ty) => {
        impl From<$t> for Any {
            #[inline]
            fn from(v: $t) -> Self {
                Self::Number(Number::Float(v as f64))
            }
        }

        impl TryFrom<Any> for $t {
            type Error = Any;

            fn try_from(v: Any) -> Result<Self, Self::Error> {
                match v {
                    Any::Number(num) => match num.as_f64() {
                        Some(n) => Ok(n as Self),
                        None => Err(v),
                    },
                    other => Err(other),
                }
            }
        }
    };
}
macro_rules! impl_from_int {
    ($t:ty) => {
        impl From<$t> for Any {
            fn from(value: $t) -> Self {
                Any::Number(Number::from(value as i64))
            }
        }

        impl TryFrom<Any> for $t {
            type Error = Any;

            fn try_from(v: Any) -> Result<Self, Self::Error> {
                match v {
                    Any::Number(num) => match num.as_i64() {
                        Some(n) => Ok(n as Self),
                        None => Err(v),
                    },
                    other => Err(other),
                }
            }
        }
    };
}

impl_from_float!(f32);
impl_from_float!(f64);
impl_from_int!(i16);
impl_from_int!(i32);
impl_from_int!(u16);
impl_from_int!(u32);
impl_from_int!(i64);
impl_from_int!(isize);

impl TryFrom<u64> for Any {
    type Error = u64;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        Ok(Any::Number(Number::try_from(value)?))
    }
}

impl TryFrom<Any> for u64 {
    type Error = Any;

    fn try_from(v: Any) -> Result<Self, Self::Error> {
        match v {
            Any::Number(num) => match num.as_i64() {
                Some(n) if n >= 0 => Ok(n as u64),
                _ => Err(Any::Number(num)),
            },
            other => Err(other),
        }
    }
}

impl TryFrom<usize> for Any {
    type Error = usize;

    #[cfg(target_pointer_width = "32")]
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        // for 32-bit architectures we know that usize will always fit,
        // so there's no need to check for length, but we stick to TryInto
        // trait to keep API compatibility
        Ok(Any::Number(Number::Int(value as i64)))
    }

    #[cfg(target_pointer_width = "64")]
    fn try_from(value: usize) -> Result<Self, Self::Error> {
        use std::convert::TryInto;
        if let Ok(v) = (value as u64).try_into() {
            Ok(v)
        } else {
            Err(value)
        }
    }
}

impl TryFrom<Any> for usize {
    type Error = Any;

    fn try_from(v: Any) -> Result<Self, Self::Error> {
        match v {
            Any::Number(num) => match num.as_i64() {
                Some(n) if n >= 0 => Ok(n as usize),
                _ => Err(v),
            },
            other => Err(other),
        }
    }
}

impl From<bool> for Any {
    #[inline]
    fn from(value: bool) -> Self {
        Any::Bool(value)
    }
}

impl TryFrom<Any> for bool {
    type Error = Any;

    fn try_from(v: Any) -> Result<Self, Self::Error> {
        match v {
            Any::Bool(num) => Ok(num),
            other => Err(other),
        }
    }
}

impl From<String> for Any {
    #[inline]
    fn from(value: String) -> Self {
        Any::String(value.into())
    }
}

impl TryFrom<Any> for String {
    type Error = Any;

    fn try_from(v: Any) -> Result<Self, Self::Error> {
        match v {
            Any::String(value) => Ok(String::from(value.as_ref())),
            other => Err(other),
        }
    }
}

impl From<&str> for Any {
    #[inline]
    fn from(value: &str) -> Self {
        Any::String(value.into())
    }
}

impl From<Arc<str>> for Any {
    #[inline]
    fn from(value: Arc<str>) -> Self {
        Any::String(value.clone())
    }
}

impl TryFrom<Any> for Arc<str> {
    type Error = Any;

    fn try_from(v: Any) -> Result<Self, Self::Error> {
        match v {
            Any::String(value) => Ok(value),
            other => Err(other),
        }
    }
}

impl From<Vec<u8>> for Any {
    #[inline]
    fn from(value: Vec<u8>) -> Self {
        Any::Buffer(Arc::from(value))
    }
}

impl TryFrom<Any> for Vec<u8> {
    type Error = Any;

    fn try_from(v: Any) -> Result<Self, Self::Error> {
        match v {
            Any::Buffer(value) => Ok(Vec::from(value.as_ref())),
            other => Err(other),
        }
    }
}

impl From<Arc<[u8]>> for Any {
    #[inline]
    fn from(value: Arc<[u8]>) -> Self {
        Any::Buffer(value)
    }
}

impl TryFrom<Any> for Arc<[u8]> {
    type Error = Any;

    fn try_from(v: Any) -> Result<Self, Self::Error> {
        match v {
            Any::Buffer(value) => Ok(value),
            other => Err(other),
        }
    }
}

impl From<&[u8]> for Any {
    #[inline]
    fn from(value: &[u8]) -> Self {
        Any::Buffer(Arc::from(value))
    }
}

impl<T> From<Option<T>> for Any
where
    T: Into<Any>,
{
    fn from(v: Option<T>) -> Any {
        match v {
            None => Any::Null,
            Some(value) => value.into(),
        }
    }
}

impl<T> From<Vec<T>> for Any
where
    T: Into<Any>,
{
    fn from(v: Vec<T>) -> Any {
        let mut array = Vec::with_capacity(v.len());
        for value in v {
            array.push(value.into())
        }
        Any::Array(Arc::from(array))
    }
}

impl<T> From<HashMap<String, T>> for Any
where
    T: Into<Any>,
{
    /// The entries come in the `HashMap`'s order. Build from an [AnyMap] (or any ordered
    /// iterator, with `collect`) to choose it.
    fn from(v: HashMap<String, T>) -> Any {
        v.into_iter().collect()
    }
}

impl<T> From<indexmap::IndexMap<String, T>> for Any
where
    T: Into<Any>,
{
    fn from(v: indexmap::IndexMap<String, T>) -> Any {
        v.into_iter().collect()
    }
}

impl<K, T> std::iter::FromIterator<(K, T)> for Any
where
    K: Into<String>,
    T: Into<Any>,
{
    /// An [Any::Map] with the entries in iteration order.
    fn from_iter<I: IntoIterator<Item = (K, T)>>(iter: I) -> Self {
        let map: AnyMap = iter
            .into_iter()
            .map(|(k, v)| (k.into(), v.into()))
            .collect();
        Any::Map(Arc::new(map))
    }
}

// This code is based on serde_json::json! macro (see: https://docs.rs/serde_json/latest/src/serde_json/macros.rs.html#53-58).
// Kudos to the original authors.

/// Construct a lib0 [Any] value literal.
///
/// # Examples
///
/// ```rust
///
/// use yrs::any;
///
/// let value = any!({
///   "code": 200,
///   "success": true,
///   "payload": {
///     "features": [
///       "lib0",
///       true
///     ]
///   }
/// });
/// ```
#[macro_export(local_inner_macros)]
macro_rules! any {
    // Hide distracting implementation details from the generated rustdoc.
    ($($any:tt)+) => {
        any_internal!($($any)+)
    };
}

#[macro_export(local_inner_macros)]
#[doc(hidden)]
macro_rules! any_internal {
    (@array [$($items:expr,)*]) => {
        any_internal_array![$($items,)*]
    };

    // Done without trailing comma.
    (@array [$($items:expr),*]) => {
        any_internal_array![$($items),*]
    };

    // Next item is `null`.
    (@array [$($items:expr,)*] null $($rest:tt)*) => {
        any_internal!(@array [$($items,)* any_internal!(null)] $($rest)*)
    };

    // Next item is `true`.
    (@array [$($items:expr,)*] true $($rest:tt)*) => {
        any_internal!(@array [$($items,)* any_internal!(true)] $($rest)*)
    };

    // Next item is `false`.
    (@array [$($items:expr,)*] false $($rest:tt)*) => {
        any_internal!(@array [$($items,)* any_internal!(false)] $($rest)*)
    };

    // Next item is an array.
    (@array [$($items:expr,)*] [$($array:tt)*] $($rest:tt)*) => {
        any_internal!(@array [$($items,)* any_internal!([$($array)*])] $($rest)*)
    };

    // Next item is a map.
    (@array [$($items:expr,)*] {$($map:tt)*} $($rest:tt)*) => {
        any_internal!(@array [$($items,)* any_internal!({$($map)*})] $($rest)*)
    };

    // Next item is an expression followed by comma.
    (@array [$($items:expr,)*] $next:expr, $($rest:tt)*) => {
        any_internal!(@array [$($items,)* any_internal!($next),] $($rest)*)
    };

    // Last item is an expression with no trailing comma.
    (@array [$($items:expr,)*] $last:expr) => {
        any_internal!(@array [$($items,)* any_internal!($last)])
    };

    // Comma after the most recent item.
    (@array [$($items:expr),*] , $($rest:tt)*) => {
        any_internal!(@array [$($items,)*] $($rest)*)
    };

    // Unexpected token after most recent item.
    (@array [$($items:expr),*] $unexpected:tt $($rest:tt)*) => {
        any_unexpected!($unexpected)
    };

    (@object $object:ident () () ()) => {};

    // Insert the current entry followed by trailing comma.
    (@object $object:ident [$($key:tt)+] ($value:expr) , $($rest:tt)*) => {
        let _ = $object.insert(($($key)+).into(), $value);
        any_internal!(@object $object () ($($rest)*) ($($rest)*));
    };

    // Current entry followed by unexpected token.
    (@object $object:ident [$($key:tt)+] ($value:expr) $unexpected:tt $($rest:tt)*) => {
        any_unexpected!($unexpected);
    };

    // Insert the last entry without trailing comma.
    (@object $object:ident [$($key:tt)+] ($value:expr)) => {
        let _ = $object.insert(($($key)+).into(), $value);
    };

    // Next value is `null`.
    (@object $object:ident ($($key:tt)+) (: null $($rest:tt)*) $copy:tt) => {
        any_internal!(@object $object [$($key)+] (any_internal!(null)) $($rest)*);
    };

    // Next value is `true`.
    (@object $object:ident ($($key:tt)+) (: true $($rest:tt)*) $copy:tt) => {
        any_internal!(@object $object [$($key)+] (any_internal!(true)) $($rest)*);
    };

    // Next value is `false`.
    (@object $object:ident ($($key:tt)+) (: false $($rest:tt)*) $copy:tt) => {
        any_internal!(@object $object [$($key)+] (any_internal!(false)) $($rest)*);
    };

    // Next value is an array.
    (@object $object:ident ($($key:tt)+) (: [$($array:tt)*] $($rest:tt)*) $copy:tt) => {
        any_internal!(@object $object [$($key)+] (any_internal!([$($array)*])) $($rest)*);
    };

    // Next value is a map.
    (@object $object:ident ($($key:tt)+) (: {$($map:tt)*} $($rest:tt)*) $copy:tt) => {
        any_internal!(@object $object [$($key)+] (any_internal!({$($map)*})) $($rest)*);
    };

    // Next value is an expression followed by comma.
    (@object $object:ident ($($key:tt)+) (: $value:expr , $($rest:tt)*) $copy:tt) => {
        any_internal!(@object $object [$($key)+] (any_internal!($value)) , $($rest)*);
    };

    // Last value is an expression with no trailing comma.
    (@object $object:ident ($($key:tt)+) (: $value:expr) $copy:tt) => {
        any_internal!(@object $object [$($key)+] (any_internal!($value)));
    };

    // Missing value for last entry. Trigger a reasonable error message.
    (@object $object:ident ($($key:tt)+) (:) $copy:tt) => {
        // "unexpected end of macro invocation"
        any_internal!();
    };

    // Missing colon and value for last entry. Trigger a reasonable error
    // message.
    (@object $object:ident ($($key:tt)+) () $copy:tt) => {
        // "unexpected end of macro invocation"
        any_internal!();
    };

    // Misplaced colon. Trigger a reasonable error message.
    (@object $object:ident () (: $($rest:tt)*) ($colon:tt $($copy:tt)*)) => {
        // Takes no arguments so "no rules expected the token `:`".
        any_unexpected!($colon);
    };

    // Found a comma inside a key. Trigger a reasonable error message.
    (@object $object:ident ($($key:tt)*) (, $($rest:tt)*) ($comma:tt $($copy:tt)*)) => {
        // Takes no arguments so "no rules expected the token `,`".
        any_unexpected!($comma);
    };

    // Key is fully parenthesized. This avoids clippy double_parens false
    // positives because the parenthesization may be necessary here.
    (@object $object:ident () (($key:expr) : $($rest:tt)*) $copy:tt) => {
        any_internal!(@object $object ($key) (: $($rest)*) (: $($rest)*));
    };

    // Refuse to absorb colon token into key expression.
    (@object $object:ident ($($key:tt)*) (: $($unexpected:tt)+) $copy:tt) => {
        json_expect_expr_comma!($($unexpected)+);
    };

    // Munch a token into the current key.
    (@object $object:ident ($($key:tt)*) ($tt:tt $($rest:tt)*) $copy:tt) => {
        any_internal!(@object $object ($($key)* $tt) ($($rest)*) ($($rest)*));
    };

    //////////////////////////////////////////////////////////////////////////
    // The main implementation.
    //
    // Must be invoked as: any_internal!($($json)+)
    //////////////////////////////////////////////////////////////////////////

    (null) => {
        $crate::any::Any::Null
    };

    (true) => {
        $crate::any::Any::Bool(true)
    };

    (false) => {
        $crate::any::Any::Bool(false)
    };

    ([]) => {
        $crate::any::Any::Array(any_internal_array![])
    };

    ([ $($tt:tt)+ ]) => {
        $crate::any::Any::Array(any_internal!(@array [] $($tt)+))
    };

    ({}) => {
        $crate::any::Any::Map(std::sync::Arc::new($crate::any::AnyMap::new()))
    };

    ({ $($tt:tt)+ }) => {
        $crate::any::Any::Map({
            let mut object = $crate::any::AnyMap::new();
            any_internal!(@object object () ($($tt)+) ($($tt)+));
            std::sync::Arc::new(object)
        })
    };

    // Any Serialize type: numbers, strings, struct literals, variables etc.
    // Must be below every other rule.
    ($other:expr) => {
        ($other).into()
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! any_internal_array {
    ($($content:tt)*) => {
        std::sync::Arc::from([$($content)*])
    };
}

#[macro_export]
#[doc(hidden)]
macro_rules! any_unexpected {
    () => {};
}

#[macro_export]
#[doc(hidden)]
macro_rules! any_expect_expr_comma {
    ($e:expr , $($tt:tt)*) => {};
}

#[cfg(test)]
mod test {
    use crate::any::Any;
    use crate::encoding::read::Cursor;
    use crate::Number;

    #[test]
    fn decode_map_rejects_length_amplification() {
        // Regression: an `Any::Map` whose length prefix declares a huge entry count (a few bytes)
        // must not trigger an eager multi-hundred-MB allocation. `try_reserve` turns it into a
        // recoverable decode error instead of an abort under a hard memory limit.
        // Bytes: tag 118 (Map) + var-int len ~250M (0xFF 0xFF 0xFF 0x7A) + no data.
        let adversarial = [118u8, 0xFF, 0xFF, 0xFF, 0x7A];
        let mut cursor = Cursor::new(&adversarial);
        assert!(
            Any::decode(&mut cursor).is_err(),
            "an oversized Any::Map length must be rejected, not eagerly allocated"
        );
    }

    #[test]
    fn decode_array_rejects_length_amplification() {
        // As above, for `Any::Array`. Bytes: tag 117 (Array) + var-int len ~250M + no data.
        let adversarial = [117u8, 0xFF, 0xFF, 0xFF, 0x7A];
        let mut cursor = Cursor::new(&adversarial);
        assert!(
            Any::decode(&mut cursor).is_err(),
            "an oversized Any::Array length must be rejected, not eagerly allocated"
        );
    }

    fn hex(n: Number) -> String {
        let mut buf = Vec::new();
        Any::Number(n).encode(&mut buf);
        buf.iter().map(|b| format!("{:02x}", b)).collect()
    }

    #[test]
    fn number_encoding_yjs_compat() {
        assert_eq!(hex(Number::Int(0)), "7d00");
        assert_eq!(hex(Number::Int(42)), "7d2a");
        assert_eq!(hex(Number::Int(-42)), "7d6a");
        assert_eq!(hex(Number::Int(2147483647)), "7dbfffffff0f");
        // above 2^31 lib0 stops using the var int tag, even for whole numbers
        assert_eq!(hex(Number::Int(2147483648)), "7c4f000000");
        assert_eq!(hex(Number::Int(-2147483648)), "7ccf000000");
        assert_eq!(hex(Number::Int(4294967295)), "7b41efffffffe00000");
        assert_eq!(hex(Number::Int(9007199254740991)), "7b433fffffffffffff");
        assert_eq!(hex(Number::Float(1.5)), "7c3fc00000");
        assert_eq!(hex(Number::Float(1.1)), "7b3ff199999999999a");
        assert_eq!(hex(Number::Float(2147483648.0)), "7c4f000000");
        assert_eq!(hex(Number::Float(5e9)), "7c4f9502f9");
        assert_eq!(hex(Number::Float(1e30)), "7b46293e5939a08cea");
        assert_eq!(hex(Number::Float(f64::NAN)), "7b7ff8000000000000");
        assert_eq!(hex(Number::Float(f64::INFINITY)), "7c7f800000");
        assert_eq!(hex(Number::Float(f64::NEG_INFINITY)), "7cff800000");
    }

    fn decode_hex(hex: &str) -> Any {
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        Any::decode(&mut Cursor::new(&bytes)).unwrap()
    }

    #[test]
    fn big_integers_are_js_numbers_unless_they_are_bigints() {
        // Beyond 2^53 an Int is a JS number if one holds it exactly, and a BigInt if not.
        // i64::MAX used to come out as the float32 2^63.
        assert_eq!(hex(Number::Int(1 << 62)), "7c5e800000");
        assert_eq!(hex(Number::Int((1 << 53) + 2)), "7b4340000000000001");
        assert_eq!(hex(Number::Int((1 << 62) + 1)), "7a4000000000000001");
        assert_eq!(hex(Number::Int(i64::MAX)), "7a7fffffffffffffff");
        // A BigInt is written as lib0's bigint, whatever its size, and reads back as one.
        for n in [0, 5, (1 << 62) + 1, i64::MAX, i64::MIN] {
            let mut buf = Vec::new();
            Any::Number(Number::BigInt(n)).encode(&mut buf);
            assert_eq!(buf[0], 122);
            match Any::decode(&mut Cursor::new(&buf)).unwrap() {
                Any::Number(Number::BigInt(m)) => assert_eq!(m, n),
                other => panic!("{:?}", other),
            }
        }
        assert_eq!(hex(Number::BigInt(5)), "7a0000000000000005");
        // Int and BigInt of the same value are equal, as in JS' `5n == 5`.
        assert_eq!(Number::BigInt(5), Number::Int(5));
        assert_ne!(Number::BigInt((1 << 62) + 1), Number::Int(1 << 62));
    }

    #[test]
    fn maps_keep_insertion_order_so_decoding_and_encoding_is_stable() {
        // Yjs' encoding of `{z: 1, a: 2, m: 3}`: lib0 writes keys in insertion order.
        let yjs = "7603017a7d0101617d02016d7d03";
        let map = decode_hex(yjs);
        let keys: Vec<_> = match &map {
            Any::Map(m) => m.keys().cloned().collect(),
            other => panic!("{:?}", other),
        };
        assert_eq!(keys, ["z", "a", "m"]);
        let mut buf = Vec::new();
        map.encode(&mut buf);
        let hex: String = buf.iter().map(|b| format!("{:02x}", b)).collect();
        assert_eq!(hex, yjs);
        let mut json = String::new();
        map.to_json(&mut json);
        assert_eq!(json, r#"{"z":1,"a":2,"m":3}"#);
        // Equality ignores order, as for JS objects compared by content.
        let reordered: Any = vec![("a", 2), ("m", 3), ("z", 1)].into_iter().collect();
        assert_eq!(map, reordered);
    }

    #[test]
    fn floats_out_of_i64_range_are_not_integers() {
        assert_eq!(Number::Float(9_223_372_036_854_775_808.0).as_i64(), None);
        assert_eq!(
            Number::Float(-9_223_372_036_854_775_808.0).as_i64(),
            Some(i64::MIN)
        );
        assert_eq!(Number::Float(f64::NAN).as_i64(), None);
        assert_eq!(Number::Float(3.0).as_i64(), Some(3));
    }

    #[test]
    fn negative_zero_keeps_its_sign() {
        // lib0 writes -0 as a var int with the sign bit set.
        match decode_hex("7d40") {
            Any::Number(Number::Float(z)) => assert!(z == 0.0 && z.is_sign_negative()),
            other => panic!("{:?}", other),
        }
        assert_eq!(decode_hex("7d00"), Any::Number(Number::Int(0)));
        match Number::try_i64(-0.0) {
            Number::Float(z) => assert!(z.is_sign_negative()),
            other => panic!("{:?}", other),
        }
        let mut buf = Vec::new();
        Any::Number(Number::Float(-0.0)).encode(&mut buf);
        match Any::decode(&mut Cursor::new(&buf)).unwrap() {
            Any::Number(Number::Float(z)) => assert!(z.is_sign_negative()),
            other => panic!("{:?}", other),
        }
    }
}
