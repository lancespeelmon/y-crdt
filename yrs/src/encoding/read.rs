use crate::encoding::varint::{Signed, SignedVarInt, VarInt};
use std::any::type_name;
use std::collections::TryReserveError;
use std::fmt::{Display, Formatter};
use thiserror::Error;

/// An error from decoding lib0-encoded data.
#[derive(Error, Debug)]
#[non_exhaustive]
pub enum DecodeError {
    /// A variable-length integer is malformed.
    #[error("failed to decode variable length integer")]
    InvalidVarInt,

    /// The input ended before the given number of bytes could be read.
    #[error("while trying to read more data (expected: {0} bytes), an unexpected end of buffer was reached")]
    EndOfBuffer(usize),

    /// A value that's invalid at its position was found, e.g. an unknown tag or a length that is out of bounds.
    #[error("while reading, an unexpected value was found")]
    UnexpectedValue,

    /// Memory for the decoded data couldn't be allocated.
    #[error("failed to allocate memory: {0}")]
    NotEnoughMemory(#[from] TryReserveError),

    /// A JSON string couldn't be parsed.
    #[error("JSON parsing error: {0}")]
    InvalidJSON(#[from] crate::json::JsonError),

    /// The data couldn't be deserialized into the named target type.
    #[error("couldn't deserialize to target type of {0}")]
    TypeMismatch(&'static str),

    /// The data is not a valid update.
    #[error("malformed update: {0}")]
    Malformed(MalformedKind),

    /// An error with a custom message.
    #[error("{0}")]
    Custom(String),
}

/// The name this module's error had before [DecodeError].
pub type Error = DecodeError;

impl DecodeError {
    /// Creates a [DecodeError::TypeMismatch] that names the type `T`.
    pub fn type_mismatch<T>() -> Self {
        DecodeError::TypeMismatch(type_name::<T>())
    }
}

/// What was wrong with a [DecodeError::Malformed] update.
#[derive(Debug)]
#[non_exhaustive]
pub enum MalformedKind {
    /// A string is not valid UTF-8.
    Utf8,
    /// A block with a length of 0 was found.
    EmptyBlock,
}

impl Display for MalformedKind {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            MalformedKind::Utf8 => write!(f, "invalid UTF-8 sequence"),
            MalformedKind::EmptyBlock => write!(f, "block cannot have 0 length"),
        }
    }
}

/// A [Read] implementation over a slice of bytes.
#[derive(Default)]
pub struct Cursor<'a> {
    /// The bytes that are being read.
    pub buf: &'a [u8],
    /// The index in `buf` of the next byte to read.
    pub next: usize,
}

impl<'a> Cursor<'a> {
    /// Creates a cursor that reads `buf` from its start.
    pub fn new(buf: &'a [u8]) -> Cursor<'a> {
        Cursor { buf, next: 0 }
    }

    /// Checks if there are bytes left to read.
    pub fn has_content(&self) -> bool {
        self.next != self.buf.len()
    }
}

impl<'a, R> From<&'a R> for Cursor<'a>
where
    R: AsRef<[u8]>,
{
    fn from(buf: &'a R) -> Self {
        Self::new(buf.as_ref())
    }
}

impl<'a> Read for Cursor<'a> {
    /// Take a slice of the next `len` bytes and advance the position by `len`.
    fn read_exact(&mut self, len: usize) -> Result<&[u8], Error> {
        if self.next + len > self.buf.len() {
            Err(Error::EndOfBuffer(len))
        } else {
            let slice = &self.buf[self.next..(self.next + len)];
            self.next += len as usize;
            Ok(slice)
        }
    }

    /// Read a single byte.
    fn read_u8(&mut self) -> Result<u8, Error> {
        if let Some(&b) = self.buf.get(self.next) {
            self.next += 1;
            Ok(b)
        } else {
            Err(Error::EndOfBuffer(1))
        }
    }

    #[inline]
    fn remaining(&self) -> usize {
        self.buf.len().saturating_sub(self.next)
    }
}

/// A source of bytes that primitive values are read from, in the lib0 encoding.
pub trait Read: Sized {
    /// Takes the next `len` bytes and advances by that many. Fails with [DecodeError::EndOfBuffer] if fewer remain.
    ///
    /// # Errors
    ///
    /// [Error::EndOfBuffer] if fewer than `len` bytes are left.
    fn read_exact(&mut self, len: usize) -> Result<&[u8], Error>;

    /// Returns the number of bytes this reader can still produce.
    ///
    /// Element counts are read straight from the input, so one can claim far more than the
    /// input holds. Decoders use this value to limit how many elements they reserve space for
    /// up front. It is not a validity check: `DecoderV2` keeps some fields in separate streams,
    /// so an element may cost no bytes of this one, and its collection then grows as elements
    /// are decoded.
    ///
    /// A reader that cannot tell can keep the default of `0`, which only turns off reserving.
    fn remaining(&self) -> usize {
        0
    }

