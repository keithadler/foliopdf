//! Batch processing with storable presets.
//!
//! A [`Preset`] is a JSON-serialisable description of a pipeline: how inputs
//! are combined, which [`Step`]s run, and how output is written (compression,
//! encryption, file naming). Presets are plain data, so they can be saved by
//! the host (a file, `localStorage`, a database) and replayed later with
//! [`run`]. [`PresetStore`] is a small helper for keeping a named collection
//! and round-tripping it through JSON.
//!
//! ```
//! use foliopdf::batch::{Preset, Input, run};
//! let preset: Preset = serde_json::from_str(r#"{
//!   "name": "compress-and-lock",
//!   "steps": [{ "op": "strip-metadata" }],
//!   "output": { "compress": true, "encryption": { "userPassword": "", "ownerPassword": "s3cret" } }
//! }"#).unwrap();
//! # let pdf = { let mut d = foliopdf::Document::new(); d.add_page(foliopdf::PageSize::A4); d.save(&Default::default()).unwrap() };
//! let result = run(&preset, &[Input::new("report.pdf", pdf)], &[]).unwrap();
//! assert_eq!(result.outputs.len(), 1);
//! assert_eq!(result.outputs[0].name, "report.pdf");
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::compress::ImageOptions;
use crate::crypto::EncryptionOptions;
use crate::document::{Document, LoadOptions, Metadata, SaveOptions};
use crate::error::{Error, Result};
use crate::ops::{self, FitMode, ImageStamp, PageNumbers, TextStamp};
use crate::page::PageSize;

/// Current preset schema version.
pub const PRESET_SCHEMA: u32 = 1;

/// How multiple inputs are treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    /// Process every input independently (one output set per input).
    #[default]
    Each,
    /// Merge all inputs into a single document first.
    Merge,
}

/// One processing step. Serialised with an `op` tag, e.g.
/// `{ "op": "rotate", "pages": "odd", "degrees": 90 }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum Step {
    /// Keep only these pages, in this order (see [`ops::parse_page_ranges`]).
    SelectPages {
        /// Page range expression.
        pages: String,
    },
    /// Delete these pages.
    DeletePages {
        /// Page range expression.
        pages: String,
    },
    /// Rotate pages by a multiple of 90 degrees.
    Rotate {
        /// Page range expression; omitted means all pages.
        #[serde(default)]
        pages: Option<String>,
        /// Degrees clockwise.
        degrees: i64,
    },
    /// Draw a text stamp or watermark.
    StampText {
        /// Page range expression; omitted means all pages.
        #[serde(default)]
        pages: Option<String>,
        /// Stamp settings.
        #[serde(flatten)]
        stamp: TextStamp,
    },
    /// Draw an image from the job's assets.
    StampImage {
        /// Page range expression; omitted means all pages.
        #[serde(default)]
        pages: Option<String>,
        /// Name of the asset (JPEG or PNG) supplied to [`run`].
        asset: String,
        /// Stamp settings.
        #[serde(flatten)]
        stamp: ImageStamp,
    },
    /// Add page numbers.
    PageNumbers {
        /// Page range expression; omitted means all pages.
        #[serde(default)]
        pages: Option<String>,
        /// Number settings.
        #[serde(flatten)]
        settings: PageNumbers,
    },
    /// Set document information fields.
    Metadata {
        /// Fields to set; omitted fields are untouched, empty strings clear.
        #[serde(flatten)]
        metadata: Metadata,
    },
    /// Remove XMP metadata, the info dictionary and thumbnails.
    StripMetadata,
    /// Change the page size, scaling content to fit.
    Resize {
        /// Page range expression; omitted means all pages.
        #[serde(default)]
        pages: Option<String>,
        /// Named size (`a4`, `letter`, `legal`, `a3`, `a5`, `tabloid`,
        /// optionally with `-landscape`). Alternatively give `width` and `height`.
        #[serde(default)]
        size: Option<String>,
        /// Width in points when no `size` is given.
        #[serde(default)]
        width: Option<f64>,
        /// Height in points when no `size` is given.
        #[serde(default)]
        height: Option<f64>,
        /// `fit` (default), `fill` or `stretch`.
        #[serde(default)]
        mode: FitMode,
    },
    /// Scale pages (and their content) by a factor; 0.5 halves them.
    Scale {
        /// Page range expression; omitted means all pages.
        #[serde(default)]
        pages: Option<String>,
        /// Multiplier, greater than zero.
        factor: f64,
    },
    /// Reverse the page order.
    Reverse,
    /// Insert blank pages.
    BlankPages {
        /// Insert before this 1-based page number; omitted or 0 appends.
        #[serde(default)]
        at: Option<usize>,
        /// How many pages (default 1).
        #[serde(default)]
        count: Option<usize>,
        /// Named size; defaults to the size of the page before the insertion
        /// point (or Letter in an empty document).
        #[serde(default)]
        size: Option<String>,
    },
    /// Downsample and re-encode images as JPEG (lossy) for much smaller files.
    CompressImages {
        /// Settings; omitted fields use the defaults (150 dpi, quality 75).
        #[serde(flatten)]
        options: ImageOptions,
    },
    /// Split into several documents. Must be the last step.
    Split {
        /// Pages per output file.
        #[serde(default)]
        every: Option<usize>,
        /// Explicit page range expressions, one per output file.
        #[serde(default)]
        ranges: Option<Vec<String>>,
    },
}

