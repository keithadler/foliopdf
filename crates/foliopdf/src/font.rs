//! Fonts: the standard 14 (no embedding needed) and embedded TrueType.
//!
//! Standard fonts are encoded as WinAnsi single bytes and measured with the
//! Adobe AFM metrics. TrueType fonts are embedded as `CIDFontType2` with
//! `Identity-H` encoding: text is encoded as 16-bit glyph ids, and only the
//! glyphs actually used are kept in the embedded font program.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::error::{Error, Result};
use crate::filters::flate_encode;
use crate::object::{Dict, ObjRef, Object, Stream};

/// One of the 12 standard text fonts every PDF viewer provides. (Symbol and
/// ZapfDingbats are omitted; they use non-text encodings.)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[allow(missing_docs)]
pub enum StandardFont {
    Helvetica,
    HelveticaBold,
    HelveticaOblique,
    HelveticaBoldOblique,
    TimesRoman,
    TimesBold,
    TimesItalic,
    TimesBoldItalic,
    Courier,
    CourierBold,
    CourierOblique,
    CourierBoldOblique,
}

impl StandardFont {
    /// The PostScript `/BaseFont` name.
    pub fn base_font(&self) -> &'static str {
        use StandardFont::*;
        match self {
            Helvetica => "Helvetica",
            HelveticaBold => "Helvetica-Bold",
            HelveticaOblique => "Helvetica-Oblique",
            HelveticaBoldOblique => "Helvetica-BoldOblique",
            TimesRoman => "Times-Roman",
            TimesBold => "Times-Bold",
            TimesItalic => "Times-Italic",
            TimesBoldItalic => "Times-BoldItalic",
            Courier => "Courier",
            CourierBold => "Courier-Bold",
            CourierOblique => "Courier-Oblique",
            CourierBoldOblique => "Courier-BoldOblique",
        }
    }

    /// Looks a font up by name, accepting common aliases (`Arial`, `Times New
    /// Roman`, `Courier New`, with `Bold`/`Italic` suffixes).
    pub fn by_name(name: &str) -> Option<Self> {
        use StandardFont::*;
        let n = name.to_ascii_lowercase().replace([' ', '-', '_', ','], "");
        let bold = n.contains("bold");
        let italic = n.contains("italic") || n.contains("oblique");
        let family = if n.starts_with("helvetica") || n.starts_with("arial") {
            0
        } else if n.starts_with("times") {
            1
        } else if n.starts_with("courier") {
            2
        } else {
            return None;
        };
        Some(match (family, bold, italic) {
            (0, false, false) => Helvetica,
            (0, true, false) => HelveticaBold,
            (0, false, true) => HelveticaOblique,
            (0, true, true) => HelveticaBoldOblique,
            (1, false, false) => TimesRoman,
            (1, true, false) => TimesBold,
            (1, false, true) => TimesItalic,
            (1, true, true) => TimesBoldItalic,
            (_, false, false) => Courier,
            (_, true, false) => CourierBold,
            (_, false, true) => CourierOblique,
            _ => CourierBoldOblique,
        })
    }

    /// Advance width of WinAnsi code `code` in 1/1000 em.
    pub fn width(&self, code: u8) -> u16 {
        use StandardFont::*;
        let (ascii, hi): (&[u16; 95], &[(u8, u16)]) = match self {
            Courier | CourierBold | CourierOblique | CourierBoldOblique => return 600,
            Helvetica | HelveticaOblique => (&metrics::HELVETICA, &metrics::HELVETICA_HI),
            HelveticaBold | HelveticaBoldOblique => {
                (&metrics::HELVETICA_BOLD, &metrics::HELVETICA_BOLD_HI)
            }
            TimesRoman => (&metrics::TIMES, &metrics::TIMES_HI),
            TimesBold => (&metrics::TIMES_BOLD, &metrics::TIMES_BOLD_HI),
            TimesItalic => (&metrics::TIMES_ITALIC, &metrics::TIMES_ITALIC_HI),
            TimesBoldItalic => (&metrics::TIMES_BOLD_ITALIC, &metrics::TIMES_BOLD_ITALIC_HI),
        };
        if (32..127).contains(&code) {
            return ascii[(code - 32) as usize];
        }
        if let Some(b) = metrics::accent_base(code) {
            return ascii[(b - 32) as usize];
        }
        hi.iter()
            .find(|(c, _)| *c == code)
            .map(|(_, w)| *w)
            .unwrap_or(ascii[0])
    }
}

/// Maps a char to its WinAnsi code, or `None` if unrepresentable.
pub fn winansi_encode(c: char) -> Option<u8> {
    let u = c as u32;
    match u {
        0x20..=0x7E => Some(u as u8),
        0xA0..=0xFF => Some(u as u8),
        0x20AC => Some(128),
        0x201A => Some(130),
        0x0192 => Some(131),
        0x201E => Some(132),
        0x2026 => Some(133),
        0x2020 => Some(134),
        0x2021 => Some(135),
        0x02C6 => Some(136),
        0x2030 => Some(137),
        0x0160 => Some(138),
        0x2039 => Some(139),
        0x0152 => Some(140),
        0x017D => Some(142),
        0x2018 => Some(145),
        0x2019 => Some(146),
        0x201C => Some(147),
        0x201D => Some(148),
        0x2022 => Some(149),
        0x2013 => Some(150),
        0x2014 => Some(151),
        0x02DC => Some(152),
        0x2122 => Some(153),
        0x0161 => Some(154),
        0x203A => Some(155),
        0x0153 => Some(156),
        0x017E => Some(158),
        0x0178 => Some(159),
        _ => None,
    }
}

