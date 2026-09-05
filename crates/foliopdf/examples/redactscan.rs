//! Redaction robustness: redacts a word from every page of every PDF under
//! the given directories, re-opens the result and checks the word is gone.
//! `cargo run --release -p foliopdf --example redactscan -- WORD DIR...`
use foliopdf::redact::{self, RedactOptions};
use foliopdf::text::{self, SearchOptions};
use foliopdf::Document;
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
    let word = &args[0];
    let mut files = Vec::new();
    for a in &args[1..] {
        walk(Path::new(a), &mut files);
    }
    let so = SearchOptions {
        case_insensitive: true,
        ..Default::default()
    };
    let (
        mut files_hit,
        mut pages_hit,
        mut matches,
        mut glyphs,
        mut leftovers,
        mut failures,
        mut images_edited,
        mut images_removed,
        mut forms,
    ) = (0, 0, 0, 0, 0, 0, 0, 0, 0);
    let t0 = std::time::Instant::now();
    for f in &files {
        let bytes = match std::fs::read(f) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let mut doc = match Document::load(&bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let pages: Vec<usize> = (0..doc.page_count()).collect();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            redact::redact_text(&mut doc, &pages, word, &so, &RedactOptions::default())
        }));
        let (rep, m) = match r {
            Ok(Ok(x)) => x,
            Ok(Err(e)) => {
                failures += 1;
                eprintln!("ERR {}: {e}", f.display());
                continue;
            }
            Err(_) => {
                failures += 1;
                eprintln!("PANIC {}", f.display());
                continue;
            }
        };
        if m == 0 {
            continue;
        }
        files_hit += 1;
        matches += m;
        glyphs += rep.glyphs_removed;
        images_edited += rep.images_edited;
        images_removed += rep.images_removed;
        forms += rep.forms_edited;
        let out = match doc.save(&Default::default()) {
            Ok(o) => o,
            Err(e) => {
                failures += 1;
                eprintln!("SAVE {}: {e}", f.display());
                continue;
            }
        };
        let re = match Document::load(&out) {
            Ok(d) => d,
            Err(e) => {
                failures += 1;
                eprintln!("RELOAD {}: {e}", f.display());
                continue;
            }
        };
        for p in 0..re.page_count() {
            let left = text::search(&re, p, word, &so)
                .map(|v| v.len())
                .unwrap_or(0);
            if left > 0 {
                leftovers += left;
                eprintln!("LEFT {} p{}: {left}", f.display(), p + 1);
            }
            pages_hit += 1;
        }
    }
    println!("{} files scanned, {} with '{word}': {matches} matches, {glyphs} glyphs removed, {images_edited} images blanked, {images_removed} removed, {forms} forms rewritten, {leftovers} leftovers, {failures} failures, {pages_hit} pages re-checked, {:.1}s", files.len(), files_hit, t0.elapsed().as_secs_f64());
}
