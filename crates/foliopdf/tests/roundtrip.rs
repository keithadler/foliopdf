//! End-to-end tests: build documents, save, reload, edit, and check that the
//! output is a well-formed PDF that survives a second parse.

use foliopdf::batch::{self, Input, Preset, Step};
use foliopdf::content::ContentBuilder;
use foliopdf::font::StandardFont;
use foliopdf::ops::{self, TextStamp};
use foliopdf::{
    Document, EncryptionMethod, EncryptionOptions, LoadOptions, Object, PageSize, Rect, Rotation,
    SaveOptions,
};

fn plain_save() -> SaveOptions {
    SaveOptions {
        compress: false,
        object_streams: false,
        ..Default::default()
    }
}

/// A document with `n` pages, each carrying a line of text saying its number.
fn make_doc(n: usize) -> Document {
    let mut doc = Document::new();
    let font = doc.add_standard_font(StandardFont::Helvetica);
    for i in 0..n {
        doc.add_page(PageSize::LETTER);
        let name = doc.add_page_resource(i, "Font", font).unwrap();
        let text = format!("Page {}", i + 1);
        let encoded = doc.font_mut(font).unwrap().encode(&text);
        let mut cb = ContentBuilder::new();
        cb.begin_text()
            .font(&name, 24.0)
            .text_position(72.0, 700.0)
            .show_literal(&encoded)
            .end_text();
        doc.draw(i, &cb.finish()).unwrap();
    }
    doc.set_title("Test document");
    doc
}

fn page_text(doc: &Document, i: usize) -> String {
    String::from_utf8_lossy(&doc.page_content(i).unwrap()).into_owned()
}

#[test]
fn new_document_round_trips_plain_and_compressed() {
    let mut doc = make_doc(3);
    for opts in [plain_save(), SaveOptions::default()] {
        let bytes = doc.save(&opts).unwrap();
        assert!(bytes.starts_with(b"%PDF-1."));
        assert!(bytes.ends_with(b"%%EOF\n"));
        let back = Document::load(&bytes).unwrap();
        assert_eq!(back.page_count(), 3);
        assert_eq!(back.metadata().title.as_deref(), Some("Test document"));
        assert!(page_text(&back, 1).contains("(Page 2) Tj"));
        assert!(!back.was_reconstructed());
        let info = back.page_info(0).unwrap();
        assert_eq!(info.media_box, PageSize::LETTER.rect());
    }
}

#[test]
fn compressed_output_is_smaller_and_uses_object_streams() {
    let mut doc = make_doc(20);
    let plain = doc.save(&plain_save()).unwrap();
    let packed = doc.save(&SaveOptions::default()).unwrap();
    assert!(
        packed.len() < plain.len(),
        "{} vs {}",
        packed.len(),
        plain.len()
    );
    assert!(packed.windows(6).any(|w| w == b"ObjStm"));
    assert!(Document::load(&packed).unwrap().page_count() == 20);
}

#[test]
fn edit_pages() {
    let mut doc = make_doc(5);
    doc.remove_page(0).unwrap();
    assert_eq!(doc.page_count(), 4);
    assert!(page_text(&doc, 0).contains("(Page 2)"));
    doc.move_page(3, 0).unwrap();
    assert!(page_text(&doc, 0).contains("(Page 5)"));
    doc.select_pages(&[1, 1, 0]).unwrap();
    assert_eq!(doc.page_count(), 3);
    assert!(page_text(&doc, 0).contains("(Page 2)"));
    assert!(page_text(&doc, 2).contains("(Page 5)"));
    doc.rotate_page(0, 90).unwrap();
    doc.rotate_page(0, -180).unwrap();
    let bytes = doc.save(&SaveOptions::default()).unwrap();
    let mut back = Document::load(&bytes).unwrap();
    assert_eq!(back.page_count(), 3);
    assert_eq!(back.page_info(0).unwrap().rotation, Rotation::R270);
    // Removed pages must not survive in the output.
    assert!(!String::from_utf8_lossy(&back.save(&plain_save()).unwrap()).contains("(Page 1)"));
}

