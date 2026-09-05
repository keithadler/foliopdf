//! Shows groups of objects the writer would merge as duplicates. `-- FILE`
use foliopdf::{Document, Object};
use std::collections::HashMap;
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let doc = Document::load(&std::fs::read(&a[0]).unwrap()).unwrap();
    let mut groups: HashMap<Vec<u8>, Vec<u32>> = HashMap::new();
    for (r, obj) in doc.objects() {
        let dict = match obj {
            Object::Stream(s) => &s.dict,
            Object::Dict(d) => d,
            _ => continue,
        };
        let ty = dict
            .get("Type")
            .and_then(Object::as_name)
            .map(|n| n.as_str().into_owned())
            .unwrap_or_default();
        if !(ty == "Font"
            || ty == "FontDescriptor"
            || ty == "XObject"
            || ty == "ExtGState"
            || ty == "Encoding"
            || matches!(obj, Object::Stream(_)))
        {
            continue;
        }
        let mut key = Vec::new();
        foliopdf::writer::serialize(&mut key, obj, &HashMap::new());
        groups.entry(key).or_default().push(r.num);
    }
    let mut n = 0;
    for (key, members) in groups.iter().filter(|(_, m)| m.len() > 1) {
        n += 1;
        if n > 12 {
            break;
        }
        let head = String::from_utf8_lossy(&key[..key.len().min(160)]).replace('\n', " ");
        println!(
            "{} objects identical: {:?}  :: {}",
            members.len(),
            &members[..members.len().min(6)],
            head
        );
    }
    // Font dicts: show a couple with their ToUnicode/FontDescriptor refs.
    let mut fonts = 0;
    for (r, obj) in doc.objects() {
        if let Some(d) = obj.as_dict() {
            if d.get("Type")
                .and_then(Object::as_name)
                .map(|n| n == "Font")
                .unwrap_or(false)
                && d.get("Subtype")
                    .and_then(Object::as_name)
                    .map(|n| n == "Type0")
                    .unwrap_or(false)
            {
                fonts += 1;
                if fonts <= 4 {
                    let mut k = Vec::new();
                    foliopdf::writer::serialize(&mut k, obj, &HashMap::new());
                    println!("font {}: {}", r.num, String::from_utf8_lossy(&k));
                }
            }
        }
    }
    println!("{fonts} Type0 fonts");
}
