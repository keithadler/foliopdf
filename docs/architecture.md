# Architecture

foliopdf is a workspace of three crates. The core crate does all the work
and knows nothing about files, browsers or terminals.

```
             ┌──────────────────┐   ┌──────────────────┐
             │  foliopdf-wasm   │   │  foliopdf-cli    │
             │  wasm-bindgen    │   │  `folio` binary  │
             └────────┬─────────┘   └────────┬─────────┘
                      └──────────┬───────────┘
                          ┌──────┴──────┐
                          │  foliopdf   │  bytes in, bytes out
                          └─────────────┘
```

## Core crate modules

| Module | Responsibility |
|---|---|
| `object` | The eight PDF object types plus `ObjRef`, `Dict`, `Stream`. Plain values, no interior mutability. |
| `lexer` | Tokeniser over a byte slice. Lenient about malformed numbers and stray delimiters. |
| `parser` | Objects, indirect objects, streams with wrong `/Length`; cross-reference tables and streams; `/Prev` and `/XRefStm` chains; reconstruction by scanning for `N G obj`. |
| `filters` | Flate, LZW, ASCIIHex, ASCII85, RunLength, predictors. Image codecs are recognised and passed through. |
| `crypto` | Standard security handler R2–R6 for reading; RC4-128, AES-128, AES-256 for writing. |
| `document` | `Document`: eager object store, page tree, inheritable attributes, editing, page import, drawing resources, save orchestration. |
| `writer` | Garbage collection, deduplication, renumbering, recompression, object streams, xref streams, encryption on output. |
| `content` | Builder for content-stream operators with compact number formatting. |
| `geometry` | `Point`, `Rect`, `Matrix`. |
| `page` | `PageInfo`, `PageSize`, `Rotation`, and the display-to-user matrix that keeps stamps upright on rotated pages. |
| `font` | Standard 14 metrics and WinAnsi; TrueType parsing, cmap, subsetting, CID font dictionaries, ToUnicode. |
| `image` | JPEG header parsing (pass-through) and PNG decoding (re-encoded as Flate with soft mask). |
| `ops` | Page ranges, merge, split, stamps, page numbers, rotate, delete. |
| `batch` | `Preset`, `Step`, `run`, `PresetStore`. |

## Loading

1. Read the `%PDF-x.y` header (tolerating junk before it).
2. Find `startxref` near the end and follow the chain of cross-reference
   sections. Tables and streams are handled uniformly; the newest definition
   of each object wins, `/XRefStm` hybrid streams are honoured.
3. Parse every object the xref names. Object offsets are verified: if an
   offset does not point at `N G obj` for the expected `N`, the xref is
   treated as unreliable.
4. If anything in steps 2–3 fails, **reconstruct**: scan the whole file for
   `N G obj` headers and `trailer` dictionaries, later definitions winning.
   Object streams found this way are expanded too, and if no trailer names a
   `/Root`, any `/Type /Catalog` object is used.
5. If the trailer has `/Encrypt`, authenticate with the password (empty by
   default), then decrypt every string and stream **in memory**. From this
   point on the document is plaintext; encryption is purely a save option.
6. Expand object streams into individual objects; drop the now-redundant
   `ObjStm` and `XRef` objects. They are regenerated on save.

Everything is loaded eagerly. Streams keep their encoded bytes, so memory is
roughly the file size plus the parsed dictionaries. A 200-page text document
parses in half a millisecond; the corpus of 452 files (211 MB) averages 25 ms
per file including the re-save.

## Editing

Objects live in a `BTreeMap<u32, Object>`. `Document::get` returns `&Object`
(with `null` for dangling references), `resolve` follows references, and
`get_mut`/`add`/`set` mutate. Higher-level methods keep the page tree
consistent.

Structural page edits (insert, remove, reorder) first **flatten** the page
tree into one `/Pages` node with every page as a direct kid, pushing
inherited attributes (`Resources`, `MediaBox`, `CropBox`, `Rotate`) down onto
the pages. This is simpler and far more robust than surgery on a balanced
tree, and a flat array of tens of thousands of kids costs nothing.

**Importing pages** between documents is an iterative deep copy with a
worklist. Every object reachable from the chosen pages is copied and
renumbered. References to pages that were *not* chosen, and to any `/Pages`
node, become `null` so a single page never drags the entire source tree
along. Outlines and AcroForm field trees are not copied (the widget
annotations on the pages are).

