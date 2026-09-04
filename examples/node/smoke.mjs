// Node smoke test for the web-target package: instantiate from bytes, create,
// edit, encrypt, batch. Run: node examples/node/smoke.mjs
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const here = path.dirname(fileURLToPath(import.meta.url));
const pkg = path.join(here, "../../pkg/foliopdf.js");
const wasm = await readFile(path.join(here, "../../pkg/foliopdf_bg.wasm"));
const mod = await import(pkg);
await mod.default({ module_or_path: wasm });
const { PdfDocument, PresetStore, runBatch, merge, version, parsePageRanges } = mod;

console.log("foliopdf", version());

const doc = new PdfDocument();
doc.addPage(612, 792);
doc.addPage(612, 792);
doc.addPage(595.28, 841.89);
doc.setMetadata({ title: "Made in Node", author: "smoke" });
doc.stampText(null, { text: "DRAFT", rotation: 45, opacity: 0.3, size: 72 });
doc.addPageNumbers(null, {});
doc.rotatePages("2", 90);
const bytes = doc.save({ encryption: { userPassword: "", ownerPassword: "owner" } });
console.log("saved", bytes.length, "bytes, pages:", doc.pageCount());

const back = PdfDocument.load(bytes);
console.log("reloaded pages:", back.pageCount(), "encrypted:", back.wasEncrypted(), back.encryptionDescription());
console.log("metadata:", back.metadata());
console.log("page 2:", back.pages()[1]);
if (!back.pageContent(0).includes("(DRAFT) Tj")) throw new Error("stamp missing");

const merged = merge([bytes, bytes]);
console.log("merged pages:", merged.pageCount());
console.log("ranges:", Array.from(parsePageRanges("odd,last", 6)));

const store = PresetStore.withBuiltins();
store.add({ name: "my-export", steps: [{ op: "strip-metadata" }, { op: "split", every: 2 }], output: { filename: "{stem}-{index}.pdf" } });
const json = store.toJson();
const store2 = PresetStore.fromJson(json);
console.log("presets:", store2.names());

const result = runBatch(store2.get("my-export"), [{ name: "node.pdf", data: bytes }], []);
console.log("batch outputs:", result.outputs.map((o) => `${o.name} (${o.pages}p, ${o.bytes}B)`), "warnings:", result.warnings);
if (result.outputs.length !== 2) throw new Error("expected 2 outputs");

try {
  PdfDocument.load(new Uint8Array(100));
} catch (e) {
  console.log("expected error:", e.message);
}
console.log("OK");
