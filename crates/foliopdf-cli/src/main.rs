//! `folio` – command-line PDF editing powered by foliopdf.
//!
//! Zero dependencies beyond the core crate and serde_json; argument parsing
//! is deliberately simple so the binary stays small and starts instantly.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use foliopdf::annot::{self, FlattenOptions};
use foliopdf::batch::{self, Asset, Input, Preset, PresetStore};
use foliopdf::crypto::{EncryptionOptions, Method, Permissions};
use foliopdf::document::Metadata;
use foliopdf::forms::{self, FieldValue};
use foliopdf::ops::{self, ImageStamp, PageNumbers, Position, TextStamp};
use foliopdf::redact::{self, RedactOptions};
use foliopdf::text::{self, SearchOptions};
use foliopdf::{Document, LoadOptions, SaveOptions};

const HELP: &str = "\
folio – fast PDF editing (merge, split, compress, encrypt, stamp)

USAGE
  folio <command> [options]

COMMANDS
  info     <in.pdf>                       Show pages, encryption, metadata
  merge    <out.pdf> <in.pdf>...          Merge files in order
  images   <out.pdf> <image>...           One page per JPEG/PNG [--size a4] [--margin 36] [--dpi 150]
  extract-images <in.pdf> [--out-dir D] [--pages all]   Save the images as JPEG/PNG files
  nup      <in.pdf> <out.pdf> [--per-sheet 2] [--sheet letter] [--portrait] [--margin 18] [--frames]
  booklet  <in.pdf> <out.pdf> [--sheet letter] [--margin 18]
  split    <in.pdf> --every N | --ranges \"1-3\" \"4-\"   [--out-dir D] [--name T]
  pages    <in.pdf> <out.pdf> --select \"1-3,7\" | --delete \"2,4\"
  rotate   <in.pdf> <out.pdf> --degrees 90 [--pages odd]
  resize   <in.pdf> <out.pdf> --size a4|letter|WxH [--mode fit|fill|stretch] | --scale 0.5
                              [--pages all]
  reverse  <in.pdf> <out.pdf>
  blank    <in.pdf> <out.pdf> [--at N] [--count 1] [--size a4]   Insert blank pages before page N
  crop     <in.pdf> <out.pdf> --box x,y,w,h [--pages all] | --reset
  bookmarks <in.pdf> [--json]              List bookmarks
  bookmarks <in.pdf> <out.pdf> --set tree.json | --clear
  compress <in.pdf> <out.pdf> [--level 1-10] [--strip-metadata]
                              [--images [--dpi 150] [--quality 75] [--keep-lossless]]
  encrypt  <in.pdf> <out.pdf> [--user PW] [--owner PW] [--method aes256|aes128|rc4]
                              [--no-print] [--no-copy] [--no-modify] [--no-annotate]
  decrypt  <in.pdf> <out.pdf> --password PW
  stamp    <in.pdf> <out.pdf> --text \"DRAFT\" | --image logo.png
                              [--pages all] [--position center] [--size 60] [--opacity 0.3]
                              [--rotation 45] [--color r,g,b] [--font Helvetica] [--under]
                              [--width W] [--height H] [--margin M]
  numbers  <in.pdf> <out.pdf> [--format \"{page} / {pages}\"] [--position bottom-center]
                              [--size 10] [--pages all] [--start 1]
  meta     <in.pdf> <out.pdf> [--title T] [--author A] [--subject S] [--keywords K]
  fields   <in.pdf> [--json]               List form fields and their values
  fill     <in.pdf> <out.pdf> --set name=value... [--data values.json] [--flatten]
  annots   <in.pdf> [--json]               List annotations
  text     <in.pdf> [--pages all] [--out file.txt]      Extract text
  search   <in.pdf> \"needle\" [--pages all] [-i] [--word] [--json]
  redact   <in.pdf> <out.pdf> --text \"needle\"... [-i] [--word] [--pages all]
                              | --area PAGE:x,y,w,h...   [--no-fill] [--color r,g,b]
                              Remove text, graphics and image pixels for good
  flatten  <in.pdf> <out.pdf> [--forms | --annots] [--pages all]
                                           Burn form fields and/or annotations into the pages
  batch    <preset.json> <in.pdf>... [--out-dir D] [--asset name=path]...
  presets  [export <file.json>]           List built-in presets or write them as a store

GLOBAL OPTIONS
  --password PW      Password for encrypted inputs
  --no-compress      Write uncompressed output (debugging)
  --level N          Flate level 1–10 (default 6)
  -h, --help         This help
  -V, --version      Version

Page ranges are 1-based: \"1-3,7\", \"4-\", \"-2\", \"odd\", \"even\", \"last\", \"r2\" (2nd from end).
";

struct Args {
    positional: Vec<String>,
    flags: HashMap<String, Vec<String>>,
}

impl Args {
    fn parse(argv: &[String]) -> Self {
        let mut positional = Vec::new();
        let mut flags: HashMap<String, Vec<String>> = HashMap::new();
        let mut i = 0;
        while i < argv.len() {
            let a = &argv[i];
            if let Some(name) = a.strip_prefix("--") {
                let (name, inline) = match name.split_once('=') {
                    Some((n, v)) => (n.to_string(), Some(v.to_string())),
                    None => (name.to_string(), None),
                };
                let boolean = matches!(
                    name.as_str(),
                    "help"
                        | "version"
                        | "no-compress"
                        | "strip-metadata"
                        | "under"
                        | "no-print"
                        | "no-copy"
                        | "no-modify"
                        | "no-annotate"
                        | "no-forms"
                        | "no-assemble"
                );
                let value = match inline {
                    Some(v) => v,
                    None if boolean => "true".into(),
                    None => {
                        i += 1;
                        argv.get(i).cloned().unwrap_or_default()
                    }
                };
                flags.entry(name).or_default().push(value);
            } else if a == "-h" {
                flags.entry("help".into()).or_default().push("true".into());
            } else if a == "-V" {
                flags
                    .entry("version".into())
                    .or_default()
                    .push("true".into());
            } else {
                positional.push(a.clone());
            }
            i += 1;
        }
        Self { positional, flags }
    }
    fn flag(&self, name: &str) -> Option<&str> {
        self.flags
            .get(name)
            .and_then(|v| v.last())
            .map(String::as_str)
    }
    fn has(&self, name: &str) -> bool {
        self.flags.contains_key(name)
    }
    fn all(&self, name: &str) -> Vec<String> {
        self.flags.get(name).cloned().unwrap_or_default()
    }
    fn num<T: std::str::FromStr>(&self, name: &str, default: T) -> Result<T, String> {
        match self.flag(name) {
            Some(v) => v
                .parse()
                .map_err(|_| format!("--{name} expects a number, got '{v}'")),
            None => Ok(default),
        }
    }
    fn pos(&self, i: usize, what: &str) -> Result<&str, String> {
        self.positional
            .get(i)
            .map(String::as_str)
            .ok_or_else(|| format!("missing {what}\n\n{HELP}"))
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    match run(&argv) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("folio: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(argv: &[String]) -> Result<(), String> {
    let args = Args::parse(argv);
    if args.has("version") {
        println!("folio {}", foliopdf::VERSION);
        return Ok(());
    }
    if args.has("help") || args.positional.is_empty() {
        print!("{HELP}");
        return Ok(());
    }
    let cmd = args.positional[0].as_str();
    match cmd {
        "info" => info(&args),
        "merge" => merge(&args),
        "images" => images_cmd(&args),
        "extract-images" => extract_images_cmd(&args),
        "nup" => nup_cmd(&args),
        "booklet" => booklet_cmd(&args),
        "split" => split(&args),
        "pages" => pages(&args),
        "rotate" => rotate(&args),
        "resize" => resize(&args),
        "reverse" => reverse(&args),
        "blank" => blank(&args),
        "crop" => crop_cmd(&args),
        "bookmarks" => bookmarks_cmd(&args),
        "compress" => compress(&args),
        "encrypt" => encrypt(&args),
        "decrypt" => decrypt(&args),
        "stamp" => stamp(&args),
        "numbers" => numbers(&args),
        "meta" => meta(&args),
        "fields" => fields(&args),
        "fill" => fill(&args),
        "annots" => annots(&args),
        "text" => text_cmd(&args),
        "search" => search_cmd(&args),
        "redact" => redact_cmd(&args),
        "flatten" => flatten(&args),
        "batch" => batch_cmd(&args),
        "presets" => presets(&args),
        other => Err(format!("unknown command '{other}'\n\n{HELP}")),
    }
}

fn load(args: &Args, path: &str) -> Result<Document, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let opts = LoadOptions {
        password: args.flag("password").unwrap_or("").to_string(),
        recover: true,
    };
    let doc = Document::load_with(&bytes, &opts).map_err(|e| format!("{path}: {e}"))?;
    if doc.was_reconstructed() {
        eprintln!("folio: {path}: cross-reference table was damaged and has been rebuilt");
    }
    Ok(doc)
}

fn save_opts(args: &Args) -> Result<SaveOptions, String> {
    Ok(SaveOptions {
        compress: !args.has("no-compress"),
        compression_level: args.num("level", 6u8)?,
        strip_metadata: args.has("strip-metadata"),
        ..Default::default()
    })
}

fn write(doc: &mut Document, path: &str, opts: &SaveOptions) -> Result<(), String> {
    let bytes = doc.save(opts).map_err(|e| e.to_string())?;
    std::fs::write(path, &bytes).map_err(|e| format!("{path}: {e}"))?;
    eprintln!(
        "folio: wrote {path} ({} pages, {})",
        doc.page_count(),
        human(bytes.len())
    );
    Ok(())
}

fn human(n: usize) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1u64 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{n} B")
    }
}

