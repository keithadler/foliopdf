# Batch presets

A preset is a JSON document describing a repeatable export: how inputs are
combined, which steps run in order, and how outputs are written. Presets are
plain data, so they can be stored anywhere (a file, `localStorage`, a row in
a database) and replayed with `folio batch`, `runBatch()` in JavaScript, or
`batch::run` in Rust.

```json
{
  "schema": 1,
  "name": "client-pack",
  "description": "Merge, watermark, number, lock.",
  "mode": "merge",
  "steps": [
    { "op": "rotate", "pages": "even", "degrees": 90 },
    { "op": "stamp-text", "text": "CONFIDENTIAL", "rotation": 45, "opacity": 0.2, "size": 72 },
    { "op": "stamp-image", "asset": "logo", "position": "top-right", "width": 90 },
    { "op": "page-numbers", "format": "{page} / {pages}" },
    { "op": "metadata", "title": "Client pack", "author": "Ops" }
  ],
  "output": {
    "filename": "{stem}-{n}.pdf",
    "compress": true,
    "compressionLevel": 6,
    "encryption": { "userPassword": "", "ownerPassword": "s3cret", "method": "aes-256",
                    "permissions": { "copy": false, "modify": false } }
  }
}
```

## Top level

| Field | Type | Default | Meaning |
|---|---|---|---|
| `schema` | number | `1` | Schema version. Only `1` exists. |
| `name` | string | required | Unique name; the key in a `PresetStore`. |
| `description` | string | – | For UIs. |
| `mode` | `"each"` \| `"merge"` | `"each"` | `each`: run the steps on every input separately. `merge`: concatenate all inputs first, then run the steps once. |
| `steps` | `Step[]` | `[]` | Executed in order. |
| `output` | object | see below | How files are written. |

## Steps

Every step has an `op`. Where a step takes `pages`, it is a 1-based range
expression (`"1-3,7"`, `"odd"`, `"last"`, `"r2"`), and omitting it means
all pages.

| `op` | Fields | Effect |
|---|---|---|
| `select-pages` | `pages` | Keep only these pages, in this order. Repeats duplicate the page. |
| `delete-pages` | `pages` | Remove these pages. |
| `rotate` | `pages?`, `degrees` | Rotate by a multiple of 90 (clockwise; negative allowed). |
| `stamp-text` | `pages?`, `text`, `font?`, `size?`, `color?`, `opacity?`, `position?`, `rotation?`, `margin?`, `under?` | Text watermark or label. `{page}`/`{pages}` substituted. Defaults: Helvetica 36 pt grey, 50 % opacity, centred. |
| `stamp-image` | `pages?`, `asset`, `width?`, `height?`, `opacity?`, `position?`, `margin?`, `under?` | Draw a JPEG/PNG supplied as a named asset. Missing width/height keep the aspect ratio; images never exceed the page. |
| `page-numbers` | `pages?`, `format?`, `position?`, `font?`, `size?`, `margin?`, `color?`, `startAt?` | Default `"{page} / {pages}"`, bottom centre, 10 pt. `{page:6}` zero-pads (Bates numbering). |
| `metadata` | `title?`, `author?`, `subject?`, `keywords?`, `creator?`, `producer?` | Omitted fields untouched; `""` removes a field. |
| `strip-metadata` | – | Remove XMP, the info dictionary and page thumbnails. |
| `resize` | `pages?`, `size` or `width`+`height`, `mode?` | Change the page size, scaling content to match. `size` is `letter`, `legal`, `tabloid`, `a3`, `a4`, `a5` (add `-landscape` to turn it); `width`/`height` are points. `mode`: `fit` (default, keeps everything), `fill` (crops), `stretch`. |
| `scale` | `pages?`, `factor` | Multiply the page size and its content; `0.5` halves. |
| `reverse` | – | Reverse the page order. |
| `compress-images` | `maxDpi?`, `quality?`, `convertLossless?`, `minPixels?` | Lossy: downsample images shown above `maxDpi` (default 150) and re-encode grey/RGB images as JPEG at `quality` (default 75). CMYK, indexed, 1-bit and JPEG 2000 images are kept. |
| `flatten` | `forms?`, `annotations?` | Paint form fields and/or comments into the pages and remove them (both by default). |
| `nup` | `perSheet` (2 or 4), `sheet?`, `landscape?`, `margin?`, `frames?` | Several pages per sheet. |
| `booklet` | `sheet?`, `landscape?`, `margin?`, `frames?` | Fold-in-half order, two pages per sheet. |
| `blank-pages` | `at?`, `count?`, `size?` | Insert `count` (default 1) empty pages before 1-based page `at` (omit to append). `size` as for `resize`; default matches the neighbouring page. |
| `split` | `every?` or `ranges?` | Produce several files. Must be the **last** step. |

`position` is one of `top-left top-center top-right center-left center
center-right bottom-left bottom-center bottom-right`, always relative to
the page as displayed (rotation and crop box are taken into account).
`color` is `[r, g, b]` in 0–1. `under: true` paints beneath the existing
content (for paper-like backgrounds).

## Output

| Field | Default | Meaning |
|---|---|---|
| `compress` | `true` | Recompress streams, deduplicate, pack objects. |
| `compressionLevel` | `6` | Flate level 1–10. 9–10 are noticeably slower for a few percent. |
| `objectStreams` | `true` | Use object and cross-reference streams (PDF 1.5+). |
| `encryption` | none | See below. |
| `filename` | `"{stem}.pdf"` | Template. `{stem}` = input name without extension (for `merge`, first input's stem plus `-merged`); `{index}`/`{total}` = part number within a split; `{n}` = running output number. If a split produces several files and the template has no `{index}` or `{n}`, `-1`, `-2`, … are appended automatically. |

### Encryption

| Field | Default | Meaning |
|---|---|---|
| `userPassword` | `""` | Needed to open. Empty means anyone can open. |
| `ownerPassword` | `""` | Unlocks permissions. Empty means same as user password. |
| `method` | `"aes-256"` | `"aes-256"`, `"aes-128"`, `"rc4-128"`. |
| `permissions` | all `true` | `print`, `modify`, `copy`, `annotate`, `fillForms`, `accessibility`, `assemble`, `printHighQuality`. |
| `encryptMetadata` | `true` | Whether XMP metadata is encrypted too. |

## Inputs and assets

Inputs are `{ name, data, password? }`. Assets are `{ name, data }` and are
referenced by `stamp-image` steps. Both are supplied at run time, not
stored in the preset, so a preset never contains file contents.

## Results

`run` returns `outputs` (each `{ name, data, pages, bytes, sources }`) and
`warnings` (non-fatal notes such as "cross-reference table was damaged and
has been rebuilt"). A failing input aborts the run with an error naming the
file.

## Validation

`Preset::from_json` / `validatePreset` reject: unknown `schema`, empty
`name`, `degrees` not a multiple of 90, `opacity` outside 0–1, empty stamp
text, `split` that is not last or lacks `every`/`ranges`, `compressionLevel`
outside 1–10, and any unknown `op` or field type (serde is strict about
types but ignores unknown keys).

## Built-in presets

`compress`, `merge`, `encrypt-aes256`, `draft-watermark`,
`split-single-pages`. Export them with `folio presets export presets.json`
or `PresetStore.withBuiltins().toJson()` and use them as starting points.
