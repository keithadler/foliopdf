//! True redaction: removes text, vector graphics and image pixels under a
//! set of rectangles, then paints the rectangles over. Unlike drawing a box
//! on top, the removed content is gone from the file.
//!
//! What happens under a redaction area:
//!
//! * **Text** – every glyph whose box overlaps the area is cut out of its
//!   text-showing operator (the remaining glyphs keep their positions).
//!   Invisible text (OCR layers) is treated the same.
//! * **Paths** – filled or stroked paths lying entirely inside the area are
//!   removed; paths that merely cross it are kept.
//! * **Images** – images lying entirely inside the area are removed. Images
//!   that partly overlap have the overlapping pixels blanked when they can
//!   be decoded (raw, Flate and JPEG data); otherwise the whole image is
//!   removed and a warning is reported. Inline images that overlap are
//!   removed.
//! * **Form XObjects** – handled recursively; the form is copied first so
//!   other pages using it are unaffected.
//! * **Annotations** – any annotation overlapping the area is removed.
//!
//! Areas are given in display space (see [`crate::annot`]).

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::annot;
use crate::content::ContentBuilder;
use crate::cstream::{self, Op};
use crate::document::Document;
use crate::error::{Error, Result};
use crate::filters;
use crate::geometry::{Matrix, Point, Rect};
use crate::imgcodec::{decode_pixels, encode_jpeg, set_sample};
use crate::object::{Dict, ObjRef, Object, PdfString, Stream};
use crate::text::{self, GlyphLoc, SearchOptions, StreamId};

/// Redaction settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RedactOptions {
    /// Colour of the box painted over each area; `None` paints nothing.
    pub fill: Option<[f64; 3]>,
    /// Remove annotations that overlap an area. Default true.
    pub remove_annotations: bool,
    /// Grow each area by this many points before matching. Default 0.5.
    pub margin: f64,
}

impl Default for RedactOptions {
    fn default() -> Self {
        Self {
            fill: Some([0.0, 0.0, 0.0]),
            remove_annotations: true,
            margin: 0.5,
        }
    }
}

/// What a redaction did.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactReport {
    /// Glyphs cut from text operators.
    pub glyphs_removed: usize,
    /// Images deleted outright.
    pub images_removed: usize,
    /// Images whose pixels were blanked.
    pub images_edited: usize,
    /// Vector paths deleted.
    pub paths_removed: usize,
    /// Annotations deleted.
    pub annotations_removed: usize,
    /// Form XObjects rewritten.
    pub forms_edited: usize,
    /// Things worth telling the user (e.g. an image that had to be removed whole).
    pub warnings: Vec<String>,
}

impl RedactReport {
    /// Adds another report's counts and warnings to this one.
    pub fn merge(&mut self, o: RedactReport) {
        self.glyphs_removed += o.glyphs_removed;
        self.images_removed += o.images_removed;
        self.images_edited += o.images_edited;
        self.paths_removed += o.paths_removed;
        self.annotations_removed += o.annotations_removed;
        self.forms_edited += o.forms_edited;
        self.warnings.extend(o.warnings);
    }
}

fn hit_glyph(r: &Rect, areas: &[Rect]) -> bool {
    let c = r.center();
    let area = (r.width() * r.height()).max(1e-9);
    areas.iter().any(|a| {
        a.contains_point(c)
            || a.intersection(r)
                .map(|i| i.width() * i.height() >= 0.25 * area)
                .unwrap_or(false)
    })
}