#[test]
fn merge_and_split() {
    let mut a = make_doc(2);
    let mut b = make_doc(3);
    // Give b a different title so we can check metadata comes from the first.
    b.set_title("B");
    // Round-trip through bytes so both are "loaded" documents.
    let a = Document::load(&a.save(&SaveOptions::default()).unwrap()).unwrap();
    let b = Document::load(&b.save(&SaveOptions::default()).unwrap()).unwrap();
    let mut merged = ops::merge(&[&a, &b]).unwrap();
    assert_eq!(merged.page_count(), 5);
    assert_eq!(merged.metadata().title.as_deref(), Some("Test document"));
    assert!(page_text(&merged, 4).contains("(Page 3)"));
    let bytes = merged.save(&SaveOptions::default()).unwrap();
    let back = Document::load(&bytes).unwrap();
    assert_eq!(back.page_count(), 5);
    // Fonts came along: every page has a /Font resource.
    for i in 0..5 {
        let p = back.page_ref(i).unwrap();
        let res = back
            .page_attr(p, "Resources")
            .and_then(Object::as_dict)
            .unwrap();
        assert!(res.contains("Font"), "page {i} lost its resources");
    }
    let parts = ops::split(&back, &ops::chunk_pages(5, 2)).unwrap();
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[2].page_count(), 1);
    assert!(page_text(&parts[1], 1).contains("(Page 2)"));
}

#[test]
fn encryption_round_trip_all_methods() {
    for method in [
        EncryptionMethod::Aes256,
        EncryptionMethod::Aes128,
        EncryptionMethod::Rc4_128,
    ] {
        let mut doc = make_doc(2);
        let opts = SaveOptions {
            encryption: Some(EncryptionOptions {
                method,
                ..EncryptionOptions::new("user", "owner")
            }),
            ..Default::default()
        };
        let bytes = doc.save(&opts).unwrap();
        assert!(bytes.windows(8).any(|w| w == b"/Encrypt"));
        // Content must not appear in clear text.
        assert!(
            !bytes.windows(6).any(|w| w == b"Page 1"),
            "{method:?} leaked plaintext"
        );
        assert!(
            matches!(Document::load(&bytes), Err(foliopdf::Error::WrongPassword)),
            "{method:?}"
        );
        assert!(matches!(
            Document::load_with(&bytes, &LoadOptions::with_password("nope")),
            Err(foliopdf::Error::WrongPassword)
        ));
        for pw in ["user", "owner"] {
            let back = Document::load_with(&bytes, &LoadOptions::with_password(pw)).unwrap();
            assert!(back.was_encrypted());
            assert_eq!(back.has_owner_access(), pw == "owner", "{method:?} {pw}");
            assert_eq!(back.page_count(), 2);
            assert!(page_text(&back, 0).contains("(Page 1) Tj"), "{method:?}");
            assert_eq!(back.metadata().title.as_deref(), Some("Test document"));
        }
        // Re-save without encryption: plain again.
        let mut back = Document::load_with(&bytes, &LoadOptions::with_password("user")).unwrap();
        let plain = back.save(&plain_save()).unwrap();
        assert!(plain.windows(6).any(|w| w == b"Page 1"));
    }
}

#[test]
fn empty_user_password_opens_without_password() {
    let mut doc = make_doc(1);
    let opts = SaveOptions {
        encryption: Some(EncryptionOptions::new("", "owner")),
        ..Default::default()
    };
    let bytes = doc.save(&opts).unwrap();
    let back = Document::load(&bytes).unwrap();
    assert!(back.was_encrypted());
    assert_eq!(back.encryption_description(), Some("AES-256"));
    assert!(!back.has_owner_access());
}

#[test]
fn recovers_from_broken_xref() {
    let mut doc = make_doc(3);
    let mut bytes = doc.save(&plain_save()).unwrap();
    // Corrupt the startxref offset.
    let pos = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
    for b in &mut bytes[pos + 10..pos + 14] {
        *b = b'9';
    }
    let back = Document::load(&bytes).unwrap();
    assert!(back.was_reconstructed());
    assert_eq!(back.page_count(), 3);
    assert!(page_text(&back, 2).contains("(Page 3)"));
    // Now chop the tail off entirely.
    bytes.truncate(pos);
    let back = Document::load(&bytes).unwrap();
    assert_eq!(back.page_count(), 3);
}

#[test]
fn recovers_when_object_streams_and_xref_stream_are_damaged() {
    let mut doc = make_doc(4);
    let mut bytes = doc.save(&SaveOptions::default()).unwrap();
    let pos = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
    bytes.truncate(pos);
    let back = Document::load(&bytes).unwrap();
    assert!(back.was_reconstructed());
    assert_eq!(back.page_count(), 4);
}

#[test]
fn stamps_and_page_numbers() {
    let mut doc = make_doc(3);
    doc.rotate_page(1, 90).unwrap();
    ops::stamp_text(&mut doc, &[0, 1, 2], &TextStamp::watermark("DRAFT")).unwrap();
    ops::add_page_numbers(&mut doc, &[], &Default::default()).unwrap();
    let bytes = doc.save(&SaveOptions::default()).unwrap();
    let back = Document::load(&bytes).unwrap();
    let t = page_text(&back, 1);
    assert!(t.contains("(DRAFT) Tj"));
    assert!(t.contains("(2 / 3) Tj"));
    assert!(t.contains(" gs"), "opacity ExtGState applied");
    // The original content is still there, wrapped.
    assert!(t.contains("(Page 2) Tj"));
    let p = back.page_ref(1).unwrap();
    let res = back
        .page_attr(p, "Resources")
        .and_then(Object::as_dict)
        .unwrap();
    assert!(res.get("ExtGState").is_some());
}

