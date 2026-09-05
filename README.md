# foliopdf

Fast, portable PDF editing. Pure Rust, compiles to WebAssembly, MIT licensed.

Open PDFs (including damaged and encrypted ones), then fill forms, sign,
annotate, merge, split, reorder, rotate, resize, stamp, compress and encrypt
them, and write clean compact output. Runs in browsers, Node, Deno, Bun and
natively on Linux, macOS and Windows.

- **Web app**: [keithadler.github.io/foliopdf](https://keithadler.github.io/foliopdf/): fill & sign, fill forms, comment, redact, OCR scans, compare versions, extract text and images, merge, split, compress, protect, organize, booklets, headers and Bates numbers and more, all in the browser, nothing uploaded.
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

### Install

**Web app**: nothing to install, open
[keithadler.github.io/foliopdf](https://keithadler.github.io/foliopdf/)
(or install it as an app from there, see above).

**CLI**: download `folio` for your platform from the
[releases page](https://github.com/keithadler/foliopdf/releases) (Linux
x86-64 and arm64, macOS Apple silicon and Intel, Windows), or build it:

```bash
cargo install --git https://github.com/keithadler/foliopdf foliopdf-cli
```

**JavaScript / TypeScript**: the package is not on the npm registry yet;
install it from the release tarball, which is exactly what `npm publish`
would upload:

```bash
npm install https://github.com/keithadler/foliopdf/releases/download/v1.0.0/foliopdf-1.0.0.tgz
```

The package is an ES module with the `.wasm` file alongside. Bundlers (Vite,
webpack 5, esbuild) handle it directly. In Node, pass the bytes yourself:

```js
import { readFile } from "node:fs/promises";
import init, { PdfDocument } from "foliopdf";
await init({ module_or_path: await readFile("node_modules/foliopdf/foliopdf_bg.wasm") });
```

**Rust**: the crate is not on crates.io yet; use the git dependency:

```toml
[dependencies]
foliopdf = { git = "https://github.com/keithadler/foliopdf", tag = "v1.0.0" }
```

Registry releases (`npm install foliopdf`, `cargo add foliopdf`) will
follow once publishing is set up; the version numbers will match.

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

## How it is tested

- **Unit and round-trip tests** run on every push on Linux, macOS and
  Windows (`cargo test --workspace`: 113 tests covering the parser,
  writer, encryption, fonts, forms, annotations, text engine, redaction,
  image recompression and bookmarks), plus clippy with warnings as errors,
  a minimum-Rust-version build, and a Node smoke test of the npm package.
- **A corpus of real files.** Before a release the engine is run over a
  private corpus of 470+ PDFs (11,600 pages) from dozens of producers:
  Word, Chrome, Quartz, Qt, WeasyPrint, DocuSign, Acrobat, iText, Google
  Docs, scanners. Every file must load, re-save with compression, and
  reload; every page must extract to the same text before and after
  re-saving; redacting a common word on every page must leave zero
  matches on re-reading; recompressing images must produce files that
  reload. The 0.2.0 runs: 0 load or save failures, 0 text changes after
  re-save, 205,292 redaction matches with 0 leftovers, 582 images
  recompressed with 0 failures.
- **Rendering checks.** Generated forms, annotations, signatures,
  flattened output and redactions are rendered with pdf.js (a separate
  code base) and inspected page by page; the web app has a browser test
  harness that drives every tool end to end.
- **What is not tested automatically:** appearance in Acrobat itself and
  other commercial viewers. The output follows ISO 32000 and is checked
  against pdf.js; please report anything a viewer shows differently.

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
