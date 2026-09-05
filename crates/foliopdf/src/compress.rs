//! Lossy image recompression: downsamples images that are larger than they
//! are displayed and re-encodes them as JPEG. This is what makes scanned
//! documents and photo-heavy files dramatically smaller; the lossless save
//! path only repacks streams.
//!
//! Only images that can be handled safely are touched: 8-bit grey and RGB
//! samples (raw, Flate or JPEG), with no colour-key mask or decode array.
//! Everything else (CMYK, indexed, 1-bit scans, JPEG 2000, fax) is left as it
//! is and listed in the report.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::error::Result;
use crate::filters;
use crate::imgcodec::{color_components, decode_pixels, encode_jpeg, Pixels};
use crate::object::{ObjRef, Object, Stream};
use crate::text;

/// Settings for [`compress_images`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImageOptions {
    /// Images displayed at more than this resolution are downsampled to it.
    /// Default 150 dpi (crisp on screen and in print at normal sizes).
    pub max_dpi: f64,
    /// JPEG quality 1–100. Default 75.
    pub quality: u8,
    /// Re-encode lossless (Flate) photos as JPEG too. Default true.
    pub convert_lossless: bool,
    /// Skip images with fewer pixels than this. Default 4096 (64 × 64).
    pub min_pixels: usize,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            max_dpi: 150.0,
            quality: 75,
            convert_lossless: true,
            min_pixels: 4096,
        }
    }
}

/// What [`compress_images`] did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageReport {
    /// Images examined.
    pub images: usize,
    /// Images re-encoded.
    pub recompressed: usize,
    /// Of those, images that were also downsampled.
    pub downsampled: usize,
    /// Encoded bytes of the examined images before.
    pub bytes_before: usize,
    /// Encoded bytes after.
    pub bytes_after: usize,
    /// Why images were left alone (deduplicated messages).
    pub skipped: Vec<String>,
}

