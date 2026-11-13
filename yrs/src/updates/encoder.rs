use crate::block::ClientID;
use crate::encoding::varint::Signed;
use crate::encoding::write::Write;
use crate::*;
use std::collections::HashMap;

/// A trait that can be implemented by any other type in order to support lib0 encoding capability.
pub trait Encode {
    /// Encodes this value with the given `encoder`.
    fn encode<E: Encoder>(&self, encoder: &mut E);

    /// Helper function for encoding 1st version of lib0 encoding.
    fn encode_v1(&self) -> Vec<u8> {
        let mut encoder = EncoderV1::new();
        self.encode(&mut encoder);
        encoder.to_vec()
    }

    /// Helper function for encoding 2nd version of lib0 encoding.
    fn encode_v2(&self) -> Vec<u8> {
        let mut encoder = EncoderV2::new();
        self.encode(&mut encoder);
        encoder.to_vec()
    }
}

/// Trait used by lib0 encoders. Natively lib0 encoding supports two versions:
///
/// 1. 1st version (implemented in Yrs) uses simple optimization techniques like var int encoding.
/// 2. 2nd version optimizes bigger batches of blocks by using run-length encoding.
///
/// Both of these define a common set of operations defined in this trait.
pub trait Encoder: Write {
    /// Consume current encoder and return a binary with all data encoded so far.
    fn to_vec(self) -> Vec<u8>;

    /// Reset the state of currently encoded `DeleteSet`.
    fn reset_ds_cur_val(&mut self);

    /// Write a clock value of currently encoded `DeleteSet` client.
    fn write_ds_clock(&mut self, clock: u32);

    /// Write a number of client entries used by currently encoded `DeleteSet`.
    fn write_ds_len(&mut self, len: u32);

    /// Write unique identifier of a currently encoded `Block`'s left origin.
    fn write_left_id(&mut self, id: &block::ID);

    /// Write unique identifier of a currently encoded `Block`'s right origin.
    fn write_right_id(&mut self, id: &block::ID);

    /// Write currently encoded client identifier.
    fn write_client(&mut self, client: ClientID);

    /// Write currently encoded `Block`'s info flags. These contain information about which fields
    /// have been provided and which should be skipped during decoding process as well as a type of
    /// block currently encoded.
    fn write_info(&mut self, info: u8);

    /// Write info flag about currently encoded `Block`'s parent. Is is another block or root type.
    fn write_parent_info(&mut self, is_y_key: bool);

    /// Writes type ref data of currently encoded `Block`'s parent.
    fn write_type_ref(&mut self, info: u8);

    /// Write length parameter.
    fn write_len(&mut self, len: u32);

    /// Encode JSON-like data type. This is a complex structure which is an extension to JavaScript
    /// Object Notation with some extra cases.
    fn write_any(&mut self, any: &Any);

    /// Encode JSON-like data type as nested JSON string. This is a complex structure which is an
    /// extension to JavaScript Object Notation with some extra cases.
    fn write_json(&mut self, any: &Any);

    /// Write a string key.
    fn write_key(&mut self, string: &str);
}

/// Version 1 of lib0 encoder.
pub struct EncoderV1 {
    buf: Vec<u8>,
}

impl EncoderV1 {
    /// Creates an encoder with an empty buffer.
    pub fn new() -> Self {
        EncoderV1 {
            buf: Vec::with_capacity(1024),
        }
    }

    fn write_id(&mut self, id: &ID) {
        self.write_var(id.client.get());
        self.write_var(id.clock)
    }
}

impl Write for EncoderV1 {
    #[inline]
    fn write_all(&mut self, buf: &[u8]) {
        self.buf.write_all(buf)
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.buf.write_u8(value)
    }
}

impl Encoder for EncoderV1 {
    #[inline]
    fn to_vec(self) -> Vec<u8> {
        self.buf
    }

    #[inline]
    fn reset_ds_cur_val(&mut self) {
        /* no op */
    }

    #[inline]
    fn write_ds_clock(&mut self, clock: u32) {
        self.write_var(clock)
    }

    #[inline]
    fn write_ds_len(&mut self, len: u32) {
        self.write_var(len)
    }

