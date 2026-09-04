#!/usr/bin/env sh
# Builds the npm package into ./pkg (ES module, works in browsers, Deno, Bun
# and Node 18+). Requires wasm-pack: https://rustwasm.github.io/wasm-pack/
set -eu
cd "$(dirname "$0")/.."
wasm-pack build crates/foliopdf-wasm --release --target web --out-dir ../../pkg --out-name foliopdf
cp LICENSE pkg/LICENSE
cp README.md pkg/README.md
# wasm-pack names the package after the crate; publish it as plain "foliopdf".
node - <<'JS'
const fs = require("fs");
const p = JSON.parse(fs.readFileSync("pkg/package.json", "utf8"));
p.name = "foliopdf";
p.description = "Fast PDF editing in WebAssembly: merge, split, compress, encrypt, stamp, batch presets.";
p.keywords = ["pdf", "wasm", "webassembly", "merge", "split", "encrypt", "compress", "watermark"];
p.homepage = "https://github.com/keithadler/foliopdf";
p.repository = { type: "git", url: "git+https://github.com/keithadler/foliopdf.git" };
p.bugs = { url: "https://github.com/keithadler/foliopdf/issues" };
p.license = "MIT";
p.type = "module";
p.sideEffects = ["./foliopdf.js", "./snippets/*"];
p.files = Array.from(new Set([...(p.files || []), "foliopdf_bg.wasm", "foliopdf.js", "foliopdf.d.ts", "foliopdf_bg.wasm.d.ts", "LICENSE", "README.md"]));
fs.writeFileSync("pkg/package.json", JSON.stringify(p, null, 2) + "\n");
JS
echo "built pkg/ ($(wc -c < pkg/foliopdf_bg.wasm) bytes of wasm)"
