//! Stream filters (ISO 32000-1 §7.4).
//!
//! Lossless, general-purpose filters are decoded here: `FlateDecode`,
//! `LZWDecode`, `ASCIIHexDecode`, `ASCII85Decode` and `RunLengthDecode`,
//! plus the PNG and TIFF predictors. Image codecs (`DCTDecode`, `JPXDecode`,
//! `CCITTFaxDecode`, `JBIG2Decode`) are *not* decoded: their bytes are the
//! image and are passed through untouched. Editing never needs to look inside
//! them.

use crate::error::{Error, Result};
use crate::lexer::hex_val;
use crate::object::{Dict, Name, Object, Stream};

/// Filters whose output is an encoded image rather than generic bytes.
pub const IMAGE_FILTERS: [&str; 4] = ["DCTDecode", "JPXDecode", "CCITTFaxDecode", "JBIG2Decode"];

/// Whether `name` is an image codec filter (left encoded by [`decode_stream`]).
pub fn is_image_filter(name: &Name) -> bool {
    IMAGE_FILTERS.iter().any(|f| name == f) || name == "DCT" || name == "CCF"
}

/// Whether every filter on `stream` is one this module can fully decode.
pub fn is_fully_decodable(stream: &Stream) -> bool {
    stream
        .filters()
        .iter()
        .all(|f| !is_image_filter(f) && f != "Crypt")
}

/// Decodes all filters on `stream`. Filters are applied in order; decoding
/// stops (successfully) at the first image codec. `resolve` is used to look
/// through indirect references in `/DecodeParms`; pass `None` when the
/// dictionary is known to be direct.
pub fn decode_stream(
    stream: &Stream,
    resolve: Option<&dyn Fn(&Object) -> Object>,
) -> Result<Vec<u8>> {
    let deref = |o: &Object| -> Object {
        match (o, resolve) {
            (Object::Reference(_), Some(f)) => f(o),
            _ => o.clone(),
        }
    };
    let filters = match deref(stream.dict.get("Filter").unwrap_or(&Object::Null)) {
        Object::Name(n) => vec![n],
        Object::Array(a) => a
            .iter()
            .filter_map(|o| deref(o).as_name().cloned())
            .collect(),
        _ => Vec::new(),
    };
    let parms_key = if stream.dict.contains("DecodeParms") {
        "DecodeParms"
    } else {
        "DP"
    };
    let parms: Vec<Option<Dict>> = match deref(stream.dict.get(parms_key).unwrap_or(&Object::Null))
    {
        Object::Dict(d) => vec![Some(d)],
        Object::Array(a) => a.iter().map(|o| deref(o).into_dict()).collect(),
        _ => Vec::new(),
    };
    let mut data = std::borrow::Cow::Borrowed(stream.data.as_slice());
    for (i, f) in filters.iter().enumerate() {
        if is_image_filter(f) {
            break;
        }
        let p = parms.get(i).cloned().flatten();
        // Resolve indirect values inside the parms dict.
        let p = p.map(|d| Dict(d.0.into_iter().map(|(k, v)| (k, deref(&v))).collect()));
        data = std::borrow::Cow::Owned(decode(&data, f, p.as_ref())?);
    }
    Ok(data.into_owned())
}

/// Applies a single named filter.
pub fn decode(data: &[u8], filter: &Name, parms: Option<&Dict>) -> Result<Vec<u8>> {
    let out = match filter.as_bytes() {
        b"FlateDecode" | b"Fl" => flate_decode(data)?,
        b"LZWDecode" | b"LZW" => {
            let early = parms
                .and_then(|d| d.get("EarlyChange"))
                .and_then(Object::as_i64)
                .unwrap_or(1)
                != 0;
            lzw_decode(data, early)?
        }
        b"ASCIIHexDecode" | b"AHx" => ascii_hex_decode(data),
        b"ASCII85Decode" | b"A85" => ascii85_decode(data),
        b"RunLengthDecode" | b"RL" => run_length_decode(data),
        b"Crypt" => data.to_vec(), // Identity crypt filter; real decryption happens at load.
        other => {
            return Err(Error::UnsupportedFilter(
                String::from_utf8_lossy(other).into_owned(),
            ))
        }
    };
    match parms {
        Some(p) => apply_predictor(out, p),
        None => Ok(out),
    }
}

