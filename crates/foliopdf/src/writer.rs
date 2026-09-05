//! Serialisation: garbage collection, renumbering, stream recompression,
//! object streams, cross-reference streams and encryption.

use std::collections::{HashMap, HashSet};

use crate::content::{write_hex_string, write_literal_string, write_name, write_num};
use crate::crypto::{Method, SecurityHandler};
use crate::document::{Document, SaveOptions};
use crate::error::Result;
use crate::filters;
use crate::object::{Dict, ObjRef, Object, Stream};

/// Maximum objects per object stream.
const OBJSTM_CAPACITY: usize = 200;

/// Serialises `doc`.
pub fn write(doc: &mut Document, opts: &SaveOptions) -> Result<Vec<u8>> {
    let id0 = doc.ensure_id()?;
    let level = opts.compression_level.clamp(1, 10);
    let use_objstm = opts.compress && opts.object_streams;

    // Encryption setup.
    let mut security: Option<SecurityHandler> = None;
    let mut encrypt_dict: Option<Dict> = None;
    let mut min_version = (1, 4);
    if let Some(enc) = &opts.encryption {
        let (h, d) = SecurityHandler::for_writing(enc, &id0)?;
        min_version = match enc.method {
            Method::Aes256 => (2, 0),
            Method::Aes128 => (1, 6),
            Method::Rc4_128 => (1, 4),
        };
        security = Some(h);
        encrypt_dict = Some(d);
    }
    if use_objstm && min_version < (1, 5) {
        min_version = (1, 5);
    }
    let version = doc.version().max(min_version);
    doc.set_version(version);

    // Reachability, duplicate elimination and renumbering.
    let mut order = reachable(doc);
    let mut alias: HashMap<u32, u32> = HashMap::new();
    if opts.compress {
        alias = dedupe(doc, &mut order);
    }
    let mut remap: HashMap<u32, u32> = order
        .iter()
        .enumerate()
        .map(|(i, &old)| (old, i as u32 + 1))
        .collect();
    for (dup, canon) in &alias {
        if let Some(&n) = remap.get(canon) {
            remap.insert(*dup, n);
        }
    }
    let total = order.len() as u32;

    let mut out = Vec::with_capacity(64 * 1024);
    out.extend_from_slice(format!("%PDF-{}.{}\n", version.0, version.1).as_bytes());
    // Binary comment marker (bytes > 127 so transfer tools treat the file as binary).
    out.extend_from_slice(&[b'%', 0xE2, 0xE3, 0xCF, 0xD3, b'\n']);

    // Decide which objects go into object streams.
    let mut in_objstm: HashSet<u32> = HashSet::new();
    if use_objstm {
        for &old in &order {
            if !matches!(doc.get(ObjRef::new(old, 0)), Object::Stream(_)) {
                in_objstm.insert(old);
            }
        }
    }

    let mut offsets: HashMap<u32, usize> = HashMap::new(); // new num -> offset
    let mut compressed_loc: HashMap<u32, (u32, u32)> = HashMap::new(); // new num -> (objstm new num, idx)
    let mut next_extra = total + 1; // numbers for ObjStm / XRef / Encrypt objects

    // Direct objects.
    for &old in &order {
        if in_objstm.contains(&old) {
            continue;
        }
        let new = remap[&old];
        let r = ObjRef::new(new, 0);
        let mut obj = doc.get(ObjRef::new(old, 0)).clone();
        prepare_stream(&mut obj, doc, opts, level);
        if let Some(h) = &security {
            h.encrypt_object(&mut obj, r)?;
        }
        offsets.insert(new, out.len());
        write_indirect(&mut out, r, &obj, &remap);
    }

    // Object streams.
    if use_objstm {
        let members: Vec<u32> = order
            .iter()
            .copied()
            .filter(|o| in_objstm.contains(o))
            .collect();
        for chunk in members.chunks(OBJSTM_CAPACITY) {
            let stm_num = next_extra;
            next_extra += 1;
            let mut header = Vec::new();
            let mut body = Vec::new();
            for (i, &old) in chunk.iter().enumerate() {
                let new = remap[&old];
                let mut obj = doc.get(ObjRef::new(old, 0)).clone();
                if let Some(h) = &security {
                    // Strings inside object streams are encrypted as part of the
                    // stream, not individually. Nothing to do here, but if the
                    // object were a stream it could not be in an ObjStm.
                    let _ = h;
                }
                header.extend_from_slice(format!("{new} {} ", body.len()).as_bytes());
                serialize(&mut body, &obj, &remap);
                body.push(b'\n');
                compressed_loc.insert(new, (stm_num, i as u32));
                obj = Object::Null;
                let _ = obj;
            }
            let mut data = header;
            let first = data.len();
            data.extend_from_slice(&body);
            let mut dict = Dict::new();
            dict.set("Type", "ObjStm")
                .set("N", chunk.len())
                .set("First", first)
                .set("Filter", "FlateDecode");
            let mut stream_obj: Object =
                Stream::new(dict, filters::flate_encode(&data, level)).into();
            let r = ObjRef::new(stm_num, 0);
            if let Some(h) = &security {
                h.encrypt_object(&mut stream_obj, r)?;
            }
            offsets.insert(stm_num, out.len());
            write_indirect(&mut out, r, &stream_obj, &remap);
        }
    }

    // Encrypt dictionary (never encrypted, never in an object stream).
    let mut trailer = Dict::new();
    if let Some(d) = encrypt_dict {
        let num = next_extra;
        next_extra += 1;
        offsets.insert(num, out.len());
        write_indirect(&mut out, ObjRef::new(num, 0), &d.into(), &HashMap::new());
        trailer.set("Encrypt", ObjRef::new(num, 0));
    }
    if let Some(root) = doc.trailer().get("Root").and_then(Object::as_reference) {
        if let Some(&n) = remap.get(&root.num) {
            trailer.set("Root", ObjRef::new(n, 0));
        }
    }
    if let Some(info) = doc.trailer().get("Info").and_then(Object::as_reference) {
        if let Some(&n) = remap.get(&info.num) {
            trailer.set("Info", ObjRef::new(n, 0));
        }
    }
    if let Some(id) = doc.trailer().get("ID") {
        trailer.set("ID", id.clone());
    }

    let xref_pos = out.len();
    if use_objstm {
        let xref_num = next_extra;
        let size = xref_num + 1;
        trailer.set("Type", "XRef").set("Size", size as i64).set(
            "W",
            vec![Object::Integer(1), Object::Integer(4), Object::Integer(2)],
        );
        let mut rows = Vec::with_capacity(size as usize * 7);
        for n in 0..size {
            let (t, f2, f3): (u8, u32, u16) = if n == 0 {
                (0, 0, 65535)
            } else if let Some(&(s, i)) = compressed_loc.get(&n) {
                (2, s, i as u16)
            } else if let Some(&off) = offsets.get(&n) {
                (1, off as u32, 0)
            } else if n == xref_num {
                (1, xref_pos as u32, 0)
            } else {
                (0, 0, 0)
            };
            rows.push(t);
            rows.extend_from_slice(&f2.to_be_bytes());
            rows.extend_from_slice(&f3.to_be_bytes());
        }
        trailer.set("Filter", "FlateDecode");
        let stream: Object = Stream::new(trailer, filters::flate_encode(&rows, level)).into();
        write_indirect(&mut out, ObjRef::new(xref_num, 0), &stream, &HashMap::new());
    } else {
        let size = next_extra;
        out.extend_from_slice(format!("xref\n0 {size}\n").as_bytes());
        out.extend_from_slice(b"0000000000 65535 f \n");
        for n in 1..size {
            match offsets.get(&n) {
                Some(off) => out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes()),
                None => out.extend_from_slice(b"0000000000 65535 f \n"),
            }
        }
        trailer.set("Size", size as i64);
        out.extend_from_slice(b"trailer\n");
        serialize(&mut out, &trailer.into(), &HashMap::new());
        out.push(b'\n');
    }
    out.extend_from_slice(format!("startxref\n{xref_pos}\n%%EOF\n").as_bytes());
    Ok(out)
}

