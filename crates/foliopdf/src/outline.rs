//! Bookmarks (the document outline, ISO 32000-1 §12.3.3): read the tree,
//! replace it, and carry it across page imports.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::document::Document;
use crate::error::{Error, Result};
use crate::object::{Dict, ObjRef, Object, PdfString};

/// One bookmark and its children.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Bookmark {
    /// Title shown in the reader's sidebar.
    pub title: String,
    /// 0-based page index; `None` for bookmarks with no page (or an external link).
    pub page: Option<usize>,
    /// Optional vertical position on the page (points from the bottom) to
    /// scroll to; `None` fits the page.
    pub top: Option<f64>,
    /// Web address for link bookmarks.
    pub uri: Option<String>,
    /// Whether the reader shows the children expanded (ignored for bookmarks
    /// without children).
    pub open: bool,
    /// Bold / italic style flags as in the file (bit 1 italic, bit 2 bold).
    pub style: u8,
    /// Nested bookmarks.
    pub children: Vec<Bookmark>,
}

impl Bookmark {
    /// A bookmark to a page.
    pub fn new(title: &str, page: usize) -> Self {
        Self {
            title: title.into(),
            page: Some(page),
            open: true,
            ..Default::default()
        }
    }
    /// Adds a child and returns `self` for chaining.
    pub fn with_child(mut self, child: Bookmark) -> Self {
        self.children.push(child);
        self
    }
    fn count(&self) -> usize {
        1 + self.children.iter().map(Bookmark::count).sum::<usize>()
    }
}

/// Resolves a destination (array, name or string) to a page index and top.
fn resolve_dest(
    doc: &Document,
    dest: &Object,
    pages: &HashMap<u32, usize>,
    depth: usize,
) -> (Option<usize>, Option<f64>) {
    if depth > 4 {
        return (None, None);
    }
    match doc.resolve(dest) {
        Object::Array(a) => {
            let page = a.first().and_then(|o| match o {
                Object::Reference(r) => pages.get(&r.num).copied(),
                Object::Integer(i) => Some(*i as usize),
                _ => None,
            });
            let kind = a
                .get(1)
                .and_then(Object::as_name)
                .map(|n| n.as_str().into_owned());
            let top = match kind.as_deref() {
                Some("XYZ") => a.get(3).and_then(Object::as_f64),
                Some("FitH") | Some("FitBH") => a.get(2).and_then(Object::as_f64),
                _ => None,
            };
            (page, top)
        }
        Object::String(s) => named_dest(doc, s.as_bytes(), pages, depth),
        Object::Name(n) => named_dest(doc, n.as_bytes(), pages, depth),
        Object::Dict(d) => match d.get("D") {
            Some(inner) => resolve_dest(doc, inner, pages, depth + 1),
            None => (None, None),
        },
        _ => (None, None),
    }
}

/// Looks a named destination up in `/Dests` or the `/Names /Dests` tree.
fn named_dest(
    doc: &Document,
    name: &[u8],
    pages: &HashMap<u32, usize>,
    depth: usize,
) -> (Option<usize>, Option<f64>) {
    let cat = doc.catalog();
    if let Some(dests) = doc.dict_get(cat, "Dests").and_then(Object::as_dict) {
        let key = String::from_utf8_lossy(name).into_owned();
        if let Some(d) = dests.get(&key) {
            return resolve_dest(doc, d, pages, depth + 1);
        }
    }
    if let Some(names) = doc.dict_get(cat, "Names").and_then(Object::as_dict) {
        if let Some(tree) = names.get("Dests") {
            if let Some(v) = name_tree_lookup(doc, tree, name, 0) {
                return resolve_dest(doc, &v, pages, depth + 1);
            }
        }
    }
    (None, None)
}

