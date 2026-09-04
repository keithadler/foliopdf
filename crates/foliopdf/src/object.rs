//! The PDF object model.
//!
//! PDF files are built from eight basic object types (ISO 32000-1 §7.3). This
//! module models them as the [`Object`] enum together with a few strongly
//! typed helpers: [`Name`], [`PdfString`], [`Dict`], [`Stream`] and [`ObjRef`].
//!
//! Objects are plain values. A [`Dict`] owns its entries; an [`ObjRef`] is an
//! indirect reference that must be resolved through a
//! [`Document`](crate::Document).

use std::collections::BTreeMap;
use std::fmt;

/// An indirect object reference (`12 0 R`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct ObjRef {
    /// Object number, starting at 1.
    pub num: u32,
    /// Generation number. Almost always 0.
    pub gen: u16,
}

impl ObjRef {
    /// Creates a reference to object `num` with generation `gen`.
    pub const fn new(num: u32, gen: u16) -> Self {
        Self { num, gen }
    }
}

impl fmt::Display for ObjRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {} R", self.num, self.gen)
    }
}

/// A PDF name object such as `/Type`. Stored without the leading slash.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Name(pub Vec<u8>);

impl Name {
    /// Creates a name from a string slice.
    pub fn new(s: &str) -> Self {
        Name(s.as_bytes().to_vec())
    }
    /// Returns the name as bytes (without the `/`).
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
    /// Returns the name as a lossy UTF-8 string.
    pub fn as_str(&self) -> std::borrow::Cow<'_, str> {
        String::from_utf8_lossy(&self.0)
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "/{}", self.as_str())
    }
}

impl From<&str> for Name {
    fn from(s: &str) -> Self {
        Name::new(s)
    }
}

impl PartialEq<str> for Name {
    fn eq(&self, other: &str) -> bool {
        self.0 == other.as_bytes()
    }
}
impl PartialEq<&str> for Name {
    fn eq(&self, other: &&str) -> bool {
        self.0 == other.as_bytes()
    }
}

/// A PDF string object. Strings are byte sequences; text encoding depends on
/// context (PDFDocEncoding, UTF-16BE with BOM, or raw bytes).
#[derive(Clone, PartialEq, Eq, Hash, Default)]
pub struct PdfString {
    /// Raw, unescaped bytes.
    pub bytes: Vec<u8>,
    /// Whether the string was written in hex form (`<...>`). Preserved for
    /// round-tripping; has no semantic meaning.
    pub hex: bool,
}

impl PdfString {
    /// Creates a literal string from raw bytes.
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
            hex: false,
        }
    }
    /// Creates a hex-form string from raw bytes.
    pub fn hex(bytes: impl Into<Vec<u8>>) -> Self {
        Self {
            bytes: bytes.into(),
            hex: true,
        }
    }
    /// Encodes a Rust string as a PDF text string. ASCII-only input is stored
    /// as PDFDocEncoding; anything else as UTF-16BE with a byte-order mark.
    pub fn from_text(s: &str) -> Self {
        if s.is_ascii() {
            return Self::new(s.as_bytes().to_vec());
        }
        let mut out = vec![0xFE, 0xFF];
        for u in s.encode_utf16() {
            out.extend_from_slice(&u.to_be_bytes());
        }
        Self::new(out)
    }
    /// Decodes a PDF text string (UTF-16 with BOM, UTF-8 with BOM, or
    /// PDFDocEncoding) to a Rust string.
    pub fn to_text(&self) -> String {
        let b = &self.bytes;
        if b.len() >= 2 && b[0] == 0xFE && b[1] == 0xFF {
            let units: Vec<u16> = b[2..]
                .chunks(2)
                .filter(|c| c.len() == 2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            return String::from_utf16_lossy(&units);
        }
        if b.len() >= 2 && b[0] == 0xFF && b[1] == 0xFE {
            let units: Vec<u16> = b[2..]
                .chunks(2)
                .filter(|c| c.len() == 2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]))
                .collect();
            return String::from_utf16_lossy(&units);
        }
        if b.len() >= 3 && b[0] == 0xEF && b[1] == 0xBB && b[2] == 0xBF {
            return String::from_utf8_lossy(&b[3..]).into_owned();
        }
        b.iter().map(|&c| pdfdoc_to_char(c)).collect()
    }
    /// Raw bytes of the string.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl fmt::Debug for PdfString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({:?})", self.to_text())
    }
}