/// Redacts `areas` (display space) on one page.
pub fn redact(
    doc: &mut Document,
    page: usize,
    areas: &[Rect],
    opts: &RedactOptions,
) -> Result<RedactReport> {
    let info = doc.page_info(page)?;
    let areas_user: Vec<Rect> = areas
        .iter()
        .map(|a| annot::to_user_rect(&info, a).expand(opts.margin))
        .collect();
    let mut report = RedactReport::default();
    if areas_user.is_empty() {
        return Ok(report);
    }
    let content = text::page_content(doc, page)?;

    // ---- decide what goes ----------------------------------------------------
    // Glyphs to remove, grouped by stream and operator.
    let mut glyph_edits: HashMap<StreamId, BTreeMap<usize, Vec<GlyphLoc>>> = HashMap::new();
    for g in &content.glyphs {
        if hit_glyph(&g.rect, &areas_user) {
            glyph_edits
                .entry(g.loc.stream)
                .or_default()
                .entry(g.loc.op)
                .or_default()
                .push(g.loc);
            report.glyphs_removed += 1;
        }
    }
    // Paths fully inside an area.
    let mut path_edits: HashMap<StreamId, Vec<(usize, usize)>> = HashMap::new();
    for p in &content.paths {
        if p.rect.width() >= 0.0 && areas_user.iter().any(|a| a.contains(&p.rect)) {
            path_edits
                .entry(p.stream)
                .or_default()
                .push((p.first_op, p.paint_op));
            report.paths_removed += 1;
        }
    }
    // Images.
    enum ImgEdit {
        Remove,
        Replace(ObjRef),
    }
    let mut image_edits: HashMap<StreamId, BTreeMap<usize, ImgEdit>> = HashMap::new();
    for im in &content.images {
        let hit = areas_user.iter().any(|a| a.intersects(&im.rect));
        if !hit {
            continue;
        }
        let full = areas_user.iter().any(|a| a.contains(&im.rect));
        let edit = match (full, im.xobject) {
            (true, _) | (false, None) => {
                if !full {
                    report
                        .warnings
                        .push("an inline image overlapping the area was removed whole".into());
                }
                report.images_removed += 1;
                ImgEdit::Remove
            }
            (false, Some(xref)) => match mask_image(doc, xref, &im.ctm, &areas_user, opts.fill) {
                Ok(Some(new_ref)) => {
                    report.images_edited += 1;
                    ImgEdit::Replace(new_ref)
                }
                Ok(None) => {
                    report.warnings.push(format!("image {} could not be decoded (JPEG 2000, fax or JBIG2 data) and was removed whole", im.name.clone().unwrap_or_default()));
                    report.images_removed += 1;
                    ImgEdit::Remove
                }
                Err(e) => {
                    report.warnings.push(format!(
                        "image {}: {e}; removed whole",
                        im.name.clone().unwrap_or_default()
                    ));
                    report.images_removed += 1;
                    ImgEdit::Remove
                }
            },
        };
        image_edits
            .entry(im.stream)
            .or_default()
            .insert(im.op, edit);
    }

    // ---- which streams need rewriting ------------------------------------------
    let mut dirty: HashSet<StreamId> = HashSet::new();
    dirty.extend(glyph_edits.keys().copied());
    dirty.extend(path_edits.keys().copied());
    dirty.extend(image_edits.keys().copied());
    // Ancestors of dirty forms are dirty too (they must point at the copies).
    let parent_of: HashMap<ObjRef, StreamId> = content
        .forms
        .iter()
        .map(|f| (f.xobject, f.stream))
        .collect();
    let mut queue: Vec<StreamId> = dirty.iter().copied().collect();
    while let Some(s) = queue.pop() {
        if let StreamId::Form(r) = s {
            if let Some(p) = parent_of.get(&r) {
                if dirty.insert(*p) {
                    queue.push(*p);
                }
            }
        }
    }
    // Copy every dirty form so other pages are untouched.
    let mut form_copy: HashMap<ObjRef, ObjRef> = HashMap::new();
    for s in &dirty {
        if let StreamId::Form(r) = s {
            let obj = doc.get(*r).clone();
            let n = doc.add(obj);
            form_copy.insert(*r, n);
            report.forms_edited += 1;
        }
    }

    // ---- rewrite each dirty stream ---------------------------------------------
    for s in dirty.iter().copied().collect::<Vec<_>>() {
        let (data, mut resources): (Vec<u8>, Dict) = match s {
            StreamId::Page => (
                doc.page_content(page)?,
                doc.page_attr(info.obj, "Resources")
                    .map(|o| doc.resolve(o).clone())
                    .and_then(Object::into_dict)
                    .unwrap_or_default(),
            ),
            StreamId::Form(r) => {
                let st = doc
                    .get(r)
                    .as_stream()
                    .ok_or_else(|| Error::malformed("form XObject is not a stream"))?;
                (
                    doc.stream_data(st)?,
                    doc.dict_get(&st.dict, "Resources")
                        .and_then(Object::as_dict)
                        .cloned()
                        .unwrap_or_default(),
                )
            }
        };
        let ops = cstream::parse(&data);
        let mut xobjects: Dict = resources
            .get("XObject")
            .map(|o| doc.resolve(o).clone())
            .and_then(Object::into_dict)
            .unwrap_or_default();
        // Child forms that were copied: repoint by name.
        for f in content.forms.iter().filter(|f| f.stream == s) {
            if let Some(copy) = form_copy.get(&f.xobject) {
                xobjects.set(&f.name, *copy);
            }
        }
        let glyph_ops = glyph_edits.get(&s);
        let empty_paths = Vec::new();
        let paths: &Vec<(usize, usize)> = path_edits.get(&s).unwrap_or(&empty_paths);
        let images = image_edits.get(&s);
        let mut out: Vec<Op> = Vec::with_capacity(ops.len());
        let mut name_counter = 0usize;
        for (i, op) in ops.iter().enumerate() {
            if paths.iter().any(|(a, b)| i >= *a && i <= *b) {
                continue;
            }
            if let Some(imgs) = images {
                if let Some(edit) = imgs.get(&i) {
                    match edit {
                        ImgEdit::Remove => continue,
                        ImgEdit::Replace(new_ref) => {
                            name_counter += 1;
                            let mut name = format!("FolioR{name_counter}");
                            while xobjects.contains(&name) {
                                name_counter += 1;
                                name = format!("FolioR{name_counter}");
                            }
                            xobjects.set(&name, *new_ref);
                            out.push(Op::new("Do", vec![Object::name(&name)]));
                            continue;
                        }
                    }
                }
            }
            if let Some(locs) = glyph_ops.and_then(|m| m.get(&i)) {
                rewrite_text_op(op, locs, &mut out);
                continue;
            }
            out.push(op.clone());
        }
        let new_data = cstream::write(&out);
        if !xobjects.is_empty() {
            resources.set("XObject", xobjects);
        }
        match s {
            StreamId::Page => {
                doc.set_page_content(page, &new_data)?;
                let pr = doc.page_ref(page)?;
                if let Some(d) = doc.get_mut(pr).and_then(Object::as_dict_mut) {
                    d.set("Resources", resources);
                }
            }
            StreamId::Form(r) => {
                let copy = form_copy[&r];
                if let Some(st) = doc.get_mut(copy).and_then(Object::as_stream_mut) {
                    st.dict.remove("DecodeParms");
                    st.dict.remove("DP");
                    st.dict.set("Filter", "FlateDecode");
                    st.dict.set("Resources", resources);
                    st.data = filters::flate_encode(&new_data, 6);
                }
            }
        }
    }

    // ---- annotations ------------------------------------------------------------
    if opts.remove_annotations {
        let display_areas: Vec<Rect> = areas.iter().map(|a| a.expand(opts.margin)).collect();
        let list = annot::list_annotations(doc, page)?;
        let gone: Vec<u32> = list
            .iter()
            .filter(|a| display_areas.iter().any(|d| d.intersects(&a.rect)))
            .map(|a| a.object)
            .collect();
        if !gone.is_empty() {
            report.annotations_removed += annot::remove_annotations(
                doc,
                &[page],
                &annot::FlattenOptions {
                    objects: Some(gone),
                    ..Default::default()
                },
            )?;
        }
    }

    // ---- paint the boxes --------------------------------------------------------
    if let Some(c) = opts.fill {
        let mut cb = ContentBuilder::new();
        cb.save()
            .transform(&info.display_to_user())
            .fill_rgb(c[0], c[1], c[2]);
        for a in areas {
            cb.rect(a).fill();
        }
        cb.restore();
        doc.draw(page, &cb.finish())?;
    }
    Ok(report)
}