/// Recompresses the document's images in place.
pub fn compress_images(doc: &mut Document, opts: &ImageOptions) -> Result<ImageReport> {
    let mut report = ImageReport::default();
    // Largest displayed size (points) of every image, over all pages.
    let mut display: HashMap<u32, (f64, f64)> = HashMap::new();
    for p in 0..doc.page_count() {
        let content = match text::page_content(doc, p) {
            Ok(c) => c,
            Err(_) => continue,
        };
        for im in content.images {
            if let Some(x) = im.xobject {
                let w = (im.ctm.a * im.ctm.a + im.ctm.b * im.ctm.b).sqrt();
                let h = (im.ctm.c * im.ctm.c + im.ctm.d * im.ctm.d).sqrt();
                let e = display.entry(x.num).or_insert((0.0, 0.0));
                e.0 = e.0.max(w);
                e.1 = e.1.max(h);
            }
        }
    }
    let mut skipped: HashMap<String, usize> = HashMap::new();
    let mut skip = |why: &str| *skipped.entry(why.to_string()).or_default() += 1;
    let images: Vec<ObjRef> = doc
        .objects()
        .filter(|(_, o)| {
            o.as_stream()
                .map(|s| {
                    s.dict
                        .get("Subtype")
                        .and_then(Object::as_name)
                        .map(|n| n == "Image")
                        .unwrap_or(false)
                })
                .unwrap_or(false)
        })
        .map(|(r, _)| r)
        .collect();
    for r in images {
        let s = match doc.get(r).as_stream() {
            Some(s) => s.clone(),
            None => continue,
        };
        report.images += 1;
        let before = s.data.len();
        report.bytes_before += before;
        let d = &s.dict;
        if doc
            .dict_get(d, "ImageMask")
            .and_then(Object::as_bool)
            .unwrap_or(false)
        {
            skip("1-bit image masks are kept");
            report.bytes_after += before;
            continue;
        }
        if d.contains("Mask") || d.contains("Decode") {
            skip("images with a colour-key mask or decode array are kept");
            report.bytes_after += before;
            continue;
        }
        let ncomp = color_components(doc, d.get("ColorSpace"));
        let cs_name = d
            .get("ColorSpace")
            .map(|o| doc.resolve(o))
            .and_then(|o| match o {
                Object::Name(n) => Some(n.as_str().into_owned()),
                Object::Array(a) => a
                    .first()
                    .and_then(Object::as_name)
                    .map(|n| n.as_str().into_owned()),
                _ => None,
            });
        let simple_cs = matches!(
            cs_name.as_deref(),
            Some("DeviceGray")
                | Some("DeviceRGB")
                | Some("ICCBased")
                | Some("CalRGB")
                | Some("CalGray")
        );
        if !simple_cs || !matches!(ncomp, Some(1) | Some(3)) {
            skip("only grey and RGB images are recompressed (CMYK, indexed and separation colours are kept)");
            report.bytes_after += before;
            continue;
        }
        let bpc = doc
            .dict_get(d, "BitsPerComponent")
            .and_then(Object::as_i64)
            .unwrap_or(8);
        if bpc != 8 {
            skip("only 8-bit images are recompressed (1-bit scans are already compact)");
            report.bytes_after += before;
            continue;
        }
        let mut px = match decode_pixels(doc, &s) {
            Ok(Some(p)) => p,
            _ => {
                skip("JPEG 2000, fax and JBIG2 images are kept");
                report.bytes_after += before;
                continue;
            }
        };
        if px.ncomp != ncomp.unwrap_or(0) {
            skip("image samples did not match the colour space");
            report.bytes_after += before;
            continue;
        }
        if px.width * px.height < opts.min_pixels {
            skip("small images are kept");
            report.bytes_after += before;
            continue;
        }
        // Target size from the largest placement.
        let mut target = (px.width, px.height);
        if let Some((dw, dh)) = display.get(&r.num) {
            if *dw > 1.0 && *dh > 1.0 {
                let tw = (dw / 72.0 * opts.max_dpi).round().max(1.0) as usize;
                let th = (dh / 72.0 * opts.max_dpi).round().max(1.0) as usize;
                if px.width as f64 > tw as f64 * 1.15 && px.height as f64 > th as f64 * 1.15 {
                    let f = (tw as f64 / px.width as f64).max(th as f64 / px.height as f64);
                    target = (
                        ((px.width as f64 * f).round() as usize).max(1),
                        ((px.height as f64 * f).round() as usize).max(1),
                    );
                }
            }
        }
        let resized = target != (px.width, px.height);
        if !resized && !px.jpeg && !opts.convert_lossless {
            skip("lossless images kept (convertLossless is off)");
            report.bytes_after += before;
            continue;
        }
        if resized {
            px = resample(&px, target.0, target.1);
        }
        if px.width > 65_000 || px.height > 65_000 {
            skip("image too large to encode");
            report.bytes_after += before;
            continue;
        }
        let encoded = match encode_jpeg(&px, opts.quality) {
            Some(e) => e,
            None => {
                skip("JPEG encoding unavailable");
                report.bytes_after += before;
                continue;
            }
        };
        // Keep the original when nothing is gained (already a lean JPEG).
        if !resized && encoded.len() as f64 > before as f64 * 0.9 {
            report.bytes_after += before;
            skip("already compact JPEGs are kept");
            continue;
        }
        // Soft mask: resample to match when possible.
        let smask = d.get("SMask").and_then(Object::as_reference);
        if let (true, Some(sm)) = (resized, smask) {
            if let Some(ms) = doc.get(sm).as_stream().cloned() {
                if let Ok(Some(mp)) = decode_pixels(doc, &ms) {
                    if mp.ncomp == 1
                        && mp.bpc == 8
                        && mp.width == s_dim(doc, &s, "Width")
                        && mp.height == s_dim(doc, &s, "Height")
                    {
                        let m2 = resample(&mp, px.width, px.height);
                        let mut md = ms.dict.clone();
                        md.remove("DecodeParms");
                        md.remove("DP");
                        md.set("Width", m2.width as i64);
                        md.set("Height", m2.height as i64);
                        md.set("BitsPerComponent", 8);
                        md.set("Filter", "FlateDecode");
                        doc.set(
                            sm,
                            Stream::new(md, filters::flate_encode(&m2.data, 6)).into(),
                        );
                    }
                }
            }
        }
        let mut nd = s.dict.clone();
        nd.remove("DecodeParms");
        nd.remove("DP");
        nd.remove("Length");
        nd.set("Width", px.width as i64);
        nd.set("Height", px.height as i64);
        nd.set("BitsPerComponent", 8);
        nd.set("Filter", "DCTDecode");
        report.bytes_after += encoded.len();
        report.recompressed += 1;
        if resized {
            report.downsampled += 1;
        }
        doc.set(r, Stream::new(nd, encoded).into());
    }
    let mut sk: Vec<(String, usize)> = skipped.into_iter().collect();
    sk.sort_by_key(|x| std::cmp::Reverse(x.1));
    report.skipped = sk
        .into_iter()
        .map(|(w, n)| if n > 1 { format!("{w} ({n})") } else { w })
        .collect();
    Ok(report)
}

fn s_dim(doc: &Document, s: &Stream, key: &str) -> usize {
    doc.dict_get(&s.dict, key)
        .and_then(Object::as_i64)
        .unwrap_or(0)
        .max(0) as usize
}

