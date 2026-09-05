//! Higher-level operations: page ranges, merge, split, stamps and page
//! numbers. Everything here is built on the [`Document`] API.

use serde::{Deserialize, Serialize};

use crate::content::ContentBuilder;
use crate::document::{Document, Metadata};
use crate::error::{Error, Result};
use crate::font::{Font, StandardFont};
use crate::geometry::{Matrix, Rect};
use crate::image::Image;
use crate::object::{ObjRef, Object};
use crate::page::PageSize;

// ---------------------------------------------------------------------------
// Page ranges
// ---------------------------------------------------------------------------

/// Parses a 1-based page range expression into 0-based indices.
///
/// Grammar (comma separated, whitespace ignored):
/// * `3` – a single page
/// * `2-5` – inclusive range; `4-` to the end; `-3` from the start
/// * `all`, `first`, `last`, `even`, `odd`
/// * `r2` – the second page from the end
///
/// Pages appear in the order written; duplicates are kept.
pub fn parse_page_ranges(spec: &str, page_count: usize) -> Result<Vec<usize>> {
    let bad = || Error::PageRange(spec.to_string());
    let mut out = Vec::new();
    if page_count == 0 {
        return Ok(out);
    }
    let n = page_count;
    let resolve = |tok: &str| -> Result<usize> {
        let t = tok.trim();
        if let Some(r) = t.strip_prefix('r') {
            let k: usize = r.parse().map_err(|_| bad())?;
            if k == 0 || k > n {
                return Err(bad());
            }
            return Ok(n - k);
        }
        match t {
            "first" => Ok(0),
            "last" => Ok(n - 1),
            _ => {
                let k: usize = t.parse().map_err(|_| bad())?;
                if k == 0 || k > n {
                    return Err(Error::PageOutOfRange {
                        index: k.saturating_sub(1),
                        count: n,
                    });
                }
                Ok(k - 1)
            }
        }
    };
    for part in spec.split(',') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        match p {
            "all" | "*" => out.extend(0..n),
            "even" => out.extend((1..n).step_by(2)),
            "odd" => out.extend((0..n).step_by(2)),
            _ if p.contains('-') && !p.starts_with('r') => {
                let (a, b) = p.split_once('-').ok_or_else(bad)?;
                let start = if a.trim().is_empty() { 0 } else { resolve(a)? };
                let end = if b.trim().is_empty() {
                    n - 1
                } else {
                    resolve(b)?
                };
                if start <= end {
                    out.extend(start..=end);
                } else {
                    out.extend((end..=start).rev());
                }
            }
            _ => out.push(resolve(p)?),
        }
    }
    Ok(out)
}

/// Splits `0..count` into consecutive chunks of `every` pages.
pub fn chunk_pages(count: usize, every: usize) -> Vec<Vec<usize>> {
    if every == 0 || count == 0 {
        return vec![(0..count).collect()];
    }
    (0..count)
        .collect::<Vec<_>>()
        .chunks(every)
        .map(|c| c.to_vec())
        .collect()
}

// ---------------------------------------------------------------------------
// Merge / split
// ---------------------------------------------------------------------------

/// Merges documents in order into a new one. Metadata is taken from the
/// first document.
pub fn merge(docs: &[&Document]) -> Result<Document> {
    let mut out = Document::new();
    for (i, d) in docs.iter().enumerate() {
        let all: Vec<usize> = (0..d.page_count()).collect();
        out.import_pages(d, &all, None)?;
        if i == 0 {
            out.set_metadata(&d.metadata());
        }
    }
    Ok(out)
}

/// A new document containing only `pages` (0-based, in the given order).
pub fn extract(doc: &Document, pages: &[usize]) -> Result<Document> {
    let mut out = Document::new();
    out.import_pages(doc, pages, None)?;
    out.set_metadata(&doc.metadata());
    Ok(out)
}

