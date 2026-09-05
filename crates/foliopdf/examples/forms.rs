//! Scans directories for PDFs with form fields and prints a summary.
//! `cargo run --release -p foliopdf --example forms -- DIR...`
use foliopdf::forms;
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
    let mut files = Vec::new();
    for a in std::env::args().skip(1) {
        walk(Path::new(&a), &mut files);
    }
    let mut with = 0;
    for f in &files {
        let bytes = match std::fs::read(f) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let doc = match Document::load(&bytes) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let fields = forms::list_fields(&doc);
        if fields.is_empty() {
            continue;
        }
        with += 1;
        let kinds: std::collections::BTreeMap<String, usize> =
            fields.iter().fold(Default::default(), |mut m, f| {
                *m.entry(format!("{:?}", f.kind)).or_default() += 1;
                m
            });
        println!("{} :: {} fields {:?}", f.display(), fields.len(), kinds);
        for fl in fields.iter().take(6) {
            println!(
                "    {:<40} {:?} page={:?} v={:?} opts={}",
                fl.name,
                fl.kind,
                fl.page,
                fl.value,
                fl.options.len()
            );
        }
    }
    println!("{} of {} files have form fields", with, files.len());
}
