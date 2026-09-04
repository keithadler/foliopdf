//! Tokenizer for PDF syntax (ISO 32000-1 §7.2).
//!
//! The lexer works on a borrowed byte slice and never allocates for
//! delimiters or numbers. It is shared by the object parser and the content
//! stream parser.

use crate::error::{Error, Result};
use crate::object::{Name, PdfString};

/// A lexical token.
#[derive(Debug, Clone, PartialEq)]
#[allow(missing_docs)]
pub enum Token {
    Integer(i64),
    Real(f64),
    String(PdfString),
    Name(Name),
    ArrayOpen,
    ArrayClose,
    DictOpen,
    DictClose,
    BraceOpen,
    BraceClose,
    /// A bare keyword such as `obj`, `R`, `true` or a content operator.
    Keyword(Vec<u8>),
    Eof,
}

#[inline]
pub(crate) fn is_whitespace(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\r' | b'\n' | b'\x0C' | b'\0')
}

#[inline]
pub(crate) fn is_delimiter(c: u8) -> bool {
    matches!(
        c,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

#[inline]
pub(crate) fn is_regular(c: u8) -> bool {
    !is_whitespace(c) && !is_delimiter(c)
}

/// A cursor over PDF source bytes.
#[derive(Clone)]
pub struct Lexer<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) pos: usize,
}