mod metrics {
    /// Codes 192–255 that are accented forms of an ASCII letter.
    pub fn accent_base(code: u8) -> Option<u8> {
        Some(match code {
            192..=197 => b'A',
            199 => b'C',
            200..=203 => b'E',
            204..=207 => b'I',
            209 => b'N',
            210..=214 => b'O',
            217..=220 => b'U',
            221 => b'Y',
            224..=229 => b'a',
            231 => b'c',
            232..=235 => b'e',
            236..=239 => b'i',
            241 => b'n',
            242..=246 => b'o',
            249..=252 => b'u',
            253 | 255 => b'y',
            160 => b' ',
            173 => b'-',
            _ => return None,
        })
    }

    pub const HELVETICA: [u16; 95] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556,
        556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722,
        722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722,
        667, 944, 667, 667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556,
        556, 222, 222, 500, 222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500,
        500, 334, 260, 334, 584,
    ];
    pub const HELVETICA_BOLD: [u16; 95] = [
        278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556,
        556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611, 975, 722, 722, 722,
        722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778, 667, 778, 722, 667, 611, 722,
        667, 944, 667, 667, 611, 333, 278, 333, 584, 556, 333, 556, 611, 556, 611, 556, 333, 611,
        611, 278, 278, 556, 278, 889, 611, 611, 611, 611, 389, 556, 333, 611, 556, 778, 556, 556,
        500, 389, 280, 389, 584,
    ];
    pub const TIMES: [u16; 95] = [
        250, 333, 408, 500, 500, 833, 778, 180, 333, 333, 500, 564, 250, 333, 250, 278, 500, 500,
        500, 500, 500, 500, 500, 500, 500, 500, 278, 278, 564, 564, 564, 444, 921, 722, 667, 667,
        722, 611, 556, 722, 722, 333, 389, 722, 611, 889, 722, 722, 556, 722, 667, 556, 611, 722,
        722, 944, 722, 722, 611, 333, 278, 333, 469, 500, 333, 444, 500, 444, 500, 444, 333, 500,
        500, 278, 278, 500, 278, 778, 500, 500, 500, 500, 333, 389, 278, 500, 500, 722, 500, 500,
        444, 480, 200, 480, 541,
    ];
    pub const TIMES_BOLD: [u16; 95] = [
        250, 333, 555, 500, 500, 1000, 833, 278, 333, 333, 500, 570, 250, 333, 250, 278, 500, 500,
        500, 500, 500, 500, 500, 500, 500, 500, 333, 333, 570, 570, 570, 500, 930, 722, 667, 722,
        722, 667, 611, 778, 778, 389, 500, 778, 667, 944, 722, 778, 611, 778, 722, 556, 667, 722,
        722, 1000, 722, 722, 667, 333, 278, 333, 581, 500, 333, 500, 556, 444, 556, 444, 333, 500,
        556, 278, 333, 556, 278, 833, 556, 500, 556, 556, 444, 389, 333, 556, 500, 722, 500, 500,
        444, 394, 220, 394, 520,
    ];
    pub const TIMES_ITALIC: [u16; 95] = [
        250, 333, 420, 500, 500, 833, 778, 214, 333, 333, 500, 675, 250, 333, 250, 278, 500, 500,
        500, 500, 500, 500, 500, 500, 500, 500, 333, 333, 675, 675, 675, 500, 920, 611, 611, 667,
        722, 611, 611, 722, 722, 333, 444, 667, 556, 833, 667, 722, 611, 722, 611, 500, 556, 722,
        611, 833, 611, 556, 556, 389, 278, 389, 422, 500, 333, 500, 500, 444, 500, 444, 278, 500,
        500, 278, 278, 444, 278, 722, 500, 500, 500, 500, 389, 389, 278, 500, 444, 667, 444, 444,
        389, 400, 275, 400, 541,
    ];
    pub const TIMES_BOLD_ITALIC: [u16; 95] = [
        250, 389, 555, 500, 500, 833, 778, 278, 333, 333, 500, 570, 250, 333, 250, 278, 500, 500,
        500, 500, 500, 500, 500, 500, 500, 500, 333, 333, 570, 570, 570, 500, 832, 667, 667, 667,
        722, 667, 667, 722, 778, 389, 500, 667, 611, 889, 722, 722, 611, 722, 667, 556, 611, 722,
        667, 889, 667, 611, 611, 333, 278, 333, 570, 500, 333, 500, 500, 444, 500, 444, 333, 500,
        556, 278, 278, 500, 278, 778, 556, 500, 500, 500, 389, 389, 278, 556, 444, 667, 500, 444,
        389, 348, 220, 348, 570,
    ];

    // (WinAnsi code, width) for symbols and ligatures above 127.
    pub const HELVETICA_HI: [(u8, u16); 71] = [
        (128, 556),
        (130, 222),
        (131, 556),
        (132, 333),
        (133, 1000),
        (134, 556),
        (135, 556),
        (136, 333),
        (137, 1000),
        (138, 667),
        (139, 333),
        (140, 1000),
        (142, 611),
        (145, 222),
        (146, 222),
        (147, 333),
        (148, 333),
        (149, 350),
        (150, 556),
        (151, 1000),
        (152, 333),
        (153, 1000),
        (154, 500),
        (155, 333),
        (156, 944),
        (158, 500),
        (159, 667),
        (161, 333),
        (162, 556),
        (163, 556),
        (164, 556),
        (165, 556),
        (166, 260),
        (167, 556),
        (168, 333),
        (169, 737),
        (170, 370),
        (171, 556),
        (172, 584),
        (174, 737),
        (175, 333),
        (176, 400),
        (177, 584),
        (178, 333),
        (179, 333),
        (180, 333),
        (181, 556),
        (182, 537),
        (183, 278),
        (184, 333),
        (185, 333),
        (186, 365),
        (187, 556),
        (188, 834),
        (189, 834),
        (190, 834),
        (191, 611),
        (198, 1000),
        (208, 722),
        (215, 584),
        (216, 778),
        (222, 667),
        (223, 611),
        (230, 889),
        (240, 556),
        (247, 584),
        (248, 611),
        (254, 556),
        (0, 0),
        (0, 0),
        (0, 0),
    ];
    pub const HELVETICA_BOLD_HI: [(u8, u16); 71] = [
        (128, 556),
        (130, 278),
        (131, 556),
        (132, 500),
        (133, 1000),
        (134, 556),
        (135, 556),
        (136, 333),
        (137, 1000),
        (138, 667),
        (139, 333),
        (140, 1000),
        (142, 611),
        (145, 278),
        (146, 278),
        (147, 500),
        (148, 500),
        (149, 350),
        (150, 556),
        (151, 1000),
        (152, 333),
        (153, 1000),
        (154, 556),
        (155, 333),
        (156, 944),
        (158, 500),
        (159, 667),
        (161, 333),
        (162, 556),
        (163, 556),
        (164, 556),
        (165, 556),
        (166, 280),
        (167, 556),
        (168, 333),
        (169, 737),
        (170, 370),
        (171, 556),
        (172, 584),
        (174, 737),
        (175, 333),
        (176, 400),
        (177, 584),
        (178, 333),
        (179, 333),
        (180, 333),
        (181, 611),
        (182, 556),
        (183, 278),
        (184, 333),
        (185, 333),
        (186, 365),
        (187, 556),
        (188, 834),
        (189, 834),
        (190, 834),
        (191, 611),
        (198, 1000),
        (208, 722),
        (215, 584),
        (216, 778),
        (222, 667),
        (223, 611),
        (230, 889),
        (240, 611),
        (247, 584),
        (248, 611),
        (254, 611),
        (0, 0),
        (0, 0),
        (0, 0),
    ];
    pub const TIMES_HI: [(u8, u16); 71] = [
        (128, 500),
        (130, 333),
        (131, 500),
        (132, 444),
        (133, 1000),
        (134, 500),
        (135, 500),
        (136, 333),
        (137, 1000),
        (138, 556),
        (139, 333),
        (140, 889),
        (142, 611),
        (145, 333),
        (146, 333),
        (147, 444),
        (148, 444),
        (149, 350),
        (150, 500),
        (151, 1000),
        (152, 333),
        (153, 980),
        (154, 389),
        (155, 333),
        (156, 722),
        (158, 444),
        (159, 722),
        (161, 333),
        (162, 500),
        (163, 500),
        (164, 500),
        (165, 500),
        (166, 200),
        (167, 500),
        (168, 333),
        (169, 760),
        (170, 276),
        (171, 500),
        (172, 564),
        (174, 760),
        (175, 333),
        (176, 400),
        (177, 564),
        (178, 300),
        (179, 300),
        (180, 333),
        (181, 500),
        (182, 453),
        (183, 250),
        (184, 333),
        (185, 300),
        (186, 310),
        (187, 500),
        (188, 750),
        (189, 750),
        (190, 750),
        (191, 444),
        (198, 889),
        (208, 722),
        (215, 564),
        (216, 722),
        (222, 556),
        (223, 500),
        (230, 667),
        (240, 500),
        (247, 564),
        (248, 500),
        (254, 500),
        (0, 0),
        (0, 0),
        (0, 0),
    ];
    pub const TIMES_BOLD_HI: [(u8, u16); 71] = [
        (128, 500),
        (130, 333),
        (131, 500),
        (132, 500),
        (133, 1000),
        (134, 500),
        (135, 500),
        (136, 333),
        (137, 1000),
        (138, 556),
        (139, 333),
        (140, 1000),
        (142, 667),
        (145, 333),
        (146, 333),
        (147, 500),
        (148, 500),
        (149, 350),
        (150, 500),
        (151, 1000),
        (152, 333),
        (153, 1000),
        (154, 389),
        (155, 333),
        (156, 722),
        (158, 444),
        (159, 722),
        (161, 333),
        (162, 500),
        (163, 500),
        (164, 500),
        (165, 500),
        (166, 220),
        (167, 500),
        (168, 333),
        (169, 747),
        (170, 300),
        (171, 500),
        (172, 570),
        (174, 747),
        (175, 333),
        (176, 400),
        (177, 570),
        (178, 300),
        (179, 300),
        (180, 333),
        (181, 556),
        (182, 540),
        (183, 250),
        (184, 333),
        (185, 300),
        (186, 330),
        (187, 500),
        (188, 750),
        (189, 750),
        (190, 750),
        (191, 500),
        (198, 1000),
        (208, 722),
        (215, 570),
        (216, 778),
        (222, 611),
        (223, 556),
        (230, 722),
        (240, 500),
        (247, 570),
        (248, 500),
        (254, 556),
        (0, 0),
        (0, 0),
        (0, 0),
    ];
    pub const TIMES_ITALIC_HI: [(u8, u16); 71] = [
        (128, 500),
        (130, 333),
        (131, 500),
        (132, 556),
        (133, 889),
        (134, 500),
        (135, 500),
        (136, 333),
        (137, 1000),
        (138, 500),
        (139, 333),
        (140, 944),
        (142, 556),
        (145, 333),
        (146, 333),
        (147, 556),
        (148, 556),
        (149, 350),
        (150, 500),
        (151, 889),
        (152, 333),
        (153, 980),
        (154, 389),
        (155, 333),
        (156, 667),
        (158, 389),
        (159, 556),
        (161, 389),
        (162, 500),
        (163, 500),
        (164, 500),
        (165, 500),
        (166, 275),
        (167, 500),
        (168, 333),
        (169, 760),
        (170, 276),
        (171, 500),
        (172, 675),
        (174, 760),
        (175, 333),
        (176, 400),
        (177, 675),
        (178, 300),
        (179, 300),
        (180, 333),
        (181, 500),
        (182, 523),
        (183, 250),
        (184, 333),
        (185, 300),
        (186, 310),
        (187, 500),
        (188, 750),
        (189, 750),
        (190, 750),
        (191, 500),
        (198, 889),
        (208, 722),
        (215, 675),
        (216, 722),
        (222, 611),
        (223, 500),
        (230, 667),
        (240, 500),
        (247, 675),
        (248, 500),
        (254, 500),
        (0, 0),
        (0, 0),
        (0, 0),
    ];
    pub const TIMES_BOLD_ITALIC_HI: [(u8, u16); 71] = [
        (128, 500),
        (130, 333),
        (131, 500),
        (132, 500),
        (133, 1000),
        (134, 500),
        (135, 500),
        (136, 333),
        (137, 1000),
        (138, 556),
        (139, 333),
        (140, 944),
        (142, 611),
        (145, 333),
        (146, 333),
        (147, 500),
        (148, 500),
        (149, 350),
        (150, 500),
        (151, 1000),
        (152, 333),
        (153, 1000),
        (154, 389),
        (155, 333),
        (156, 722),
        (158, 389),
        (159, 611),
        (161, 389),
        (162, 500),
        (163, 500),
        (164, 500),
        (165, 500),
        (166, 220),
        (167, 500),
        (168, 333),
        (169, 747),
        (170, 266),
        (171, 500),
        (172, 606),
        (174, 747),
        (175, 333),
        (176, 400),
        (177, 570),
        (178, 300),
        (179, 300),
        (180, 333),
        (181, 576),
        (182, 500),
        (183, 250),
        (184, 333),
        (185, 300),
        (186, 300),
        (187, 500),
        (188, 750),
        (189, 750),
        (190, 750),
        (191, 500),
        (198, 944),
        (208, 722),
        (215, 570),
        (216, 722),
        (222, 611),
        (223, 500),
        (230, 722),
        (240, 500),
        (247, 570),
        (248, 500),
        (254, 500),
        (0, 0),
        (0, 0),
        (0, 0),
    ];
}

