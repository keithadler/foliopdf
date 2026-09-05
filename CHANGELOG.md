# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- OCR in the web app (Tesseract.js, on-device): recognised words become an
  invisible, searchable text layer (`ops::add_text_layer`, wasm
  `addTextLayer`).
- Compare two PDFs (word-level text diff per page) in the web app.
- Extract images (`extract::extract_images`, CLI `extract-images`, wasm
  `extractImages`, web tool) as JPEG/PNG.
- Headers & footers tool with `{date}`, `{title}`, `{file}` and Bates
  numbering; page-number formats accept `{page:6}` zero padding.
- N-up sheets and booklet imposition (`impose` module, batch steps `nup`
  and `booklet`, CLI `nup`/`booklet`, wasm `nup`/`booklet`, web tool).
- Flatten and Repair web tools; batch step `flatten`.

## [0.2.0] - 2026-09-04

The "no one needs Adobe" release: forms, signing, comments, redaction,
text extraction and search, image compression, bookmarks, cropping,
conversions, and an installable web app.

### Changed
- `EncryptionOptions.method` now serialises as `aes-256`, `aes-128` and
  `rc4-128` as documented; the old spellings (`aes256`, `aes128`, `rc4`) are
  still accepted when reading.

### Fixed
- Saving with `compress: true` could merge font dictionaries that differed
  only in the objects they referenced (once any real duplicate object was
  present), which scrambled extracted text and could show wrong
  characters. Object de-duplication now compares references properly.

### Added
- Text engine (`text` module): glyph positions and Unicode for simple and
  composite fonts, words and lines in reading order, page text extraction,
  and search (case-insensitive, whole-word, across line breaks). CLI `text`
  and `search`; wasm `pageText`, `pageWords`, `search`.
- True redaction (`redact` module): removes text, vector graphics and image
  pixels (raw, Flate, JPEG) under areas or search matches, including inside
  form XObjects and invisible OCR text; removes overlapping annotations;
  paints the areas. CLI `redact`; wasm `redact`, `redactText`.
- Web app: Redact (mark areas, find and mark every occurrence, report of
  what was removed) and Extract text tools; Compress gains image quality
  presets.
- Bookmarks (`outline` module): read the tree (named destinations resolved),
  replace it, and carry it across merge, extract and page selection. CLI
  `bookmarks`; wasm `bookmarks`/`setBookmarks`; web Bookmarks tool.
- Crop: `ops::crop_pages`/`uncrop_pages`, CLI `crop`, wasm `cropPages`/
  `uncropPages`, web Crop tool (drag the area to keep).
- Web app is installable (web manifest, icons, service worker): works
  fully offline once opened, registers as a `.pdf` file handler, shows the
  build in the footer and offers a one-click update when a new deploy is
  live.
- `ops::images_to_pdf` / `add_image_page`, CLI `images`, wasm `imagesToPdf`
  / `addImagePage`; web Images to PDF (any format the browser can decode)
  and PDF to images (PNG/JPEG, ZIP for several pages) tools.
- Lossy image recompression (`compress` module, batch step
  `compress-images`, CLI `compress --images`, wasm `compressImages`):
  downsample oversized grey/RGB images and re-encode as JPEG.
- Interactive forms (`forms` module): list fields, fill text / check box /
  radio / drop-down / list fields with generated appearances, create and
  remove fields, flatten. Merging and extracting pages now carries form
  fields along (renamed on clashes). CLI `fields`, `fill`, `flatten`; wasm
  `fields`, `setField(s)`, `addField`, `removeField`, `flattenFields`.
- Annotations (`annot` module): highlight, underline, strike-out, square,
  circle, line, ink, free text, sticky note, link and image stamp, each with
  an appearance stream; list, remove, and flatten (all or selected). CLI
  `annots`, `flatten`; wasm `annotations`, `addAnnotation`,
  `addImageAnnotation`, `removeAnnotation(s)`, `flattenAnnotations`.
- Web app: Fill & Sign (type, tick, date, draw/type/upload a signature),
  Fill a form (fields overlaid on the page, keep editable or flatten), and
  Comment & mark up (select text to highlight, draw, notes, shapes, stamps),
  all on a zoomable page view with undo.
- `ops::resize_pages`, `ops::scale_pages`, `ops::reverse_pages` and
  `ops::insert_blank_pages`; batch steps `resize`, `scale`, `reverse` and
  `blank-pages`; CLI commands `resize`, `reverse` and `blank`; wasm methods
  `resizePages`, `scalePages`, `reversePages`, `insertBlankPages` and
  `hasOwnerAccess`.
- Web app: Page size tool; blank pages in Organize; light/dark theme toggle;
  thumbnail size control; page thumbnails (via pdf.js) in Organize and in
  clickable page grids for Delete, Split, Rotate, Watermark and Page numbers;
  visual preset builder with import/export; phone layout.

## [0.1.0] - 2026-09-03

First release.

### Added
- Parser for PDF 1.0–2.0: cross-reference tables and streams, hybrid files,
  object streams, incremental updates, and full-file reconstruction when the
  cross-reference data is missing or wrong.
- Filters: Flate, LZW, ASCIIHex, ASCII85, RunLength, PNG and TIFF predictors.
  Image codecs are passed through untouched.
- Encryption: open RC4 40–128, AES-128 and AES-256 (R2–R6) with user or
  owner password; save with RC4-128, AES-128 or AES-256 (default) and
  permission flags.
- Editing: insert, remove, reorder, duplicate and rotate pages; set media
  box; import pages between documents with all dependencies; metadata.
- Drawing: content-stream builder, standard 14 fonts with AFM metrics,
  embedded TrueType/OpenType fonts with glyph subsetting and ToUnicode,
  JPEG and PNG images with soft masks, opacity.
- Operations: merge, split, extract, page range expressions
  (`1-3,odd,last,r2`), text and image stamps that stay upright on rotated
  pages, page numbers.
- Writer: garbage collection, renumbering, stream recompression,
  deduplication of identical resources, object streams and cross-reference
  streams, deterministic output.
- Batch: JSON presets with `each`/`merge` modes, steps for every operation,
  split to many files, output naming templates; `PresetStore` for saving
  export configurations.
- `folio` command-line tool.
- `foliopdf` npm package (WebAssembly) with TypeScript definitions.
