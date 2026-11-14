//! JSON for [Any]: the text Yjs stores embeds, format attributes and `Any::from_json` values as.
//!
//! The writer produces the same text serde_json did for an [Any], so encoded updates are
//! byte-for-byte unchanged. The reader follows the JSON grammar (RFC 8259) and reads numbers as
//! serde_json did: an integer that fits an `i64` is an integer, anything else a float. Floats
//! are rounded correctly, as `JSON.parse` does.

use crate::any::{Any, AnyMap};
use std::fmt::Write as _;
use std::sync::Arc;

/// Nesting deeper than this is rejected, as serde_json's recursion limit did.
const MAX_DEPTH: usize = 127;

/// Text that isn't valid JSON, or JSON that has no [Any] counterpart.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason} at byte {offset}")]
pub struct JsonError {
    reason: &'static str,
    offset: usize,
}

impl JsonError {
    /// What was wrong.
    pub fn reason(&self) -> &'static str {
        self.reason
    }

    /// The byte offset in the input where it was found.
    pub fn offset(&self) -> usize {
        self.offset
    }
}

/// How integers beyond `i64` are read.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Integers {
    /// Integers from 2^63 to 2^64 − 1 are an error, as `Any::from_json` has always done.
    Strict,
    /// Every integer beyond `i64` becomes a float, as `JSON.parse` does.
    JsLike,
}

/// Parses `src` into an [Any].
pub(crate) fn parse(src: &str, integers: Integers) -> Result<Any, JsonError> {
    let mut p = Parser {
        src: src.as_bytes(),
        pos: 0,
        integers,
    };
    p.skip_ws();
    let value = p.value(0)?;
    p.skip_ws();
    if p.pos != p.src.len() {
        return Err(p.err("trailing characters"));
    }
    Ok(value)
}

struct Parser<'a> {
    src: &'a [u8],
    pos: usize,
    integers: Integers,
}

impl<'a> Parser<'a> {
    fn err(&self, reason: &'static str) -> JsonError {
        JsonError {
            reason,
            offset: self.pos,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.src.get(self.pos).copied()
    }

    fn skip_ws(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
            self.pos += 1;
        }
    }

    fn literal(&mut self, word: &[u8], value: Any) -> Result<Any, JsonError> {
        if self.src[self.pos..].starts_with(word) {
            self.pos += word.len();
            Ok(value)
        } else {
            Err(self.err("expected a value"))
        }
    }

    fn value(&mut self, depth: usize) -> Result<Any, JsonError> {
        match self.peek() {
            None => Err(self.err("unexpected end of input")),
            Some(b'n') => self.literal(b"null", Any::Null),
            Some(b't') => self.literal(b"true", Any::Bool(true)),
            Some(b'f') => self.literal(b"false", Any::Bool(false)),
            Some(b'"') => Ok(Any::String(Arc::from(self.string()?))),
            Some(b'[') => self.array(depth + 1),
            Some(b'{') => self.object(depth + 1),
            Some(b'-' | b'0'..=b'9') => self.number(),
            Some(_) => Err(self.err("expected a value")),
        }
    }

