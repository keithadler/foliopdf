# JavaScript and WebAssembly

The npm package `foliopdf` is the core crate compiled to WebAssembly with
[wasm-bindgen](https://rustwasm.github.io/wasm-bindgen/). It is an ES
module; TypeScript definitions are included. The `.wasm` file is about
780 KB (roughly 300 KB over the wire with Brotli).

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