/// Maps a PDFDocEncoding byte to a char. Bytes 0x80..0xA0 hold a small set of
/// typographic characters; the rest is Latin-1.
fn pdfdoc_to_char(c: u8) -> char {
    match c {
        0x80 => '\u{2022}',
        0x81 => '\u{2020}',
        0x82 => '\u{2021}',
        0x83 => '\u{2026}',
        0x84 => '\u{2014}',
        0x85 => '\u{2013}',
        0x86 => '\u{0192}',
        0x87 => '\u{2044}',
        0x88 => '\u{2039}',
        0x89 => '\u{203A}',
        0x8A => '\u{2212}',
        0x8B => '\u{2030}',
        0x8C => '\u{201E}',
        0x8D => '\u{201C}',
        0x8E => '\u{201D}',
        0x8F => '\u{2018}',
        0x90 => '\u{2019}',
        0x91 => '\u{201A}',
        0x92 => '\u{2122}',
        0x93 => '\u{FB01}',
        0x94 => '\u{FB02}',
        0x95 => '\u{0141}',
        0x96 => '\u{0152}',
        0x97 => '\u{0160}',
        0x98 => '\u{0178}',
        0x99 => '\u{017D}',
        0x9A => '\u{0131}',
        0x9B => '\u{0142}',
        0x9C => '\u{0153}',
        0x9D => '\u{0161}',
        0x9E => '\u{017E}',
        0xA0 => '\u{20AC}',
        c => c as char,
    }
}

/// A PDF dictionary. Keys are [`Name`]s; iteration order is sorted by key so
/// output is deterministic.
#[derive(Clone, PartialEq, Default)]
pub struct Dict(pub BTreeMap<Name, Object>);

impl Dict {
    /// Creates an empty dictionary.
    pub fn new() -> Self {
        Self::default()
    }
    /// Looks up `key`, returning `None` when absent.
    pub fn get(&self, key: &str) -> Option<&Object> {
        self.0.get(key.as_bytes())
    }
    /// Mutable lookup.
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Object> {
        self.0.get_mut(key.as_bytes())
    }
    /// Inserts or replaces `key`.
    pub fn set(&mut self, key: &str, value: impl Into<Object>) -> &mut Self {
        self.0.insert(Name::new(key), value.into());
        self
    }
    /// Removes `key`, returning the previous value.
    pub fn remove(&mut self, key: &str) -> Option<Object> {
        self.0.remove(key.as_bytes())
    }
    /// Whether `key` is present.
    pub fn contains(&self, key: &str) -> bool {
        self.0.contains_key(key.as_bytes())
    }
    /// Number of entries.
    pub fn len(&self) -> usize {
        self.0.len()
    }
    /// Whether the dictionary has no entries.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    /// Iterates over `(key, value)` pairs in sorted key order.
    pub fn iter(&self) -> impl Iterator<Item = (&Name, &Object)> {
        self.0.iter()
    }
    /// Builder-style insert.
    pub fn with(mut self, key: &str, value: impl Into<Object>) -> Self {
        self.set(key, value);
        self
    }
}

impl std::borrow::Borrow<[u8]> for Name {
    fn borrow(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Dict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.0.iter()).finish()
    }
}

/// A stream: a dictionary plus a byte payload. `data` holds the bytes exactly
/// as they appear in the file (still encoded). Use
/// [`Document::stream_data`](crate::Document::stream_data) to decode.
#[derive(Clone, PartialEq)]
pub struct Stream {
    /// The stream dictionary (`/Length`, `/Filter`, ...).
    pub dict: Dict,
    /// Encoded payload.
    pub data: Vec<u8>,
}

