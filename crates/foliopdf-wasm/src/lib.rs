//! WebAssembly bindings for [foliopdf](https://github.com/keithadler/foliopdf).
//!
//! The JavaScript API mirrors the Rust one: a `PdfDocument` class for
//! editing, `runBatch` for presets, and a `PresetStore` for keeping export
//! configurations. All byte arguments are `Uint8Array`s; all option objects
//! are plain JSON (see the TypeScript definitions shipped in the package).

use foliopdf::annot::{self, Annotation, AnnotationMeta, FlattenOptions};
use foliopdf::batch::{self, Asset, Input, Preset, PresetStore as CoreStore};
use foliopdf::document::Metadata;
use foliopdf::forms::{self, FieldValue, NewField};
use foliopdf::geometry::{Point, Rect};
use foliopdf::ops::{self, FitMode, ImageStamp, PageNumbers, TextStamp};
use foliopdf::redact::{self, RedactOptions};
use foliopdf::text::{self, SearchOptions};
use foliopdf::{Document, LoadOptions, PageSize, SaveOptions};
use js_sys::{Array, Object, Reflect, Uint8Array};
use wasm_bindgen::prelude::*;

#[wasm_bindgen(typescript_custom_section)]
const TS_TYPES: &'static str = r#"
/** Rectangle in PDF points, origin bottom-left. */
export interface Rect { x0: number; y0: number; x1: number; y1: number }

/** Geometry of one page. */
export interface PageInfo {
  index: number;
  mediaBox: Rect;
  cropBox: Rect | null;
  /** Clockwise rotation: "0" | "90" | "180" | "270". */
  rotation: "0" | "90" | "180" | "270";
}

export interface Metadata {
  title?: string | null; author?: string | null; subject?: string | null;
  keywords?: string | null; creator?: string | null; producer?: string | null;
}

export interface Permissions {
  print?: boolean; modify?: boolean; copy?: boolean; annotate?: boolean;
  fillForms?: boolean; accessibility?: boolean; assemble?: boolean; printHighQuality?: boolean;
}

export interface EncryptionOptions {
  /** Password needed to open. Empty string = anyone can open. */
  userPassword?: string;
  /** Password that unlocks all permissions. */
  ownerPassword?: string;
  /** "aes-256" (default) | "aes-128" | "rc4-128" */
  method?: "aes-256" | "aes-128" | "rc4-128";
  permissions?: Permissions;
  encryptMetadata?: boolean;
}

export interface SaveOptions {
  /** Recompress streams and pack objects. Default true. */
  compress?: boolean;
  /** 1–10. Default 6. */
  compressionLevel?: number;
  objectStreams?: boolean;
  stripMetadata?: boolean;
  encryption?: EncryptionOptions | null;
  producer?: string | null;
}

export type Position =
  | "top-left" | "top-center" | "top-right"
  | "center-left" | "center" | "center-right"
  | "bottom-left" | "bottom-center" | "bottom-right";

export interface TextStampOptions {
  /** `{page}` and `{pages}` are substituted. */
  text: string;
  /** Standard font: Helvetica, Helvetica-Bold, Times-Roman, Courier, ... (Arial aliases accepted). */
  font?: string;
  size?: number;
  /** RGB in 0..1 */
  color?: [number, number, number];
  opacity?: number;
  position?: Position;
  /** Counter-clockwise degrees. */
  rotation?: number;
  margin?: number;
  /** Paint beneath existing content. */
  under?: boolean;
}

export interface ImageStampOptions {
  width?: number | null; height?: number | null; opacity?: number;
  position?: Position; margin?: number; under?: boolean;
}

export interface PageNumberOptions {
  format?: string; position?: Position; font?: string; size?: number;
  margin?: number; color?: [number, number, number]; startAt?: number;
}

export type Step =
  | { op: "select-pages"; pages: string }
  | { op: "delete-pages"; pages: string }
  | { op: "rotate"; pages?: string; degrees: number }
  | ({ op: "stamp-text"; pages?: string } & TextStampOptions)
  | ({ op: "stamp-image"; pages?: string; asset: string } & ImageStampOptions)
  | ({ op: "page-numbers"; pages?: string } & PageNumberOptions)
  | ({ op: "metadata" } & Metadata)
  | { op: "strip-metadata" }
  | { op: "split"; every?: number; ranges?: string[] };

export interface OutputOptions {
  compress?: boolean; compressionLevel?: number; objectStreams?: boolean;
  encryption?: EncryptionOptions | null;
  /** Template with {stem} {index} {total} {n}. Default "{stem}.pdf". */
  filename?: string;
}

/** A storable export configuration. */
export interface Preset {
  schema?: number;
  name: string;
  description?: string | null;
  /** "each" (default) processes inputs separately; "merge" combines them first. */
  mode?: "each" | "merge";
  steps?: Step[];
  output?: OutputOptions;
}

/**
 * Geometry for annotations and form fields is in *screen* coordinates: points,
 * measured from the top-left corner of the page as it is displayed (after
 * rotation), x to the right and y downwards. Multiply by your zoom factor to
 * get CSS pixels.
 */