impl<'a> Lexer<'a> {
    /// Creates a lexer positioned at byte 0.
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }
    /// Creates a lexer positioned at `pos`.
    pub fn at(data: &'a [u8], pos: usize) -> Self {
        Self {
            data,
            pos: pos.min(data.len()),
        }
    }
    /// Current byte offset.
    pub fn pos(&self) -> usize {
        self.pos
    }
    /// Moves to `pos`.
    pub fn seek(&mut self, pos: usize) {
        self.pos = pos.min(self.data.len());
    }
    /// Remaining bytes.
    pub fn rest(&self) -> &'a [u8] {
        &self.data[self.pos..]
    }
    #[inline]
    fn peek_byte(&self) -> Option<u8> {
        self.data.get(self.pos).copied()
    }

    /// Skips whitespace and comments.
    pub fn skip_whitespace(&mut self) {
        while let Some(c) = self.peek_byte() {
            if is_whitespace(c) {
                self.pos += 1;
            } else if c == b'%' {
                while let Some(c) = self.peek_byte() {
                    if c == b'\n' || c == b'\r' {
                        break;
                    }
                    self.pos += 1;
                }
            } else {
                break;
            }
        }
    }

    /// Returns the next token without consuming it.
    pub fn peek(&self) -> Result<Token> {
        let mut c = self.clone();
        c.next_token()
    }

    /// Reads the next token.
    pub fn next_token(&mut self) -> Result<Token> {
        self.skip_whitespace();
        let start = self.pos;
        let Some(c) = self.peek_byte() else {
            return Ok(Token::Eof);
        };
        match c {
            b'[' => {
                self.pos += 1;
                Ok(Token::ArrayOpen)
            }
            b']' => {
                self.pos += 1;
                Ok(Token::ArrayClose)
            }
            b'{' => {
                self.pos += 1;
                Ok(Token::BraceOpen)
            }
            b'}' => {
                self.pos += 1;
                Ok(Token::BraceClose)
            }
            b'<' => {
                if self.data.get(self.pos + 1) == Some(&b'<') {
                    self.pos += 2;
                    Ok(Token::DictOpen)
                } else {
                    self.pos += 1;
                    self.hex_string()
                }
            }
            b'>' => {
                if self.data.get(self.pos + 1) == Some(&b'>') {
                    self.pos += 2;
                    Ok(Token::DictClose)
                } else {
                    // A stray '>' is a syntax error; skip it to stay lenient.
                    self.pos += 1;
                    self.next_token()
                }
            }
            b'(' => {
                self.pos += 1;
                self.literal_string()
            }
            b')' => {
                self.pos += 1;
                self.next_token()
            }
            b'/' => {
                self.pos += 1;
                self.name()
            }
            b'+' | b'-' | b'.' | b'0'..=b'9' => self.number(),
            _ => {
                while let Some(c) = self.peek_byte() {
                    if !is_regular(c) {
                        break;
                    }
                    self.pos += 1;
                }
                if self.pos == start {
                    self.pos += 1;
                    return Err(Error::syntax(start, format!("unexpected byte 0x{c:02X}")));
                }
                Ok(Token::Keyword(self.data[start..self.pos].to_vec()))
            }
        }
    }

    fn number(&mut self) -> Result<Token> {
        let start = self.pos;
        let mut is_real = false;
        let mut seen_digit = false;
        while let Some(c) = self.peek_byte() {
            match c {
                b'0'..=b'9' => {
                    seen_digit = true;
                    self.pos += 1;
                }
                b'.' | b'-' | b'+' | b'e' | b'E' => {
                    if c == b'.' || c == b'e' || c == b'E' {
                        is_real = true;
                    }
                    // A sign in the middle of a number ("--5", "3-4") shows up
                    // in real files; treat as part of the token and parse leniently.
                    self.pos += 1;
                }
                _ => break,
            }
        }
        let text = &self.data[start..self.pos];
        if !seen_digit {
            // Something like a lone "-" or "."; PDF viewers treat as 0.
            return Ok(Token::Integer(0));
        }
        if !is_real {
            if let Some(v) = parse_int_lenient(text) {
                return Ok(Token::Integer(v));
            }
            return Ok(Token::Real(parse_real_lenient(text)));
        }
        Ok(Token::Real(parse_real_lenient(text)))
    }

    fn name(&mut self) -> Result<Token> {
        let mut out = Vec::new();
        while let Some(c) = self.peek_byte() {
            if !is_regular(c) {
                break;
            }
            self.pos += 1;
            if c == b'#' {
                let h = self.data.get(self.pos).copied().and_then(hex_val);
                let l = self.data.get(self.pos + 1).copied().and_then(hex_val);
                if let (Some(h), Some(l)) = (h, l) {
                    out.push(h << 4 | l);
                    self.pos += 2;
                    continue;
                }
            }
            out.push(c);
        }
        Ok(Token::Name(Name(out)))
    }

    fn literal_string(&mut self) -> Result<Token> {
        let start = self.pos;
        let mut out = Vec::new();
        let mut depth = 1usize;
        while let Some(c) = self.peek_byte() {
            self.pos += 1;
            match c {
                b'\\' => {
                    let Some(e) = self.peek_byte() else { break };
                    self.pos += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'(' => out.push(b'('),
                        b')' => out.push(b')'),
                        b'\\' => out.push(b'\\'),
                        b'\r' => {
                            if self.peek_byte() == Some(b'\n') {
                                self.pos += 1;
                            }
                        }
                        b'\n' => {}
                        b'0'..=b'7' => {
                            let mut v = (e - b'0') as u32;
                            for _ in 0..2 {
                                match self.peek_byte() {
                                    Some(d @ b'0'..=b'7') => {
                                        v = v * 8 + (d - b'0') as u32;
                                        self.pos += 1;
                                    }
                                    _ => break,
                                }
                            }
                            out.push((v & 0xFF) as u8);
                        }
                        other => out.push(other),
                    }
                }
                b'(' => {
                    depth += 1;
                    out.push(c);
                }
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(Token::String(PdfString::new(out)));
                    }
                    out.push(c);
                }
                b'\r' => {
                    // CR or CRLF inside a literal is normalised to LF.
                    if self.peek_byte() == Some(b'\n') {
                        self.pos += 1;
                    }
                    out.push(b'\n');
                }
                _ => out.push(c),
            }
        }
        Err(Error::syntax(start, "unterminated string"))
    }

    fn hex_string(&mut self) -> Result<Token> {
        let mut out = Vec::new();
        let mut hi: Option<u8> = None;
        while let Some(c) = self.peek_byte() {
            self.pos += 1;
            if c == b'>' {
                if let Some(h) = hi {
                    out.push(h << 4);
                }
                return Ok(Token::String(PdfString::hex(out)));
            }
            if let Some(v) = hex_val(c) {
                match hi.take() {
                    None => hi = Some(v),
                    Some(h) => out.push(h << 4 | v),
                }
            }
            // Non-hex bytes are ignored per spec-tolerant behaviour.
        }
        Err(Error::syntax(self.pos, "unterminated hex string"))
    }

    /// Consumes the given keyword if it comes next; returns whether it did.
    pub fn expect_keyword(&mut self, kw: &[u8]) -> bool {
        let save = self.pos;
        if let Ok(Token::Keyword(k)) = self.next_token() {
            if k == kw {
                return true;
            }
        }
        self.pos = save;
        false
    }

    /// Finds the next occurrence of `needle` at or after the current position.
    pub fn find(&self, needle: &[u8]) -> Option<usize> {
        find_bytes(self.data, self.pos, needle)
    }
}

