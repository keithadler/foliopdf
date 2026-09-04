//! Throughput benchmark. Run with `cargo bench -p foliopdf`.
//!
//! Prints time per operation and MB/s for the core paths: parse, save
//! (plain and compressed), merge, encrypt and decrypt.

use std::hint::black_box;
use std::time::{Duration, Instant};

use foliopdf::content::ContentBuilder;
use foliopdf::font::StandardFont;
use foliopdf::{Document, EncryptionOptions, LoadOptions, PageSize, SaveOptions};

fn build(pages: usize) -> Document {
    let mut doc = Document::new();
    let font = doc.add_standard_font(StandardFont::TimesRoman);
    for i in 0..pages {
        doc.add_page(PageSize::LETTER);
        let name = doc.add_page_resource(i, "Font", font).unwrap();
        let mut cb = ContentBuilder::new();
        cb.begin_text()
            .font(&name, 11.0)
            .leading(14.0)
            .text_position(72.0, 720.0);
        for line in 0..45 {
            let text = format!(
                "Page {} line {line}: the quick brown fox jumps over the lazy dog 0123456789",
                i + 1
            );
            let enc = doc.font_mut(font).unwrap().encode(&text);
            cb.show_literal(&enc).next_line();
        }
        cb.end_text();
        doc.draw(i, &cb.finish()).unwrap();
    }
    doc
}

fn bench<F: FnMut() -> usize>(name: &str, iters: usize, mut f: F) {
    // Warm-up.
    let bytes = f();
    let start = Instant::now();
    let mut total_bytes = 0usize;
    for _ in 0..iters {
        total_bytes += f();
    }
    let el = start.elapsed();
    let per = el / iters as u32;
    let mbps = total_bytes as f64 / (1 << 20) as f64 / el.as_secs_f64();
    println!(
        "{name:<34} {:>9}  {:>8.1} MB/s  ({} bytes/op)",
        fmt(per),
        mbps,
        bytes
    );
}

fn fmt(d: Duration) -> String {
    if d.as_millis() >= 10 {
        format!("{:.1} ms", d.as_secs_f64() * 1e3)
    } else {
        format!("{:.0} µs", d.as_secs_f64() * 1e6)
    }
}

fn main() {
    let mut doc = build(200);
    let plain = doc
        .save(&SaveOptions {
            compress: false,
            object_streams: false,
            ..Default::default()
        })
        .unwrap();
    let packed = doc.save(&SaveOptions::default()).unwrap();
    let encrypted = doc
        .save(&SaveOptions {
            encryption: Some(EncryptionOptions::new("u", "o")),
            ..Default::default()
        })
        .unwrap();
    println!(
        "200-page text document: plain {} KB, compressed {} KB\n",
        plain.len() / 1024,
        packed.len() / 1024
    );

    bench("load (plain xref table)", 50, || {
        black_box(Document::load(&plain).unwrap());
        plain.len()
    });
    bench("load (object + xref streams)", 50, || {
        black_box(Document::load(&packed).unwrap());
        packed.len()
    });
    bench("load + decrypt AES-256", 30, || {
        black_box(Document::load_with(&encrypted, &LoadOptions::with_password("u")).unwrap());
        encrypted.len()
    });
    let loaded = Document::load(&packed).unwrap();
    bench("save uncompressed", 30, || {
        loaded
            .clone()
            .save(&SaveOptions {
                compress: false,
                object_streams: false,
                ..Default::default()
            })
            .unwrap()
            .len()
    });
    bench("save compressed (level 6)", 20, || {
        loaded.clone().save(&SaveOptions::default()).unwrap().len()
    });
    bench("save compressed + AES-256", 20, || {
        loaded
            .clone()
            .save(&SaveOptions {
                encryption: Some(EncryptionOptions::new("u", "o")),
                ..Default::default()
            })
            .unwrap()
            .len()
    });
    let a = Document::load(&packed).unwrap();
    bench("merge 2 x 200 pages", 20, || {
        black_box(foliopdf::ops::merge(&[&a, &a]).unwrap());
        packed.len() * 2
    });
    bench("recover with damaged xref", 20, || {
        let cut = &plain[..plain.len() - 40];
        black_box(Document::load(cut).unwrap());
        cut.len()
    });
}