export interface Point { x: number; y: number }

export type Annotation =
  | { kind: "highlight"; quads: Rect[]; color?: [number, number, number]; opacity?: number }
  | { kind: "underline"; quads: Rect[]; color?: [number, number, number]; opacity?: number }
  | { kind: "strike-out"; quads: Rect[]; color?: [number, number, number]; opacity?: number }
  | { kind: "square"; rect: Rect; stroke?: [number, number, number] | null; fill?: [number, number, number] | null; width?: number; opacity?: number }
  | { kind: "circle"; rect: Rect; stroke?: [number, number, number] | null; fill?: [number, number, number] | null; width?: number; opacity?: number }
  | { kind: "line"; from: Point; to: Point; color?: [number, number, number]; width?: number; opacity?: number }
  | { kind: "ink"; paths: Point[][]; color?: [number, number, number]; width?: number; opacity?: number }
  | { kind: "free-text"; rect: Rect; text: string; font?: string; size?: number; color?: [number, number, number]; align?: "left" | "center" | "right"; background?: [number, number, number] | null; border?: [number, number, number] | null; opacity?: number }
  | { kind: "note"; at: Point; icon?: "comment" | "note" | "key" | "help" | "paragraph" | "insert"; color?: [number, number, number] }
  | { kind: "link"; rect: Rect; uri?: string; page?: number };

export interface AnnotationMeta {
  author?: string; contents?: string; subject?: string;
  /** PDF date, e.g. "D:20260904120000Z". */
  modified?: string;
  /** Default true. */
  print?: boolean;
}

export interface AnnotInfo {
  index: number; object: number; subtype: string; rect: Rect;
  contents: string | null; author: string | null; hidden: boolean;
  /** For form widgets: the field name. */
  field: string | null;
  hasAppearance: boolean;
}

export interface FlattenOptions {
  /** Include form field widgets. Default true. */
  widgets?: boolean;
  /** Only these object numbers (from AnnotInfo.object / addAnnotation). */
  objects?: number[];
  /** Only these subtypes, e.g. ["Highlight", "Ink"]. */
  subtypes?: string[];
}

export type FieldKind = "text" | "checkbox" | "radio" | "combo" | "list" | "button" | "signature" | "unknown";
export interface FieldOption { value: string; label: string }
export interface Widget { page: number | null; rect: Rect; onState: string | null; object: number }
export interface Field {
  name: string; kind: FieldKind; value: string | null; values: string[]; options: FieldOption[];
  page: number | null; rect: Rect | null; widgets: Widget[];
  readOnly: boolean; required: boolean; multiline: boolean; password: boolean; maxLen: number | null; object: number;
}
/** A string (text, export value, radio choice), a boolean (check box) or a string[] (multi-select list). */
export type FieldValue = string | boolean | string[];

/** A field to create with `addField` (screen coordinates). */
export interface NewField {
  name: string;
  /** Default "text". */
  kind?: "text" | "checkbox" | "radio" | "combo" | "list";
  rect: Rect;
  value?: string;
  /** Choices for radio groups, drop-downs and lists. */
  options?: string[];
  /** One rectangle per radio button; otherwise buttons are laid out inside `rect`. */
  widgets?: Rect[];
  multiline?: boolean; required?: boolean; readOnly?: boolean; password?: boolean;
  maxLen?: number; comb?: boolean;
  /** 0 (default) fits the text to the box. */
  fontSize?: number;
  color?: [number, number, number];
  align?: "left" | "center" | "right";
  background?: [number, number, number] | null;
  border?: [number, number, number] | null;
  borderWidth?: number;
  tooltip?: string;
}

/** A word or line of text with its bounds (screen coordinates). */
export interface TextSpan { text: string; rect: Rect; line: number }
/** A search hit: one rectangle per line it spans (screen coordinates). */
export interface SearchMatch { text: string; rects: Rect[]; line: number }
export interface SearchOptions { caseInsensitive?: boolean; wholeWord?: boolean }

export interface RedactOptions {
  /** Colour of the box painted over each area; null paints nothing. Default black. */
  fill?: [number, number, number] | null;
  /** Remove annotations overlapping an area. Default true. */
  removeAnnotations?: boolean;
  /** Grow each area by this many points. Default 0.5. */
  margin?: number;
}
export interface RedactReport {
  glyphsRemoved: number; imagesRemoved: number; imagesEdited: number; pathsRemoved: number;
  annotationsRemoved: number; formsEdited: number; warnings: string[];
  /** Only for redactText: number of matches found. */
  matches?: number;
}