fn name_tree_lookup(doc: &Document, node: &Object, key: &[u8], depth: usize) -> Option<Object> {
    if depth > 32 {
        return None;
    }
    let d = doc.resolve(node).as_dict()?;
    if let Some(Object::Array(names)) = doc.dict_get(d, "Names") {
        let mut i = 0;
        while i + 1 < names.len() {
            if doc
                .resolve(&names[i])
                .as_string()
                .map(|s| s.as_bytes() == key)
                .unwrap_or(false)
            {
                return Some(names[i + 1].clone());
            }
            i += 2;
        }
    }
    if let Some(Object::Array(kids)) = doc.dict_get(d, "Kids") {
        for k in kids {
            if let Some(kd) = doc.resolve(k).as_dict() {
                if let Some(Object::Array(lim)) = doc.dict_get(kd, "Limits") {
                    let lo = lim
                        .first()
                        .and_then(|o| doc.resolve(o).as_string())
                        .map(|s| s.as_bytes().to_vec());
                    let hi = lim
                        .get(1)
                        .and_then(|o| doc.resolve(o).as_string())
                        .map(|s| s.as_bytes().to_vec());
                    if let (Some(lo), Some(hi)) = (lo, hi) {
                        if key < lo.as_slice() || key > hi.as_slice() {
                            continue;
                        }
                    }
                }
            }
            if let Some(v) = name_tree_lookup(doc, k, key, depth + 1) {
                return Some(v);
            }
        }
    }
    None
}

fn read_item(
    doc: &Document,
    r: ObjRef,
    pages: &HashMap<u32, usize>,
    seen: &mut HashSet<u32>,
    depth: usize,
) -> Option<Bookmark> {
    if depth > 32 || !seen.insert(r.num) {
        return None;
    }
    let d = doc.get(r).as_dict()?.clone();
    let title = doc
        .dict_get(&d, "Title")
        .and_then(Object::as_string)
        .map(PdfString::to_text)
        .unwrap_or_default();
    let mut page = None;
    let mut top = None;
    let mut uri = None;
    if let Some(dest) = d.get("Dest") {
        let (p, t) = resolve_dest(doc, dest, pages, 0);
        page = p;
        top = t;
    } else if let Some(a) = doc.dict_get(&d, "A").and_then(Object::as_dict) {
        match doc
            .dict_get(a, "S")
            .and_then(Object::as_name)
            .map(|n| n.as_str().into_owned())
            .as_deref()
        {
            Some("GoTo") => {
                if let Some(dd) = a.get("D") {
                    let (p, t) = resolve_dest(doc, dd, pages, 0);
                    page = p;
                    top = t;
                }
            }
            Some("URI") => {
                uri = doc
                    .dict_get(a, "URI")
                    .and_then(Object::as_string)
                    .map(|s| String::from_utf8_lossy(s.as_bytes()).into_owned())
            }
            _ => {}
        }
    }
    let count = doc
        .dict_get(&d, "Count")
        .and_then(Object::as_i64)
        .unwrap_or(0);
    let style = doc.dict_get(&d, "F").and_then(Object::as_i64).unwrap_or(0) as u8 & 3;
    let mut children = Vec::new();
    let mut cur = d.get("First").and_then(Object::as_reference);
    let mut guard = 0;
    while let Some(c) = cur {
        guard += 1;
        if guard > 10_000 {
            break;
        }
        if let Some(b) = read_item(doc, c, pages, seen, depth + 1) {
            children.push(b);
        }
        cur = doc
            .get(c)
            .as_dict()
            .and_then(|cd| cd.get("Next"))
            .and_then(Object::as_reference);
    }
    let open = count > 0 || children.is_empty(); // `open` only means something with children
    Some(Bookmark {
        title,
        page,
        top,
        uri,
        open,
        style,
        children,
    })
}

