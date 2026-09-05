//! Content stream parsing and writing (ISO 32000-1 §7.8.2).
//!
//! A content stream is a flat sequence of operands followed by an operator.
//! [`parse()`] turns it into [`Op`]s (inline images are kept as opaque byte
//! runs) and [`write()`] serialises them again, which is what lets the text
//! engine rewrite a page without disturbing anything it does not understand.

use std::collections::HashMap;

use crate::lexer::{is_whitespace, Lexer, Token};
use crate::object::{Dict, Name, Object};
use crate::writer::serialize;

/// One operator with its operands.
#[derive(Debug, Clone, PartialEq)]
pub struct Op {
    /// Operator name, e.g. `Tj`, `cm`, `Do`. Inline images use `BI`.
    pub name: String,
    /// Operands in order.
    pub operands: Vec<Object>,
    /// For `BI`: the raw bytes from `BI` through `EI`, re-emitted verbatim.
    pub raw: Option<Vec<u8>>,
}

impl Op {
    /// Creates an operator with operands.
    pub fn new(name: &str, operands: Vec<Object>) -> Self {
        Self {
            name: name.into(),
            operands,
            raw: None,
        }
    }
    /// Operand `i` as a number.
    pub fn num(&self, i: usize) -> Option<f64> {
        self.operands.get(i).and_then(Object::as_f64)
    }
    /// Operand `i` as a name.
    pub fn name_at(&self, i: usize) -> Option<&Name> {
        self.operands.get(i).and_then(Object::as_name)
    }
}

/// Parses a decoded content stream. Malformed input is tolerated: unknown
/// tokens are skipped and operands without an operator are dropped.
pub fn parse(data: &[u8]) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut lex = Lexer::new(data);
    let mut operands: Vec<Object> = Vec::new();
    let mut guard = 0usize;
    loop {
        guard += 1;
        if guard > 50_000_000 {
            break;
        }
        lex.skip_whitespace();
        let before = lex.pos();
        let tok = match lex.next_token() {
            Ok(Token::Eof) => break,
            Ok(t) => t,
            Err(_) => {
                // Skip one byte and try again.
                if lex.pos() <= before {
                    lex.seek(before + 1);
                }
                if lex.pos() >= data.len() {
                    break;
                }
                continue;
            }
        };
        match tok {
            Token::Keyword(k) => {
                let name = String::from_utf8_lossy(&k).into_owned();
                match name.as_str() {
                    "true" => operands.push(Object::Bool(true)),
                    "false" => operands.push(Object::Bool(false)),
                    "null" => operands.push(Object::Null),
                    "BI" => {
                        let start = before;
                        let (end, dict) = inline_image(data, lex.pos());
                        lex.seek(end);
                        ops.push(Op {
                            name: "BI".into(),
                            operands: vec![Object::Dict(dict)],
                            raw: Some(data[start..end].to_vec()),
                        });
                        operands.clear();
                    }
                    _ => {
                        ops.push(Op {
                            name,
                            operands: std::mem::take(&mut operands),
                            raw: None,
                        });
                    }
                }
            }
            Token::ArrayOpen => operands.push(Object::Array(parse_array(&mut lex, 0))),
            Token::DictOpen => operands.push(Object::Dict(parse_dict(&mut lex, 0))),
            Token::ArrayClose | Token::DictClose | Token::BraceOpen | Token::BraceClose => {}
            t => {
                if let Some(o) = simple(t) {
                    operands.push(o);
                }
            }
        }
        if operands.len() > 4096 {
            operands.clear();
        }
    }
    ops
}

fn simple(t: Token) -> Option<Object> {
    Some(match t {
        Token::Integer(i) => Object::Integer(i),
        Token::Real(r) => Object::Real(r),
        Token::String(s) => Object::String(s),
        Token::Name(n) => Object::Name(n),
        _ => return None,
    })
}