fn range(args: &Args, doc: &Document, flag: &str) -> Result<Vec<usize>, String> {
    match args.flag(flag) {
        Some(spec) => ops::parse_page_ranges(spec, doc.page_count()).map_err(|e| e.to_string()),
        None => Ok((0..doc.page_count()).collect()),
    }
}

fn position(args: &Args, default: Position) -> Result<Position, String> {
    match args.flag("position") {
        None => Ok(default),
        Some(p) => serde_json::from_value(serde_json::Value::String(p.to_string())).map_err(|_| {
            format!("unknown --position '{p}' (use e.g. center, top-left, bottom-right)")
        }),
    }
}

fn color(args: &Args, default: [f64; 3]) -> Result<[f64; 3], String> {
    match args.flag("color") {
        None => Ok(default),
        Some(c) => {
            let parts: Result<Vec<f64>, _> =
                c.split(',').map(|s| s.trim().parse::<f64>()).collect();
            match parts {
                Ok(v) if v.len() == 3 => Ok([v[0], v[1], v[2]]),
                _ => Err(format!("--color expects r,g,b in 0..1, got '{c}'")),
            }
        }
    }
}

fn info(args: &Args) -> Result<(), String> {
    let path = args.pos(1, "input file")?;
    let doc = load(args, path)?;
    let m = doc.metadata();
    println!("file:        {path}");
    println!("version:     PDF {}.{}", doc.version().0, doc.version().1);
    println!("pages:       {}", doc.page_count());
    println!("objects:     {}", doc.object_count());
    match doc.encryption_description() {
        Some(d) => {
            let p = doc.input_permissions().unwrap_or_default();
            println!(
                "encryption:  {d} (owner access: {})",
                if doc.has_owner_access() { "yes" } else { "no" }
            );
            println!(
                "permissions: print={} modify={} copy={} annotate={} forms={} assemble={}",
                p.print, p.modify, p.copy, p.annotate, p.fill_forms, p.assemble
            );
        }
        None => println!("encryption:  none"),
    }
    for (k, v) in [
        ("title", &m.title),
        ("author", &m.author),
        ("subject", &m.subject),
        ("keywords", &m.keywords),
        ("creator", &m.creator),
        ("producer", &m.producer),
    ] {
        if let Some(v) = v {
            println!("{k:<12} {v}");
        }
    }
    for p in doc.pages() {
        let b = p.visible_box();
        println!(
            "  page {:>4}: {:.0} x {:.0} pt{}",
            p.index + 1,
            b.width(),
            b.height(),
            if p.rotation.degrees() != 0 {
                format!(", rotated {}°", p.rotation.degrees())
            } else {
                String::new()
            }
        );
    }
    Ok(())
}