/// Reads the bookmark tree.
pub fn bookmarks(doc: &Document) -> Vec<Bookmark> {
    let pages: HashMap<u32, usize> = doc
        .page_refs()
        .iter()
        .enumerate()
        .map(|(i, r)| (r.num, i))
        .collect();
    let root = match doc
        .dict_get(doc.catalog(), "Outlines")
        .and_then(Object::as_dict)
    {
        Some(r) => r.clone(),
        None => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut cur = root.get("First").and_then(Object::as_reference);
    let mut guard = 0;
    while let Some(c) = cur {
        guard += 1;
        if guard > 10_000 {
            break;
        }
        if let Some(b) = read_item(doc, c, &pages, &mut seen, 0) {
            out.push(b);
        }
        cur = doc
            .get(c)
            .as_dict()
            .and_then(|cd| cd.get("Next"))
            .and_then(Object::as_reference);
    }
    out
}

/// Number of bookmarks including nested ones.
pub fn bookmark_count(doc: &Document) -> usize {
    bookmarks(doc).iter().map(Bookmark::count).sum()
}

fn write_items(
    doc: &mut Document,
    items: &[Bookmark],
    parent: ObjRef,
    page_refs: &[ObjRef],
) -> Result<(Option<ObjRef>, Option<ObjRef>, i64)> {
    let refs: Vec<ObjRef> = items.iter().map(|_| doc.add(Object::Null)).collect();
    let mut total = 0i64;
    for (i, b) in items.iter().enumerate() {
        let r = refs[i];
        let mut d = Dict::new()
            .with("Title", PdfString::from_text(&b.title))
            .with("Parent", parent);
        if i > 0 {
            d.set("Prev", refs[i - 1]);
        }
        if i + 1 < refs.len() {
            d.set("Next", refs[i + 1]);
        }
        if let Some(p) = b.page {
            let pr = *page_refs.get(p).ok_or(Error::PageOutOfRange {
                index: p,
                count: page_refs.len(),
            })?;
            let dest = match b.top {
                Some(t) => Object::Array(vec![
                    pr.into(),
                    Object::name("XYZ"),
                    Object::Null,
                    Object::Real(t),
                    Object::Null,
                ]),
                None => Object::Array(vec![pr.into(), Object::name("Fit")]),
            };
            d.set("Dest", dest);
        } else if let Some(u) = &b.uri {
            d.set(
                "A",
                Dict::new()
                    .with("S", "URI")
                    .with("URI", PdfString::new(u.as_bytes().to_vec())),
            );
        }
        if b.style != 0 {
            d.set("F", b.style as i64);
        }
        if !b.children.is_empty() {
            let (first, last, n) = write_items(doc, &b.children, r, page_refs)?;
            if let (Some(f), Some(l)) = (first, last) {
                d.set("First", f);
                d.set("Last", l);
            }
            d.set("Count", if b.open { n } else { -n });
            total += if b.open { n } else { 0 };
        }
        total += 1;
        doc.set(r, d.into());
    }
    Ok((refs.first().copied(), refs.last().copied(), total))
}

/// Replaces the bookmark tree. An empty list removes it.
pub fn set_bookmarks(doc: &mut Document, items: &[Bookmark]) -> Result<()> {
    if items.is_empty() {
        doc.catalog_mut().remove("Outlines");
        return Ok(());
    }
    let page_refs = doc.page_refs();
    let root = doc.add(Object::Null);
    let (first, last, count) = write_items(doc, items, root, &page_refs)?;
    let mut d = Dict::new().with("Type", "Outlines").with("Count", count);
    if let (Some(f), Some(l)) = (first, last) {
        d.set("First", f);
        d.set("Last", l);
    }
    doc.set(root, d.into());
    doc.catalog_mut().set("Outlines", root);
    Ok(())
}

/// Bookmarks of `src` restricted to the imported pages, with page indices
/// translated through `map` (source page index → destination page index).
/// Items whose page was not imported are dropped; their children move up.
pub fn imported_bookmarks(src: &Document, map: &HashMap<usize, usize>) -> Vec<Bookmark> {
    fn translate(items: &[Bookmark], map: &HashMap<usize, usize>) -> Vec<Bookmark> {
        let mut out = Vec::new();
        for b in items {
            let kids = translate(&b.children, map);
            match b.page {
                Some(p) => match map.get(&p) {
                    Some(&np) => out.push(Bookmark {
                        page: Some(np),
                        children: kids,
                        ..b.clone()
                    }),
                    None => out.extend(kids),
                },
                None if b.uri.is_some() => out.push(Bookmark {
                    children: kids,
                    ..b.clone()
                }),
                None => {
                    // A heading with no target: keep it only if it has surviving children.
                    if !kids.is_empty() {
                        out.push(Bookmark {
                            children: kids,
                            ..b.clone()
                        });
                    }
                }
            }
        }
        out
    }
    translate(&bookmarks(src), map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::PageSize;

    fn doc(n: usize) -> Document {
        let mut d = Document::new();
        for _ in 0..n {
            d.add_page(PageSize::LETTER);
        }
        d
    }

    #[test]
    fn roundtrip_tree() {
        let mut d = doc(5);
        let tree = vec![
            Bookmark::new("Intro", 0)
                .with_child(Bookmark {
                    top: Some(500.0),
                    ..Bookmark::new("Scope", 0)
                })
                .with_child(Bookmark::new("Terms", 1)),
            Bookmark {
                open: false,
                style: 2,
                ..Bookmark::new("Body", 2).with_child(Bookmark::new("Detail", 3))
            },
            Bookmark {
                title: "Site".into(),
                uri: Some("https://example.com".into()),
                open: true,
                ..Default::default()
            },
        ];
        set_bookmarks(&mut d, &tree).unwrap();
        let bytes = d.save(&Default::default()).unwrap();
        let re = Document::load(&bytes).unwrap();
        let got = bookmarks(&re);
        assert_eq!(got, tree);
        assert_eq!(bookmark_count(&re), 6);
        let mut re = re;
        set_bookmarks(&mut re, &[]).unwrap();
        assert!(bookmarks(&re).is_empty());
    }

    #[test]
    fn out_of_range_page_is_an_error() {
        let mut d = doc(1);
        assert!(set_bookmarks(&mut d, &[Bookmark::new("x", 5)]).is_err());
    }

    #[test]
    fn named_destinations() {
        let mut d = doc(3);
        let p2 = d.page_refs()[2];
        d.catalog_mut().set(
            "Dests",
            Dict::new().with(
                "chap2",
                Object::Array(vec![
                    p2.into(),
                    Object::name("XYZ"),
                    Object::Null,
                    Object::Real(700.0),
                    Object::Null,
                ]),
            ),
        );
        let item = d.add(
            Dict::new()
                .with("Title", PdfString::from_text("Chapter 2"))
                .with("Dest", PdfString::new(b"chap2".to_vec()))
                .into(),
        );
        let root = d.add(
            Dict::new()
                .with("Type", "Outlines")
                .with("First", item)
                .with("Last", item)
                .with("Count", 1)
                .into(),
        );
        d.catalog_mut().set("Outlines", root);
        let b = bookmarks(&d);
        assert_eq!(b[0].page, Some(2));
        assert_eq!(b[0].top, Some(700.0));
    }

    #[test]
    fn merge_and_extract_keep_bookmarks() {
        let mut a = doc(3);
        set_bookmarks(
            &mut a,
            &[
                Bookmark::new("A1", 0).with_child(Bookmark::new("A1.1", 1)),
                Bookmark::new("A2", 2),
            ],
        )
        .unwrap();
        let mut b = doc(2);
        set_bookmarks(&mut b, &[Bookmark::new("B1", 1)]).unwrap();
        let merged = crate::ops::merge(&[&a, &b]).unwrap();
        let got = bookmarks(&merged);
        assert_eq!(
            got.iter()
                .map(|x| (x.title.as_str(), x.page))
                .collect::<Vec<_>>(),
            [("A1", Some(0)), ("A2", Some(2)), ("B1", Some(4))]
        );
        assert_eq!(got[0].children[0].page, Some(1));
        // Extracting page 2 only: A1 is dropped, its child A1.1 moves up.
        let part = crate::ops::extract(&a, &[1]).unwrap();
        let got = bookmarks(&part);
        assert_eq!(
            got.iter()
                .map(|x| (x.title.as_str(), x.page))
                .collect::<Vec<_>>(),
            [("A1.1", Some(0))]
        );
        let bytes = merged.clone().save(&Default::default()).unwrap();
        assert_eq!(bookmark_count(&Document::load(&bytes).unwrap()), 4);
    }
}