/// Inflates zlib (or raw deflate) data. Truncated streams yield the bytes
/// that could be recovered rather than an error, matching viewer behaviour.
pub fn flate_decode(data: &[u8]) -> Result<Vec<u8>> {
    use miniz_oxide::DataFormat;
    // Skip leading whitespace/garbage some producers emit before the zlib header.
    let mut start = 0;
    while start < data.len() && crate::lexer::is_whitespace(data[start]) {
        start += 1;
    }
    let d = &data[start..];
    if d.is_empty() {
        return Ok(Vec::new());
    }
    let (out, complete) = inflate_lenient(d, DataFormat::Zlib);
    if complete || !out.is_empty() {
        return Ok(out);
    }
    // Maybe raw deflate without the zlib wrapper, or a corrupt header byte.
    let (out, complete) = inflate_lenient(d, DataFormat::Raw);
    if complete || !out.is_empty() {
        return Ok(out);
    }
    if d.len() > 1 {
        let (out, complete) = inflate_lenient(&d[1..], DataFormat::Zlib);
        if complete || !out.is_empty() {
            return Ok(out);
        }
    }
    Err(Error::Decompress("not a valid deflate stream".into()))
}

/// Streams through the inflater, returning whatever was produced and whether
/// the stream ended cleanly.
fn inflate_lenient(data: &[u8], format: miniz_oxide::DataFormat) -> (Vec<u8>, bool) {
    use miniz_oxide::inflate::stream::{inflate, InflateState};
    use miniz_oxide::{MZFlush, MZStatus};
    let mut state = InflateState::new_boxed(format);
    let mut out = Vec::with_capacity(data.len() * 3);
    let mut buf = vec![0u8; 64 * 1024];
    let mut pos = 0usize;
    loop {
        let res = inflate(&mut state, &data[pos..], &mut buf, MZFlush::None);
        out.extend_from_slice(&buf[..res.bytes_written]);
        pos += res.bytes_consumed;
        match res.status {
            Ok(MZStatus::StreamEnd) => return (out, true),
            Ok(_) => {
                if res.bytes_consumed == 0 && res.bytes_written == 0 {
                    return (out, false);
                }
            }
            Err(_) => return (out, false),
        }
        if pos >= data.len() && res.bytes_written == 0 {
            return (out, false);
        }
    }
}

/// Deflates data with a zlib wrapper. `level` is 0..=10 (miniz scale).
pub fn flate_encode(data: &[u8], level: u8) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, level.min(10))
}

fn ascii_hex_decode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() / 2);
    let mut hi = None;
    for &c in data {
        if c == b'>' {
            break;
        }
        if let Some(v) = hex_val(c) {
            match hi.take() {
                None => hi = Some(v),
                Some(h) => out.push(h << 4 | v),
            }
        }
    }
    if let Some(h) = hi {
        out.push(h << 4);
    }
    out
}

fn ascii85_decode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 4 / 5);
    let mut group = [0u8; 5];
    let mut n = 0;
    let mut i = 0;
    if data.starts_with(b"<~") {
        i = 2;
    }
    while i < data.len() {
        let c = data[i];
        i += 1;
        if crate::lexer::is_whitespace(c) {
            continue;
        }
        if c == b'~' {
            break;
        }
        if c == b'z' && n == 0 {
            out.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        if !(b'!'..=b'u').contains(&c) {
            continue;
        }
        group[n] = c - b'!';
        n += 1;
        if n == 5 {
            let mut v: u32 = 0;
            for g in group {
                v = v.wrapping_mul(85).wrapping_add(g as u32);
            }
            out.extend_from_slice(&v.to_be_bytes());
            n = 0;
        }
    }
    if n > 0 {
        for g in group.iter_mut().skip(n) {
            *g = 84;
        }
        let mut v: u32 = 0;
        for g in group {
            v = v.wrapping_mul(85).wrapping_add(g as u32);
        }
        let bytes = v.to_be_bytes();
        out.extend_from_slice(&bytes[..n - 1]);
    }
    out
}