/// Finds `needle` on `pages` and redacts every match. Returns the report
/// and the number of matches.
pub fn redact_text(
    doc: &mut Document,
    pages: &[usize],
    needle: &str,
    search: &SearchOptions,
    opts: &RedactOptions,
) -> Result<(RedactReport, usize)> {
    let mut report = RedactReport::default();
    let mut count = 0;
    for &p in pages {
        let info = doc.page_info(p)?;
        let matches = text::search(doc, p, needle, search)?;
        if matches.is_empty() {
            continue;
        }
        count += matches.len();
        let areas: Vec<Rect> = matches
            .iter()
            .flat_map(|m| m.rects.iter().map(|r| annot::to_display_rect(&info, r)))
            .collect();
        report.merge(redact(doc, p, &areas, opts)?);
    }
    Ok((report, count))
}

/// Rewrites one text-showing operator with the listed glyphs cut out.
fn rewrite_text_op(op: &Op, locs: &[GlyphLoc], out: &mut Vec<Op>) {
    let by_elem: HashMap<usize, Vec<&GlyphLoc>> = locs.iter().fold(HashMap::new(), |mut m, l| {
        m.entry(l.elem).or_default().push(l);
        m
    });
    let rewrite_string = |s: &PdfString, elem: usize, arr: &mut Vec<Object>| {
        let bytes = s.as_bytes();
        let mut cuts: Vec<&GlyphLoc> = by_elem.get(&elem).cloned().unwrap_or_default();
        cuts.sort_by_key(|l| l.start);
        let mut pos = 0usize;
        let mut pending_adjust = 0.0;
        let mut seg: Vec<u8> = Vec::new();
        for l in cuts {
            if l.start < pos || l.end > bytes.len() {
                continue;
            }
            seg.extend_from_slice(&bytes[pos..l.start]);
            if !seg.is_empty() {
                if pending_adjust != 0.0 {
                    arr.push(Object::Real(pending_adjust));
                    pending_adjust = 0.0;
                }
                arr.push(Object::String(PdfString {
                    bytes: std::mem::take(&mut seg),
                    hex: s.hex,
                }));
            }
            pending_adjust += l.adjust;
            pos = l.end;
        }
        seg.extend_from_slice(&bytes[pos..]);
        if pending_adjust != 0.0 {
            arr.push(Object::Real(pending_adjust));
        }
        if !seg.is_empty() {
            arr.push(Object::String(PdfString {
                bytes: seg,
                hex: s.hex,
            }));
        }
    };
    match op.name.as_str() {
        "Tj" | "'" | "\"" => {
            if op.name == "\"" {
                if let (Some(aw), Some(ac)) = (op.operands.first(), op.operands.get(1)) {
                    out.push(Op::new("Tw", vec![aw.clone()]));
                    out.push(Op::new("Tc", vec![ac.clone()]));
                }
            }
            if op.name != "Tj" {
                out.push(Op::new("T*", Vec::new()));
            }
            let mut arr = Vec::new();
            if let Some(Object::String(s)) = op.operands.last() {
                rewrite_string(s, 0, &mut arr);
            }
            out.push(Op::new("TJ", vec![Object::Array(arr)]));
        }
        "TJ" => {
            let mut arr = Vec::new();
            if let Some(Object::Array(items)) = op.operands.first() {
                for (k, item) in items.iter().enumerate() {
                    match item {
                        Object::String(s) => rewrite_string(s, k, &mut arr),
                        other => arr.push(other.clone()),
                    }
                }
            }
            out.push(Op::new("TJ", vec![Object::Array(arr)]));
        }
        _ => out.push(op.clone()),
    }
}

