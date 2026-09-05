# foliopdf

Fast, portable PDF editing. Pure Rust, compiles to WebAssembly, MIT licensed.

Open PDFs (including damaged and encrypted ones), then fill forms, sign,
annotate, merge, split, reorder, rotate, resize, stamp, compress and encrypt
them, and write clean compact output. Runs in browsers, Node, Deno, Bun and
natively on Linux, macOS and Windows.

- **Web app**: [keithadler.github.io/foliopdf](https://keithadler.github.io/foliopdf/): fill & sign, fill forms, comment and mark up, redact, extract text, merge, split, compress, protect, organize, watermark and more, all in the browser, nothing uploaded.
- **Rust crate** `foliopdf`: the engine, no I/O, no `unsafe`.
- **npm package** `foliopdf`: WebAssembly bindings with TypeScript types.
- **CLI** `folio`: single binary for scripts and shells.

```bash
folio merge out.pdf a.pdf b.pdf c.pdf
folio fill form.pdf done.pdf --set name="Ada Lovelace" --set agree=true --flatten
folio redact report.pdf clean.pdf --text "555-0134" -i
folio encrypt out.pdf locked.pdf --owner s3cret --no-copy --no-modify
folio stamp report.pdf draft.pdf --text DRAFT --rotation 45 --opacity 0.3
folio batch presets.json --preset draft-watermark *.pdf --out-dir out/
```

```ts
import init, { PdfDocument } from "foliopdf";
await init();

const doc = PdfDocument.load(bytes);          // Uint8Array
doc.setField("name", "Ada Lovelace");
doc.addAnnotation(0, { kind: "highlight", quads: [{ x0: 72, y0: 80, x1: 300, y1: 94 }] }, { author: "Ada" });
doc.redactText(null, "555-0134", { caseInsensitive: true }, {});
const text = doc.pageText(0);
doc.deletePages("2,5-7");
doc.stampText(null, { text: "CONFIDENTIAL", rotation: 45, opacity: 0.25 });
doc.addPageNumbers(null, { format: "{page} / {pages}" });
const out = doc.save({ encryption: { ownerPassword: "s3cret", permissions: { copy: false } } });
```

```rust
use foliopdf::{Document, SaveOptions, EncryptionOptions, ops};

let mut doc = Document::load(&std::fs::read("in.pdf")?)?;
ops::stamp_text(&mut doc, &[0], &ops::TextStamp::watermark("DRAFT"))?;
let out = doc.save(&SaveOptions {
    encryption: Some(EncryptionOptions::new("", "owner-password")),
    ..Default::default()
})?;
```

## Why another PDF library

Most open-source PDF editing runs either in a browser (JavaScript, slow on
big files) or on a server (C libraries with awkward bindings). foliopdf is
one engine that runs everywhere at native speed, with the same behaviour in
the browser as on the command line, and no files ever leaving the machine.

It reads what real-world producers write (Word, Chrome, WeasyPrint,
DocuSign, Acrobat, iText, Google Docs, scanners) and writes what the
standard says. Every save is a full rewrite: unreferenced objects are
dropped, identical resources are deduplicated, streams are recompressed,
and the result has one clean cross-reference section.

## Features

| Area | What you get |
|---|---|
| Reading | PDF 1.0–2.0, xref tables and streams, object streams, incremental updates, hybrid files; reconstruction by scanning when the xref is missing or lies |
| Filters | Flate, LZW, ASCIIHex, ASCII85, RunLength, PNG/TIFF predictors; image codecs passed through intact |
| Encryption in | RC4 40–128 bit, AES-128, AES-256 (revisions 2–6), user or owner password |
| Encryption out | AES-256 (default), AES-128, RC4-128; full permission flags; optional unencrypted metadata |
| Pages | insert, delete, reorder, duplicate, rotate, reverse, blank pages; resize to A4/Letter/any size or scale, with content and annotations scaled to match; import pages between documents with all their dependencies |
| Merge and split | merge any number of files; split by page count or by ranges; page-range language `1-3,7,odd,last,r2` |
| Forms | list fields (text, check box, radio, drop-down, list, signature), fill them with generated appearances, create and remove fields, flatten; fields survive merging and extraction |
| Annotations | highlight, underline, strike-out, box, circle, line, ink, text box, sticky note, link, image stamp (signatures), each with an appearance stream; list, remove, flatten selectively |
| Text | extract text with positions (simple and composite fonts, encodings, ToUnicode, form XObjects), words and lines in reading order, search with case and whole-word options |
| Redaction | true removal of text, vector graphics and image pixels (raw, Flate and JPEG) under an area or a search term, including inside form XObjects; overlapping annotations removed; boxes painted over |
| Stamps | text watermarks and image logos (JPEG/PNG with alpha) with opacity, rotation and nine anchor positions; page numbers; stamps stay upright on rotated pages |
| Fonts | standard 14 with real metrics; embedded TrueType/OpenType with glyph subsetting and ToUnicode |
| Compression | stream recompression, object streams, cross-reference streams, dedup of identical fonts and images, metadata stripping; optional lossy image downsampling and JPEG re-encoding for scans and photos |
| Batch | JSON presets: merge-or-each modes, ordered steps, output naming templates, encryption; `PresetStore` for saving export configurations |

Not in scope (yet): rendering pages to pixels, cryptographic digital
signatures, OCR. See [docs/limitations.md](docs/limitations.md).

## Install

**CLI**: download a binary from the
[releases page](https://github.com/keithadler/foliopdf/releases), or

```bash
cargo install foliopdf-cli
```

**Rust**:

```toml
[dependencies]
foliopdf = "0.1"
```

**JavaScript / TypeScript**:

```bash
npm install foliopdf
```

The package is an ES module with the `.wasm` file alongside. Bundlers (Vite,
webpack 5, esbuild) handle it directly. In Node, pass the bytes yourself:

```js
import { readFile } from "node:fs/promises";
import init, { PdfDocument } from "foliopdf";
await init({ module_or_path: await readFile("node_modules/foliopdf/foliopdf_bg.wasm") });
```

## Performance

`cargo bench -p foliopdf` on an Apple M-series laptop, 200-page text document
(115 KB uncompressed, 81 KB compressed), single thread:

| Operation | Time | Throughput |
|---|---|---|
| Load (xref table) | 0.47 ms | 235 MB/s |
| Load (object streams) | 0.54 ms | 146 MB/s |
| Load and decrypt AES-256 | 6.8 ms | 13 MB/s |
| Save uncompressed | 0.59 ms | 188 MB/s |
| Save compressed | 6.9 ms | 11.5 MB/s |
| Save compressed and AES-256 encrypted | 27 ms | 3.4 MB/s |
| Merge two 200-page documents | 0.92 ms | 172 MB/s |
| Recover a file with a destroyed xref | 1.0 ms | 111 MB/s |

AES-256 numbers are dominated by the deliberately slow password hash the
standard mandates (revision 6, tens of thousands of AES rounds), not by the
data size; encrypting a 50 MB file costs about the same extra 20 ms.

A local corpus of 452 real PDFs (211 MB, dozens of producers) loads,
re-saves compressed, and reloads in 11.5 seconds total, with output 86% of
the input size. WebAssembly runs at roughly half native speed.

## Documentation

- [Architecture](docs/architecture.md): how the pieces fit, design decisions
- [API guide](docs/api-guide.md): the Rust API by task
- [JavaScript and WebAssembly](docs/wasm.md): browser, Node, Deno, bundlers
- [Command line](docs/cli.md): every `folio` command
- [Batch presets](docs/presets.md): the JSON schema and every step
- [Performance notes](docs/performance.md): what is fast, what is not, and why
- [Limitations](docs/limitations.md): what it does not do
- API reference: [docs.rs/foliopdf](https://docs.rs/foliopdf)

## Project layout

```
crates/foliopdf        core library (parser, writer, crypto, fonts, ops, batch)
crates/foliopdf-wasm   wasm-bindgen bindings + TypeScript types
crates/foliopdf-cli    the `folio` binary
examples/web           the free web app (fill & sign, forms, comments, every tool, preset builder; all in the browser)
examples/node          Node smoke test for the npm package
docs/                  guides
```

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Bug reports with a PDF that
misbehaves are the most valuable thing you can send; see
[SECURITY.md](SECURITY.md) for anything that could be a safety issue.

## Support the project

foliopdf is free and always will be. If it replaced a subscription for you,
a tip is welcome: [Venmo @Keith-Adler-1](https://venmo.com/u/Keith-Adler-1).

## Licence

MIT. See [LICENSE](LICENSE).
