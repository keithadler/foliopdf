//! Image recompression robustness: compresses every PDF under the given
//! directories in memory and checks the result reloads. `-- DIR...`
use foliopdf::compress::{self, ImageOptions};
use foliopdf::{Document, SaveOptions};
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
    let (mut n, mut before, mut after, mut images, mut recompressed, mut failures) =
        (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
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
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compress::compress_images(&mut doc, &ImageOptions::default())
        }));
        let rep = match r {
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
        if rep.images == 0 {
            continue;
        }
        n += 1;
        images += rep.images;
        recompressed += rep.recompressed;
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
        if let Err(e) = Document::load(&out) {
            failures += 1;
            eprintln!("RELOAD {}: {e}", f.display());
            continue;
        }
        before += bytes.len();
        after += out.len();
    }
    println!("{} files, {n} with images: {images} images, {recompressed} recompressed, {} -> {} bytes ({:.0}%), {failures} failures, {:.1}s", files.len(), before, after, 100.0 * after as f64 / before.max(1) as f64, t0.elapsed().as_secs_f64());
}