    #[inline]
    fn write_left_id(&mut self, id: &ID) {
        self.write_id(id)
    }

    #[inline]
    fn write_right_id(&mut self, id: &ID) {
        self.write_id(id)
    }

    #[inline]
    fn write_client(&mut self, client: ClientID) {
        self.write_var(client.get())
    }

    #[inline]
    fn write_info(&mut self, info: u8) {
        self.write_u8(info)
    }

    #[inline]
    fn write_parent_info(&mut self, is_y_key: bool) {
        self.write_var(if is_y_key { 1 as u32 } else { 0 as u32 })
    }

    #[inline]
    fn write_type_ref(&mut self, info: u8) {
        self.write_u8(info)
    }

    #[inline]
    fn write_len(&mut self, len: u32) {
        self.write_var(len)
    }

    #[inline]
    fn write_any(&mut self, any: &Any) {
        any.encode(self)
    }

    fn write_json(&mut self, any: &Any) {
        let mut buf = String::new();
        any.to_json(&mut buf);
        self.write_string(buf.as_str())
    }

    #[inline]
    fn write_key(&mut self, key: &str) {
        self.write_string(key)
    }
}

/// A v1 encoder that hands its output to an [std::io::Write] in chunks instead of keeping it all:
/// it produces exactly [EncoderV1]'s bytes, to which it delegates. The first I/O error is kept
/// and later output dropped; [StreamEncoderV1::finish] reports it.
pub(crate) struct StreamEncoderV1<'a, W: std::io::Write> {
    inner: EncoderV1,
    sink: &'a mut W,
    error: Option<std::io::Error>,
}

impl<'a, W: std::io::Write> StreamEncoderV1<'a, W> {
    /// Output is handed to the sink once this much has been encoded.
    const CHUNK: usize = 64 * 1024;

    pub fn new(sink: &'a mut W) -> Self {
        StreamEncoderV1 {
            inner: EncoderV1 {
                buf: Vec::with_capacity(Self::CHUNK + 1024),
            },
            sink,
            error: None,
        }
    }

    fn flush_if_full(&mut self) {
        if self.inner.buf.len() >= Self::CHUNK {
            self.flush_buf();
        }
    }

    fn flush_buf(&mut self) {
        if self.error.is_none() {
            if let Err(e) = self.sink.write_all(&self.inner.buf) {
                self.error = Some(e);
            }
        }
        self.inner.buf.clear();
    }

    /// Writes what's left, and returns the first I/O error, if there was one.
    pub fn finish(mut self) -> std::io::Result<()> {
        self.flush_buf();
        match self.error.take() {
            Some(e) => Err(e),
            None => self.sink.flush(),
        }
    }
}

impl<W: std::io::Write> Write for StreamEncoderV1<'_, W> {
    fn write_all(&mut self, buf: &[u8]) {
        self.inner.write_all(buf);
        self.flush_if_full();
    }

    fn write_u8(&mut self, value: u8) {
        self.inner.write_u8(value);
        self.flush_if_full();
    }
}

impl<W: std::io::Write> Encoder for StreamEncoderV1<'_, W> {
    /// The output not yet handed to the sink.
    fn to_vec(self) -> Vec<u8> {
        self.inner.buf
    }

    fn reset_ds_cur_val(&mut self) {
        self.inner.reset_ds_cur_val()
    }

    fn write_ds_clock(&mut self, clock: u32) {
        self.inner.write_ds_clock(clock);
        self.flush_if_full();
    }

    fn write_ds_len(&mut self, len: u32) {
        self.inner.write_ds_len(len);
        self.flush_if_full();
    }

    fn write_left_id(&mut self, id: &ID) {
        self.inner.write_left_id(id);
        self.flush_if_full();
    }

    fn write_right_id(&mut self, id: &ID) {
        self.inner.write_right_id(id);
        self.flush_if_full();
    }

    fn write_client(&mut self, client: ClientID) {
        self.inner.write_client(client);
        self.flush_if_full();
    }

    fn write_info(&mut self, info: u8) {
        self.inner.write_info(info);
        self.flush_if_full();
    }

    fn write_parent_info(&mut self, is_y_key: bool) {
        self.inner.write_parent_info(is_y_key);
        self.flush_if_full();
    }

    fn write_type_ref(&mut self, info: u8) {
        self.inner.write_type_ref(info);
        self.flush_if_full();
    }

    fn write_len(&mut self, len: u32) {
        self.inner.write_len(len);
        self.flush_if_full();
    }

    fn write_any(&mut self, any: &Any) {
        self.inner.write_any(any);
        self.flush_if_full();
    }

    fn write_json(&mut self, any: &Any) {
        self.inner.write_json(any);
        self.flush_if_full();
    }

    fn write_key(&mut self, key: &str) {
        self.inner.write_key(key);
        self.flush_if_full();
    }
}