fn run_length_decode(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 2);
    let mut i = 0;
    while i < data.len() {
        let l = data[i] as usize;
        i += 1;
        if l == 128 {
            break;
        }
        if l < 128 {
            let end = (i + l + 1).min(data.len());
            out.extend_from_slice(&data[i..end]);
            i = end;
        } else {
            if let Some(&b) = data.get(i) {
                out.extend(std::iter::repeat(b).take(257 - l));
            }
            i += 1;
        }
    }
    out
}

fn lzw_decode(data: &[u8], early_change: bool) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() * 3);
    let mut dict: Vec<Vec<u8>> = (0..256).map(|i| vec![i as u8]).collect();
    dict.push(Vec::new()); // 256 clear
    dict.push(Vec::new()); // 257 eod
    let mut code_len = 9;
    let mut prev: Option<Vec<u8>> = None;
    let mut bitbuf: u32 = 0;
    let mut nbits = 0;
    let early = if early_change { 1 } else { 0 };
    for &byte in data {
        bitbuf = (bitbuf << 8) | byte as u32;
        nbits += 8;
        while nbits >= code_len {
            let code = ((bitbuf >> (nbits - code_len)) & ((1 << code_len) - 1)) as usize;
            nbits -= code_len;
            match code {
                256 => {
                    dict.truncate(258);
                    code_len = 9;
                    prev = None;
                }
                257 => return Ok(out),
                _ => {
                    let entry = if code < dict.len() {
                        dict[code].clone()
                    } else if let Some(p) = &prev {
                        let mut e = p.clone();
                        e.push(p[0]);
                        e
                    } else {
                        return Err(Error::Decompress("bad LZW code".into()));
                    };
                    out.extend_from_slice(&entry);
                    if let Some(p) = prev.take() {
                        let mut ne = p;
                        ne.push(entry[0]);
                        dict.push(ne);
                    }
                    prev = Some(entry);
                    if dict.len() + early >= (1 << code_len) && code_len < 12 {
                        code_len += 1;
                    }
                }
            }
        }
    }
    Ok(out)
}

