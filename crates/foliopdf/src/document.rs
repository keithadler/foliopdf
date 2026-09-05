//! The [`Document`] type: loading, inspecting, editing and saving.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use crate::crypto::{EncryptionOptions, SecurityHandler};
use crate::error::{Error, Result};
use crate::filters;
use crate::font::{Font, StandardFont};
use crate::geometry::Rect;
use crate::image::Image;
use crate::object::{Dict, Name, ObjRef, Object, PdfString, Stream};
use crate::page::{PageInfo, PageSize, Rotation};
use crate::parser::{self, Parser, XrefEntry};
use crate::writer;

/// Options for [`Document::load_with`].
#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    /// Password for encrypted files (user or owner). Empty tries the empty
    /// user password, which opens most "encrypted" files that only restrict
    /// permissions.
    pub password: String,
    /// When the cross-reference data is unusable, scan the file for objects
    /// instead of failing. Default `true`.
    pub recover: bool,
}

impl LoadOptions {
    /// Default options with a password.
    pub fn with_password(password: &str) -> Self {
        Self {
            password: password.to_owned(),
            recover: true,
        }
    }
}

/// Options for [`Document::save`].
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SaveOptions {
    /// Recompress every lossless stream with Flate and pack non-stream
    /// objects into object streams. Default `true`.
    pub compress: bool,
    /// Flate level 1–10 (miniz scale; 6 is a good default, 10 is slowest).
    pub compression_level: u8,
    /// Use object streams and a cross-reference stream (PDF 1.5+). Ignored
    /// when `compress` is false. Default `true`.
    pub object_streams: bool,
    /// Remove the XMP metadata stream, document info dictionary and
    /// per-page thumbnails. Default `false`.
    pub strip_metadata: bool,
    /// Encrypt the output.
    pub encryption: Option<EncryptionOptions>,
    /// Producer string written to the info dictionary. `None` leaves the
    /// existing value; `Some("")` removes it.
    pub producer: Option<String>,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self {
            compress: true,
            compression_level: 6,
            object_streams: true,
            strip_metadata: false,
            encryption: None,
            producer: Some(format!("foliopdf {}", crate::VERSION)),
        }
    }
}

/// Document information fields (the `/Info` dictionary).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Metadata {
    /// Title.
    pub title: Option<String>,
    /// Author.
    pub author: Option<String>,
    /// Subject.
    pub subject: Option<String>,
    /// Keywords.
    pub keywords: Option<String>,
    /// Creating application.
    pub creator: Option<String>,
    /// Producing library.
    pub producer: Option<String>,
}

/// An in-memory PDF document.
///
/// All indirect objects are held in memory (streams stay compressed). Object
/// numbers are stable while editing; saving renumbers and drops anything
/// unreachable from the catalog.
#[derive(Debug, Clone)]
pub struct Document {
    objects: BTreeMap<u32, Object>,
    trailer: Dict,
    next_num: u32,
    version: (u8, u8),
    pub(crate) fonts: Vec<(ObjRef, Font)>,
    security: Option<SecurityHandler>,
    encryption_desc: Option<String>,
    reconstructed: bool,
}

impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}

static NULL: Object = Object::Null;

impl Document {
    /// Creates an empty document with no pages.
    pub fn new() -> Self {
        let mut doc = Self {
            objects: BTreeMap::new(),
            trailer: Dict::new(),
            next_num: 1,
            version: (1, 7),
            fonts: Vec::new(),
            security: None,
            encryption_desc: None,
            reconstructed: false,
        };
        let pages = doc.add(
            Dict::new()
                .with("Type", "Pages")
                .with("Kids", Vec::<Object>::new())
                .with("Count", 0)
                .into(),
        );
        let catalog = doc.add(
            Dict::new()
                .with("Type", "Catalog")
                .with("Pages", pages)
                .into(),
        );
        doc.trailer.set("Root", catalog);
        doc
    }

    /// Loads a document, trying the empty password if it is encrypted.
    pub fn load(bytes: &[u8]) -> Result<Self> {
        Self::load_with(
            bytes,
            &LoadOptions {
                recover: true,
                ..Default::default()
            },
        )
    }

    /// Loads a document with options.
    pub fn load_with(bytes: &[u8], opts: &LoadOptions) -> Result<Self> {
        if bytes.len() < 16 {
            return Err(Error::malformed("file too small to be a PDF"));
        }
        let version = parser::header_version(bytes).unwrap_or((1, 4));
        let mut xref = match parser::read_xref(bytes) {
            Ok(x) => x,
            Err(e) if opts.recover => parser::reconstruct(bytes).map_err(|_| e)?,
            Err(e) => return Err(e),
        };
        let mut doc = match Self::load_objects(bytes, &xref, opts) {
            Ok(d) => d,
            Err(e) if opts.recover && !xref.reconstructed => {
                xref = parser::reconstruct(bytes)?;
                Self::load_objects(bytes, &xref, opts).map_err(|_| e)?
            }
            Err(e) => return Err(e),
        };
        doc.version = version;
        doc.reconstructed = xref.reconstructed;
        // Validate the catalog; fall back to scanning if it is broken.
        if doc
            .catalog_ref()
            .and_then(|r| doc.get(r).as_dict())
            .is_none()
            && opts.recover
            && !xref.reconstructed
        {
            xref = parser::reconstruct(bytes)?;
            doc = Self::load_objects(bytes, &xref, opts)?;
            doc.version = version;
            doc.reconstructed = true;
        }
        if doc
            .catalog_ref()
            .and_then(|r| doc.get(r).as_dict())
            .is_none()
        {
            // Last resort: find any /Type /Catalog object.
            let found = doc
                .objects
                .iter()
                .find(|(_, o)| {
                    o.as_dict()
                        .and_then(|d| d.get("Type"))
                        .and_then(Object::as_name)
                        .map(|n| n == "Catalog")
                        .unwrap_or(false)
                })
                .map(|(&n, _)| n);
            match found {
                Some(n) => {
                    doc.trailer.set("Root", ObjRef::new(n, 0));
                }
                None => return Err(Error::malformed("no document catalog")),
            }
        }
        Ok(doc)
    }

