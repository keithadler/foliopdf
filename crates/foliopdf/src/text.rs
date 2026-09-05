//! Text engine: runs a page's content stream to find every glyph with its
//! position and Unicode value, assembles words and lines, and searches.
//!
//! This is the foundation for text extraction, find, and true redaction
//! (see [`crate::redact`]). It understands simple fonts (Type1, TrueType,
//! Type3) with their encodings and `Differences`, composite (Type0) fonts
//! with Identity or embedded CMaps, `ToUnicode` maps, form XObjects, and
//! the full text state (spacing, scaling, rise, leading).

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use crate::cstream::{self, Op};
use crate::document::Document;
use crate::error::Result;
use crate::font::StandardFont;
use crate::geometry::{Matrix, Point, Rect};
use crate::glyphlist;
use crate::lexer::{Lexer, Token};
use crate::object::{Dict, ObjRef, Object, PdfString};

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Which content stream a glyph or image came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamId {
    /// The page's own content.
    Page,
    /// A form XObject (possibly nested), by object reference.
    Form(ObjRef),
}

/// Where a glyph's bytes sit in the content stream, for rewriting.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlyphLoc {
    /// Which stream.
    pub stream: StreamId,
    /// Index of the text-showing operator in that stream's [`Op`] list.
    pub op: usize,
    /// For `TJ`: index of the string in the array; 0 otherwise.
    pub elem: usize,
    /// Byte range of the glyph's code inside that string.
    pub start: usize,
    /// End of the byte range (exclusive).
    pub end: usize,
    /// `TJ` adjustment that reproduces this glyph's advance when it is removed.
    pub adjust: f64,
}

/// One positioned glyph.
#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    /// Unicode text (may be empty when the font has no mapping, or several
    /// characters for ligatures).
    pub text: String,
    /// Bounding box in user space.
    pub rect: Rect,
    /// Start of the baseline in user space.
    pub origin: Point,
    /// Unit vector along the writing direction in user space.
    pub dir: Point,
    /// Effective font size in user-space units.
    pub size: f64,
    /// Width of a space in this font, user-space units.
    pub space_width: f64,
    /// Whether this is a space character.
    pub is_space: bool,
    /// Location in the content stream.
    pub loc: GlyphLoc,
}

/// A placed image (XObject or inline).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageUse {
    /// Bounding box in user space.
    pub rect: Rect,
    /// Matrix mapping the unit square to user space.
    pub ctm: Matrix,
    /// Stream and operator index of the `Do`/`BI`.
    pub stream: StreamId,
    /// Operator index.
    pub op: usize,
    /// The image XObject, or `None` for inline images.
    pub xobject: Option<ObjRef>,
    /// Resource name used in the `Do` operator.
    pub name: Option<String>,
    /// Clip rectangle in effect (user space), if any.
    pub clip: Option<Rect>,
}

/// A painted path.
#[derive(Debug, Clone, PartialEq)]
pub struct PathUse {
    /// Bounding box in user space (stroked paths are not widened).
    pub rect: Rect,
    /// Stream.
    pub stream: StreamId,
    /// Index of the first construction operator.
    pub first_op: usize,
    /// Index of the painting operator.
    pub paint_op: usize,
}

/// A form XObject use.
#[derive(Debug, Clone, PartialEq)]
pub struct FormUse {
    /// The form object.
    pub xobject: ObjRef,
    /// Bounding box of its `BBox` in user space.
    pub rect: Rect,
    /// Stream and operator of the `Do`.
    pub stream: StreamId,
    /// Operator index.
    pub op: usize,
    /// Resource name.
    pub name: String,
    /// Matrix in effect inside the form (form space → user space).
    pub ctm: Matrix,
}

/// Everything found on a page.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageContent {
    /// Glyphs in content order.
    pub glyphs: Vec<Glyph>,
    /// Images.
    pub images: Vec<ImageUse>,
    /// Painted paths.
    pub paths: Vec<PathUse>,
    /// Form XObjects used (including nested ones).
    pub forms: Vec<FormUse>,
    /// Fonts that produced no Unicode at all (text cannot be extracted from them).
    pub unmapped_fonts: Vec<String>,
}

/// A run of text on one line (a word or a whole line), in user space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextSpan {
    /// The text.
    pub text: String,
    /// Bounding box.
    pub rect: Rect,
    /// Line number on the page (0-based, top to bottom).
    pub line: usize,
}

/// A search hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Match {
    /// The matched text as it appears on the page.
    pub text: String,
    /// One rectangle per line the match spans, in user space.
    pub rects: Vec<Rect>,
    /// Line number of the first rectangle.
    pub line: usize,
}

/// Search options.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SearchOptions {
    /// Match regardless of case. Default false.
    pub case_insensitive: bool,
    /// Only match whole words. Default false.
    pub whole_word: bool,
}

// ---------------------------------------------------------------------------
// Fonts
// ---------------------------------------------------------------------------

/// A code range from a CMap's `codespacerange`.
#[derive(Debug, Clone)]
struct CodeRange {
    bytes: usize,
    lo: u32,
    hi: u32,
}

/// Decoded font: how to split bytes into codes, widths and Unicode.
pub(crate) struct LoadedFont {
    name: String,
    ranges: Vec<CodeRange>,
    /// code → CID (composite fonts); identity when empty.
    cid_map: HashMap<u32, u32>,
    cid_ranges: Vec<(u32, u32, u32)>,
    /// CID (or code for simple fonts) → advance, text space (already /1000).
    widths: HashMap<u32, f64>,
    default_width: f64,
    /// code → Unicode.
    unicode: HashMap<u32, String>,
    /// Simple-font fallback: code → char via encoding.
    encoding: Option<[Option<char>; 256]>,
    standard: Option<StandardFont>,
    type3: Option<Matrix>,
    ascent: f64,
    descent: f64,
    composite: bool,
    any_unicode: bool,
}

impl LoadedFont {
    fn split(&self, bytes: &[u8]) -> Vec<(u32, usize, usize)> {
        let mut out = Vec::with_capacity(bytes.len());
        let mut i = 0;
        while i < bytes.len() {
            let mut taken = None;
            // Try 1..4 byte codes against the code space ranges.
            for n in 1..=4 {
                if i + n > bytes.len() {
                    break;
                }
                let mut code = 0u32;
                for b in &bytes[i..i + n] {
                    code = (code << 8) | *b as u32;
                }
                if self
                    .ranges
                    .iter()
                    .any(|r| r.bytes == n && code >= r.lo && code <= r.hi)
                {
                    taken = Some((code, n));
                    break;
                }
            }
            let (code, n) = taken.unwrap_or_else(|| {
                // No range matched: use the shortest range length (spec: partial match rules), default 1.
                let n = self
                    .ranges
                    .iter()
                    .map(|r| r.bytes)
                    .min()
                    .unwrap_or(1)
                    .min(bytes.len() - i);
                let mut code = 0u32;
                for b in &bytes[i..i + n] {
                    code = (code << 8) | *b as u32;
                }
                (code, n)
            });
            out.push((code, i, i + n));
            i += n;
        }
        out
    }
    fn cid(&self, code: u32) -> u32 {
        if !self.composite {
            return code;
        }
        if let Some(c) = self.cid_map.get(&code) {
            return *c;
        }
        for (lo, hi, cid) in &self.cid_ranges {
            if code >= *lo && code <= *hi {
                return cid + (code - lo);
            }
        }
        code
    }
    /// Advance in text space units (glyph width / 1000, or through the Type3 matrix).
    fn width(&self, code: u32) -> f64 {
        let key = self.cid(code);
        if let Some(w) = self.widths.get(&key) {
            return *w;
        }
        if let Some(sf) = self.standard {
            if code < 256 {
                return sf.width(code as u8) as f64 / 1000.0;
            }
        }
        self.default_width
    }
    fn text(&self, code: u32) -> Option<String> {
        if let Some(s) = self.unicode.get(&code) {
            return Some(s.clone());
        }
        if let Some(enc) = &self.encoding {
            if code < 256 {
                return enc[code as usize].map(|c| c.to_string());
            }
        }
        None
    }
}

