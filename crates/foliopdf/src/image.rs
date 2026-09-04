//! Image loading for JPEG and PNG.
//!
//! JPEG data is embedded as-is with `DCTDecode`; only the header is parsed.
//! PNG files are decoded (they must be, since PDF has no PNG codec) and
//! re-encoded with `FlateDecode`. Alpha channels become a soft mask.

use crate::error::{Error, Result};
use crate::filters::{flate_decode, flate_encode};
use crate::object::{Dict, Object, PdfString, Stream};

/// Colour model of a decoded image.
#[derive(Debug, Clone, PartialEq)]
pub enum ColorSpace {
    /// One component.
    Gray,
    /// Three components.
    Rgb,
    /// Four components (JPEG only).
    Cmyk,
    /// Palette of RGB triplets; samples are indices.
    Indexed(Vec<u8>),
}

/// A loaded image ready to be turned into an image XObject.
#[derive(Debug, Clone)]
pub struct Image {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// Colour space of `data`.
    pub color_space: ColorSpace,
    /// Bits per component (1, 2, 4, 8).
    pub bits: u8,
    /// Sample data, either raw (PNG) or JPEG-encoded.
    pub data: Vec<u8>,
    /// Whether `data` is JPEG (`DCTDecode`) rather than raw samples.
    pub is_jpeg: bool,
    /// Adobe CMYK JPEGs are stored inverted; when set a `/Decode` array fixes it.
    pub inverted_cmyk: bool,
    /// 8-bit alpha samples, if any.
    pub alpha: Option<Vec<u8>>,
}