/// Splits into one document per chunk.
pub fn split(doc: &Document, chunks: &[Vec<usize>]) -> Result<Vec<Document>> {
    chunks.iter().map(|c| extract(doc, c)).collect()
}

// ---------------------------------------------------------------------------
// Stamps
// ---------------------------------------------------------------------------

/// Anchor position on the page (in display orientation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[allow(missing_docs)]
pub enum Position {
    TopLeft,
    TopCenter,
    TopRight,
    CenterLeft,
    #[default]
    Center,
    CenterRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl Position {
    /// Anchor point for a box of `w × h` inside `area` with `margin`.
    /// Returns the bottom-left corner of the box.
    pub fn place(&self, area_w: f64, area_h: f64, w: f64, h: f64, margin: f64) -> (f64, f64) {
        let x = match self {
            Position::TopLeft | Position::CenterLeft | Position::BottomLeft => margin,
            Position::TopCenter | Position::Center | Position::BottomCenter => (area_w - w) / 2.0,
            Position::TopRight | Position::CenterRight | Position::BottomRight => {
                area_w - w - margin
            }
        };
        let y = match self {
            Position::TopLeft | Position::TopCenter | Position::TopRight => area_h - h - margin,
            Position::CenterLeft | Position::Center | Position::CenterRight => (area_h - h) / 2.0,
            Position::BottomLeft | Position::BottomCenter | Position::BottomRight => margin,
        };
        (x, y)
    }
}

/// A text stamp or watermark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TextStamp {
    /// Text to draw. `{page}` and `{pages}` are substituted.
    pub text: String,
    /// Standard font name (`Helvetica`, `Times-Bold`, `Courier`, ...).
    pub font: String,
    /// Font size in points.
    pub size: f64,
    /// Fill colour as RGB in 0..1.
    pub color: [f64; 3],
    /// Opacity 0..1.
    pub opacity: f64,
    /// Where to anchor the text.
    pub position: Position,
    /// Counter-clockwise rotation in degrees about the text centre.
    pub rotation: f64,
    /// Distance from the page edge in points.
    pub margin: f64,
    /// Draw beneath the existing content instead of on top.
    pub under: bool,
}

impl Default for TextStamp {
    fn default() -> Self {
        Self {
            text: String::new(),
            font: "Helvetica".into(),
            size: 36.0,
            color: [0.5, 0.5, 0.5],
            opacity: 0.5,
            position: Position::Center,
            rotation: 0.0,
            margin: 36.0,
            under: false,
        }
    }
}

impl TextStamp {
    /// A grey diagonal watermark.
    pub fn watermark(text: &str) -> Self {
        Self {
            text: text.into(),
            size: 60.0,
            rotation: 45.0,
            opacity: 0.3,
            ..Default::default()
        }
    }
}

fn font_for(doc: &mut Document, name: &str) -> Result<ObjRef> {
    let sf = StandardFont::by_name(name)
        .ok_or_else(|| Error::Font(format!("unknown standard font '{name}'")))?;
    Ok(doc.add_font(Font::standard(sf)))
}