#[allow(clippy::while_let_loop)]
fn parse_cmap(
    data: &[u8],
    ranges: &mut Vec<CodeRange>,
    cid_map: &mut HashMap<u32, u32>,
    cid_ranges: &mut Vec<(u32, u32, u32)>,
    unicode: &mut HashMap<u32, String>,
) {
    let mut lex = Lexer::new(data);
    let mut stack: Vec<Token> = Vec::new();
    let mut guard = 0;
    loop {
        guard += 1;
        if guard > 5_000_000 {
            break;
        }
        let t = match lex.next_token() {
            Ok(Token::Eof) => break,
            Ok(t) => t,
            Err(_) => {
                let p = lex.pos();
                lex.seek(p + 1);
                if p + 1 >= data.len() {
                    break;
                }
                continue;
            }
        };
        match t {
            Token::Keyword(k) => {
                match k.as_slice() {
                    b"begincodespacerange" => loop {
                        let lo = match lex.next_token() {
                            Ok(Token::String(s)) => s,
                            _ => break,
                        };
                        let hi = match lex.next_token() {
                            Ok(Token::String(s)) => s,
                            _ => break,
                        };
                        let n = lo.as_bytes().len().clamp(1, 4);
                        ranges.push(CodeRange {
                            bytes: n,
                            lo: be(lo.as_bytes()),
                            hi: be(hi.as_bytes()),
                        });
                    },
                    b"begincidrange" => loop {
                        let lo = match lex.next_token() {
                            Ok(Token::String(s)) => be(s.as_bytes()),
                            _ => break,
                        };
                        let hi = match lex.next_token() {
                            Ok(Token::String(s)) => be(s.as_bytes()),
                            _ => break,
                        };
                        let cid = match lex.next_token() {
                            Ok(Token::Integer(i)) => i.max(0) as u32,
                            _ => break,
                        };
                        cid_ranges.push((lo, hi, cid));
                    },
                    b"begincidchar" => loop {
                        let code = match lex.next_token() {
                            Ok(Token::String(s)) => be(s.as_bytes()),
                            _ => break,
                        };
                        let cid = match lex.next_token() {
                            Ok(Token::Integer(i)) => i.max(0) as u32,
                            _ => break,
                        };
                        cid_map.insert(code, cid);
                    },
                    b"beginbfchar" => loop {
                        let src = match lex.next_token() {
                            Ok(Token::String(s)) => s,
                            _ => break,
                        };
                        match lex.next_token() {
                            Ok(Token::String(dst)) => {
                                unicode.insert(be(src.as_bytes()), utf16(dst.as_bytes()));
                            }
                            Ok(Token::Name(n)) => {
                                if let Some(c) = glyphlist::glyph_to_char(&n.as_str()) {
                                    unicode.insert(be(src.as_bytes()), c.to_string());
                                }
                            }
                            _ => break,
                        }
                    },
                    b"beginbfrange" => loop {
                        let lo = match lex.next_token() {
                            Ok(Token::String(s)) => be(s.as_bytes()),
                            _ => break,
                        };
                        let hi = match lex.next_token() {
                            Ok(Token::String(s)) => be(s.as_bytes()),
                            _ => break,
                        };
                        if hi < lo || hi - lo > 65535 {
                            // Still consume the destination.
                            let _ = lex.next_token();
                            continue;
                        }
                        match lex.next_token() {
                            Ok(Token::String(dst)) => {
                                let base = dst.as_bytes().to_vec();
                                for (k, code) in (lo..=hi).enumerate() {
                                    let mut d = base.clone();
                                    // Increment the last UTF-16 unit.
                                    if d.len() >= 2 {
                                        let n = d.len();
                                        let last = u16::from_be_bytes([d[n - 2], d[n - 1]])
                                            .wrapping_add(k as u16);
                                        d[n - 2..].copy_from_slice(&last.to_be_bytes());
                                    } else if let Some(b) = d.last_mut() {
                                        *b = b.wrapping_add(k as u8);
                                    }
                                    unicode.insert(code, utf16(&d));
                                }
                            }
                            Ok(Token::ArrayOpen) => {
                                let mut code = lo;
                                loop {
                                    match lex.next_token() {
                                        Ok(Token::String(dst)) => {
                                            if code <= hi {
                                                unicode.insert(code, utf16(dst.as_bytes()));
                                            }
                                            code += 1;
                                        }
                                        _ => break,
                                    }
                                }
                            }
                            _ => break,
                        }
                    },
                    _ => {}
                }
                stack.clear();
            }
            other => {
                stack.push(other);
                if stack.len() > 32 {
                    stack.remove(0);
                }
            }
        }
    }
}

fn be(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .take(4)
        .fold(0u32, |acc, b| (acc << 8) | *b as u32)
}