// ---------------------------------------------------------------------------
// TrueType
// ---------------------------------------------------------------------------

fn rd16(d: &[u8], p: usize) -> u16 {
    u16::from_be_bytes([
        d.get(p).copied().unwrap_or(0),
        d.get(p + 1).copied().unwrap_or(0),
    ])
}
fn rd32(d: &[u8], p: usize) -> u32 {
    ((rd16(d, p) as u32) << 16) | rd16(d, p + 2) as u32
}
fn rdi16(d: &[u8], p: usize) -> i16 {
    rd16(d, p) as i16
}

/// A parsed TrueType (or OpenType) font program.
#[derive(Debug, Clone)]
pub struct TrueTypeFont {
    data: Vec<u8>,
    tables: HashMap<[u8; 4], (usize, usize)>,
    /// Design units per em (usually 1000 or 2048).
    pub units_per_em: u16,
    /// Number of glyphs.
    pub num_glyphs: u16,
    cmap: HashMap<u32, u16>,
    advances: Vec<u16>,
    loca: Vec<u32>,
    /// Bounding box in design units.
    pub bbox: [i16; 4],
    /// Typographic ascender in design units.
    pub ascent: i16,
    /// Typographic descender in design units (negative).
    pub descent: i16,
    /// Cap height in design units.
    pub cap_height: i16,
    /// Italic angle in degrees.
    pub italic_angle: f64,
    /// PostScript name.
    pub name: String,
    /// Whether the outlines are CFF (`OTTO`); such fonts are embedded whole.
    pub is_cff: bool,
}