/// Output settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct OutputOptions {
    /// Recompress streams and use object streams.
    pub compress: bool,
    /// Flate level 1–10.
    pub compression_level: u8,
    /// Use object streams (PDF 1.5+). Ignored unless `compress`.
    pub object_streams: bool,
    /// Encrypt outputs.
    pub encryption: Option<EncryptionOptions>,
    /// Output file name template. Placeholders: `{stem}` (input name without
    /// extension), `{index}` (1-based part number), `{total}` (part count),
    /// `{n}` (1-based output number across the job).
    pub filename: String,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self {
            compress: true,
            compression_level: 6,
            object_streams: true,
            encryption: None,
            filename: "{stem}.pdf".into(),
        }
    }
}

impl OutputOptions {
    fn save_options(&self) -> SaveOptions {
        SaveOptions {
            compress: self.compress,
            compression_level: self.compression_level,
            object_streams: self.object_streams,
            strip_metadata: false,
            encryption: self.encryption.clone(),
            ..Default::default()
        }
    }
}

/// A reusable, storable pipeline description.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Preset {
    /// Schema version (currently 1).
    pub schema: u32,
    /// Short unique name.
    pub name: String,
    /// Optional description for UIs.
    pub description: Option<String>,
    /// How inputs are combined.
    pub mode: Mode,
    /// Steps in order.
    pub steps: Vec<Step>,
    /// Output settings.
    pub output: OutputOptions,
}

impl Default for Preset {
    fn default() -> Self {
        Self {
            schema: PRESET_SCHEMA,
            name: "untitled".into(),
            description: None,
            mode: Mode::Each,
            steps: Vec::new(),
            output: OutputOptions::default(),
        }
    }
}