impl Image {
    /// Detects the format from magic bytes and decodes.
    pub fn load(bytes: &[u8]) -> Result<Self> {
        if bytes.starts_with(&[0xFF, 0xD8]) {
            Self::load_jpeg(bytes)
        } else if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
            Self::load_png(bytes)
        } else {
            Err(Error::Image(
                "unrecognised format (expected JPEG or PNG)".into(),
            ))
        }
    }

    /// Parses JPEG headers; the data is kept verbatim.
    pub fn load_jpeg(bytes: &[u8]) -> Result<Self> {
        let mut i = 2usize;
        let mut adobe_inverted = false;
        while i + 4 <= bytes.len() {
            if bytes[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = bytes[i + 1];
            if marker == 0xFF {
                i += 1;
                continue;
            }
            if marker == 0xD8 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
                i += 2;
                continue;
            }
            let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            if marker == 0xEE && i + 4 + 11 <= bytes.len() && &bytes[i + 4..i + 9] == b"Adobe" {
                // APP14 "Adobe" segment: Photoshop writes CMYK JPEGs with
                // inverted samples, whatever the transform flag says.
                adobe_inverted = true;
            }
            match marker {
                0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                    if i + 10 > bytes.len() {
                        break;
                    }
                    let bits = bytes[i + 4];
                    let height = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                    let width = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                    let comps = bytes[i + 9];
                    let color_space = match comps {
                        1 => ColorSpace::Gray,
                        3 => ColorSpace::Rgb,
                        4 => ColorSpace::Cmyk,
                        n => return Err(Error::Image(format!("JPEG with {n} components"))),
                    };
                    if width == 0 || height == 0 {
                        return Err(Error::Image("JPEG has zero size".into()));
                    }
                    return Ok(Image {
                        width,
                        height,
                        inverted_cmyk: comps == 4 && adobe_inverted,
                        color_space,
                        bits,
                        data: bytes.to_vec(),
                        is_jpeg: true,
                        alpha: None,
                    });
                }
                0xDA => break, // start of scan without SOF
                _ => {}
            }
            i += 2 + len;
        }
        Err(Error::Image("JPEG has no frame header".into()))
    }

    /// Decodes a PNG file.
    pub fn load_png(bytes: &[u8]) -> Result<Self> {
        let mut pos = 8usize;
        let (mut width, mut height, mut depth, mut ctype, mut interlace) =
            (0u32, 0u32, 0u8, 0u8, 0u8);
        let mut palette: Vec<u8> = Vec::new();
        let mut trns: Vec<u8> = Vec::new();
        let mut idat = Vec::new();
        while pos + 8 <= bytes.len() {
            let len =
                u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
                    as usize;
            let kind = &bytes[pos + 4..pos + 8];
            let start = pos + 8;
            let end = start
                .checked_add(len)
                .filter(|&e| e <= bytes.len())
                .ok_or_else(|| Error::Image("truncated PNG".into()))?;
            let chunk = &bytes[start..end];
            match kind {
                b"IHDR" => {
                    if chunk.len() < 13 {
                        return Err(Error::Image("bad IHDR".into()));
                    }
                    width = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    height = u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
                    depth = chunk[8];
                    ctype = chunk[9];
                    interlace = chunk[12];
                }
                b"PLTE" => palette = chunk.to_vec(),
                b"tRNS" => trns = chunk.to_vec(),
                b"IDAT" => idat.extend_from_slice(chunk),
                b"IEND" => break,
                _ => {}
            }
            pos = end + 4; // skip CRC
        }
        if width == 0 || height == 0 {
            return Err(Error::Image("PNG has no IHDR".into()));
        }
        if interlace != 0 {
            return Err(Error::Image(
                "interlaced PNG is not supported; re-save without Adam7".into(),
            ));
        }
        if (width as u64) * (height as u64) > 200_000_000 {
            return Err(Error::Limit("image larger than 200 megapixels".into()));
        }
        let channels = match ctype {
            0 => 1,
            2 => 3,
            3 => 1,
            4 => 2,
            6 => 4,
            t => return Err(Error::Image(format!("PNG colour type {t}"))),
        };
        let raw = flate_decode(&idat)?;
        let bpp_bits = channels * depth as usize;
        let row_len = (width as usize * bpp_bits).div_ceil(8);
        let bpp = bpp_bits.div_ceil(8).max(1);
        let mut pixels = Vec::with_capacity(row_len * height as usize);
        let mut prev = vec![0u8; row_len];
        let mut p = 0usize;
        for _ in 0..height {
            if p >= raw.len() {
                break;
            }
            let ft = raw[p];
            p += 1;
            let end = (p + row_len).min(raw.len());
            let mut row = raw[p..end].to_vec();
            row.resize(row_len, 0);
            p = end;
            for i in 0..row_len {
                let a = if i >= bpp { row[i - bpp] } else { 0 };
                let b = prev[i];
                let c = if i >= bpp { prev[i - bpp] } else { 0 };
                row[i] = match ft {
                    1 => row[i].wrapping_add(a),
                    2 => row[i].wrapping_add(b),
                    3 => row[i].wrapping_add(((a as u16 + b as u16) / 2) as u8),
                    4 => {
                        let pp = a as i16 + b as i16 - c as i16;
                        let (pa, pb, pc) = (
                            (pp - a as i16).abs(),
                            (pp - b as i16).abs(),
                            (pp - c as i16).abs(),
                        );
                        let pr = if pa <= pb && pa <= pc {
                            a
                        } else if pb <= pc {
                            b
                        } else {
                            c
                        };
                        row[i].wrapping_add(pr)
                    }
                    _ => row[i],
                };
            }
            pixels.extend_from_slice(&row);
            prev = row;
        }
        pixels.resize(row_len * height as usize, 0);

        // 16-bit samples: keep the high byte.
        let (pixels, depth) = if depth == 16 {
            (pixels.chunks(2).map(|c| c[0]).collect::<Vec<u8>>(), 8u8)
        } else {
            (pixels, depth)
        };

        let n = (width * height) as usize;
        match ctype {
            0 => {
                let alpha = if trns.len() >= 2 && depth == 8 {
                    let key = trns[1];
                    Some(
                        pixels
                            .iter()
                            .map(|&v| if v == key { 0 } else { 255 })
                            .collect(),
                    )
                } else {
                    None
                };
                Ok(Image {
                    width,
                    height,
                    color_space: ColorSpace::Gray,
                    bits: depth,
                    data: pixels,
                    is_jpeg: false,
                    inverted_cmyk: false,
                    alpha,
                })
            }
            2 => {
                let alpha = if trns.len() >= 6 && depth == 8 {
                    let key = [trns[1], trns[3], trns[5]];
                    Some(
                        pixels
                            .chunks(3)
                            .map(|px| if px == key { 0 } else { 255 })
                            .collect(),
                    )
                } else {
                    None
                };
                Ok(Image {
                    width,
                    height,
                    color_space: ColorSpace::Rgb,
                    bits: depth,
                    data: pixels,
                    is_jpeg: false,
                    inverted_cmyk: false,
                    alpha,
                })
            }
            3 => {
                if palette.is_empty() {
                    return Err(Error::Image("indexed PNG without palette".into()));
                }
                if trns.is_empty() {
                    return Ok(Image {
                        width,
                        height,
                        color_space: ColorSpace::Indexed(palette),
                        bits: depth,
                        data: pixels,
                        is_jpeg: false,
                        inverted_cmyk: false,
                        alpha: None,
                    });
                }
                // Palette with transparency: expand to RGB + alpha.
                let mut rgb = Vec::with_capacity(n * 3);
                let mut alpha = Vec::with_capacity(n);
                for idx in unpack_indices(&pixels, width as usize, height as usize, depth) {
                    let i = idx as usize;
                    rgb.extend_from_slice(palette.get(i * 3..i * 3 + 3).unwrap_or(&[0, 0, 0]));
                    alpha.push(trns.get(i).copied().unwrap_or(255));
                }
                Ok(Image {
                    width,
                    height,
                    color_space: ColorSpace::Rgb,
                    bits: 8,
                    data: rgb,
                    is_jpeg: false,
                    inverted_cmyk: false,
                    alpha: Some(alpha),
                })
            }
            4 => {
                let mut gray = Vec::with_capacity(n);
                let mut alpha = Vec::with_capacity(n);
                for px in pixels.chunks(2) {
                    gray.push(px[0]);
                    alpha.push(px[1]);
                }
                Ok(Image {
                    width,
                    height,
                    color_space: ColorSpace::Gray,
                    bits: 8,
                    data: gray,
                    is_jpeg: false,
                    inverted_cmyk: false,
                    alpha: Some(alpha),
                })
            }
            6 => {
                let mut rgb = Vec::with_capacity(n * 3);
                let mut alpha = Vec::with_capacity(n);
                for px in pixels.chunks(4) {
                    rgb.extend_from_slice(&px[..3]);
                    alpha.push(px[3]);
                }
                Ok(Image {
                    width,
                    height,
                    color_space: ColorSpace::Rgb,
                    bits: 8,
                    data: rgb,
                    is_jpeg: false,
                    inverted_cmyk: false,
                    alpha: Some(alpha),
                })
            }
            _ => unreachable!(),
        }
    }

    /// Builds the image XObject stream and, if the image has alpha, a soft
    /// mask stream. The caller must add the mask as an indirect object and set
    /// `/SMask` on the image dictionary to its reference.
    pub fn to_streams(&self, compression_level: u8) -> (Stream, Option<Stream>) {
        let mut dict = Dict::new();
        dict.set("Type", "XObject")
            .set("Subtype", "Image")
            .set("Width", self.width as i64)
            .set("Height", self.height as i64)
            .set("BitsPerComponent", self.bits as i64);
        match &self.color_space {
            ColorSpace::Gray => dict.set("ColorSpace", "DeviceGray"),
            ColorSpace::Rgb => dict.set("ColorSpace", "DeviceRGB"),
            ColorSpace::Cmyk => dict.set("ColorSpace", "DeviceCMYK"),
            ColorSpace::Indexed(p) => {
                let hival = (p.len() / 3).saturating_sub(1) as i64;
                dict.set(
                    "ColorSpace",
                    Object::Array(vec![
                        Object::name("Indexed"),
                        Object::name("DeviceRGB"),
                        Object::Integer(hival),
                        Object::String(PdfString::hex(p.clone())),
                    ]),
                )
            }
        };
        if self.inverted_cmyk {
            dict.set(
                "Decode",
                Object::Array(
                    [1, 0, 1, 0, 1, 0, 1, 0]
                        .iter()
                        .map(|&v| Object::Integer(v))
                        .collect(),
                ),
            );
        }
        let data = if self.is_jpeg {
            dict.set("Filter", "DCTDecode");
            self.data.clone()
        } else {
            dict.set("Filter", "FlateDecode");
            flate_encode(&self.data, compression_level)
        };
        let smask = self.alpha.as_ref().map(|a| {
            let mut d = Dict::new();
            d.set("Type", "XObject")
                .set("Subtype", "Image")
                .set("Width", self.width as i64)
                .set("Height", self.height as i64)
                .set("ColorSpace", "DeviceGray")
                .set("BitsPerComponent", 8)
                .set("Filter", "FlateDecode");
            Stream::new(d, flate_encode(a, compression_level))
        });
        (Stream::new(dict, data), smask)
    }
}

