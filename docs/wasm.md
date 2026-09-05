# JavaScript and WebAssembly

The npm package `foliopdf` is the core crate compiled to WebAssembly with
[wasm-bindgen](https://rustwasm.github.io/wasm-bindgen/). It is an ES
module; TypeScript definitions are included. The `.wasm` file is about
1.5 MB (roughly 550 KB over the wire with Brotli); it includes the text
engine, JPEG codecs and everything else, and loads in well under a second.

## Loading

**Bundlers (Vite, webpack 5, esbuild, Parcel):**

```ts
import init, { PdfDocument, runBatch, PresetStore } from "foliopdf";
await init();   // fetches foliopdf_bg.wasm next to the JS
```

**Plain browser, no bundler:**

```html
<script type="module">
  import init, { PdfDocument } from "./node_modules/foliopdf/foliopdf.js";
  await init();
</script>
```

**Node 18+ / Bun / Deno:** pass the bytes (no `fetch` of local files):

```js
import { readFile } from "node:fs/promises";
import init, { PdfDocument } from "foliopdf";
await init({ module_or_path: await readFile(new URL("foliopdf_bg.wasm", import.meta.resolve("foliopdf"))) });
```

**Web Worker:** the module works unchanged in a worker; do heavy work there
and post the resulting `Uint8Array` back with a transfer list.

## PdfDocument

```ts
const doc = PdfDocument.load(bytes);                       // Uint8Array
const doc = PdfDocument.loadWithPassword(bytes, "secret");
const doc = new PdfDocument();                             // empty

doc.pageCount();
doc.pages();                 // PageInfo[]: { index, mediaBox, cropBox, rotation }
doc.metadata();              // { title, author, ... }
doc.wasEncrypted(); doc.encryptionDescription(); doc.wasReconstructed();

doc.addPage(612, 792);
doc.removePage(0);
doc.movePage(3, 0);
doc.selectPages("1-3,last");          // keep + reorder
doc.deletePages("even");
doc.rotatePages("odd", 90);           // null = all pages
doc.importPages(otherDoc, "1-2", 0);  // range over other; insert index (null = append)

doc.stampText(null, { text: "DRAFT", rotation: 45, opacity: 0.3, size: 72 });
doc.stampImage("1", pngBytes, { width: 120, position: "bottom-left" });
doc.addPageNumbers(null, { format: "{page} / {pages}", position: "bottom-center" });
doc.setMetadata({ title: "Report", keywords: "" });   // "" removes a field
doc.stripMetadata();

const out: Uint8Array = doc.save({
  compress: true,
  compressionLevel: 6,
  encryption: { userPassword: "", ownerPassword: "s3cret", method: "aes-256",
                permissions: { copy: false, modify: false } },
});
```

Errors throw a JavaScript `Error` whose message is the Rust error text, e.g.
`the password does not open this document`.

## Annotations and forms

Geometry is in *screen* coordinates: points from the top-left corner of the
page as displayed, y downwards, so an overlay drawn over a rendered page
maps directly (divide CSS pixels by your zoom factor).

```ts
const id = doc.addAnnotation(0, { kind: "highlight", quads: [{ x0: 72, y0: 80, x1: 300, y1: 94 }] }, { author: "Ada" });
doc.addAnnotation(0, { kind: "free-text", rect: { x0: 72, y0: 120, x1: 300, y1: 160 }, text: "Approved", size: 14, color: [0, 0.4, 0] });
doc.addAnnotation(0, { kind: "ink", paths: [[{ x: 10, y: 10 }, { x: 60, y: 40 }]], color: [0.8, 0, 0], width: 3 });
doc.addAnnotation(0, { kind: "note", at: { x: 400, y: 100 } }, { contents: "Remember this" });
doc.addImageAnnotation(0, { x0: 350, y0: 700, x1: 500, y1: 750 }, signaturePng);   // JPEG or PNG bytes
doc.annotations(0);                                   // AnnotInfo[]: subtype, rect, author, contents, object
doc.removeAnnotation(0, 2);                           // by index
doc.flattenAnnotations(null, { objects: [id] });      // burn selected ones into the page
doc.flattenAnnotations("1-3", {});                    // everything on pages 1–3, widgets included

doc.fields();                                         // Field[]: name, kind, value, options, widgets
doc.setField("name", "Ada Lovelace");
doc.setField("agree", true);
doc.setField("colour", "Blue");                       // radio export value
doc.setFields({ city: "London", toppings: ["ham", "olives"] });   // returns unknown names
doc.flattenFields();                                  // make the form permanent
doc.addField(0, { name: "email", rect: { x0: 72, y0: 80, x1: 300, y1: 104 }, border: [0.5, 0.5, 0.5] });
doc.removeField("email");
```

The web app's Fill & Sign, Fill a form and Comment tools are built on
exactly these calls (see `examples/web/editor.js`).

## Sheets, booklets, images out, OCR text in

```ts
doc.nup(2, { sheet: "letter" });                    // or doc.booklet({ sheet: "a4" })
const images = doc.extractImages(null);            // [{ page, width, height, format, data }]
doc.addTextLayer(0, words);                        // [{ text, rect }] from OCR, screen coordinates
```

The web app's OCR tool renders each page with pdf.js, runs Tesseract.js in
the browser, and hands the words to `addTextLayer`.

## Crop and bookmarks

```ts
doc.cropPages("1-3", { x0: 36, y0: 36, x1: 576, y1: 756 });   // screen coordinates
doc.uncropPages(null);
const tree = doc.bookmarks();                                   // [{ title, page, top, uri, open, style, children }]
doc.setBookmarks([{ title: "Intro", page: 0, children: [{ title: "Scope", page: 0, top: 500 }] }]);
```

## Images to PDF

```ts
import init, { imagesToPdf } from "foliopdf";
const doc = imagesToPdf([jpegBytes, pngBytes], { size: "a4", margin: 36 });   // or {} for image-sized pages
doc.addImagePage(moreBytes, {});
```

## Smaller images

```ts
const r = doc.compressImages({ maxDpi: 150, quality: 75 });   // lossy; then doc.save({ compress: true })
```

## Text and redaction

```ts
doc.pageText(0);                                             // string
doc.pageWords(0);                                            // [{ text, rect, line }]
doc.search(0, "total", { caseInsensitive: true });           // [{ text, rects: Rect[], line }]
const report = doc.redact(0, [{ x0: 72, y0: 80, x1: 300, y1: 100 }], { fill: [0, 0, 0] });
const r2 = doc.redactText(null, "555-0134", { caseInsensitive: true }, {});   // all pages; r2.matches
doc.stripMetadata();                                         // if the document info is sensitive too
```

`redact` removes what is under the rectangles (text, vector graphics, image
pixels, annotations) and paints them over; it is not a cover-up. See the
API guide for the details and limitations.

## Merge

```ts
import { merge } from "foliopdf";
const doc = merge([bytesA, bytesB, bytesC]);   // Uint8Array[]
```

or `docA.importPages(docB)` for control over where pages land.

## Batch and presets

```ts
import { runBatch, builtinPresets, validatePreset, PresetStore } from "foliopdf";

const preset = {
  name: "client-export",
  mode: "merge",
  steps: [
    { op: "stamp-text", text: "CONFIDENTIAL", rotation: 45, opacity: 0.2 },
    { op: "page-numbers" },
    { op: "metadata", title: "Client pack" },
  ],
  output: { filename: "{stem}.pdf", encryption: { ownerPassword: "x", permissions: { copy: false } } },
};
validatePreset(preset);                                   // throws if invalid
const { outputs, warnings } = runBatch(preset,
  [{ name: "a.pdf", data: bytesA }, { name: "b.pdf", data: bytesB, password: "pw" }],
  [{ name: "logo", data: pngBytes }]);
for (const o of outputs) download(o.name, o.data);       // o.pages, o.bytes, o.sources

// Persist export configurations
const store = PresetStore.withBuiltins();
store.add(preset);
localStorage.setItem("foliopdf.presets", store.toJson());
const restored = PresetStore.fromJson(localStorage.getItem("foliopdf.presets"));
restored.names();  restored.get("client-export");  restored.remove("compress");
```

The preset schema is documented in [presets.md](presets.md); the TypeScript
`Preset` and `Step` types in the package are the authoritative shapes.

## Memory

`PdfDocument` objects hold Rust memory. Call `doc.free()` when you are done
with one in long-running pages, or let it be collected (wasm-bindgen
registers a `FinalizationRegistry` where available). Byte arrays returned by
`save` are ordinary JavaScript `Uint8Array`s.

## Building from source

```bash
cargo install wasm-pack
./scripts/build-wasm.sh       # -> ./pkg
node examples/node/smoke.mjs  # sanity check
```

`examples/web/index.html` is the complete web app (merge, split, compress,
protect, unlock, rotate, organize, watermark, page numbers, info, batch).
Page thumbnails in the app are drawn by [pdf.js](https://mozilla.github.io/pdf.js/)
(Apache-2.0), fetched into `examples/web/vendor` by `scripts/fetch-vendor.sh`; the
editing engine never depends on it and the app degrades to numbered tiles without
it. It is deployed to https://keithadler.github.io/foliopdf/ by
`.github/workflows/pages.yml` on every push to `main`. Locally, build `pkg/`
and serve the repo root (`python3 -m http.server 8765`), then open
`/examples/web/` (a `pkg` symlink there points at the build). It is a single
file with no build step; host it anywhere by placing `index.html` next to a
`pkg/` folder containing `foliopdf.js` and `foliopdf_bg.wasm`.
