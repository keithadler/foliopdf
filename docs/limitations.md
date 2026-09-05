# Limitations

Honest list of what foliopdf does **not** do, so you can pick another tool
when you need one of these.

## Not implemented

- **Rendering** pages to bitmaps. Use pdf.js, MuPDF or PDFium.
- **Text extraction** with layout. `Document::page_content` gives you the
  raw operators, but there is no text-run reconstruction or font decoding.
- **Outlines (bookmarks)**, named destinations and article threads are not
  carried across `import_pages`/merge. They are preserved when you edit a
  single document in place.
- **Cryptographic digital signatures.** Existing signatures are invalidated
  by any save (that is inherent to signing); creating certificate-based
  signatures is out of scope. Drawn, typed or scanned signatures placed as
  images are supported.
- **Non-Latin text in generated appearances.** Filled fields, text boxes
  and stamps are drawn with the standard 14 fonts (WinAnsi); Cyrillic, Greek,
  CJK and other scripts come out as `?`. The Rust API can draw with an
  embedded TrueType font; the form and annotation helpers cannot yet.
- **Rich text** (`/RV`) in fields and free-text annotations is ignored;
  plain text is drawn. XFA forms are not supported (only the AcroForm part).
- **JavaScript** actions in forms (calculations, validation) are not run.
- **Redaction.** Stamps and annotations draw over content; they do not
  remove it.
- **Linearisation** (fast web view). Output is compact but not linearised.
- **Incremental updates.** Every save is a full rewrite. This is deliberate.
- **Public-key encryption** (`/Filter /Adobe.PubSec`). Only the standard
  password security handler is supported.
- **Image transcoding.** JPEG data is embedded as is; PNG is decoded and
  stored as Flate. There is no downsampling or re-encoding of existing
  images, so scanned PDFs do not shrink beyond what Flate offers.
- **Interlaced (Adam7) PNG** input is rejected; re-save the image.
- **Type 1 / CFF font subsetting.** CFF-flavoured OpenType fonts are embedded
  whole. TrueType outlines are subset.
- **Fonts for stamps** are limited to the standard 14 in the CLI and presets;
  the Rust API accepts any TrueType/OpenType file.

## Leniencies worth knowing

- Damaged files are reconstructed by scanning. When two definitions of an
  object exist, the one later in the file wins, matching incremental-update
  semantics. In rare pathological files this can pick a stale object.
- Truncated Flate streams yield the recoverable prefix rather than an error.
- Encrypted files are opened with the empty user password when none is
  given, which is how most "protected" files in circulation behave.

## Platform notes

- The core crate needs only `std`; it has no platform-specific code.
- Randomness comes from `getrandom`; on exotic targets you may need to
  enable a backend feature for that crate.
- WebAssembly builds are single-threaded.
