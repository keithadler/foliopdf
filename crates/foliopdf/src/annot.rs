//! Annotations (ISO 32000-1 §12.5): highlights, shapes, drawings, text boxes,
//! sticky notes, links and image stamps.
//!
//! Every annotation created here carries an appearance stream, so it looks
//! the same in every viewer, and can be *flattened* (painted into the page
//! content and removed) with [`flatten_annotations`].
//!
//! ## Coordinates
//!
//! All geometry passed to and returned from this module is in **display
//! space**: points, with the origin at the bottom-left corner of the page *as
//! the reader sees it*, x to the right and y upwards. Page rotation and crop
//! boxes are handled internally, so a highlight at the top of a rotated page
//! is given at the top of the rotated page.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::content::{write_num, ContentBuilder};
use crate::document::Document;
use crate::error::{Error, Result};
use crate::filters;
use crate::font::{Font, StandardFont};
use crate::geometry::{Matrix, Point, Rect};
use crate::image::Image;
use crate::object::{Dict, ObjRef, Object, PdfString, Stream};
use crate::page::PageInfo;

fn one() -> f64 {
    1.0
}
fn two() -> f64 {
    2.0
}
fn twelve() -> f64 {
    12.0
}
fn yellow() -> [f64; 3] {
    [1.0, 0.92, 0.23]
}
fn red() -> [f64; 3] {
    [0.85, 0.15, 0.1]
}
fn red_opt() -> Option<[f64; 3]> {
    Some(red())
}
fn black() -> [f64; 3] {
    [0.0, 0.0, 0.0]
}
fn helv() -> String {
    "Helvetica".into()
}

/// Horizontal text alignment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[allow(missing_docs)]
pub enum Align {
    #[default]
    Left,
    Center,
    Right,
}

/// Icon of a sticky note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[allow(missing_docs)]
pub enum NoteIcon {
    #[default]
    Comment,
    Note,
    Key,
    Help,
    Paragraph,
    Insert,
}

impl NoteIcon {
    fn name(self) -> &'static str {
        match self {
            NoteIcon::Comment => "Comment",
            NoteIcon::Note => "Note",
            NoteIcon::Key => "Key",
            NoteIcon::Help => "Help",
            NoteIcon::Paragraph => "Paragraph",
            NoteIcon::Insert => "Insert",
        }
    }
}

/// What to draw. Serialised with a `kind` tag, e.g.
/// `{ "kind": "highlight", "quads": [{ "x0": 72, "y0": 700, "x1": 300, "y1": 712 }] }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Annotation {
    /// Translucent colour over text, one rectangle per line.
    Highlight {
        /// Rectangles to cover.
        quads: Vec<Rect>,
        /// RGB 0..1. Default yellow.
        #[serde(default = "yellow")]
        color: [f64; 3],
        /// 0..1. Default 1 (highlights use multiply blending, so 1 is fine).
        #[serde(default = "one")]
        opacity: f64,
    },
    /// A line under each rectangle.
    Underline {
        /// Rectangles of the text.
        quads: Vec<Rect>,
        /// RGB 0..1. Default red.
        #[serde(default = "red")]
        color: [f64; 3],
        /// 0..1.
        #[serde(default = "one")]
        opacity: f64,
    },
    /// A line through the middle of each rectangle.
    StrikeOut {
        /// Rectangles of the text.
        quads: Vec<Rect>,
        /// RGB 0..1. Default red.
        #[serde(default = "red")]
        color: [f64; 3],
        /// 0..1.
        #[serde(default = "one")]
        opacity: f64,
    },
    /// A rectangle.
    Square {
        /// Outer bounds.
        rect: Rect,
        /// Stroke colour; `None` for no outline. Default red.
        #[serde(default = "red_opt")]
        stroke: Option<[f64; 3]>,
        /// Fill colour; `None` for transparent.
        #[serde(default)]
        fill: Option<[f64; 3]>,
        /// Line width in points.
        #[serde(default = "two")]
        width: f64,
        /// 0..1.
        #[serde(default = "one")]
        opacity: f64,
    },
    /// An ellipse inscribed in `rect`.
    Circle {
        /// Outer bounds.
        rect: Rect,
        /// Stroke colour; `None` for no outline. Default red.
        #[serde(default = "red_opt")]
        stroke: Option<[f64; 3]>,
        /// Fill colour; `None` for transparent.
        #[serde(default)]
        fill: Option<[f64; 3]>,
        /// Line width in points.
        #[serde(default = "two")]
        width: f64,
        /// 0..1.
        #[serde(default = "one")]
        opacity: f64,
    },
    /// A straight line.
    Line {
        /// Start.
        from: Point,
        /// End.
        to: Point,
        /// RGB 0..1.
        #[serde(default = "red")]
        color: [f64; 3],
        /// Line width in points.
        #[serde(default = "two")]
        width: f64,
        /// 0..1.
        #[serde(default = "one")]
        opacity: f64,
    },
    /// Freehand strokes.
    Ink {
        /// One polyline per stroke.
        paths: Vec<Vec<Point>>,
        /// RGB 0..1.
        #[serde(default = "red")]
        color: [f64; 3],
        /// Line width in points.
        #[serde(default = "two")]
        width: f64,
        /// 0..1.
        #[serde(default = "one")]
        opacity: f64,
    },
    /// A box of text.
    FreeText {
        /// The box.
        rect: Rect,
        /// Text; `\n` starts a new line, long lines wrap.
        text: String,
        /// Standard font name (`Helvetica`, `Times-Bold`, `Courier`, ...).
        #[serde(default = "helv")]
        font: String,
        /// Font size in points.
        #[serde(default = "twelve")]
        size: f64,
        /// Text colour, RGB 0..1.
        #[serde(default = "black")]
        color: [f64; 3],
        /// Alignment inside the box.
        #[serde(default)]
        align: Align,
        /// Background colour; `None` for transparent.
        #[serde(default)]
        background: Option<[f64; 3]>,
        /// Border colour; `None` for no border.
        #[serde(default)]
        border: Option<[f64; 3]>,
        /// 0..1.
        #[serde(default = "one")]
        opacity: f64,
    },
    /// A sticky note icon; the text goes in [`AnnotationMeta::contents`].
    Note {
        /// Top-left corner of the icon.
        at: Point,
        /// Icon style.
        #[serde(default)]
        icon: NoteIcon,
        /// Icon colour, RGB 0..1.
        #[serde(default = "yellow")]
        color: [f64; 3],
    },
    /// A clickable area opening a URL or jumping to a page.
    Link {
        /// The clickable area.
        rect: Rect,
        /// Web address.
        #[serde(default)]
        uri: Option<String>,
        /// 0-based page to jump to (used when `uri` is `None`).
        #[serde(default)]
        page: Option<usize>,
    },
}