/// Finds indirect objects with identical content and maps duplicates onto
/// one canonical object. Only streams and font/graphics-state dictionaries
/// are considered: they are the objects that get duplicated when documents
/// are merged (the same embedded font in every input) and they carry no
/// identity of their own. Pages, the catalog and anything with a `/Parent`
/// are never merged. Runs to a fixed point so a font dictionary whose only
/// difference was a reference to a now-deduplicated font file collapses too.
fn dedupe(doc: &Document, order: &mut Vec<u32>) -> HashMap<u32, u32> {
    let mut alias: HashMap<u32, u32> = HashMap::new();
    for _ in 0..6 {
        // Keys must see every reference: unaliased objects map to themselves.
        // (Serialising through the alias map alone wrote unmapped references
        // as `null`, which made distinct fonts compare equal.)
        let remap: HashMap<u32, u32> = order
            .iter()
            .map(|&n| (n, *alias.get(&n).unwrap_or(&n)))
            .collect();
        let mut seen: HashMap<Vec<u8>, u32> = HashMap::new();
        let mut changed = false;
        for &num in order.iter() {
            if alias.contains_key(&num) {
                continue;
            }
            let obj = doc.get(ObjRef::new(num, 0));
            if !dedupable(obj) {
                continue;
            }
            let mut key = Vec::new();
            serialize(&mut key, obj, &remap);
            match seen.get(&key) {
                Some(&canon) if canon != num => {
                    alias.insert(num, canon);
                    changed = true;
                }
                Some(_) => {}
                None => {
                    seen.insert(key, num);
                }
            }
        }
        if !changed {
            break;
        }
    }
    order.retain(|n| !alias.contains_key(n));
    alias
}