fn merge(args: &Args) -> Result<(), String> {
    let out = args.pos(1, "output file")?;
    if args.positional.len() < 3 {
        return Err("merge needs at least one input".into());
    }
    let mut docs = Vec::new();
    for p in &args.positional[2..] {
        docs.push(load(args, p)?);
    }
    let refs: Vec<&Document> = docs.iter().collect();
    let mut merged = ops::merge(&refs).map_err(|e| e.to_string())?;
    write(&mut merged, out, &save_opts(args)?)
}

fn images_cmd(args: &Args) -> Result<(), String> {
    let out = args.pos(1, "output file")?;
    if args.positional.len() < 3 {
        return Err("images needs at least one JPEG or PNG file".into());
    }
    let opts = foliopdf::ops::ImagePageOptions {
        size: args.flag("size").map(str::to_owned),
        margin: args.num("margin", 0.0)?,
        dpi: args.num("dpi", 150.0)?,
        ..Default::default()
    };
    let mut doc = Document::new();
    for path in &args.positional[2..] {
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        foliopdf::ops::add_image_page(&mut doc, &bytes, &opts)
            .map_err(|e| format!("{path}: {e}"))?;
    }
    write(&mut doc, out, &save_opts(args)?)
}

fn impose_opts(args: &Args) -> Result<foliopdf::impose::ImposeOptions, String> {
    Ok(foliopdf::impose::ImposeOptions {
        sheet: args.flag("sheet").unwrap_or("letter").to_string(),
        landscape: !args.has("portrait"),
        margin: args.num("margin", 18.0)?,
        frames: args.has("frames"),
    })
}

fn nup_cmd(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    let per: usize = args.num("per-sheet", 2usize)?;
    foliopdf::impose::nup(&mut doc, per, &impose_opts(args)?).map_err(|e| e.to_string())?;
    write(&mut doc, out, &save_opts(args)?)
}

fn booklet_cmd(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    foliopdf::impose::booklet(&mut doc, &impose_opts(args)?).map_err(|e| e.to_string())?;
    write(&mut doc, out, &save_opts(args)?)
}

fn extract_images_cmd(args: &Args) -> Result<(), String> {
    let input = args.pos(1, "input file")?;
    let doc = load(args, input)?;
    let idx = range(args, &doc, "pages")?;
    let dir = PathBuf::from(args.flag("out-dir").unwrap_or("."));
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let stem = Path::new(input)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string();
    let list = foliopdf::extract::extract_images(&doc, &idx).map_err(|e| e.to_string())?;
    for (k, im) in list.iter().enumerate() {
        let name = format!(
            "{stem}-p{}-{}.{}",
            im.page + 1,
            k + 1,
            if im.format == "jpeg" { "jpg" } else { "png" }
        );
        std::fs::write(dir.join(&name), &im.data).map_err(|e| format!("{name}: {e}"))?;
        println!("{name}  {}x{}", im.width, im.height);
    }
    eprintln!("folio: {} image(s)", list.len());
    Ok(())
}

