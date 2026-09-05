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
folio reverse <in.pdf> <out.pdf>
folio blank <in.pdf> <out.pdf> [--at N] [--count 1] [--size a4]
```
`blank` inserts empty pages before page N (omit `--at` to append). Without
`--size` the new pages match their neighbour.

```
folio compress <in.pdf> <out.pdf> [--level 9] [--strip-metadata]
```
Recompresses streams, packs objects, drops unreferenced and duplicate
objects. Already-optimised files may not shrink; scanned images are never
re-encoded (that would be lossy).

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
