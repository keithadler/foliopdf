//! Imposition: several pages per sheet (N-up) and booklet order for
//! printing and folding.

use serde::{Deserialize, Serialize};

use crate::content::ContentBuilder;
use crate::document::Document;
use crate::error::{Error, Result};
use crate::geometry::{Matrix, Rect};
use crate::object::{Dict, ObjRef, Stream};
use crate::page::PageSize;

/// Settings for [`nup`] and [`booklet`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImposeOptions {
    /// Sheet size name (`a4`, `letter`, ...). Default `letter`.
    pub sheet: String,
    /// Turn the sheet to landscape (the usual choice for 2-up). Default true.
    pub landscape: bool,
    /// Margin around each placed page in points. Default 18.
    pub margin: f64,
    /// Draw a thin frame around each page. Default false.
    pub frames: bool,
}

impl Default for ImposeOptions {
    fn default() -> Self {
        Self {
            sheet: "letter".into(),
            landscape: true,
            margin: 18.0,
            frames: false,
        }
    }
}

/// Wraps page `index` in a form XObject that draws it at its own size.
fn page_as_form(doc: &mut Document, index: usize) -> Result<(ObjRef, Rect, Matrix)> {
    let info = doc.page_info(index)?;
    let content = doc.page_content(index)?;
    let page = doc.page_ref(index)?;
    let resources = doc
        .page_attr(page, "Resources")
        .cloned()
        .unwrap_or_else(|| Dict::new().into());
    let bbox = info.visible_box();
    let dict = Dict::new()
        .with("Type", "XObject")
        .with("Subtype", "Form")
        .with("BBox", bbox.to_object())
        .with("Resources", resources)
        .with("Filter", "FlateDecode");
    let r = doc.add(Stream::new(dict, crate::filters::flate_encode(&content, 6)).into());
    // Matrix that puts the displayed page (rotation applied) with its
    // bottom-left corner at the origin.
    let disp_to_user = info.display_to_user();
    let user_to_disp = disp_to_user.invert().unwrap_or(Matrix::IDENTITY);
    let disp = Rect::new(0.0, 0.0, info.display_width(), info.display_height());
    Ok((r, disp, user_to_disp))
}

fn sheet_size(opts: &ImposeOptions) -> Result<PageSize> {
    let mut s = PageSize::by_name(&opts.sheet)
        .ok_or_else(|| Error::Preset(format!("unknown sheet size '{}'", opts.sheet)))?;
    if opts.landscape && s.width < s.height {
        s = PageSize::new(s.height, s.width);
    }
    Ok(s)
}

/// Builds sheets holding `slots` (as `(page index or None, cell rect)`) and
/// appends them; returns the new page indices.
fn build_sheets(
    doc: &mut Document,
    sheets: &[Vec<Option<usize>>],
    cells: &[Rect],
    size: PageSize,
    opts: &ImposeOptions,
) -> Result<Vec<usize>> {
    let mut forms: std::collections::HashMap<usize, (ObjRef, Rect, Matrix)> =
        std::collections::HashMap::new();
    let mut out = Vec::new();
    for sheet in sheets {
        let idx = doc.page_count();
        doc.add_page(size);
        let mut cb = ContentBuilder::new();
        for (slot, cell) in sheet.iter().zip(cells.iter()) {
            let p = match slot {
                Some(p) => *p,
                None => continue,
            };
            let (xref, disp, user_to_disp) = match forms.get(&p) {
                Some(f) => *f,
                None => {
                    let f = page_as_form(doc, p)?;
                    forms.insert(p, f);
                    f
                }
            };
            let name = doc.add_page_resource(idx, "XObject", xref)?;
            let inner = Rect::new(
                cell.x0 + opts.margin,
                cell.y0 + opts.margin,
                cell.x1 - opts.margin,
                cell.y1 - opts.margin,
            );
            let scale = (inner.width() / disp.width()).min(inner.height() / disp.height());
            let (w, h) = (disp.width() * scale, disp.height() * scale);
            let (x, y) = (
                inner.x0 + (inner.width() - w) / 2.0,
                inner.y0 + (inner.height() - h) / 2.0,
            );
            let place = user_to_disp.then(&Matrix::new(scale, 0.0, 0.0, scale, x, y));
            cb.save()
                .rect(&Rect::new(x, y, x + w, y + h))
                .clip()
                .end_path()
                .transform(&place)
                .xobject(&name)
                .restore();
            if opts.frames {
                cb.save()
                    .stroke_gray(0.6)
                    .line_width(0.5)
                    .rect(&Rect::new(x, y, x + w, y + h))
                    .stroke()
                    .restore();
            }
        }
        doc.draw(idx, &cb.finish())?;
        out.push(idx);
    }
    Ok(out)
}

