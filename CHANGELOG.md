# Changelog

All notable changes to this project are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/) and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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