/// Draws `stamp` on each listed page.
pub fn stamp_text(doc: &mut Document, pages: &[usize], stamp: &TextStamp) -> Result<()> {
    let font_ref = font_for(doc, &stamp.font)?;
    let total = doc.page_count();
    let gs_ref = if stamp.opacity < 1.0 {
        Some(doc.add_opacity_state(stamp.opacity.clamp(0.0, 1.0)))
    } else {
        None
    };
    for &idx in pages {
        let info = doc.page_info(idx)?;
        let text = stamp
            .text
            .replace("{page}", &(idx + 1).to_string())
            .replace("{pages}", &total.to_string());
        let font_name = doc.add_page_resource(idx, "Font", font_ref)?;
        let gs_name = match gs_ref {
            Some(g) => Some(doc.add_page_resource(idx, "ExtGState", g)?),
            None => None,
        };
        let (encoded, width, ascent) = {
            let f = doc.font_mut(font_ref).expect("font registered");
            (
                f.encode(&text),
                f.measure(&text, stamp.size),
                f.ascent() * stamp.size / 1000.0,
            )
        };
        let h = ascent;
        let (w_disp, h_disp) = (info.display_width(), info.display_height());
        // Bounding box of the rotated text, for placement.
        let rad = stamp.rotation.to_radians();
        let (s, c) = (rad.sin().abs(), rad.cos().abs());
        let (bw, bh) = (width * c + h * s, width * s + h * c);
        let (bx, by) = stamp.position.place(w_disp, h_disp, bw, bh, stamp.margin);
        let (cx, cy) = (bx + bw / 2.0, by + bh / 2.0);
        let tm = Matrix::translate(-width / 2.0, -h / 2.0)
            .then(&Matrix::rotate_deg(stamp.rotation))
            .then(&Matrix::translate(cx, cy));
        let mut cb = ContentBuilder::new();
        cb.save();
        if let Some(g) = &gs_name {
            cb.ext_gstate(g);
        }
        cb.transform(&info.display_to_user())
            .fill_rgb(stamp.color[0], stamp.color[1], stamp.color[2])
            .begin_text()
            .font(&font_name, stamp.size)
            .text_matrix(&tm);
        if doc.font(font_ref).map(|f| f.is_two_byte()).unwrap_or(false) {
            cb.show_bytes(&encoded);
        } else {
            cb.show_literal(&encoded);
        }
        cb.end_text().restore();
        let content = cb.finish();
        if stamp.under {
            doc.draw_under(idx, &content)?;
        } else {
            doc.draw(idx, &content)?;
        }
    }
    Ok(())
}

/// An image stamp (logo, signature, watermark image).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImageStamp {
    /// Width in points. `None` uses the pixel size at 72 dpi.
    pub width: Option<f64>,
    /// Height in points. `None` keeps the aspect ratio.
    pub height: Option<f64>,
    /// Opacity 0..1.
    pub opacity: f64,
    /// Anchor.
    pub position: Position,
    /// Margin from the edge in points.
    pub margin: f64,
    /// Draw beneath the existing content.
    pub under: bool,
}

impl Default for ImageStamp {
    fn default() -> Self {
        Self {
            width: None,
            height: None,
            opacity: 1.0,
            position: Position::BottomRight,
            margin: 36.0,
            under: false,
        }
    }
}

/// Draws `image` (JPEG or PNG bytes) on each listed page.
pub fn stamp_image(
    doc: &mut Document,
    pages: &[usize],
    image: &[u8],
    stamp: &ImageStamp,
) -> Result<()> {
    let img = Image::load(image)?;
    let xref = doc.add_image(&img, 6);
    let gs_ref = if stamp.opacity < 1.0 {
        Some(doc.add_opacity_state(stamp.opacity.clamp(0.0, 1.0)))
    } else {
        None
    };
    let aspect = img.height as f64 / img.width as f64;
    for &idx in pages {
        let info = doc.page_info(idx)?;
        let (w_disp, h_disp) = (info.display_width(), info.display_height());
        let mut w = stamp.width.unwrap_or(img.width as f64);
        let mut h = stamp.height.unwrap_or(w * aspect);
        if stamp.width.is_none() && stamp.height.is_some() {
            w = h / aspect;
        }
        // Never exceed the page.
        let max_w = (w_disp - 2.0 * stamp.margin).max(1.0);
        let max_h = (h_disp - 2.0 * stamp.margin).max(1.0);
        if w > max_w {
            let f = max_w / w;
            w *= f;
            h *= f;
        }
        if h > max_h {
            let f = max_h / h;
            w *= f;
            h *= f;
        }
        let (x, y) = stamp.position.place(w_disp, h_disp, w, h, stamp.margin);
        let name = doc.add_page_resource(idx, "XObject", xref)?;
        let gs_name = match gs_ref {
            Some(g) => Some(doc.add_page_resource(idx, "ExtGState", g)?),
            None => None,
        };
        let mut cb = ContentBuilder::new();
        cb.save();
        if let Some(g) = &gs_name {
            cb.ext_gstate(g);
        }
        cb.transform(&info.display_to_user())
            .image(&name, &Rect::from_xywh(x, y, w, h))
            .restore();
        let content = cb.finish();
        if stamp.under {
            doc.draw_under(idx, &content)?;
        } else {
            doc.draw(idx, &content)?;
        }
    }
    Ok(())
}