fn split(args: &Args) -> Result<(), String> {
    let input = args.pos(1, "input file")?;
    let doc = load(args, input)?;
    let chunks: Vec<Vec<usize>> = if let Some(e) = args.flag("every") {
        let n: usize = e
            .parse()
            .map_err(|_| "--every expects a number".to_string())?;
        ops::chunk_pages(doc.page_count(), n)
    } else {
        let ranges = args.all("ranges");
        if ranges.is_empty() {
            return Err("split needs --every N or --ranges \"1-3\" \"4-\"".into());
        }
        // Allow both repeated flags and a single space-separated value.
        let mut out = Vec::new();
        for r in ranges {
            for part in r.split_whitespace() {
                out.push(
                    ops::parse_page_ranges(part, doc.page_count()).map_err(|e| e.to_string())?,
                );
            }
        }
        out
    };
    let out_dir = PathBuf::from(args.flag("out-dir").unwrap_or("."));
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let stem = Path::new(input)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("part")
        .to_string();
    let template = args.flag("name").unwrap_or("{stem}-{index}.pdf");
    let opts = save_opts(args)?;
    let parts = ops::split(&doc, &chunks).map_err(|e| e.to_string())?;
    let total = parts.len();
    for (i, mut d) in parts.into_iter().enumerate() {
        let name = template
            .replace("{stem}", &stem)
            .replace("{index}", &(i + 1).to_string())
            .replace("{total}", &total.to_string());
        write(&mut d, out_dir.join(name).to_str().unwrap(), &opts)?;
    }
    Ok(())
}

fn pages(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    if args.has("select") {
        let idx = range(args, &doc, "select")?;
        doc.select_pages(&idx).map_err(|e| e.to_string())?;
    } else if args.has("delete") {
        let idx = range(args, &doc, "delete")?;
        ops::delete_pages(&mut doc, &idx).map_err(|e| e.to_string())?;
    } else {
        return Err("pages needs --select or --delete".into());
    }
    write(&mut doc, out, &save_opts(args)?)
}

fn rotate(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    let degrees: i64 = args.num("degrees", 90)?;
    let idx = range(args, &doc, "pages")?;
    ops::rotate_pages(&mut doc, &idx, degrees).map_err(|e| e.to_string())?;
    write(&mut doc, out, &save_opts(args)?)
}

fn resize(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    let idx = range(args, &doc, "pages")?;
    if let Some(f) = args.flag("scale") {
        let factor: f64 = f
            .parse()
            .map_err(|_| format!("--scale: '{f}' is not a number"))?;
        ops::scale_pages(&mut doc, &idx, factor).map_err(|e| e.to_string())?;
    } else {
        let size = args
            .flag("size")
            .ok_or("resize needs --size (e.g. a4, letter, 612x792) or --scale")?;
        let target = match size.split_once(['x', 'X']) {
            Some((w, h)) if w.parse::<f64>().is_ok() && h.parse::<f64>().is_ok() => {
                foliopdf::PageSize::new(w.parse().unwrap(), h.parse().unwrap())
            }
            _ => foliopdf::PageSize::by_name(size).ok_or_else(|| format!("unknown page size '{size}' (try a4, letter, legal, a3, a5, tabloid, a4-landscape, or WxH in points)"))?,
        };
        let mode = match args.flag("mode").unwrap_or("fit") {
            "fit" => ops::FitMode::Fit,
            "fill" => ops::FitMode::Fill,
            "stretch" => ops::FitMode::Stretch,
            m => return Err(format!("--mode must be fit, fill or stretch, not '{m}'")),
        };
        ops::resize_pages(&mut doc, &idx, target, mode).map_err(|e| e.to_string())?;
    }
    write(&mut doc, out, &save_opts(args)?)
}

fn reverse(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    ops::reverse_pages(&mut doc).map_err(|e| e.to_string())?;
    write(&mut doc, out, &save_opts(args)?)
}

fn blank(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    let n = doc.page_count();
    let at: usize = args.num("at", 0usize)?;
    let at = if at == 0 { n } else { (at - 1).min(n) };
    let count: usize = args.num("count", 1usize)?;
    let size = match args.flag("size") {
        Some(s) => {
            foliopdf::PageSize::by_name(s).ok_or_else(|| format!("unknown page size '{s}'"))?
        }
        None if n == 0 => foliopdf::PageSize::LETTER,
        None => {
            let info = doc
                .page_info(at.saturating_sub(1).min(n - 1))
                .map_err(|e| e.to_string())?;
            foliopdf::PageSize::new(info.media_box.width(), info.media_box.height())
        }
    };
    ops::insert_blank_pages(&mut doc, at, count.max(1), size).map_err(|e| e.to_string())?;
    write(&mut doc, out, &save_opts(args)?)
}