#[test]
fn image_stamp_png() {
    // 1×1 red RGBA PNG.
    let png = {
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
        let idat = foliopdf::filters::flate_encode(&[0, 255, 0, 0, 255], 6);
        let chunk = |k: &[u8], d: &[u8]| {
            let mut v = (d.len() as u32).to_be_bytes().to_vec();
            v.extend_from_slice(k);
            v.extend_from_slice(d);
            v.extend_from_slice(&[0; 4]);
            v
        };
        let mut p = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
        p.extend(chunk(b"IHDR", &ihdr));
        p.extend(chunk(b"IDAT", &idat));
        p.extend(chunk(b"IEND", &[]));
        p
    };
    let mut doc = make_doc(1);
    ops::stamp_image(
        &mut doc,
        &[0],
        &png,
        &ops::ImageStamp {
            width: Some(100.0),
            ..Default::default()
        },
    )
    .unwrap();
    let bytes = doc.save(&SaveOptions::default()).unwrap();
    let back = Document::load(&bytes).unwrap();
    assert!(page_text(&back, 0).contains(" Do"));
    let p = back.page_ref(0).unwrap();
    let res = back
        .page_attr(p, "Resources")
        .and_then(Object::as_dict)
        .unwrap();
    let xo = back
        .dict_get(res, "XObject")
        .and_then(Object::as_dict)
        .unwrap();
    let img = back.dict_get(xo, "X1").and_then(Object::as_stream).unwrap();
    assert!(img.dict.contains("SMask"));
}