impl Stream {
    /// Creates a stream with already-encoded data. The `/Length` key is
    /// maintained by the writer, so callers need not set it.
    pub fn new(dict: Dict, data: Vec<u8>) -> Self {
        Self { dict, data }
    }
    /// Names of the filters applied to this stream, outermost first.
    pub fn filters(&self) -> Vec<Name> {
        match self.dict.get("Filter") {
            Some(Object::Name(n)) => vec![n.clone()],
            Some(Object::Array(a)) => a.iter().filter_map(|o| o.as_name().cloned()).collect(),
            _ => Vec::new(),
        }
    }
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "stream {:?} [{} bytes]", self.dict, self.data.len())
    }
}

/// Any PDF object.
#[derive(Clone, PartialEq, Default)]
pub enum Object {
    /// The `null` object.
    #[default]
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// An integer.
    Integer(i64),
    /// A real number.
    Real(f64),
    /// A string.
    String(PdfString),
    /// A name.
    Name(Name),
    /// An array.
    Array(Vec<Object>),
    /// A dictionary.
    Dict(Dict),
    /// A stream (always an indirect object).
    Stream(Box<Stream>),
    /// A reference to an indirect object.
    Reference(ObjRef),
}

impl fmt::Debug for Object {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Object::Null => write!(f, "null"),
            Object::Bool(b) => write!(f, "{b}"),
            Object::Integer(i) => write!(f, "{i}"),
            Object::Real(r) => write!(f, "{r}"),
            Object::String(s) => write!(f, "{s:?}"),
            Object::Name(n) => write!(f, "{n:?}"),
            Object::Array(a) => f.debug_list().entries(a).finish(),
            Object::Dict(d) => write!(f, "{d:?}"),
            Object::Stream(s) => write!(f, "{s:?}"),
            Object::Reference(r) => write!(f, "{r}"),
        }
    }
}

impl Object {
    /// A short, static description of the variant. Used in error messages.
    pub fn type_name(&self) -> &'static str {
        match self {
            Object::Null => "null",
            Object::Bool(_) => "boolean",
            Object::Integer(_) => "integer",
            Object::Real(_) => "real",
            Object::String(_) => "string",
            Object::Name(_) => "name",
            Object::Array(_) => "array",
            Object::Dict(_) => "dictionary",
            Object::Stream(_) => "stream",
            Object::Reference(_) => "reference",
        }
    }
    /// Creates a name object.
    pub fn name(s: &str) -> Self {
        Object::Name(Name::new(s))
    }
    /// Creates a literal string object from raw bytes.
    pub fn string(bytes: impl Into<Vec<u8>>) -> Self {
        Object::String(PdfString::new(bytes))
    }
    /// Creates a text string object (see [`PdfString::from_text`]).
    pub fn text(s: &str) -> Self {
        Object::String(PdfString::from_text(s))
    }
    /// Whether this is `null`.
    pub fn is_null(&self) -> bool {
        matches!(self, Object::Null)
    }
    /// Returns the boolean value if this is a boolean.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Object::Bool(b) => Some(*b),
            _ => None,
        }
    }
    /// Returns the integer value if this is an integer (or an integral real).
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Object::Integer(i) => Some(*i),
            Object::Real(r) if r.fract() == 0.0 => Some(*r as i64),
            _ => None,
        }
    }
    /// Returns the value as `f64` if this is numeric.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Object::Integer(i) => Some(*i as f64),
            Object::Real(r) => Some(*r),
            _ => None,
        }
    }
    /// Returns the name if this is a name object.
    pub fn as_name(&self) -> Option<&Name> {
        match self {
            Object::Name(n) => Some(n),
            _ => None,
        }
    }
    /// Returns the string if this is a string object.
    pub fn as_string(&self) -> Option<&PdfString> {
        match self {
            Object::String(s) => Some(s),
            _ => None,
        }
    }
    /// Returns the array if this is an array object.
    pub fn as_array(&self) -> Option<&[Object]> {
        match self {
            Object::Array(a) => Some(a),
            _ => None,
        }
    }
    /// Mutable array access.
    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Object>> {
        match self {
            Object::Array(a) => Some(a),
            _ => None,
        }
    }
    /// Returns the dictionary of a dictionary *or* stream object.
    pub fn as_dict(&self) -> Option<&Dict> {
        match self {
            Object::Dict(d) => Some(d),
            Object::Stream(s) => Some(&s.dict),
            _ => None,
        }
    }
    /// Mutable dictionary access (dictionary or stream).
    pub fn as_dict_mut(&mut self) -> Option<&mut Dict> {
        match self {
            Object::Dict(d) => Some(d),
            Object::Stream(s) => Some(&mut s.dict),
            _ => None,
        }
    }
    /// Returns the stream if this is a stream object.
    pub fn as_stream(&self) -> Option<&Stream> {
        match self {
            Object::Stream(s) => Some(s),
            _ => None,
        }
    }
    /// Mutable stream access.
    pub fn as_stream_mut(&mut self) -> Option<&mut Stream> {
        match self {
            Object::Stream(s) => Some(s),
            _ => None,
        }
    }
    /// Returns the reference if this is an indirect reference.
    pub fn as_reference(&self) -> Option<ObjRef> {
        match self {
            Object::Reference(r) => Some(*r),
            _ => None,
        }
    }
    /// Converts into a dictionary, if possible.
    pub fn into_dict(self) -> Option<Dict> {
        match self {
            Object::Dict(d) => Some(d),
            Object::Stream(s) => Some(s.dict),
            _ => None,
        }
    }
    /// Converts into an array, if possible.
    pub fn into_array(self) -> Option<Vec<Object>> {
        match self {
            Object::Array(a) => Some(a),
            _ => None,
        }
    }
}