/// Area-averaging downsample (or nearest-neighbour upsample) of 8-bit samples.
pub fn resample(p: &Pixels, nw: usize, nh: usize) -> Pixels {
    let (w, h, n) = (p.width, p.height, p.ncomp);
    let mut out = vec![0u8; nw * nh * n];
    for y in 0..nh {
        let sy0 = y * h / nh;
        let sy1 = ((y + 1) * h).div_ceil(nh).max(sy0 + 1).min(h);
        for x in 0..nw {
            let sx0 = x * w / nw;
            let sx1 = ((x + 1) * w).div_ceil(nw).max(sx0 + 1).min(w);
            let count = ((sy1 - sy0) * (sx1 - sx0)) as u32;
            for c in 0..n {
                let mut sum = 0u32;
                for sy in sy0..sy1 {
                    let row = sy * w * n;
                    for sx in sx0..sx1 {
                        sum += p.data[row + sx * n + c] as u32;
                    }
                }
                out[(y * nw + x) * n + c] = ((sum + count / 2) / count) as u8;
            }
        }
    }
    Pixels {
        width: nw,
        height: nh,
        bpc: 8,
        ncomp: n,
        data: out,
        jpeg: p.jpeg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::Dict;
    use crate::page::PageSize;

    fn photo(w: usize, h: usize) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                v.push((x * 255 / w) as u8);
                v.push((y * 255 / h) as u8);
                v.push(((x + y) % 256) as u8);
            }
        }
        v
    }

    #[test]
    fn downsamples_oversized_photo() {
        let mut d = Document::new();
        d.add_page(PageSize::LETTER);
        let (w, h) = (1200, 900);
        let img = d.add(
            Stream::new(
                Dict::new()
                    .with("Type", "XObject")
                    .with("Subtype", "Image")
                    .with("Width", w as i64)
                    .with("Height", h as i64)
                    .with("ColorSpace", "DeviceRGB")
                    .with("BitsPerComponent", 8)
                    .with("Filter", "FlateDecode"),
                filters::flate_encode(&photo(w, h), 6),
            )
            .into(),
        );
        let name = d.add_page_resource(0, "XObject", img).unwrap();
        // Displayed 3 inches wide: 1200 px is 400 dpi, well over 150.
        d.draw(
            0,
            format!("q 216 0 0 162 72 500 cm /{name} Do Q").as_bytes(),
        )
        .unwrap();
        let before = d.get(img).as_stream().unwrap().data.len();
        let rep = compress_images(&mut d, &ImageOptions::default()).unwrap();
        assert_eq!(
            (rep.images, rep.recompressed, rep.downsampled),
            (1, 1, 1),
            "{rep:?}"
        );
        let s = d.get(img).as_stream().unwrap();
        assert_eq!(
            s.dict.get("Filter").and_then(Object::as_name).unwrap(),
            "DCTDecode"
        );
        let nw = s.dict.get("Width").and_then(Object::as_i64).unwrap();
        assert!(
            (440..=460).contains(&nw),
            "216pt at 150dpi = 450px, got {nw}"
        );
        assert!(s.data.len() < before / 4, "{} -> {}", before, s.data.len());
        assert!(rep.bytes_after < rep.bytes_before);
        // It still decodes and the document still saves.
        let px = decode_pixels(&d, s).unwrap().unwrap();
        assert_eq!((px.width as i64, px.ncomp), (nw, 3));
        Document::load(&d.save(&Default::default()).unwrap()).unwrap();
    }

    #[test]
    fn leaves_unsupported_and_tiny_images() {
        let mut d = Document::new();
        d.add_page(PageSize::LETTER);
        let mask = d.add(
            Stream::new(
                Dict::new()
                    .with("Type", "XObject")
                    .with("Subtype", "Image")
                    .with("Width", 8)
                    .with("Height", 8)
                    .with("ImageMask", true),
                vec![0; 8],
            )
            .into(),
        );
        let tiny = d.add(
            Stream::new(
                Dict::new()
                    .with("Type", "XObject")
                    .with("Subtype", "Image")
                    .with("Width", 4)
                    .with("Height", 4)
                    .with("ColorSpace", "DeviceGray")
                    .with("BitsPerComponent", 8),
                vec![9; 16],
            )
            .into(),
        );
        let cmyk = d.add(
            Stream::new(
                Dict::new()
                    .with("Type", "XObject")
                    .with("Subtype", "Image")
                    .with("Width", 100)
                    .with("Height", 100)
                    .with("ColorSpace", "DeviceCMYK")
                    .with("BitsPerComponent", 8),
                vec![9; 40000],
            )
            .into(),
        );
        for r in [mask, tiny, cmyk] {
            d.add_page_resource(0, "XObject", r).unwrap();
        }
        let rep = compress_images(&mut d, &ImageOptions::default()).unwrap();
        assert_eq!(rep.images, 3);
        assert_eq!(rep.recompressed, 0);
        assert_eq!(rep.skipped.len(), 3, "{:?}", rep.skipped);
        assert_eq!(rep.bytes_after, rep.bytes_before);
    }

    #[test]
    fn resample_averages() {
        let p = Pixels {
            width: 4,
            height: 2,
            bpc: 8,
            ncomp: 1,
            data: vec![0, 100, 200, 250, 0, 100, 200, 250],
            jpeg: false,
        };
        let r = resample(&p, 2, 1);
        assert_eq!(r.data, vec![50, 225]);
    }
}