fn parse_array(lex: &mut Lexer, depth: usize) -> Vec<Object> {
    let mut out = Vec::new();
    loop {
        match lex.next_token() {
            Ok(Token::ArrayClose) | Ok(Token::Eof) | Err(_) => break,
            Ok(Token::ArrayOpen) => {
                if depth < 64 {
                    out.push(Object::Array(parse_array(lex, depth + 1)));
                }
            }
            Ok(Token::DictOpen) => {
                if depth < 64 {
                    out.push(Object::Dict(parse_dict(lex, depth + 1)));
                }
            }
            Ok(Token::Keyword(k)) => match k.as_slice() {
                b"true" => out.push(Object::Bool(true)),
                b"false" => out.push(Object::Bool(false)),
                b"null" => out.push(Object::Null),
                _ => {} // operators inside an array are garbage; ignore
            },
            Ok(t) => {
                if let Some(o) = simple(t) {
                    out.push(o);
                }
            }
        }
    }
    out
}

fn parse_dict(lex: &mut Lexer, depth: usize) -> Dict {
    let mut d = Dict::new();
    loop {
        let key = match lex.next_token() {
            Ok(Token::Name(n)) => n,
            Ok(Token::DictClose) | Ok(Token::Eof) | Err(_) => break,
            Ok(_) => continue,
        };
        let value = match lex.next_token() {
            Ok(Token::ArrayOpen) => Object::Array(parse_array(lex, depth + 1)),
            Ok(Token::DictOpen) => Object::Dict(parse_dict(lex, depth + 1)),
            Ok(Token::DictClose) | Ok(Token::Eof) | Err(_) => break,
            Ok(Token::Keyword(k)) => match k.as_slice() {
                b"true" => Object::Bool(true),
                b"false" => Object::Bool(false),
                _ => Object::Null,
            },
            Ok(t) => simple(t).unwrap_or(Object::Null),
        };
        d.0.insert(key, value);
    }
    d
}

/// Parses an inline image starting just after `BI`. Returns the byte offset
/// just past `EI` and the image dictionary.
fn inline_image(data: &[u8], pos: usize) -> (usize, Dict) {
    let mut lex = Lexer::at(data, pos);
    let mut dict = Dict::new();
    // Key/value pairs until ID.
    loop {
        match lex.next_token() {
            Ok(Token::Name(k)) => {
                let v = match lex.next_token() {
                    Ok(Token::ArrayOpen) => Object::Array(parse_array(&mut lex, 0)),
                    Ok(Token::DictOpen) => Object::Dict(parse_dict(&mut lex, 0)),
                    Ok(Token::Keyword(w)) if w == b"ID" => {
                        break;
                    }
                    Ok(t) => simple(t).unwrap_or(Object::Null),
                    Err(_) => break,
                };
                dict.0.insert(k, v);
            }
            Ok(Token::Keyword(w)) if w == b"ID" => break,
            Ok(Token::Eof) | Err(_) => return (data.len(), dict),
            Ok(_) => {}
        }
    }
    // One whitespace byte after ID, then binary data.
    let mut p = lex.pos();
    if p < data.len() && is_whitespace(data[p]) {
        p += 1;
    }
    let start = p;
    // Prefer an explicit length (PDF 2.0 /L or /Length).
    let declared = dict
        .get("L")
        .or_else(|| dict.get("Length"))
        .and_then(Object::as_i64)
        .filter(|&l| l >= 0)
        .map(|l| l as usize);
    if let Some(len) = declared {
        if start + len <= data.len() {
            if let Some(end) = find_ei(data, start + len) {
                return (end, dict);
            }
        }
    }
    // Otherwise the expected size for uncompressed data, then scan for EI.
    let expected = expected_size(&dict);
    let scan_from = match expected {
        Some(n) if start + n <= data.len() => start + n,
        _ => start,
    };
    match find_ei(data, scan_from) {
        Some(end) => (end, dict),
        None => (data.len(), dict),
    }
}

