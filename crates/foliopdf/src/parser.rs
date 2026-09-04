//! Object parser and cross-reference reader.
//!
//! Two entry points matter to the rest of the crate:
//!
//! * [`Parser`] turns tokens into [`Object`]s, including indirect objects and
//!   streams.
//! * [`read_xref`] locates and follows the cross-reference chain (tables and
//!   streams, `/Prev` and `/XRefStm` links). When that fails,
//!   [`reconstruct`] scans the whole file for `N G obj` headers so damaged
//!   documents still open.

use std::collections::{HashMap, HashSet};

use crate::error::{Error, Result};
use crate::filters;
use crate::lexer::{find_bytes, is_regular, is_whitespace, rfind_bytes, Lexer, Token};
use crate::object::{Dict, ObjRef, Object, Stream};

/// Maximum nesting depth for arrays/dictionaries. Protects against stack
/// exhaustion on hostile input.
const MAX_DEPTH: usize = 256;

/// Callback used to resolve an indirect `/Length` while parsing a stream.
pub type LengthResolver<'r> = &'r dyn Fn(ObjRef) -> Option<i64>;

/// Parses PDF objects from a byte slice.
pub struct Parser<'a> {
    pub(crate) lex: Lexer<'a>,
    depth: usize,
}

impl<'a> Parser<'a> {
    /// Creates a parser over `data`, starting at byte `pos`.
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Self {
            lex: Lexer::at(data, pos),
            depth: 0,
        }
    }

    /// Current byte offset.
    pub fn pos(&self) -> usize {
        self.lex.pos()
    }

    /// Parses one object. `n g R` references are recognised; streams are
    /// parsed when a dictionary is followed by the `stream` keyword.
    pub fn parse_object(&mut self, resolver: Option<LengthResolver<'_>>) -> Result<Object> {
        let tok = self.lex.next_token()?;
        self.parse_from_token(tok, resolver)
    }

    fn parse_from_token(
        &mut self,
        tok: Token,
        resolver: Option<LengthResolver<'_>>,
    ) -> Result<Object> {
        match tok {
            Token::Integer(i) => {
                // Possible reference: int int R
                let save = self.lex.pos();
                if i >= 0 {
                    if let Ok(Token::Integer(g)) = self.lex.next_token() {
                        if g >= 0 {
                            let save2 = self.lex.pos();
                            if let Ok(Token::Keyword(k)) = self.lex.next_token() {
                                if k == b"R" {
                                    return Ok(Object::Reference(ObjRef::new(
                                        i as u32,
                                        g.min(65535) as u16,
                                    )));
                                }
                            }
                            self.lex.seek(save2);
                        }
                    }
                }
                self.lex.seek(save);
                Ok(Object::Integer(i))
            }
            Token::Real(r) => Ok(Object::Real(r)),
            Token::String(s) => Ok(Object::String(s)),
            Token::Name(n) => Ok(Object::Name(n)),
            Token::ArrayOpen => {
                self.enter()?;
                let mut items = Vec::new();
                loop {
                    let t = self.lex.next_token()?;
                    match t {
                        Token::ArrayClose => break,
                        Token::Eof => break,
                        Token::DictClose => continue, // stray, ignore
                        Token::Keyword(ref k) if k == b"endobj" || k == b"endstream" => {
                            // Unterminated array; rewind so the caller sees the keyword.
                            self.lex.seek(self.lex.pos() - k.len());
                            break;
                        }
                        t => items.push(self.parse_from_token(t, resolver)?),
                    }
                }
                self.depth -= 1;
                Ok(Object::Array(items))
            }
            Token::DictOpen => {
                self.enter()?;
                let mut dict = Dict::new();
                loop {
                    let t = self.lex.next_token()?;
                    match t {
                        Token::DictClose | Token::Eof => break,
                        Token::Name(key) => {
                            let vt = self.lex.next_token()?;
                            if vt == Token::DictClose {
                                dict.0.insert(key, Object::Null);
                                break;
                            }
                            let v = self.parse_from_token(vt, resolver)?;
                            dict.0.insert(key, v);
                        }
                        Token::Keyword(ref k) if k == b"endobj" || k == b"stream" => {
                            self.lex.seek(self.lex.pos() - k.len());
                            break;
                        }
                        // Non-name key: skip the value-ish token and continue.
                        _ => continue,
                    }
                }
                self.depth -= 1;
                // Stream?
                let save = self.lex.pos();
                if let Ok(Token::Keyword(k)) = self.lex.next_token() {
                    if k == b"stream" {
                        return self.parse_stream_body(dict, resolver);
                    }
                }
                self.lex.seek(save);
                Ok(Object::Dict(dict))
            }
            Token::Keyword(k) => match k.as_slice() {
                b"true" => Ok(Object::Bool(true)),
                b"false" => Ok(Object::Bool(false)),
                b"null" => Ok(Object::Null),
                _ => Err(Error::syntax(
                    self.lex.pos(),
                    format!("unexpected keyword '{}'", String::from_utf8_lossy(&k)),
                )),
            },
            Token::ArrayClose | Token::DictClose | Token::BraceOpen | Token::BraceClose => {
                Err(Error::syntax(self.lex.pos(), "unexpected delimiter"))
            }
            Token::Eof => Err(Error::syntax(self.lex.pos(), "unexpected end of data")),
        }
    }

    fn enter(&mut self) -> Result<()> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(Error::Limit(format!(
                "object nesting deeper than {MAX_DEPTH}"
            )));
        }
        Ok(())
    }

    /// Reads stream data after the `stream` keyword has been consumed.
    fn parse_stream_body(
        &mut self,
        dict: Dict,
        resolver: Option<LengthResolver<'_>>,
    ) -> Result<Object> {
        let data = self.lex.data;
        let mut p = self.lex.pos();
        // EOL after 'stream': CRLF or LF (tolerate lone CR and spaces).
        while p < data.len() && data[p] == b' ' {
            p += 1;
        }
        if p < data.len() && data[p] == b'\r' {
            p += 1;
        }
        if p < data.len() && data[p] == b'\n' {
            p += 1;
        }
        let start = p.min(data.len());

        let declared = match dict.get("Length") {
            Some(Object::Integer(n)) => Some(*n),
            Some(Object::Reference(r)) => resolver.and_then(|f| f(*r)),
            _ => None,
        };

        let mut end: Option<usize> = None;
        if let Some(len) = declared {
            if len >= 0 {
                let e = start.saturating_add(len as usize);
                if e <= data.len() && endstream_follows(data, e) {
                    end = Some(e);
                }
            }
        }
        let end = match end {
            Some(e) => e,
            None => {
                // Length is wrong or missing: search for the terminator.
                let idx = find_bytes(data, start, b"endstream").unwrap_or(data.len());
                let mut e = idx;
                // Strip one trailing EOL that belongs to the syntax, not the data.
                if e > start && data[e - 1] == b'\n' {
                    e -= 1;
                }
                if e > start && data[e - 1] == b'\r' {
                    e -= 1;
                }
                e
            }
        };
        let payload = data[start..end].to_vec();
        // Position after 'endstream'
        let after = find_bytes(data, end, b"endstream")
            .map(|i| i + 9)
            .unwrap_or(data.len());
        self.lex.seek(after);
        Ok(Object::Stream(Box::new(Stream::new(dict, payload))))
    }

    /// Parses `N G obj <object> endobj` at the current position.
    pub fn parse_indirect(
        &mut self,
        resolver: Option<LengthResolver<'_>>,
    ) -> Result<(ObjRef, Object)> {
        let start = self.lex.pos();
        let num = match self.lex.next_token()? {
            Token::Integer(n) if n >= 0 => n as u32,
            _ => return Err(Error::syntax(start, "expected object number")),
        };
        let gen = match self.lex.next_token()? {
            Token::Integer(g) if g >= 0 => g.min(65535) as u16,
            _ => return Err(Error::syntax(start, "expected generation number")),
        };
        if !self.lex.expect_keyword(b"obj") {
            return Err(Error::syntax(start, "expected 'obj'"));
        }
        let save = self.lex.pos();
        let obj = match self.lex.next_token()? {
            Token::Keyword(ref k) if k == b"endobj" => Object::Null,
            t => {
                self.lex.seek(save);
                let _ = t;
                self.parse_object(resolver)?
            }
        };
        // Consume optional endobj.
        let _ = self.lex.expect_keyword(b"endobj");
        Ok((ObjRef::new(num, gen), obj))
    }
}

