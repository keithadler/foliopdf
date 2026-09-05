//! Image sample access shared by redaction and recompression: decoding
//! raw, Flate and JPEG image XObjects to packed samples, and encoding JPEG.

use crate::document::Document;
use crate::error::Result;
use crate::filters;
use crate::object::{Object, Stream};

/// Decoded image samples.
pub struct Pixels {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Bits per component.
    pub bpc: usize,
    /// Components per pixel.
    pub ncomp: usize,
    /// Packed rows.
    pub data: Vec<u8>,
    /// True when `data` came from a JPEG and should be re-encoded as one.
    pub jpeg: bool,
}

/// Number of components of a colour space object, if known.
pub fn color_components(doc: &Document, cs: Option<&Object>) -> Option<usize> {
    let cs = cs.map(|o| doc.resolve(o))?;
    match cs {
        Object::Name(n) => match n.as_str().as_ref() {
            "DeviceGray" | "G" | "CalGray" | "Indexed" | "I" | "Separation" | "Pattern" => Some(1),
            "DeviceRGB" | "RGB" | "CalRGB" | "Lab" => Some(3),
            "DeviceCMYK" | "CMYK" => Some(4),
            _ => None,
        },
        Object::Array(a) => {
            let fam = a
                .first()
                .map(|o| doc.resolve(o))
                .and_then(Object::as_name)
                .map(|n| n.as_str().into_owned())?;
            match fam.as_str() {
                "ICCBased" => a
                    .get(1)
                    .map(|o| doc.resolve(o))
                    .and_then(Object::as_stream)
                    .and_then(|s| doc.dict_get(&s.dict, "N"))
                    .and_then(Object::as_i64)
                    .map(|n| n as usize)
                    .or(Some(3)),
                "Indexed" | "I" | "Separation" | "CalGray" => Some(1),
                "CalRGB" | "Lab" => Some(3),
                "DeviceN" => a
                    .get(1)
                    .map(|o| doc.resolve(o))
                    .and_then(Object::as_array)
                    .map(|names| names.len()),
                "DeviceGray" => Some(1),
                "DeviceRGB" => Some(3),
                "DeviceCMYK" => Some(4),
                _ => None,
            }
        }
        _ => None,
    }
}

/// Decodes an image XObject to raw samples. `None` when the codec is not supported.
pub fn decode_pixels(doc: &Document, s: &Stream) -> Result<Option<Pixels>> {
    let d = &s.dict;
    let width = doc
        .dict_get(d, "Width")
        .or_else(|| doc.dict_get(d, "W"))
        .and_then(Object::as_i64)
        .unwrap_or(0)
        .max(0) as usize;
    let height = doc
        .dict_get(d, "Height")
        .or_else(|| doc.dict_get(d, "H"))
        .and_then(Object::as_i64)
        .unwrap_or(0)
        .max(0) as usize;
    if width == 0 || height == 0 || width.saturating_mul(height) > 80_000_000 {
        return Ok(None);
    }
    let is_mask = doc
        .dict_get(d, "ImageMask")
        .or_else(|| doc.dict_get(d, "IM"))
        .and_then(Object::as_bool)
        .unwrap_or(false);
    let data = doc.stream_data(s)?;
    let last_filter = s
        .filters()
        .last()
        .map(|n| n.as_str().into_owned())
        .unwrap_or_default();
    if filters::is_image_filter(&crate::object::Name::new(&last_filter)) {
        if last_filter == "DCTDecode" || last_filter == "DCT" {
            return Ok(decode_jpeg(&data));
        }
        return Ok(None);
    }
    let bpc = if is_mask {
        1
    } else {
        doc.dict_get(d, "BitsPerComponent")
            .or_else(|| doc.dict_get(d, "BPC"))
            .and_then(Object::as_i64)
            .unwrap_or(8) as usize
    };
    let ncomp = if is_mask {
        1
    } else {
        color_components(doc, d.get("ColorSpace").or_else(|| d.get("CS"))).unwrap_or(1)
    };
    if ![1, 2, 4, 8, 16].contains(&bpc) {
        return Ok(None);
    }
    let row = (width * ncomp * bpc).div_ceil(8);
    if data.len() < row * height {
        return Ok(None);
    }
    Ok(Some(Pixels {
        width,
        height,
        bpc,
        ncomp,
        data,
        jpeg: false,
    }))
}