fn dedupable(obj: &Object) -> bool {
    let dict = match obj {
        Object::Stream(s) => &s.dict,
        Object::Dict(d) => d,
        _ => return false,
    };
    if dict.contains("Parent") || dict.contains("Kids") || dict.contains("Annots") {
        return false;
    }
    let ty = dict
        .get("Type")
        .and_then(Object::as_name)
        .map(|n| n.as_str().into_owned());
    match ty.as_deref() {
        Some("Page") | Some("Pages") | Some("Catalog") | Some("XRef") | Some("ObjStm")
        | Some("Annot") => false,
        Some("Font")
        | Some("FontDescriptor")
        | Some("ExtGState")
        | Some("XObject")
        | Some("Encoding") => true,
        _ => matches!(obj, Object::Stream(_)),
    }
}

/// Objects reachable from the trailer, in discovery order (catalog first).
fn reachable(doc: &Document) -> Vec<u32> {
    let mut seen = HashSet::new();
    let mut order = Vec::new();
    let mut stack: Vec<ObjRef> = Vec::new();
    for key in ["Root", "Info"] {
        if let Some(r) = doc.trailer().get(key).and_then(Object::as_reference) {
            stack.push(r);
        }
    }
    while let Some(r) = stack.pop() {
        if !seen.insert(r.num) {
            continue;
        }
        let obj = doc.get(r);
        if obj.is_null() && !doc.objects().any(|(o, _)| o.num == r.num) {
            seen.remove(&r.num);
            continue;
        }
        order.push(r.num);
        collect_refs(obj, &mut stack);
    }
    order
}

fn collect_refs(obj: &Object, out: &mut Vec<ObjRef>) {
    match obj {
        Object::Reference(r) => out.push(*r),
        Object::Array(a) => a.iter().for_each(|o| collect_refs(o, out)),
        Object::Dict(d) => d.0.values().for_each(|o| collect_refs(o, out)),
        Object::Stream(s) => s.dict.0.values().for_each(|o| collect_refs(o, out)),
        _ => {}
    }
}