export interface BatchInput { name: string; data: Uint8Array; password?: string }
export interface BatchAsset { name: string; data: Uint8Array }
export interface BatchOutput { name: string; data: Uint8Array; pages: number; bytes: number; sources: string[] }
export interface BatchResult { outputs: BatchOutput[]; warnings: string[] }
"#;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(typescript_type = "SaveOptions | undefined")]
    pub type SaveOptionsJs;
    #[wasm_bindgen(typescript_type = "TextStampOptions")]
    pub type TextStampJs;
    #[wasm_bindgen(typescript_type = "ImageStampOptions | undefined")]
    pub type ImageStampJs;
    #[wasm_bindgen(typescript_type = "PageNumberOptions | undefined")]
    pub type PageNumbersJs;
    #[wasm_bindgen(typescript_type = "ResizeOptions")]
    pub type ResizeJs;
    #[wasm_bindgen(typescript_type = "Annotation")]
    pub type AnnotationJs;
    #[wasm_bindgen(typescript_type = "AnnotationMeta | undefined")]
    pub type AnnotationMetaJs;
    #[wasm_bindgen(typescript_type = "AnnotInfo[]")]
    pub type AnnotInfoArrayJs;
    #[wasm_bindgen(typescript_type = "FlattenOptions | undefined")]
    pub type FlattenOptionsJs;
    #[wasm_bindgen(typescript_type = "Rect")]
    pub type RectJs;
    #[wasm_bindgen(typescript_type = "Field[]")]
    pub type FieldArrayJs;
    #[wasm_bindgen(typescript_type = "FieldValue")]
    pub type FieldValueJs;
    #[wasm_bindgen(typescript_type = "Record<string, FieldValue>")]
    pub type FieldValuesJs;
    #[wasm_bindgen(typescript_type = "NewField")]
    pub type NewFieldJs;
    #[wasm_bindgen(typescript_type = "TextSpan[]")]
    pub type TextSpanArrayJs;
    #[wasm_bindgen(typescript_type = "SearchMatch[]")]
    pub type SearchMatchArrayJs;
    #[wasm_bindgen(typescript_type = "SearchOptions | undefined")]
    pub type SearchOptionsJs;
    #[wasm_bindgen(typescript_type = "Rect[]")]
    pub type RectArrayJs;
    #[wasm_bindgen(typescript_type = "RedactOptions | undefined")]
    pub type RedactOptionsJs;
    #[wasm_bindgen(typescript_type = "RedactReport")]
    pub type RedactReportJs;
    #[wasm_bindgen(typescript_type = "Metadata")]
    pub type MetadataJs;
    #[wasm_bindgen(typescript_type = "PageInfo[]")]
    pub type PageInfoArrayJs;
    #[wasm_bindgen(typescript_type = "Preset")]
    pub type PresetJs;
    #[wasm_bindgen(typescript_type = "Preset[]")]
    pub type PresetArrayJs;
    #[wasm_bindgen(typescript_type = "BatchInput[]")]
    pub type BatchInputsJs;
    #[wasm_bindgen(typescript_type = "BatchAsset[] | undefined")]
    pub type BatchAssetsJs;
    #[wasm_bindgen(typescript_type = "BatchResult")]
    pub type BatchResultJs;
}

fn err(e: foliopdf::Error) -> JsError {
    JsError::new(&e.to_string())
}

fn from_js<T: serde::de::DeserializeOwned + Default>(v: JsValue) -> Result<T, JsError> {
    if v.is_undefined() || v.is_null() {
        return Ok(T::default());
    }
    serde_wasm_bindgen::from_value(v).map_err(|e| JsError::new(&format!("invalid options: {e}")))
}

fn to_js<T: serde::Serialize>(v: &T) -> Result<JsValue, JsError> {
    let ser = serde_wasm_bindgen::Serializer::json_compatible();
    v.serialize(&ser).map_err(|e| JsError::new(&e.to_string()))
}

/// Library version.
#[wasm_bindgen]
pub fn version() -> String {
    foliopdf::VERSION.to_string()
}

/// Expands a page range expression (1-based, e.g. `"1-3,odd,last"`) into
/// 0-based indices.
#[wasm_bindgen(js_name = parsePageRanges)]
pub fn parse_page_ranges(spec: &str, page_count: usize) -> Result<Vec<u32>, JsError> {
    Ok(ops::parse_page_ranges(spec, page_count)
        .map_err(err)?
        .into_iter()
        .map(|i| i as u32)
        .collect())
}

/// An editable PDF document.
#[wasm_bindgen]
pub struct PdfDocument {
    inner: Document,
}

#[wasm_bindgen]
impl PdfDocument {
    /// A new, empty document.
    #[wasm_bindgen(constructor)]
    pub fn new() -> PdfDocument {
        PdfDocument {
            inner: Document::new(),
        }
    }

    /// Opens a PDF. Encrypted files that open with the empty user password
    /// are opened transparently; otherwise use `loadWithPassword`.
    pub fn load(bytes: &[u8]) -> Result<PdfDocument, JsError> {
        Ok(PdfDocument {
            inner: Document::load(bytes).map_err(err)?,
        })
    }

    /// Opens an encrypted PDF with a user or owner password.
    #[wasm_bindgen(js_name = loadWithPassword)]
    pub fn load_with_password(bytes: &[u8], password: &str) -> Result<PdfDocument, JsError> {
        Ok(PdfDocument {
            inner: Document::load_with(bytes, &LoadOptions::with_password(password))
                .map_err(err)?,
        })
    }