fn crop_cmd(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    let idx = range(args, &doc, "pages")?;
    if args.has("reset") {
        ops::uncrop_pages(&mut doc, &idx).map_err(|e| e.to_string())?;
    } else {
        let b = args.flag("box").ok_or("crop needs --box x,y,w,h (points from the bottom-left of the displayed page) or --reset")?;
        let v: Vec<f64> = b.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if v.len() != 4 {
            return Err("--box expects x,y,w,h".into());
        }
        ops::crop_pages(
            &mut doc,
            &idx,
            foliopdf::Rect::from_xywh(v[0], v[1], v[2], v[3]),
        )
        .map_err(|e| e.to_string())?;
    }
    write(&mut doc, out, &save_opts(args)?)
}

fn bookmarks_cmd(args: &Args) -> Result<(), String> {
    let input = args.pos(1, "input file")?;
    let mut doc = load(args, input)?;
    if let Some(out) = args.positional.get(2) {
        if args.has("clear") {
            foliopdf::outline::set_bookmarks(&mut doc, &[]).map_err(|e| e.to_string())?;
        } else {
            let path = args
                .flag("set")
                .ok_or("bookmarks needs --set tree.json or --clear when writing")?;
            let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
            let items: Vec<foliopdf::outline::Bookmark> =
                serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
            foliopdf::outline::set_bookmarks(&mut doc, &items).map_err(|e| e.to_string())?;
        }
        return write(&mut doc, out, &save_opts(args)?);
    }
    let items = foliopdf::outline::bookmarks(&doc);
    if args.has("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&items).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if items.is_empty() {
        println!("no bookmarks");
        return Ok(());
    }
    fn show(items: &[foliopdf::outline::Bookmark], depth: usize) {
        for b in items {
            let target = match (b.page, &b.uri) {
                (Some(p), _) => format!("p{}", p + 1),
                (None, Some(u)) => u.clone(),
                _ => "-".into(),
            };
            println!("{}{}  ({target})", "  ".repeat(depth), b.title);
            show(&b.children, depth + 1);
        }
    }
    show(&items, 0);
    Ok(())
}

fn compress(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let before = std::fs::metadata(input)
        .map(|m| m.len() as usize)
        .unwrap_or(0);
    let mut doc = load(args, input)?;
    if args.has("images") {
        let o = foliopdf::compress::ImageOptions {
            max_dpi: args.num("dpi", 150.0)?,
            quality: args.num("quality", 75u8)?,
            convert_lossless: !args.has("keep-lossless"),
            ..Default::default()
        };
        let r = foliopdf::compress::compress_images(&mut doc, &o).map_err(|e| e.to_string())?;
        eprintln!(
            "folio: {} of {} images recompressed ({} downsampled), image data {} -> {}",
            r.recompressed,
            r.images,
            r.downsampled,
            human(r.bytes_before),
            human(r.bytes_after)
        );
        for w in &r.skipped {
            eprintln!("folio: kept: {w}");
        }
    }
    let opts = SaveOptions {
        compress: true,
        ..save_opts(args)?
    };
    write(&mut doc, out, &opts)?;
    let after = std::fs::metadata(out)
        .map(|m| m.len() as usize)
        .unwrap_or(0);
    if before > 0 {
        eprintln!(
            "folio: {} -> {} ({:.0}%)",
            human(before),
            human(after),
            100.0 * after as f64 / before as f64
        );
    }
    Ok(())
}

fn encrypt(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    let method = match args.flag("method").unwrap_or("aes256") {
        "aes256" | "aes-256" => Method::Aes256,
        "aes128" | "aes-128" => Method::Aes128,
        "rc4" | "rc4-128" => Method::Rc4_128,
        m => return Err(format!("unknown --method '{m}' (aes256, aes128, rc4)")),
    };
    let permissions = Permissions {
        print: !args.has("no-print"),
        print_high_quality: !args.has("no-print"),
        copy: !args.has("no-copy"),
        accessibility: !args.has("no-copy"),
        modify: !args.has("no-modify"),
        annotate: !args.has("no-annotate"),
        fill_forms: !args.has("no-forms"),
        assemble: !args.has("no-assemble"),
    };
    let enc = EncryptionOptions {
        user_password: args.flag("user").unwrap_or("").into(),
        owner_password: args.flag("owner").unwrap_or("").into(),
        method,
        permissions,
        encrypt_metadata: true,
    };
    if enc.user_password.is_empty() && enc.owner_password.is_empty() {
        return Err("encrypt needs --user and/or --owner password".into());
    }
    let opts = SaveOptions {
        encryption: Some(enc),
        ..save_opts(args)?
    };
    write(&mut doc, out, &opts)
}

fn decrypt(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    if doc.was_encrypted() && !doc.has_owner_access() {
        eprintln!("folio: warning: opened with the user password only; the owner did not permit removing protection");
    }
    write(&mut doc, out, &save_opts(args)?)
}