/// Recompresses a stream when that is allowed and beneficial.
fn prepare_stream(obj: &mut Object, doc: &Document, opts: &SaveOptions, level: u8) {
    let Object::Stream(s) = obj else { return };
    if !opts.compress {
        return;
    }
    if !filters::is_fully_decodable(s) {
        return;
    }
    let ty = s.dict.get("Type").and_then(Object::as_name);
    if ty.map(|n| n.is_any(&["Metadata"])).unwrap_or(false) {
        return;
    }
    let Ok(raw) = doc.stream_data(s) else { return };
    let filters_now = s.filters();
    let already_plain_flate = filters_now.len() == 1
        && filters_now[0] == "FlateDecode"
        && !s.dict.contains("DecodeParms")
        && !s.dict.contains("DP");
    let encoded = filters::flate_encode(&raw, level);
    if already_plain_flate && encoded.len() >= s.data.len() {
        return;
    }
    if encoded.len() >= raw.len() + 16 && filters_now.is_empty() && raw.len() < 64 {
        return; // Tiny uncompressible stream: leave as is.
    }
    s.dict.remove("DecodeParms");
    s.dict.remove("DP");
    s.dict.set("Filter", "FlateDecode");
    s.data = encoded;
}

fn write_indirect(out: &mut Vec<u8>, r: ObjRef, obj: &Object, remap: &HashMap<u32, u32>) {
    out.extend_from_slice(format!("{} {} obj\n", r.num, r.gen).as_bytes());
    serialize(out, obj, remap);
    out.extend_from_slice(b"\nendobj\n");
}

/// Writes `obj` in PDF syntax, translating references through `remap`
/// (references with no mapping are written as `null`).
pub fn serialize(out: &mut Vec<u8>, obj: &Object, remap: &HashMap<u32, u32>) {
    match obj {
        Object::Null => out.extend_from_slice(b"null"),
        Object::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        Object::Integer(i) => out.extend_from_slice(i.to_string().as_bytes()),
        Object::Real(r) => write_num(out, *r),
        Object::String(s) => {
            let binary = s
                .bytes
                .iter()
                .filter(|&&b| !(32..=126).contains(&b))
                .count();
            if s.hex || binary * 3 > s.bytes.len() {
                write_hex_string(out, &s.bytes);
            } else {
                write_literal_string(out, &s.bytes);
            }
        }
        Object::Name(n) => write_name(out, n),
        Object::Array(a) => {
            out.push(b'[');
            for (i, o) in a.iter().enumerate() {
                if i > 0 {
                    out.push(b' ');
                }
                serialize(out, o, remap);
            }
            out.push(b']');
        }
        Object::Dict(d) => serialize_dict(out, d, remap, None),
        Object::Stream(s) => {
            serialize_dict(out, &s.dict, remap, Some(s.data.len()));
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(&s.data);
            out.extend_from_slice(b"\nendstream");
        }
        Object::Reference(r) => {
            if remap.is_empty() {
                out.extend_from_slice(format!("{} {} R", r.num, r.gen).as_bytes());
            } else {
                match remap.get(&r.num) {
                    Some(n) => out.extend_from_slice(format!("{n} 0 R").as_bytes()),
                    None => out.extend_from_slice(b"null"),
                }
            }
        }
    }
}

