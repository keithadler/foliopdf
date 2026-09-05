//! Pulling images out of a document as ordinary files.

use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::error::Result;
use crate::filters;
use crate::imgcodec::{color_components, decode_pixels};
use crate::object::{ObjRef, Object};
use crate::text;

/// An image taken out of a page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractedImage {
    /// Page the image was found on (0-based).
    pub page: usize,
    /// Pixel width.
    pub width: usize,
    /// Pixel height.
    pub height: usize,
    /// `"jpeg"` or `"png"`.
    pub format: String,
    /// File bytes.
    pub data: Vec<u8>,
}

/// Extracts the images drawn on `pages`, each once, in page order. JPEG
/// data is passed through untouched; other decodable images become PNG.
/// Images in unsupported encodings (JPEG 2000, fax, JBIG2) are skipped.
pub fn extract_images(doc: &Document, pages: &[usize]) -> Result<Vec<ExtractedImage>> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for &p in pages {
        let content = text::page_content(doc, p)?;
        for im in content.images {
            let xref: ObjRef = match im.xobject {
                Some(r) => r,
                None => continue,
            };
            if !seen.insert(xref.num) {
                continue;
            }
            if let Some(e) = extract_one(doc, xref, p) {
                out.push(e);
            }
        }
    }
    Ok(out)
}

fn extract_one(doc: &Document, xref: ObjRef, page: usize) -> Option<ExtractedImage> {
    let s = doc.get(xref).as_stream()?;
    let d = &s.dict;
    let width = doc.dict_get(d, "Width").and_then(Object::as_i64)? as usize;
    let height = doc.dict_get(d, "Height").and_then(Object::as_i64)? as usize;
    if width == 0 || height == 0 {
        return None;
    }
    let last = s
        .filters()
        .last()
        .map(|n| n.as_str().into_owned())
        .unwrap_or_default();
    let ncomp = color_components(doc, d.get("ColorSpace"));
    if (last == "DCTDecode" || last == "DCT")
        && d.get("Decode").is_none()
        && matches!(ncomp, Some(1) | Some(3))
    {
        let data = doc.stream_data(s).ok()?;
        return Some(ExtractedImage {
            page,
            width,
            height,
            format: "jpeg".into(),
            data,
        });
    }
    let px = decode_pixels(doc, s).ok()??;
    let is_mask = doc
        .dict_get(d, "ImageMask")
        .and_then(Object::as_bool)
        .unwrap_or(false);
    // Convert to 8-bit grey or RGB rows.
    let (ncomp, rows): (usize, Vec<u8>) = if is_mask || (px.ncomp == 1 && px.bpc == 1) {
        (
            1,
            unpack_bits(&px.data, px.width, px.height, 1, 1)
                .into_iter()
                .map(|v| if v == 0 { 0 } else { 255 })
                .collect(),
        )
    } else if px.bpc == 8 && (px.ncomp == 1 || px.ncomp == 3) {
        (px.ncomp, px.data.clone())
    } else if px.ncomp == 1 && (px.bpc == 2 || px.bpc == 4) {
        let max = ((1u32 << px.bpc) - 1) as u32;
        (
            1,
            unpack_bits(&px.data, px.width, px.height, 1, px.bpc)
                .into_iter()
                .map(|v| (v * 255 / max) as u8)
                .collect(),
        )
    } else if px.ncomp == 1 && px.bpc == 8 {
        (1, px.data.clone())
    } else {
        return None;
    };
    // Indexed colour: expand the palette.
    let (ncomp, rows) = match indexed_palette(doc, d.get("ColorSpace")) {
        Some(pal) if ncomp == 1 => {
            let mut rgb = Vec::with_capacity(rows.len() * 3);
            for &i in &rows {
                let k = i as usize * 3;
                rgb.extend_from_slice(pal.get(k..k + 3).unwrap_or(&[0, 0, 0]));
            }
            (3, rgb)
        }
        _ => (ncomp, rows),
    };
    let data = encode_png(width, height, ncomp, &rows)?;
    Some(ExtractedImage {
        page,
        width,
        height,
        format: "png".into(),
        data,
    })
}