impl Document {
    /// Replaces a page's content streams with a single new stream.
    pub fn set_page_content(&mut self, index: usize, content: &[u8]) -> Result<()> {
        let page = self.page_ref(index)?;
        let s = Stream::new(
            Dict::new().with("Filter", "FlateDecode"),
            filters::flate_encode(content, 6),
        );
        let r = self.add(s.into());
        let d = self
            .get_mut(page)
            .and_then(Object::as_dict_mut)
            .ok_or_else(|| Error::malformed("page is not a dictionary"))?;
        d.set("Contents", r);
        Ok(())
    }
}

/// Blanks the pixels of image `xref` that fall inside `areas` when placed
/// with `ctm`. Returns a new image object, or `None` if it cannot be decoded.
fn mask_image(
    doc: &mut Document,
    xref: ObjRef,
    ctm: &Matrix,
    areas: &[Rect],
    fill: Option<[f64; 3]>,
) -> Result<Option<ObjRef>> {
    let s = match doc.get(xref).as_stream() {
        Some(s) => s.clone(),
        None => return Ok(None),
    };
    let mut px = match decode_pixels(doc, &s)? {
        Some(p) => p,
        None => return Ok(None),
    };
    let inv = match ctm.invert() {
        Some(m) => m,
        None => return Ok(None),
    };
    // Pixel bounds worth testing: the areas mapped into image space.
    let (w, h) = (px.width as f64, px.height as f64);
    let mut x0 = usize::MAX;
    let mut y0 = usize::MAX;
    let mut x1 = 0usize;
    let mut y1 = 0usize;
    for a in areas {
        let b = a.transform(&inv); // unit square coords
        let bx0 = ((b.x0 * w).floor().max(0.0)) as usize;
        let bx1 = ((b.x1 * w).ceil().min(w)) as usize;
        let by0 = (((1.0 - b.y1) * h).floor().max(0.0)) as usize;
        let by1 = (((1.0 - b.y0) * h).ceil().min(h)) as usize;
        if bx1 > bx0 && by1 > by0 {
            x0 = x0.min(bx0);
            y0 = y0.min(by0);
            x1 = x1.max(bx1);
            y1 = y1.max(by1);
        }
    }
    if x0 >= x1 || y0 >= y1 {
        return Ok(Some(xref)); // nothing to blank after all
    }
    let fill_val = fill.unwrap_or([0.0, 0.0, 0.0]);
    let gray = 0.299 * fill_val[0] + 0.587 * fill_val[1] + 0.114 * fill_val[2];
    let max = ((1u32 << px.bpc) - 1) as f64;
    let sample_values: Vec<u32> = match px.ncomp {
        3 => fill_val.iter().map(|v| (v * max).round() as u32).collect(),
        4 => vec![0, 0, 0, ((1.0 - gray) * max).round() as u32],
        1 => vec![(gray * max).round() as u32],
        n => vec![0; n],
    };
    let row_bytes = (px.width * px.ncomp * px.bpc).div_ceil(8);
    let mut changed = 0usize;
    for py in y0..y1 {
        for pxx in x0..x1 {
            let u = (pxx as f64 + 0.5) / w;
            let v = 1.0 - (py as f64 + 0.5) / h;
            let p = ctm.apply(Point::new(u, v));
            if !areas.iter().any(|a| a.contains_point(p)) {
                continue;
            }
            changed += 1;
            for c in 0..px.ncomp {
                let val = sample_values.get(c).copied().unwrap_or(0);
                set_sample(&mut px.data, row_bytes, py, pxx * px.ncomp + c, px.bpc, val);
            }
        }
    }
    if changed == 0 {
        return Ok(Some(xref));
    }
    // Build the replacement object.
    let mut dict = s.dict.clone();
    dict.remove("DecodeParms");
    dict.remove("DP");
    dict.remove("Length");
    let data = if px.jpeg {
        match encode_jpeg(&px, 88) {
            Some(j) => {
                dict.set("Filter", "DCTDecode");
                j
            }
            None => {
                dict.set("Filter", "FlateDecode");
                dict.set("BitsPerComponent", 8);
                filters::flate_encode(&px.data, 6)
            }
        }
    } else {
        dict.set("Filter", "FlateDecode");
        filters::flate_encode(&px.data, 6)
    };
    // Blank the soft mask too, so nothing of the shape survives in the alpha.
    if let Some(sm) = dict.get("SMask").and_then(Object::as_reference) {
        if let Ok(Some(new_sm)) = mask_image(doc, sm, ctm, areas, Some([1.0, 1.0, 1.0])) {
            dict.set("SMask", new_sm);
        }
    }
    Ok(Some(doc.add(Stream::new(dict, data).into())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::StandardFont;
    use crate::page::PageSize;

    fn doc_with_text(lines: &[(&str, f64, f64)]) -> Document {
        let mut d = Document::new();
        d.add_page(PageSize::LETTER);
        let f = d.add_standard_font(StandardFont::Helvetica);
        let name = d.add_page_resource(0, "Font", f).unwrap();
        let mut cb = ContentBuilder::new();
        for (t, x, y) in lines {
            let enc = d.font_mut(f).unwrap().encode(t);
            cb.begin_text()
                .font(&name, 12.0)
                .text_matrix(&Matrix::translate(*x, *y))
                .show_literal(&enc)
                .end_text();
        }
        d.draw(0, &cb.finish()).unwrap();
        Document::load(&d.save(&Default::default()).unwrap()).unwrap()
    }

    #[test]
    fn redacts_a_word_and_keeps_layout() {
        let mut d = doc_with_text(&[("Account 12345 closed", 72.0, 700.0)]);
        let before = text::page_content(&d, 0).unwrap();
        let closed_x = before
            .glyphs
            .iter()
            .find(|g| g.text == "c" && g.origin.x > 100.0)
            .unwrap()
            .origin
            .x;
        let m = text::search(&d, 0, "12345", &Default::default()).unwrap();
        assert_eq!(m.len(), 1);
        let info = d.page_info(0).unwrap();
        let area = annot::to_display_rect(&info, &m[0].rects[0]);
        let report = redact(&mut d, 0, &[area], &Default::default()).unwrap();
        assert_eq!(report.glyphs_removed, 5);
        let d = Document::load(&d.save(&Default::default()).unwrap()).unwrap();
        let after = text::page_content(&d, 0).unwrap();
        let txt = text::text_from_lines(&text::lines(&after.glyphs));
        assert_eq!(txt, "Account closed");
        assert!(text::search(&d, 0, "12345", &Default::default())
            .unwrap()
            .is_empty());
        // "closed" did not move.
        let closed_after = after
            .glyphs
            .iter()
            .find(|g| g.text == "c" && g.origin.x > 100.0)
            .unwrap()
            .origin
            .x;
        assert!(
            (closed_after - closed_x).abs() < 0.01,
            "{closed_after} vs {closed_x}"
        );
        // A black box was painted and the raw bytes are gone.
        let content = String::from_utf8_lossy(&d.page_content(0).unwrap()).into_owned();
        assert!(!content.contains("12345"), "{content}");
        assert!(content.contains(" re\nf"), "{content}");
    }

    #[test]
    fn redact_text_helper_and_no_fill() {
        let mut d = doc_with_text(&[
            ("secret one", 72.0, 700.0),
            ("nothing here", 72.0, 680.0),
            ("SECRET two", 72.0, 660.0),
        ]);
        let (rep, n) = redact_text(
            &mut d,
            &[0],
            "secret",
            &SearchOptions {
                case_insensitive: true,
                ..Default::default()
            },
            &RedactOptions {
                fill: None,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(n, 2);
        assert_eq!(rep.glyphs_removed, 12);
        let txt = text::page_text(&d, 0).unwrap();
        assert_eq!(txt, "one\nnothing here\ntwo");
        assert!(!String::from_utf8_lossy(&d.page_content(0).unwrap()).contains(" re\nf"));
    }

    #[test]
    fn tj_rewrite_keeps_adjustments() {
        let op = Op::new(
            "TJ",
            vec![Object::Array(vec![
                Object::String(PdfString::new(b"ABCD".to_vec())),
                Object::Integer(-250),
                Object::String(PdfString::new(b"EF".to_vec())),
            ])],
        );
        let locs = vec![
            GlyphLoc {
                stream: StreamId::Page,
                op: 0,
                elem: 0,
                start: 1,
                end: 2,
                adjust: -600.0,
            },
            GlyphLoc {
                stream: StreamId::Page,
                op: 0,
                elem: 0,
                start: 2,
                end: 3,
                adjust: -700.0,
            },
            GlyphLoc {
                stream: StreamId::Page,
                op: 0,
                elem: 2,
                start: 0,
                end: 1,
                adjust: -500.0,
            },
        ];
        let mut out = Vec::new();
        rewrite_text_op(&op, &locs, &mut out);
        let arr = out[0].operands[0].as_array().unwrap();
        let shown: Vec<String> = arr
            .iter()
            .map(|o| match o {
                Object::String(s) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
                o => format!("{}", o.as_f64().unwrap()),
            })
            .collect();
        assert_eq!(shown, ["A", "-1300", "D", "-250", "-500", "F"]);
    }

    #[test]
    fn paths_and_images() {
        let mut d = Document::new();
        d.add_page(PageSize::LETTER);
        let png = crate::image::tests::tiny_png();
        let img = d.add_image(&crate::image::Image::load(&png).unwrap(), 6);
        let iname = d.add_page_resource(0, "XObject", img).unwrap();
        // A square inside the area, a line crossing it, an image inside, an image half in.
        let content = format!("q 1 0 0 rg 100 100 20 20 re f Q q 0 0 1 RG 50 110 m 400 110 l S Q q 30 0 0 30 100 100 cm /{iname} Do Q q 100 0 0 100 150 100 cm /{iname} Do Q");
        d.draw(0, content.as_bytes()).unwrap();
        let mut d = Document::load(&d.save(&Default::default()).unwrap()).unwrap();
        let info = d.page_info(0).unwrap();
        let area = annot::to_display_rect(&info, &Rect::new(90.0, 90.0, 260.0, 140.0));
        let rep = redact(&mut d, 0, &[area], &Default::default()).unwrap();
        assert_eq!(rep.paths_removed, 1, "{rep:?}");
        assert_eq!(rep.images_removed, 1);
        assert_eq!(rep.images_edited, 1);
        let d = Document::load(&d.save(&Default::default()).unwrap()).unwrap();
        let c = text::page_content(&d, 0).unwrap();
        assert_eq!(c.images.len(), 1);
        assert_eq!(
            c.paths.len(),
            2,
            "the crossing line stays; the box is the redaction fill"
        );
        // The edited 2x2 image: its bottom row (y 100..150) is inside the area, the top row is not.
        let s = d
            .get(c.images[0].xobject.unwrap())
            .as_stream()
            .unwrap()
            .clone();
        let data = d.stream_data(&s).unwrap();
        assert_eq!(
            &data[6..12],
            &[0, 0, 0, 0, 0, 0],
            "bottom row blanked: {data:?}"
        );
        assert_eq!(
            &data[..6],
            &[255, 0, 0, 0, 255, 0],
            "top row kept: {data:?}"
        );
    }
}