fn serialize_dict(out: &mut Vec<u8>, d: &Dict, remap: &HashMap<u32, u32>, length: Option<usize>) {
    out.extend_from_slice(b"<<");
    for (k, v) in d.iter() {
        if length.is_some() && k == "Length" {
            continue;
        }
        write_name(out, k);
        // A delimiter-starting value needs no space; names/numbers do.
        match v {
            Object::Array(_) | Object::Dict(_) | Object::String(_) | Object::Name(_) => {}
            _ => out.push(b' '),
        }
        serialize(out, v, remap);
    }
    if let Some(len) = length {
        out.extend_from_slice(format!("/Length {len}").as_bytes());
    }
    out.extend_from_slice(b">>");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::PdfString;

    #[test]
    fn serialize_forms() {
        let mut d = Dict::new();
        d.set("A", 1)
            .set("B", Object::Real(2.5))
            .set("C", Object::String(PdfString::new(b"hi".to_vec())))
            .set("D", ObjRef::new(4, 0));
        let mut out = Vec::new();
        serialize(&mut out, &d.into(), &HashMap::new());
        assert_eq!(out, b"<</A 1/B 2.5/C(hi)/D 4 0 R>>");
    }

    #[test]
    fn dedupe_keeps_fonts_with_different_children() {
        use crate::object::PdfString;
        use crate::page::PageSize;
        let mut doc = Document::new();
        doc.add_page(PageSize::LETTER);
        // Two identical image streams: a genuine duplicate that seeds the alias map.
        let img = |d: &mut Document| {
            d.add(
                Stream::new(
                    Dict::new()
                        .with("Type", "XObject")
                        .with("Subtype", "Image")
                        .with("Width", 1)
                        .with("Height", 1)
                        .with("ColorSpace", "DeviceGray")
                        .with("BitsPerComponent", 8),
                    vec![7],
                )
                .into(),
            )
        };
        let (i1, i2) = (img(&mut doc), img(&mut doc));
        // Two fonts whose dictionaries differ only in the ToUnicode stream they point to.
        let tu =
            |d: &mut Document, body: &[u8]| d.add(Stream::new(Dict::new(), body.to_vec()).into());
        let (t1, t2) = (tu(&mut doc, b"map one"), tu(&mut doc, b"map two"));
        let font = |d: &mut Document, t: ObjRef| {
            d.add(
                Dict::new()
                    .with("Type", "Font")
                    .with("Subtype", "Type1")
                    .with("BaseFont", "Helvetica")
                    .with("ToUnicode", t)
                    .into(),
            )
        };
        let (f1, f2) = (font(&mut doc, t1), font(&mut doc, t2));
        let res = Dict::new()
            .with("Font", Dict::new().with("A", f1).with("B", f2))
            .with("XObject", Dict::new().with("I1", i1).with("I2", i2));
        let page = doc.page_ref(0).unwrap();
        doc.get_mut(page)
            .unwrap()
            .as_dict_mut()
            .unwrap()
            .set("Resources", res);
        let _ = PdfString::new(vec![]);
        let bytes = doc
            .save(&SaveOptions {
                compress: true,
                ..Default::default()
            })
            .unwrap();
        let re = Document::load(&bytes).unwrap();
        let page = re.page_ref(0).unwrap();
        let res = re
            .page_attr(page, "Resources")
            .unwrap()
            .as_dict()
            .unwrap()
            .clone();
        let fonts = re
            .dict_get(&res, "Font")
            .unwrap()
            .as_dict()
            .unwrap()
            .clone();
        let body = |name: &str| -> Vec<u8> {
            let f = re
                .resolve(fonts.get(name).unwrap())
                .as_dict()
                .unwrap()
                .clone();
            let t = re
                .resolve(f.get("ToUnicode").unwrap())
                .as_stream()
                .unwrap()
                .clone();
            re.stream_data(&t).unwrap()
        };
        assert_eq!(body("A"), b"map one");
        assert_eq!(
            body("B"),
            b"map two",
            "fonts with different ToUnicode maps must not be merged"
        );
        // The two images did merge.
        let xo = re
            .dict_get(&res, "XObject")
            .unwrap()
            .as_dict()
            .unwrap()
            .clone();
        assert_eq!(
            xo.get("I1").and_then(Object::as_reference),
            xo.get("I2").and_then(Object::as_reference)
        );
    }

    #[test]
    fn remap_and_dangling() {
        let mut remap = HashMap::new();
        remap.insert(4u32, 1u32);
        let mut out = Vec::new();
        serialize(
            &mut out,
            &Object::Array(vec![ObjRef::new(4, 0).into(), ObjRef::new(9, 0).into()]),
            &remap,
        );
        assert_eq!(out, b"[1 0 R null]");
    }
}