impl Preset {
    /// A preset with a name and no steps (compress only).
    pub fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }
    /// Parses JSON.
    pub fn from_json(json: &str) -> Result<Self> {
        let p: Preset = serde_json::from_str(json)?;
        p.validate()?;
        Ok(p)
    }
    /// Serialises to pretty JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("preset is serialisable")
    }
    /// Checks fields that serde cannot: schema version, degrees, opacity,
    /// split placement, compression level.
    pub fn validate(&self) -> Result<()> {
        if self.schema != PRESET_SCHEMA {
            return Err(Error::Preset(format!(
                "unsupported schema version {}",
                self.schema
            )));
        }
        if self.name.trim().is_empty() {
            return Err(Error::Preset("name must not be empty".into()));
        }
        if !(1..=10).contains(&self.output.compression_level) {
            return Err(Error::Preset("compressionLevel must be 1–10".into()));
        }
        for (i, s) in self.steps.iter().enumerate() {
            match s {
                Step::Rotate { degrees, .. } if degrees % 90 != 0 => {
                    return Err(Error::Preset(format!(
                        "step {}: degrees must be a multiple of 90",
                        i + 1
                    )));
                }
                Step::StampText { stamp, .. } if !(0.0..=1.0).contains(&stamp.opacity) => {
                    return Err(Error::Preset(format!(
                        "step {}: opacity must be 0–1",
                        i + 1
                    )));
                }
                Step::StampText { stamp, .. } if stamp.text.is_empty() => {
                    return Err(Error::Preset(format!(
                        "step {}: text must not be empty",
                        i + 1
                    )));
                }
                Step::StampImage { stamp, .. } if !(0.0..=1.0).contains(&stamp.opacity) => {
                    return Err(Error::Preset(format!(
                        "step {}: opacity must be 0–1",
                        i + 1
                    )));
                }
                Step::Resize {
                    size,
                    width,
                    height,
                    ..
                } => {
                    resolve_size(size.as_deref(), *width, *height)
                        .map_err(|e| Error::Preset(format!("step {}: {e}", i + 1)))?;
                }
                Step::CompressImages { options }
                    if !(1..=100).contains(&options.quality)
                        || options.max_dpi <= 0.0
                        || options.max_dpi.is_nan() =>
                {
                    return Err(Error::Preset(format!(
                        "step {}: quality must be 1–100 and maxDpi positive",
                        i + 1
                    )));
                }
                Step::Scale { factor, .. } if !(*factor > 0.0 && factor.is_finite()) => {
                    return Err(Error::Preset(format!(
                        "step {}: factor must be greater than zero",
                        i + 1
                    )));
                }
                Step::BlankPages { size: Some(sz), .. } if PageSize::by_name(sz).is_none() => {
                    return Err(Error::Preset(format!(
                        "step {}: unknown page size '{sz}'",
                        i + 1
                    )));
                }
                Step::Split { every, ranges } => {
                    if i + 1 != self.steps.len() {
                        return Err(Error::Preset("split must be the last step".into()));
                    }
                    match (every, ranges) {
                        (Some(0), _) => {
                            return Err(Error::Preset("split.every must be at least 1".into()))
                        }
                        (None, None) => {
                            return Err(Error::Preset("split needs 'every' or 'ranges'".into()))
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Built-in presets that double as examples.
    pub fn builtins() -> Vec<Preset> {
        vec![
            Preset {
                name: "compress".into(),
                description: Some("Recompress streams, pack objects, drop unused data.".into()),
                ..Default::default()
            },
            Preset {
                name: "merge".into(),
                description: Some("Merge all inputs into one file.".into()),
                mode: Mode::Merge,
                output: OutputOptions {
                    filename: "merged.pdf".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
            Preset {
                name: "encrypt-aes256".into(),
                description: Some(
                    "AES-256 with an owner password; anyone can open, nobody can edit or copy."
                        .into(),
                ),
                output: OutputOptions {
                    encryption: Some(EncryptionOptions {
                        owner_password: "change-me".into(),
                        permissions: crate::crypto::Permissions {
                            modify: false,
                            copy: false,
                            annotate: false,
                            assemble: false,
                            ..Default::default()
                        },
                        ..EncryptionOptions::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            Preset {
                name: "draft-watermark".into(),
                description: Some(
                    "Diagonal DRAFT watermark on every page plus page numbers.".into(),
                ),
                steps: vec![
                    Step::StampText {
                        pages: None,
                        stamp: TextStamp::watermark("DRAFT"),
                    },
                    Step::PageNumbers {
                        pages: None,
                        settings: PageNumbers::default(),
                    },
                ],
                ..Default::default()
            },
            Preset {
                name: "split-single-pages".into(),
                description: Some("One file per page.".into()),
                steps: vec![Step::Split {
                    every: Some(1),
                    ranges: None,
                }],
                output: OutputOptions {
                    filename: "{stem}-{index}.pdf".into(),
                    ..Default::default()
                },
                ..Default::default()
            },
        ]
    }
}

/// An input file.
#[derive(Debug, Clone)]
pub struct Input {
    /// File name (used for `{stem}`).
    pub name: String,
    /// PDF bytes.
    pub data: Vec<u8>,
    /// Password if the file is encrypted.
    pub password: Option<String>,
}

impl Input {
    /// Creates an input.
    pub fn new(name: &str, data: Vec<u8>) -> Self {
        Self {
            name: name.into(),
            data,
            password: None,
        }
    }
    /// Sets a password.
    pub fn with_password(mut self, password: &str) -> Self {
        self.password = Some(password.into());
        self
    }
    fn stem(&self) -> String {
        let base = self.name.rsplit(['/', '\\']).next().unwrap_or(&self.name);
        match base.rsplit_once('.') {
            Some((s, _)) if !s.is_empty() => s.to_string(),
            _ => base.to_string(),
        }
    }
}

/// A named binary asset (image) available to steps.
#[derive(Debug, Clone)]
pub struct Asset {
    /// Name referenced by `stamp-image` steps.
    pub name: String,
    /// Bytes.
    pub data: Vec<u8>,
}

/// One produced file.
#[derive(Debug, Clone, Serialize)]
pub struct Output {
    /// File name from the template.
    pub name: String,
    /// PDF bytes.
    #[serde(skip)]
    pub data: Vec<u8>,
    /// Page count.
    pub pages: usize,
    /// Size in bytes.
    pub bytes: usize,
    /// Names of the inputs that fed this output.
    pub sources: Vec<String>,
}

/// Result of a batch run.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BatchResult {
    /// Produced files in order.
    pub outputs: Vec<Output>,
    /// Non-fatal notes (e.g. an input needed cross-reference recovery).
    pub warnings: Vec<String>,
}

/// Runs `preset` over `inputs`.
pub fn run(preset: &Preset, inputs: &[Input], assets: &[Asset]) -> Result<BatchResult> {
    preset.validate()?;
    if inputs.is_empty() {
        return Err(Error::Preset("no inputs".into()));
    }
    let mut result = BatchResult::default();
    let load = |inp: &Input, warnings: &mut Vec<String>| -> Result<Document> {
        let opts = LoadOptions {
            password: inp.password.clone().unwrap_or_default(),
            recover: true,
        };
        let d = Document::load_with(&inp.data, &opts)
            .map_err(|e| Error::Malformed(format!("{}: {e}", inp.name)))?;
        if d.was_reconstructed() {
            warnings.push(format!(
                "{}: cross-reference table was damaged and has been rebuilt",
                inp.name
            ));
        }
        Ok(d)
    };
    match preset.mode {
        Mode::Merge => {
            let mut docs = Vec::with_capacity(inputs.len());
            for inp in inputs {
                docs.push(load(inp, &mut result.warnings)?);
            }
            let refs: Vec<&Document> = docs.iter().collect();
            let doc = ops::merge(&refs)?;
            let stem = format!("{}-merged", inputs[0].stem());
            let sources: Vec<String> = inputs.iter().map(|i| i.name.clone()).collect();
            process(preset, doc, &stem, sources, assets, &mut result)?;
        }
        Mode::Each => {
            for inp in inputs {
                let doc = load(inp, &mut result.warnings)?;
                process(
                    preset,
                    doc,
                    &inp.stem(),
                    vec![inp.name.clone()],
                    assets,
                    &mut result,
                )?;
            }
        }
    }
    Ok(result)
}

fn pages_or_all(doc: &Document, spec: &Option<String>) -> Result<Vec<usize>> {
    match spec {
        Some(s) => ops::parse_page_ranges(s, doc.page_count()),
        None => Ok((0..doc.page_count()).collect()),
    }
}

/// Turns a `resize` step's size fields into a [`PageSize`].
pub fn resolve_size(
    size: Option<&str>,
    width: Option<f64>,
    height: Option<f64>,
) -> Result<PageSize> {
    match (size, width, height) {
        (Some(name), _, _) => PageSize::by_name(name)
            .ok_or_else(|| Error::Preset(format!("unknown page size '{name}' (try a4, letter, legal, a3, a5, tabloid, or add -landscape)"))),
        (None, Some(w), Some(h)) if w > 1.0 && h > 1.0 && w.is_finite() && h.is_finite() => Ok(PageSize::new(w, h)),
        (None, Some(_), Some(_)) => Err(Error::Preset("width and height must be larger than 1 point".into())),
        _ => Err(Error::Preset("resize needs a 'size' name or both 'width' and 'height'".into())),
    }
}

fn process(
    preset: &Preset,
    mut doc: Document,
    stem: &str,
    sources: Vec<String>,
    assets: &[Asset],
    result: &mut BatchResult,
) -> Result<()> {
    let mut outputs: Vec<Document> = Vec::new();
    let mut split_done = false;
    for step in &preset.steps {
        match step {
            Step::SelectPages { pages } => {
                let idx = ops::parse_page_ranges(pages, doc.page_count())?;
                doc.select_pages(&idx)?;
            }
            Step::DeletePages { pages } => {
                let idx = ops::parse_page_ranges(pages, doc.page_count())?;
                ops::delete_pages(&mut doc, &idx)?;
            }
            Step::Rotate { pages, degrees } => {
                let idx = pages_or_all(&doc, pages)?;
                ops::rotate_pages(&mut doc, &idx, *degrees)?;
            }
            Step::StampText { pages, stamp } => {
                let idx = pages_or_all(&doc, pages)?;
                ops::stamp_text(&mut doc, &idx, stamp)?;
            }
            Step::StampImage {
                pages,
                asset,
                stamp,
            } => {
                let idx = pages_or_all(&doc, pages)?;
                let a = assets
                    .iter()
                    .find(|a| a.name == *asset)
                    .ok_or_else(|| Error::Preset(format!("asset '{asset}' not provided")))?;
                ops::stamp_image(&mut doc, &idx, &a.data, stamp)?;
            }
            Step::PageNumbers { pages, settings } => {
                let idx = pages_or_all(&doc, pages)?;
                ops::add_page_numbers(&mut doc, &idx, settings)?;
            }
            Step::Metadata { metadata } => doc.set_metadata(metadata),
            Step::StripMetadata => doc.strip_metadata(),
            Step::Resize {
                pages,
                size,
                width,
                height,
                mode,
            } => {
                let idx = pages_or_all(&doc, pages)?;
                let target = resolve_size(size.as_deref(), *width, *height)?;
                ops::resize_pages(&mut doc, &idx, target, *mode)?;
            }
            Step::Scale { pages, factor } => {
                let idx = pages_or_all(&doc, pages)?;
                ops::scale_pages(&mut doc, &idx, *factor)?;
            }
            Step::Reverse => ops::reverse_pages(&mut doc)?,
            Step::CompressImages { options } => {
                crate::compress::compress_images(&mut doc, options)?;
            }
            Step::BlankPages { at, count, size } => {
                let n = doc.page_count();
                let at = match at {
                    Some(a) if *a > 0 => (*a - 1).min(n),
                    _ => n,
                };
                let sz = match size {
                    Some(s) => PageSize::by_name(s)
                        .ok_or_else(|| Error::Preset(format!("unknown page size '{s}'")))?,
                    None if n == 0 => PageSize::LETTER,
                    None => {
                        let info = doc.page_info(at.saturating_sub(1).min(n - 1))?;
                        PageSize::new(info.media_box.width(), info.media_box.height())
                    }
                };
                ops::insert_blank_pages(&mut doc, at, count.unwrap_or(1).max(1), sz)?;
            }
            Step::Split { every, ranges } => {
                let chunks: Vec<Vec<usize>> = match (every, ranges) {
                    (Some(e), _) => ops::chunk_pages(doc.page_count(), *e),
                    (None, Some(rs)) => rs
                        .iter()
                        .map(|r| ops::parse_page_ranges(r, doc.page_count()))
                        .collect::<Result<_>>()?,
                    (None, None) => vec![(0..doc.page_count()).collect()],
                };
                outputs = ops::split(&doc, &chunks)?;
                split_done = true;
            }
        }
    }
    if !split_done {
        outputs.push(doc);
    }
    let total = outputs.len();
    let save_opts = preset.output.save_options();
    for (i, mut d) in outputs.into_iter().enumerate() {
        let data = d.save(&save_opts)?;
        let n = result.outputs.len() + 1;
        let name = preset
            .output
            .filename
            .replace("{stem}", stem)
            .replace("{index}", &(i + 1).to_string())
            .replace("{total}", &total.to_string())
            .replace("{n}", &n.to_string());
        let name = if total > 1
            && !preset.output.filename.contains("{index}")
            && !preset.output.filename.contains("{n}")
        {
            // Avoid silently overwriting when splitting without a numbered template.
            match name.rsplit_once('.') {
                Some((s, ext)) => format!("{s}-{}.{ext}", i + 1),
                None => format!("{name}-{}", i + 1),
            }
        } else {
            name
        };
        result.outputs.push(Output {
            name,
            pages: d.page_count(),
            bytes: data.len(),
            data,
            sources: sources.clone(),
        });
    }
    Ok(())
}

/// A named collection of presets that round-trips through JSON.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PresetStore {
    presets: BTreeMap<String, Preset>,
}

impl PresetStore {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }
    /// Store pre-filled with [`Preset::builtins`].
    pub fn with_builtins() -> Self {
        let mut s = Self::new();
        for p in Preset::builtins() {
            s.add(p);
        }
        s
    }
    /// Adds or replaces a preset (keyed by its name).
    pub fn add(&mut self, preset: Preset) {
        self.presets.insert(preset.name.clone(), preset);
    }
    /// Removes a preset.
    pub fn remove(&mut self, name: &str) -> Option<Preset> {
        self.presets.remove(name)
    }
    /// Looks up a preset.
    pub fn get(&self, name: &str) -> Option<&Preset> {
        self.presets.get(name)
    }
    /// Names in sorted order.
    pub fn names(&self) -> Vec<&str> {
        self.presets.keys().map(String::as_str).collect()
    }
    /// All presets.
    pub fn iter(&self) -> impl Iterator<Item = &Preset> {
        self.presets.values()
    }
    /// Number of presets.
    pub fn len(&self) -> usize {
        self.presets.len()
    }
    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.presets.is_empty()
    }
    /// Serialises the whole store.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("store is serialisable")
    }
    /// Parses a store, validating every preset.
    pub fn from_json(json: &str) -> Result<Self> {
        let s: PresetStore = serde_json::from_str(json)?;
        for p in s.presets.values() {
            p.validate()?;
        }
        Ok(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_json_round_trip() {
        for p in Preset::builtins() {
            let j = p.to_json();
            let back = Preset::from_json(&j).unwrap();
            assert_eq!(p, back, "{}", p.name);
        }
    }

    #[test]
    fn step_tags() {
        let s: Step =
            serde_json::from_str(r#"{"op":"rotate","pages":"1-2","degrees":90}"#).unwrap();
        assert_eq!(
            s,
            Step::Rotate {
                pages: Some("1-2".into()),
                degrees: 90
            }
        );
        let s: Step =
            serde_json::from_str(r#"{"op":"stamp-text","text":"HELLO","size":20}"#).unwrap();
        match s {
            Step::StampText { stamp, .. } => {
                assert_eq!(stamp.text, "HELLO");
                assert_eq!(stamp.size, 20.0);
            }
            _ => panic!(),
        }
    }

    #[test]
    fn validation() {
        let mut p = Preset::new("x");
        p.steps.push(Step::Rotate {
            pages: None,
            degrees: 45,
        });
        assert!(p.validate().is_err());
        p.steps.clear();
        p.steps.push(Step::Split {
            every: Some(1),
            ranges: None,
        });
        p.steps.push(Step::StripMetadata);
        assert!(p.validate().is_err());
    }

    #[test]
    fn store_round_trip() {
        let s = PresetStore::with_builtins();
        let j = s.to_json();
        let back = PresetStore::from_json(&j).unwrap();
        assert_eq!(s, back);
        assert!(back.get("compress").is_some());
    }

    #[test]
    fn stem() {
        assert_eq!(Input::new("dir/a.b.pdf", vec![]).stem(), "a.b");
        assert_eq!(Input::new("noext", vec![]).stem(), "noext");
        assert_eq!(Input::new(".hidden", vec![]).stem(), ".hidden");
    }
}
