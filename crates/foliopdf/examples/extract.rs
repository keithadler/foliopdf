//! Text extraction check. `cargo run --release -p foliopdf --example extract -- FILE [PAGE]`
//! prints a page's text; `-- --scan DIR...` extracts every page of every PDF and reports
//! errors, pages without text, and fonts that could not be mapped to Unicode.
use foliopdf::{text, Document};
use std::path::Path;

fn walk(p: &Path, out: &mut Vec<std::path::PathBuf>) {
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                if path
                    .file_name()
                    .map(|n| n == "node_modules" || n == "target" || n == ".git")
                    .unwrap_or(false)
                {
                    continue;
                }
                walk(&path, out);
            } else if path
                .extension()
                .map(|x| x.eq_ignore_ascii_case("pdf"))
                .unwrap_or(false)
            {
                out.push(path);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--scan") {
        let mut files = Vec::new();
        for a in &args[1..] {
            walk(Path::new(a), &mut files);
        }
        let (mut pages, mut with_text, mut errors, mut unmapped_pages) =
            (0usize, 0usize, 0usize, 0usize);
        let mut unmapped: std::collections::BTreeMap<String, usize> = Default::default();
        let t0 = std::time::Instant::now();
        let mut chars = 0usize;
        for f in &files {
            let bytes = match std::fs::read(f) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let doc = match Document::load(&bytes) {
                Ok(d) => d,
                Err(_) => continue,
            };
            for p in 0..doc.page_count() {
                pages += 1;
                let r = std::panic::catch_unwind(|| text::page_content(&doc, p));
                match r {
                    Ok(Ok(c)) => {
                        let t = text::text_from_lines(&text::lines(&c.glyphs));
                        chars += t.len();
                        if t.trim().len() > 20 {
                            with_text += 1;
                        }
                        if !c.unmapped_fonts.is_empty() {
                            unmapped_pages += 1;
                            for u in c.unmapped_fonts {
                                *unmapped.entry(u).or_default() += 1;
                            }
                        }
                    }
                    Ok(Err(e)) => {
                        errors += 1;
                        eprintln!("ERR {} p{}: {e}", f.display(), p + 1);
                    }
                    Err(_) => {
                        errors += 1;
                        eprintln!("PANIC {} p{}", f.display(), p + 1);
                    }
                }
            }
        }
        println!("{} files, {} pages, {} with text, {} errors, {} pages with unmapped fonts, {} chars, {:.1}s", files.len(), pages, with_text, errors, unmapped_pages, chars, t0.elapsed().as_secs_f64());
        let mut u: Vec<_> = unmapped.into_iter().collect();
        u.sort_by_key(|x| std::cmp::Reverse(x.1));
        for (name, n) in u.iter().take(15) {
            println!("  unmapped {n:>4}  {name}");
        }
        return;
    }
    let bytes = std::fs::read(&args[0]).unwrap();
    let doc = Document::load(&bytes).unwrap();
    let page: usize = args.get(1).and_then(|p| p.parse().ok()).unwrap_or(1);
    let t = text::page_text(&doc, page - 1).unwrap();
    println!("{t}");
    let c = text::page_content(&doc, page - 1).unwrap();
    eprintln!(
        "--- {} glyphs, {} images, {} paths, {} forms, unmapped: {:?}",
        c.glyphs.len(),
        c.images.len(),
        c.paths.len(),
        c.forms.len(),
        c.unmapped_fonts
    );
}