#[test]
fn truetype_embedding_when_a_system_font_exists() {
    let candidates = [
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/Library/Fonts/Arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    let Some(path) = candidates.iter().find(|p| std::path::Path::new(p).exists()) else {
        eprintln!("no system TrueType font found; skipping");
        return;
    };
    let ttf = std::fs::read(path).unwrap();
    let mut doc = Document::new();
    doc.add_page(PageSize::A4);
    let font = doc.add_font(foliopdf::font::Font::truetype(&ttf).unwrap());
    let name = doc.add_page_resource(0, "Font", font).unwrap();
    let text = "Ünïcödé — ok";
    let encoded = doc.font_mut(font).unwrap().encode(text);
    assert_eq!(encoded.len(), text.chars().count() * 2);
    let width = doc.font(font).unwrap().measure(text, 12.0);
    assert!(width > 40.0 && width < 120.0, "{width}");
    let mut cb = ContentBuilder::new();
    cb.begin_text()
        .font(&name, 12.0)
        .text_position(50.0, 750.0)
        .show_bytes(&encoded)
        .end_text();
    doc.draw(0, &cb.finish()).unwrap();
    let bytes = doc.save(&SaveOptions::default()).unwrap();
    assert!(
        bytes.len() < ttf.len() / 2,
        "subset font should be much smaller than the full file ({} vs {})",
        bytes.len(),
        ttf.len()
    );
    let back = Document::load(&bytes).unwrap();
    let p = back.page_ref(0).unwrap();
    let res = back
        .page_attr(p, "Resources")
        .and_then(Object::as_dict)
        .unwrap();
    let fonts = back
        .dict_get(res, "Font")
        .and_then(Object::as_dict)
        .unwrap();
    let f0 = back
        .dict_get(fonts, "F1")
        .and_then(Object::as_dict)
        .unwrap();
    assert_eq!(
        f0.get("Subtype").and_then(Object::as_name).unwrap(),
        "Type0"
    );
    assert!(f0.contains("ToUnicode"));
    let desc = back
        .dict_get(f0, "DescendantFonts")
        .and_then(Object::as_array)
        .unwrap();
    let cid = back.resolve(&desc[0]).as_dict().unwrap();
    let fd = back
        .dict_get(cid, "FontDescriptor")
        .and_then(Object::as_dict)
        .unwrap();
    let ff = back
        .dict_get(fd, "FontFile2")
        .and_then(Object::as_stream)
        .unwrap();
    let program = back.stream_data(ff).unwrap();
    // The subset must itself parse as a TrueType font.
    let sub = foliopdf::font::TrueTypeFont::parse(&program).unwrap();
    assert!(sub.num_glyphs > 0);
}

#[test]
fn batch_each_and_merge() {
    let mut a = make_doc(2);
    let mut b = make_doc(3);
    let ia = Input::new("a.pdf", a.save(&Default::default()).unwrap());
    let ib = Input::new("b.pdf", b.save(&Default::default()).unwrap());

    let mut merge = Preset::new("m");
    merge.mode = batch::Mode::Merge;
    merge.steps.push(Step::Rotate {
        pages: Some("even".into()),
        degrees: 90,
    });
    merge.output.encryption = Some(EncryptionOptions::new("", "own"));
    merge.output.filename = "{stem}.pdf".into();
    let r = batch::run(&merge, &[ia.clone(), ib.clone()], &[]).unwrap();
    assert_eq!(r.outputs.len(), 1);
    assert_eq!(r.outputs[0].name, "a-merged.pdf");
    assert_eq!(r.outputs[0].pages, 5);
    let back = Document::load(&r.outputs[0].data).unwrap();
    assert!(back.was_encrypted());
    assert_eq!(back.page_info(1).unwrap().rotation, Rotation::R90);
    assert_eq!(back.page_info(0).unwrap().rotation, Rotation::R0);

    let mut each = Preset::new("e");
    each.steps.push(Step::Split {
        every: Some(2),
        ranges: None,
    });
    each.output.filename = "{stem}-part{index}of{total}.pdf".into();
    let r = batch::run(&each, &[ia, ib], &[]).unwrap();
    let names: Vec<&str> = r.outputs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(
        names,
        ["a-part1of1.pdf", "b-part1of2.pdf", "b-part2of2.pdf"]
    );
    assert_eq!(r.outputs[2].pages, 1);
}

#[test]
fn media_box_and_metadata() {
    let mut doc = make_doc(1);
    doc.set_media_box(0, Rect::new(0.0, 0.0, 300.0, 400.0))
        .unwrap();
    doc.set_metadata(&foliopdf::document::Metadata {
        author: Some("Me".into()),
        title: Some(String::new()),
        ..Default::default()
    });
    let bytes = doc
        .save(&SaveOptions {
            strip_metadata: false,
            ..Default::default()
        })
        .unwrap();
    let back = Document::load(&bytes).unwrap();
    assert_eq!(back.page_info(0).unwrap().media_box.width(), 300.0);
    let m = back.metadata();
    assert_eq!(m.author.as_deref(), Some("Me"));
    assert_eq!(m.title, None);
    assert!(m.producer.unwrap().starts_with("foliopdf"));
    let mut back = back;
    let stripped = back
        .save(&SaveOptions {
            strip_metadata: true,
            producer: None,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        Document::load(&stripped).unwrap().metadata(),
        Default::default()
    );
}

#[test]
fn garbage_in_is_an_error_not_a_panic() {
    assert!(Document::load(b"").is_err());
    assert!(Document::load(b"not a pdf at all, just some text that is long enough").is_err());
    let junk: Vec<u8> = (0..5000u32).map(|i| (i * 7919 % 256) as u8).collect();
    assert!(Document::load(&junk).is_err());
}

#[test]
fn merge_deduplicates_identical_resources() {
    let candidates = [
        "/System/Library/Fonts/Supplemental/Arial.ttf",
        "/Library/Fonts/Arial.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    ];
    let mut doc = make_doc(2);
    // A big shared resource: an embedded TrueType font if available, else a large image-like stream.
    if let Some(path) = candidates.iter().find(|p| std::path::Path::new(p).exists()) {
        let ttf = std::fs::read(path).unwrap();
        let font = doc.add_font(foliopdf::font::Font::truetype(&ttf).unwrap());
        for i in 0..2 {
            let name = doc.add_page_resource(i, "Font", font).unwrap();
            let enc = doc.font_mut(font).unwrap().encode("Shared font text");
            let mut cb = ContentBuilder::new();
            cb.begin_text()
                .font(&name, 12.0)
                .text_position(72.0, 600.0)
                .show_bytes(&enc)
                .end_text();
            doc.draw(i, &cb.finish()).unwrap();
        }
    } else {
        let noise: Vec<u8> = (0..200_000u32)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8)
            .collect();
        let big = doc.add(foliopdf::Stream::new(Default::default(), noise).into());
        for i in 0..2 {
            doc.add_page_resource(i, "XObject", big).unwrap();
        }
    }
    let single = doc.save(&SaveOptions::default()).unwrap();
    let one = Document::load(&single).unwrap();
    let mut merged = ops::merge(&[&one, &one, &one]).unwrap();
    let out = merged.save(&SaveOptions::default()).unwrap();
    assert_eq!(Document::load(&out).unwrap().page_count(), 6);
    assert!(
        out.len() < single.len() * 3 / 2,
        "merged {} vs single {}",
        out.len(),
        single.len()
    );
    // Without compression nothing is deduplicated (objects are written as-is).
    let raw = merged.save(&plain_save()).unwrap();
    assert!(raw.len() > out.len());
}