fn apply_predictor(data: Vec<u8>, parms: &Dict) -> Result<Vec<u8>> {
    let pred = parms.get("Predictor").and_then(Object::as_i64).unwrap_or(1);
    if pred <= 1 {
        return Ok(data);
    }
    let colors = parms
        .get("Colors")
        .and_then(Object::as_i64)
        .unwrap_or(1)
        .clamp(1, 64) as usize;
    let bpc = parms
        .get("BitsPerComponent")
        .and_then(Object::as_i64)
        .unwrap_or(8)
        .clamp(1, 16) as usize;
    let columns = parms
        .get("Columns")
        .and_then(Object::as_i64)
        .unwrap_or(1)
        .max(1) as usize;
    let bpp = (colors * bpc).div_ceil(8).max(1);
    let row_len = (columns * colors * bpc).div_ceil(8);
    if pred == 2 {
        return Ok(tiff_predictor(data, colors, bpc, columns));
    }
    // PNG predictors: each row is prefixed with a filter-type byte.
    let mut out = Vec::with_capacity(data.len());
    let mut prev = vec![0u8; row_len];
    let mut pos = 0;
    while pos < data.len() {
        let ft = data[pos];
        pos += 1;
        let end = (pos + row_len).min(data.len());
        let mut row = data[pos..end].to_vec();
        row.resize(row_len, 0);
        pos = end;
        for i in 0..row_len {
            let a = if i >= bpp { row[i - bpp] } else { 0 };
            let b = prev[i];
            let c = if i >= bpp { prev[i - bpp] } else { 0 };
            row[i] = match ft {
                0 => row[i],
                1 => row[i].wrapping_add(a),
                2 => row[i].wrapping_add(b),
                3 => row[i].wrapping_add(((a as u16 + b as u16) / 2) as u8),
                4 => row[i].wrapping_add(paeth(a, b, c)),
                _ => row[i],
            };
        }
        out.extend_from_slice(&row);
        prev = row;
        if pos >= data.len() {
            break;
        }
    }
    Ok(out)
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).abs();
    let pb = (p - b as i16).abs();
    let pc = (p - c as i16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

fn tiff_predictor(mut data: Vec<u8>, colors: usize, bpc: usize, columns: usize) -> Vec<u8> {
    match bpc {
        8 => {
            let row_len = columns * colors;
            for row in data.chunks_mut(row_len) {
                for i in colors..row.len() {
                    row[i] = row[i].wrapping_add(row[i - colors]);
                }
            }
        }
        16 => {
            let row_len = columns * colors * 2;
            for row in data.chunks_mut(row_len) {
                for i in (colors * 2..row.len().saturating_sub(1)).step_by(2) {
                    let prev = u16::from_be_bytes([row[i - colors * 2], row[i - colors * 2 + 1]]);
                    let cur = u16::from_be_bytes([row[i], row[i + 1]]);
                    let v = cur.wrapping_add(prev).to_be_bytes();
                    row[i] = v[0];
                    row[i + 1] = v[1];
                }
            }
        }
        _ => {} // Sub-byte TIFF prediction is vanishingly rare; return as-is.
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flate_round_trip() {
        let src = b"hello hello hello hello".repeat(20);
        let enc = flate_encode(&src, 6);
        assert!(enc.len() < src.len());
        assert_eq!(flate_decode(&enc).unwrap(), src);
    }

    #[test]
    fn flate_truncated_returns_partial() {
        let src: Vec<u8> = (0..5000u32).map(|i| (i % 251) as u8).collect();
        let enc = flate_encode(&src, 6);
        let cut = &enc[..enc.len() / 2];
        let out = flate_decode(cut).unwrap();
        assert!(!out.is_empty());
        assert_eq!(&src[..out.len()], &out[..]);
    }

    #[test]
    fn ascii_filters() {
        assert_eq!(ascii_hex_decode(b"48656C6C6F>"), b"Hello");
        assert_eq!(ascii_hex_decode(b"4"), vec![0x40]);
        assert_eq!(ascii85_decode(b"87cURD]i,\"Ebo80~>"), b"Hello World!");
        assert_eq!(ascii85_decode(b"<~z~>"), vec![0, 0, 0, 0]);
    }

    #[test]
    fn run_length() {
        assert_eq!(
            run_length_decode(&[2, b'a', b'b', b'c', 254, b'x', 128]),
            b"abcxxx"
        );
    }

    #[test]
    fn lzw_known_vector() {
        // Example from ISO 32000-1 §7.4.4.2: input "-----A---B" encodes to
        // 80 0B 60 50 22 0C 0C 85 01.
        let enc = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];
        assert_eq!(lzw_decode(&enc, true).unwrap(), b"-----A---B");
    }

    #[test]
    fn png_up_predictor() {
        // Two rows of 3 bytes, predictor "Up" on the second.
        let data = vec![0, 1, 2, 3, 2, 1, 1, 1];
        let mut p = Dict::new();
        p.set("Predictor", 12).set("Columns", 3);
        assert_eq!(apply_predictor(data, &p).unwrap(), vec![1, 2, 3, 2, 3, 4]);
    }

    #[test]
    fn tiff_predictor_8bit() {
        let data = vec![10, 1, 1, 20, 2, 2];
        let mut p = Dict::new();
        p.set("Predictor", 2).set("Columns", 3);
        assert_eq!(
            apply_predictor(data, &p).unwrap(),
            vec![10, 11, 12, 20, 22, 24]
        );
    }
}
