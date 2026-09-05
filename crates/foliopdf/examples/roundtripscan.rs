//! Text fidelity across a compressed re-save: extracts every page before and
//! after, and reports any page whose text changed. `-- DIR...`
use foliopdf::{text, Document, SaveOptions};
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
    let mut files = Vec::new();
    for a in std::env::args().skip(1) {
        walk(Path::new(&a), &mut files);
    }
    let (mut pages, mut changed, mut failures) = (0, 0, 0);
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
        let before: Vec<String> = (0..doc.page_count())
            .map(|p| text::page_text(&doc, p).unwrap_or_default())
            .collect();
        let out = match doc.save(&SaveOptions {
            compress: true,
            ..Default::default()
        }) {
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
        for p in 0..re.page_count().min(before.len()) {
            pages += 1;
            let after = text::page_text(&re, p).unwrap_or_default();
            if after != before[p] {
                changed += 1;
                eprintln!(
                    "CHANGED {} p{}: {:?} -> {:?}",
                    f.display(),
                    p + 1,
                    before[p].chars().take(60).collect::<String>(),
                    after.chars().take(60).collect::<String>()
                );
            }
        }
    }
    println!("{} files, {pages} pages, {changed} pages changed text after re-save, {failures} failures, {:.1}s", files.len(), t0.elapsed().as_secs_f64());
}