impl TrueTypeFont {
    /// Parses a `.ttf` or `.otf` file. For collections (`.ttc`) the first
    /// font is used.
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < 12 {
            return Err(Error::Font("file too small".into()));
        }
        let mut base = 0usize;
        let tag = rd32(bytes, 0);
        if tag == 0x74746366 {
            // 'ttcf'
            base = rd32(bytes, 12) as usize;
        }
        let sfnt = rd32(bytes, base);
        let is_cff = sfnt == 0x4F54544F; // 'OTTO'
        if !(sfnt == 0x00010000 || sfnt == 0x74727565 || is_cff) {
            return Err(Error::Font("not a TrueType/OpenType font".into()));
        }
        let num_tables = rd16(bytes, base + 4) as usize;
        let mut tables = HashMap::new();
        for i in 0..num_tables {
            let p = base + 12 + i * 16;
            if p + 16 > bytes.len() {
                break;
            }
            let mut t = [0u8; 4];
            t.copy_from_slice(&bytes[p..p + 4]);
            let off = rd32(bytes, p + 8) as usize;
            let len = rd32(bytes, p + 12) as usize;
            if off < bytes.len() {
                tables.insert(t, (off, len.min(bytes.len() - off)));
            }
        }
        let table = |t: &[u8; 4]| tables.get(t).map(|&(o, l)| &bytes[o..o + l]);
        let head = table(b"head").ok_or_else(|| Error::Font("missing head table".into()))?;
        let units_per_em = match rd16(head, 18) {
            0 => 1000,
            u => u,
        };
        let bbox = [
            rdi16(head, 36),
            rdi16(head, 38),
            rdi16(head, 40),
            rdi16(head, 42),
        ];
        let index_to_loc = rdi16(head, 50);
        let maxp = table(b"maxp").ok_or_else(|| Error::Font("missing maxp table".into()))?;
        let num_glyphs = rd16(maxp, 4);
        let hhea = table(b"hhea").ok_or_else(|| Error::Font("missing hhea table".into()))?;
        let ascent = rdi16(hhea, 4);
        let descent = rdi16(hhea, 6);
        let num_hmetrics = rd16(hhea, 34) as usize;
        let hmtx = table(b"hmtx").unwrap_or(&[]);
        let mut advances = Vec::with_capacity(num_glyphs as usize);
        for i in 0..num_glyphs as usize {
            let idx = i.min(num_hmetrics.saturating_sub(1));
            advances.push(rd16(hmtx, idx * 4));
        }
        let mut loca = Vec::new();
        if !is_cff {
            let l = table(b"loca").ok_or_else(|| Error::Font("missing loca table".into()))?;
            let n = num_glyphs as usize + 1;
            loca.reserve(n);
            for i in 0..n {
                loca.push(if index_to_loc == 0 {
                    rd16(l, i * 2) as u32 * 2
                } else {
                    rd32(l, i * 4)
                });
            }
        }
        let cmap = table(b"cmap").map(parse_cmap).unwrap_or_default();
        let italic_angle = table(b"post")
            .map(|p| rd32(p, 4) as i32 as f64 / 65536.0)
            .unwrap_or(0.0);
        let cap_height = table(b"OS/2")
            .filter(|o| rd16(o, 0) >= 2 && o.len() >= 90)
            .map(|o| rdi16(o, 88))
            .filter(|&c| c > 0)
            .unwrap_or(ascent);
        let name = table(b"name").map(parse_ps_name).unwrap_or_default();
        let name = if name.is_empty() {
            "EmbeddedFont".to_string()
        } else {
            name
        };
        Ok(Self {
            data: bytes.to_vec(),
            tables,
            units_per_em,
            num_glyphs,
            cmap,
            advances,
            loca,
            bbox,
            ascent,
            descent,
            cap_height,
            italic_angle,
            name,
            is_cff,
        })
    }

    /// Glyph id for a character (0 = missing glyph).
    pub fn glyph_id(&self, c: char) -> u16 {
        self.cmap.get(&(c as u32)).copied().unwrap_or(0)
    }
    /// Whether the font has a glyph for `c`.
    pub fn has_glyph(&self, c: char) -> bool {
        self.cmap.contains_key(&(c as u32))
    }
    /// Advance width of a glyph in 1/1000 em.
    pub fn advance_1000(&self, gid: u16) -> f64 {
        let a = self.advances.get(gid as usize).copied().unwrap_or(0) as f64;
        a * 1000.0 / self.units_per_em as f64
    }
    fn scale(&self, v: i16) -> f64 {
        v as f64 * 1000.0 / self.units_per_em as f64
    }

    fn glyph_data(&self, gid: u16) -> &[u8] {
        let Some(&(off, len)) = self.tables.get(b"glyf") else {
            return &[];
        };
        let (Some(&s), Some(&e)) = (self.loca.get(gid as usize), self.loca.get(gid as usize + 1))
        else {
            return &[];
        };
        let (s, e) = (s as usize, (e as usize).min(len));
        if s >= e || off + e > self.data.len() {
            return &[];
        }
        &self.data[off + s..off + e]
    }

    /// Adds component glyphs of composites so the subset is self-contained.
    fn close_over_components(&self, gids: &mut BTreeSet<u16>) {
        let mut stack: Vec<u16> = gids.iter().copied().collect();
        while let Some(g) = stack.pop() {
            let d = self.glyph_data(g);
            if d.len() < 10 || rdi16(d, 0) >= 0 {
                continue;
            }
            let mut p = 10;
            loop {
                let flags = rd16(d, p);
                let comp = rd16(d, p + 2);
                p += 4;
                if gids.insert(comp) {
                    stack.push(comp);
                }
                p += if flags & 0x0001 != 0 { 4 } else { 2 };
                if flags & 0x0008 != 0 {
                    p += 2;
                } else if flags & 0x0040 != 0 {
                    p += 4;
                } else if flags & 0x0080 != 0 {
                    p += 8;
                }
                if flags & 0x0020 == 0 || p >= d.len() {
                    break;
                }
            }
        }
    }

    /// Produces a font program containing only `gids` (plus glyph 0 and any
    /// composite components). Glyph ids are preserved, so no re-mapping of
    /// text is required. CFF fonts are returned unchanged.
    pub fn subset(&self, gids: &BTreeSet<u16>) -> Vec<u8> {
        if self.is_cff || self.loca.is_empty() {
            return self.data.clone();
        }
        let mut keep = gids.clone();
        keep.insert(0);
        self.close_over_components(&mut keep);
        let n = self.num_glyphs as usize;
        let mut glyf = Vec::new();
        let mut loca = Vec::with_capacity((n + 1) * 4);
        for g in 0..n {
            loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
            if keep.contains(&(g as u16)) {
                let d = self.glyph_data(g as u16);
                glyf.extend_from_slice(d);
                while glyf.len() % 4 != 0 {
                    glyf.push(0);
                }
            }
        }
        loca.extend_from_slice(&(glyf.len() as u32).to_be_bytes());
        let mut head = self
            .tables
            .get(b"head")
            .map(|&(o, l)| self.data[o..o + l].to_vec())
            .unwrap_or_default();
        if head.len() >= 54 {
            head[50] = 0;
            head[51] = 1; // long loca
            head[8..12].copy_from_slice(&[0, 0, 0, 0]); // checkSumAdjustment
        }
        let mut out_tables: Vec<([u8; 4], Vec<u8>)> = Vec::new();
        for tag in [
            b"cvt ", b"fpgm", b"prep", b"hhea", b"hmtx", b"maxp", b"OS/2", b"post",
        ] {
            if let Some(&(o, l)) = self.tables.get(tag) {
                let mut d = self.data[o..o + l].to_vec();
                if tag == b"post" && d.len() > 32 {
                    // Drop glyph names: format 3 has no extra data.
                    d.truncate(32);
                    d[0..4].copy_from_slice(&[0, 3, 0, 0]);
                }
                out_tables.push((*tag, d));
            }
        }
        out_tables.push((*b"glyf", glyf));
        out_tables.push((*b"head", head));
        out_tables.push((*b"loca", loca));
        out_tables.sort_by_key(|t| t.0);
        build_sfnt(out_tables)
    }
}