/// Page number settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PageNumbers {
    /// Template; `{page}` and `{pages}` are substituted.
    pub format: String,
    /// Anchor position.
    pub position: Position,
    /// Standard font name.
    pub font: String,
    /// Size in points.
    pub size: f64,
    /// Margin from the edge.
    pub margin: f64,
    /// Colour.
    pub color: [f64; 3],
    /// Number shown on the first stamped page.
    pub start_at: i64,
}

impl Default for PageNumbers {
    fn default() -> Self {
        Self {
            format: "{page} / {pages}".into(),
            position: Position::BottomCenter,
            font: "Helvetica".into(),
            size: 10.0,
            margin: 30.0,
            color: [0.0, 0.0, 0.0],
            start_at: 1,
        }
    }
}

/// Adds page numbers to the listed pages (all pages when `pages` is empty).
pub fn add_page_numbers(doc: &mut Document, pages: &[usize], settings: &PageNumbers) -> Result<()> {
    let all: Vec<usize> = if pages.is_empty() {
        (0..doc.page_count()).collect()
    } else {
        pages.to_vec()
    };
    let total = all.len();
    for (k, &idx) in all.iter().enumerate() {
        let n = settings.start_at + k as i64;
        let text = settings.format.replace("{page}", &n.to_string()).replace(
            "{pages}",
            &(settings.start_at + total as i64 - 1).to_string(),
        );
        let stamp = TextStamp {
            text,
            font: settings.font.clone(),
            size: settings.size,
            color: settings.color,
            opacity: 1.0,
            position: settings.position,
            rotation: 0.0,
            margin: settings.margin,
            under: false,
        };
        stamp_text(doc, &[idx], &stamp)?;
    }
    Ok(())
}

/// Rotates the listed pages by `degrees` (multiple of 90).
pub fn rotate_pages(doc: &mut Document, pages: &[usize], degrees: i64) -> Result<()> {
    if degrees % 90 != 0 {
        return Err(Error::Preset(format!(
            "rotation must be a multiple of 90, got {degrees}"
        )));
    }
    for &p in pages {
        doc.rotate_page(p, degrees)?;
    }
    Ok(())
}

/// Deletes the listed pages (indices may be in any order).
pub fn delete_pages(doc: &mut Document, pages: &[usize]) -> Result<()> {
    let count = doc.page_count();
    let remove: std::collections::HashSet<usize> = pages.iter().copied().collect();
    if let Some(&bad) = remove.iter().find(|&&p| p >= count) {
        return Err(Error::PageOutOfRange { index: bad, count });
    }
    let keep: Vec<usize> = (0..count).filter(|i| !remove.contains(i)).collect();
    doc.select_pages(&keep)
}

/// How page content is fitted when the page size changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FitMode {
    /// Scale uniformly so the whole page fits, centred, leaving margins.
    #[default]
    Fit,
    /// Scale uniformly so the page is covered, cropping the overflow.
    Fill,
    /// Stretch each axis independently to fill exactly.
    Stretch,
}

