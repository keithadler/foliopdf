#!/usr/bin/env sh
# Downloads pdf.js (Apache-2.0, Mozilla) into examples/web/vendor for page
# thumbnails in the web app. The PDF engine itself never depends on it.
set -eu
cd "$(dirname "$0")/.."
VERSION="${PDFJS_VERSION:-6.3.289}"
OUT=examples/web/vendor
mkdir -p "$OUT" tmp-vendor
curl -sSL "https://registry.npmjs.org/pdfjs-dist/-/pdfjs-dist-$VERSION.tgz" | tar xz -C tmp-vendor
cp tmp-vendor/package/build/pdf.min.mjs tmp-vendor/package/build/pdf.worker.min.mjs "$OUT/"
cp tmp-vendor/package/LICENSE "$OUT/LICENSE-pdfjs.txt"
rm -rf tmp-vendor
echo "pdf.js $VERSION -> $OUT"