    /// Number of pages.
    #[wasm_bindgen(js_name = pageCount)]
    pub fn page_count(&self) -> usize {
        self.inner.page_count()
    }

    /// Geometry of every page.
    pub fn pages(&self) -> Result<PageInfoArrayJs, JsError> {
        Ok(to_js(&self.inner.pages())?.unchecked_into())
    }

    /// Whether the input was encrypted.
    #[wasm_bindgen(js_name = wasEncrypted)]
    pub fn was_encrypted(&self) -> bool {
        self.inner.was_encrypted()
    }

    /// Description of the input's encryption (e.g. `"AES-256"`), or null.
    #[wasm_bindgen(js_name = encryptionDescription)]
    pub fn encryption_description(&self) -> Option<String> {
        self.inner.encryption_description().map(str::to_owned)
    }

    /// Whether the file was damaged and the cross-reference table rebuilt.
    #[wasm_bindgen(js_name = wasReconstructed)]
    pub fn was_reconstructed(&self) -> bool {
        self.inner.was_reconstructed()
    }

    /// Document information fields.
    pub fn metadata(&self) -> Result<MetadataJs, JsError> {
        Ok(to_js(&self.inner.metadata())?.unchecked_into())
    }

    /// Sets information fields. Omitted fields are untouched; empty strings
    /// remove a field.
    #[wasm_bindgen(js_name = setMetadata)]
    pub fn set_metadata(&mut self, metadata: MetadataJs) -> Result<(), JsError> {
        let m: Metadata = from_js(metadata.into())?;
        self.inner.set_metadata(&m);
        Ok(())
    }

    /// Removes XMP metadata, the info dictionary and thumbnails.
    #[wasm_bindgen(js_name = stripMetadata)]
    pub fn strip_metadata(&mut self) {
        self.inner.strip_metadata();
    }

    /// Appends a blank page of `width` × `height` points.
    #[wasm_bindgen(js_name = addPage)]
    pub fn add_page(&mut self, width: f64, height: f64) {
        self.inner.add_page(PageSize::new(width, height));
    }

    /// Removes page `index` (0-based).
    #[wasm_bindgen(js_name = removePage)]
    pub fn remove_page(&mut self, index: usize) -> Result<(), JsError> {
        self.inner.remove_page(index).map_err(err)
    }

    /// Moves a page.
    #[wasm_bindgen(js_name = movePage)]
    pub fn move_page(&mut self, from: usize, to: usize) -> Result<(), JsError> {
        self.inner.move_page(from, to).map_err(err)
    }

    /// Keeps only the pages in `pages` (a 1-based range expression), in
    /// that order.
    #[wasm_bindgen(js_name = selectPages)]
    pub fn select_pages(&mut self, pages: &str) -> Result<(), JsError> {
        let idx = ops::parse_page_ranges(pages, self.inner.page_count()).map_err(err)?;
        self.inner.select_pages(&idx).map_err(err)
    }

    /// Deletes the pages in `pages`.
    #[wasm_bindgen(js_name = deletePages)]
    pub fn delete_pages(&mut self, pages: &str) -> Result<(), JsError> {
        let idx = ops::parse_page_ranges(pages, self.inner.page_count()).map_err(err)?;
        ops::delete_pages(&mut self.inner, &idx).map_err(err)
    }

    /// Rotates pages by a multiple of 90 degrees. `pages` may be null for all.
    #[wasm_bindgen(js_name = rotatePages)]
    pub fn rotate_pages(&mut self, pages: Option<String>, degrees: i32) -> Result<(), JsError> {
        let idx = self.range(pages)?;
        ops::rotate_pages(&mut self.inner, &idx, degrees as i64).map_err(err)
    }

    /// Copies pages from another document. `pages` is a range expression
    /// over `other` (null = all); `at` is the insertion index (null = append).
    #[wasm_bindgen(js_name = importPages)]
    pub fn import_pages(
        &mut self,
        other: &PdfDocument,
        pages: Option<String>,
        at: Option<usize>,
    ) -> Result<(), JsError> {
        let idx = match pages {
            Some(p) => ops::parse_page_ranges(&p, other.inner.page_count()).map_err(err)?,
            None => (0..other.inner.page_count()).collect(),
        };
        self.inner
            .import_pages(&other.inner, &idx, at)
            .map_err(err)?;
        Ok(())
    }

    /// Draws a text stamp or watermark on `pages` (null = all).
    #[wasm_bindgen(js_name = stampText)]
    pub fn stamp_text(
        &mut self,
        pages: Option<String>,
        options: TextStampJs,
    ) -> Result<(), JsError> {
        let stamp: TextStamp = from_js(options.into())?;
        let idx = self.range(pages)?;
        ops::stamp_text(&mut self.inner, &idx, &stamp).map_err(err)
    }

    /// Draws a JPEG or PNG image on `pages` (null = all).
    #[wasm_bindgen(js_name = stampImage)]
    pub fn stamp_image(
        &mut self,
        pages: Option<String>,
        image: &[u8],
        options: ImageStampJs,
    ) -> Result<(), JsError> {
        let stamp: ImageStamp = from_js(options.into())?;
        let idx = self.range(pages)?;
        ops::stamp_image(&mut self.inner, &idx, image, &stamp).map_err(err)
    }

