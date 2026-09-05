# Command line: `folio`

One static binary, no runtime dependencies. Install from the
[releases page](https://github.com/keithadler/foliopdf/releases) or
`cargo install foliopdf-cli`.

Page ranges are **1-based**: `1-3,7`, `4-`, `-2`, `odd`, `even`, `first`,
`last`, `r2` (second from the end). Global options: `--password PW` for
encrypted inputs, `--no-compress` to write an uncompressed file (handy for
inspecting output), `--level N` for the Flate level (1–10, default 6).

## Commands

```
folio info <in.pdf>
```
Pages, sizes, rotation, encryption method and permissions, metadata.

```
folio merge <out.pdf> <in.pdf>...
```
Concatenate in order. Identical fonts and images shared across inputs are
written once.

```
folio images <out.pdf> photo1.jpg scan.png ... [--size a4] [--margin 36] [--dpi 150]
```
One page per JPEG or PNG. Without `--size` each page is the image's own
size at `--dpi`; with a named size the image is fitted inside (turned to
landscape when it is wider than tall).

```
folio split <in.pdf> --every N              [--out-dir D] [--name "{stem}-{index}.pdf"]
folio split <in.pdf> --ranges "1-3" "4-9" "10-"
```

```
folio pages <in.pdf> <out.pdf> --select "1-3,7"      # keep + reorder
folio pages <in.pdf> <out.pdf> --delete "2,4"
```

```
folio rotate <in.pdf> <out.pdf> --degrees 90 [--pages odd]
```

```
folio resize <in.pdf> <out.pdf> --size a4 [--mode fit|fill|stretch] [--pages all]
folio resize <in.pdf> <out.pdf> --size 612x792
folio resize <in.pdf> <out.pdf> --scale 0.5
```
Changes the page box and scales the content (and annotations) to match.
Named sizes: `letter`, `legal`, `tabloid`, `a3`, `a4`, `a5`, each optionally
with `-landscape`; or `WxH` in points. `fit` (default) keeps everything and
centres it, `fill` crops the overflow, `stretch` distorts to fill exactly.
`--scale` multiplies both the page and its content.

```
folio crop <in.pdf> <out.pdf> --box 36,36,540,720 [--pages all]
folio crop <in.pdf> <out.pdf> --reset
```
Sets the crop box (x, y, width, height in points from the bottom-left of
the displayed page); readers and printers show only that area. The
content outside stays in the file; `redact` removes it. `--reset` shows
the whole page again.

```
folio bookmarks <in.pdf> [--json]
folio bookmarks <in.pdf> <out.pdf> --set tree.json
folio bookmarks <in.pdf> <out.pdf> --clear
```
Lists or replaces the bookmarks (the outline in the reader's sidebar).
`tree.json` is an array of `{ "title", "page" (0-based), "top"?, "uri"?,
"open"?, "children"? }` objects, the same shape `--json`
prints. Bookmarks survive `merge`, `split` and `pages --select`.

```
folio reverse <in.pdf> <out.pdf>
folio blank <in.pdf> <out.pdf> [--at N] [--count 1] [--size a4]
```
`blank` inserts empty pages before page N (omit `--at` to append). Without
`--size` the new pages match their neighbour.

```
folio compress <in.pdf> <out.pdf> [--level 9] [--strip-metadata]
folio compress <in.pdf> <out.pdf> --images [--dpi 150] [--quality 75] [--keep-lossless]
```
Recompresses streams, packs objects, drops unreferenced and duplicate
objects. Already-optimised files may not shrink. With `--images`, grey and
RGB images displayed above `--dpi` are downsampled to it and re-encoded as
JPEG at `--quality`, which is what makes scans and photo-heavy files
small (lossy). `--keep-lossless` leaves Flate images alone.

```
folio encrypt <in.pdf> <out.pdf> [--user PW] [--owner PW] [--method aes256|aes128|rc4]
              [--no-print] [--no-copy] [--no-modify] [--no-annotate] [--no-forms] [--no-assemble]
```
At least one password is required. With only `--owner`, anyone can open the
file but the permission flags apply. AES-256 is the default and is what
every current reader expects; use `aes128` or `rc4` only for very old
software.

```
folio decrypt <in.pdf> <out.pdf> --password PW
```

```
folio stamp <in.pdf> <out.pdf> --text "DRAFT" [--size 60] [--opacity 0.3] [--rotation 45]
            [--position center] [--color 0.5,0.5,0.5] [--font Helvetica] [--pages all] [--under] [--margin 36]
folio stamp <in.pdf> <out.pdf> --image logo.png [--width 120] [--height H] [--position bottom-right] [--opacity 1]
```
Positions: `top-left top-center top-right center-left center center-right
bottom-left bottom-center bottom-right`. `{page}` and `{pages}` in `--text`
are substituted. Fonts: the standard 14 by name (`Helvetica-Bold`,
`Times-Roman`, `Courier`, ...; `Arial` and `Times New Roman` aliases work).

```
folio numbers <in.pdf> <out.pdf> [--format "{page} / {pages}"] [--position bottom-center]
              [--size 10] [--start 1] [--pages all]
```

```
folio meta <in.pdf> <out.pdf> [--title T] [--author A] [--subject S] [--keywords K] [--creator C]
```

```
folio batch <preset.json> <in.pdf>... [--out-dir D] [--asset name=path]... [--preset NAME]
```
Runs a preset (see [presets.md](presets.md)). The JSON file may be a single
preset or a store exported with `folio presets export`, in which case
`--preset NAME` picks one. `--asset` supplies images for `stamp-image` steps.

```
folio presets
folio presets export presets.json
```
List the built-in presets, or write them all to a store file you can edit
and reuse.

```
folio fields <in.pdf> [--json]
```
Lists every form field: name, type, page, value, choices and flags.

```
folio fill <in.pdf> <out.pdf> --set name=value --set agree=true [--data values.json] [--flatten]
```
Fills fields by name. `true`/`false` (also `yes`/`no`, `on`/`off`) set
check boxes; anything else is text, a drop-down's export value, or a radio
button's choice. `--data` takes a JSON object (strings, booleans, or
arrays for multi-select lists). Unknown names are reported and skipped.
`--flatten` makes the result permanent.

```
folio annots <in.pdf> [--json]
```
Lists annotations with page, type, author, bounds and comment text.

```
folio flatten <in.pdf> <out.pdf> [--forms | --annots] [--pages all]
```
Paints annotations into the page content and removes them. `--forms` does
only form fields, `--annots` only comments and markup; the default does both.

```
folio text <in.pdf> [--pages 1-3] [--out file.txt]
```
Extracts the text, lines top to bottom, pages separated by a form feed.

```
folio search <in.pdf> "needle" [--pages all] [-i] [--word] [--json]
```
Lists matches with page and position (points from the bottom-left of the
page as displayed). `-i` ignores case, `--word` matches whole words.

```
folio redact <in.pdf> <out.pdf> --text "needle" [--text "other"] [-i] [--word] [--pages all]
folio redact <in.pdf> <out.pdf> --area 2:72,700,200,20 [--no-fill] [--color 1,1,1]
```
Removes for good whatever is under each match or area: the text (so a
later search finds nothing), vector graphics inside the area, and the
pixels of images under it (undecodable image formats are removed whole and
reported). Then paints a black box, unless `--no-fill` or another
`--color`. `--area` takes `PAGE:x,y,width,height` in points from the
bottom-left of the displayed page. Consider `--strip-metadata` as well.

## Exit status and output

`0` on success. Errors go to stderr prefixed with `folio:` and exit `1`.
Progress lines (files written, sizes) also go to stderr so stdout stays
clean for `info` and `presets`.

## Examples

```bash
# Combine scans, add page numbers, lock against copying
folio merge all.pdf scan-*.pdf
folio numbers all.pdf numbered.pdf --format "Page {page} of {pages}"
folio encrypt numbered.pdf final.pdf --owner "$(openssl rand -hex 12)" --no-copy --no-modify

# One file per page, named after the source
folio split contract.pdf --every 1 --out-dir pages/ --name "contract-p{index}.pdf"

# Same export every time
folio presets export presets.json      # edit, add your own
folio batch presets.json --preset draft-watermark ~/Reports/*.pdf --out-dir ~/Reports/draft/
```