fn indexed_palette(doc: &Document, cs: Option<&Object>) -> Option<Vec<u8>> {
    let a = doc.resolve(cs?).as_array()?;
    let fam = doc.resolve(a.first()?).as_name()?.as_str().into_owned();
    if fam != "Indexed" && fam != "I" {
        return None;
    }
    let base = doc.resolve(a.get(1)?);
    let base_n = color_components(doc, Some(base)).unwrap_or(3);
    let lookup = match doc.resolve(a.get(3)?) {
        Object::String(s) => s.as_bytes().to_vec(),
        Object::Stream(st) => doc.stream_data(st).ok()?,
        _ => return None,
    };
    Some(match base_n {
        3 => lookup,
        1 => lookup.iter().flat_map(|&g| [g, g, g]).collect(),
        4 => lookup
            .chunks(4)
            .flat_map(|c| {
                let k = 255 - c.get(3).copied().unwrap_or(0) as u32;
                [
                    ((255 - c[0] as u32) * k / 255) as u8,
                    ((255 - c[1] as u32) * k / 255) as u8,
                    ((255 - c[2] as u32) * k / 255) as u8,
                ]
            })
            .collect(),
        _ => return None,
    })
}

fn unpack_bits(data: &[u8], w: usize, h: usize, ncomp: usize, bpc: usize) -> Vec<u32> {
    let row_bytes = (w * ncomp * bpc).div_ceil(8);
    let mut out = Vec::with_capacity(w * h * ncomp);
    for y in 0..h {
        for i in 0..w * ncomp {
            let bit = i * bpc;
            let byte = data.get(y * row_bytes + bit / 8).copied().unwrap_or(0);
            let shift = 8 - bpc - (bit % 8);
            out.push(((byte >> shift) as u32) & ((1 << bpc) - 1));
        }
    }
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
    }
    !c
}

/// Encodes 8-bit grey (`ncomp` 1) or RGB (`ncomp` 3) rows as a PNG.
pub fn encode_png(width: usize, height: usize, ncomp: usize, rows: &[u8]) -> Option<Vec<u8>> {
    if rows.len() < width * height * ncomp || !(ncomp == 1 || ncomp == 3) {
        return None;
    }
    let mut raw = Vec::with_capacity(height * (width * ncomp + 1));
    for y in 0..height {
        raw.push(0);
        raw.extend_from_slice(&rows[y * width * ncomp..(y + 1) * width * ncomp]);
    }
    let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut chunk = |kind: &[u8], body: &[u8]| {
        png.extend_from_slice(&(body.len() as u32).to_be_bytes());
        let mut c = kind.to_vec();
        c.extend_from_slice(body);
        png.extend_from_slice(&c);
        png.extend_from_slice(&crc32(&c).to_be_bytes());
    };
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, if ncomp == 3 { 2 } else { 0 }, 0, 0, 0]);
    chunk(b"IHDR", &ihdr);
    chunk(b"IDAT", &filters::flate_encode(&raw, 6));
    chunk(b"IEND", &[]);
    Some(png)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::Image;
    use crate::page::PageSize;

    #[test]
    fn png_roundtrip_and_extract() {
        let png = crate::image::tests::tiny_png();
        let mut d = Document::new();
        d.add_page(PageSize::LETTER);
        let img = d.add_image(&Image::load(&png).unwrap(), 6);
        let name = d.add_page_resource(0, "XObject", img).unwrap();
        d.draw(
            0,
            format!("q 100 0 0 100 50 50 cm /{name} Do Q q 10 0 0 10 300 300 cm /{name} Do Q")
                .as_bytes(),
        )
        .unwrap();
        let d = Document::load(&d.save(&Default::default()).unwrap()).unwrap();
        let out = extract_images(&d, &[0]).unwrap();
        assert_eq!(out.len(), 1, "same image drawn twice is extracted once");
        assert_eq!(
            (out[0].width, out[0].height, out[0].format.as_str()),
            (2, 2, "png")
        );
        let back = Image::load(&out[0].data).unwrap();
        assert_eq!((back.width, back.height), (2, 2));
        assert_eq!(&back.data[..3], &[255, 0, 0], "first pixel red");
    }

    #[test]
    fn crc_matches_known_value() {
        assert_eq!(crc32(b"IEND"), 0xAE42_6082);
    }
}