**Drawing** appends a new content stream. The existing content is wrapped
as `[ "q", ...existing, "Q" + new ]` so state set by the original content
(transforms, clips, colours) cannot affect the stamp, and vice versa.
Stamps are positioned in *display* coordinates and mapped through
`PageInfo::display_to_user`, which accounts for `/Rotate` and the crop box,
so a "bottom-right" logo is bottom-right on screen whatever the page's
internal orientation.

## Saving

Saving is always a full rewrite. Incremental updates were deliberately not
implemented: they are the main reason PDFs in the wild are bloated and
inconsistent, and a full rewrite is fast enough to be free.

1. Registered fonts are finalised: glyphs used by `Font::encode` are subset
   and the font dictionaries generated.
2. **Reachability**: walk from `/Root` and `/Info`; anything unreachable is
   dropped.
3. **Deduplication** (when `compress` is on): streams and font-related
   dictionaries with byte-identical serialisation are merged, to a fixed
   point. Merging five documents that embed the same font yields one copy.
4. **Renumbering**: reachable objects get consecutive numbers starting at 1.
5. **Recompression**: every stream whose filters are all lossless is decoded
   and re-encoded with Flate at the chosen level, keeping the original when
   that is smaller. Image codecs and XMP metadata are left alone.
6. **Object streams**: non-stream objects are packed 200 per `ObjStm`, and
   the file ends with a cross-reference stream. Without `compress`, a classic
   xref table is written instead (useful for debugging with a text editor).
7. **Encryption**: if requested, a fresh file key and salts are generated,
   the `/Encrypt` dictionary is built, and every object is encrypted with its
   *new* object number as it is written. The `/ID` array always gets a fresh
   second element.

Output is deterministic apart from the random `/ID` and encryption
material: dictionaries are written in sorted key order and numbers in a
canonical short form.

## Error handling

Every fallible path returns `Result<_, Error>`. The reader is lenient by
policy (it accepts what viewers accept) but never panics on hostile input:
nesting depth is capped, image dimensions are capped, xref chains are
cycle-guarded, and truncated Flate streams yield what could be recovered.

## Dependencies

| Crate | Why |
|---|---|
| `miniz_oxide` | Flate (pure Rust, `no_std`-friendly, fast) |
| `md-5`, `sha2`, `aes`, `cbc` | RustCrypto primitives for the security handler |
| `getrandom` | OS or browser randomness (`js` feature for wasm) |
| `serde`, `serde_json` | Presets and option structs |
| `thiserror` | Error type derivation |

No C code anywhere, so the same source builds for every target Rust does.

## Added in 0.2

| Module | Role |
|---|---|
| `cstream` | Tolerant content-stream parser and writer; inline images are kept as opaque byte runs. |
| `text` | Content interpreter: graphics and text state, font decoding (encodings, Differences, ToUnicode, CMaps, W arrays, Type3), form XObjects; glyph positions with stream locations; lines, words, search. |
| `redact` | Rewrites text operators glyph by glyph, drops covered paths, blanks image pixels (`imgcodec`), copies and rewrites forms, removes annotations, paints boxes. |
| `annot` | Annotation dictionaries plus generated appearance streams; listing, removal, flattening (the §12.5.5 form-to-rect mapping). Display-space geometry. |
| `forms` | AcroForm walking (inherited attributes, orphaned widgets), appearance generation from `DA`/`MK`, field creation, pruning; `Document::import_pages` calls back into it. |
| `outline` | Bookmark tree read/write; destinations resolved through `/Dests` and the names tree; page maps for import and selection. |
| `compress` | Lossy image pipeline: display-size analysis via `text`, area-averaged resampling, JPEG re-encoding. |
| `imgcodec` | Raw/Flate/JPEG image sample access shared by `redact` and `compress` (`jpeg` feature: jpeg-decoder, jpeg-encoder). |
| `glyphlist` | Glyph-name and encoding tables. |

The web editor (`examples/web/editor.js`) draws pages with pdf.js and
keeps everything the user adds as plain items in page points; only on
save does it call the engine (`addAnnotation`, `setFields`, `redact`,
`cropPages`, …), so the engine never depends on the browser.