fn build_sfnt(tables: Vec<([u8; 4], Vec<u8>)>) -> Vec<u8> {
    let n = tables.len();
    let mut entry_selector = 0u16;
    while (1usize << (entry_selector + 1)) <= n {
        entry_selector += 1;
    }
    let search_range = (1u16 << entry_selector) * 16;
    let range_shift = (n as u16) * 16 - search_range;
    let mut out = Vec::new();
    out.extend_from_slice(&0x00010000u32.to_be_bytes());
    out.extend_from_slice(&(n as u16).to_be_bytes());
    out.extend_from_slice(&search_range.to_be_bytes());
    out.extend_from_slice(&entry_selector.to_be_bytes());
    out.extend_from_slice(&range_shift.to_be_bytes());
    let dir_start = out.len();
    out.resize(dir_start + n * 16, 0);
    let mut head_off = None;
    for (i, (tag, data)) in tables.iter().enumerate() {
        while out.len() % 4 != 0 {
            out.push(0);
        }
        let off = out.len();
        if tag == b"head" {
            head_off = Some(off);
        }
        out.extend_from_slice(data);
        let mut padded = data.clone();
        while padded.len() % 4 != 0 {
            padded.push(0);
        }
        let sum = checksum(&padded);
        let e = dir_start + i * 16;
        out[e..e + 4].copy_from_slice(tag);
        out[e + 4..e + 8].copy_from_slice(&sum.to_be_bytes());
        out[e + 8..e + 12].copy_from_slice(&(off as u32).to_be_bytes());
        out[e + 12..e + 16].copy_from_slice(&(data.len() as u32).to_be_bytes());
    }
    while out.len() % 4 != 0 {
        out.push(0);
    }
    if let Some(h) = head_off {
        let total = checksum(&out);
        let adj = 0xB1B0AFBAu32.wrapping_sub(total);
        out[h + 8..h + 12].copy_from_slice(&adj.to_be_bytes());
    }
    out
}