fn expected_size(d: &Dict) -> Option<usize> {
    if d.contains("F") || d.contains("Filter") {
        return None;
    }
    let w = d.get("W").or_else(|| d.get("Width"))?.as_i64()? as usize;
    let h = d.get("H").or_else(|| d.get("Height"))?.as_i64()? as usize;
    let bpc = d
        .get("BPC")
        .or_else(|| d.get("BitsPerComponent"))
        .and_then(Object::as_i64)
        .unwrap_or(8) as usize;
    let mask = d
        .get("IM")
        .or_else(|| d.get("ImageMask"))
        .and_then(Object::as_bool)
        .unwrap_or(false);
    let ncomp = if mask {
        1
    } else {
        match d
            .get("CS")
            .or_else(|| d.get("ColorSpace"))
            .and_then(Object::as_name)
            .map(|n| n.as_str().into_owned())
            .as_deref()
        {
            Some("DeviceRGB") | Some("RGB") | Some("CalRGB") => 3,
            Some("DeviceCMYK") | Some("CMYK") => 4,
            Some("DeviceGray") | Some("G") | Some("CalGray") | Some("I") | Some("Indexed")
            | None => 1,
            _ => 1,
        }
    };
    let bpc = if mask { 1 } else { bpc };
    Some(h * ((w * ncomp * bpc).div_ceil(8)))
}

/// Finds `EI` delimited by whitespace at or after `from`; returns the
/// offset just past it.
fn find_ei(data: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i + 1 < data.len() {
        if data[i] == b'E'
            && data[i + 1] == b'I'
            && (i == 0 || is_whitespace(data[i - 1]))
            && (i + 2 >= data.len()
                || is_whitespace(data[i + 2])
                || data[i + 2] == b'/'
                || data[i + 2] == b'['
                || data[i + 2] == b'q'
                || data[i + 2] == b'Q')
        {
            return Some(i + 2);
        }
        i += 1;
    }
    None
}

/// Serialises operators back into a content stream.
pub fn write(ops: &[Op]) -> Vec<u8> {
    let mut out = Vec::new();
    let remap = HashMap::new();
    for op in ops {
        if let Some(raw) = &op.raw {
            out.extend_from_slice(raw);
            out.push(b'\n');
            continue;
        }
        for o in &op.operands {
            serialize(&mut out, o, &remap);
            out.push(b' ');
        }
        out.extend_from_slice(op.name.as_bytes());
        out.push(b'\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_rewrites() {
        let src = b"q 1 0 0 1 10 20 cm BT /F1 12 Tf [(Hel) -20 (lo)] TJ (x) Tj ET Q\nBI /W 2 /H 1 /CS /G /BPC 8 ID \x00\xff EI\n0 0 m 1 1 l S";
        let ops = parse(src);
        let names: Vec<&str> = ops.iter().map(|o| o.name.as_str()).collect();
        assert_eq!(
            names,
            ["q", "cm", "BT", "Tf", "TJ", "Tj", "ET", "Q", "BI", "m", "l", "S"]
        );
        assert_eq!(ops[4].operands[0].as_array().unwrap().len(), 3);
        let bi = &ops[8];
        assert_eq!(
            bi.raw.as_ref().unwrap().len(),
            b"BI /W 2 /H 1 /CS /G /BPC 8 ID \x00\xff EI".len()
        );
        let out = write(&ops);
        let again = parse(&out);
        assert_eq!(again, ops);
    }

    #[test]
    fn tolerates_garbage() {
        let ops = parse(b"1 2 3 ] >> foo (unterminated");
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].name, "foo");
        assert_eq!(ops[0].operands.len(), 3);
    }

    #[test]
    fn inline_image_with_ei_bytes_inside() {
        // Data contains the bytes "EI" but not whitespace-delimited; the expected size steers past it.
        let src = b"BI /W 3 /H 1 /CS /G /BPC 8 ID \x00EI EI Q";
        let ops = parse(src);
        assert_eq!(ops[0].name, "BI");
        assert_eq!(ops[1].name, "Q");
    }
}