/// Replaces the document's pages with sheets showing `per_sheet` pages
/// each (2 or 4), in reading order.
pub fn nup(doc: &mut Document, per_sheet: usize, opts: &ImposeOptions) -> Result<()> {
    if per_sheet != 2 && per_sheet != 4 {
        return Err(Error::Preset("perSheet must be 2 or 4".into()));
    }
    let n = doc.page_count();
    if n == 0 {
        return Ok(());
    }
    let size = sheet_size(opts)?;
    let (cols, rows) = if per_sheet == 2 {
        if size.width >= size.height {
            (2, 1)
        } else {
            (1, 2)
        }
    } else {
        (2, 2)
    };
    let (cw, ch) = (size.width / cols as f64, size.height / rows as f64);
    let mut cells = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let y1 = size.height - r as f64 * ch;
            cells.push(Rect::new(c as f64 * cw, y1 - ch, (c + 1) as f64 * cw, y1));
        }
    }
    let sheets: Vec<Vec<Option<usize>>> = (0..n)
        .step_by(per_sheet)
        .map(|s| {
            (0..per_sheet)
                .map(|k| (s + k < n).then_some(s + k))
                .collect()
        })
        .collect();
    let new = build_sheets(doc, &sheets, &cells, size, opts)?;
    doc.select_pages(&new)
}

/// Replaces the pages with a booklet: 2-up sheets in the order that folds
/// into a book when printed double-sided (flip on the short edge). Blank
/// slots pad the count to a multiple of four.
pub fn booklet(doc: &mut Document, opts: &ImposeOptions) -> Result<()> {
    let n = doc.page_count();
    if n == 0 {
        return Ok(());
    }
    let size = sheet_size(opts)?;
    let total = n.div_ceil(4) * 4;
    let page = |i: usize| -> Option<usize> { (i < n).then_some(i) };
    let mut sheets = Vec::new();
    let (mut lo, mut hi) = (0usize, total - 1);
    while lo < hi {
        sheets.push(vec![page(hi), page(lo)]); // front: last | first
        sheets.push(vec![page(lo + 1), page(hi - 1)]); // back: second | second-to-last
        lo += 2;
        hi -= 2;
    }
    let cw = size.width / 2.0;
    let cells = vec![
        Rect::new(0.0, 0.0, cw, size.height),
        Rect::new(cw, 0.0, size.width, size.height),
    ];
    let new = build_sheets(doc, &sheets, &cells, size, opts)?;
    doc.select_pages(&new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::{self, TextStamp};

    fn numbered(n: usize) -> Document {
        let mut d = Document::new();
        for i in 0..n {
            d.add_page(PageSize::A4);
            ops::stamp_text(
                &mut d,
                &[i],
                &TextStamp {
                    text: format!("P{}", i + 1),
                    size: 40.0,
                    opacity: 1.0,
                    ..Default::default()
                },
            )
            .unwrap();
        }
        Document::load(&d.save(&Default::default()).unwrap()).unwrap()
    }

    #[test]
    fn two_up() {
        let mut d = numbered(5);
        nup(&mut d, 2, &ImposeOptions::default()).unwrap();
        assert_eq!(d.page_count(), 3);
        let info = d.page_info(0).unwrap();
        assert!(
            (info.media_box.width() - 792.0).abs() < 0.01,
            "landscape letter"
        );
        let d = Document::load(&d.save(&Default::default()).unwrap()).unwrap();
        let t = crate::text::page_text(&d, 0).unwrap();
        assert!(t.contains("P1") && t.contains("P2"), "{t}");
        let c = crate::text::page_content(&d, 0).unwrap();
        let p1 = c.glyphs.iter().find(|g| g.text == "P").unwrap();
        assert!(
            p1.rect.x1 < 396.0,
            "first page sits in the left half: {:?}",
            p1.rect
        );
        assert!(crate::text::page_text(&d, 2).unwrap().contains("P5"));
    }

    #[test]
    fn four_up_and_booklet() {
        let mut d = numbered(6);
        nup(
            &mut d,
            4,
            &ImposeOptions {
                landscape: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(d.page_count(), 2);
        let mut b = numbered(6);
        booklet(&mut b, &ImposeOptions::default()).unwrap();
        assert_eq!(b.page_count(), 4, "6 pages pad to 8 = 4 sheet sides");
        let b = Document::load(&b.save(&Default::default()).unwrap()).unwrap();
        let first = crate::text::page_text(&b, 0).unwrap();
        assert!(
            first.contains("P1") && !first.contains("P2"),
            "front of sheet 1 holds page 1 (and blank 8): {first}"
        );
        let second = crate::text::page_text(&b, 1).unwrap();
        assert!(
            second.contains("P2") && !second.contains("P7"),
            "{second}"
        );
        assert!(nup(&mut numbered(1), 3, &ImposeOptions::default()).is_err());
    }
}