impl Annotation {
    /// Applies `f` to every coordinate. Rectangles are re-normalised, so a
    /// flip or rotation is fine.
    pub fn map_points(&mut self, f: impl Fn(Point) -> Point) {
        let fr = |r: &mut Rect| {
            let a = f(Point::new(r.x0, r.y0));
            let b = f(Point::new(r.x1, r.y1));
            *r = Rect::new(a.x, a.y, b.x, b.y);
        };
        match self {
            Annotation::Highlight { quads, .. }
            | Annotation::Underline { quads, .. }
            | Annotation::StrikeOut { quads, .. } => quads.iter_mut().for_each(fr),
            Annotation::Square { rect, .. }
            | Annotation::Circle { rect, .. }
            | Annotation::FreeText { rect, .. }
            | Annotation::Link { rect, .. } => fr(rect),
            Annotation::Line { from, to, .. } => {
                *from = f(*from);
                *to = f(*to);
            }
            Annotation::Ink { paths, .. } => paths.iter_mut().flatten().for_each(|p| *p = f(*p)),
            Annotation::Note { at, .. } => *at = f(*at),
        }
    }
}

/// Who, what and when. All optional.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct AnnotationMeta {
    /// Author, shown by viewers (`/T`).
    pub author: Option<String>,
    /// Comment text or note body (`/Contents`).
    pub contents: Option<String>,
    /// Short subject line (`/Subj`).
    pub subject: Option<String>,
    /// Modification date as a PDF date string, e.g. `D:20260904120000Z`.
    pub modified: Option<String>,
    /// Whether the annotation prints. Default true.
    pub print: Option<bool>,
}

/// Summary of an existing annotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnotInfo {
    /// Position in the page's annotation array.
    pub index: usize,
    /// Object number, stable until the document is saved.
    pub object: u32,
    /// `/Subtype`, e.g. `Highlight`, `Widget`, `Link`.
    pub subtype: String,
    /// Bounds in display space.
    pub rect: Rect,
    /// `/Contents`.
    pub contents: Option<String>,
    /// `/T` (author or field title).
    pub author: Option<String>,
    /// Whether the Hidden flag is set.
    pub hidden: bool,
    /// For widgets: the form field's fully qualified name.
    pub field: Option<String>,
    /// Whether it has an appearance stream (needed for flattening).
    pub has_appearance: bool,
}

/// Which annotations [`flatten_annotations`] paints.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FlattenOptions {
    /// Include form field widgets. Default true.
    pub widgets: Option<bool>,
    /// Only these object numbers (from [`AnnotInfo::object`] or the value
    /// returned by [`add_annotation`]). `None` means all.
    pub objects: Option<Vec<u32>>,
    /// Only these subtypes (e.g. `["Highlight", "Ink"]`). `None` means all.
    pub subtypes: Option<Vec<String>>,
}

// ---------------------------------------------------------------------------
// Display <-> user space
// ---------------------------------------------------------------------------

/// Maps a display-space rectangle to user space.
pub fn to_user_rect(info: &PageInfo, r: &Rect) -> Rect {
    r.transform(&info.display_to_user())
}

/// Maps a user-space rectangle to display space.
pub fn to_display_rect(info: &PageInfo, r: &Rect) -> Rect {
    match info.display_to_user().invert() {
        Some(inv) => r.transform(&inv),
        None => *r,
    }
}

fn to_user_point(info: &PageInfo, p: Point) -> Point {
    info.display_to_user().apply(p)
}

// ---------------------------------------------------------------------------
// Page annotation arrays
// ---------------------------------------------------------------------------

impl Document {
    /// Object references in a page's `/Annots` array (direct dictionaries in
    /// the array are skipped).
    pub fn page_annots(&self, index: usize) -> Result<Vec<ObjRef>> {
        let page = self.page_ref(index)?;
        Ok(
            match self.get(page).as_dict().and_then(|d| d.get("Annots")) {
                Some(o) => match self.resolve(o) {
                    Object::Array(a) => a.iter().filter_map(Object::as_reference).collect(),
                    _ => Vec::new(),
                },
                None => Vec::new(),
            },
        )
    }

    /// Replaces a page's `/Annots` array.
    pub fn set_page_annots(&mut self, index: usize, annots: &[ObjRef]) -> Result<()> {
        let page = self.page_ref(index)?;
        let d = self
            .get_mut(page)
            .and_then(Object::as_dict_mut)
            .ok_or_else(|| Error::malformed("page is not a dictionary"))?;
        if annots.is_empty() {
            d.remove("Annots");
        } else {
            d.set(
                "Annots",
                Object::Array(annots.iter().map(|&r| r.into()).collect()),
            );
        }
        Ok(())
    }

    /// Appends an annotation object to a page.
    pub fn push_annot(&mut self, index: usize, annot: ObjRef) -> Result<()> {
        let mut a = self.page_annots(index)?;
        a.push(annot);
        self.set_page_annots(index, &a)
    }