    /// Adds page numbers to `pages` (null = all).
    #[wasm_bindgen(js_name = addPageNumbers)]
    pub fn add_page_numbers(
        &mut self,
        pages: Option<String>,
        options: PageNumbersJs,
    ) -> Result<(), JsError> {
        let settings: PageNumbers = from_js(options.into())?;
        let idx = self.range(pages)?;
        ops::add_page_numbers(&mut self.inner, &idx, &settings).map_err(err)
    }

    /// Resizes pages to a named size or explicit dimensions, scaling the
    /// content to match. `pages` may be null for all.
    #[wasm_bindgen(js_name = resizePages)]
    pub fn resize_pages(
        &mut self,
        pages: Option<String>,
        options: ResizeJs,
    ) -> Result<(), JsError> {
        #[derive(serde::Deserialize, Default)]
        #[serde(default, rename_all = "camelCase")]
        struct Opts {
            size: Option<String>,
            width: Option<f64>,
            height: Option<f64>,
            mode: FitMode,
        }
        let o: Opts = from_js(options.into())?;
        let target = batch::resolve_size(o.size.as_deref(), o.width, o.height).map_err(err)?;
        let idx = self.range(pages)?;
        ops::resize_pages(&mut self.inner, &idx, target, o.mode).map_err(err)
    }

    /// Scales pages and their content by `factor` (0.5 halves them).
    #[wasm_bindgen(js_name = scalePages)]
    pub fn scale_pages(&mut self, pages: Option<String>, factor: f64) -> Result<(), JsError> {
        let idx = self.range(pages)?;
        ops::scale_pages(&mut self.inner, &idx, factor).map_err(err)
    }

    /// Inserts `count` blank pages of `width` × `height` points before
    /// 0-based index `at` (pass the page count to append).
    #[wasm_bindgen(js_name = insertBlankPages)]
    pub fn insert_blank_pages(
        &mut self,
        at: usize,
        count: usize,
        width: f64,
        height: f64,
    ) -> Result<(), JsError> {
        let at = at.min(self.inner.page_count());
        ops::insert_blank_pages(&mut self.inner, at, count, PageSize::new(width, height))
            .map_err(err)?;
        Ok(())
    }

    /// Reverses the page order.
    #[wasm_bindgen(js_name = reversePages)]
    pub fn reverse_pages(&mut self) -> Result<(), JsError> {
        ops::reverse_pages(&mut self.inner).map_err(err)
    }

    /// Whether the file was opened with owner (full) rights. True for
    /// unencrypted files.
    #[wasm_bindgen(js_name = hasOwnerAccess)]
    pub fn has_owner_access(&self) -> bool {
        self.inner.has_owner_access()
    }

    // -- annotations ------------------------------------------------------------

    /// Lists the annotations on a page (screen coordinates).
    pub fn annotations(&self, page: usize) -> Result<AnnotInfoArrayJs, JsError> {
        let h = self.display_height(page)?;
        let mut list = annot::list_annotations(&self.inner, page).map_err(err)?;
        for a in &mut list {
            a.rect = flip_rect(&a.rect, h);
        }
        Ok(to_js(&list)?.unchecked_into())
    }

    /// Adds an annotation (see the `Annotation` type; screen coordinates).
    /// Returns its object number, usable with `flattenAnnotations`.
    #[wasm_bindgen(js_name = addAnnotation)]
    pub fn add_annotation(
        &mut self,
        page: usize,
        annotation: AnnotationJs,
        meta: AnnotationMetaJs,
    ) -> Result<u32, JsError> {
        let mut a: Annotation = serde_wasm_bindgen::from_value(annotation.into())
            .map_err(|e| JsError::new(&format!("invalid annotation: {e}")))?;
        let m: AnnotationMeta = from_js(meta.into())?;
        let h = self.display_height(page)?;
        a.map_points(|p| Point::new(p.x, h - p.y));
        Ok(annot::add_annotation(&mut self.inner, page, &a, &m)
            .map_err(err)?
            .num)
    }

    /// Places a JPEG or PNG (a signature, a logo) filling `rect` on a page
    /// as a stamp annotation. Returns its object number.
    #[wasm_bindgen(js_name = addImageAnnotation)]
    pub fn add_image_annotation(
        &mut self,
        page: usize,
        rect: RectJs,
        image: &[u8],
        opacity: Option<f64>,
        meta: AnnotationMetaJs,
    ) -> Result<u32, JsError> {
        let r: Rect = serde_wasm_bindgen::from_value(rect.into())
            .map_err(|e| JsError::new(&format!("invalid rect: {e}")))?;
        let m: AnnotationMeta = from_js(meta.into())?;
        let h = self.display_height(page)?;
        Ok(annot::add_image_annotation(
            &mut self.inner,
            page,
            flip_rect(&r, h),
            image,
            opacity.unwrap_or(1.0),
            &m,
        )
        .map_err(err)?
        .num)
    }