    fn array(&mut self, depth: usize) -> Result<Any, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nested too deeply"));
        }
        self.pos += 1; // [
        let mut items = Vec::new();
        self.skip_ws();
        if self.peek() == Some(b']') {
            self.pos += 1;
            return Ok(Any::from(items));
        }
        loop {
            self.skip_ws();
            items.push(self.value(depth)?);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b']') => {
                    self.pos += 1;
                    return Ok(Any::from(items));
                }
                None => return Err(self.err("unexpected end of input")),
                Some(_) => return Err(self.err("expected `,` or `]`")),
            }
        }
    }

    fn object(&mut self, depth: usize) -> Result<Any, JsonError> {
        if depth > MAX_DEPTH {
            return Err(self.err("nested too deeply"));
        }
        self.pos += 1; // {
        let mut map = AnyMap::new();
        self.skip_ws();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(Any::Map(Arc::new(map)));
        }
        loop {
            self.skip_ws();
            if self.peek() != Some(b'"') {
                return Err(self.err("expected a string key"));
            }
            let key = self.string()?;
            self.skip_ws();
            if self.peek() != Some(b':') {
                return Err(self.err("expected `:`"));
            }
            self.pos += 1;
            self.skip_ws();
            let value = self.value(depth)?;
            // A repeated key keeps its first position and takes the last value.
            map.insert(key, value);
            self.skip_ws();
            match self.peek() {
                Some(b',') => self.pos += 1,
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(Any::Map(Arc::new(map)));
                }
                None => return Err(self.err("unexpected end of input")),
                Some(_) => return Err(self.err("expected `,` or `}`")),
            }
        }
    }

    fn hex4(&mut self) -> Result<u16, JsonError> {
        let digits = self
            .src
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| self.err("unexpected end of input"))?;
        let mut n = 0u16;
        for &d in digits {
            let v = match d {
                b'0'..=b'9' => d - b'0',
                b'a'..=b'f' => d - b'a' + 10,
                b'A'..=b'F' => d - b'A' + 10,
                _ => return Err(self.err("invalid `\\u` escape")),
            };
            n = n * 16 + u16::from(v);
        }
        self.pos += 4;
        Ok(n)
    }

    fn string(&mut self) -> Result<String, JsonError> {
        self.pos += 1; // opening quote
        let mut out = String::new();
        loop {
            // Copy the run of plain bytes. The input is a `&str`, so a run that stops at an
            // ASCII byte ends on a character boundary.
            let start = self.pos;
            while let Some(b) = self.peek() {
                if b == b'"' || b == b'\\' || b < 0x20 {
                    break;
                }
                self.pos += 1;
            }
            out.push_str(std::str::from_utf8(&self.src[start..self.pos]).expect("utf-8 input"));
            match self.peek() {
                None => return Err(self.err("unterminated string")),
                Some(b'"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some(b'\\') => {
                    self.pos += 1;
                    let esc = self.peek().ok_or_else(|| self.err("unterminated string"))?;
                    self.pos += 1;
                    match esc {
                        b'"' => out.push('"'),
                        b'\\' => out.push('\\'),
                        b'/' => out.push('/'),
                        b'b' => out.push('\u{8}'),
                        b'f' => out.push('\u{c}'),
                        b'n' => out.push('\n'),
                        b'r' => out.push('\r'),
                        b't' => out.push('\t'),
                        b'u' => out.push(self.unicode_escape()?),
                        _ => {
                            self.pos -= 1;
                            return Err(self.err("invalid escape"));
                        }
                    }
                }
                Some(_) => return Err(self.err("control character in string")),
            }
        }
    }

    /// The character after `\u`: one escape, or a surrogate pair written as two.
    fn unicode_escape(&mut self) -> Result<char, JsonError> {
        let first = self.hex4()?;
        match first {
            0xD800..=0xDBFF => {
                if self.src.get(self.pos..self.pos + 2) != Some(b"\\u") {
                    return Err(self.err("lone leading surrogate"));
                }
                self.pos += 2;
                let second = self.hex4()?;
                if !(0xDC00..=0xDFFF).contains(&second) {
                    return Err(self.err("lone leading surrogate"));
                }
                let c =
                    0x10000 + ((u32::from(first) - 0xD800) << 10) + (u32::from(second) - 0xDC00);
                Ok(char::from_u32(c).expect("a surrogate pair is a valid scalar"))
            }
            0xDC00..=0xDFFF => Err(self.err("lone trailing surrogate")),
            n => Ok(char::from_u32(u32::from(n)).expect("a non-surrogate is a valid scalar")),
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.pos;
        while let Some(b'0'..=b'9') = self.peek() {
            self.pos += 1;
        }
        self.pos - start
    }

    fn number(&mut self) -> Result<Any, JsonError> {
        let start = self.pos;
        let negative = self.peek() == Some(b'-');
        if negative {
            self.pos += 1;
        }
        match self.peek() {
            Some(b'0') => {
                self.pos += 1;
                if let Some(b'0'..=b'9') = self.peek() {
                    return Err(self.err("leading zero in number"));
                }
            }
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.err("invalid number")),
        }
        let mut float = false;
        if self.peek() == Some(b'.') {
            self.pos += 1;
            if self.digits() == 0 {
                return Err(self.err("invalid number"));
            }
            float = true;
        }
        if let Some(b'e' | b'E') = self.peek() {
            self.pos += 1;
            if let Some(b'+' | b'-') = self.peek() {
                self.pos += 1;
            }
            if self.digits() == 0 {
                return Err(self.err("invalid number"));
            }
            float = true;
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).expect("ascii number");
        if !float {
            if let Ok(n) = text.parse::<i64>() {
                // `-0` is a float, as in JavaScript (and serde_json).
                return Ok(if n == 0 && negative {
                    Any::from(-0.0f64)
                } else {
                    Any::from(n)
                });
            }
            if self.integers == Integers::Strict && !negative && text.parse::<u64>().is_ok() {
                self.pos = start;
                return Err(self.err("integer out of range"));
            }
        }
        // Beyond i64, or a float: rounded correctly, so this can't fail on valid number text.
        Ok(Any::from(text.parse::<f64>().expect("valid float text")))
    }
}