fn utf16(bytes: &[u8]) -> String {
    if bytes.len() % 2 == 1 {
        // Odd length: treat as single bytes (some producers write 1-byte dsts).
        return bytes.iter().map(|&b| b as char).collect();
    }
    let units: Vec<u16> = bytes
        .chunks(2)
        .map(|c| u16::from_be_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
}

fn load_font(doc: &Document, fd: &Dict, name: &str) -> LoadedFont {
    let get = |d: &Dict, k: &str| doc.dict_get(d, k).cloned();
    let subtype = get(fd, "Subtype")
        .and_then(|o| o.as_name().map(|n| n.as_str().into_owned()))
        .unwrap_or_default();
    let base = get(fd, "BaseFont")
        .and_then(|o| o.as_name().map(|n| n.as_str().into_owned()))
        .unwrap_or_default();
    let mut f = LoadedFont {
        name: if base.is_empty() {
            name.to_string()
        } else {
            base.clone()
        },
        ranges: vec![CodeRange {
            bytes: 1,
            lo: 0,
            hi: 255,
        }],
        cid_map: HashMap::new(),
        cid_ranges: Vec::new(),
        widths: HashMap::new(),
        default_width: 0.0,
        unicode: HashMap::new(),
        encoding: None,
        standard: None,
        type3: None,
        ascent: 0.9,
        descent: -0.22,
        composite: false,
        any_unicode: false,
    };
    // ToUnicode applies to every font type.
    if let Some(Object::Stream(s)) = get(fd, "ToUnicode") {
        if let Ok(data) = doc.stream_data(&s) {
            let mut r = Vec::new();
            let (mut cm, mut cr) = (HashMap::new(), Vec::new());
            parse_cmap(&data, &mut r, &mut cm, &mut cr, &mut f.unicode);
        }
    }
    let descriptor =
        |d: &Dict| -> Option<Dict> { get(d, "FontDescriptor").and_then(Object::into_dict) };
    let apply_descriptor = |f: &mut LoadedFont, desc: &Dict| {
        if let Some(a) = get(desc, "Ascent").and_then(|o| o.as_f64()) {
            if a.abs() > 1.0 && a.abs() < 5000.0 {
                f.ascent = a.abs() / 1000.0;
            }
        }
        if let Some(d) = get(desc, "Descent").and_then(|o| o.as_f64()) {
            if d.abs() > 1.0 && d.abs() < 5000.0 {
                f.descent = -d.abs() / 1000.0;
            }
        }
        if let Some(mw) = get(desc, "MissingWidth").and_then(|o| o.as_f64()) {
            f.default_width = mw / 1000.0;
        }
    };
    if subtype == "Type0" {
        f.composite = true;
        f.default_width = 1.0;
        // Encoding CMap.
        match get(fd, "Encoding") {
            Some(Object::Name(n)) => {
                let n = n.as_str();
                if n.starts_with("Identity") {
                    f.ranges = vec![CodeRange {
                        bytes: 2,
                        lo: 0,
                        hi: 0xFFFF,
                    }];
                } else {
                    // Predefined CMaps are not bundled; assume 2-byte codes.
                    f.ranges = vec![CodeRange {
                        bytes: 2,
                        lo: 0,
                        hi: 0xFFFF,
                    }];
                }
            }
            Some(Object::Stream(s)) => {
                if let Ok(data) = doc.stream_data(&s) {
                    let mut ranges = Vec::new();
                    let mut uni = HashMap::new();
                    parse_cmap(
                        &data,
                        &mut ranges,
                        &mut f.cid_map,
                        &mut f.cid_ranges,
                        &mut uni,
                    );
                    f.ranges = if ranges.is_empty() {
                        vec![CodeRange {
                            bytes: 2,
                            lo: 0,
                            hi: 0xFFFF,
                        }]
                    } else {
                        ranges
                    };
                    if let Some(Object::Stream(us)) = get(&s.dict, "UseCMap") {
                        if let Ok(d2) = doc.stream_data(&us) {
                            let mut r2 = Vec::new();
                            parse_cmap(&d2, &mut r2, &mut f.cid_map, &mut f.cid_ranges, &mut uni);
                        }
                    }
                }
            }
            _ => {
                f.ranges = vec![CodeRange {
                    bytes: 2,
                    lo: 0,
                    hi: 0xFFFF,
                }];
            }
        }
        // Descendant font: widths and descriptor.
        let desc_font = get(fd, "DescendantFonts")
            .and_then(|o| o.into_array())
            .and_then(|a| a.first().cloned())
            .map(|o| doc.resolve(&o).clone())
            .and_then(Object::into_dict);
        if let Some(df) = desc_font {
            if let Some(dw) = get(&df, "DW").and_then(|o| o.as_f64()) {
                f.default_width = dw / 1000.0;
            }
            if let Some(Object::Array(w)) = get(&df, "W") {
                let w: Vec<Object> = w.iter().map(|o| doc.resolve(o).clone()).collect();
                let mut i = 0;
                while i < w.len() {
                    let first = match w[i].as_f64() {
                        Some(v) => v as u32,
                        None => break,
                    };
                    match w.get(i + 1) {
                        Some(Object::Array(list)) => {
                            for (k, wv) in list.iter().enumerate() {
                                if let Some(v) = doc.resolve(wv).as_f64() {
                                    f.widths.insert(first + k as u32, v / 1000.0);
                                }
                            }
                            i += 2;
                        }
                        Some(o) => {
                            let last = o.as_f64().unwrap_or(first as f64) as u32;
                            let v =
                                w.get(i + 2).and_then(Object::as_f64).unwrap_or(1000.0) / 1000.0;
                            if last >= first && last - first < 65536 {
                                for c in first..=last {
                                    f.widths.insert(c, v);
                                }
                            }
                            i += 3;
                        }
                        None => break,
                    }
                }
            }
            if let Some(desc) = descriptor(&df) {
                apply_descriptor(&mut f, &desc);
            }
        }
        f.any_unicode = !f.unicode.is_empty();
        return f;
    }

    // Simple fonts.
    let flags = descriptor(fd)
        .and_then(|d| get(&d, "Flags"))
        .and_then(|o| o.as_i64())
        .unwrap_or(0);
    let symbolic = flags & 4 != 0 && flags & 32 == 0;
    if let Some(desc) = descriptor(fd) {
        apply_descriptor(&mut f, &desc);
    }
    f.standard = StandardFont::by_name(&base).or_else(|| {
        let b = base.to_ascii_lowercase();
        if b.contains("arial")
            || b.contains("helvetica")
            || b.contains("verdana")
            || b.contains("calibri")
        {
            StandardFont::by_name(if b.contains("bold") {
                "Helvetica-Bold"
            } else {
                "Helvetica"
            })
        } else if b.contains("times") || b.contains("georgia") || b.contains("garamond") {
            StandardFont::by_name(if b.contains("bold") {
                "Times-Bold"
            } else {
                "Times-Roman"
            })
        } else if b.contains("courier") || b.contains("mono") {
            Some(StandardFont::Courier)
        } else {
            None
        }
    });
    if subtype == "Type3" {
        f.type3 = Some(
            get(fd, "FontMatrix")
                .and_then(|o| Matrix::from_object(&o))
                .unwrap_or(Matrix::new(0.001, 0.0, 0.0, 0.001, 0.0, 0.0)),
        );
        f.standard = None;
        if let Some(Object::Array(bbox)) = get(fd, "FontBBox") {
            let v: Vec<f64> = bbox.iter().filter_map(Object::as_f64).collect();
            if v.len() == 4 {
                let m = f.type3.unwrap();
                let top = m.apply(Point::new(0.0, v[3])).y;
                let bottom = m.apply(Point::new(0.0, v[1])).y;
                if top > 0.0 {
                    f.ascent = top;
                }
                if bottom < 0.0 {
                    f.descent = bottom;
                }
            }
        }
    }
    // Widths.
    let first = get(fd, "FirstChar")
        .and_then(|o| o.as_i64())
        .unwrap_or(0)
        .max(0) as u32;
    if let Some(Object::Array(w)) = get(fd, "Widths") {
        for (k, wv) in w.iter().enumerate() {
            if let Some(v) = doc.resolve(wv).as_f64() {
                let width = match f.type3 {
                    Some(m) => m.apply(Point::new(v, 0.0)).x - m.apply(Point::new(0.0, 0.0)).x,
                    None => v / 1000.0,
                };
                f.widths.insert(first + k as u32, width);
            }
        }
        if !w.is_empty() {
            // Widths present: unlisted codes fall back to MissingWidth, not to the standard metrics.
            if f.standard.is_some() && f.default_width == 0.0 {
                // keep standard as a fallback only for codes outside the array
            }
        }
    }
    // Encoding.
    let mut table: [Option<char>; 256] = [None; 256];
    let base_is_symbol = base.to_ascii_lowercase().contains("symbol");
    let base_is_dingbat = base.to_ascii_lowercase().contains("dingbat");
    let builtin: fn(u8) -> Option<char> = if base_is_symbol {
        glyphlist::symbol_encoding
    } else if base_is_dingbat {
        |_| None
    } else if symbolic && subtype == "TrueType" {
        // Symbolic TrueType: codes are usually still ASCII-like.
        glyphlist::standard_encoding
    } else if f.standard.is_some() || subtype == "TrueType" {
        if subtype == "TrueType" {
            glyphlist::winansi_encoding
        } else {
            glyphlist::standard_encoding
        }
    } else {
        glyphlist::standard_encoding
    };
    for c in 0..=255u8 {
        table[c as usize] = builtin(c);
    }
    let apply_base = |table: &mut [Option<char>; 256], name: &str| {
        let f: Option<fn(u8) -> Option<char>> = match name {
            "WinAnsiEncoding" => Some(glyphlist::winansi_encoding),
            "MacRomanEncoding" => Some(glyphlist::macroman_encoding),
            "StandardEncoding" | "MacExpertEncoding" => Some(glyphlist::standard_encoding),
            _ => None,
        };
        if let Some(f) = f {
            for c in 0..=255u8 {
                table[c as usize] = f(c);
            }
        }
    };
    match get(fd, "Encoding") {
        Some(Object::Name(n)) => apply_base(&mut table, &n.as_str()),
        Some(Object::Dict(ed)) => {
            if let Some(Object::Name(n)) = get(&ed, "BaseEncoding") {
                apply_base(&mut table, &n.as_str());
            }
            if let Some(Object::Array(diff)) = get(&ed, "Differences") {
                let mut code: i64 = 0;
                for item in &diff {
                    match doc.resolve(item) {
                        Object::Integer(i) => code = *i,
                        Object::Real(r) => code = *r as i64,
                        Object::Name(n) => {
                            if (0..256).contains(&code) {
                                table[code as usize] = glyphlist::glyph_to_char(&n.as_str());
                            }
                            code += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
        _ => {}
    }
    f.encoding = Some(table);
    f.any_unicode = !f.unicode.is_empty() || table.iter().any(|c| c.is_some());
    f
}

// ---------------------------------------------------------------------------
// Interpreter
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct GState {
    ctm: Matrix,
    clip: Option<Rect>,
    font: Option<Rc<LoadedFont>>,
    size: f64,
    tc: f64,
    tw: f64,
    th: f64,
    tl: f64,
    rise: f64,
    mode: i64,
}

impl Default for GState {
    fn default() -> Self {
        Self {
            ctm: Matrix::IDENTITY,
            clip: None,
            font: None,
            size: 0.0,
            tc: 0.0,
            tw: 0.0,
            th: 1.0,
            tl: 0.0,
            rise: 0.0,
            mode: 0,
        }
    }
}

struct Interp<'a> {
    doc: &'a Document,
    fonts: HashMap<String, Rc<LoadedFont>>,
    out: PageContent,
    depth: usize,
    active_forms: HashSet<u32>,
    unmapped: HashSet<String>,
    ops_budget: usize,
}

impl<'a> Interp<'a> {
    fn font_for(&mut self, resources: &Dict, name: &str) -> Option<Rc<LoadedFont>> {
        let fonts = self
            .doc
            .dict_get(resources, "Font")
            .and_then(Object::as_dict)?;
        let entry = fonts.get(name)?;
        let key = match entry {
            Object::Reference(r) => format!("R{}", r.num),
            _ => format!("{}:{name}", resources as *const Dict as usize),
        };
        if let Some(f) = self.fonts.get(&key) {
            return Some(f.clone());
        }
        let fd = self.doc.resolve(entry).as_dict()?.clone();
        let lf = Rc::new(load_font(self.doc, &fd, name));
        if !lf.any_unicode {
            self.unmapped.insert(lf.name.clone());
        }
        self.fonts.insert(key, lf.clone());
        Some(lf)
    }

    fn run(&mut self, ops: &[Op], resources: &Dict, base: GState, stream: StreamId) {
        let mut gs = base;
        let mut stack: Vec<GState> = Vec::new();
        let mut tm = Matrix::IDENTITY;
        let mut tlm = Matrix::IDENTITY;
        // Path construction.
        let mut path_pts: Vec<Point> = Vec::new();
        let mut path_start: Option<usize> = None;
        let mut pending_clip = false;
        for (i, op) in ops.iter().enumerate() {
            if self.ops_budget == 0 {
                return;
            }
            self.ops_budget -= 1;
            let n = |k: usize| op.num(k).unwrap_or(0.0);
            match op.name.as_str() {
                "q" => {
                    stack.push(gs.clone());
                    if stack.len() > 256 {
                        stack.remove(0);
                    }
                }
                "Q" => {
                    if let Some(g) = stack.pop() {
                        gs = g;
                    }
                }
                "cm" if op.operands.len() >= 6 => {
                    gs.ctm = Matrix::new(n(0), n(1), n(2), n(3), n(4), n(5)).then(&gs.ctm);
                }
                "gs" => {
                    if let Some(name) = op.name_at(0) {
                        if let Some(eg) = self
                            .doc
                            .dict_get(resources, "ExtGState")
                            .and_then(Object::as_dict)
                            .and_then(|d| self.doc.dict_get(d, &name.as_str()))
                            .and_then(Object::as_dict)
                        {
                            if let Some(Object::Array(fa)) = self.doc.dict_get(eg, "Font") {
                                if let (Some(Object::Reference(r)), Some(sz)) =
                                    (fa.first(), fa.get(1).and_then(Object::as_f64))
                                {
                                    let key = format!("R{}", r.num);
                                    let f = match self.fonts.get(&key) {
                                        Some(f) => Some(f.clone()),
                                        None => self.doc.get(*r).as_dict().map(|fd| {
                                            let lf = Rc::new(load_font(self.doc, fd, "gs"));
                                            self.fonts.insert(key, lf.clone());
                                            lf
                                        }),
                                    };
                                    gs.font = f;
                                    gs.size = sz;
                                }
                            }
                        }
                    }
                }
                "BT" => {
                    tm = Matrix::IDENTITY;
                    tlm = Matrix::IDENTITY;
                }
                "ET" => {}
                "Tf" => {
                    gs.size = n(1);
                    gs.font = op
                        .name_at(0)
                        .and_then(|nm| self.font_for(resources, &nm.as_str()));
                }
                "Tc" => gs.tc = n(0),
                "Tw" => gs.tw = n(0),
                "Tz" => gs.th = n(0) / 100.0,
                "TL" => gs.tl = n(0),
                "Ts" => gs.rise = n(0),
                "Tr" => gs.mode = op.operands.first().and_then(Object::as_i64).unwrap_or(0),
                "Td" => {
                    tlm = Matrix::translate(n(0), n(1)).then(&tlm);
                    tm = tlm;
                }
                "TD" => {
                    gs.tl = -n(1);
                    tlm = Matrix::translate(n(0), n(1)).then(&tlm);
                    tm = tlm;
                }
                "Tm" if op.operands.len() >= 6 => {
                    tlm = Matrix::new(n(0), n(1), n(2), n(3), n(4), n(5));
                    tm = tlm;
                }
                "T*" => {
                    tlm = Matrix::translate(0.0, -gs.tl).then(&tlm);
                    tm = tlm;
                }
                "Tj" | "'" | "\"" => {
                    if op.name != "Tj" {
                        if op.name == "\"" {
                            gs.tw = n(0);
                            gs.tc = n(1);
                        }
                        tlm = Matrix::translate(0.0, -gs.tl).then(&tlm);
                        tm = tlm;
                    }
                    if let Some(Object::String(s)) = op.operands.last() {
                        self.show(&gs, &mut tm, s, stream, i, 0);
                    }
                }
                "TJ" => {
                    if let Some(Object::Array(arr)) = op.operands.first() {
                        for (k, el) in arr.iter().enumerate() {
                            match el {
                                Object::String(s) => self.show(&gs, &mut tm, s, stream, i, k),
                                o => {
                                    if let Some(adj) = o.as_f64() {
                                        let tx = -adj / 1000.0 * gs.size * gs.th;
                                        tm = Matrix::translate(tx, 0.0).then(&tm);
                                    }
                                }
                            }
                        }
                    }
                }
                "Do" => {
                    if let Some(name) = op.name_at(0) {
                        self.do_xobject(resources, &name.as_str(), &gs, stream, i);
                    }
                }
                "BI" => {
                    let rect = Rect::new(0.0, 0.0, 1.0, 1.0).transform(&gs.ctm);
                    self.out.images.push(ImageUse {
                        rect,
                        ctm: gs.ctm,
                        stream,
                        op: i,
                        xobject: None,
                        name: None,
                        clip: gs.clip,
                    });
                }
                // Paths.
                "m" => {
                    if path_start.is_none() {
                        path_start = Some(i);
                    }
                    path_pts.push(gs.ctm.apply(Point::new(n(0), n(1))));
                }
                "l" => {
                    if path_start.is_none() {
                        path_start = Some(i);
                    }
                    path_pts.push(gs.ctm.apply(Point::new(n(0), n(1))));
                }
                "c" | "v" | "y" => {
                    if path_start.is_none() {
                        path_start = Some(i);
                    }
                    let pts: Vec<f64> = op.operands.iter().filter_map(Object::as_f64).collect();
                    for pair in pts.chunks(2) {
                        if pair.len() == 2 {
                            path_pts.push(gs.ctm.apply(Point::new(pair[0], pair[1])));
                        }
                    }
                }
                "re" => {
                    if path_start.is_none() {
                        path_start = Some(i);
                    }
                    let r = Rect::from_xywh(n(0), n(1), n(2), n(3));
                    for p in [
                        Point::new(r.x0, r.y0),
                        Point::new(r.x1, r.y0),
                        Point::new(r.x1, r.y1),
                        Point::new(r.x0, r.y1),
                    ] {
                        path_pts.push(gs.ctm.apply(p));
                    }
                }
                "h" => {}
                "W" | "W*" => pending_clip = true,
                "n" | "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" => {
                    let bbox = Rect::bounds(path_pts.iter().copied());
                    if pending_clip {
                        if let Some(b) = bbox {
                            gs.clip = Some(match gs.clip {
                                Some(c) => c
                                    .intersection(&b)
                                    .unwrap_or(Rect::new(b.x0, b.y0, b.x0, b.y0)),
                                None => b,
                            });
                        }
                        pending_clip = false;
                    }
                    if op.name != "n" {
                        if let (Some(b), Some(first)) = (bbox, path_start) {
                            self.out.paths.push(PathUse {
                                rect: b,
                                stream,
                                first_op: first,
                                paint_op: i,
                            });
                        }
                    }
                    path_pts.clear();
                    path_start = None;
                }
                "d0" | "d1" => {}
                _ => {}
            }
        }
    }

    fn do_xobject(
        &mut self,
        resources: &Dict,
        name: &str,
        gs: &GState,
        stream: StreamId,
        op: usize,
    ) {
        let xobjs = match self
            .doc
            .dict_get(resources, "XObject")
            .and_then(Object::as_dict)
        {
            Some(x) => x,
            None => return,
        };
        let entry = match xobjs.get(name) {
            Some(e) => e,
            None => return,
        };
        let xref = entry.as_reference();
        let s = match self.doc.resolve(entry).as_stream() {
            Some(s) => s,
            None => return,
        };
        let subtype = self
            .doc
            .dict_get(&s.dict, "Subtype")
            .and_then(Object::as_name)
            .map(|n| n.as_str().into_owned())
            .unwrap_or_default();
        if subtype == "Image" {
            let rect = Rect::new(0.0, 0.0, 1.0, 1.0).transform(&gs.ctm);
            self.out.images.push(ImageUse {
                rect,
                ctm: gs.ctm,
                stream,
                op,
                xobject: xref,
                name: Some(name.to_string()),
                clip: gs.clip,
            });
            return;
        }
        if subtype != "Form" {
            return;
        }
        let xref = match xref {
            Some(r) => r,
            None => return, // direct form streams are not valid PDF; skip
        };
        if self.depth >= 12 || self.active_forms.contains(&xref.num) {
            return;
        }
        let matrix = self
            .doc
            .dict_get(&s.dict, "Matrix")
            .and_then(Matrix::from_object)
            .unwrap_or(Matrix::IDENTITY);
        let ctm = matrix.then(&gs.ctm);
        let bbox = self
            .doc
            .dict_get(&s.dict, "BBox")
            .and_then(Rect::from_object);
        let rect = bbox
            .map(|b| b.transform(&ctm))
            .unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0));
        self.out.forms.push(FormUse {
            xobject: xref,
            rect,
            stream,
            op,
            name: name.to_string(),
            ctm,
        });
        let data = match self.doc.stream_data(s) {
            Ok(d) => d,
            Err(_) => return,
        };
        let res = self
            .doc
            .dict_get(&s.dict, "Resources")
            .and_then(Object::as_dict)
            .cloned()
            .unwrap_or_else(|| resources.clone());
        let mut inner = gs.clone();
        inner.ctm = ctm;
        if let Some(b) = bbox {
            let bb = b.transform(&ctm);
            inner.clip = Some(match gs.clip {
                Some(c) => c
                    .intersection(&bb)
                    .unwrap_or(Rect::new(bb.x0, bb.y0, bb.x0, bb.y0)),
                None => bb,
            });
        }
        let ops = cstream::parse(&data);
        self.depth += 1;
        self.active_forms.insert(xref.num);
        self.run(&ops, &res, inner, StreamId::Form(xref));
        self.active_forms.remove(&xref.num);
        self.depth -= 1;
    }

    fn show(
        &mut self,
        gs: &GState,
        tm: &mut Matrix,
        s: &PdfString,
        stream: StreamId,
        op: usize,
        elem: usize,
    ) {
        let font = match &gs.font {
            Some(f) => f.clone(),
            None => {
                // No font: still advance nothing; nothing to record.
                return;
            }
        };
        let bytes = s.as_bytes();
        let invisible = gs.mode == 3 || gs.mode == 7;
        let fsize = gs.size;
        for (code, start, end) in font.split(bytes) {
            let w0 = font.width(code);
            let is_space = end - start == 1 && code == 32;
            let extra = gs.tc + if is_space { gs.tw } else { 0.0 };
            let advance = w0 * fsize + extra;
            let trm = Matrix::new(fsize * gs.th, 0.0, 0.0, fsize, 0.0, gs.rise)
                .then(tm)
                .then(&gs.ctm);
            let (asc, desc) = (font.ascent, font.descent);
            let glyph_box = Rect::new(0.0, desc, w0.max(0.0), asc);
            let rect = glyph_box.transform(&trm);
            let origin = trm.apply(Point::new(0.0, 0.0));
            let dx = trm.apply(Point::new(1.0, 0.0));
            let len = ((dx.x - origin.x).powi(2) + (dx.y - origin.y).powi(2)).sqrt();
            let dir = if len > 1e-9 {
                Point::new((dx.x - origin.x) / len, (dx.y - origin.y) / len)
            } else {
                Point::new(1.0, 0.0)
            };
            let up = trm.apply(Point::new(0.0, 1.0));
            let size = ((up.x - origin.x).powi(2) + (up.y - origin.y).powi(2)).sqrt();
            // Invisible text (render mode 3/7, typically OCR layers) is kept: it is
            // what search and redaction need to see.
            let _ = invisible;
            let text = font.text(code).unwrap_or_default();
            let adjust = if fsize.abs() > 1e-9 {
                -(w0 * 1000.0 + extra * 1000.0 / fsize)
            } else {
                0.0
            };
            let space_w = font.width(32).max(0.0) * size;
            self.out.glyphs.push(Glyph {
                text,
                rect,
                origin,
                dir,
                size,
                space_width: if space_w > 0.0 { space_w } else { size * 0.25 },
                is_space,
                loc: GlyphLoc {
                    stream,
                    op,
                    elem,
                    start,
                    end,
                    adjust,
                },
            });
            *tm = Matrix::translate(advance * gs.th, 0.0).then(tm);
        }
    }
}

/// Page resources, walking up the page tree.
fn page_resources(doc: &Document, page: ObjRef) -> Dict {
    doc.page_attr(page, "Resources")
        .map(|o| doc.resolve(o))
        .and_then(Object::as_dict)
        .cloned()
        .unwrap_or_default()
}

/// Runs the page's content and returns every glyph, image, path and form use.
pub fn page_content(doc: &Document, page: usize) -> Result<PageContent> {
    let info = doc.page_info(page)?;
    let data = doc.page_content(page)?;
    let ops = cstream::parse(&data);
    let res = page_resources(doc, info.obj);
    let mut it = Interp {
        doc,
        fonts: HashMap::new(),
        out: PageContent::default(),
        depth: 0,
        active_forms: HashSet::new(),
        unmapped: HashSet::new(),
        ops_budget: 5_000_000,
    };
    it.run(&ops, &res, GState::default(), StreamId::Page);
    let mut out = it.out;
    out.unmapped_fonts = it.unmapped.into_iter().collect();
    out.unmapped_fonts.sort();
    Ok(out)
}

/// Parsed operators of the page's own content stream (concatenated).
pub fn page_ops(doc: &Document, page: usize) -> Result<Vec<Op>> {
    Ok(cstream::parse(&doc.page_content(page)?))
}

// ---------------------------------------------------------------------------
// Assembly: glyphs → lines → text
// ---------------------------------------------------------------------------

/// A line of text with a mapping from characters back to glyph indices.
#[derive(Debug, Clone)]
pub struct Line {
    /// The text of the line, words separated by single spaces.
    pub text: String,
    /// Bounding box in user space.
    pub rect: Rect,
    /// For each `char` of `text`: index into the glyph list, or `None` for
    /// inserted spaces.
    pub chars: Vec<Option<usize>>,
    /// Baseline y (or the along-line coordinate for rotated text).
    pub baseline: f64,
    /// Median font size.
    pub size: f64,
}

/// Writing direction bucketed to 5°, so slightly wobbly text still lines up
/// while genuinely different angles stay apart.
fn orientation(g: &Glyph) -> i32 {
    let deg = g.dir.y.atan2(g.dir.x).to_degrees();
    ((deg / 5.0).round() as i32).rem_euclid(72)
}

/// Coordinates in the text's own frame: `u` along the writing direction,
/// `v` perpendicular to it (increasing towards the top of the text).
fn frame(g: &Glyph, o: i32) -> (f64, f64) {
    let a = (o as f64 * 5.0).to_radians();
    let (sn, cs) = a.sin_cos();
    let p = g.origin;
    (p.x * cs + p.y * sn, -p.x * sn + p.y * cs)
}

/// Assembles glyphs into lines in reading order.
pub fn lines(glyphs: &[Glyph]) -> Vec<Line> {
    if glyphs.is_empty() {
        return Vec::new();
    }
    // Group by orientation, then by baseline.
    let mut idx: Vec<usize> = (0..glyphs.len())
        .filter(|&i| glyphs[i].size > 0.0 && glyphs[i].rect.width().is_finite())
        .collect();
    let orient: Vec<i32> = glyphs.iter().map(orientation).collect();
    let fr: Vec<(f64, f64)> = glyphs
        .iter()
        .enumerate()
        .map(|(i, g)| frame(g, orient[i]))
        .collect();
    idx.sort_by(|&a, &b| {
        orient[a]
            .cmp(&orient[b])
            .then_with(|| {
                fr[b]
                    .1
                    .partial_cmp(&fr[a].1)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .then_with(|| {
                fr[a]
                    .0
                    .partial_cmp(&fr[b].0)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    // Cluster into lines: a glyph joins the current line when its baseline is
    // within a fraction of the font size of the line's baseline.
    let mut groups: Vec<Vec<usize>> = Vec::new();
    let mut cur: Vec<usize> = Vec::new();
    let mut cur_v = 0.0;
    let mut cur_size: f64 = 0.0;
    let mut cur_o = 0i32;
    for &i in &idx {
        let (_, v) = fr[i];
        let g = &glyphs[i];
        let tol = (cur_size.max(g.size)) * 0.45;
        if cur.is_empty() || orient[i] != cur_o || (v - cur_v).abs() > tol {
            if !cur.is_empty() {
                groups.push(std::mem::take(&mut cur));
            }
            cur_v = v;
            cur_size = g.size;
            cur_o = orient[i];
        } else {
            // Running average keeps slightly sloped baselines together.
            cur_v = (cur_v * cur.len() as f64 + v) / (cur.len() as f64 + 1.0);
            cur_size = cur_size.max(g.size);
        }
        cur.push(i);
    }
    if !cur.is_empty() {
        groups.push(cur);
    }
    // Within a line: sort along the line and insert spaces at gaps.
    let mut out = Vec::with_capacity(groups.len());
    for mut g in groups {
        g.sort_by(|&a, &b| {
            fr[a]
                .0
                .partial_cmp(&fr[b].0)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let mut text = String::new();
        let mut chars: Vec<Option<usize>> = Vec::new();
        let mut rect: Option<Rect> = None;
        let mut prev_end: Option<f64> = None;
        let mut prev_space = true;
        let mut sizes: Vec<f64> = Vec::new();
        for &i in &g {
            let gl = &glyphs[i];
            let o = orient[i];
            let (u0, u1) = extent(gl, o);
            if let Some(pe) = prev_end {
                let gap = u0 - pe;
                let thresh = (gl.space_width * 0.5).max(gl.size * 0.12);
                if gap > thresh && !prev_space && !gl.is_space && !gl.text.is_empty() {
                    text.push(' ');
                    chars.push(None);
                    prev_space = true;
                }
            }
            if gl.is_space || gl.text.trim().is_empty() && !gl.text.is_empty() {
                if !prev_space {
                    text.push(' ');
                    chars.push(Some(i));
                    prev_space = true;
                }
            } else if !gl.text.is_empty() {
                for c in gl.text.chars() {
                    text.push(c);
                    chars.push(Some(i));
                }
                prev_space = false;
                rect = Some(match rect {
                    Some(r) => r.union(&gl.rect),
                    None => gl.rect,
                });
                sizes.push(gl.size);
            }
            prev_end = Some(prev_end.map(|p| p.max(u1)).unwrap_or(u1));
        }
        while text.ends_with(' ') {
            text.pop();
            chars.pop();
        }
        if text.is_empty() {
            continue;
        }
        sizes.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let size = sizes.get(sizes.len() / 2).copied().unwrap_or(0.0);
        out.push(Line {
            text,
            rect: rect.unwrap_or_default(),
            chars,
            baseline: fr[g[0]].1,
            size,
        });
    }
    out
}

/// Extent of the glyph box along the writing direction.
fn extent(g: &Glyph, o: i32) -> (f64, f64) {
    let a = (o as f64 * 5.0).to_radians();
    let (sn, cs) = a.sin_cos();
    let r = g.rect;
    let us = [
        r.x0 * cs + r.y0 * sn,
        r.x1 * cs + r.y0 * sn,
        r.x0 * cs + r.y1 * sn,
        r.x1 * cs + r.y1 * sn,
    ];
    (
        us.iter().cloned().fold(f64::INFINITY, f64::min),
        us.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
    )
}

/// The page's text: lines top to bottom, a blank line between paragraphs.
pub fn page_text(doc: &Document, page: usize) -> Result<String> {
    let content = page_content(doc, page)?;
    Ok(text_from_lines(&lines(&content.glyphs)))
}

/// Joins lines into a string with paragraph breaks at large gaps.
pub fn text_from_lines(ls: &[Line]) -> String {
    let mut out = String::new();
    let mut prev: Option<&Line> = None;
    for l in ls {
        if let Some(p) = prev {
            let gap = (p.baseline - l.baseline).abs();
            let size = p.size.max(l.size).max(1.0);
            out.push('\n');
            if gap > size * 1.9 {
                out.push('\n');
            }
        }
        out.push_str(&l.text);
        prev = Some(l);
    }
    out
}

/// Words on the page with their bounding boxes (user space).
pub fn page_words(doc: &Document, page: usize) -> Result<Vec<TextSpan>> {
    let content = page_content(doc, page)?;
    let ls = lines(&content.glyphs);
    let mut out = Vec::new();
    for (li, l) in ls.iter().enumerate() {
        let mut word = String::new();
        let mut rect: Option<Rect> = None;
        let flush = |word: &mut String, rect: &mut Option<Rect>, out: &mut Vec<TextSpan>| {
            if !word.is_empty() {
                out.push(TextSpan {
                    text: std::mem::take(word),
                    rect: rect.take().unwrap_or_default(),
                    line: li,
                });
            }
        };
        for (ci, c) in l.text.chars().enumerate() {
            let gi = l.chars.get(ci).copied().flatten();
            if c == ' ' {
                flush(&mut word, &mut rect, &mut out);
                continue;
            }
            word.push(c);
            if let Some(gi) = gi {
                let r = content.glyphs[gi].rect;
                rect = Some(match rect {
                    Some(x) => x.union(&r),
                    None => r,
                });
            }
        }
        flush(&mut word, &mut rect, &mut out);
    }
    Ok(out)
}

fn fold(c: char, ci: bool) -> char {
    let c = match c {
        '\u{A0}' => ' ',
        '‘' | '’' | '‚' => '\'',
        '“' | '”' | '„' => '"',
        '–' | '—' | '‒' | '−' => '-',
        'ﬁ' => 'f', // handled below (expanded)
        c => c,
    };
    if ci {
        c.to_lowercase().next().unwrap_or(c)
    } else {
        c
    }
}

/// Expands ligatures so "fi" matches "ﬁ".
fn expand(text: &str) -> Vec<(char, usize)> {
    let mut out = Vec::new();
    for (i, c) in text.chars().enumerate() {
        match c {
            'ﬁ' => {
                out.push(('f', i));
                out.push(('i', i));
            }
            'ﬂ' => {
                out.push(('f', i));
                out.push(('l', i));
            }
            'ﬀ' => {
                out.push(('f', i));
                out.push(('f', i));
            }
            'ﬃ' => {
                out.push(('f', i));
                out.push(('f', i));
                out.push(('i', i));
            }
            'ﬄ' => {
                out.push(('f', i));
                out.push(('f', i));
                out.push(('l', i));
            }
            c => out.push((c, i)),
        }
    }
    out
}

/// Finds `needle` on the page. Matches may span line breaks (a break counts
/// as a space). Rectangles are in user space, one per line.
pub fn search(
    doc: &Document,
    page: usize,
    needle: &str,
    opts: &SearchOptions,
) -> Result<Vec<Match>> {
    let content = page_content(doc, page)?;
    Ok(search_content(&content, needle, opts))
}

/// [`search`] over already-extracted content.
pub fn search_content(content: &PageContent, needle: &str, opts: &SearchOptions) -> Vec<Match> {
    let needle: Vec<char> = {
        let n = needle.split_whitespace().collect::<Vec<_>>().join(" ");
        expand(&n)
            .into_iter()
            .map(|(c, _)| fold(c, opts.case_insensitive))
            .collect()
    };
    if needle.is_empty() {
        return Vec::new();
    }
    let ls = lines(&content.glyphs);
    // Flatten lines into one char sequence with (line, char index) back-references.
    let mut hay: Vec<char> = Vec::new();
    let mut back: Vec<(usize, usize)> = Vec::new(); // (line, char index in line)
    for (li, l) in ls.iter().enumerate() {
        if li > 0 {
            hay.push(' ');
            back.push((li, usize::MAX));
        }
        for (c, ci) in expand(&l.text) {
            hay.push(fold(c, opts.case_insensitive));
            back.push((li, ci));
        }
    }
    let mut out = Vec::new();
    let n = needle.len();
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let mut i = 0;
    while i + n <= hay.len() {
        if hay[i..i + n] == needle[..] {
            let ok_word = !opts.whole_word
                || ((i == 0 || !is_word(hay[i - 1]))
                    && (i + n == hay.len() || !is_word(hay[i + n])));
            if ok_word {
                // Collect glyph rects per line.
                let mut rects: Vec<(usize, Rect)> = Vec::new();
                let mut text = String::new();
                for k in i..i + n {
                    let (li, ci) = back[k];
                    text.push(hay[k]);
                    if ci == usize::MAX {
                        continue;
                    }
                    if let Some(Some(gi)) = ls[li].chars.get(ci) {
                        let r = content.glyphs[*gi].rect;
                        match rects.last_mut() {
                            Some((l, rr)) if *l == li => *rr = rr.union(&r),
                            _ => rects.push((li, r)),
                        }
                    }
                }
                if !rects.is_empty() {
                    let line = rects[0].0;
                    out.push(Match {
                        text: text.trim().to_string(),
                        rects: rects.into_iter().map(|(_, r)| r).collect(),
                        line,
                    });
                }
                i += n.max(1);
                continue;
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{self, TextStamp};
    use crate::page::PageSize;

    fn stamped(texts: &[(&str, f64, f64, f64)]) -> Document {
        let mut d = Document::new();
        d.add_page(PageSize::LETTER);
        for (t, x, y, size) in texts {
            let f = d.add_standard_font(StandardFont::Helvetica);
            let name = d.add_page_resource(0, "Font", f).unwrap();
            let enc = d.font_mut(f).unwrap().encode(t);
            let mut cb = crate::content::ContentBuilder::new();
            cb.begin_text()
                .font(&name, *size)
                .text_matrix(&Matrix::translate(*x, *y))
                .show_literal(&enc)
                .end_text();
            d.draw(0, &cb.finish()).unwrap();
        }
        reload(d)
    }

    /// Fonts registered with `add_font` are materialised on save.
    fn reload(mut d: Document) -> Document {
        Document::load(&d.save(&Default::default()).unwrap()).unwrap()
    }

    #[test]
    fn extracts_stamped_text() {
        let d = stamped(&[
            ("Hello world", 72.0, 700.0, 12.0),
            ("Second line here", 72.0, 680.0, 12.0),
            ("Far away", 300.0, 700.0, 12.0),
        ]);
        let c = page_content(&d, 0).unwrap();
        assert_eq!(c.glyphs.len(), 11 + 16 + 8);
        let first = &c.glyphs[0];
        assert!(
            (first.rect.x0 - 72.0).abs() < 0.01 && (first.origin.y - 700.0).abs() < 0.01,
            "{:?}",
            first.rect
        );
        assert!((first.size - 12.0).abs() < 1e-9);
        let text = page_text(&d, 0).unwrap();
        assert_eq!(text, "Hello world Far away\nSecond line here");
        let words = page_words(&d, 0).unwrap();
        assert_eq!(words[0].text, "Hello");
        assert!(words[0].rect.x1 < words[1].rect.x0);
    }

    #[test]
    fn search_hits_and_spans_lines() {
        let d = stamped(&[
            ("The quick brown", 72.0, 700.0, 12.0),
            ("fox jumps", 72.0, 686.0, 12.0),
        ]);
        let m = search(&d, 0, "brown fox", &SearchOptions::default()).unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].rects.len(), 2, "one rect per line");
        assert_eq!(
            search(
                &d,
                0,
                "QUICK",
                &SearchOptions {
                    case_insensitive: true,
                    ..Default::default()
                }
            )
            .unwrap()
            .len(),
            1
        );
        assert_eq!(
            search(&d, 0, "QUICK", &SearchOptions::default())
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            search(
                &d,
                0,
                "ox",
                &SearchOptions {
                    whole_word: true,
                    ..Default::default()
                }
            )
            .unwrap()
            .len(),
            0
        );
        assert_eq!(
            search(
                &d,
                0,
                "fox",
                &SearchOptions {
                    whole_word: true,
                    ..Default::default()
                }
            )
            .unwrap()
            .len(),
            1
        );
        let r = m[0].rects[0];
        assert!(
            r.x0 > 72.0 + 40.0 && r.y1 > 700.0,
            "brown sits after 'The quick': {r:?}"
        );
    }

    #[test]
    fn watermark_and_rotated_text() {
        let mut d = Document::new();
        d.add_page(PageSize::LETTER);
        ops::stamp_text(&mut d, &[0], &TextStamp::watermark("DRAFT")).unwrap();
        ops::stamp_text(
            &mut d,
            &[0],
            &TextStamp {
                text: "Sideways".into(),
                rotation: 90.0,
                size: 20.0,
                position: ops::Position::CenterLeft,
                ..Default::default()
            },
        )
        .unwrap();
        let d = reload(d);
        let text = page_text(&d, 0).unwrap();
        assert!(
            text.contains("DRAFT") && text.contains("Sideways"),
            "{text}"
        );
        let c = page_content(&d, 0).unwrap();
        let s = c.glyphs.iter().find(|g| g.text == "S").unwrap();
        assert!(s.dir.y > 0.9, "rotated text direction {:?}", s.dir);
    }

    #[test]
    fn tj_adjustments_and_spacing() {
        let mut d = Document::new();
        d.add_page(PageSize::LETTER);
        let f = d.add_standard_font(StandardFont::Courier);
        let name = d.add_page_resource(0, "Font", f).unwrap();
        // Courier: 600/1000 em per glyph. "AB" then a -1000 adjustment (moves right by 10 at 10pt) then "C".
        let content = format!("BT /{name} 10 Tf 2 Tc 100 100 Td [(AB) -1000 (C)] TJ ET");
        d.draw(0, content.as_bytes()).unwrap();
        let d = reload(d);
        let c = page_content(&d, 0).unwrap();
        let xs: Vec<f64> = c.glyphs.iter().map(|g| g.origin.x).collect();
        assert!((xs[0] - 100.0).abs() < 1e-6);
        assert!((xs[1] - 108.0).abs() < 1e-6, "6 + 2 char spacing: {xs:?}");
        assert!(
            (xs[2] - 126.0).abs() < 1e-6,
            "108 + 8 + 10 adjustment: {xs:?}"
        );
        assert_eq!(
            c.glyphs[1].loc.op, 6,
            "q Q wrap the original content, then BT Tf Tc Td TJ"
        );
        assert_eq!(c.glyphs[2].loc.elem, 2);
        assert!(
            (c.glyphs[0].loc.adjust + 800.0).abs() < 1e-6,
            "adjust reproduces advance: {}",
            c.glyphs[0].loc.adjust
        );
        assert_eq!(page_text(&d, 0).unwrap(), "AB C");
    }

    #[test]
    fn form_xobject_and_image() {
        let mut d = Document::new();
        d.add_page(PageSize::LETTER);
        let f = d.add_standard_font(StandardFont::Helvetica);
        let fname = d.add_page_resource(0, "Font", f).unwrap();
        let font_ref = f;
        let enc = d.font_mut(font_ref).unwrap().encode("Inside");
        let mut cb = crate::content::ContentBuilder::new();
        cb.begin_text()
            .font(&fname, 10.0)
            .text_matrix(&Matrix::translate(0.0, 0.0))
            .show_literal(&enc)
            .end_text();
        let res = Dict::new().with("Font", Dict::new().with(fname.as_str(), font_ref));
        let form = crate::annot::make_form(
            &mut d,
            100.0,
            20.0,
            Some(Matrix::translate(0.0, 0.0)),
            res,
            cb.finish(),
        );
        let xname = d.add_page_resource(0, "XObject", form).unwrap();
        let png = crate::image::tests::tiny_png();
        let img = d.add_image(&crate::image::Image::load(&png).unwrap(), 6);
        let iname = d.add_page_resource(0, "XObject", img).unwrap();
        let content =
            format!("q 1 0 0 1 200 300 cm /{xname} Do Q q 50 0 0 40 10 10 cm /{iname} Do Q");
        d.draw(0, content.as_bytes()).unwrap();
        let d = reload(d);
        let c = page_content(&d, 0).unwrap();
        assert_eq!(c.forms.len(), 1);
        assert_eq!(c.images.len(), 1);
        assert!(
            (c.images[0].rect.x0 - 10.0).abs() < 1e-9 && (c.images[0].rect.x1 - 60.0).abs() < 1e-9
        );
        let g = &c.glyphs[0];
        assert_eq!(g.loc.stream, StreamId::Form(c.forms[0].xobject));
        assert!(
            (g.origin.x - 200.0).abs() < 1e-9 && (g.origin.y - 300.0).abs() < 1e-9,
            "{:?}",
            g.origin
        );
        assert_eq!(page_text(&d, 0).unwrap(), "Inside");
    }

    #[test]
    fn cmap_parsing() {
        let data = b"/CIDInit /ProcSet findresource begin begincmap 1 begincodespacerange <00> <FF> endcodespacerange 2 beginbfchar <41> <0042> <42> /eacute endbfchar 1 beginbfrange <61> <63> <0078> endbfrange 1 beginbfrange <70> <71> [<0050> <00510052>] endbfrange endcmap";
        let mut r = Vec::new();
        let (mut cm, mut cr, mut uni) = (HashMap::new(), Vec::new(), HashMap::new());
        parse_cmap(data, &mut r, &mut cm, &mut cr, &mut uni);
        assert_eq!(r.len(), 1);
        assert_eq!(uni[&0x41], "B");
        assert_eq!(uni[&0x42], "é");
        assert_eq!(uni[&0x63], "z");
        assert_eq!(uni[&0x71], "QR");
    }
}
