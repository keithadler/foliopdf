# Rust API guide

Full reference: [docs.rs/foliopdf](https://docs.rs/foliopdf). This page is
organised by task.

## Open and save

```rust
use foliopdf::{Document, LoadOptions, SaveOptions};

let bytes = std::fs::read("in.pdf")?;
let mut doc = Document::load(&bytes)?;                       // empty password tried
let mut doc = Document::load_with(&bytes, &LoadOptions::with_password("pw"))?;

doc.was_encrypted();             // input had /Encrypt
doc.encryption_description();    // Some("AES-256")
doc.has_owner_access();          // opened with owner password (or not encrypted)
doc.was_reconstructed();         // xref had to be rebuilt

let out = doc.save(&SaveOptions::default())?;               // compressed, object streams
let out = doc.save(&SaveOptions { compress: false, object_streams: false, ..Default::default() })?;
```

`SaveOptions` fields: `compress`, `compression_level` (1–10), `object_streams`,
`strip_metadata`, `encryption`, `producer`.

## Inspect

```rust
doc.page_count();
for p in doc.pages() {                      // Vec<PageInfo>
    println!("{} {}x{} rot {}", p.index, p.display_width(), p.display_height(), p.rotation.degrees());
}
let info = doc.page_info(0)?;               // media_box, crop_box, rotation, obj
let m = doc.metadata();                     // title, author, subject, keywords, creator, producer
let content = doc.page_content(0)?;         // decoded, concatenated content stream bytes
```

## Pages

```rust
use foliopdf::{PageSize, Rotation, Rect};

doc.add_page(PageSize::A4);                 // append blank
doc.insert_page(0, Dict::new().with("MediaBox", PageSize::LETTER.rect().to_object()))?;
doc.remove_page(3)?;
doc.move_page(5, 0)?;
doc.select_pages(&[2, 0, 0])?;              // keep + reorder; repeats become copies
doc.rotate_page(0, 90)?;                    // relative
doc.set_page_rotation(0, Rotation::R0)?;    // absolute
doc.set_media_box(0, Rect::new(0.0, 0.0, 300.0, 400.0))?;

// Copy pages from another document (fonts, images, annotations come along).
let other = Document::load(&other_bytes)?;
doc.import_pages(&other, &[0, 1], Some(0))?;   // insert at front; None = append

// Resize, scale, reverse, insert blank pages (in `ops`).
use foliopdf::ops::{self, FitMode};
ops::resize_pages(&mut doc, &[0, 1], PageSize::A4, FitMode::Fit)?;   // content scaled to fit
ops::scale_pages(&mut doc, &[2], 0.5)?;                             // page and content halved
ops::reverse_pages(&mut doc)?;
ops::insert_blank_pages(&mut doc, 0, 2, PageSize::LETTER)?;         // two blank pages at the front
```

## Page ranges

```rust
use foliopdf::ops::parse_page_ranges;
let idx = parse_page_ranges("1-3,7,odd,last,r2", doc.page_count())?;   // 0-based indices
```

Grammar: `N`, `N-M`, `N-`, `-M`, `all`, `first`, `last`, `even`, `odd`, `rN`
(N-th from the end). Order is preserved; duplicates are kept.

## Merge, split, extract

```rust
use foliopdf::ops;
let merged = ops::merge(&[&a, &b, &c])?;                 // metadata from `a`
let parts = ops::split(&doc, &ops::chunk_pages(doc.page_count(), 10))?;
let excerpt = ops::extract(&doc, &[0, 4, 5])?;
ops::delete_pages(&mut doc, &[1, 2])?;
ops::rotate_pages(&mut doc, &idx, 180)?;
```

## Stamps and page numbers

```rust
use foliopdf::ops::{stamp_text, stamp_image, add_page_numbers, TextStamp, ImageStamp, PageNumbers, Position};

stamp_text(&mut doc, &all_pages, &TextStamp::watermark("DRAFT"))?;
stamp_text(&mut doc, &[0], &TextStamp {
    text: "Reviewed {page}/{pages}".into(),
    font: "Times-Bold".into(),
    size: 14.0,
    color: [0.0, 0.3, 0.8],
    opacity: 1.0,
    position: Position::TopRight,
    rotation: 0.0,
    margin: 24.0,
    under: false,
})?;
stamp_image(&mut doc, &all_pages, &std::fs::read("logo.png")?, &ImageStamp {
    width: Some(120.0), position: Position::BottomLeft, opacity: 0.9, ..Default::default()
})?;
add_page_numbers(&mut doc, &[], &PageNumbers { format: "Page {page} of {pages}".into(), ..Default::default() })?;
```

Positions are in display orientation and honour `/Rotate` and `/CropBox`.

## Metadata

```rust
use foliopdf::document::Metadata;
doc.set_title("Quarterly report");
doc.set_metadata(&Metadata { author: Some("Ada".into()), keywords: Some(String::new()), ..Default::default() }); // empty string removes
doc.strip_metadata();   // XMP, /Info, thumbnails
```

## Encryption

```rust
use foliopdf::{EncryptionOptions, EncryptionMethod, Permissions};

let enc = EncryptionOptions {
    user_password: String::new(),          // anyone can open
    owner_password: "s3cret".into(),       // needed to change permissions
    method: EncryptionMethod::Aes256,      // Aes128, Rc4_128 for legacy readers
    permissions: Permissions { copy: false, modify: false, ..Default::default() },
    encrypt_metadata: true,
};
let out = doc.save(&SaveOptions { encryption: Some(enc), ..Default::default() })?;
```

To remove encryption, open with a password and save without `encryption`.

## Drawing with your own content

```rust
use foliopdf::content::ContentBuilder;
use foliopdf::font::{Font, StandardFont};
use foliopdf::image::Image;

let font = doc.add_standard_font(StandardFont::Helvetica);          // or
let font = doc.add_font(Font::truetype(&std::fs::read("Inter.ttf")?)?);
let font_name = doc.add_page_resource(0, "Font", font)?;             // "F1"
let img = doc.add_image(&Image::load(&png_bytes)?, 6);
let img_name = doc.add_page_resource(0, "XObject", img)?;            // "X1"
let gs = doc.add_opacity_state(0.5);
let gs_name = doc.add_page_resource(0, "ExtGState", gs)?;

let text = "Hello, wörld";
let encoded = doc.font_mut(font).unwrap().encode(text);              // records glyph usage
let width = doc.font(font).unwrap().measure(text, 12.0);

let mut cb = ContentBuilder::new();
cb.save().ext_gstate(&gs_name)
  .fill_rgb(0.1, 0.1, 0.1)
  .begin_text().font(&font_name, 12.0).text_position(72.0, 720.0);
if doc.font(font).unwrap().is_two_byte() { cb.show_bytes(&encoded); } else { cb.show_literal(&encoded); }
cb.end_text()
  .image(&img_name, &Rect::from_xywh(72.0, 500.0, 200.0, 150.0))
  .restore();
doc.draw(0, &cb.finish())?;        // on top; doc.draw_under(...) for beneath
```

TrueType fonts are subset when the document is saved, so draw all your text
before calling `save`.

## Low level

```rust
use foliopdf::{Object, ObjRef, Dict, Stream};

let catalog = doc.catalog();
let root = doc.catalog_ref();
let obj = doc.get(ObjRef::new(12, 0));
let value = doc.resolve(dict.get("Resources").unwrap());
let data = doc.stream_data(stream)?;          // decoded
let r = doc.add(Dict::new().with("Type", "Foo").into());
doc.set(r, Object::Null);
for (r, obj) in doc.objects() { /* ... */ }
```

## Batch

See [presets.md](presets.md). From Rust:

```rust
use foliopdf::batch::{Preset, Input, Asset, run};
let preset = Preset::from_json(&json)?;
let result = run(&preset, &[Input::new("a.pdf", a_bytes)], &[Asset { name: "logo".into(), data: png }])?;
for o in result.outputs { std::fs::write(&o.name, &o.data)?; }
```