fn stamp(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    let idx = range(args, &doc, "pages")?;
    if let Some(text) = args.flag("text") {
        let default = TextStamp::watermark(text);
        let stamp = TextStamp {
            text: text.to_string(),
            font: args.flag("font").unwrap_or("Helvetica").to_string(),
            size: args.num("size", default.size)?,
            color: color(args, default.color)?,
            opacity: args.num("opacity", default.opacity)?,
            position: position(args, default.position)?,
            rotation: args.num("rotation", default.rotation)?,
            margin: args.num("margin", default.margin)?,
            under: args.has("under"),
        };
        ops::stamp_text(&mut doc, &idx, &stamp).map_err(|e| e.to_string())?;
    } else if let Some(image) = args.flag("image") {
        let bytes = std::fs::read(image).map_err(|e| format!("{image}: {e}"))?;
        let stamp = ImageStamp {
            width: args
                .flag("width")
                .map(|w| w.parse::<f64>())
                .transpose()
                .map_err(|_| "--width expects a number")?,
            height: args
                .flag("height")
                .map(|h| h.parse::<f64>())
                .transpose()
                .map_err(|_| "--height expects a number")?,
            opacity: args.num("opacity", 1.0)?,
            position: position(args, Position::BottomRight)?,
            margin: args.num("margin", 36.0)?,
            under: args.has("under"),
        };
        ops::stamp_image(&mut doc, &idx, &bytes, &stamp).map_err(|e| e.to_string())?;
    } else {
        return Err("stamp needs --text or --image".into());
    }
    write(&mut doc, out, &save_opts(args)?)
}

fn numbers(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    let idx = range(args, &doc, "pages")?;
    let d = PageNumbers::default();
    let settings = PageNumbers {
        format: args.flag("format").unwrap_or(&d.format).to_string(),
        position: position(args, d.position)?,
        font: args.flag("font").unwrap_or(&d.font).to_string(),
        size: args.num("size", d.size)?,
        margin: args.num("margin", d.margin)?,
        color: color(args, d.color)?,
        start_at: args.num("start", d.start_at)?,
    };
    ops::add_page_numbers(&mut doc, &idx, &settings).map_err(|e| e.to_string())?;
    write(&mut doc, out, &save_opts(args)?)
}

fn meta(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    let m = Metadata {
        title: args.flag("title").map(String::from),
        author: args.flag("author").map(String::from),
        subject: args.flag("subject").map(String::from),
        keywords: args.flag("keywords").map(String::from),
        creator: args.flag("creator").map(String::from),
        producer: None,
    };
    doc.set_metadata(&m);
    write(&mut doc, out, &save_opts(args)?)
}