/// Appends `value` to `buf` as JSON, exactly as serde_json wrote an [Any].
pub(crate) fn write(value: &Any, buf: &mut String) {
    match value {
        Any::Null | Any::Undefined => buf.push_str("null"),
        Any::Bool(true) => buf.push_str("true"),
        Any::Bool(false) => buf.push_str("false"),
        Any::Number(n) => match n.as_i64() {
            Some(i) => {
                let _ = write!(buf, "{i}");
            }
            None => write_f64(n.as_f64().expect("a float has an f64 value"), buf),
        },
        Any::String(s) => write_str(s, buf),
        Any::Buffer(bytes) => {
            buf.push('[');
            for (i, b) in bytes.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                let _ = write!(buf, "{b}");
            }
            buf.push(']');
        }
        Any::Array(items) => {
            buf.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                write(item, buf);
            }
            buf.push(']');
        }
        Any::Map(entries) => {
            buf.push('{');
            for (i, (key, item)) in entries.iter().enumerate() {
                if i > 0 {
                    buf.push(',');
                }
                write_str(key, buf);
                buf.push(':');
                write(item, buf);
            }
            buf.push('}');
        }
    }
}

fn write_f64(value: f64, buf: &mut String) {
    if value.is_finite() {
        buf.push_str(zmij::Buffer::new().format_finite(value));
    } else {
        buf.push_str("null");
    }
}

fn write_str(s: &str, buf: &mut String) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    buf.push('"');
    let mut start = 0;
    for (i, b) in s.bytes().enumerate() {
        let escape = match b {
            b'"' => "\\\"",
            b'\\' => "\\\\",
            0x08 => "\\b",
            0x09 => "\\t",
            0x0A => "\\n",
            0x0C => "\\f",
            0x0D => "\\r",
            0x00..=0x1F => "",
            _ => continue,
        };
        buf.push_str(&s[start..i]);
        if escape.is_empty() {
            buf.push_str("\\u00");
            buf.push(char::from(HEX[usize::from(b >> 4)]));
            buf.push(char::from(HEX[usize::from(b & 0xF)]));
        } else {
            buf.push_str(escape);
        }
        start = i + 1;
    }
    buf.push_str(&s[start..]);
    buf.push('"');
}

#[cfg(test)]
mod test {
    use super::{parse, write, Integers};
    use crate::any::Any;
    use proptest::prelude::*;

    fn to_json(any: &Any) -> String {
        let mut s = String::new();
        write(any, &mut s);
        s
    }

