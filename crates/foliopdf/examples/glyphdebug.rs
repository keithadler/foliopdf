//! Prints how a word is matched on a page: each hit's rectangles and the glyphs behind them.
//! `cargo run --release -p foliopdf --example glyphdebug -- FILE PAGE WORD`
use foliopdf::text::{self, SearchOptions};
use foliopdf::Document;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let doc = Document::load(&std::fs::read(&a[0]).unwrap()).unwrap();
    let page: usize = a[1].parse::<usize>().unwrap() - 1;
    let c = text::page_content(&doc, page).unwrap();
    let hits = text::search_content(
        &c,
        &a[2],
        &SearchOptions {
            case_insensitive: true,
            ..Default::default()
        },
    );
    println!(
        "{} hits; {} glyphs; unmapped {:?}",
        hits.len(),
        c.glyphs.len(),
        c.unmapped_fonts
    );
    for h in hits.iter().take(12) {
        println!(
            "hit {:?} rects {:?}",
            h.text,
            h.rects
                .iter()
                .map(|r| format!(
                    "[{:.1},{:.1} {:.1}x{:.1}]",
                    r.x0,
                    r.y0,
                    r.width(),
                    r.height()
                ))
                .collect::<Vec<_>>()
        );
    }
    // Zero-width glyphs and their streams.
    let zero = c.glyphs.iter().filter(|g| g.rect.width() < 0.01).count();
    println!("zero-width glyphs: {zero} / {}", c.glyphs.len());
    let mut streams = std::collections::BTreeMap::new();
    for g in &c.glyphs {
        *streams.entry(format!("{:?}", g.loc.stream)).or_insert(0) += 1;
    }
    println!("glyphs per stream: {streams:?}");
    println!("forms: {}", c.forms.len());
    // Redact and see what survives, with the operators the survivors came from.
    let ops = text::page_ops(&doc, page).unwrap();
    let mut d2 = Document::load(&std::fs::read(&a[0]).unwrap()).unwrap();
    let (rep, n) = foliopdf::redact::redact_text(
        &mut d2,
        &[page],
        &a[2],
        &SearchOptions {
            case_insensitive: true,
            ..Default::default()
        },
        &Default::default(),
    )
    .unwrap();
    println!("redacted {n} matches: {rep:?}");
    let d3 = Document::load(&d2.save(&Default::default()).unwrap()).unwrap();
    let c3 = text::page_content(&d3, page).unwrap();
    let left = text::search_content(
        &c3,
        &a[2],
        &SearchOptions {
            case_insensitive: true,
            ..Default::default()
        },
    );
    println!("{} left", left.len());
    let ops3 = text::page_ops(&d3, page).unwrap();
    let show = |ops: &[foliopdf::cstream::Op], from: usize, to: usize| {
        for (i, op) in ops.iter().enumerate().skip(from).take(to - from) {
            println!(
                "   #{i} {} {}",
                op.name,
                String::from_utf8_lossy(&foliopdf::cstream::write(std::slice::from_ref(op))).trim()
            );
        }
    };
    println!("ORIGINAL ops 590..616:");
    show(&ops, 590, 616);
    println!("REWRITTEN ops 590..618:");
    show(&ops3, 590, 618);
    for h in left.iter().take(5) {
        let r = h.rects[0];
        println!(
            "left {:?} at [{:.1},{:.1} {:.1}x{:.1}]",
            h.text,
            r.x0,
            r.y0,
            r.width(),
            r.height()
        );
        // Original glyphs overlapping this rect.
        for g in c
            .glyphs
            .iter()
            .filter(|g| g.rect.intersects(&r.expand(0.5)))
        {
            let op = &ops[g.loc.op];
            println!("   orig glyph {:?} rect [{:.1},{:.1} {:.1}x{:.1}] op#{} {} elem {} bytes {}..{} operands {}", g.text, g.rect.x0, g.rect.y0, g.rect.width(), g.rect.height(), g.loc.op, op.name, g.loc.elem, g.loc.start, g.loc.end, op.operands.len());
        }
        for g in c3
            .glyphs
            .iter()
            .filter(|g| g.rect.intersects(&r.expand(0.5)))
        {
            println!(
                "   NOW glyph {:?} rect [{:.1},{:.1} {:.1}x{:.1}] op#{}",
                g.text,
                g.rect.x0,
                g.rect.y0,
                g.rect.width(),
                g.rect.height(),
                g.loc.op
            );
        }
    }
    for f in c.forms.iter().take(10) {
        println!(
            "  form {:?} via {:?} name {} rect [{:.0},{:.0} {:.0}x{:.0}]",
            f.xobject,
            f.stream,
            f.name,
            f.rect.x0,
            f.rect.y0,
            f.rect.width(),
            f.rect.height()
        );
    }
}
#[allow(dead_code)]
fn unused() {}