/// Version 2 of lib0 encoder. It writes values of the same kind in columns, which are run-length encoded.
pub struct EncoderV2 {
    key_table: HashMap<String, u32>,
    buf: Vec<u8>,
    ds_curr_val: u32,
    seqeuncer: u32,
    key_clock_encoder: IntDiffOptRleEncoder,
    client_encoder: UIntOptRleEncoder,
    left_clock_encoder: IntDiffOptRleEncoder,
    right_clock_encoder: IntDiffOptRleEncoder,
    info_encoder: RleEncoder,
    string_encoder: StringEncoder,
    parent_info_encoder: RleEncoder,
    type_ref_encoder: UIntOptRleEncoder,
    len_encoder: UIntOptRleEncoder,
}

impl EncoderV2 {
    /// Only what was written to the rest buffer, without the v2 feature flag and column
    /// encoders. This is Yjs' `DSEncoderV2.toUint8Array()`, used for v2 snapshots: delete sets
    /// and state vectors only ever write there.
    pub(crate) fn into_rest(self) -> Vec<u8> {
        self.buf
    }

    /// Creates an encoder with an empty buffer.
    pub fn new() -> Self {
        EncoderV2 {
            key_table: HashMap::new(),
            buf: Vec::new(),
            seqeuncer: 0,
            ds_curr_val: 0,
            key_clock_encoder: IntDiffOptRleEncoder::new(),
            client_encoder: UIntOptRleEncoder::new(),
            left_clock_encoder: IntDiffOptRleEncoder::new(),
            right_clock_encoder: IntDiffOptRleEncoder::new(),
            info_encoder: RleEncoder::new(),
            string_encoder: StringEncoder::new(),
            parent_info_encoder: RleEncoder::new(),
            type_ref_encoder: UIntOptRleEncoder::new(),
            len_encoder: UIntOptRleEncoder::new(),
        }
    }
}

impl Write for EncoderV2 {
    /// Writes raw bytes to the rest buffer. Length-prefixed content goes through
    /// [Write::write_buf], which writes the length and then calls this.
    #[inline]
    fn write_all(&mut self, buf: &[u8]) {
        self.buf.write_all(buf)
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.buf.write_u8(value)
    }

    #[inline]
    fn write_string(&mut self, str: &str) {
        self.string_encoder.write(str)
    }
}

impl Encoder for EncoderV2 {
    fn to_vec(self) -> Vec<u8> {
        let key_clock = self.key_clock_encoder.to_vec();
        let client = self.client_encoder.to_vec();
        let left_clock = self.left_clock_encoder.to_vec();
        let right_clock = self.right_clock_encoder.to_vec();
        let info = self.info_encoder.to_vec();
        let string = self.string_encoder.to_vec();
        let parent_info = self.parent_info_encoder.to_vec();
        let type_ref = self.type_ref_encoder.to_vec();
        let len = self.len_encoder.to_vec();
        let rest = self.buf;
        let mut buf = Vec::new();
        buf.write_u8(0); // this is a feature flag that we might use in the future
        buf.write_buf(key_clock);
        buf.write_buf(client);
        buf.write_buf(left_clock);
        buf.write_buf(right_clock);
        buf.write_buf(info);
        buf.write_buf(string);
        buf.write_buf(parent_info);
        buf.write_buf(type_ref);
        buf.write_buf(len);
        buf.write_all(rest.as_slice());
        buf
    }