    fn any_strategy() -> impl Strategy<Value = Any> {
        let leaf = prop_oneof![
            Just(Any::Null),
            Just(Any::Undefined),
            any::<bool>().prop_map(Any::Bool),
            any::<i64>().prop_map(Any::from),
            any::<f64>().prop_map(Any::from),
            prop_oneof![Just(0.1), Just(1e21), Just(-0.0), Just(5e-324), Just(1.0)]
                .prop_map(Any::from),
            "\\PC*".prop_map(|s: String| Any::from(s)),
            "[\\x00-\\x1f\"\\\\/a\u{7f}\u{e9}\u{1f600}]*".prop_map(|s: String| Any::from(s)),
            prop::collection::vec(any::<u8>(), 0..6).prop_map(|b| Any::from(b)),
        ];
        leaf.prop_recursive(4, 32, 6, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..6).prop_map(Any::from),
                prop::collection::vec(("[a-z\"\\\\]{0,4}", inner), 0..6).prop_map(|kv| {
                    Any::from(
                        kv.into_iter()
                            .collect::<std::collections::HashMap<String, Any>>(),
                    )
                }),
            ]
        })
    }

    proptest! {
        /// The writer must not change encoded updates: compare it with serde_json.
        #[test]
        fn writes_what_serde_json_wrote(any in any_strategy()) {
            prop_assert_eq!(to_json(&any), serde_json::to_string(&any).unwrap());
        }

        /// Reading back what was written gives the same value (NaN and infinities aside, which
        /// are written as null).
        #[test]
        fn reads_back_what_it_writes(any in any_strategy()) {
            let text = to_json(&any);
            let back = parse(&text, Integers::Strict).unwrap();
            prop_assert_eq!(to_json(&back), text);
        }

        /// On JSON text, the reader agrees with serde_json. Floats may differ in the last few
        /// places: serde_json's default parser doesn't always round correctly. This one does,
        /// which `reads_back_what_it_writes` checks exactly.
        #[test]
        fn reads_what_serde_json_read(any in any_strategy()) {
            let text = serde_json::to_string(&any).unwrap();
            let ours = parse(&text, Integers::Strict).unwrap();
            let theirs: Any = serde_json::from_str(&text).unwrap();
            prop_assert!(same(&ours, &theirs), "{:?} vs {:?}", ours, theirs);
        }
    }

    /// Equal, with floats allowed to differ by a few units in the last place.
    fn same(a: &Any, b: &Any) -> bool {
        match (a, b) {
            (Any::Number(x), Any::Number(y)) => match (x.as_i64(), y.as_i64()) {
                (Some(i), Some(j)) => i == j,
                _ => {
                    let (x, y) = (x.as_f64().unwrap(), y.as_f64().unwrap());
                    x == y || x.to_bits().abs_diff(y.to_bits()) <= 4
                }
            },
            (Any::Array(x), Any::Array(y)) => {
                x.len() == y.len() && x.iter().zip(y.iter()).all(|(a, b)| same(a, b))
            }
            (Any::Map(x), Any::Map(y)) => {
                x.len() == y.len()
                    && x.iter()
                        .zip(y.iter())
                        .all(|((k, a), (l, b))| k == l && same(a, b))
            }
            _ => a == b,
        }
    }

    #[test]
    fn numbers_read_as_serde_json_and_json_parse_do() {
        let read = |s| parse(s, Integers::JsLike).unwrap();
        assert_eq!(read("42"), Any::from(42i64));
        assert_eq!(read("-9223372036854775808"), Any::from(i64::MIN));
        assert_eq!(
            read("9223372036854775808"),
            Any::from(9223372036854775808f64)
        );
        assert_eq!(read("1e400"), Any::from(f64::INFINITY));
        assert!(
            matches!(read("-0"), Any::Number(n) if n.as_f64() == Some(0.0) && n.as_i64().is_some())
        );
        assert_eq!(read("1.5e3"), Any::from(1500.0));
        assert!(parse("9223372036854775808", Integers::Strict).is_err());
        assert!(parse("18446744073709551616", Integers::Strict).is_ok());
    }

    #[test]
    fn invalid_json_is_an_error() {
        for bad in [
            "",
            "nul",
            "01",
            "1.",
            "1e",
            "-",
            "[1,]",
            "{\"a\"}",
            "{\"a\":1,}",
            "\"\\x\"",
            "\"\\ud800\"",
            "\"\\udc00\"",
            "\"a\nb\"",
            "[1] 2",
            "{1:2}",
            "\"",
        ] {
            assert!(
                parse(bad, Integers::JsLike).is_err(),
                "{bad:?} should not parse"
            );
        }
        let deep = "[".repeat(127) + &"]".repeat(127);
        assert!(parse(&deep, Integers::JsLike).is_ok());
        let deeper = "[".repeat(128) + &"]".repeat(128);
        assert!(parse(&deeper, Integers::JsLike).is_err());
        assert!(serde_json::from_str::<Any>(&deeper).is_err());
        assert!(serde_json::from_str::<Any>(&deep).is_ok());
    }

    #[test]
    fn strings_unescape_and_keep_key_order() {
        let any = parse(
            r#"{"b":"\u00e9\ud83d\ude00\n\/","a":1,"b":2}"#,
            Integers::Strict,
        )
        .unwrap();
        let Any::Map(map) = &any else { panic!() };
        assert_eq!(map.keys().collect::<Vec<_>>(), ["b", "a"]);
        assert_eq!(map["b"], Any::from(2i64));
        assert_eq!(
            parse(r#""\u00e9\ud83d\ude00\n\/""#, Integers::Strict).unwrap(),
            Any::from("é😀\n/")
        );
    }
}