/// Resizes pages to `target`, scaling their content to match.
///
/// The target is interpreted in *display* orientation: a rotated page keeps
/// its rotation and is fitted so that what the reader sees matches the target.
/// Annotation rectangles are transformed with the content. Crop boxes are
/// removed, since the page box is being redefined.
pub fn resize_pages(
    doc: &mut Document,
    pages: &[usize],
    target: PageSize,
    mode: FitMode,
) -> Result<()> {
    if !(target.width.is_finite() && target.height.is_finite())
        || target.width <= 1.0
        || target.height <= 1.0
    {
        return Err(Error::Preset(
            "page size must be larger than 1 point".into(),
        ));
    }
    for &i in pages {
        let info = doc.page_info(i)?;
        // Work in unrotated user space: swap the target when the viewer rotates the page.
        let (tw, th) = if info.rotation.swaps_axes() {
            (target.height, target.width)
        } else {
            (target.width, target.height)
        };
        let src = info.media_box;
        let (sw, sh) = (src.width().max(1.0), src.height().max(1.0));
        let (sx, sy) = match mode {
            FitMode::Stretch => (tw / sw, th / sh),
            FitMode::Fit => {
                let s = (tw / sw).min(th / sh);
                (s, s)
            }
            FitMode::Fill => {
                let s = (tw / sw).max(th / sh);
                (s, s)
            }
        };
        let tx = (tw - sw * sx) / 2.0 - src.x0 * sx;
        let ty = (th - sh * sy) / 2.0 - src.y0 * sy;
        let m = Matrix::new(sx, 0.0, 0.0, sy, tx, ty);
        let mut prefix = Vec::new();
        prefix.extend_from_slice(b"q ");
        for v in [m.a, m.b, m.c, m.d, m.e, m.f] {
            crate::content::write_num(&mut prefix, v);
            prefix.push(b' ');
        }
        prefix.extend_from_slice(
            b"cm
",
        );
        doc.wrap_content(
            i, &prefix, b"
Q
",
        )?;
        transform_annotations(doc, info.obj, &m);
        let page = doc.page_ref(i)?;
        let d = doc
            .get_mut(page)
            .and_then(Object::as_dict_mut)
            .ok_or_else(|| Error::malformed("page is not a dictionary"))?;
        d.set("MediaBox", Rect::new(0.0, 0.0, tw, th).to_object());
        d.remove("CropBox");
        d.remove("BleedBox");
        d.remove("TrimBox");
        d.remove("ArtBox");
    }
    Ok(())
}

/// Crops pages to `area`, given in display space (points from the
/// bottom-left of the page as shown). Sets the crop box, which is what
/// viewers and printers show; the content outside it stays in the file
/// (use [`crate::redact`] to remove content).
pub fn crop_pages(doc: &mut Document, pages: &[usize], area: Rect) -> Result<()> {
    if !(area.width() > 1.0 && area.height() > 1.0) {
        return Err(Error::Preset(
            "crop area must be larger than 1 point".into(),
        ));
    }
    for &i in pages {
        let info = doc.page_info(i)?;
        let user = crate::annot::to_user_rect(&info, &area);
        let clipped = user
            .intersection(&info.media_box)
            .ok_or_else(|| Error::Preset(format!("crop area lies outside page {}", i + 1)))?;
        let page = doc.page_ref(i)?;
        let d = doc
            .get_mut(page)
            .and_then(Object::as_dict_mut)
            .ok_or_else(|| Error::malformed("page is not a dictionary"))?;
        d.set("CropBox", clipped.to_object());
        d.remove("TrimBox");
        d.remove("ArtBox");
        d.remove("BleedBox");
    }
    Ok(())
}

/// Removes any crop box so the whole media box shows again.
pub fn uncrop_pages(doc: &mut Document, pages: &[usize]) -> Result<()> {
    for &i in pages {
        let page = doc.page_ref(i)?;
        if let Some(d) = doc.get_mut(page).and_then(Object::as_dict_mut) {
            d.remove("CropBox");
        }
    }
    Ok(())
}