fn unpack_indices(pixels: &[u8], width: usize, height: usize, depth: u8) -> Vec<u8> {
    if depth == 8 {
        return pixels.to_vec();
    }
    let row_len = (width * depth as usize).div_ceil(8);
    let mut out = Vec::with_capacity(width * height);
    for row in pixels.chunks(row_len).take(height) {
        for x in 0..width {
            let bit = x * depth as usize;
            let byte = row.get(bit / 8).copied().unwrap_or(0);
            let shift = 8 - depth as usize - (bit % 8);
            out.push((byte >> shift) & ((1u16 << depth) - 1) as u8);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png_chunk(kind: &[u8], data: &[u8]) -> Vec<u8> {
        let mut v = (data.len() as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(data);
        v.extend_from_slice(&[0, 0, 0, 0]); // CRC not checked
        v
    }

    /// Builds a minimal 2×2 RGBA PNG (filter type 0 rows).
    fn tiny_png() -> Vec<u8> {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&2u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let raw = vec![
            0, 255, 0, 0, 255, 0, 255, 0, 128, //
            0, 0, 0, 255, 0, 10, 20, 30, 40,
        ];
        let idat = flate_encode(&raw, 6);
        let mut png = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        png.extend(png_chunk(b"IHDR", &ihdr));
        png.extend(png_chunk(b"IDAT", &idat));
        png.extend(png_chunk(b"IEND", &[]));
        png
    }

    #[test]
    fn png_rgba() {
        let img = Image::load(&tiny_png()).unwrap();
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.color_space, ColorSpace::Rgb);
        assert_eq!(img.data, vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 10, 20, 30]);
        assert_eq!(img.alpha.as_deref(), Some(&[255, 128, 0, 40][..]));
        let (s, m) = img.to_streams(6);
        assert_eq!(s.dict.get("Width").unwrap().as_i64(), Some(2));
        assert!(m.is_some());
    }

    #[test]
    fn jpeg_header() {
        // SOI, APP0 (empty-ish), SOF0 with 8 bits, 4x3, 3 components.
        let mut j = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        j.extend_from_slice(&[
            0xFF, 0xC0, 0x00, 0x11, 8, 0, 3, 0, 4, 3, 1, 0x22, 0, 2, 0x11, 1, 3, 0x11, 1,
        ]);
        let img = Image::load(&j).unwrap();
        assert_eq!((img.width, img.height), (4, 3));
        assert_eq!(img.color_space, ColorSpace::Rgb);
        assert!(img.is_jpeg);
    }

    #[test]
    fn unpack_bits() {
        assert_eq!(unpack_indices(&[0b1011_0000], 4, 1, 1), vec![1, 0, 1, 1]);
        assert_eq!(unpack_indices(&[0x12], 2, 1, 4), vec![1, 2]);
    }
}