macro_rules! from_impl {
    ($t:ty, $variant:ident) => {
        impl From<$t> for Object {
            fn from(v: $t) -> Self {
                Object::$variant(v.into())
            }
        }
    };
}
from_impl!(bool, Bool);
from_impl!(i64, Integer);
from_impl!(f64, Real);
from_impl!(Name, Name);
from_impl!(PdfString, String);
from_impl!(Dict, Dict);
from_impl!(ObjRef, Reference);
from_impl!(Vec<Object>, Array);

impl From<i32> for Object {
    fn from(v: i32) -> Self {
        Object::Integer(v as i64)
    }
}
impl From<u32> for Object {
    fn from(v: u32) -> Self {
        Object::Integer(v as i64)
    }
}
impl From<usize> for Object {
    fn from(v: usize) -> Self {
        Object::Integer(v as i64)
    }
}
impl From<f32> for Object {
    fn from(v: f32) -> Self {
        Object::Real(v as f64)
    }
}
impl From<Stream> for Object {
    fn from(v: Stream) -> Self {
        Object::Stream(Box::new(v))
    }
}
impl From<&str> for Object {
    /// A `&str` becomes a **name**; use [`Object::text`] for strings.
    fn from(v: &str) -> Self {
        Object::Name(Name::new(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_round_trip() {
        let s = PdfString::from_text("héllo — wörld");
        assert_eq!(s.to_text(), "héllo — wörld");
        let a = PdfString::from_text("plain");
        assert_eq!(a.bytes, b"plain");
        assert_eq!(a.to_text(), "plain");
    }

    #[test]
    fn pdfdoc_specials() {
        assert_eq!(PdfString::new(vec![0x84]).to_text(), "\u{2014}");
    }

    #[test]
    fn dict_access() {
        let mut d = Dict::new();
        d.set("Type", "Page").set("Count", 3);
        assert_eq!(d.get("Count").and_then(Object::as_i64), Some(3));
        assert_eq!(d.get("Type").and_then(Object::as_name).unwrap(), "Page");
    }
}