    /// Removes the annotation at `index` of a page's list.
    #[wasm_bindgen(js_name = removeAnnotation)]
    pub fn remove_annotation(&mut self, page: usize, index: usize) -> Result<(), JsError> {
        annot::remove_annotation(&mut self.inner, page, index).map_err(err)
    }

    /// Removes annotations matching `options` on `pages` (null = all).
    /// Returns how many were removed.
    #[wasm_bindgen(js_name = removeAnnotations)]
    pub fn remove_annotations(
        &mut self,
        pages: Option<String>,
        options: FlattenOptionsJs,
    ) -> Result<usize, JsError> {
        let o: FlattenOptions = from_js(options.into())?;
        let idx = self.range(pages)?;
        annot::remove_annotations(&mut self.inner, &idx, &o).map_err(err)
    }

    /// Paints annotations into the page content and removes them, so they
    /// become a permanent part of the page. Returns how many were flattened.
    #[wasm_bindgen(js_name = flattenAnnotations)]
    pub fn flatten_annotations(
        &mut self,
        pages: Option<String>,
        options: FlattenOptionsJs,
    ) -> Result<usize, JsError> {
        let o: FlattenOptions = from_js(options.into())?;
        let idx = self.range(pages)?;
        annot::flatten_annotations(&mut self.inner, &idx, &o).map_err(err)
    }

    // -- forms ------------------------------------------------------------------

    /// Lists the form fields (screen coordinates).
    pub fn fields(&self) -> Result<FieldArrayJs, JsError> {
        let mut list = forms::list_fields(&self.inner);
        for f in &mut list {
            for w in &mut f.widgets {
                if let Some(p) = w.page {
                    let h = self.display_height(p)?;
                    w.rect = flip_rect(&w.rect, h);
                }
            }
            f.rect = f.widgets.first().map(|w| w.rect);
        }
        Ok(to_js(&list)?.unchecked_into())
    }

    /// Whether the document has form fields.
    #[wasm_bindgen(js_name = hasFields)]
    pub fn has_fields(&self) -> bool {
        forms::has_fields(&self.inner)
    }

    /// Sets a field's value and regenerates its appearance. Accepts a
    /// string, a boolean (check boxes) or a string array (multi-select lists).
    #[wasm_bindgen(js_name = setField)]
    pub fn set_field(&mut self, name: &str, value: FieldValueJs) -> Result<(), JsError> {
        let v: FieldValue = serde_wasm_bindgen::from_value(value.into())
            .map_err(|e| JsError::new(&format!("invalid value: {e}")))?;
        forms::set_field(&mut self.inner, name, &v).map_err(err)
    }

    /// Sets several fields from a `{ name: value }` object. Returns the
    /// names that do not exist.
    #[wasm_bindgen(js_name = setFields)]
    pub fn set_fields(&mut self, values: FieldValuesJs) -> Result<Vec<String>, JsError> {
        let map: std::collections::BTreeMap<String, FieldValue> =
            serde_wasm_bindgen::from_value(values.into())
                .map_err(|e| JsError::new(&format!("invalid values: {e}")))?;
        let list: Vec<(String, FieldValue)> = map.into_iter().collect();
        forms::set_fields(&mut self.inner, &list).map_err(err)
    }

    /// Paints every field into its page and removes the form. Returns how
    /// many widgets were flattened.
    #[wasm_bindgen(js_name = flattenFields)]
    pub fn flatten_fields(&mut self) -> Result<usize, JsError> {
        forms::flatten_fields(&mut self.inner).map_err(err)
    }

    /// Creates a form field on a page. Returns the field's object number.
    #[wasm_bindgen(js_name = addField)]
    pub fn add_field(&mut self, page: usize, field: NewFieldJs) -> Result<u32, JsError> {
        let mut f: NewField = serde_wasm_bindgen::from_value(field.into())
            .map_err(|e| JsError::new(&format!("invalid field: {e}")))?;
        let h = self.display_height(page)?;
        f.rect = flip_rect(&f.rect, h);
        f.widgets = f.widgets.iter().map(|r| flip_rect(r, h)).collect();
        Ok(forms::add_field(&mut self.inner, page, &f)
            .map_err(err)?
            .num)
    }

    /// Removes a field and its widgets. Returns whether it existed.
    #[wasm_bindgen(js_name = removeField)]
    pub fn remove_field(&mut self, name: &str) -> Result<bool, JsError> {
        forms::remove_field(&mut self.inner, name).map_err(err)
    }

    // -- text -------------------------------------------------------------------

    /// The page's text, lines top to bottom with a blank line between paragraphs.
    #[wasm_bindgen(js_name = pageText)]
    pub fn page_text(&self, page: usize) -> Result<String, JsError> {
        text::page_text(&self.inner, page).map_err(err)
    }