fn endstream_follows(data: &[u8], mut p: usize) -> bool {
    let limit = (p + 4).min(data.len());
    while p < limit && is_whitespace(data[p]) {
        p += 1;
    }
    data.len() >= p + 9 && &data[p..p + 9] == b"endstream"
}

/// Where an object lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrefEntry {
    /// Object number is unused.
    Free,
    /// Uncompressed object at a byte offset.
    Offset {
        /// Absolute byte offset of `N G obj`.
        offset: usize,
        /// Generation number.
        gen: u16,
    },
    /// Object stored inside an object stream (`/Type /ObjStm`).
    InStream {
        /// Object number of the containing stream.
        stream_num: u32,
        /// Index within the stream.
        index: u32,
    },
}

/// The merged cross-reference information for a document.
#[derive(Debug, Clone, Default)]
pub struct Xref {
    /// Object number to location. Newest definition wins.
    pub entries: HashMap<u32, XrefEntry>,
    /// The merged trailer dictionary (newest keys win).
    pub trailer: Dict,
    /// Whether the table was rebuilt by scanning instead of read from the
    /// cross-reference structures.
    pub reconstructed: bool,
}

/// Reads the cross-reference chain starting from `startxref`.
pub fn read_xref(data: &[u8]) -> Result<Xref> {
    let tail_start = data.len().saturating_sub(2048);
    let sx = rfind_bytes(data, data.len(), b"startxref")
        .filter(|&i| i >= tail_start)
        .or_else(|| rfind_bytes(data, data.len(), b"startxref"))
        .ok_or_else(|| Error::malformed("no startxref"))?;
    let mut lex = Lexer::at(data, sx + 9);
    let offset = match lex.next_token()? {
        Token::Integer(o) if o >= 0 => o as usize,
        _ => return Err(Error::malformed("bad startxref offset")),
    };
    let mut xref = Xref::default();
    let mut seen = HashSet::new();
    let mut queue = vec![offset];
    while let Some(off) = queue.pop() {
        if !seen.insert(off) || seen.len() > 1024 {
            continue;
        }
        let trailer = read_xref_section(data, off, &mut xref)?;
        // Hybrid files: /XRefStm points to an xref stream that supplements
        // the table. It takes precedence over /Prev.
        if let Some(Object::Integer(p)) = trailer.get("Prev") {
            if *p >= 0 {
                queue.push(*p as usize);
            }
        }
        if let Some(Object::Integer(s)) = trailer.get("XRefStm") {
            if *s >= 0 {
                queue.push(*s as usize);
            }
        }
        for (k, v) in trailer.iter() {
            if !xref.trailer.0.contains_key(k) {
                xref.trailer.0.insert(k.clone(), v.clone());
            }
        }
    }
    if !xref.trailer.contains("Root") {
        return Err(Error::malformed("trailer has no /Root"));
    }
    Ok(xref)
}

