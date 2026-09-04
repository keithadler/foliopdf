//! Builder for page content streams (ISO 32000-1 §8 and §9).
//!
//! [`ContentBuilder`] appends operators to a byte buffer. Numbers are written
//! with at most four decimals and no trailing zeros, which keeps streams
//! compact and deterministic.

use crate::geometry::{Matrix, Rect};
use crate::object::Name;

/// Appends drawing operators to a content stream.
#[derive(Debug, Default, Clone)]
pub struct ContentBuilder {
    buf: Vec<u8>,
}

/// Writes a number in compact PDF form.
pub fn write_num(buf: &mut Vec<u8>, v: f64) {
    if !v.is_finite() {
        buf.push(b'0');
        return;
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        buf.extend_from_slice((v as i64).to_string().as_bytes());
        return;
    }
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    let s = if s == "-0" { "0" } else { s };
    buf.extend_from_slice(s.as_bytes());
}

/// Escapes bytes for a literal string `( ... )`.
pub fn write_literal_string(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.push(b'(');
    for &b in bytes {
        match b {
            b'(' | b')' | b'\\' => {
                buf.push(b'\\');
                buf.push(b);
            }
            b'\r' => buf.extend_from_slice(b"\\r"),
            b'\n' => buf.extend_from_slice(b"\\n"),
            0..=31 | 127..=255 => buf.extend_from_slice(format!("\\{b:03o}").as_bytes()),
            _ => buf.push(b),
        }
    }
    buf.push(b')');
}

/// Writes a hex string `<...>`.
pub fn write_hex_string(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.push(b'<');
    for b in bytes {
        buf.extend_from_slice(format!("{b:02X}").as_bytes());
    }
    buf.push(b'>');
}

/// Writes a name with `#xx` escapes where required.
pub fn write_name(buf: &mut Vec<u8>, name: &Name) {
    buf.push(b'/');
    for &b in name.as_bytes() {
        if !(33..=126).contains(&b) || b"()<>[]{}/%#".contains(&b) {
            buf.extend_from_slice(format!("#{b:02X}").as_bytes());
        } else {
            buf.push(b);
        }
    }
}