/// Scales pages by `factor` (1.0 leaves them unchanged), keeping the aspect
/// ratio. The page box grows or shrinks with the content.
pub fn scale_pages(doc: &mut Document, pages: &[usize], factor: f64) -> Result<()> {
    if !factor.is_finite() || factor <= 0.0 {
        return Err(Error::Preset("scale must be greater than zero".into()));
    }
    for &i in pages {
        let info = doc.page_info(i)?;
        let b = info.media_box;
        let (w, h) = (b.width() * factor, b.height() * factor);
        let target = if info.rotation.swaps_axes() {
            PageSize::new(h, w)
        } else {
            PageSize::new(w, h)
        };
        resize_pages(doc, &[i], target, FitMode::Stretch)?;
    }
    Ok(())
}

fn transform_annotations(doc: &mut Document, page: ObjRef, m: &Matrix) {
    let refs: Vec<ObjRef> = match doc
        .get(page)
        .as_dict()
        .and_then(|d| d.get("Annots"))
        .map(|o| doc.resolve(o).clone())
    {
        Some(Object::Array(a)) => a.iter().filter_map(Object::as_reference).collect(),
        _ => return,
    };
    for r in refs {
        let rect = doc
            .get(r)
            .as_dict()
            .and_then(|d| d.get("Rect"))
            .and_then(Rect::from_object);
        if let Some(rc) = rect {
            let p0 = m.apply(crate::geometry::Point::new(rc.x0, rc.y0));
            let p1 = m.apply(crate::geometry::Point::new(rc.x1, rc.y1));
            if let Some(d) = doc.get_mut(r).and_then(Object::as_dict_mut) {
                d.set("Rect", Rect::new(p0.x, p0.y, p1.x, p1.y).to_object());
            }
        }
    }
}

/// Inserts `count` blank pages of `size` at `at` (0 = before the first page).
pub fn insert_blank_pages(
    doc: &mut Document,
    at: usize,
    count: usize,
    size: PageSize,
) -> Result<Vec<ObjRef>> {
    let mut out = Vec::with_capacity(count);
    for k in 0..count {
        let page = crate::object::Dict::new().with("MediaBox", size.rect().to_object());
        out.push(doc.insert_page(at + k, page)?);
    }
    Ok(out)
}

/// Reverses the order of all pages.
pub fn reverse_pages(doc: &mut Document) -> Result<()> {
    let order: Vec<usize> = (0..doc.page_count()).rev().collect();
    doc.select_pages(&order)
}

/// How images are placed on pages by [`images_to_pdf`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImagePageOptions {
    /// Named page size (`a4`, `letter`, ...). `None` sizes each page to its
    /// image at `dpi`.
    pub size: Option<String>,
    /// Turn a fixed-size page to match the image's orientation. Default true.
    pub auto_orient: bool,
    /// Resolution used when the page is sized to the image. Default 150.
    pub dpi: f64,
    /// Margin in points on fixed-size pages. Default 0.
    pub margin: f64,
}

impl Default for ImagePageOptions {
    fn default() -> Self {
        Self {
            size: None,
            auto_orient: true,
            dpi: 150.0,
            margin: 0.0,
        }
    }
}

/// Appends one page showing `image` (JPEG or PNG bytes).
pub fn add_image_page(doc: &mut Document, image: &[u8], opts: &ImagePageOptions) -> Result<usize> {
    let img = Image::load(image)?;
    let (iw, ih) = (img.width.max(1) as f64, img.height.max(1) as f64);
    let dpi = if opts.dpi.is_finite() && opts.dpi > 0.0 {
        opts.dpi
    } else {
        150.0
    };
    let (page, rect) = match &opts.size {
        None => {
            // Keep the page a sensible size even for tiny images.
            let (w, h) = ((iw / dpi * 72.0).max(3.0), (ih / dpi * 72.0).max(3.0));
            (PageSize::new(w, h), Rect::new(0.0, 0.0, w, h))
        }
        Some(name) => {
            let mut size = PageSize::by_name(name)
                .ok_or_else(|| Error::Preset(format!("unknown page size '{name}'")))?;
            if opts.auto_orient && (iw > ih) != (size.width > size.height) {
                size = PageSize::new(size.height, size.width);
            }
            let m = opts.margin.max(0.0);
            let (aw, ah) = (
                (size.width - 2.0 * m).max(1.0),
                (size.height - 2.0 * m).max(1.0),
            );
            let s = (aw / iw).min(ah / ih);
            let (w, h) = (iw * s, ih * s);
            (
                size,
                Rect::from_xywh(m + (aw - w) / 2.0, m + (ah - h) / 2.0, w, h),
            )
        }
    };
    let index = doc.page_count();
    doc.add_page(page);
    let img_ref = doc.add_image(&img, 6);
    let name = doc.add_page_resource(index, "XObject", img_ref)?;
    let mut cb = ContentBuilder::new();
    cb.image(&name, &rect);
    doc.draw(index, &cb.finish())?;
    Ok(index)
}