/// Reads one xref section (table or stream) at `offset`, adding entries not
/// already present. Returns that section's trailer dictionary.
fn read_xref_section(data: &[u8], offset: usize, xref: &mut Xref) -> Result<Dict> {
    if offset >= data.len() {
        return Err(Error::malformed(format!(
            "xref offset {offset} beyond end of file"
        )));
    }
    let mut lex = Lexer::at(data, offset);
    lex.skip_whitespace();
    if lex.rest().starts_with(b"xref") {
        lex.seek(lex.pos() + 4);
        return read_xref_table(data, lex.pos(), xref);
    }
    // Cross-reference stream: "N G obj <<...>> stream"
    let mut parser = Parser::new(data, lex.pos());
    let (_r, obj) = parser.parse_indirect(None)?;
    let stream = match obj {
        Object::Stream(s) => *s,
        _ => {
            return Err(Error::malformed(
                "xref offset does not point at a table or stream",
            ))
        }
    };
    read_xref_stream(&stream, xref)
}

fn read_xref_table(data: &[u8], pos: usize, xref: &mut Xref) -> Result<Dict> {
    let mut lex = Lexer::at(data, pos);
    loop {
        lex.skip_whitespace();
        if lex.rest().starts_with(b"trailer") {
            lex.seek(lex.pos() + 7);
            let mut p = Parser::new(data, lex.pos());
            return match p.parse_object(None)? {
                Object::Dict(d) => Ok(d),
                _ => Err(Error::malformed("trailer is not a dictionary")),
            };
        }
        let start = match lex.next_token()? {
            Token::Integer(s) if s >= 0 => s as u32,
            Token::Eof => return Ok(Dict::new()),
            _ => return Err(Error::syntax(lex.pos(), "bad xref subsection header")),
        };
        let count = match lex.next_token()? {
            Token::Integer(c) if c >= 0 => c as u32,
            _ => return Err(Error::syntax(lex.pos(), "bad xref subsection count")),
        };
        for i in 0..count {
            lex.skip_whitespace();
            // Some writers emit "trailer" early when count is wrong.
            if lex.rest().starts_with(b"trailer") {
                break;
            }
            let off = match lex.next_token()? {
                Token::Integer(o) => o,
                _ => return Err(Error::syntax(lex.pos(), "bad xref entry offset")),
            };
            let gen = match lex.next_token()? {
                Token::Integer(g) => g,
                _ => return Err(Error::syntax(lex.pos(), "bad xref entry generation")),
            };
            let kind = match lex.next_token()? {
                Token::Keyword(k) => k,
                _ => return Err(Error::syntax(lex.pos(), "bad xref entry type")),
            };
            let num = start + i;
            if xref.entries.contains_key(&num) {
                continue;
            }
            let entry = if kind == b"n" && off >= 0 {
                XrefEntry::Offset {
                    offset: off as usize,
                    gen: gen.clamp(0, 65535) as u16,
                }
            } else {
                XrefEntry::Free
            };
            xref.entries.insert(num, entry);
        }
    }
}