#[inline]
pub(crate) fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn parse_int_lenient(text: &[u8]) -> Option<i64> {
    let mut neg = false;
    let mut i = 0;
    while i < text.len() && (text[i] == b'+' || text[i] == b'-') {
        if text[i] == b'-' {
            neg = !neg;
        }
        i += 1;
    }
    let mut v: i64 = 0;
    let mut any = false;
    while i < text.len() {
        let c = text[i];
        if !c.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((c - b'0') as i64)?;
        any = true;
        i += 1;
    }
    if !any {
        return None;
    }
    Some(if neg { -v } else { v })
}

fn parse_real_lenient(text: &[u8]) -> f64 {
    // Strip everything the standard parser would choke on: repeated signs,
    // trailing garbage and multiple dots ("1.2.3" -> 1.2).
    let mut s = String::with_capacity(text.len());
    let mut seen_dot = false;
    let mut seen_digit = false;
    for &c in text {
        match c {
            b'0'..=b'9' => {
                seen_digit = true;
                s.push(c as char);
            }
            b'.' if !seen_dot => {
                seen_dot = true;
                s.push('.');
            }
            b'-' if !seen_digit && s.is_empty() => s.push('-'),
            b'-' | b'+' if !seen_digit => {}
            _ => break,
        }
    }
    s.parse().unwrap_or(0.0)
}

/// Forward byte search starting at `from`.
pub(crate) fn find_bytes(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || from >= hay.len() || hay.len() - from < needle.len() {
        return None;
    }
    let first = needle[0];
    let mut i = from;
    let end = hay.len() - needle.len();
    while i <= end {
        let off = hay[i..=end].iter().position(|&b| b == first)?;
        i += off;
        if &hay[i..i + needle.len()] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Backward byte search: last occurrence starting before `before`.
pub(crate) fn rfind_bytes(hay: &[u8], before: usize, needle: &[u8]) -> Option<usize> {
    let before = before.min(hay.len());
    if needle.is_empty() || before < needle.len() {
        return None;
    }
    let mut i = before - needle.len();
    loop {
        if &hay[i..i + needle.len()] == needle {
            return Some(i);
        }
        if i == 0 {
            return None;
        }
        i -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(src: &str) -> Vec<Token> {
        let mut l = Lexer::new(src.as_bytes());
        let mut v = Vec::new();
        loop {
            let t = l.next_token().unwrap();
            if t == Token::Eof {
                break;
            }
            v.push(t);
        }
        v
    }

    #[test]
    fn numbers_and_names() {
        assert_eq!(
            toks("12 -3 4.5 -.5 /A#20B"),
            vec![
                Token::Integer(12),
                Token::Integer(-3),
                Token::Real(4.5),
                Token::Real(-0.5),
                Token::Name(Name::new("A B")),
            ]
        );
    }

    #[test]
    fn strings() {
        assert_eq!(
            toks(r"(a\(b\)\101\n) <48 65 6C>"),
            vec![
                Token::String(PdfString::new(b"a(b)A\n".to_vec())),
                Token::String(PdfString::hex(b"Hel".to_vec())),
            ]
        );
        assert_eq!(
            toks("(nested (paren) ok)"),
            vec![Token::String(PdfString::new(b"nested (paren) ok".to_vec()))]
        );
    }

    #[test]
    fn comments_and_delims() {
        assert_eq!(
            toks("<< /K [1 % comment\n 2] >> obj"),
            vec![
                Token::DictOpen,
                Token::Name(Name::new("K")),
                Token::ArrayOpen,
                Token::Integer(1),
                Token::Integer(2),
                Token::ArrayClose,
                Token::DictClose,
                Token::Keyword(b"obj".to_vec()),
            ]
        );
    }

    #[test]
    fn lenient_numbers() {
        assert_eq!(
            toks("--5 3.4.5 6-2"),
            vec![Token::Integer(5), Token::Real(3.4), Token::Real(6.0)]
        );
    }

    #[test]
    fn search() {
        let h = b"abc startxref 123";
        assert_eq!(find_bytes(h, 0, b"startxref"), Some(4));
        assert_eq!(rfind_bytes(h, h.len(), b"startxref"), Some(4));
        assert_eq!(find_bytes(h, 5, b"startxref"), None);
    }
}