/// Builds a document with one page per image (JPEG or PNG bytes).
pub fn images_to_pdf(images: &[&[u8]], opts: &ImagePageOptions) -> Result<Document> {
    let mut doc = Document::new();
    for im in images {
        add_image_page(&mut doc, im, opts)?;
    }
    Ok(doc)
}

/// Applies metadata.
pub fn set_metadata(doc: &mut Document, m: &Metadata) {
    doc.set_metadata(m);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges() {
        assert_eq!(parse_page_ranges("1-3,7", 10).unwrap(), vec![0, 1, 2, 6]);
        assert_eq!(parse_page_ranges("8-", 10).unwrap(), vec![7, 8, 9]);
        assert_eq!(parse_page_ranges("-2", 10).unwrap(), vec![0, 1]);
        assert_eq!(parse_page_ranges("even", 5).unwrap(), vec![1, 3]);
        assert_eq!(parse_page_ranges("odd", 5).unwrap(), vec![0, 2, 4]);
        assert_eq!(
            parse_page_ranges("last, first, r2", 5).unwrap(),
            vec![4, 0, 3]
        );
        assert_eq!(parse_page_ranges("3-1", 5).unwrap(), vec![2, 1, 0]);
        assert!(parse_page_ranges("0", 5).is_err());
        assert!(parse_page_ranges("6", 5).is_err());
        assert!(parse_page_ranges("x", 5).is_err());
        assert_eq!(chunk_pages(5, 2), vec![vec![0, 1], vec![2, 3], vec![4]]);
    }

    #[test]
    fn fit_modes() {
        use crate::Document;
        let mut doc = Document::new();
        doc.add_page(PageSize::new(200.0, 100.0));
        resize_pages(&mut doc, &[0], PageSize::new(400.0, 400.0), FitMode::Fit).unwrap();
        let info = doc.page_info(0).unwrap();
        assert_eq!(
            (info.media_box.width(), info.media_box.height()),
            (400.0, 400.0)
        );
        let content = String::from_utf8(doc.page_content(0).unwrap()).unwrap();
        // Fit: scale 2, centred vertically (400 - 200) / 2 = 100.
        assert!(content.starts_with("q 2 0 0 2 0 100 cm"), "{content}");
        assert!(content.trim_end().ends_with("Q"), "{content}");

        let mut doc = Document::new();
        doc.add_page(PageSize::new(200.0, 100.0));
        resize_pages(
            &mut doc,
            &[0],
            PageSize::new(400.0, 400.0),
            FitMode::Stretch,
        )
        .unwrap();
        assert!(String::from_utf8(doc.page_content(0).unwrap())
            .unwrap()
            .starts_with("q 2 0 0 4 0 0 cm"));

        let mut doc = Document::new();
        doc.add_page(PageSize::new(200.0, 100.0));
        scale_pages(&mut doc, &[0], 0.5).unwrap();
        let info = doc.page_info(0).unwrap();
        assert_eq!(
            (info.media_box.width(), info.media_box.height()),
            (100.0, 50.0)
        );
    }

    #[test]
    fn resize_respects_rotation() {
        use crate::Document;
        let mut doc = Document::new();
        doc.add_page(PageSize::new(200.0, 100.0));
        doc.rotate_page(0, 90).unwrap();
        resize_pages(&mut doc, &[0], PageSize::LETTER, FitMode::Fit).unwrap();
        let info = doc.page_info(0).unwrap();
        // The media box is swapped so the displayed page is Letter-shaped.
        assert_eq!(
            (info.media_box.width(), info.media_box.height()),
            (792.0, 612.0)
        );
        assert_eq!(info.display_width(), 612.0);
    }

    #[test]
    fn blank_pages_and_reverse() {
        use crate::Document;
        let mut doc = Document::new();
        doc.add_page(PageSize::LETTER);
        doc.add_page(PageSize::LETTER);
        insert_blank_pages(&mut doc, 1, 2, PageSize::A4).unwrap();
        assert_eq!(doc.page_count(), 4);
        assert!((doc.page_info(1).unwrap().media_box.width() - 595.28).abs() < 0.01);
        reverse_pages(&mut doc).unwrap();
        assert!((doc.page_info(2).unwrap().media_box.width() - 595.28).abs() < 0.01);
    }

    #[test]
    fn crop() {
        use crate::Document;
        let mut doc = Document::new();
        doc.add_page(PageSize::LETTER);
        crop_pages(&mut doc, &[0], Rect::new(50.0, 100.0, 300.0, 400.0)).unwrap();
        let info = doc.page_info(0).unwrap();
        assert_eq!(info.crop_box, Some(Rect::new(50.0, 100.0, 300.0, 400.0)));
        assert_eq!(info.display_width(), 250.0);
        // Rotated page: display coordinates map through the rotation.
        doc.rotate_page(0, 90).unwrap();
        crop_pages(&mut doc, &[0], Rect::new(0.0, 0.0, 100.0, 50.0)).unwrap();
        let info = doc.page_info(0).unwrap();
        assert!(
            (info.display_width() - 100.0).abs() < 1e-9
                && (info.display_height() - 50.0).abs() < 1e-9
        );
        uncrop_pages(&mut doc, &[0]).unwrap();
        assert_eq!(doc.page_info(0).unwrap().crop_box, None);
        assert!(crop_pages(&mut doc, &[0], Rect::new(0.0, 0.0, 0.5, 0.5)).is_err());
    }

    #[test]
    fn image_pages() {
        let png = crate::image::tests::tiny_png();
        let doc = images_to_pdf(
            &[&png, &png],
            &ImagePageOptions {
                dpi: 1.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(doc.page_count(), 2);
        let info = doc.page_info(0).unwrap();
        assert!(
            (info.media_box.width() - 144.0).abs() < 1e-9,
            "2 px at 1 dpi = 144 pt, got {}",
            info.media_box.width()
        );
        let mut doc = Document::new();
        add_image_page(
            &mut doc,
            &png,
            &ImagePageOptions {
                size: Some("a4".into()),
                margin: 36.0,
                ..Default::default()
            },
        )
        .unwrap();
        let info = doc.page_info(0).unwrap();
        assert!((info.media_box.width() - 595.28).abs() < 0.01);
        let content = String::from_utf8(doc.page_content(0).unwrap()).unwrap();
        assert!(content.contains("Do"), "{content}");
        assert!(add_image_page(
            &mut doc,
            &png,
            &ImagePageOptions {
                size: Some("nope".into()),
                ..Default::default()
            }
        )
        .is_err());
    }

    #[test]
    fn placement() {
        assert_eq!(
            Position::BottomLeft.place(100.0, 50.0, 10.0, 5.0, 2.0),
            (2.0, 2.0)
        );
        assert_eq!(
            Position::TopRight.place(100.0, 50.0, 10.0, 5.0, 2.0),
            (88.0, 43.0)
        );
        assert_eq!(
            Position::Center.place(100.0, 50.0, 10.0, 6.0, 2.0),
            (45.0, 22.0)
        );
    }
}