    fn load_objects(bytes: &[u8], xref: &parser::Xref, opts: &LoadOptions) -> Result<Self> {
        let mut objects: BTreeMap<u32, Object> = BTreeMap::new();
        let mut gens: HashMap<u32, u16> = HashMap::new();
        let length_of = |r: ObjRef| -> Option<i64> {
            match xref.entries.get(&r.num) {
                Some(XrefEntry::Offset { offset, .. }) => {
                    let mut p = Parser::new(bytes, *offset);
                    p.parse_indirect(None).ok().and_then(|(_, o)| o.as_i64())
                }
                _ => None,
            }
        };
        // Pass 1: objects stored directly in the file.
        let mut in_stream: Vec<(u32, u32, u32)> = Vec::new();
        for (&num, entry) in &xref.entries {
            match entry {
                XrefEntry::Offset { offset, gen } => {
                    if *offset >= bytes.len() {
                        continue;
                    }
                    let mut p = Parser::new(bytes, *offset);
                    match p.parse_indirect(Some(&length_of)) {
                        Ok((r, obj)) if r.num == num => {
                            objects.insert(num, obj);
                            gens.insert(num, *gen);
                        }
                        // Offsets that point at the wrong object mean the xref is stale.
                        Ok(_) | Err(_) => {
                            if !xref.reconstructed {
                                return Err(Error::malformed(format!(
                                    "object {num} not at recorded offset"
                                )));
                            }
                        }
                    }
                }
                XrefEntry::InStream { stream_num, index } => {
                    in_stream.push((num, *stream_num, *index))
                }
                XrefEntry::Free => {}
            }
        }
        let mut trailer = xref.trailer.clone();
        // Decrypt before expanding object streams (the streams themselves are encrypted).
        let mut security = None;
        let mut encryption_desc = None;
        if let Some(enc) = trailer.get("Encrypt").cloned() {
            let enc_ref = enc.as_reference();
            let enc_dict = match &enc {
                Object::Reference(r) => objects.get(&r.num).and_then(Object::as_dict).cloned(),
                Object::Dict(d) => Some(d.clone()),
                _ => None,
            }
            .ok_or_else(|| Error::malformed("missing /Encrypt dictionary"))?;
            let id0 = trailer
                .get("ID")
                .and_then(Object::as_array)
                .and_then(|a| a.first())
                .and_then(Object::as_string)
                .map(|s| s.bytes.clone())
                .unwrap_or_default();
            encryption_desc = Some(crate::crypto::describe(&enc_dict));
            let handler = SecurityHandler::open(&enc_dict, &id0, &opts.password)?;
            for (&num, obj) in objects.iter_mut() {
                if Some(num) == enc_ref.map(|r| r.num) {
                    continue;
                }
                let gen = gens.get(&num).copied().unwrap_or(0);
                handler.decrypt_object(obj, ObjRef::new(num, gen))?;
            }
            if let Some(r) = enc_ref {
                objects.remove(&r.num);
            }
            trailer.remove("Encrypt");
            security = Some(handler);
        }
        // Pass 2: objects inside object streams.
        let mut expanded: HashMap<u32, Vec<(u32, Object)>> = HashMap::new();
        for (num, stream_num, index) in in_stream {
            if let std::collections::hash_map::Entry::Vacant(e) = expanded.entry(stream_num) {
                let parsed = match objects.get(&stream_num) {
                    Some(Object::Stream(s)) => {
                        let data = filters::decode_stream(s, None)?;
                        parser::parse_object_stream(&s.dict, &data)?
                    }
                    _ => Vec::new(),
                };
                e.insert(parsed);
            }
            let list = &expanded[&stream_num];
            let found = list
                .get(index as usize)
                .filter(|(n, _)| *n == num)
                .or_else(|| list.iter().find(|(n, _)| *n == num));
            if let Some((_, obj)) = found {
                objects.insert(num, obj.clone());
            }
        }
        // Reconstructed files: also expand every object stream we can find.
        if xref.reconstructed {
            let stream_nums: Vec<u32> = objects
                .iter()
                .filter(|(_, o)| {
                    o.as_stream()
                        .and_then(|s| s.dict.get("Type"))
                        .and_then(Object::as_name)
                        .map(|n| n == "ObjStm")
                        .unwrap_or(false)
                })
                .map(|(&n, _)| n)
                .collect();
            for sn in stream_nums {
                if let Some(Object::Stream(s)) = objects.get(&sn).cloned() {
                    if let Ok(data) = filters::decode_stream(&s, None) {
                        if let Ok(list) = parser::parse_object_stream(&s.dict, &data) {
                            for (n, o) in list {
                                objects.entry(n).or_insert(o);
                            }
                        }
                    }
                }
            }
        }
        // Drop structural objects we regenerate on save.
        objects.retain(|_, o| {
            let ty = o
                .as_stream()
                .and_then(|s| s.dict.get("Type"))
                .and_then(Object::as_name);
            !matches!(
                ty.map(|n| n.as_str().into_owned()).as_deref(),
                Some("ObjStm") | Some("XRef")
            )
        });
        trailer.remove("Prev");
        trailer.remove("XRefStm");
        trailer.remove("Size");
        let next_num = objects.keys().next_back().map(|n| n + 1).unwrap_or(1);
        Ok(Self {
            objects,
            trailer,
            next_num,
            version: (1, 7),
            fonts: Vec::new(),
            security,
            encryption_desc,
            reconstructed: false,
        })
    }

    // -- inspection -----------------------------------------------------------

