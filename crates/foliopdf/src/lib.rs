//! # foliopdf
//!
//! Fast, portable PDF editing in pure Rust that compiles to WebAssembly.
//!
//! `foliopdf` opens existing PDFs (including damaged and encrypted ones),
//! lets you merge, split, reorder, rotate, stamp, compress and encrypt them,
//! and writes clean, compact output. It ships as a Rust crate, an npm package
//! (`foliopdf`) and a command-line tool (`folio`).
//!
//! ## Quick start
//!
//! ```no_run
//! use foliopdf::{Document, EncryptionOptions, SaveOptions};
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let bytes = std::fs::read("in.pdf")?;
//! let mut doc = Document::load(&bytes)?;
//! doc.remove_page(0)?;
//! doc.set_title("Edited");
//! let out = doc.save(&SaveOptions {
//!     compress: true,
//!     encryption: Some(EncryptionOptions::new("open-me", "owner")),
//!     ..Default::default()
//! })?;
//! std::fs::write("out.pdf", out)?;
//! # Ok(())
//! # }
//! ```
//!
//! ## Modules
//!
//! * [`Document`] – load, inspect, edit and save a file.
//! * [`ops`] – higher-level operations: merge, split, page ranges, stamps.
//! * [`annot`] – highlights, drawings, notes, links, image stamps; flattening.
//! * [`forms`] – list, fill and flatten interactive form fields.
//! * [`text`] – extract text with positions, and search.
//! * [`redact`] – remove text, graphics and image pixels under an area.
//! * [`compress`] – downsample and re-encode images for much smaller files.
//! * [`outline`] – read and write bookmarks.
//! * [`batch`] – run a JSON-described pipeline over many files; store and
//!   reuse export presets.
//! * [`object`] – the low-level object model when you need full control.
//!
//! ## Design notes
//!
//! * All objects are loaded eagerly at open time; streams stay compressed in
//!   memory. A 100-page text document parses in a few milliseconds.
//! * Saving always rewrites the whole file, producing a single clean
//!   cross-reference section with unused objects dropped. Incremental updates
//!   are deliberately not produced: they are the main source of bloated and
//!   corrupt PDFs in the wild.
//! * The crate has no unsafe code and no I/O; the CLI and WASM crates own the
//!   file system and browser boundaries.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod annot;
pub mod batch;
pub mod compress;
pub mod content;
pub mod crypto;
pub mod cstream;
pub mod document;
pub mod error;
pub mod filters;
pub mod font;
pub mod forms;
pub mod geometry;
pub mod glyphlist;
pub mod image;
pub mod imgcodec;
pub mod lexer;
pub mod object;
pub mod ops;
pub mod outline;
pub mod page;
pub mod parser;
pub mod redact;
pub mod text;
pub mod writer;

pub use crypto::{EncryptionOptions, Method as EncryptionMethod, Permissions};
pub use document::{Document, LoadOptions, SaveOptions};
pub use error::{Error, Result};
pub use geometry::{Matrix, Point, Rect};
pub use object::{Dict, Name, ObjRef, Object, PdfString, Stream};
pub use page::{PageInfo, PageSize, Rotation};

/// Crate version string.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