fn checksum(d: &[u8]) -> u32 {
    d.chunks(4).fold(0u32, |acc, c| {
        let mut b = [0u8; 4];
        b[..c.len()].copy_from_slice(c);
        acc.wrapping_add(u32::from_be_bytes(b))
    })
}

fn parse_cmap(cmap: &[u8]) -> HashMap<u32, u16> {
    let n = rd16(cmap, 2) as usize;
    let mut best: Option<(u8, usize)> = None; // (score, offset)
    for i in 0..n {
        let p = 4 + i * 8;
        let (pid, eid, off) = (rd16(cmap, p), rd16(cmap, p + 2), rd32(cmap, p + 4) as usize);
        if off >= cmap.len() {
            continue;
        }
        let fmt = rd16(cmap, off);
        let score = match (pid, eid, fmt) {
            (3, 10, 12) => 6,
            (0, _, 12) => 5,
            (3, 1, 4) => 4,
            (0, _, 4) => 3,
            (3, 0, 4) => 2,
            (1, 0, 0) => 1,
            _ => 0,
        };
        if score > 0 && best.map(|(s, _)| score > s).unwrap_or(true) {
            best = Some((score, off));
        }
    }
    let mut map = HashMap::new();
    let Some((_, off)) = best else { return map };
    let sub = &cmap[off..];
    match rd16(sub, 0) {
        0 => {
            for c in 0..256usize {
                let g = sub.get(6 + c).copied().unwrap_or(0) as u16;
                if g != 0 {
                    map.insert(c as u32, g);
                }
            }
        }
        4 => {
            let segx2 = rd16(sub, 6) as usize;
            let seg = segx2 / 2;
            let ends = 14;
            let starts = ends + segx2 + 2;
            let deltas = starts + segx2;
            let ranges = deltas + segx2;
            for s in 0..seg {
                let end = rd16(sub, ends + s * 2) as u32;
                let start = rd16(sub, starts + s * 2) as u32;
                let delta = rd16(sub, deltas + s * 2);
                let ro = rd16(sub, ranges + s * 2) as usize;
                if start > end || end == 0xFFFF && start == 0xFFFF {
                    continue;
                }
                for c in start..=end.min(0xFFFE) {
                    let g = if ro == 0 {
                        (c as u16).wrapping_add(delta)
                    } else {
                        let gp = ranges + s * 2 + ro + (c - start) as usize * 2;
                        if gp + 1 >= sub.len() {
                            continue;
                        }
                        let g = rd16(sub, gp);
                        if g == 0 {
                            0
                        } else {
                            g.wrapping_add(delta)
                        }
                    };
                    if g != 0 {
                        map.insert(c, g);
                    }
                }
            }
        }
        12 => {
            let ngroups = rd32(sub, 12) as usize;
            for g in 0..ngroups.min(100_000) {
                let p = 16 + g * 12;
                let (sc, ec, sg) = (rd32(sub, p), rd32(sub, p + 4), rd32(sub, p + 8));
                if ec < sc || ec - sc > 65535 {
                    continue;
                }
                for c in sc..=ec {
                    let gid = sg + (c - sc);
                    if gid != 0 && gid <= 0xFFFF {
                        map.insert(c, gid as u16);
                    }
                }
            }
        }
        _ => {}
    }
    map
}