    #[inline]
    fn reset_ds_cur_val(&mut self) {
        self.ds_curr_val = 0;
    }

    fn write_ds_clock(&mut self, clock: u32) {
        let diff = clock - self.ds_curr_val;
        self.ds_curr_val = clock;
        self.buf.write_var(diff)
    }

    fn write_ds_len(&mut self, len: u32) {
        debug_assert!(len != 0);
        self.buf.write_var(len - 1);
        self.ds_curr_val += len;
    }

    fn write_left_id(&mut self, id: &ID) {
        self.client_encoder.write_u64(id.client.get());
        self.left_clock_encoder.write_u32(id.clock)
    }

    fn write_right_id(&mut self, id: &ID) {
        self.client_encoder.write_u64(id.client.get());
        self.right_clock_encoder.write_u32(id.clock)
    }

    #[inline]
    fn write_client(&mut self, client: ClientID) {
        self.client_encoder.write_u64(client.get())
    }

    #[inline]
    fn write_info(&mut self, info: u8) {
        self.info_encoder.write_u8(info)
    }

    #[inline]
    fn write_parent_info(&mut self, is_y_key: bool) {
        self.parent_info_encoder
            .write_u8(if is_y_key { 1 } else { 0 })
    }

    #[inline]
    fn write_type_ref(&mut self, info: u8) {
        self.type_ref_encoder.write_u64(info as u64)
    }

    #[inline]
    fn write_len(&mut self, len: u32) {
        self.len_encoder.write_u64(len as u64);
    }

    #[inline]
    fn write_any(&mut self, any: &Any) {
        let mut encoder = EncoderV1 {
            buf: std::mem::take(&mut self.buf),
        };
        any.encode(&mut encoder);
        self.buf = encoder.buf;
    }

    fn write_json(&mut self, any: &Any) {
        self.write_any(any)
    }

    fn write_key(&mut self, key: &str) {
        //TODO: this is wrong (key_table is never updated), but this behavior matches Yjs
        self.key_clock_encoder.write_u32(self.seqeuncer);
        self.seqeuncer += 1;
        if self.key_table.get(key).is_none() {
            self.string_encoder.write(key);
        }
    }
}

/// A combination of the IntDiffEncoder and the UintOptRleEncoder.
///
/// The count approach is similar to the UintDiffOptRleEncoder, but instead of using the negative bitflag, it encodes
/// in the LSB whether a count is to be read. Therefore this Encoder only supports 31 bit integers!
///
/// Encodes [1, 2, 3, 2] as [3, 1, 6, -1] (more specifically [(1 << 1) | 1, (3 << 0) | 0, -1])
///
/// Internally uses variable length encoding. Contrary to normal UintVar encoding, the first byte contains:
/// * 1 bit that denotes whether the next value is a count (LSB)
/// * 1 bit that denotes whether this value is negative (MSB - 1)
/// * 1 bit that denotes whether to continue reading the variable length integer (MSB)
///
/// Therefore, only five bits remain to encode diff ranges.
///
/// Use this Encoder only when appropriate. In most cases, this is probably a bad idea.
struct IntDiffOptRleEncoder {
    buf: Vec<u8>,
    last: u32,
    count: u32,
    // i64: the difference of two u32 clocks doesn't fit an i32 (Yjs uses JS numbers)
    diff: i64,
}

impl IntDiffOptRleEncoder {
    fn new() -> Self {
        IntDiffOptRleEncoder {
            buf: Vec::new(),
            last: 0,
            count: 0,
            diff: 0,
        }
    }

    fn write_u32(&mut self, value: u32) {
        let diff = value as i64 - self.last as i64;
        if self.diff == diff {
            self.last = value;
            self.count += 1;
        } else {
            self.flush();
            self.count = 1;
            self.diff = diff;
            self.last = value;
        }
    }

    fn flush(&mut self) {
        if self.count > 0 {
            // the diff, shifted left by one | whether a count follows (Yjs: diff * 2 + bit)
            let encode_diff = self.diff * 2 + if self.count == 1 { 0 } else { 1 };
            // flush counter, unless this is the first value (count = 0)
            // case 1: just a single value. set first bit to positive
            // case 2: write several values. set first bit to negative to indicate that there is a length coming
            self.buf.write_var(encode_diff);
            if self.count > 1 {
                self.buf.write_var(self.count - 2);
            }
        }
    }

