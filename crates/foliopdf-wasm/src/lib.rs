//! WebAssembly bindings for [foliopdf](https://github.com/keithadler/foliopdf).
//!
//! The JavaScript API mirrors the Rust one: a `PdfDocument` class for
//! editing, `runBatch` for presets, and a `PresetStore` for keeping export
//! configurations. All byte arguments are `Uint8Array`s; all option objects
//! are plain JSON (see the TypeScript definitions shipped in the package).

use foliopdf::batch::{self, Asset, Input, Preset, PresetStore as CoreStore};
use foliopdf::document::Metadata;
use foliopdf::ops::{self, ImageStamp, PageNumbers, TextStamp};
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