    /// Read a single byte.
    ///
    /// # Errors
    ///
    /// [Error::EndOfBuffer] if no byte is left.
    fn read_u8(&mut self) -> Result<u8, Error> {
        let buf = self.read_exact(1)?;
        Ok(buf[0])
    }

    /// Read a variable length buffer.
    ///
    /// # Errors
    ///
    /// [Error::InvalidVarInt] if the length prefix is malformed, [Error::EndOfBuffer] if fewer
    /// bytes are left than it says.
    fn read_buf(&mut self) -> Result<&[u8], Error> {
        let len: u32 = self.read_var()?;
        self.read_exact(len as usize)
    }

    /// Read 2 bytes as unsigned integer
    ///
    /// # Errors
    ///
    /// [Error::EndOfBuffer] if fewer than 2 bytes are left.
    fn read_u16(&mut self) -> Result<u16, Error> {
        let buf = self.read_exact(2)?;
        Ok(buf[0] as u16 | ((buf[1] as u16) << 8))
    }

    /// Read 4 bytes as unsigned integer
    ///
    /// # Errors
    ///
    /// [Error::EndOfBuffer] if fewer than 4 bytes are left.
    fn read_u32(&mut self) -> Result<u32, Error> {
        let buf = self.read_exact(4)?;
        Ok(buf[0] as u32 | (buf[1] as u32) << 8 | (buf[2] as u32) << 16 | (buf[3] as u32) << 24)
    }

    /// Read 4 bytes as unsigned integer in big endian order.
    /// (most significant byte first)
    ///
    /// # Errors
    ///
    /// [Error::EndOfBuffer] if fewer than 4 bytes are left.
    fn read_u32_be(&mut self) -> Result<u32, Error> {
        let buf = self.read_exact(4)?;
        Ok((buf[0] as u32) << 24 | (buf[1] as u32) << 16 | (buf[2] as u32) << 8 | buf[3] as u32)
    }

    /// Read unsigned integer with variable length.
    /// * numbers < 2^7 are stored in one byte
    /// * numbers < 2^14 are stored in two bytes
    ///
    /// # Errors
    ///
    /// [Error::InvalidVarInt] if the integer doesn't fit `T` or isn't terminated,
    /// [Error::EndOfBuffer] if the input ends inside it.
    #[inline]
    fn read_var<T: VarInt>(&mut self) -> Result<T, Error> {
        T::read(self)
    }

    /// Read unsigned integer with variable length.
    /// * numbers < 2^7 are stored in one byte
    /// * numbers < 2^14 are stored in two bytes
    ///
    /// # Errors
    ///
    /// [Error::InvalidVarInt] if the integer doesn't fit `T` or isn't terminated,
    /// [Error::EndOfBuffer] if the input ends inside it.
    #[inline]
    fn read_var_signed<T: SignedVarInt>(&mut self) -> Result<Signed<T>, Error> {
        T::read_signed(self)
    }

    /// Read string of variable length.
    ///
    /// # Errors
    ///
    /// [Error::InvalidVarInt] if the length prefix is malformed, [Error::EndOfBuffer] if fewer
    /// bytes are left than it says, [Error::Malformed] if they aren't valid UTF-8.
    fn read_string(&mut self) -> Result<&str, Error> {
        let buf = self.read_buf()?;
        // Wire bytes must be valid UTF-8.
        std::str::from_utf8(buf).map_err(|_| Error::Malformed(MalformedKind::Utf8))
    }

    /// Read float32 in big endian order
    ///
    /// # Errors
    ///
    /// [Error::EndOfBuffer] if fewer than 4 bytes are left.
    fn read_f32(&mut self) -> Result<f32, Error> {
        let mut buf = [0; 4];
        let slice = self.read_exact(4)?;
        buf.copy_from_slice(slice);
        Ok(f32::from_be_bytes(buf))
    }

    /// Read float64 in big endian order
    // @todo there must be a more elegant way to convert a slice to a fixed-length buffer.
    /// # Errors
    ///
    /// [Error::EndOfBuffer] if fewer than 8 bytes are left.
    fn read_f64(&mut self) -> Result<f64, Error> {
        let mut buf = [0; 8];
        let slice = self.read_exact(8)?;
        buf.copy_from_slice(slice);
        Ok(f64::from_be_bytes(buf))
    }

    /// Read BigInt64 in big endian order
    ///
    /// # Errors
    ///
    /// [Error::EndOfBuffer] if fewer than 8 bytes are left.
    fn read_i64(&mut self) -> Result<i64, Error> {
        let mut buf = [0; 8];
        let slice = self.read_exact(8)?;
        buf.copy_from_slice(slice);
        Ok(i64::from_be_bytes(buf))
    }

    /// read BigUInt64 in big endian order
    ///
    /// # Errors
    ///
    /// [Error::EndOfBuffer] if fewer than 8 bytes are left.
    fn read_u64(&mut self) -> Result<u64, Error> {
        let mut buf = [0; 8];
        let slice = self.read_exact(8)?;
        buf.copy_from_slice(slice);
        Ok(u64::from_be_bytes(buf))
    }
}