    /// Map from annotation object number to page index, over all pages.
    pub fn annot_pages(&self) -> HashMap<u32, usize> {
        let mut out = HashMap::new();
        for i in 0..self.page_count() {
            if let Ok(list) = self.page_annots(i) {
                for r in list {
                    out.entry(r.num).or_insert(i);
                }
            }
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Building appearance streams
// ---------------------------------------------------------------------------

/// Creates a form XObject with `BBox [0 0 w h]`.
pub(crate) fn make_form(
    doc: &mut Document,
    w: f64,
    h: f64,
    matrix: Option<Matrix>,
    resources: Dict,
    content: Vec<u8>,
) -> ObjRef {
    let mut d = Dict::new()
        .with("Type", "XObject")
        .with("Subtype", "Form")
        .with("BBox", Rect::new(0.0, 0.0, w, h).to_object())
        .with("Resources", resources)
        .with("Filter", "FlateDecode");
    if let Some(m) = matrix {
        if m != Matrix::IDENTITY {
            d.set("Matrix", m.to_object());
        }
    }
    doc.add(Stream::new(d, filters::flate_encode(&content, 6)).into())
}

fn gstate(doc: &mut Document, opacity: f64, multiply: bool) -> Option<ObjRef> {
    if opacity >= 1.0 && !multiply {
        return None;
    }
    let mut d = Dict::new().with("Type", "ExtGState");
    let a = opacity.clamp(0.0, 1.0);
    d.set("CA", a);
    d.set("ca", a);
    if multiply {
        d.set("BM", "Multiply");
    }
    Some(doc.add(d.into()))
}

fn rgb(c: [f64; 3]) -> Object {
    Object::Array(c.iter().map(|&v| Object::Real(v.clamp(0.0, 1.0))).collect())
}

/// Splits `text` into lines that fit `max_width` at `size` points.
pub(crate) fn wrap_text(font: &Font, size: f64, text: &str, max_width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    for para in text.split('\n') {
        let para = para.trim_end_matches('\r');
        let mut line = String::new();
        for word in para.split(' ') {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if font.measure(&candidate, size) <= max_width || line.is_empty() && word.is_empty() {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            // The word alone may still be too long: break it by characters.
            let mut piece = String::new();
            for ch in word.chars() {
                let mut t = piece.clone();
                t.push(ch);
                if font.measure(&t, size) > max_width && !piece.is_empty() {
                    lines.push(std::mem::take(&mut piece));
                }
                piece.push(ch);
            }
            line = piece;
        }
        lines.push(line);
    }
    lines
}

fn ellipse(cb: &mut ContentBuilder, r: &Rect) {
    const K: f64 = 0.5522847498;
    let (cx, cy) = (r.center().x, r.center().y);
    let (rx, ry) = (r.width() / 2.0, r.height() / 2.0);
    cb.move_to(cx + rx, cy)
        .curve_to(cx + rx, cy + ry * K, cx + rx * K, cy + ry, cx, cy + ry)
        .curve_to(cx - rx * K, cy + ry, cx - rx, cy + ry * K, cx - rx, cy)
        .curve_to(cx - rx, cy - ry * K, cx - rx * K, cy - ry, cx, cy - ry)
        .curve_to(cx + rx * K, cy - ry, cx + rx, cy - ry * K, cx + rx, cy)
        .close();
}

fn note_icon(cb: &mut ContentBuilder, icon: NoteIcon, color: [f64; 3]) {
    // 20 x 20 speech bubble; darker outline of the same hue.
    let dark = [color[0] * 0.55, color[1] * 0.55, color[2] * 0.55];
    cb.save()
        .line_width(1.0)
        .fill_rgb(color[0], color[1], color[2])
        .stroke_rgb(dark[0], dark[1], dark[2]);
    // Rounded rectangle body.
    let (x0, y0, x1, y1, r) = (1.0, 5.0, 19.0, 19.0, 3.0);
    cb.move_to(x0 + r, y0)
        .line_to(x1 - r, y0)
        .curve_to(x1, y0, x1, y0, x1, y0 + r)
        .line_to(x1, y1 - r)
        .curve_to(x1, y1, x1, y1, x1 - r, y1)
        .line_to(x0 + r, y1)
        .curve_to(x0, y1, x0, y1, x0, y1 - r)
        .line_to(x0, y0 + r)
        .curve_to(x0, y0, x0, y0, x0 + r, y0)
        .close()
        .fill_stroke();
    // Tail.
    cb.move_to(5.0, 5.5)
        .line_to(4.0, 1.0)
        .line_to(9.0, 5.5)
        .close()
        .fill_stroke();
    cb.stroke_rgb(dark[0], dark[1], dark[2]).line_width(1.2);
    match icon {
        NoteIcon::Comment | NoteIcon::Note => {
            for y in [15.0, 12.0, 9.0] {
                cb.move_to(5.0, y).line_to(15.0, y).stroke();
            }
        }
        NoteIcon::Key => {
            ellipse(cb, &Rect::new(5.0, 10.0, 10.0, 15.0));
            cb.stroke().move_to(10.0, 12.5).line_to(16.0, 8.5).stroke();
        }
        NoteIcon::Help => {
            cb.move_to(7.0, 14.0)
                .curve_to(7.0, 17.5, 13.0, 17.5, 13.0, 14.0)
                .curve_to(13.0, 11.5, 10.0, 12.0, 10.0, 9.5)
                .stroke()
                .rect(&Rect::new(9.3, 6.5, 10.7, 7.9))
                .fill();
        }
        NoteIcon::Paragraph => {
            cb.move_to(8.0, 7.0)
                .line_to(8.0, 16.0)
                .line_to(14.0, 16.0)
                .stroke();
            cb.move_to(11.0, 7.0).line_to(11.0, 16.0).stroke();
        }
        NoteIcon::Insert => {
            cb.move_to(10.0, 7.0).line_to(10.0, 16.0).stroke();
            cb.move_to(6.0, 12.0)
                .line_to(10.0, 16.0)
                .line_to(14.0, 12.0)
                .stroke();
        }
    }
    cb.restore();
}

/// Resolves a standard font by name, falling back to Helvetica.
pub(crate) fn standard_font(name: &str) -> StandardFont {
    StandardFont::by_name(name).unwrap_or(StandardFont::Helvetica)
}

struct Built {
    /// Bounds in display space.
    rect: Rect,
    form: Option<ObjRef>,
    extra: Dict,
    subtype: &'static str,
}

fn build(doc: &mut Document, info: &PageInfo, a: &Annotation) -> Result<Built> {
    let m_lin = info.display_to_user().linear();
    let to_user = |p: Point| to_user_point(info, p);
    let quad_points = |quads: &[Rect]| -> Object {
        let mut v = Vec::with_capacity(quads.len() * 8);
        for q in quads {
            for p in [
                Point::new(q.x0, q.y1),
                Point::new(q.x1, q.y1),
                Point::new(q.x0, q.y0),
                Point::new(q.x1, q.y0),
            ] {
                let u = to_user(p);
                v.push(Object::Real(u.x));
                v.push(Object::Real(u.y));
            }
        }
        Object::Array(v)
    };
    match a {
        Annotation::Highlight {
            quads,
            color,
            opacity,
        }
        | Annotation::Underline {
            quads,
            color,
            opacity,
        }
        | Annotation::StrikeOut {
            quads,
            color,
            opacity,
        } => {
            if quads.is_empty() {
                return Err(Error::Preset(
                    "annotation needs at least one rectangle".into(),
                ));
            }
            let rect = quads.iter().skip(1).fold(quads[0], |acc, q| acc.union(q));
            let is_hl = matches!(a, Annotation::Highlight { .. });
            let subtype = match a {
                Annotation::Highlight { .. } => "Highlight",
                Annotation::Underline { .. } => "Underline",
                _ => "StrikeOut",
            };
            let mut res = Dict::new();
            let mut cb = ContentBuilder::new();
            if let Some(gs) = gstate(doc, *opacity, is_hl) {
                res.set("ExtGState", Dict::new().with("GS0", gs));
                cb.ext_gstate("GS0");
            }
            cb.fill_rgb(color[0], color[1], color[2]);
            for q in quads {
                let local = q.translate(-rect.x0, -rect.y0);
                let r = match a {
                    Annotation::Highlight { .. } => local,
                    Annotation::Underline { .. } => {
                        let t = (local.height() * 0.06).max(0.8);
                        Rect::new(local.x0, local.y0, local.x1, local.y0 + t)
                    }
                    _ => {
                        let t = (local.height() * 0.06).max(0.8);
                        let mid = local.y0 + local.height() * 0.5;
                        Rect::new(local.x0, mid - t / 2.0, local.x1, mid + t / 2.0)
                    }
                };
                cb.rect(&r).fill();
            }
            let form = make_form(
                doc,
                rect.width(),
                rect.height(),
                Some(m_lin),
                res,
                cb.finish(),
            );
            let mut extra = Dict::new()
                .with("QuadPoints", quad_points(quads))
                .with("C", rgb(*color));
            if *opacity < 1.0 {
                extra.set("CA", *opacity);
            }
            Ok(Built {
                rect,
                form: Some(form),
                extra,
                subtype,
            })
        }
        Annotation::Square {
            rect,
            stroke,
            fill,
            width,
            opacity,
        }
        | Annotation::Circle {
            rect,
            stroke,
            fill,
            width,
            opacity,
        } => {
            let is_square = matches!(a, Annotation::Square { .. });
            let w = if stroke.is_some() {
                width.max(0.0)
            } else {
                0.0
            };
            let mut res = Dict::new();
            let mut cb = ContentBuilder::new();
            if let Some(gs) = gstate(doc, *opacity, false) {
                res.set("ExtGState", Dict::new().with("GS0", gs));
                cb.ext_gstate("GS0");
            }
            let inner = Rect::new(
                w / 2.0,
                w / 2.0,
                rect.width() - w / 2.0,
                rect.height() - w / 2.0,
            );
            if let Some(f) = fill {
                cb.fill_rgb(f[0], f[1], f[2]);
            }
            if let Some(s) = stroke {
                cb.stroke_rgb(s[0], s[1], s[2]).line_width(w);
            }
            if is_square {
                cb.rect(&inner);
            } else {
                ellipse(&mut cb, &inner);
            }
            match (fill.is_some(), stroke.is_some() && w > 0.0) {
                (true, true) => cb.fill_stroke(),
                (true, false) => cb.fill(),
                (false, true) => cb.stroke(),
                (false, false) => cb.end_path(),
            };
            let form = make_form(
                doc,
                rect.width(),
                rect.height(),
                Some(m_lin),
                res,
                cb.finish(),
            );
            let mut extra = Dict::new().with("BS", Dict::new().with("W", w));
            if let Some(s) = stroke {
                extra.set("C", rgb(*s));
            }
            if let Some(f) = fill {
                extra.set("IC", rgb(*f));
            }
            if *opacity < 1.0 {
                extra.set("CA", *opacity);
            }
            Ok(Built {
                rect: *rect,
                form: Some(form),
                extra,
                subtype: if is_square { "Square" } else { "Circle" },
            })
        }
        Annotation::Line {
            from,
            to,
            color,
            width,
            opacity,
        } => {
            let w = width.max(0.1);
            let rect = Rect::bounds([*from, *to]).unwrap().expand(w);
            let mut res = Dict::new();
            let mut cb = ContentBuilder::new();
            if let Some(gs) = gstate(doc, *opacity, false) {
                res.set("ExtGState", Dict::new().with("GS0", gs));
                cb.ext_gstate("GS0");
            }
            cb.stroke_rgb(color[0], color[1], color[2])
                .line_width(w)
                .line_cap(1)
                .move_to(from.x - rect.x0, from.y - rect.y0)
                .line_to(to.x - rect.x0, to.y - rect.y0)
                .stroke();
            let form = make_form(
                doc,
                rect.width(),
                rect.height(),
                Some(m_lin),
                res,
                cb.finish(),
            );
            let (uf, ut) = (to_user(*from), to_user(*to));
            let mut extra = Dict::new()
                .with(
                    "L",
                    Object::Array(vec![uf.x.into(), uf.y.into(), ut.x.into(), ut.y.into()]),
                )
                .with("C", rgb(*color))
                .with("BS", Dict::new().with("W", w));
            if *opacity < 1.0 {
                extra.set("CA", *opacity);
            }
            Ok(Built {
                rect,
                form: Some(form),
                extra,
                subtype: "Line",
            })
        }
        Annotation::Ink {
            paths,
            color,
            width,
            opacity,
        } => {
            let pts: Vec<Point> = paths.iter().flatten().copied().collect();
            let w = width.max(0.1);
            let rect = Rect::bounds(pts)
                .ok_or_else(|| Error::Preset("ink annotation needs at least one point".into()))?
                .expand(w);
            let mut res = Dict::new();
            let mut cb = ContentBuilder::new();
            if let Some(gs) = gstate(doc, *opacity, false) {
                res.set("ExtGState", Dict::new().with("GS0", gs));
                cb.ext_gstate("GS0");
            }
            cb.stroke_rgb(color[0], color[1], color[2])
                .line_width(w)
                .line_cap(1)
                .raw(b"1 j");
            let mut ink = Vec::with_capacity(paths.len());
            for path in paths.iter().filter(|p| !p.is_empty()) {
                for (i, p) in path.iter().enumerate() {
                    if i == 0 {
                        cb.move_to(p.x - rect.x0, p.y - rect.y0);
                    } else {
                        cb.line_to(p.x - rect.x0, p.y - rect.y0);
                    }
                }
                if path.len() == 1 {
                    cb.line_to(path[0].x - rect.x0, path[0].y - rect.y0);
                }
                cb.stroke();
                let mut arr = Vec::with_capacity(path.len() * 2);
                for p in path {
                    let u = to_user(*p);
                    arr.push(Object::Real(u.x));
                    arr.push(Object::Real(u.y));
                }
                ink.push(Object::Array(arr));
            }
            let form = make_form(
                doc,
                rect.width(),
                rect.height(),
                Some(m_lin),
                res,
                cb.finish(),
            );
            let mut extra = Dict::new()
                .with("InkList", Object::Array(ink))
                .with("C", rgb(*color))
                .with("BS", Dict::new().with("W", w));
            if *opacity < 1.0 {
                extra.set("CA", *opacity);
            }
            Ok(Built {
                rect,
                form: Some(form),
                extra,
                subtype: "Ink",
            })
        }
        Annotation::FreeText {
            rect,
            text,
            font,
            size,
            color,
            align,
            background,
            border,
            opacity,
        } => {
            let sf = standard_font(font);
            let font_ref = doc.add_font(Font::standard(sf));
            let size = if *size > 0.0 { *size } else { 12.0 };
            let (w, h) = (rect.width(), rect.height());
            let pad = 2.0;
            let mut res = Dict::new().with("Font", Dict::new().with("Helv", font_ref));
            let mut cb = ContentBuilder::new();
            if let Some(gs) = gstate(doc, *opacity, false) {
                res.set("ExtGState", Dict::new().with("GS0", gs));
                cb.ext_gstate("GS0");
            }
            if let Some(bg) = background {
                cb.fill_rgb(bg[0], bg[1], bg[2])
                    .rect(&Rect::new(0.0, 0.0, w, h))
                    .fill();
            }
            if let Some(bc) = border {
                cb.stroke_rgb(bc[0], bc[1], bc[2])
                    .line_width(1.0)
                    .rect(&Rect::new(0.5, 0.5, w - 0.5, h - 0.5))
                    .stroke();
            }
            let (lines, encoded, ascent, descent) = {
                let f = doc.font_mut(font_ref).expect("font registered");
                let lines = wrap_text(f, size, text, (w - 2.0 * pad).max(1.0));
                let enc: Vec<(Vec<u8>, f64)> = lines
                    .iter()
                    .map(|l| (f.encode(l), f.measure(l, size)))
                    .collect();
                (
                    lines,
                    enc,
                    f.ascent() * size / 1000.0,
                    f.descent() * size / 1000.0,
                )
            };
            let leading = size * 1.18;
            cb.save()
                .rect(&Rect::new(
                    pad / 2.0,
                    pad / 2.0,
                    w - pad / 2.0,
                    h - pad / 2.0,
                ))
                .clip()
                .end_path()
                .fill_rgb(color[0], color[1], color[2])
                .begin_text()
                .font("Helv", size);
            let mut y = h - pad - ascent;
            for (i, (bytes, width)) in encoded.iter().enumerate() {
                let x = match align {
                    Align::Left => pad,
                    Align::Center => (w - width) / 2.0,
                    Align::Right => w - pad - width,
                };
                cb.text_matrix(&Matrix::translate(x, y)).show_literal(bytes);
                y -= leading;
                if y + ascent < -descent && i + 1 < lines.len() {
                    break;
                }
            }
            cb.end_text().restore();
            let form = make_form(doc, w, h, Some(m_lin), res, cb.finish());
            let mut da = Vec::new();
            da.extend_from_slice(b"/Helv ");
            write_num(&mut da, size);
            da.extend_from_slice(b" Tf ");
            for c in color {
                write_num(&mut da, *c);
                da.push(b' ');
            }
            da.extend_from_slice(b"rg");
            let mut extra = Dict::new()
                .with("DA", PdfString::new(da))
                .with(
                    "Q",
                    match align {
                        Align::Left => 0,
                        Align::Center => 1,
                        Align::Right => 2,
                    },
                )
                .with("Contents", PdfString::from_text(text));
            if let Some(bc) = border {
                extra.set("C", rgb(*bc));
            } else {
                extra.set("Border", Object::Array(vec![0.into(), 0.into(), 0.into()]));
            }
            if *opacity < 1.0 {
                extra.set("CA", *opacity);
            }
            Ok(Built {
                rect: *rect,
                form: Some(form),
                extra,
                subtype: "FreeText",
            })
        }
        Annotation::Note { at, icon, color } => {
            let rect = Rect::from_xywh(at.x, at.y - 20.0, 20.0, 20.0);
            let mut cb = ContentBuilder::new();
            note_icon(&mut cb, *icon, *color);
            let form = make_form(doc, 20.0, 20.0, Some(m_lin), Dict::new(), cb.finish());
            let extra = Dict::new()
                .with("Name", icon.name())
                .with("C", rgb(*color))
                .with("Open", false);
            Ok(Built {
                rect,
                form: Some(form),
                extra,
                subtype: "Text",
            })
        }
        Annotation::Link { rect, uri, page } => {
            let mut extra =
                Dict::new().with("Border", Object::Array(vec![0.into(), 0.into(), 0.into()]));
            match (uri, page) {
                (Some(u), _) if !u.is_empty() => {
                    extra.set(
                        "A",
                        Dict::new()
                            .with("S", "URI")
                            .with("URI", PdfString::new(u.as_bytes().to_vec())),
                    );
                }
                (_, Some(p)) => {
                    let target = doc.page_ref(*p)?;
                    extra.set(
                        "Dest",
                        Object::Array(vec![target.into(), Object::name("Fit")]),
                    );
                }
                _ => return Err(Error::Preset("link needs a uri or a page".into())),
            }
            Ok(Built {
                rect: *rect,
                form: None,
                extra,
                subtype: "Link",
            })
        }
    }
}

fn finish(
    doc: &mut Document,
    page: usize,
    info: &PageInfo,
    built: Built,
    meta: &AnnotationMeta,
) -> Result<ObjRef> {
    let mut d = built.extra;
    d.set("Type", "Annot");
    d.set("Subtype", built.subtype);
    d.set("Rect", to_user_rect(info, &built.rect).to_object());
    d.set("P", info.obj);
    let mut flags = if meta.print.unwrap_or(true) { 4 } else { 0 };
    if built.subtype == "Text" {
        flags |= 8 | 16; // NoZoom, NoRotate
    }
    d.set("F", flags);
    if let Some(f) = built.form {
        d.set("AP", Dict::new().with("N", f));
    }
    if let Some(t) = &meta.author {
        d.set("T", PdfString::from_text(t));
    }
    if let Some(c) = &meta.contents {
        d.set("Contents", PdfString::from_text(c));
    }
    if let Some(s) = &meta.subject {
        d.set("Subj", PdfString::from_text(s));
    }
    if let Some(m) = &meta.modified {
        d.set("M", PdfString::new(m.as_bytes().to_vec()));
    }
    let r = doc.add(d.into());
    if let Some(dd) = doc.get_mut(r).and_then(Object::as_dict_mut) {
        dd.set(
            "NM",
            PdfString::new(format!("folio-{}", r.num).into_bytes()),
        );
    }
    doc.push_annot(page, r)?;
    Ok(r)
}

/// Adds an annotation to page `page`. Returns its object reference (the
/// object number is what [`FlattenOptions::objects`] and
/// [`AnnotInfo::object`] refer to).
pub fn add_annotation(
    doc: &mut Document,
    page: usize,
    annot: &Annotation,
    meta: &AnnotationMeta,
) -> Result<ObjRef> {
    let info = doc.page_info(page)?;
    let built = build(doc, &info, annot)?;
    finish(doc, page, &info, built, meta)
}

/// Adds an image (JPEG or PNG bytes) as a stamp annotation filling `rect`
/// (display space). Used for signatures and logos placed by hand.
pub fn add_image_annotation(
    doc: &mut Document,
    page: usize,
    rect: Rect,
    image: &[u8],
    opacity: f64,
    meta: &AnnotationMeta,
) -> Result<ObjRef> {
    let info = doc.page_info(page)?;
    let img = Image::load(image)?;
    let img_ref = doc.add_image(&img, 6);
    let (w, h) = (rect.width().max(0.1), rect.height().max(0.1));
    let mut res = Dict::new().with("XObject", Dict::new().with("Im0", img_ref));
    let mut cb = ContentBuilder::new();
    if let Some(gs) = gstate(doc, opacity, false) {
        res.set("ExtGState", Dict::new().with("GS0", gs));
        cb.ext_gstate("GS0");
    }
    cb.image("Im0", &Rect::new(0.0, 0.0, w, h));
    let form = make_form(
        doc,
        w,
        h,
        Some(info.display_to_user().linear()),
        res,
        cb.finish(),
    );
    let mut extra = Dict::new().with("Name", "FolioImage");
    if opacity < 1.0 {
        extra.set("CA", opacity);
    }
    finish(
        doc,
        page,
        &info,
        Built {
            rect,
            form: Some(form),
            extra,
            subtype: "Stamp",
        },
        meta,
    )
}

// ---------------------------------------------------------------------------
// Inspect and remove
// ---------------------------------------------------------------------------

fn text_of(doc: &Document, d: &Dict, key: &str) -> Option<String> {
    doc.dict_get(d, key)
        .and_then(Object::as_string)
        .map(PdfString::to_text)
}

/// Fully qualified field name of a widget (walks `/Parent`).
pub(crate) fn field_name(doc: &Document, widget: ObjRef) -> Option<String> {
    let mut parts = Vec::new();
    let mut cur = Some(widget);
    let mut guard = 0;
    while let Some(r) = cur {
        guard += 1;
        if guard > 64 {
            break;
        }
        let d = doc.get(r).as_dict()?;
        if let Some(t) = text_of(doc, d, "T") {
            parts.push(t);
        }
        cur = d.get("Parent").and_then(Object::as_reference);
    }
    if parts.is_empty() {
        return None;
    }
    parts.reverse();
    Some(parts.join("."))
}

/// Lists the annotations on a page.
pub fn list_annotations(doc: &Document, page: usize) -> Result<Vec<AnnotInfo>> {
    let info = doc.page_info(page)?;
    let mut out = Vec::new();
    for (index, r) in doc.page_annots(page)?.into_iter().enumerate() {
        let d = match doc.get(r).as_dict() {
            Some(d) => d,
            None => continue,
        };
        let subtype = d
            .get("Subtype")
            .and_then(Object::as_name)
            .map(|n| n.as_str().into_owned())
            .unwrap_or_default();
        let rect = d
            .get("Rect")
            .map(|o| doc.resolve(o))
            .and_then(Rect::from_object)
            .map(|u| to_display_rect(&info, &u))
            .unwrap_or_default();
        let flags = d.get("F").and_then(Object::as_i64).unwrap_or(0);
        let field = if subtype == "Widget" {
            field_name(doc, r)
        } else {
            None
        };
        out.push(AnnotInfo {
            index,
            object: r.num,
            subtype,
            rect,
            contents: text_of(doc, d, "Contents"),
            author: text_of(doc, d, "T"),
            hidden: flags & 2 != 0,
            field,
            has_appearance: d
                .get("AP")
                .map(|o| doc.resolve(o))
                .and_then(Object::as_dict)
                .map(|ap| ap.contains("N"))
                .unwrap_or(false),
        });
    }
    Ok(out)
}

/// Removes the annotation at `index` in the page's list (see
/// [`list_annotations`]). Its popup, if any, goes too.
pub fn remove_annotation(doc: &mut Document, page: usize, index: usize) -> Result<()> {
    let list = doc.page_annots(page)?;
    let r = *list
        .get(index)
        .ok_or_else(|| Error::Preset(format!("no annotation {index} on page {}", page + 1)))?;
    remove_objects(doc, page, &[r.num].into_iter().collect())
}

fn remove_objects(doc: &mut Document, page: usize, gone: &HashSet<u32>) -> Result<()> {
    let list = doc.page_annots(page)?;
    let mut popups = HashSet::new();
    for r in &list {
        if gone.contains(&r.num) {
            if let Some(p) = doc
                .get(*r)
                .as_dict()
                .and_then(|d| d.get("Popup"))
                .and_then(Object::as_reference)
            {
                popups.insert(p.num);
            }
        }
    }
    let keep: Vec<ObjRef> = list
        .into_iter()
        .filter(|r| !gone.contains(&r.num) && !popups.contains(&r.num))
        .collect();
    doc.set_page_annots(page, &keep)?;
    for n in gone.iter().chain(popups.iter()) {
        doc.remove_object(ObjRef::new(*n, 0));
    }
    Ok(())
}

/// Removes annotations matching `opts` from `pages`. Returns how many went.
/// Widgets are only removed when `opts.widgets` is true (default true, as
/// for flattening), and the form field tree is pruned accordingly.
pub fn remove_annotations(
    doc: &mut Document,
    pages: &[usize],
    opts: &FlattenOptions,
) -> Result<usize> {
    let mut total = 0;
    let mut removed_widgets = HashSet::new();
    for &page in pages {
        let mut gone = HashSet::new();
        for r in doc.page_annots(page)? {
            if selected(doc, r, opts) {
                if is_widget(doc, r) {
                    removed_widgets.insert(r.num);
                }
                gone.insert(r.num);
            }
        }
        total += gone.len();
        remove_objects(doc, page, &gone)?;
    }
    if !removed_widgets.is_empty() {
        crate::forms::prune_fields(doc, &removed_widgets);
    }
    Ok(total)
}

fn is_widget(doc: &Document, r: ObjRef) -> bool {
    doc.get(r)
        .as_dict()
        .and_then(|d| d.get("Subtype"))
        .and_then(Object::as_name)
        .map(|n| n == "Widget")
        .unwrap_or(false)
}

fn selected(doc: &Document, r: ObjRef, opts: &FlattenOptions) -> bool {
    let d = match doc.get(r).as_dict() {
        Some(d) => d,
        None => return false,
    };
    let subtype = d
        .get("Subtype")
        .and_then(Object::as_name)
        .map(|n| n.as_str().into_owned())
        .unwrap_or_default();
    if subtype == "Popup" {
        return false;
    }
    if subtype == "Widget" && !opts.widgets.unwrap_or(true) {
        return false;
    }
    if let Some(objs) = &opts.objects {
        if !objs.contains(&r.num) {
            return false;
        }
    }
    if let Some(subs) = &opts.subtypes {
        if !subs.iter().any(|s| s.eq_ignore_ascii_case(&subtype)) {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// Flatten
// ---------------------------------------------------------------------------

/// The normal appearance stream of an annotation, honouring `/AS`.
pub(crate) fn normal_appearance(doc: &Document, annot: &Dict) -> Option<ObjRef> {
    let ap = doc.resolve(annot.get("AP")?).as_dict()?;
    let n = ap.get("N")?;
    match doc.resolve(n) {
        Object::Stream(_) => n.as_reference(),
        Object::Dict(states) => {
            if let Some(state) = annot.get("AS").and_then(Object::as_name) {
                return states.get(&state.as_str()).and_then(Object::as_reference);
            }
            if states.len() == 1 {
                return states.iter().next().and_then(|(_, v)| v.as_reference());
            }
            None
        }
        _ => None,
    }
}

/// Paints annotations into the page content and removes them. Hidden
/// annotations and popups are removed without painting; annotations without
/// an appearance stream (typically links) are left in place. Returns how
/// many annotations were flattened.
pub fn flatten_annotations(
    doc: &mut Document,
    pages: &[usize],
    opts: &FlattenOptions,
) -> Result<usize> {
    let mut total = 0;
    let mut removed_widgets = HashSet::new();
    for &page in pages {
        let list = doc.page_annots(page)?;
        let mut gone = HashSet::new();
        let mut content = Vec::new();
        for r in list {
            if !selected(doc, r, opts) {
                continue;
            }
            let d = match doc.get(r).as_dict() {
                Some(d) => d.clone(),
                None => continue,
            };
            let flags = d.get("F").and_then(Object::as_i64).unwrap_or(0);
            let hidden = flags & 2 != 0 || flags & 32 != 0;
            let rect = d
                .get("Rect")
                .map(|o| doc.resolve(o))
                .and_then(Rect::from_object);
            let ap = normal_appearance(doc, &d);
            match (hidden, rect, ap) {
                (true, _, _) => {}
                (false, Some(rect), Some(form)) => {
                    if let Some(bytes) = draw_form(doc, page, form, &rect)? {
                        content.extend_from_slice(&bytes);
                    }
                }
                _ => continue, // nothing to paint: keep the annotation
            }
            if is_widget(doc, r) {
                removed_widgets.insert(r.num);
            }
            gone.insert(r.num);
            total += 1;
        }
        if !content.is_empty() {
            doc.draw(page, &content)?;
        }
        remove_objects(doc, page, &gone)?;
    }
    if !removed_widgets.is_empty() {
        crate::forms::prune_fields(doc, &removed_widgets);
    }
    Ok(total)
}

/// Content that paints form XObject `form` so its (transformed) bounding
/// box fills `rect` (ISO 32000-1 §12.5.5). `None` if the form is unusable.
fn draw_form(
    doc: &mut Document,
    page: usize,
    form: ObjRef,
    rect: &Rect,
) -> Result<Option<Vec<u8>>> {
    let (bbox, matrix) = {
        let s = match doc.get(form).as_stream() {
            Some(s) => s,
            None => return Ok(None),
        };
        let bbox = match s
            .dict
            .get("BBox")
            .map(|o| doc.resolve(o))
            .and_then(Rect::from_object)
        {
            Some(b) => b,
            None => return Ok(None),
        };
        let m = s
            .dict
            .get("Matrix")
            .map(|o| doc.resolve(o))
            .and_then(Matrix::from_object)
            .unwrap_or(Matrix::IDENTITY);
        (bbox, m)
    };
    if let Some(s) = doc.get_mut(form).and_then(Object::as_stream_mut) {
        s.dict.set("Type", "XObject");
        s.dict.set("Subtype", "Form");
    }
    let tb = bbox.transform(&matrix);
    let sx = if tb.width() > 1e-9 {
        rect.width() / tb.width()
    } else {
        1.0
    };
    let sy = if tb.height() > 1e-9 {
        rect.height() / tb.height()
    } else {
        1.0
    };
    let a = Matrix::new(sx, 0.0, 0.0, sy, rect.x0 - tb.x0 * sx, rect.y0 - tb.y0 * sy);
    let name = doc.add_page_resource(page, "XObject", form)?;
    let mut cb = ContentBuilder::new();
    cb.save().transform(&a).xobject(&name).restore();
    Ok(Some(cb.finish()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::PageSize;

    fn doc() -> Document {
        let mut d = Document::new();
        d.add_page(PageSize::LETTER);
        d
    }

    #[test]
    fn highlight_roundtrip() {
        let mut d = doc();
        let r = add_annotation(
            &mut d,
            0,
            &Annotation::Highlight {
                quads: vec![
                    Rect::new(72.0, 700.0, 300.0, 712.0),
                    Rect::new(72.0, 686.0, 200.0, 698.0),
                ],
                color: yellow(),
                opacity: 1.0,
            },
            &AnnotationMeta {
                author: Some("Ada".into()),
                contents: Some("check this".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let list = list_annotations(&d, 0).unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].object, r.num);
        assert_eq!(list[0].subtype, "Highlight");
        assert_eq!(list[0].author.as_deref(), Some("Ada"));
        assert!(list[0].has_appearance);
        assert!((list[0].rect.x0 - 72.0).abs() < 1e-6 && (list[0].rect.y1 - 712.0).abs() < 1e-6);
        let bytes = d.save(&Default::default()).unwrap();
        let d2 = Document::load(&bytes).unwrap();
        let l2 = list_annotations(&d2, 0).unwrap();
        assert_eq!(l2.len(), 1);
        assert_eq!(l2[0].contents.as_deref(), Some("check this"));
    }

    #[test]
    fn rotated_page_maps_display_space() {
        let mut d = Document::new();
        d.add_page(PageSize::new(200.0, 100.0));
        d.rotate_page(0, 90).unwrap();
        // Displayed page is 100 wide × 200 tall. A box at display (10, 20)-(30, 40).
        let r = Rect::new(10.0, 20.0, 30.0, 40.0);
        add_annotation(
            &mut d,
            0,
            &Annotation::Square {
                rect: r,
                stroke: red_opt(),
                fill: None,
                width: 1.0,
                opacity: 1.0,
            },
            &Default::default(),
        )
        .unwrap();
        let list = list_annotations(&d, 0).unwrap();
        assert!(
            (list[0].rect.x0 - 10.0).abs() < 1e-6 && (list[0].rect.y1 - 40.0).abs() < 1e-6,
            "{:?}",
            list[0].rect
        );
        // In user space the rectangle is elsewhere.
        let annots = d.page_annots(0).unwrap();
        let user =
            Rect::from_object(d.get(annots[0]).as_dict().unwrap().get("Rect").unwrap()).unwrap();
        assert!(
            (user.x0 - 160.0).abs() < 1e-6 && (user.y0 - 10.0).abs() < 1e-6,
            "{user:?}"
        );
        // The appearance stream carries the rotation.
        let ap = normal_appearance(&d, d.get(annots[0]).as_dict().unwrap()).unwrap();
        let m = Matrix::from_object(d.get(ap).as_stream().unwrap().dict.get("Matrix").unwrap())
            .unwrap();
        assert_eq!((m.a, m.b, m.c, m.d), (0.0, 1.0, -1.0, 0.0));
    }

    #[test]
    fn free_text_wraps_and_flattens() {
        let mut d = doc();
        let r = add_annotation(
            &mut d,
            0,
            &Annotation::FreeText {
                rect: Rect::new(72.0, 600.0, 200.0, 700.0),
                text: "The quick brown fox jumps over the lazy dog again and again".into(),
                font: "Helvetica".into(),
                size: 12.0,
                color: black(),
                align: Align::Left,
                background: Some([1.0, 1.0, 0.8]),
                border: Some(black()),
                opacity: 1.0,
            },
            &Default::default(),
        )
        .unwrap();
        let ap = normal_appearance(&d, d.get(r).as_dict().unwrap()).unwrap();
        let data = d.stream_data(d.get(ap).as_stream().unwrap()).unwrap();
        let text = String::from_utf8_lossy(&data);
        assert!(
            text.matches("Tj").count() >= 3,
            "wrapped into several lines: {text}"
        );
        let n = flatten_annotations(&mut d, &[0], &Default::default()).unwrap();
        assert_eq!(n, 1);
        assert!(list_annotations(&d, 0).unwrap().is_empty());
        let content = String::from_utf8_lossy(&d.page_content(0).unwrap()).into_owned();
        assert!(content.contains("/X1 Do"), "{content}");
        assert!(content.contains("cm"), "{content}");
        d.save(&Default::default()).unwrap();
    }

    #[test]
    fn ink_note_link_image() {
        let mut d = doc();
        add_annotation(
            &mut d,
            0,
            &Annotation::Ink {
                paths: vec![
                    vec![Point::new(10.0, 10.0), Point::new(50.0, 60.0)],
                    vec![Point::new(20.0, 20.0)],
                ],
                color: red(),
                width: 3.0,
                opacity: 0.7,
            },
            &Default::default(),
        )
        .unwrap();
        add_annotation(
            &mut d,
            0,
            &Annotation::Note {
                at: Point::new(100.0, 700.0),
                icon: NoteIcon::Help,
                color: yellow(),
            },
            &AnnotationMeta {
                contents: Some("hi".into()),
                ..Default::default()
            },
        )
        .unwrap();
        add_annotation(
            &mut d,
            0,
            &Annotation::Link {
                rect: Rect::new(0.0, 0.0, 100.0, 20.0),
                uri: Some("https://example.com".into()),
                page: None,
            },
            &Default::default(),
        )
        .unwrap();
        add_annotation(
            &mut d,
            0,
            &Annotation::Line {
                from: Point::new(0.0, 0.0),
                to: Point::new(100.0, 100.0),
                color: red(),
                width: 1.0,
                opacity: 1.0,
            },
            &Default::default(),
        )
        .unwrap();
        add_annotation(
            &mut d,
            0,
            &Annotation::Circle {
                rect: Rect::new(10.0, 10.0, 60.0, 40.0),
                stroke: None,
                fill: Some(yellow()),
                width: 0.0,
                opacity: 1.0,
            },
            &Default::default(),
        )
        .unwrap();
        // A 2x2 opaque PNG.
        let png = crate::image::tests::tiny_png();
        add_image_annotation(
            &mut d,
            0,
            Rect::new(300.0, 300.0, 400.0, 350.0),
            &png,
            1.0,
            &Default::default(),
        )
        .unwrap();
        let list = list_annotations(&d, 0).unwrap();
        assert_eq!(
            list.iter().map(|a| a.subtype.as_str()).collect::<Vec<_>>(),
            ["Ink", "Text", "Link", "Line", "Circle", "Stamp"]
        );
        // Flatten everything but links (they have no appearance and stay).
        let n = flatten_annotations(&mut d, &[0], &Default::default()).unwrap();
        assert_eq!(n, 5);
        let rest = list_annotations(&d, 0).unwrap();
        assert_eq!(rest.len(), 1);
        assert_eq!(rest[0].subtype, "Link");
        remove_annotation(&mut d, 0, 0).unwrap();
        assert!(list_annotations(&d, 0).unwrap().is_empty());
        let bytes = d.save(&Default::default()).unwrap();
        Document::load(&bytes).unwrap();
    }

    #[test]
    fn selective_flatten_by_object() {
        let mut d = doc();
        let a = add_annotation(
            &mut d,
            0,
            &Annotation::Square {
                rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                stroke: red_opt(),
                fill: None,
                width: 1.0,
                opacity: 1.0,
            },
            &Default::default(),
        )
        .unwrap();
        let _b = add_annotation(
            &mut d,
            0,
            &Annotation::Square {
                rect: Rect::new(20.0, 20.0, 30.0, 30.0),
                stroke: red_opt(),
                fill: None,
                width: 1.0,
                opacity: 1.0,
            },
            &Default::default(),
        )
        .unwrap();
        let n = flatten_annotations(
            &mut d,
            &[0],
            &FlattenOptions {
                objects: Some(vec![a.num]),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(n, 1);
        let rest = list_annotations(&d, 0).unwrap();
        assert_eq!(rest.len(), 1);
        assert!((rest[0].rect.x0 - 20.0).abs() < 1e-6);
        assert_eq!(
            remove_annotations(
                &mut d,
                &[0],
                &FlattenOptions {
                    subtypes: Some(vec!["square".into()]),
                    ..Default::default()
                }
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn wrapping() {
        let f = Font::standard(StandardFont::Helvetica);
        let lines = wrap_text(&f, 10.0, "aaa bbb ccc\n\nlongwordthatdoesnotfit", 40.0);
        assert_eq!(lines[0], "aaa bbb");
        assert_eq!(lines[1], "ccc");
        assert_eq!(lines[2], "");
        assert!(lines.len() > 4);
    }
}