    fn to_vec(mut self) -> Vec<u8> {
        self.flush();
        self.buf
    }
}

/// Optimized Rle encoder that does not suffer from the mentioned problem of the basic Rle encoder.
///
/// Internally uses VarInt encoder to write unsigned integers. If the input occurs multiple times, we write
/// write it as a negative number. The UintOptRleDecoder then understands that it needs to read a count.
///
/// Encodes [1,2,3,3,3] as [1,2,-3,3] (once 1, once 2, three times 3)
struct UIntOptRleEncoder {
    buf: Vec<u8>,
    last: u64,
    count: u32,
}

impl UIntOptRleEncoder {
    fn new() -> Self {
        UIntOptRleEncoder {
            buf: Vec::new(),
            last: 0,
            count: 0,
        }
    }

    fn write_u64(&mut self, value: u64) {
        if self.last == value {
            self.count += 1;
        } else {
            self.flush();
            self.count = 1;
            self.last = value;
        }
    }

    fn to_vec(mut self) -> Vec<u8> {
        self.flush();
        self.buf
    }

    fn flush(&mut self) {
        if self.count > 0 {
            // flush counter, unless this is the first value (count = 0)
            // case 1: just a single value. set sign to positive
            // case 2: write several values. set sign to negative to indicate that there is a length coming
            if self.count == 1 {
                self.buf.write_var(self.last as i64);
            } else {
                let value = Signed::new(-(self.last as i64), true);
                self.buf.write_var_signed(&value);
                self.buf.write_var(self.count - 2);
            }
        }
    }
}

/// Basic Run Length Encoder - a basic compression implementation.
///
/// Encodes [1,1,1,7] to [1,3,7,1] (3 times 1, 1 time 7). This encoder might do more harm than good if there are a lot of values that are not repeated.
///
/// It was originally used for image compression. Cool .. article http://csbruce.com/cbm/transactor/pdfs/trans_v7_i06.pdf
struct RleEncoder {
    buf: Vec<u8>,
    last: Option<u8>,
    count: u32,
}

impl RleEncoder {
    fn new() -> Self {
        RleEncoder {
            buf: Vec::new(),
            last: None,
            count: 0,
        }
    }

    fn write_u8(&mut self, value: u8) {
        if self.last == Some(value) {
            self.count += 1;
        } else {
            if self.count > 0 {
                // flush counter, unless this is the first value (count = 0)
                self.buf.write_var(self.count - 1);
            }
            self.count = 1;
            self.buf.write_u8(value);
            self.last = Some(value);
        }
    }

    fn to_vec(self) -> Vec<u8> {
        self.buf
    }
}

/// Optimized String Encoder.
///
/// Encoding many small strings in a simple Encoder is not very efficient. The function call to decode a string takes some time and creates references that must be eventually deleted.
/// In practice, when decoding several million small strings, the GC will kick in more and more often to collect orphaned string objects (or maybe there is another reason?).
///
/// This string encoder solves the above problem. All strings are concatenated and written as a single string using a single encoding call.
///
/// The lengths are encoded using a UintOptRleEncoder.
struct StringEncoder {
    buf: String,
    len_encoder: UIntOptRleEncoder,
}

impl StringEncoder {
    fn new() -> Self {
        StringEncoder {
            buf: String::new(),
            len_encoder: UIntOptRleEncoder::new(),
        }
    }

    fn write(&mut self, str: &str) {
        let utf16_len = str.encode_utf16().count(); // Yjs encodes offsets using utf-16
        self.buf.push_str(str);
        self.len_encoder.write_u64(utf16_len as u64);
    }

    fn to_vec(self) -> Vec<u8> {
        let lengths = self.len_encoder.to_vec();
        let mut buf = Vec::with_capacity(self.buf.len() + lengths.len());
        buf.write_string(&self.buf);
        buf.write_all(lengths.as_slice());
        buf
    }
}