impl ContentBuilder {
    /// Creates an empty builder.
    pub fn new() -> Self {
        Self::default()
    }
    /// Consumes the builder, returning the raw content stream bytes.
    pub fn finish(self) -> Vec<u8> {
        self.buf
    }
    /// Current bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.buf
    }
    /// Appends raw operators verbatim.
    pub fn raw(&mut self, ops: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(ops);
        self.nl()
    }
    fn nl(&mut self) -> &mut Self {
        if !self.buf.ends_with(b"\n") {
            self.buf.push(b'\n');
        }
        self
    }
    fn num(&mut self, v: f64) -> &mut Self {
        write_num(&mut self.buf, v);
        self.buf.push(b' ');
        self
    }
    fn op(&mut self, op: &str) -> &mut Self {
        self.buf.extend_from_slice(op.as_bytes());
        self.buf.push(b'\n');
        self
    }
    fn name(&mut self, n: &str) -> &mut Self {
        write_name(&mut self.buf, &Name::new(n));
        self.buf.push(b' ');
        self
    }

    // Graphics state --------------------------------------------------------

    /// `q` – save graphics state.
    pub fn save(&mut self) -> &mut Self {
        self.op("q")
    }
    /// `Q` – restore graphics state.
    pub fn restore(&mut self) -> &mut Self {
        self.op("Q")
    }
    /// `cm` – concatenate a matrix.
    pub fn transform(&mut self, m: &Matrix) -> &mut Self {
        self.num(m.a)
            .num(m.b)
            .num(m.c)
            .num(m.d)
            .num(m.e)
            .num(m.f)
            .op("cm")
    }
    /// `w` – line width.
    pub fn line_width(&mut self, w: f64) -> &mut Self {
        self.num(w).op("w")
    }
    /// `gs` – apply a named ExtGState resource.
    pub fn ext_gstate(&mut self, name: &str) -> &mut Self {
        self.name(name).op("gs")
    }
    /// `J` – line cap (0 butt, 1 round, 2 square).
    pub fn line_cap(&mut self, cap: u8) -> &mut Self {
        self.num(cap as f64).op("J")
    }
    /// `d` – dash pattern.
    pub fn dash(&mut self, pattern: &[f64], phase: f64) -> &mut Self {
        self.buf.push(b'[');
        for &p in pattern {
            self.num(p);
        }
        self.buf.push(b']');
        self.buf.push(b' ');
        self.num(phase).op("d")
    }

    // Colour ---------------------------------------------------------------

    /// `rg` – fill colour in RGB (0..1).
    pub fn fill_rgb(&mut self, r: f64, g: f64, b: f64) -> &mut Self {
        self.num(r).num(g).num(b).op("rg")
    }
    /// `RG` – stroke colour in RGB (0..1).
    pub fn stroke_rgb(&mut self, r: f64, g: f64, b: f64) -> &mut Self {
        self.num(r).num(g).num(b).op("RG")
    }
    /// `g` – fill gray.
    pub fn fill_gray(&mut self, g: f64) -> &mut Self {
        self.num(g).op("g")
    }
    /// `G` – stroke gray.
    pub fn stroke_gray(&mut self, g: f64) -> &mut Self {
        self.num(g).op("G")
    }
    /// `k` – fill CMYK.
    pub fn fill_cmyk(&mut self, c: f64, m: f64, y: f64, k: f64) -> &mut Self {
        self.num(c).num(m).num(y).num(k).op("k")
    }

    // Paths ----------------------------------------------------------------

    /// `m` – move to.
    pub fn move_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.num(x).num(y).op("m")
    }
    /// `l` – line to.
    pub fn line_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.num(x).num(y).op("l")
    }
    /// `c` – cubic Bézier.
    pub fn curve_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x3: f64, y3: f64) -> &mut Self {
        self.num(x1).num(y1).num(x2).num(y2).num(x3).num(y3).op("c")
    }
    /// `re` – rectangle.
    pub fn rect(&mut self, r: &Rect) -> &mut Self {
        self.num(r.x0)
            .num(r.y0)
            .num(r.width())
            .num(r.height())
            .op("re")
    }
    /// `h` – close path.
    pub fn close(&mut self) -> &mut Self {
        self.op("h")
    }
    /// `S` – stroke.
    pub fn stroke(&mut self) -> &mut Self {
        self.op("S")
    }
    /// `f` – fill (non-zero).
    pub fn fill(&mut self) -> &mut Self {
        self.op("f")
    }
    /// `B` – fill and stroke.
    pub fn fill_stroke(&mut self) -> &mut Self {
        self.op("B")
    }
    /// `n` – end path without painting (use after a clip).
    pub fn end_path(&mut self) -> &mut Self {
        self.op("n")
    }
    /// `W` – set clipping path (call before the painting operator).
    pub fn clip(&mut self) -> &mut Self {
        self.op("W")
    }

    // Text -----------------------------------------------------------------

    /// `BT` – begin text.
    pub fn begin_text(&mut self) -> &mut Self {
        self.op("BT")
    }
    /// `ET` – end text.
    pub fn end_text(&mut self) -> &mut Self {
        self.op("ET")
    }
    /// `Tf` – select font resource and size.
    pub fn font(&mut self, resource: &str, size: f64) -> &mut Self {
        self.name(resource).num(size).op("Tf")
    }
    /// `Td` – move text position.
    pub fn text_position(&mut self, x: f64, y: f64) -> &mut Self {
        self.num(x).num(y).op("Td")
    }
    /// `Tm` – set text matrix.
    pub fn text_matrix(&mut self, m: &Matrix) -> &mut Self {
        self.num(m.a)
            .num(m.b)
            .num(m.c)
            .num(m.d)
            .num(m.e)
            .num(m.f)
            .op("Tm")
    }
    /// `TL` – leading.
    pub fn leading(&mut self, l: f64) -> &mut Self {
        self.num(l).op("TL")
    }
    /// `T*` – next line.
    pub fn next_line(&mut self) -> &mut Self {
        self.op("T*")
    }
    /// `Tr` – text rendering mode (0 fill, 1 stroke, 3 invisible, ...).
    pub fn text_render_mode(&mut self, mode: u8) -> &mut Self {
        self.num(mode as f64).op("Tr")
    }
    /// `Tj` – show already-encoded text bytes.
    pub fn show_bytes(&mut self, encoded: &[u8]) -> &mut Self {
        write_hex_string(&mut self.buf, encoded);
        self.buf.push(b' ');
        self.op("Tj")
    }
    /// `Tj` with a literal string (for single-byte encodings).
    pub fn show_literal(&mut self, encoded: &[u8]) -> &mut Self {
        write_literal_string(&mut self.buf, encoded);
        self.buf.push(b' ');
        self.op("Tj")
    }

    // XObjects --------------------------------------------------------------

    /// `Do` – paint a named XObject (image or form).
    pub fn xobject(&mut self, resource: &str) -> &mut Self {
        self.name(resource).op("Do")
    }
    /// Draws an image XObject scaled into `rect` (images are 1×1 unit squares).
    pub fn image(&mut self, resource: &str, rect: &Rect) -> &mut Self {
        self.save()
            .transform(&Matrix::new(
                rect.width(),
                0.0,
                0.0,
                rect.height(),
                rect.x0,
                rect.y0,
            ))
            .xobject(resource)
            .restore()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_compact() {
        let mut b = Vec::new();
        write_num(&mut b, 1.0);
        b.push(b' ');
        write_num(&mut b, 0.5);
        b.push(b' ');
        write_num(&mut b, -0.00001);
        b.push(b' ');
        write_num(&mut b, 12.3456789);
        assert_eq!(b, b"1 0.5 0 12.3457");
    }

    #[test]
    fn strings_escape() {
        let mut b = Vec::new();
        write_literal_string(&mut b, b"a(b)\\\n\x01");
        assert_eq!(b, b"(a\\(b\\)\\\\\\n\\001)");
        let mut n = Vec::new();
        write_name(&mut n, &Name::new("A B/C"));
        assert_eq!(n, b"/A#20B#2FC");
    }

    #[test]
    fn builds_ops() {
        let mut c = ContentBuilder::new();
        c.save()
            .fill_rgb(1.0, 0.0, 0.0)
            .rect(&Rect::new(0.0, 0.0, 10.0, 20.0))
            .fill()
            .restore();
        assert_eq!(c.finish(), b"q\n1 0 0 rg\n0 0 10 20 re\nf\nQ\n");
    }
}