fn read_xref_stream(stream: &Stream, xref: &mut Xref) -> Result<Dict> {
    let dict = &stream.dict;
    let content = filters::decode_stream(stream, None)?;
    let w: Vec<usize> = dict
        .get("W")
        .and_then(Object::as_array)
        .ok_or_else(|| Error::MissingKey("W".into()))?
        .iter()
        .map(|o| o.as_i64().unwrap_or(0).max(0) as usize)
        .collect();
    if w.len() < 3 || w.iter().any(|&x| x > 8) {
        return Err(Error::malformed("bad /W array in xref stream"));
    }
    let size = dict
        .get("Size")
        .and_then(Object::as_i64)
        .unwrap_or(0)
        .max(0) as u32;
    let index: Vec<u32> = match dict.get("Index").and_then(Object::as_array) {
        Some(a) => a
            .iter()
            .map(|o| o.as_i64().unwrap_or(0).max(0) as u32)
            .collect(),
        None => vec![0, size],
    };
    let row: usize = w.iter().sum();
    if row == 0 {
        return Err(Error::malformed("zero-width xref stream rows"));
    }
    let mut pos = 0usize;
    for pair in index.chunks(2) {
        if pair.len() < 2 {
            break;
        }
        let (start, count) = (pair[0], pair[1]);
        for i in 0..count {
            if pos + row > content.len() {
                break;
            }
            let mut fields = [1u64, 0, 0];
            let mut p = pos;
            for (fi, &width) in w.iter().take(3).enumerate() {
                if width == 0 {
                    continue;
                }
                let mut v = 0u64;
                for _ in 0..width {
                    v = (v << 8) | content[p] as u64;
                    p += 1;
                }
                fields[fi] = v;
            }
            pos += row;
            let num = start.saturating_add(i);
            if xref.entries.contains_key(&num) {
                continue;
            }
            let entry = match fields[0] {
                1 => XrefEntry::Offset {
                    offset: fields[1] as usize,
                    gen: fields[2].min(65535) as u16,
                },
                2 => XrefEntry::InStream {
                    stream_num: fields[1] as u32,
                    index: fields[2] as u32,
                },
                _ => XrefEntry::Free,
            };
            xref.entries.insert(num, entry);
        }
    }
    Ok(dict.clone())
}