    /// Words on a page with their bounds (screen coordinates).
    #[wasm_bindgen(js_name = pageWords)]
    pub fn page_words(&self, page: usize) -> Result<TextSpanArrayJs, JsError> {
        let info = self.inner.page_info(page).map_err(err)?;
        let h = info.display_height();
        let mut words = text::page_words(&self.inner, page).map_err(err)?;
        for w in &mut words {
            w.rect = flip_rect(&annot::to_display_rect(&info, &w.rect), h);
        }
        Ok(to_js(&words)?.unchecked_into())
    }

    /// Finds text on a page. Matches may span line breaks.
    pub fn search(
        &self,
        page: usize,
        needle: &str,
        options: SearchOptionsJs,
    ) -> Result<SearchMatchArrayJs, JsError> {
        let o: SearchOptions = from_js(options.into())?;
        let info = self.inner.page_info(page).map_err(err)?;
        let h = info.display_height();
        let mut hits = text::search(&self.inner, page, needle, &o).map_err(err)?;
        for m in &mut hits {
            for r in &mut m.rects {
                *r = flip_rect(&annot::to_display_rect(&info, r), h);
            }
        }
        Ok(to_js(&hits)?.unchecked_into())
    }

    /// Permanently removes everything under `areas` (screen coordinates) on
    /// a page: text, vector graphics, image pixels and annotations, then
    /// paints the areas over.
    pub fn redact(
        &mut self,
        page: usize,
        areas: RectArrayJs,
        options: RedactOptionsJs,
    ) -> Result<RedactReportJs, JsError> {
        let rects: Vec<Rect> = serde_wasm_bindgen::from_value(areas.into())
            .map_err(|e| JsError::new(&format!("invalid areas: {e}")))?;
        let o: RedactOptions = from_js(options.into())?;
        let h = self.display_height(page)?;
        let areas: Vec<Rect> = rects.iter().map(|r| flip_rect(r, h)).collect();
        let report = redact::redact(&mut self.inner, page, &areas, &o).map_err(err)?;
        Ok(to_js(&report)?.unchecked_into())
    }

    /// Finds `needle` on `pages` (null = all) and redacts every match.
    #[wasm_bindgen(js_name = redactText)]
    pub fn redact_text(
        &mut self,
        pages: Option<String>,
        needle: &str,
        search: SearchOptionsJs,
        options: RedactOptionsJs,
    ) -> Result<RedactReportJs, JsError> {
        let so: SearchOptions = from_js(search.into())?;
        let o: RedactOptions = from_js(options.into())?;
        let idx = self.range(pages)?;
        let (report, matches) =
            redact::redact_text(&mut self.inner, &idx, needle, &so, &o).map_err(err)?;
        let v = to_js(&report)?;
        Reflect::set(&v, &"matches".into(), &(matches as u32).into()).ok();
        Ok(v.unchecked_into())
    }

    fn display_height(&self, page: usize) -> Result<f64, JsError> {
        Ok(self.inner.page_info(page).map_err(err)?.display_height())
    }

    /// Decoded content stream of a page (for debugging).
    #[wasm_bindgen(js_name = pageContent)]
    pub fn page_content(&self, index: usize) -> Result<String, JsError> {
        Ok(String::from_utf8_lossy(&self.inner.page_content(index).map_err(err)?).into_owned())
    }

    /// Serialises the document. Returns the PDF bytes.
    pub fn save(&mut self, options: SaveOptionsJs) -> Result<Vec<u8>, JsError> {
        let opts: SaveOptions = from_js(options.into())?;
        self.inner.save(&opts).map_err(err)
    }

    fn range(&self, pages: Option<String>) -> Result<Vec<usize>, JsError> {
        match pages {
            Some(p) => ops::parse_page_ranges(&p, self.inner.page_count()).map_err(err),
            None => Ok((0..self.inner.page_count()).collect()),
        }
    }
}

impl Default for PdfDocument {
    fn default() -> Self {
        Self::new()
    }
}

/// Converts between y-up display space and y-down screen space.
fn flip_rect(r: &Rect, h: f64) -> Rect {
    Rect::new(r.x0, h - r.y1, r.x1, h - r.y0)
}

/// Merges several PDFs (each a `Uint8Array`) into one document.
#[wasm_bindgen]
pub fn merge(files: Array) -> Result<PdfDocument, JsError> {
    let mut docs = Vec::with_capacity(files.length() as usize);
    for f in files.iter() {
        let bytes = Uint8Array::new(&f).to_vec();
        docs.push(Document::load(&bytes).map_err(err)?);
    }
    let refs: Vec<&Document> = docs.iter().collect();
    Ok(PdfDocument {
        inner: ops::merge(&refs).map_err(err)?,
    })
}

fn read_inputs(inputs: &Array) -> Result<Vec<Input>, JsError> {
    let mut out = Vec::with_capacity(inputs.length() as usize);
    for v in inputs.iter() {
        let name = Reflect::get(&v, &"name".into())
            .ok()
            .and_then(|n| n.as_string())
            .unwrap_or_else(|| "input.pdf".into());
        let data =
            Reflect::get(&v, &"data".into()).map_err(|_| JsError::new("input.data missing"))?;
        let data = Uint8Array::new(&data).to_vec();
        let password = Reflect::get(&v, &"password".into())
            .ok()
            .and_then(|p| p.as_string());
        out.push(Input {
            name,
            data,
            password,
        });
    }
    Ok(out)
}