    /// PDF header version of the loaded file (or of the output for new
    /// documents).
    pub fn version(&self) -> (u8, u8) {
        self.version
    }
    /// Whether the input was encrypted.
    pub fn was_encrypted(&self) -> bool {
        self.security.is_some()
    }
    /// Human-readable description of the input's encryption, e.g. `AES-256`.
    pub fn encryption_description(&self) -> Option<&str> {
        self.encryption_desc.as_deref()
    }
    /// Whether the file was opened with owner rights (or was not encrypted).
    pub fn has_owner_access(&self) -> bool {
        self.security.as_ref().map(|s| s.is_owner).unwrap_or(true)
    }
    /// Permissions declared by an encrypted input.
    pub fn input_permissions(&self) -> Option<crate::crypto::Permissions> {
        self.security.as_ref().map(|s| s.permissions)
    }
    /// Whether the cross-reference table had to be rebuilt by scanning.
    pub fn was_reconstructed(&self) -> bool {
        self.reconstructed
    }
    /// Number of indirect objects currently held.
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }
    /// The trailer dictionary (`/Root`, `/Info`, `/ID`).
    pub fn trailer(&self) -> &Dict {
        &self.trailer
    }

    // -- objects ----------------------------------------------------------------

    /// Returns the object `r` refers to, or `null` if it does not exist.
    pub fn get(&self, r: ObjRef) -> &Object {
        self.objects.get(&r.num).unwrap_or(&NULL)
    }
    /// Follows references until reaching a direct object.
    pub fn resolve<'a>(&'a self, o: &'a Object) -> &'a Object {
        let mut cur = o;
        for _ in 0..32 {
            match cur {
                Object::Reference(r) => cur = self.get(*r),
                _ => return cur,
            }
        }
        &NULL
    }
    /// Mutable access to an indirect object.
    pub fn get_mut(&mut self, r: ObjRef) -> Option<&mut Object> {
        self.objects.get_mut(&r.num)
    }
    /// Adds a new indirect object.
    pub fn add(&mut self, o: Object) -> ObjRef {
        let n = self.next_num;
        self.next_num += 1;
        self.objects.insert(n, o);
        ObjRef::new(n, 0)
    }
    /// Replaces (or creates) the object at `r`.
    pub fn set(&mut self, r: ObjRef, o: Object) {
        if r.num >= self.next_num {
            self.next_num = r.num + 1;
        }
        self.objects.insert(r.num, o);
    }
    /// Deletes an indirect object. Dangling references resolve to `null`.
    pub fn remove_object(&mut self, r: ObjRef) {
        self.objects.remove(&r.num);
    }
    /// Iterates over all indirect objects.
    pub fn objects(&self) -> impl Iterator<Item = (ObjRef, &Object)> {
        self.objects.iter().map(|(&n, o)| (ObjRef::new(n, 0), o))
    }
    /// Fully decodes a stream's data (image codecs excepted; see
    /// [`filters`]).
    pub fn stream_data(&self, s: &Stream) -> Result<Vec<u8>> {
        let resolve = |o: &Object| self.resolve(o).clone();
        filters::decode_stream(s, Some(&resolve))
    }
    /// Convenience: resolves `dict[key]` through references.
    pub fn dict_get<'a>(&'a self, dict: &'a Dict, key: &str) -> Option<&'a Object> {
        dict.get(key)
            .map(|o| self.resolve(o))
            .filter(|o| !o.is_null())
    }

    // -- catalog and info -------------------------------------------------------

    /// Reference to the catalog.
    pub fn catalog_ref(&self) -> Option<ObjRef> {
        self.trailer.get("Root").and_then(Object::as_reference)
    }
    /// The document catalog.
    pub fn catalog(&self) -> &Dict {
        self.catalog_ref()
            .and_then(|r| self.get(r).as_dict())
            .unwrap_or_else(|| {
                static EMPTY: std::sync::OnceLock<Dict> = std::sync::OnceLock::new();
                EMPTY.get_or_init(Dict::new)
            })
    }
    /// Mutable catalog.
    pub fn catalog_mut(&mut self) -> &mut Dict {
        let r = self.catalog_ref().expect("document has a catalog");
        if !matches!(self.objects.get(&r.num), Some(Object::Dict(_))) {
            self.objects
                .insert(r.num, Dict::new().with("Type", "Catalog").into());
        }
        self.objects
            .get_mut(&r.num)
            .and_then(Object::as_dict_mut)
            .unwrap()
    }
    /// The info dictionary, if any.
    pub fn info(&self) -> Option<&Dict> {
        self.trailer
            .get("Info")
            .map(|o| self.resolve(o))
            .and_then(Object::as_dict)
    }
    /// Mutable info dictionary, created on demand.
    pub fn info_mut(&mut self) -> &mut Dict {
        let r = match self.trailer.get("Info").and_then(Object::as_reference) {
            Some(r) if matches!(self.objects.get(&r.num), Some(Object::Dict(_))) => r,
            _ => {
                let r = self.add(Dict::new().into());
                self.trailer.set("Info", r);
                r
            }
        };
        self.objects
            .get_mut(&r.num)
            .and_then(Object::as_dict_mut)
            .unwrap()
    }
    /// Reads the info dictionary into a [`Metadata`].
    pub fn metadata(&self) -> Metadata {
        let get = |k: &str| {
            self.info()
                .and_then(|i| self.dict_get(i, k))
                .and_then(Object::as_string)
                .map(PdfString::to_text)
        };
        Metadata {
            title: get("Title"),
            author: get("Author"),
            subject: get("Subject"),
            keywords: get("Keywords"),
            creator: get("Creator"),
            producer: get("Producer"),
        }
    }
    /// Writes non-`None` fields of `m` to the info dictionary. Empty strings
    /// remove the key.
    pub fn set_metadata(&mut self, m: &Metadata) {
        let pairs = [
            ("Title", &m.title),
            ("Author", &m.author),
            ("Subject", &m.subject),
            ("Keywords", &m.keywords),
            ("Creator", &m.creator),
            ("Producer", &m.producer),
        ];
        let info = self.info_mut();
        for (k, v) in pairs {
            match v {
                Some(s) if s.is_empty() => {
                    info.remove(k);
                }
                Some(s) => {
                    info.set(k, Object::text(s));
                }
                None => {}
            }
        }
    }
    /// Sets `/Title`.
    pub fn set_title(&mut self, title: &str) {
        self.info_mut().set("Title", Object::text(title));
    }
    /// Sets `/Author`.
    pub fn set_author(&mut self, author: &str) {
        self.info_mut().set("Author", Object::text(author));
    }
    /// Removes XMP metadata, the info dictionary and page thumbnails.
    pub fn strip_metadata(&mut self) {
        self.catalog_mut().remove("Metadata");
        self.catalog_mut().remove("PieceInfo");
        if let Some(r) = self.trailer.get("Info").and_then(Object::as_reference) {
            self.objects.remove(&r.num);
        }
        self.trailer.remove("Info");
        for p in self.page_refs() {
            if let Some(d) = self.get_mut(p).and_then(Object::as_dict_mut) {
                d.remove("Thumb");
                d.remove("PieceInfo");
                d.remove("Metadata");
            }
        }
    }

    // -- pages ------------------------------------------------------------------

    fn pages_root(&self) -> Option<ObjRef> {
        self.catalog().get("Pages").and_then(Object::as_reference)
    }

    /// References to all pages in display order.
    pub fn page_refs(&self) -> Vec<ObjRef> {
        let mut out = Vec::new();
        let Some(root) = self.pages_root() else {
            return out;
        };
        let mut visited = HashSet::new();
        let mut stack = vec![root];
        // Depth-first, keeping kids in order.
        while let Some(r) = stack.pop() {
            if !visited.insert(r.num) {
                continue;
            }
            let Some(d) = self.get(r).as_dict() else {
                continue;
            };
            let is_pages = d
                .get("Type")
                .and_then(Object::as_name)
                .map(|n| n == "Pages")
                .unwrap_or(false)
                || d.contains("Kids");
            if is_pages {
                if let Some(kids) = self.dict_get(d, "Kids").and_then(Object::as_array) {
                    for k in kids.iter().rev() {
                        if let Some(kr) = k.as_reference() {
                            stack.push(kr);
                        }
                    }
                }
            } else if d
                .get("Type")
                .and_then(Object::as_name)
                .map(|n| n == "Page")
                .unwrap_or(d.contains("Contents") || d.contains("MediaBox"))
            {
                out.push(r);
            }
        }
        out
    }
    /// Number of pages.
    pub fn page_count(&self) -> usize {
        self.page_refs().len()
    }
    /// Reference to page `index`.
    pub fn page_ref(&self, index: usize) -> Result<ObjRef> {
        let refs = self.page_refs();
        refs.get(index).copied().ok_or(Error::PageOutOfRange {
            index,
            count: refs.len(),
        })
    }
    /// Looks up an inheritable page attribute (`Resources`, `MediaBox`,
    /// `CropBox`, `Rotate`), walking up `/Parent`.
    pub fn page_attr(&self, page: ObjRef, key: &str) -> Option<&Object> {
        let mut cur = page;
        for _ in 0..64 {
            let d = self.get(cur).as_dict()?;
            if let Some(v) = self.dict_get(d, key) {
                return Some(v);
            }
            cur = d.get("Parent").and_then(Object::as_reference)?;
        }
        None
    }
    /// Geometry summary for page `index`.
    pub fn page_info(&self, index: usize) -> Result<PageInfo> {
        let r = self.page_ref(index)?;
        let media_box = self
            .page_attr(r, "MediaBox")
            .and_then(Rect::from_object)
            .filter(|b| b.width() > 1.0 && b.height() > 1.0)
            .unwrap_or(PageSize::LETTER.rect());
        let crop_box = self
            .page_attr(r, "CropBox")
            .and_then(Rect::from_object)
            .filter(|b| b.width() > 1.0 && b.height() > 1.0);
        let rotation = Rotation::from_degrees(
            self.page_attr(r, "Rotate")
                .and_then(Object::as_i64)
                .unwrap_or(0),
        );
        Ok(PageInfo {
            index,
            obj: r,
            media_box,
            crop_box,
            rotation,
        })
    }
    /// Geometry for every page.
    pub fn pages(&self) -> Vec<PageInfo> {
        (0..self.page_count())
            .filter_map(|i| self.page_info(i).ok())
            .collect()
    }

    /// Rewrites the page tree as a single `/Pages` node with every page as a
    /// direct kid. Inherited attributes are pushed down onto the pages first.
    pub fn flatten_page_tree(&mut self) -> ObjRef {
        let pages = self.page_refs();
        for &p in &pages {
            for key in ["Resources", "MediaBox", "CropBox", "Rotate"] {
                let inherited = self.page_attr(p, key).cloned();
                if let (Some(v), Some(d)) =
                    (inherited, self.get_mut(p).and_then(Object::as_dict_mut))
                {
                    if !d.contains(key) {
                        d.set(key, v);
                    }
                }
            }
        }
        let root = match self.pages_root() {
            Some(r) if matches!(self.get(r), Object::Dict(_)) => r,
            _ => {
                let r = self.add(Dict::new().into());
                self.catalog_mut().set("Pages", r);
                r
            }
        };
        let kids: Vec<Object> = pages.iter().map(|&p| p.into()).collect();
        let count = kids.len();
        let root_dict = self.get_mut(root).and_then(Object::as_dict_mut).unwrap();
        *root_dict = Dict::new()
            .with("Type", "Pages")
            .with("Kids", kids)
            .with("Count", count);
        for &p in &pages {
            if let Some(d) = self.get_mut(p).and_then(Object::as_dict_mut) {
                d.set("Parent", root);
            }
        }
        root
    }

    fn set_kids(&mut self, root: ObjRef, kids: Vec<ObjRef>) {
        let count = kids.len();
        for &k in &kids {
            if let Some(d) = self.get_mut(k).and_then(Object::as_dict_mut) {
                d.set("Parent", root);
            }
        }
        let d = self.get_mut(root).and_then(Object::as_dict_mut).unwrap();
        d.set(
            "Kids",
            kids.into_iter().map(Object::from).collect::<Vec<_>>(),
        )
        .set("Count", count);
    }

    /// Appends a blank page and returns its reference.
    pub fn add_page(&mut self, size: PageSize) -> ObjRef {
        let n = self.page_count();
        self.insert_page(n, Dict::new().with("MediaBox", size.rect().to_object()))
            .expect("append never fails")
    }
    /// Inserts a page dictionary at `index` (0 = first, `page_count()` = append).
    pub fn insert_page(&mut self, index: usize, mut page: Dict) -> Result<ObjRef> {
        let root = self.flatten_page_tree();
        let mut kids = self.page_refs();
        if index > kids.len() {
            return Err(Error::PageOutOfRange {
                index,
                count: kids.len(),
            });
        }
        page.set("Type", "Page").set("Parent", root);
        let r = self.add(page.into());
        kids.insert(index, r);
        self.set_kids(root, kids);
        Ok(r)
    }
    /// Removes page `index`. Its objects are dropped on save if unreferenced.
    pub fn remove_page(&mut self, index: usize) -> Result<()> {
        let root = self.flatten_page_tree();
        let mut kids = self.page_refs();
        if index >= kids.len() {
            return Err(Error::PageOutOfRange {
                index,
                count: kids.len(),
            });
        }
        let removed = kids.remove(index);
        self.set_kids(root, kids);
        self.objects.remove(&removed.num);
        Ok(())
    }
    /// Keeps only the listed pages, in the given order. Indices may repeat;
    /// a repeated page becomes a copy that shares content and resources
    /// with the original.
    pub fn select_pages(&mut self, order: &[usize]) -> Result<()> {
        let root = self.flatten_page_tree();
        let all = self.page_refs();
        let mut kids = Vec::with_capacity(order.len());
        let mut used: HashSet<u32> = HashSet::new();
        for &i in order {
            let r = *all.get(i).ok_or(Error::PageOutOfRange {
                index: i,
                count: all.len(),
            })?;
            if used.insert(r.num) {
                kids.push(r);
            } else {
                let copy = self.get(r).clone();
                kids.push(self.add(copy));
            }
        }
        let keep: HashSet<u32> = kids.iter().map(|r| r.num).collect();
        for r in all {
            if !keep.contains(&r.num) {
                self.objects.remove(&r.num);
            }
        }
        self.set_kids(root, kids);
        Ok(())
    }
    /// Moves page `from` to position `to`.
    pub fn move_page(&mut self, from: usize, to: usize) -> Result<()> {
        let n = self.page_count();
        if from >= n {
            return Err(Error::PageOutOfRange {
                index: from,
                count: n,
            });
        }
        if to >= n {
            return Err(Error::PageOutOfRange {
                index: to,
                count: n,
            });
        }
        let mut order: Vec<usize> = (0..n).collect();
        let p = order.remove(from);
        order.insert(to, p);
        self.select_pages(&order)
    }
    /// Adds `degrees` (multiple of 90) to the page's rotation.
    pub fn rotate_page(&mut self, index: usize, degrees: i64) -> Result<()> {
        let info = self.page_info(index)?;
        let rot = info.rotation.plus(degrees);
        let d = self
            .get_mut(info.obj)
            .and_then(Object::as_dict_mut)
            .ok_or_else(|| Error::malformed("page is not a dictionary"))?;
        d.set("Rotate", rot.degrees());
        Ok(())
    }
    /// Sets the page's rotation absolutely.
    pub fn set_page_rotation(&mut self, index: usize, rotation: Rotation) -> Result<()> {
        let r = self.page_ref(index)?;
        let d = self
            .get_mut(r)
            .and_then(Object::as_dict_mut)
            .ok_or_else(|| Error::malformed("page is not a dictionary"))?;
        d.set("Rotate", rotation.degrees());
        Ok(())
    }
    /// Sets the page's media box (and removes any crop box).
    pub fn set_media_box(&mut self, index: usize, rect: Rect) -> Result<()> {
        let r = self.page_ref(index)?;
        let d = self
            .get_mut(r)
            .and_then(Object::as_dict_mut)
            .ok_or_else(|| Error::malformed("page is not a dictionary"))?;
        d.set("MediaBox", rect.to_object());
        d.remove("CropBox");
        Ok(())
    }

    /// Deep-copies pages `indices` from `src` into this document, inserting
    /// them at `at` (default: append). Returns the new page references.
    ///
    /// Everything the pages reference (fonts, images, annotations) comes
    /// along; references to pages that were not selected become `null`.
    /// Form fields whose widgets sit on the imported pages are registered
    /// with this document's form (renamed on clashes). Outlines are not
    /// copied.
    pub fn import_pages(
        &mut self,
        src: &Document,
        indices: &[usize],
        at: Option<usize>,
    ) -> Result<Vec<ObjRef>> {
        let src_pages = src.page_refs();
        let mut map: HashMap<u32, ObjRef> = HashMap::new();
        let mut skip: HashSet<u32> = src_pages.iter().map(|r| r.num).collect();
        // Every Pages node is skipped too (they would drag in the whole tree).
        for (r, o) in src.objects() {
            if o.as_dict()
                .and_then(|d| d.get("Type"))
                .and_then(Object::as_name)
                .map(|n| n == "Pages")
                .unwrap_or(false)
            {
                skip.insert(r.num);
            }
        }
        let root = self.flatten_page_tree();
        let mut new_pages = Vec::with_capacity(indices.len());
        let mut work: VecDeque<(ObjRef, ObjRef)> = VecDeque::new();
        for &i in indices {
            let sp = *src_pages.get(i).ok_or(Error::PageOutOfRange {
                index: i,
                count: src_pages.len(),
            })?;
            let np = match map.get(&sp.num) {
                Some(&r) => r,
                None => {
                    let r = self.add(Object::Null);
                    map.insert(sp.num, r);
                    work.push_back((sp, r));
                    r
                }
            };
            new_pages.push(np);
        }
        self.copy_pending(src, &src_pages, root, &mut map, &skip, &mut work);
        self.import_form(src, &new_pages, &mut map, &skip, &mut work);
        self.copy_pending(src, &src_pages, root, &mut map, &skip, &mut work);
        let mut kids = self.page_refs();
        let at = at.unwrap_or(kids.len()).min(kids.len());
        for (k, &p) in new_pages.iter().enumerate() {
            kids.insert(at + k, p);
        }
        self.set_kids(root, kids);
        Ok(new_pages)
    }

    /// Copies every object queued in `work`, remapping references as it goes.
    /// Inherited page attributes are copied down onto page copies.
    fn copy_pending(
        &mut self,
        src: &Document,
        src_pages: &[ObjRef],
        root: ObjRef,
        map: &mut HashMap<u32, ObjRef>,
        skip: &HashSet<u32>,
        work: &mut VecDeque<(ObjRef, ObjRef)>,
    ) {
        while let Some((sref, dref)) = work.pop_front() {
            let mut obj = src.get(sref).clone();
            let is_page = src_pages.iter().any(|p| p.num == sref.num);
            if is_page {
                if let Some(d) = obj.as_dict_mut() {
                    for key in ["Resources", "MediaBox", "CropBox", "Rotate"] {
                        if !d.contains(key) {
                            if let Some(v) = src.page_attr(sref, key) {
                                d.set(key, v.clone());
                            }
                        }
                    }
                    d.remove("Parent");
                    d.remove("B");
                    d.remove("StructParents");
                    d.set("Type", "Page");
                }
            }
            self.remap_refs(&mut obj, src, map, skip, work);
            if is_page {
                if let Some(d) = obj.as_dict_mut() {
                    d.set("Parent", root);
                }
            }
            self.set(dref, obj);
        }
    }

    /// After pages were copied: registers the form fields of any imported
    /// widgets with this document's `/AcroForm`, pruning widgets that sit on
    /// pages that were not imported, and merging default resources.
    fn import_form(
        &mut self,
        src: &Document,
        new_pages: &[ObjRef],
        map: &mut HashMap<u32, ObjRef>,
        skip: &HashSet<u32>,
        work: &mut VecDeque<(ObjRef, ObjRef)>,
    ) {
        let src_af = match src
            .dict_get(src.catalog(), "AcroForm")
            .and_then(Object::as_dict)
        {
            Some(a) => a.clone(),
            None => return,
        };
        let mut widgets: HashSet<u32> = HashSet::new();
        let mut roots: Vec<ObjRef> = Vec::new();
        let mut seen: HashSet<u32> = HashSet::new();
        for &p in new_pages {
            let annots: Vec<ObjRef> = match self
                .get(p)
                .as_dict()
                .and_then(|d| d.get("Annots"))
                .map(|o| self.resolve(o))
            {
                Some(Object::Array(a)) => a.iter().filter_map(Object::as_reference).collect(),
                _ => Vec::new(),
            };
            for a in annots {
                let is_widget = self
                    .get(a)
                    .as_dict()
                    .map(|d| crate::forms::dict_is_widget(self, d))
                    .unwrap_or(false);
                if !is_widget {
                    continue;
                }
                widgets.insert(a.num);
                let mut top = a;
                let mut guard = 0;
                while let Some(parent) = self
                    .get(top)
                    .as_dict()
                    .and_then(|d| d.get("Parent"))
                    .and_then(Object::as_reference)
                {
                    guard += 1;
                    if guard > 64 || self.get(parent).is_null() {
                        break;
                    }
                    top = parent;
                }
                if seen.insert(top.num) {
                    roots.push(top);
                }
            }
        }
        if roots.is_empty() {
            return;
        }
        // Keep the source's field order where possible.
        let src_order: Vec<u32> = match src_af.get("Fields").map(|o| src.resolve(o)) {
            Some(Object::Array(a)) => a
                .iter()
                .filter_map(Object::as_reference)
                .filter_map(|r| map.get(&r.num))
                .map(|r| r.num)
                .collect(),
            _ => Vec::new(),
        };
        roots.sort_by_key(|r| {
            src_order
                .iter()
                .position(|n| *n == r.num)
                .unwrap_or(usize::MAX)
        });
        roots.retain(|&r| self.prune_imported(r, &widgets));
        let mut dr = src_af
            .get("DR")
            .map(|o| src.resolve(o).clone())
            .unwrap_or(Object::Null);
        self.remap_refs(&mut dr, src, map, skip, work);
        let mut da = src_af.get("DA").map(|o| src.resolve(o).clone());
        if let Some(d) = da.as_mut() {
            self.remap_refs(d, src, map, skip, work);
        }
        let need = src_af
            .get("NeedAppearances")
            .and_then(Object::as_bool)
            .unwrap_or(false);
        crate::forms::attach_roots(self, &roots, dr.into_dict(), da, need);
    }

    /// Keeps only the widgets in `keep` under `r`. Returns whether `r` survives.
    fn prune_imported(&mut self, r: ObjRef, keep: &HashSet<u32>) -> bool {
        let (is_widget, kids): (bool, Vec<ObjRef>) = match self.get(r).as_dict() {
            Some(d) => (
                crate::forms::dict_is_widget(self, d),
                match d.get("Kids").map(|o| self.resolve(o)) {
                    Some(Object::Array(a)) => a.iter().filter_map(Object::as_reference).collect(),
                    _ => Vec::new(),
                },
            ),
            None => return false,
        };
        if kids.is_empty() {
            return is_widget && keep.contains(&r.num);
        }
        let survivors: Vec<Object> = kids
            .into_iter()
            .filter(|k| self.prune_imported(*k, keep))
            .map(Object::Reference)
            .collect();
        if survivors.is_empty() {
            return false;
        }
        if let Some(d) = self.get_mut(r).and_then(Object::as_dict_mut) {
            d.set("Kids", Object::Array(survivors));
        }
        true
    }

    fn remap_refs(
        &mut self,
        obj: &mut Object,
        src: &Document,
        map: &mut HashMap<u32, ObjRef>,
        skip: &HashSet<u32>,
        work: &mut VecDeque<(ObjRef, ObjRef)>,
    ) {
        match obj {
            Object::Reference(r) => {
                if let Some(&n) = map.get(&r.num) {
                    *obj = Object::Reference(n);
                } else if skip.contains(&r.num) || src.get(*r).is_null() {
                    *obj = Object::Null;
                } else {
                    let n = self.add(Object::Null);
                    map.insert(r.num, n);
                    work.push_back((*r, n));
                    *obj = Object::Reference(n);
                }
            }
            Object::Array(a) => {
                for o in a {
                    self.remap_refs(o, src, map, skip, work);
                }
            }
            Object::Dict(d) => {
                for v in d.0.values_mut() {
                    self.remap_refs(v, src, map, skip, work);
                }
            }
            Object::Stream(s) => {
                for v in s.dict.0.values_mut() {
                    self.remap_refs(v, src, map, skip, work);
                }
            }
            _ => {}
        }
    }

    // -- drawing resources --------------------------------------------------------

    /// Registers a font for use with [`Document::draw`]. The font objects are
    /// generated when saving, so glyph subsetting reflects all text drawn.
    pub fn add_font(&mut self, font: Font) -> ObjRef {
        let r = self.add(Object::Null);
        self.fonts.push((r, font));
        r
    }
    /// Registers one of the standard 14 fonts.
    pub fn add_standard_font(&mut self, font: StandardFont) -> ObjRef {
        self.add_font(Font::standard(font))
    }
    /// Access to a registered font (for encoding and measuring text).
    pub fn font_mut(&mut self, r: ObjRef) -> Option<&mut Font> {
        self.fonts
            .iter_mut()
            .find(|(fr, _)| *fr == r)
            .map(|(_, f)| f)
    }
    /// Read access to a registered font.
    pub fn font(&self, r: ObjRef) -> Option<&Font> {
        self.fonts.iter().find(|(fr, _)| *fr == r).map(|(_, f)| f)
    }
    /// Adds an image XObject (with soft mask if it has alpha).
    pub fn add_image(&mut self, image: &Image, compression_level: u8) -> ObjRef {
        let (mut img, mask) = image.to_streams(compression_level);
        if let Some(m) = mask {
            let mr = self.add(m.into());
            img.dict.set("SMask", mr);
        }
        self.add(img.into())
    }
    /// Adds an ExtGState with constant fill and stroke alpha.
    pub fn add_opacity_state(&mut self, alpha: f64) -> ObjRef {
        self.add(
            Dict::new()
                .with("Type", "ExtGState")
                .with("ca", alpha)
                .with("CA", alpha)
                .into(),
        )
    }

    /// Makes sure page `index` has its own `/Resources` dictionary and adds
    /// `resource` under `category` (`Font`, `XObject`, `ExtGState`).
    /// Returns the generated resource name (e.g. `F3`).
    pub fn add_page_resource(
        &mut self,
        index: usize,
        category: &str,
        resource: ObjRef,
    ) -> Result<String> {
        let page = self.page_ref(index)?;
        let inherited = self
            .page_attr(page, "Resources")
            .cloned()
            .unwrap_or_else(|| Dict::new().into());
        let mut res = inherited.into_dict().unwrap_or_default();
        // Resolve a sub-dictionary reference so we can edit a private copy.
        let sub = res
            .get(category)
            .map(|o| self.resolve(o).clone())
            .and_then(Object::into_dict)
            .unwrap_or_default();
        let prefix = match category {
            "Font" => "F",
            "XObject" => "X",
            "ExtGState" => "GS",
            _ => "R",
        };
        let mut i = 1;
        let name = loop {
            let n = format!("{prefix}{i}");
            if !sub.contains(&n) {
                break n;
            }
            i += 1;
        };
        let mut sub = sub;
        sub.set(&name, resource);
        res.set(category, sub);
        let d = self
            .get_mut(page)
            .and_then(Object::as_dict_mut)
            .ok_or_else(|| Error::malformed("page is not a dictionary"))?;
        d.set("Resources", res);
        Ok(name)
    }

    /// Appends `content` to page `index`, drawing on top of the existing
    /// content. The existing content is wrapped in `q … Q` so its graphics
    /// state cannot leak into the new operators.
    pub fn draw(&mut self, index: usize, content: &[u8]) -> Result<()> {
        self.add_content(index, content, false)
    }
    /// Prepends `content` so it is painted underneath the existing content.
    pub fn draw_under(&mut self, index: usize, content: &[u8]) -> Result<()> {
        self.add_content(index, content, true)
    }

    fn add_content(&mut self, index: usize, content: &[u8], under: bool) -> Result<()> {
        let page = self.page_ref(index)?;
        let existing: Vec<Object> = match self.get(page).as_dict().and_then(|d| d.get("Contents")) {
            Some(Object::Reference(r)) => match self.get(*r) {
                Object::Array(a) => a.clone(),
                _ => vec![Object::Reference(*r)],
            },
            Some(Object::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        let new_stream = |doc: &mut Document, bytes: Vec<u8>| -> Object {
            let level = 6;
            let s = Stream::new(
                Dict::new().with("Filter", "FlateDecode"),
                filters::flate_encode(&bytes, level),
            );
            doc.add(s.into()).into()
        };
        let mut arr = Vec::with_capacity(existing.len() + 2);
        if under {
            let mut c = content.to_vec();
            c.push(b'\n');
            arr.push(new_stream(self, c));
            arr.extend(existing);
        } else {
            arr.push(new_stream(self, b"q\n".to_vec()));
            arr.extend(existing);
            let mut c = b"\nQ\n".to_vec();
            c.extend_from_slice(content);
            c.push(b'\n');
            arr.push(new_stream(self, c));
        }
        let d = self
            .get_mut(page)
            .and_then(Object::as_dict_mut)
            .ok_or_else(|| Error::malformed("page is not a dictionary"))?;
        d.set("Contents", arr);
        Ok(())
    }

    /// Wraps the page's existing content between `prefix` and `suffix`.
    ///
    /// Used to apply a transform to everything already on the page (see
    /// [`ops::resize_pages`](crate::ops::resize_pages)). The prefix and suffix
    /// become separate content streams, so the original streams are untouched.
    pub fn wrap_content(&mut self, index: usize, prefix: &[u8], suffix: &[u8]) -> Result<()> {
        let page = self.page_ref(index)?;
        let existing: Vec<Object> = match self.get(page).as_dict().and_then(|d| d.get("Contents")) {
            Some(Object::Reference(r)) => match self.get(*r) {
                Object::Array(a) => a.clone(),
                _ => vec![Object::Reference(*r)],
            },
            Some(Object::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        let new_stream = |doc: &mut Document, bytes: &[u8]| -> Object {
            let s = Stream::new(
                Dict::new().with("Filter", "FlateDecode"),
                filters::flate_encode(bytes, 6),
            );
            doc.add(s.into()).into()
        };
        let mut arr = Vec::with_capacity(existing.len() + 2);
        arr.push(new_stream(self, prefix));
        arr.extend(existing);
        arr.push(new_stream(self, suffix));
        let d = self
            .get_mut(page)
            .and_then(Object::as_dict_mut)
            .ok_or_else(|| Error::malformed("page is not a dictionary"))?;
        d.set("Contents", arr);
        Ok(())
    }

    /// Concatenated, decoded content stream of page `index`.
    pub fn page_content(&self, index: usize) -> Result<Vec<u8>> {
        let page = self.page_ref(index)?;
        let mut out = Vec::new();
        let contents = self
            .get(page)
            .as_dict()
            .and_then(|d| d.get("Contents"))
            .map(|o| self.resolve(o));
        let parts: Vec<&Object> = match contents {
            Some(Object::Array(a)) => a.iter().map(|o| self.resolve(o)).collect(),
            Some(o @ Object::Stream(_)) => vec![o],
            _ => Vec::new(),
        };
        for p in parts {
            if let Some(s) = p.as_stream() {
                out.extend(self.stream_data(s)?);
                out.push(b'\n');
            }
        }
        Ok(out)
    }

    // -- saving ---------------------------------------------------------------------

    /// Serialises the document. Fonts registered with [`Document::add_font`]
    /// are finalised (subset) at this point.
    pub fn save(&mut self, opts: &SaveOptions) -> Result<Vec<u8>> {
        // Finalise fonts into real objects.
        let level = opts.compression_level.clamp(1, 10);
        let fonts = std::mem::take(&mut self.fonts);
        for (r, font) in &fonts {
            let mut alloc = |o: Object| self.add(o);
            let dict = font.build(level, &mut alloc);
            self.set(*r, dict.into());
        }
        self.fonts = fonts;
        if opts.strip_metadata {
            self.strip_metadata();
        }
        match &opts.producer {
            Some(p) if p.is_empty() && self.info().is_some() => {
                self.info_mut().remove("Producer");
            }
            Some(p) if p.is_empty() => {}
            Some(p) => {
                self.info_mut().set("Producer", Object::text(p));
            }
            None => {}
        }
        writer::write(self, opts)
    }

    /// Ensures the trailer has a two-element `/ID`, returning the first id.
    pub(crate) fn ensure_id(&mut self) -> Result<Vec<u8>> {
        let id0 = self
            .trailer
            .get("ID")
            .and_then(Object::as_array)
            .and_then(|a| a.first())
            .and_then(Object::as_string)
            .map(|s| s.bytes.clone())
            .filter(|b| !b.is_empty());
        let id0 = match id0 {
            Some(b) => b,
            None => {
                let mut b = vec![0u8; 16];
                getrandom::getrandom(&mut b)
                    .map_err(|e| Error::Malformed(format!("random source unavailable: {e}")))?;
                b
            }
        };
        let mut id1 = vec![0u8; 16];
        getrandom::getrandom(&mut id1)
            .map_err(|e| Error::Malformed(format!("random source unavailable: {e}")))?;
        self.trailer.set(
            "ID",
            Object::Array(vec![
                Object::String(PdfString::hex(id0.clone())),
                Object::String(PdfString::hex(id1)),
            ]),
        );
        Ok(id0)
    }

    pub(crate) fn set_version(&mut self, v: (u8, u8)) {
        self.version = v;
    }
}

impl Name {
    /// Helper used by the writer to test membership in a list of names.
    pub(crate) fn is_any(&self, names: &[&str]) -> bool {
        names.iter().any(|n| self == n)
    }
}