/// Rebuilds the cross-reference table by scanning for `N G obj` headers and
/// `trailer` dictionaries. Later definitions override earlier ones, which
/// matches incremental-update semantics.
pub fn reconstruct(data: &[u8]) -> Result<Xref> {
    let mut xref = Xref {
        reconstructed: true,
        ..Default::default()
    };
    let mut i = 0usize;
    while let Some(idx) = find_bytes(data, i, b"obj") {
        i = idx + 3;
        // Must be followed by a delimiter/whitespace (not "object" in a comment).
        if let Some(&c) = data.get(idx + 3) {
            if is_regular(c) {
                continue;
            }
        }
        // Walk backwards: ws* digits(gen) ws+ digits(num) and a non-regular char before.
        let mut p = idx;
        while p > 0 && is_whitespace(data[p - 1]) {
            p -= 1;
        }
        let gen_end = p;
        while p > 0 && data[p - 1].is_ascii_digit() {
            p -= 1;
        }
        let gen_start = p;
        if gen_start == gen_end || p == 0 || !is_whitespace(data[p - 1]) {
            continue;
        }
        while p > 0 && is_whitespace(data[p - 1]) {
            p -= 1;
        }
        let num_end = p;
        while p > 0 && data[p - 1].is_ascii_digit() {
            p -= 1;
        }
        let num_start = p;
        if num_start == num_end || (p > 0 && is_regular(data[p - 1])) {
            continue;
        }
        let num: u32 = match std::str::from_utf8(&data[num_start..num_end])
            .ok()
            .and_then(|s| s.parse().ok())
        {
            Some(n) => n,
            None => continue,
        };
        let gen: u16 = std::str::from_utf8(&data[gen_start..gen_end])
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .map(|g| g.min(65535) as u16)
            .unwrap_or(0);
        xref.entries.insert(
            num,
            XrefEntry::Offset {
                offset: num_start,
                gen,
            },
        );
    }
    // Trailer dictionaries: take keys from every trailer, later ones winning.
    let mut t = 0usize;
    while let Some(idx) = find_bytes(data, t, b"trailer") {
        t = idx + 7;
        let mut p = Parser::new(data, t);
        if let Ok(Object::Dict(d)) = p.parse_object(None) {
            for (k, v) in d.0 {
                xref.trailer.0.insert(k, v);
            }
        }
    }
    // Cross-reference streams carry trailer keys too.
    if !xref.trailer.contains("Root") {
        let offsets: Vec<usize> = xref
            .entries
            .values()
            .filter_map(|e| match e {
                XrefEntry::Offset { offset, .. } => Some(*offset),
                _ => None,
            })
            .collect();
        for off in offsets {
            let mut p = Parser::new(data, off);
            if let Ok((_, Object::Stream(s))) = p.parse_indirect(None) {
                if s.dict
                    .get("Type")
                    .and_then(Object::as_name)
                    .map(|n| n == "XRef")
                    .unwrap_or(false)
                {
                    for (k, v) in s.dict.0.iter() {
                        if k == "Root" || k == "Info" || k == "ID" || k == "Encrypt" {
                            xref.trailer.0.entry(k.clone()).or_insert_with(|| v.clone());
                        }
                    }
                }
            }
        }
    }
    if xref.entries.is_empty() {
        return Err(Error::malformed("no objects found"));
    }
    Ok(xref)
}

