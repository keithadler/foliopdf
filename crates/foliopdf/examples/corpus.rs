//! Smoke test over a directory of real PDFs: load, re-save compressed,
//! reload, and report every failure. Nothing is written back.
//!
//! ```text
//! cargo run --release -p foliopdf --example corpus -- ~/Documents [--password PW] [--max N]
//! ```

use std::path::PathBuf;
use std::time::Instant;

use foliopdf::{Document, LoadOptions, SaveOptions};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dirs = Vec::new();
    let mut password = String::new();
    let mut max = usize::MAX;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--password" => {
                password = args.get(i + 1).cloned().unwrap_or_default();
                i += 1;
            }
            "--max" => {
                max = args
                    .get(i + 1)
                    .and_then(|m| m.parse().ok())
                    .unwrap_or(usize::MAX);
                i += 1;
            }
            d => dirs.push(PathBuf::from(d)),
        }
        i += 1;
    }
    let mut files = Vec::new();
    for d in dirs {
        walk(&d, 0, &mut files);
    }
    files.sort();
    files.truncate(max);
    let (mut ok, mut failed, mut reconstructed, mut encrypted) = (0, 0, 0, 0);
    let (mut in_bytes, mut out_bytes) = (0usize, 0usize);
    let start = Instant::now();
    for f in &files {
        let bytes = match std::fs::read(f) {
            Ok(b) => b,
            Err(_) => continue,
        };
        let t = Instant::now();
        let opts = LoadOptions {
            password: password.clone(),
            recover: true,
        };
        let result = Document::load_with(&bytes, &opts).and_then(|mut d| {
            let pages = d.page_count();
            if pages == 0 {
                return Err(foliopdf::Error::Malformed("no pages found".into()));
            }
            let out = d.save(&SaveOptions::default())?;
            let back = Document::load(&out)?;
            if back.page_count() != pages {
                return Err(foliopdf::Error::Malformed(format!(
                    "page count changed {pages} -> {}",
                    back.page_count()
                )));
            }
            // Every page's content must decode.
            for i in 0..pages {
                back.page_content(i)?;
            }
            Ok((d, out.len(), pages))
        });
        match result {
            Ok((d, out_len, pages)) => {
                ok += 1;
                in_bytes += bytes.len();
                out_bytes += out_len;
                if d.was_reconstructed() {
                    reconstructed += 1;
                }
                if d.was_encrypted() {
                    encrypted += 1;
                }
                println!(
                    "ok    {:>4}p {:>8} -> {:>8} {:>6.1}ms  {}",
                    pages,
                    bytes.len(),
                    out_len,
                    t.elapsed().as_secs_f64() * 1e3,
                    f.file_name().unwrap().to_string_lossy()
                );
            }
            Err(e) => {
                failed += 1;
                println!("FAIL  {e}  {}", f.display());
            }
        }
    }
    println!(
        "\n{ok} ok, {failed} failed, {reconstructed} needed xref recovery, {encrypted} encrypted; {} -> {} bytes ({:.0}%) in {:.2}s",
        in_bytes,
        out_bytes,
        if in_bytes > 0 { 100.0 * out_bytes as f64 / in_bytes as f64 } else { 0.0 },
        start.elapsed().as_secs_f64()
    );
}

fn walk(dir: &PathBuf, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 4 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name()
                .map(|n| n == "node_modules" || n == "target")
                .unwrap_or(false)
            {
                continue;
            }
            walk(&p, depth + 1, out);
        } else if p
            .extension()
            .map(|x| x.eq_ignore_ascii_case("pdf"))
            .unwrap_or(false)
        {
            out.push(p);
        }
    }
}
