//! Dumps one page's font resource chain. `-- FILE PAGE FONTNAME`
use foliopdf::{Document, Object};
use std::collections::HashMap;
fn ser(doc: &Document, o: &Object) -> String {
    let mut k = Vec::new();
    foliopdf::writer::serialize(&mut k, o, &HashMap::new());
    let s = String::from_utf8_lossy(&k).into_owned();
    let _ = doc;
    s.chars().take(300).collect()
}
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let doc = Document::load(&std::fs::read(&a[0]).unwrap()).unwrap();
    let page: usize = a[1].parse::<usize>().unwrap() - 1;
    let info = doc.page_info(page).unwrap();
    let res = doc
        .page_attr(info.obj, "Resources")
        .map(|o| doc.resolve(o).clone())
        .unwrap();
    let fonts = doc
        .dict_get(res.as_dict().unwrap(), "Font")
        .unwrap()
        .as_dict()
        .unwrap()
        .clone();
    let fref = fonts.get(&a[2]).and_then(Object::as_reference).unwrap();
    let f = doc.get(fref).clone();
    println!("font {} = {}", fref, ser(&doc, &f));
    let fd = f.as_dict().unwrap();
    if let Some(Object::Reference(tu)) = fd.get("ToUnicode") {
        let s = doc.get(*tu).as_stream().unwrap();
        let data = doc.stream_data(s).unwrap();
        println!(
            "ToUnicode {} raw {} bytes, decoded {} bytes: {}",
            tu,
            s.data.len(),
            data.len(),
            String::from_utf8_lossy(&data)
                .replace('\n', " ")
                .chars()
                .take(220)
                .collect::<String>()
        );
    }
    if let Some(Object::Array(df)) = fd.get("DescendantFonts") {
        if let Some(Object::Reference(d)) = df.first() {
            let dd = doc.get(*d).clone();
            println!("descendant {} = {}", d, ser(&doc, &dd));
            if let Some(Object::Reference(desc)) =
                dd.as_dict().and_then(|x| x.get("FontDescriptor"))
            {
                let de = doc.get(*desc).clone();
                println!("descriptor {} = {}", desc, ser(&doc, &de));
                if let Some(Object::Reference(ff)) = de.as_dict().and_then(|x| x.get("FontFile2")) {
                    let s = doc.get(*ff).as_stream().unwrap();
                    println!("fontfile2 {} raw {} bytes", ff, s.data.len());
                }
            }
        }
    }
}