fn fields(args: &Args) -> Result<(), String> {
    let input = args.pos(1, "input file")?;
    let doc = load(args, input)?;
    let list = forms::list_fields(&doc);
    if args.has("json") {
        println!(
            "{}",
            serde_json::to_string_pretty(&list).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if list.is_empty() {
        println!("no form fields");
        return Ok(());
    }
    for f in &list {
        let value = match (&f.value, f.values.len()) {
            (_, n) if n > 1 => f.values.join(", "),
            (Some(v), _) => v.clone(),
            (None, _) => String::new(),
        };
        let page = f
            .page
            .map(|p| format!("p{}", p + 1))
            .unwrap_or_else(|| "-".into());
        let opts = if f.options.is_empty() {
            String::new()
        } else {
            format!(
                "  [{}]",
                f.options
                    .iter()
                    .map(|o| o.value.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            )
        };
        let flags = [
            (f.required, "required"),
            (f.read_only, "read-only"),
            (f.multiline, "multiline"),
        ]
        .iter()
        .filter(|(b, _)| *b)
        .map(|(_, n)| *n)
        .collect::<Vec<_>>()
        .join(",");
        println!(
            "{:<32} {:<9} {:<4} = {:?}{}{}",
            f.name,
            format!("{:?}", f.kind).to_lowercase(),
            page,
            value,
            opts,
            if flags.is_empty() {
                String::new()
            } else {
                format!("  ({flags})")
            }
        );
    }
    Ok(())
}

fn fill(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    let mut values: Vec<(String, FieldValue)> = Vec::new();
    if let Some(path) = args.flag("data") {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let map: std::collections::BTreeMap<String, serde_json::Value> =
            serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
        for (k, v) in map {
            let fv = match v {
                serde_json::Value::Bool(b) => FieldValue::Bool(b),
                serde_json::Value::Array(a) => FieldValue::List(
                    a.iter()
                        .map(|x| {
                            x.as_str()
                                .map(str::to_owned)
                                .unwrap_or_else(|| x.to_string())
                        })
                        .collect(),
                ),
                serde_json::Value::String(s) => FieldValue::Text(s),
                serde_json::Value::Null => FieldValue::Text(String::new()),
                other => FieldValue::Text(other.to_string()),
            };
            values.push((k, fv));
        }
    }
    for kv in args.all("set") {
        let (k, v) = kv
            .split_once('=')
            .ok_or_else(|| format!("--set expects name=value, got '{kv}'"))?;
        let fv = match v {
            "true" | "yes" | "on" => FieldValue::Bool(true),
            "false" | "no" | "off" => FieldValue::Bool(false),
            _ => FieldValue::Text(v.to_string()),
        };
        values.push((k.to_string(), fv));
    }
    if values.is_empty() {
        return Err("nothing to fill: use --set name=value or --data values.json".into());
    }
    let missing = forms::set_fields(&mut doc, &values).map_err(|e| e.to_string())?;
    for m in &missing {
        eprintln!("folio: no field named '{m}' (run `folio fields` to list them)");
    }
    if args.has("flatten") {
        forms::flatten_fields(&mut doc).map_err(|e| e.to_string())?;
    }
    write(&mut doc, out, &save_opts(args)?)?;
    eprintln!(
        "folio: filled {} field(s){}",
        values.len() - missing.len(),
        if args.has("flatten") {
            ", flattened"
        } else {
            ""
        }
    );
    Ok(())
}

fn annots(args: &Args) -> Result<(), String> {
    let input = args.pos(1, "input file")?;
    let doc = load(args, input)?;
    let mut all = Vec::new();
    for p in 0..doc.page_count() {
        for a in annot::list_annotations(&doc, p).map_err(|e| e.to_string())? {
            all.push((p, a));
        }
    }
    if args.has("json") {
        let rows: Vec<serde_json::Value> = all
            .iter()
            .map(|(p, a)| {
                let mut v = serde_json::to_value(a).unwrap_or_default();
                v["page"] = serde_json::Value::from(*p);
                v
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&rows).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    if all.is_empty() {
        println!("no annotations");
        return Ok(());
    }
    for (p, a) in &all {
        let who = a
            .author
            .clone()
            .or_else(|| a.field.clone())
            .unwrap_or_default();
        let what = a
            .contents
            .clone()
            .unwrap_or_default()
            .replace(['\n', '\r'], " ");
        println!(
            "p{:<4} {:<10} {:<20} [{:.0} {:.0} {:.0} {:.0}] {}",
            p + 1,
            a.subtype,
            who,
            a.rect.x0,
            a.rect.y0,
            a.rect.x1,
            a.rect.y1,
            what
        );
    }
    Ok(())
}

fn text_cmd(args: &Args) -> Result<(), String> {
    let input = args.pos(1, "input file")?;
    let doc = load(args, input)?;
    let idx = range(args, &doc, "pages")?;
    let mut out = String::new();
    for (k, p) in idx.iter().enumerate() {
        if k > 0 {
            out.push_str("\n\x0c\n");
        }
        out.push_str(&text::page_text(&doc, *p).map_err(|e| e.to_string())?);
    }
    match args.flag("out") {
        Some(path) => std::fs::write(path, out).map_err(|e| format!("{path}: {e}")),
        None => {
            println!("{out}");
            Ok(())
        }
    }
}

fn search_opts(args: &Args) -> SearchOptions {
    SearchOptions {
        case_insensitive: args.has("i") || args.has("ignore-case"),
        whole_word: args.has("word"),
    }
}

fn search_cmd(args: &Args) -> Result<(), String> {
    let (input, needle) = (args.pos(1, "input file")?, args.pos(2, "search text")?);
    let doc = load(args, input)?;
    let idx = range(args, &doc, "pages")?;
    let so = search_opts(args);
    let mut rows = Vec::new();
    for p in idx {
        let info = doc.page_info(p).map_err(|e| e.to_string())?;
        for m in text::search(&doc, p, needle, &so).map_err(|e| e.to_string())? {
            let r = foliopdf::annot::to_display_rect(&info, &m.rects[0]);
            rows.push((p, m.text.clone(), r, m.rects.len()));
        }
    }
    if args.has("json") {
        let v: Vec<serde_json::Value> = rows.iter().map(|(p, t, r, n)| serde_json::json!({ "page": p, "text": t, "x": r.x0, "y": r.y0, "width": r.width(), "height": r.height(), "lines": n })).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&v).map_err(|e| e.to_string())?
        );
        return Ok(());
    }
    for (p, t, r, _) in &rows {
        println!(
            "p{:<4} [{:.0},{:.0} {:.0}x{:.0}]  {}",
            p + 1,
            r.x0,
            r.y0,
            r.width(),
            r.height(),
            t
        );
    }
    eprintln!("folio: {} match(es)", rows.len());
    Ok(())
}

fn redact_cmd(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    let mut opts = RedactOptions::default();
    if args.has("no-fill") {
        opts.fill = None;
    }
    if let Some(c) = args.flag("color") {
        let v: Vec<f64> = c.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        if v.len() != 3 {
            return Err("--color expects r,g,b in 0..1".into());
        }
        opts.fill = Some([v[0], v[1], v[2]]);
    }
    let needles = args.all("text");
    let areas = args.all("area");
    if needles.is_empty() && areas.is_empty() {
        return Err("redact needs --text \"...\" or --area PAGE:x,y,w,h".into());
    }
    let idx = range(args, &doc, "pages")?;
    let so = search_opts(args);
    let mut total = redact::RedactReport::default();
    let mut matches = 0;
    for n in &needles {
        let (r, m) =
            redact::redact_text(&mut doc, &idx, n, &so, &opts).map_err(|e| e.to_string())?;
        matches += m;
        total.merge(r);
    }
    for a in &areas {
        let (page, rest) = a
            .split_once(':')
            .ok_or_else(|| format!("--area expects PAGE:x,y,w,h, got '{a}'"))?;
        let page: usize = page
            .parse()
            .map_err(|_| format!("--area: bad page '{page}'"))?;
        let v: Vec<f64> = rest
            .split(',')
            .filter_map(|x| x.trim().parse().ok())
            .collect();
        if v.len() != 4 || page == 0 || page > doc.page_count() {
            return Err(format!(
                "--area expects PAGE:x,y,w,h (points from the bottom-left), got '{a}'"
            ));
        }
        let rect = foliopdf::Rect::from_xywh(v[0], v[1], v[2], v[3]);
        let r = redact::redact(&mut doc, page - 1, &[rect], &opts).map_err(|e| e.to_string())?;
        total.merge(r);
    }
    write(&mut doc, out, &save_opts(args)?)?;
    eprintln!(
        "folio: {} match(es); removed {} glyph(s), {} path(s), {} image(s) ({} more blanked), {} annotation(s)",
        matches, total.glyphs_removed, total.paths_removed, total.images_removed, total.images_edited, total.annotations_removed
    );
    for w in &total.warnings {
        eprintln!("folio: warning: {w}");
    }
    Ok(())
}

fn flatten(args: &Args) -> Result<(), String> {
    let (input, out) = (args.pos(1, "input file")?, args.pos(2, "output file")?);
    let mut doc = load(args, input)?;
    let idx = range(args, &doc, "pages")?;
    let only_forms = args.has("forms");
    let only_annots = args.has("annots");
    let opts = FlattenOptions {
        widgets: Some(!only_annots),
        subtypes: if only_forms {
            Some(vec!["Widget".into()])
        } else {
            None
        },
        ..Default::default()
    };
    let n = annot::flatten_annotations(&mut doc, &idx, &opts).map_err(|e| e.to_string())?;
    if !only_annots && idx.len() == doc.page_count() {
        forms::remove_form(&mut doc);
    }
    write(&mut doc, out, &save_opts(args)?)?;
    eprintln!("folio: flattened {n} annotation(s)");
    Ok(())
}

fn batch_cmd(args: &Args) -> Result<(), String> {
    let preset_path = args.pos(1, "preset file")?;
    let json = std::fs::read_to_string(preset_path).map_err(|e| format!("{preset_path}: {e}"))?;
    // Accept either a single preset or a store plus --preset NAME.
    let preset = match Preset::from_json(&json) {
        Ok(p) => p,
        Err(e) => {
            let store = PresetStore::from_json(&json).map_err(|_| format!("{preset_path}: {e}"))?;
            let name = args
                .flag("preset")
                .ok_or("file is a preset store; choose one with --preset NAME")?;
            store
                .get(name)
                .cloned()
                .ok_or_else(|| format!("no preset named '{name}' in {preset_path}"))?
        }
    };
    if args.positional.len() < 3 {
        return Err("batch needs at least one input".into());
    }
    let mut inputs = Vec::new();
    for p in &args.positional[2..] {
        let data = std::fs::read(p).map_err(|e| format!("{p}: {e}"))?;
        let mut inp = Input::new(p, data);
        if let Some(pw) = args.flag("password") {
            inp = inp.with_password(pw);
        }
        inputs.push(inp);
    }
    let mut assets = Vec::new();
    for a in args.all("asset") {
        let (name, path) = a.split_once('=').ok_or("--asset expects name=path")?;
        let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        assets.push(Asset {
            name: name.into(),
            data,
        });
    }
    let out_dir = PathBuf::from(args.flag("out-dir").unwrap_or("."));
    std::fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let result = batch::run(&preset, &inputs, &assets).map_err(|e| e.to_string())?;
    for w in &result.warnings {
        eprintln!("folio: warning: {w}");
    }
    for o in &result.outputs {
        let path = out_dir.join(&o.name);
        std::fs::write(&path, &o.data).map_err(|e| format!("{}: {e}", path.display()))?;
        eprintln!(
            "folio: wrote {} ({} pages, {})",
            path.display(),
            o.pages,
            human(o.bytes)
        );
    }
    Ok(())
}

fn presets(args: &Args) -> Result<(), String> {
    if args.positional.get(1).map(String::as_str) == Some("export") {
        let path = args.pos(2, "output file")?;
        std::fs::write(path, PresetStore::with_builtins().to_json())
            .map_err(|e| format!("{path}: {e}"))?;
        eprintln!("folio: wrote {path}");
        return Ok(());
    }
    for p in Preset::builtins() {
        println!("{:<20} {}", p.name, p.description.unwrap_or_default());
    }
    println!("\nUse `folio presets export presets.json` to save them, then `folio batch presets.json --preset NAME in.pdf`.");
    Ok(())
}