/// Parses the objects contained in an object stream (`/Type /ObjStm`).
/// `content` must already be decoded. Returns `(object number, object)`
/// pairs in stream order.
pub fn parse_object_stream(dict: &Dict, content: &[u8]) -> Result<Vec<(u32, Object)>> {
    let n = dict.get("N").and_then(Object::as_i64).unwrap_or(0).max(0) as usize;
    let first = dict
        .get("First")
        .and_then(Object::as_i64)
        .unwrap_or(0)
        .max(0) as usize;
    let mut lex = Lexer::new(content);
    let mut offsets = Vec::with_capacity(n);
    for _ in 0..n {
        let num = match lex.next_token()? {
            Token::Integer(v) if v >= 0 => v as u32,
            _ => break,
        };
        let off = match lex.next_token()? {
            Token::Integer(v) if v >= 0 => v as usize,
            _ => break,
        };
        offsets.push((num, off));
    }
    let mut out = Vec::with_capacity(offsets.len());
    for (num, off) in offsets {
        let pos = first.saturating_add(off);
        if pos >= content.len() {
            continue;
        }
        let mut p = Parser::new(content, pos);
        match p.parse_object(None) {
            Ok(o) => out.push((num, o)),
            Err(_) => out.push((num, Object::Null)),
        }
    }
    Ok(out)
}

/// Returns the `%PDF-x.y` header version if present.
pub fn header_version(data: &[u8]) -> Option<(u8, u8)> {
    let limit = data.len().min(1024);
    let idx = find_bytes(&data[..limit], 0, b"%PDF-")?;
    let major = data.get(idx + 5)?.checked_sub(b'0')?;
    let minor = data.get(idx + 7)?.checked_sub(b'0')?;
    if data.get(idx + 6) != Some(&b'.') || major > 9 || minor > 9 {
        return None;
    }
    Some((major, minor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reference_and_dict() {
        let src = b"<< /A 1 0 R /B [1 2 R 3] /C (x) >>";
        let mut p = Parser::new(src, 0);
        let d = p.parse_object(None).unwrap().into_dict().unwrap();
        assert_eq!(d.get("A").unwrap().as_reference(), Some(ObjRef::new(1, 0)));
        let b = d.get("B").unwrap().as_array().unwrap();
        assert_eq!(b.len(), 2);
        assert_eq!(b[0].as_reference(), Some(ObjRef::new(1, 2)));
        assert_eq!(b[1].as_i64(), Some(3));
    }

    #[test]
    fn parse_stream_with_bad_length() {
        let src = b"5 0 obj\n<< /Length 999 >>\nstream\nhello\nendstream\nendobj";
        let mut p = Parser::new(src, 0);
        let (r, o) = p.parse_indirect(None).unwrap();
        assert_eq!(r, ObjRef::new(5, 0));
        assert_eq!(o.as_stream().unwrap().data, b"hello");
    }

    #[test]
    fn parse_stream_with_good_length() {
        let src = b"5 0 obj\n<< /Length 5 >>\nstream\r\nhello\r\nendstream\nendobj";
        let mut p = Parser::new(src, 0);
        let (_, o) = p.parse_indirect(None).unwrap();
        assert_eq!(o.as_stream().unwrap().data, b"hello");
    }

    #[test]
    fn header() {
        assert_eq!(header_version(b"%PDF-1.7\n%..."), Some((1, 7)));
        assert_eq!(header_version(b"junk\n%PDF-2.0"), Some((2, 0)));
        assert_eq!(header_version(b"nope"), None);
    }

    #[test]
    fn reconstruct_finds_objects() {
        let src = b"%PDF-1.4\n1 0 obj << /Type /Catalog >> endobj\n2 0 obj\n<< >>\nendobj\ntrailer << /Root 1 0 R >>";
        let x = reconstruct(src).unwrap();
        assert_eq!(x.entries.len(), 2);
        assert!(matches!(
            x.entries[&1],
            XrefEntry::Offset { offset: 9, gen: 0 }
        ));
        assert_eq!(
            x.trailer.get("Root").unwrap().as_reference(),
            Some(ObjRef::new(1, 0))
        );
    }

    #[test]
    fn deep_nesting_is_rejected() {
        let src = "[".repeat(300);
        let mut p = Parser::new(src.as_bytes(), 0);
        assert!(matches!(p.parse_object(None), Err(Error::Limit(_))));
    }
}