/// Decodes a baseline or progressive JPEG to 8-bit samples.
#[cfg(feature = "jpeg")]
pub fn decode_jpeg(data: &[u8]) -> Option<Pixels> {
    let mut dec = jpeg_decoder::Decoder::new(data);
    let pixels = dec.decode().ok()?;
    let info = dec.info()?;
    let ncomp = match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 => 1,
        jpeg_decoder::PixelFormat::RGB24 => 3,
        jpeg_decoder::PixelFormat::CMYK32 => 4,
        jpeg_decoder::PixelFormat::L16 => return None,
    };
    Some(Pixels {
        width: info.width as usize,
        height: info.height as usize,
        bpc: 8,
        ncomp,
        data: pixels,
        jpeg: true,
    })
}

/// Decodes a JPEG (unavailable without the `jpeg` feature).
#[cfg(not(feature = "jpeg"))]
pub fn decode_jpeg(_data: &[u8]) -> Option<Pixels> {
    None
}

#[cfg(feature = "jpeg")]
/// Encodes 8-bit gray, RGB or CMYK samples as a baseline JPEG.
pub fn encode_jpeg(p: &Pixels, quality: u8) -> Option<Vec<u8>> {
    let ct = match p.ncomp {
        1 => jpeg_encoder::ColorType::Luma,
        3 => jpeg_encoder::ColorType::Rgb,
        4 => jpeg_encoder::ColorType::Cmyk,
        _ => return None,
    };
    let mut out = Vec::new();
    let enc = jpeg_encoder::Encoder::new(&mut out, quality.clamp(1, 100));
    enc.encode(&p.data, p.width as u16, p.height as u16, ct)
        .ok()?;
    Some(out)
}

/// Encodes a JPEG (unavailable without the `jpeg` feature).
#[cfg(not(feature = "jpeg"))]
pub fn encode_jpeg(_p: &Pixels, _quality: u8) -> Option<Vec<u8>> {
    None
}

/// Writes one sample of `bpc` bits at `index` (component index within the row).
pub fn set_sample(data: &mut [u8], row_bytes: usize, y: usize, index: usize, bpc: usize, val: u32) {
    let base = y * row_bytes;
    match bpc {
        8 => {
            if let Some(b) = data.get_mut(base + index) {
                *b = val as u8;
            }
        }
        16 => {
            let i = base + index * 2;
            if i + 1 < data.len() {
                data[i] = (val >> 8) as u8;
                data[i + 1] = val as u8;
            }
        }
        _ => {
            let bit = index * bpc;
            let i = base + bit / 8;
            if let Some(b) = data.get_mut(i) {
                let shift = 8 - bpc - (bit % 8);
                let mask = (((1u32 << bpc) - 1) as u8) << shift;
                *b = (*b & !mask) | (((val as u8) << shift) & mask);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "jpeg")]
    #[test]
    fn jpeg_roundtrip() {
        let mut out = Vec::new();
        let enc = jpeg_encoder::Encoder::new(&mut out, 90);
        let pixels: Vec<u8> = (0..8 * 8 * 3).map(|i| (i % 255) as u8).collect();
        enc.encode(&pixels, 8, 8, jpeg_encoder::ColorType::Rgb)
            .unwrap();
        let p = decode_jpeg(&out).unwrap();
        assert_eq!((p.width, p.height, p.ncomp), (8, 8, 3));
        assert!(encode_jpeg(&p, 85).is_some());
    }

    #[test]
    fn sample_setting() {
        let mut data = vec![0xFFu8; 2];
        set_sample(&mut data, 2, 0, 3, 1, 0); // 4th bit of first byte
        assert_eq!(data[0], 0b1110_1111);
        set_sample(&mut data, 2, 0, 1, 4, 0x3); // second nibble
        assert_eq!(data[0], 0b1110_0011);
    }
}