fn parse_ps_name(name: &[u8]) -> String {
    let count = rd16(name, 2) as usize;
    let string_off = rd16(name, 4) as usize;
    let mut best: Option<(u8, String)> = None;
    for i in 0..count {
        let p = 6 + i * 12;
        let (pid, _eid, _lang, nid, len, off) = (
            rd16(name, p),
            rd16(name, p + 2),
            rd16(name, p + 4),
            rd16(name, p + 6),
            rd16(name, p + 8) as usize,
            rd16(name, p + 10) as usize,
        );
        let s = string_off + off;
        if s + len > name.len() {
            continue;
        }
        let raw = &name[s..s + len];
        let text = if pid == 3 || pid == 0 {
            let u: Vec<u16> = raw
                .chunks(2)
                .filter(|c| c.len() == 2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect();
            String::from_utf16_lossy(&u)
        } else {
            String::from_utf8_lossy(raw).into_owned()
        };
        let score = match nid {
            6 => 3,
            4 => 2,
            1 => 1,
            _ => 0,
        };
        if score > 0 && best.as_ref().map(|(s, _)| score > *s).unwrap_or(true) {
            best = Some((score, text));
        }
    }
    let n = best.map(|(_, s)| s).unwrap_or_default();
    n.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect()
}

// ---------------------------------------------------------------------------
// Font handle used by Document
// ---------------------------------------------------------------------------

/// What kind of font a [`Font`] wraps.
#[derive(Debug, Clone)]
pub enum FontKind {
    /// One of the standard 14.
    Standard(StandardFont),
    /// An embedded TrueType/OpenType font.
    TrueType(Box<TrueTypeFont>),
}

/// A font usable for drawing text. Tracks which glyphs have been used so the
/// embedded program can be subset on save.
#[derive(Debug, Clone)]
pub struct Font {
    /// The underlying font.
    pub kind: FontKind,
    used: BTreeMap<u16, char>,
}

impl Font {
    /// Wraps a standard font.
    pub fn standard(f: StandardFont) -> Self {
        Self {
            kind: FontKind::Standard(f),
            used: BTreeMap::new(),
        }
    }
    /// Parses and wraps a TrueType/OpenType font file.
    pub fn truetype(bytes: &[u8]) -> Result<Self> {
        Ok(Self {
            kind: FontKind::TrueType(Box::new(TrueTypeFont::parse(bytes)?)),
            used: BTreeMap::new(),
        })
    }
    /// Whether text is encoded as 2-byte glyph ids (`true`) or single bytes.
    pub fn is_two_byte(&self) -> bool {
        matches!(self.kind, FontKind::TrueType(_))
    }
    /// Encodes `text` into show-string bytes, recording glyph usage.
    pub fn encode(&mut self, text: &str) -> Vec<u8> {
        match &self.kind {
            FontKind::Standard(_) => text
                .chars()
                .map(|c| winansi_encode(c).unwrap_or(b'?'))
                .collect(),
            FontKind::TrueType(tt) => {
                let mut out = Vec::with_capacity(text.len() * 2);
                for c in text.chars() {
                    let g = tt.glyph_id(c);
                    self.used.insert(g, c);
                    out.extend_from_slice(&g.to_be_bytes());
                }
                out
            }
        }
    }
    /// Width of `text` at `size` points.
    pub fn measure(&self, text: &str, size: f64) -> f64 {
        let w1000: f64 = match &self.kind {
            FontKind::Standard(s) => text
                .chars()
                .map(|c| s.width(winansi_encode(c).unwrap_or(b'?')) as f64)
                .sum(),
            FontKind::TrueType(tt) => text.chars().map(|c| tt.advance_1000(tt.glyph_id(c))).sum(),
        };
        w1000 * size / 1000.0
    }
    /// Ascent in 1/1000 em.
    pub fn ascent(&self) -> f64 {
        match &self.kind {
            FontKind::Standard(_) => 718.0,
            FontKind::TrueType(tt) => tt.scale(tt.ascent),
        }
    }
    /// Descent in 1/1000 em (negative).
    pub fn descent(&self) -> f64 {
        match &self.kind {
            FontKind::Standard(_) => -207.0,
            FontKind::TrueType(tt) => tt.scale(tt.descent),
        }
    }

    /// Builds the font dictionary (and any subsidiary objects through
    /// `alloc`). Returns the top-level font dictionary.
    pub(crate) fn build(
        &self,
        compression_level: u8,
        alloc: &mut dyn FnMut(Object) -> ObjRef,
    ) -> Dict {
        match &self.kind {
            FontKind::Standard(s) => Dict::new()
                .with("Type", "Font")
                .with("Subtype", "Type1")
                .with("BaseFont", s.base_font())
                .with("Encoding", "WinAnsiEncoding"),
            FontKind::TrueType(tt) => {
                let gids: BTreeSet<u16> = self.used.keys().copied().collect();
                let program = tt.subset(&gids);
                let tag = subset_tag(&gids);
                let base = format!("{tag}+{}", tt.name);
                let mut ff = Dict::new();
                if tt.is_cff {
                    ff.set("Subtype", "OpenType");
                } else {
                    ff.set("Length1", program.len() as i64);
                }
                ff.set("Filter", "FlateDecode");
                let ff_ref =
                    alloc(Stream::new(ff, flate_encode(&program, compression_level)).into());
                let mut desc = Dict::new();
                desc.set("Type", "FontDescriptor")
                    .set("FontName", base.as_str())
                    .set("Flags", 4)
                    .set(
                        "FontBBox",
                        Object::Array(tt.bbox.iter().map(|&v| Object::Real(tt.scale(v))).collect()),
                    )
                    .set("ItalicAngle", tt.italic_angle)
                    .set("Ascent", tt.scale(tt.ascent))
                    .set("Descent", tt.scale(tt.descent))
                    .set("CapHeight", tt.scale(tt.cap_height))
                    .set("StemV", 80)
                    .set(if tt.is_cff { "FontFile3" } else { "FontFile2" }, ff_ref);
                let desc_ref = alloc(desc.into());
                // W array: individual widths for used glyphs.
                let mut w = Vec::new();
                for &g in self.used.keys() {
                    w.push(Object::Integer(g as i64));
                    w.push(Object::Array(vec![Object::Real(tt.advance_1000(g))]));
                }
                let mut cid = Dict::new();
                cid.set("Type", "Font")
                    .set(
                        "Subtype",
                        if tt.is_cff {
                            "CIDFontType0"
                        } else {
                            "CIDFontType2"
                        },
                    )
                    .set("BaseFont", base.as_str())
                    .set(
                        "CIDSystemInfo",
                        Dict::new()
                            .with("Registry", Object::string(b"Adobe".to_vec()))
                            .with("Ordering", Object::string(b"Identity".to_vec()))
                            .with("Supplement", 0),
                    )
                    .set("FontDescriptor", desc_ref)
                    .set("DW", 1000)
                    .set("W", Object::Array(w));
                if !tt.is_cff {
                    cid.set("CIDToGIDMap", "Identity");
                }
                let cid_ref = alloc(cid.into());
                let tu = to_unicode_cmap(&self.used);
                let tu_ref = alloc(
                    Stream::new(
                        Dict::new().with("Filter", "FlateDecode"),
                        flate_encode(&tu, compression_level),
                    )
                    .into(),
                );
                Dict::new()
                    .with("Type", "Font")
                    .with("Subtype", "Type0")
                    .with("BaseFont", base.as_str())
                    .with("Encoding", "Identity-H")
                    .with("DescendantFonts", Object::Array(vec![cid_ref.into()]))
                    .with("ToUnicode", tu_ref)
            }
        }
    }
}

fn subset_tag(gids: &BTreeSet<u16>) -> String {
    // Deterministic 6-letter tag derived from the glyph set.
    let mut h: u32 = 2166136261;
    for g in gids {
        h = (h ^ *g as u32).wrapping_mul(16777619);
    }
    (0..6)
        .map(|i| {
            let v = (h >> (i * 5)) & 31;
            (b'A' + (v % 26) as u8) as char
        })
        .collect()
}

fn to_unicode_cmap(used: &BTreeMap<u16, char>) -> Vec<u8> {
    let mut s = String::new();
    s.push_str(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
    );
    let entries: Vec<(&u16, &char)> = used.iter().collect();
    for chunk in entries.chunks(100) {
        s.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (g, c) in chunk {
            s.push_str(&format!("<{:04X}> <", g));
            let mut buf = [0u16; 2];
            for u in c.encode_utf16(&mut buf) {
                s.push_str(&format!("{u:04X}"));
            }
            s.push_str(">\n");
        }
        s.push_str("endbfchar\n");
    }
    s.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    s.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_metrics() {
        let f = Font::standard(StandardFont::Helvetica);
        // "Hello" = 722+556+222+222+556 = 2278
        assert!((f.measure("Hello", 10.0) - 22.78).abs() < 1e-9);
        assert_eq!(StandardFont::Courier.width(b'W'), 600);
        assert_eq!(StandardFont::TimesRoman.width(233), 444); // é -> e
        assert_eq!(StandardFont::Helvetica.width(128), 556); // €
    }

    #[test]
    fn aliases() {
        assert_eq!(
            StandardFont::by_name("Arial Bold"),
            Some(StandardFont::HelveticaBold)
        );
        assert_eq!(
            StandardFont::by_name("Times New Roman, Italic"),
            Some(StandardFont::TimesItalic)
        );
        assert_eq!(
            StandardFont::by_name("Courier-BoldOblique"),
            Some(StandardFont::CourierBoldOblique)
        );
        assert_eq!(StandardFont::by_name("Comic Sans"), None);
    }

    #[test]
    fn winansi() {
        assert_eq!(winansi_encode('A'), Some(65));
        assert_eq!(winansi_encode('€'), Some(128));
        assert_eq!(winansi_encode('—'), Some(151));
        assert_eq!(winansi_encode('中'), None);
        let mut f = Font::standard(StandardFont::Helvetica);
        assert_eq!(f.encode("a€中"), vec![b'a', 128, b'?']);
    }

    #[test]
    fn sfnt_checksum() {
        assert_eq!(checksum(&[0, 0, 0, 1, 0, 0, 0, 2]), 3);
        let out = build_sfnt(vec![(*b"head", vec![0u8; 54])]);
        assert_eq!(checksum(&out), 0xB1B0AFBA);
    }

    #[test]
    fn subset_tag_is_stable() {
        let a: BTreeSet<u16> = [1, 2, 3].into_iter().collect();
        assert_eq!(subset_tag(&a), subset_tag(&a));
        assert_eq!(subset_tag(&a).len(), 6);
    }
}
