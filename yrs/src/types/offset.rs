//! Text positions in an explicit unit.
//!
//! A document counts text indexes in one [OffsetKind] (UTF-16 code units by default). The
//! newtypes here say which unit a number is in, so code that has a byte offset from Rust, a
//! character count from a user, or a UTF-16 index from JavaScript can use it as it is, whatever
//! the document counts in. See [crate::Text::offset] and the `_at` methods of [crate::Text].

use crate::block::ItemContent;
use crate::branch::Branch;
use crate::doc::OffsetKind;
use crate::error::OutOfRange;
use thiserror::Error;

/// A position in UTF-8 bytes, like an index into a Rust `str`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Utf8Offset(pub u32);

/// A position in UTF-16 code units, like an index into a JavaScript string or a Yjs `Y.Text`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Utf16Offset(pub u32);

/// A position in Unicode scalar values, like a count of Rust `char`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CharOffset(pub u32);

/// A text position in one of the units above. Embedded values and nested types count as one in
/// every unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TextOffset {
    /// See [Utf8Offset].
    Utf8(u32),
    /// See [Utf16Offset].
    Utf16(u32),
    /// See [CharOffset].
    Char(u32),
}

impl From<Utf8Offset> for TextOffset {
    fn from(o: Utf8Offset) -> Self {
        TextOffset::Utf8(o.0)
    }
}

impl From<Utf16Offset> for TextOffset {
    fn from(o: Utf16Offset) -> Self {
        TextOffset::Utf16(o.0)
    }
}

impl From<CharOffset> for TextOffset {
    fn from(o: CharOffset) -> Self {
        TextOffset::Char(o.0)
    }
}

impl TextOffset {
    fn value(self) -> u32 {
        match self {
            TextOffset::Utf8(n) | TextOffset::Utf16(n) | TextOffset::Char(n) => n,
        }
    }

    fn measure(self, s: &str) -> u32 {
        (match self {
            TextOffset::Utf8(_) => s.len(),
            TextOffset::Utf16(_) => s.encode_utf16().count(),
            TextOffset::Char(_) => s.chars().count(),
        }) as u32
    }

    fn measure_char(self, c: char) -> u32 {
        (match self {
            TextOffset::Utf8(_) => c.len_utf8(),
            TextOffset::Utf16(_) => c.len_utf16(),
            TextOffset::Char(_) => 1,
        }) as u32
    }
}

/// Why a [TextOffset] couldn't be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum OffsetError {
    /// The offset is past the end of the text.
    #[error(transparent)]
    OutOfRange(#[from] OutOfRange),
    /// The offset falls inside a character: within a UTF-8 sequence or a UTF-16 surrogate pair.
    #[error("offset {0} is inside a character")]
    InsideCharacter(u32),
}

fn measure_kind(kind: OffsetKind, s: &str) -> u32 {
    (match kind {
        OffsetKind::Bytes => s.len(),
        OffsetKind::Utf16 => s.encode_utf16().count(),
    }) as u32
}

/// Converts `at` into the units of `kind`, by walking the visible content of `branch`.
pub(crate) fn to_kind(
    branch: &Branch,
    kind: OffsetKind,
    at: TextOffset,
) -> Result<u32, OffsetError> {
    let mut remaining = at.value();
    let mut total_in_unit = 0u32;
    let mut out = 0u32;
    let mut current = branch.start;
    while let Some(item) = current {
        current = item.right;
        if item.is_deleted() || !item.is_countable() {
            continue;
        }
        let (in_unit, in_kind) = match &item.content {
            ItemContent::String(s) => {
                let s = s.as_str();
                let in_unit = at.measure(s);
                if remaining < in_unit {
                    // The offset is inside this chunk: count its prefix character by character.
                    let mut prefix = 0u32;
                    let mut prefix_kind = 0u32;
                    for c in s.chars() {
                        if prefix == remaining {
                            break;
                        }
                        prefix += at.measure_char(c);
                        prefix_kind += match kind {
                            OffsetKind::Bytes => c.len_utf8() as u32,
                            OffsetKind::Utf16 => c.len_utf16() as u32,
                        };
                        if prefix > remaining {
                            return Err(OffsetError::InsideCharacter(at.value()));
                        }
                    }
                    return Ok(out + prefix_kind);
                }
                (in_unit, measure_kind(kind, s))
            }
            _ => (1, item.content_len(kind)),
        };
        if remaining < in_unit {
            // A one-unit embed with nothing left to skip: the offset is right before it.
            return Ok(out);
        }
        remaining -= in_unit;
        total_in_unit += in_unit;
        out += in_kind;
        if remaining == 0 {
            return Ok(out);
        }
    }
    if remaining == 0 {
        Ok(out)
    } else {
        Err(OutOfRange {
            index: at.value(),
            len: 0,
            available: total_in_unit,
        }
        .into())
    }
}