/// Runs a preset over inputs. See the `Preset` type for the schema.
#[wasm_bindgen(js_name = runBatch)]
pub fn run_batch(
    preset: PresetJs,
    inputs: BatchInputsJs,
    assets: BatchAssetsJs,
) -> Result<BatchResultJs, JsError> {
    let preset: Preset = from_js(preset.into())?;
    let inputs = read_inputs(&Array::from(&JsValue::from(inputs)))?;
    let assets_js: JsValue = assets.into();
    let mut asset_list = Vec::new();
    if !assets_js.is_undefined() && !assets_js.is_null() {
        for v in Array::from(&assets_js).iter() {
            let name = Reflect::get(&v, &"name".into())
                .ok()
                .and_then(|n| n.as_string())
                .unwrap_or_default();
            let data =
                Reflect::get(&v, &"data".into()).map_err(|_| JsError::new("asset.data missing"))?;
            asset_list.push(Asset {
                name,
                data: Uint8Array::new(&data).to_vec(),
            });
        }
    }
    let result = batch::run(&preset, &inputs, &asset_list).map_err(err)?;
    let outputs = Array::new();
    for o in &result.outputs {
        let obj = Object::new();
        Reflect::set(&obj, &"name".into(), &o.name.clone().into()).ok();
        Reflect::set(
            &obj,
            &"data".into(),
            &Uint8Array::from(o.data.as_slice()).into(),
        )
        .ok();
        Reflect::set(&obj, &"pages".into(), &(o.pages as u32).into()).ok();
        Reflect::set(&obj, &"bytes".into(), &(o.bytes as u32).into()).ok();
        let sources = Array::new();
        for s in &o.sources {
            sources.push(&s.clone().into());
        }
        Reflect::set(&obj, &"sources".into(), &sources.into()).ok();
        outputs.push(&obj.into());
    }
    let res = Object::new();
    Reflect::set(&res, &"outputs".into(), &outputs.into()).ok();
    let warnings = Array::new();
    for w in &result.warnings {
        warnings.push(&w.clone().into());
    }
    Reflect::set(&res, &"warnings".into(), &warnings.into()).ok();
    Ok(JsValue::from(res).unchecked_into())
}

/// Checks a preset for errors. Throws with a message if invalid.
#[wasm_bindgen(js_name = validatePreset)]
pub fn validate_preset(preset: PresetJs) -> Result<(), JsError> {
    let p: Preset = from_js(preset.into())?;
    p.validate().map_err(err)
}

/// The built-in example presets.
#[wasm_bindgen(js_name = builtinPresets)]
pub fn builtin_presets() -> Result<PresetArrayJs, JsError> {
    Ok(to_js(&Preset::builtins())?.unchecked_into())
}

/// A named collection of presets that serialises to JSON, for persisting
/// export configurations in `localStorage`, a file or a database.
#[wasm_bindgen]
pub struct PresetStore {
    inner: CoreStore,
}

#[wasm_bindgen]
impl PresetStore {
    /// Empty store.
    #[wasm_bindgen(constructor)]
    pub fn new() -> PresetStore {
        PresetStore {
            inner: CoreStore::new(),
        }
    }
    /// Store pre-filled with the built-in presets.
    #[wasm_bindgen(js_name = withBuiltins)]
    pub fn with_builtins() -> PresetStore {
        PresetStore {
            inner: CoreStore::with_builtins(),
        }
    }
    /// Parses a store previously produced by `toJson`.
    #[wasm_bindgen(js_name = fromJson)]
    pub fn from_json(json: &str) -> Result<PresetStore, JsError> {
        Ok(PresetStore {
            inner: CoreStore::from_json(json).map_err(err)?,
        })
    }
    /// Serialises the store.
    #[wasm_bindgen(js_name = toJson)]
    pub fn to_json(&self) -> String {
        self.inner.to_json()
    }
    /// Adds or replaces a preset (validated first).
    pub fn add(&mut self, preset: PresetJs) -> Result<(), JsError> {
        let p: Preset = from_js(preset.into())?;
        p.validate().map_err(err)?;
        self.inner.add(p);
        Ok(())
    }
    /// Removes a preset by name. Returns whether it existed.
    pub fn remove(&mut self, name: &str) -> bool {
        self.inner.remove(name).is_some()
    }
    /// Gets a preset by name, or undefined.
    pub fn get(&self, name: &str) -> Result<JsValue, JsError> {
        match self.inner.get(name) {
            Some(p) => to_js(p),
            None => Ok(JsValue::UNDEFINED),
        }
    }
    /// Sorted preset names.
    pub fn names(&self) -> Vec<String> {
        self.inner.names().into_iter().map(str::to_owned).collect()
    }
    /// Number of presets.
    pub fn size(&self) -> usize {
        self.inner.len()
    }
}

impl Default for PresetStore {
    fn default() -> Self {
        Self::new()
    }
}
