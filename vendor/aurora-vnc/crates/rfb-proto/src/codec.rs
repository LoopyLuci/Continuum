//! Low-level big-endian reader and writer helpers plus decode errors.

use std::fmt;

/// Why a decode did not produce a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// At least `needed` bytes in total are required before decoding can finish.
    /// No input was consumed; call again once more bytes arrive.
    Incomplete { needed: usize },
    /// The bytes violate the protocol. The connection should be closed.
    Invalid(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Incomplete { needed } => write!(f, "incomplete: need {needed} bytes"),
            DecodeError::Invalid(why) => write!(f, "invalid RFB data: {why}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Upper bounds on attacker-controlled lengths, so a hostile peer cannot make
/// us allocate gigabytes. RFC 6143 allows u32 lengths; we refuse anything past
/// these unless the embedder raises them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_name_len: u32,
    pub max_reason_len: u32,
    pub max_cut_text_len: u32,
    pub max_encodings: u16,
    pub max_colour_map_entries: u16,
    /// Most VeNCrypt subtypes accepted in (or sent as) one offer.
    pub max_vencrypt_subtypes: u8,
    /// Longest VeNCrypt Plain username or password accepted.
    pub max_credential_len: u32,
    /// Largest cursor width or height (Cursor, Cursor With Alpha) accepted.
    pub max_cursor_size: u16,
    /// Longest Aurora message body (message type 65, either direction).
    pub max_aurora_msg_len: u32,
    /// Most tiles (independent video streams) in one VideoConfig.
    pub max_video_tiles: u16,
    /// Largest compressed payload in one Aurora Video rectangle, bytes.
    pub max_video_payload: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_name_len: 64 * 1024,
            max_reason_len: 64 * 1024,
            max_cut_text_len: 16 * 1024 * 1024,
            max_encodings: 1024,
            max_colour_map_entries: u16::MAX,
            max_vencrypt_subtypes: 32,
            max_credential_len: 4096,
            max_cursor_size: 1024,
            max_aurora_msg_len: 64 * 1024,
            max_video_tiles: 64,
            max_video_payload: 64 * 1024 * 1024,
        }
    }
}

/// Cursor over an input slice that reports `Incomplete` with the total length needed.
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub fn consumed(&self) -> usize {
        self.pos
    }

    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        let end = self.pos.checked_add(n).ok_or_else(|| DecodeError::Invalid("length overflow".into()))?;
        if end > self.buf.len() {
            return Err(DecodeError::Incomplete { needed: end });
        }
        let out = &self.buf[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    pub fn skip(&mut self, n: usize) -> Result<(), DecodeError> {
        self.bytes(n).map(|_| ())
    }

    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.bytes(1)?[0])
    }

    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&mut self) -> Result<i32, DecodeError> {
        Ok(self.u32()? as i32)
    }

    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        let b = self.bytes(8)?;
        Ok(u64::from_be_bytes(b.try_into().expect("8 bytes")))
    }

    /// Bytes not yet read.
    pub fn rest(&mut self) -> &'a [u8] {
        let out = &self.buf[self.pos..];
        self.pos = self.buf.len();
        out
    }

    /// A u32 length prefix followed by that many bytes, bounded by `max`.
    pub fn len_prefixed(&mut self, max: u32, what: &str) -> Result<&'a [u8], DecodeError> {
        let len = self.u32()?;
        if len > max {
            return Err(DecodeError::Invalid(format!("{what} length {len} exceeds limit {max}")));
        }
        self.bytes(len as usize)
    }
}

/// Big-endian append helpers.
pub trait PutBe {
    fn put_u8(&mut self, v: u8);
    fn put_u16(&mut self, v: u16);
    fn put_u32(&mut self, v: u32);
    fn put_i32(&mut self, v: i32);
    fn put_u64(&mut self, v: u64);
}

impl PutBe for Vec<u8> {
    fn put_u8(&mut self, v: u8) {
        self.push(v);
    }
    fn put_u16(&mut self, v: u16) {
        self.extend_from_slice(&v.to_be_bytes());
    }
    fn put_u32(&mut self, v: u32) {
        self.extend_from_slice(&v.to_be_bytes());
    }
    fn put_i32(&mut self, v: i32) {
        self.extend_from_slice(&v.to_be_bytes());
    }
    fn put_u64(&mut self, v: u64) {
        self.extend_from_slice(&v.to_be_bytes());
    }
}

/// Encode a byte length as u32, refusing values that do not fit.
pub(crate) fn len_u32(len: usize, what: &str) -> u32 {
    u32::try_from(len).unwrap_or_else(|_| panic!("{what} longer than u32::MAX bytes"))
}
