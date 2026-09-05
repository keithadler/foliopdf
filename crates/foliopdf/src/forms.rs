//! Interactive forms (AcroForm, ISO 32000-1 §12.7): list fields, fill them
//! in, generate appearances, and flatten.
//!
//! Field geometry is reported in display space (see [`crate::annot`]).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::annot::{self, make_form, wrap_text};
use crate::content::{write_num, ContentBuilder};
use crate::document::Document;
use crate::error::{Error, Result};
use crate::font::{Font, StandardFont};
use crate::geometry::{Matrix, Rect};
use crate::lexer::{Lexer, Token};
use crate::object::{Dict, ObjRef, Object, PdfString};

/// The type of a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[allow(missing_docs)]
pub enum FieldKind {
    Text,
    Checkbox,
    Radio,
    /// Drop-down list.
    Combo,
    /// Scrolling list box.
    List,
    /// Push button (no value).
    Button,
    Signature,
    Unknown,
}

/// One choice of a list or drop-down.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FieldOption {
    /// The export value stored in the file.
    pub value: String,
    /// What the reader sees.
    pub label: String,
}

/// One on-page widget of a field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Widget {
    /// Page index, if the widget is on a page.
    pub page: Option<usize>,
    /// Bounds in display space.
    pub rect: Rect,
    /// For check boxes and radio buttons: the name of the "on" state.
    pub on_state: Option<String>,
    /// Object number.
    pub object: u32,
}

/// A form field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    /// Fully qualified name (`parent.child`).
    pub name: String,
    /// What kind of field.
    pub kind: FieldKind,
    /// Current value. For check boxes: the on-state name or `Off`.
    pub value: Option<String>,
    /// All selected values of a multi-select list.
    pub values: Vec<String>,
    /// Choices (lists, drop-downs, radio groups).
    pub options: Vec<FieldOption>,
    /// Page of the first widget.
    pub page: Option<usize>,
    /// Bounds of the first widget in display space.
    pub rect: Option<Rect>,
    /// Every widget.
    pub widgets: Vec<Widget>,
    /// Whether the field is read-only.
    pub read_only: bool,
    /// Whether the field is marked required.
    pub required: bool,
    /// Whether a text field wraps onto several lines.
    pub multiline: bool,
    /// Whether a text field hides its value.
    pub password: bool,
    /// Maximum length of a text field.
    pub max_len: Option<usize>,
    /// Object number of the field dictionary.
    pub object: u32,
}

/// A value to store in a field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum FieldValue {
    /// A check box or radio button state.
    Bool(bool),
    /// Text, a choice's export value, or a radio button's on-state name.
    Text(String),
    /// Several choices of a multi-select list.
    List(Vec<String>),
}

#[derive(Default, Clone)]
struct Inherited {
    ft: Option<String>,
    ff: i64,
    v: Option<Object>,
    da: Option<Vec<u8>>,
    q: i64,
    opt: Option<Vec<Object>>,
    max_len: Option<usize>,
}

fn acroform(doc: &Document) -> Option<&Dict> {
    doc.dict_get(doc.catalog(), "AcroForm")
        .and_then(Object::as_dict)
}

fn refs(doc: &Document, o: Option<&Object>) -> Vec<ObjRef> {
    match o.map(|o| doc.resolve(o)) {
        Some(Object::Array(a)) => a.iter().filter_map(Object::as_reference).collect(),
        _ => Vec::new(),
    }
}

fn text(doc: &Document, d: &Dict, key: &str) -> Option<String> {
    doc.dict_get(d, key)
        .and_then(Object::as_string)
        .map(PdfString::to_text)
}

fn is_widget(doc: &Document, d: &Dict) -> bool {
    doc.dict_get(d, "Subtype")
        .and_then(Object::as_name)
        .map(|n| n == "Widget")
        .unwrap_or(false)
}

fn has_title(doc: &Document, r: ObjRef) -> bool {
    doc.get(r)
        .as_dict()
        .map(|d| d.contains("T"))
        .unwrap_or(false)
}

fn on_state(doc: &Document, widget: &Dict) -> Option<String> {
    let ap = doc.dict_get(widget, "AP").and_then(Object::as_dict)?;
    for key in ["N", "D"] {
        if let Some(states) = doc.dict_get(ap, key).and_then(Object::as_dict) {
            for (k, _) in states.iter() {
                if k != "Off" {
                    return Some(k.as_str().into_owned());
                }
            }
        }
    }
    None
}

fn value_of(o: &Object, doc: &Document) -> (Option<String>, Vec<String>) {
    match doc.resolve(o) {
        Object::String(s) => (Some(s.to_text()), Vec::new()),
        Object::Name(n) => (Some(n.as_str().into_owned()), Vec::new()),
        Object::Array(a) => {
            let vs: Vec<String> = a
                .iter()
                .filter_map(|x| match doc.resolve(x) {
                    Object::String(s) => Some(s.to_text()),
                    Object::Name(n) => Some(n.as_str().into_owned()),
                    _ => None,
                })
                .collect();
            (vs.first().cloned(), vs)
        }
        _ => (None, Vec::new()),
    }
}

fn options_of(doc: &Document, opt: &[Object]) -> Vec<FieldOption> {
    opt.iter()
        .filter_map(|o| match doc.resolve(o) {
            Object::String(s) => Some(FieldOption {
                value: s.to_text(),
                label: s.to_text(),
            }),
            Object::Array(pair) if !pair.is_empty() => {
                let v = doc
                    .resolve(&pair[0])
                    .as_string()
                    .map(PdfString::to_text)
                    .unwrap_or_default();
                let l = pair
                    .get(1)
                    .and_then(|x| doc.resolve(x).as_string())
                    .map(PdfString::to_text)
                    .unwrap_or_else(|| v.clone());
                Some(FieldOption { value: v, label: l })
            }
            _ => None,
        })
        .collect()
}

struct Walker<'a> {
    doc: &'a Document,
    pages: HashMap<u32, usize>,
    out: Vec<Field>,
    seen: HashSet<u32>,
}

impl Walker<'_> {
    fn inherit(&self, d: &Dict, inh: &Inherited) -> Inherited {
        let doc = self.doc;
        let mut i = inh.clone();
        if let Some(ft) = doc.dict_get(d, "FT").and_then(Object::as_name) {
            i.ft = Some(ft.as_str().into_owned());
        }
        if let Some(ff) = doc.dict_get(d, "Ff").and_then(Object::as_i64) {
            i.ff = ff;
        }
        if let Some(v) = d.get("V") {
            i.v = Some(doc.resolve(v).clone());
        }
        if let Some(da) = doc.dict_get(d, "DA").and_then(Object::as_string) {
            i.da = Some(da.as_bytes().to_vec());
        }
        if let Some(q) = doc.dict_get(d, "Q").and_then(Object::as_i64) {
            i.q = q;
        }
        if let Some(Object::Array(a)) = doc.dict_get(d, "Opt") {
            i.opt = Some(a.clone());
        }
        if let Some(m) = doc.dict_get(d, "MaxLen").and_then(Object::as_i64) {
            i.max_len = Some(m.max(0) as usize);
        }
        i
    }

    fn walk(&mut self, r: ObjRef, prefix: &str, inh: &Inherited) {
        if !self.seen.insert(r.num) {
            return;
        }
        let doc = self.doc;
        let d = match doc.get(r).as_dict() {
            Some(d) => d.clone(),
            None => return,
        };
        let name = match text(doc, &d, "T") {
            Some(t) if prefix.is_empty() => t,
            Some(t) => format!("{prefix}.{t}"),
            None => prefix.to_string(),
        };
        let inh = self.inherit(&d, inh);
        let kids = refs(doc, d.get("Kids"));
        let field_kids: Vec<ObjRef> = kids
            .iter()
            .copied()
            .filter(|k| has_title(doc, *k))
            .collect();
        let widget_kids: Vec<ObjRef> = kids
            .iter()
            .copied()
            .filter(|k| !has_title(doc, *k))
            .collect();
        for k in &field_kids {
            self.walk(*k, &name, &inh);
        }
        let widgets: Vec<ObjRef> = if widget_kids.is_empty() {
            if is_widget(doc, &d) || kids.is_empty() && inh.ft.is_some() {
                vec![r]
            } else {
                Vec::new()
            }
        } else {
            widget_kids
        };
        if widgets.is_empty() || inh.ft.is_none() && !is_widget(doc, &d) {
            return;
        }
        for w in &widgets {
            self.seen.insert(w.num);
        }
        let ff = inh.ff;
        let kind = match inh.ft.as_deref() {
            Some("Tx") => FieldKind::Text,
            Some("Btn") if ff & (1 << 16) != 0 => FieldKind::Button,
            Some("Btn") if ff & (1 << 15) != 0 => FieldKind::Radio,
            Some("Btn") => FieldKind::Checkbox,
            Some("Ch") if ff & (1 << 17) != 0 => FieldKind::Combo,
            Some("Ch") => FieldKind::List,
            Some("Sig") => FieldKind::Signature,
            _ => FieldKind::Unknown,
        };
        let (mut value, values) = inh
            .v
            .as_ref()
            .map(|v| value_of(v, doc))
            .unwrap_or((None, Vec::new()));
        let mut options = inh
            .opt
            .as_deref()
            .map(|o| options_of(doc, o))
            .unwrap_or_default();
        let ws: Vec<Widget> = widgets
            .iter()
            .map(|&w| {
                let wd = doc.get(w).as_dict().cloned().unwrap_or_default();
                let page = self.pages.get(&w.num).copied();
                let raw = doc
                    .dict_get(&wd, "Rect")
                    .and_then(Rect::from_object)
                    .unwrap_or_default();
                let rect = match page.and_then(|p| doc.page_info(p).ok()) {
                    Some(info) => annot::to_display_rect(&info, &raw),
                    None => raw,
                };
                Widget {
                    page,
                    rect,
                    on_state: on_state(doc, &wd),
                    object: w.num,
                }
            })
            .collect();
        if kind == FieldKind::Radio && options.is_empty() {
            options = ws
                .iter()
                .filter_map(|w| w.on_state.clone())
                .map(|s| FieldOption {
                    label: s.clone(),
                    value: s,
                })
                .collect();
        }
        if matches!(kind, FieldKind::Checkbox | FieldKind::Radio) && value.is_none() {
            value = Some("Off".into());
        }
        self.out.push(Field {
            name,
            kind,
            value,
            values,
            options,
            page: ws.first().and_then(|w| w.page),
            rect: ws.first().map(|w| w.rect),
            widgets: ws,
            read_only: ff & 1 != 0,
            required: ff & 2 != 0,
            multiline: ff & (1 << 12) != 0,
            password: ff & (1 << 13) != 0,
            max_len: inh.max_len,
            object: r.num,
        });
    }
}

/// Lists every field, including widgets that are on a page but missing
/// from the `/AcroForm` field list (a common defect).
pub fn list_fields(doc: &Document) -> Vec<Field> {
    let mut w = Walker {
        doc,
        pages: doc.annot_pages(),
        out: Vec::new(),
        seen: HashSet::new(),
    };
    let roots = acroform(doc)
        .map(|a| refs(doc, a.get("Fields")))
        .unwrap_or_default();
    for r in roots {
        w.walk(r, "", &Inherited::default());
    }
    // Orphans: climb to the top of their parent chain, then walk from there.
    for i in 0..doc.page_count() {
        for r in doc.page_annots(i).unwrap_or_default() {
            if w.seen.contains(&r.num) {
                continue;
            }
            let d = match doc.get(r).as_dict() {
                Some(d) => d,
                None => continue,
            };
            if !is_widget(doc, d) {
                continue;
            }
            let mut top = r;
            let mut guard = 0;
            while let Some(p) = doc
                .get(top)
                .as_dict()
                .and_then(|d| d.get("Parent"))
                .and_then(Object::as_reference)
            {
                guard += 1;
                if guard > 64 || w.seen.contains(&p.num) {
                    break;
                }
                top = p;
            }
            w.walk(top, "", &Inherited::default());
        }
    }
    w.out
}

/// Whether the document has any form fields.
pub fn has_fields(doc: &Document) -> bool {
    !list_fields(doc).is_empty()
}

// ---------------------------------------------------------------------------
// Filling
// ---------------------------------------------------------------------------

/// Sets one field by fully qualified name and regenerates its appearance.
pub fn set_field(doc: &mut Document, name: &str, value: &FieldValue) -> Result<()> {
    let fields = list_fields(doc);
    let f = fields
        .iter()
        .find(|f| f.name == name)
        .or_else(|| {
            fields
                .iter()
                .find(|f| f.name.rsplit('.').next() == Some(name))
        })
        .ok_or_else(|| Error::Preset(format!("no form field named '{name}'")))?
        .clone();
    apply(doc, &f, value)
}

/// Sets several fields at once. Unknown names are reported in the returned
/// list rather than failing the whole batch.
pub fn set_fields(doc: &mut Document, values: &[(String, FieldValue)]) -> Result<Vec<String>> {
    let fields = list_fields(doc);
    let mut missing = Vec::new();
    for (name, value) in values {
        match fields.iter().find(|f| &f.name == name).or_else(|| {
            fields
                .iter()
                .find(|f| f.name.rsplit('.').next() == Some(name.as_str()))
        }) {
            Some(f) => apply(doc, &f.clone(), value)?,
            None => missing.push(name.clone()),
        }
    }
    Ok(missing)
}

fn truthy(s: &str) -> bool {
    matches!(
        s.to_ascii_lowercase().as_str(),
        "true" | "yes" | "on" | "1" | "checked" | "x"
    )
}

fn set_key(doc: &mut Document, r: ObjRef, key: &str, value: Object) {
    if let Some(d) = doc.get_mut(r).and_then(Object::as_dict_mut) {
        d.set(key, value);
    }
}

fn remove_key(doc: &mut Document, r: ObjRef, key: &str) {
    if let Some(d) = doc.get_mut(r).and_then(Object::as_dict_mut) {
        d.remove(key);
    }
}

fn apply(doc: &mut Document, f: &Field, value: &FieldValue) -> Result<()> {
    let field_ref = ObjRef::new(f.object, 0);
    match f.kind {
        FieldKind::Text | FieldKind::Combo | FieldKind::List => {
            let (text, list): (String, Vec<String>) = match value {
                FieldValue::Text(s) => (s.clone(), vec![s.clone()]),
                FieldValue::Bool(b) => (if *b { "Yes".into() } else { String::new() }, Vec::new()),
                FieldValue::List(v) => (v.first().cloned().unwrap_or_default(), v.clone()),
            };
            let mut text = text;
            if let Some(m) = f.max_len {
                if m > 0 && text.chars().count() > m {
                    text = text.chars().take(m).collect();
                }
            }
            if f.kind == FieldKind::List && list.len() > 1 {
                set_key(
                    doc,
                    field_ref,
                    "V",
                    Object::Array(
                        list.iter()
                            .map(|s| PdfString::from_text(s).into())
                            .collect(),
                    ),
                );
            } else if text.is_empty() {
                remove_key(doc, field_ref, "V");
            } else {
                set_key(doc, field_ref, "V", PdfString::from_text(&text).into());
            }
            // Selected indices for list boxes.
            if f.kind == FieldKind::List {
                let idx: Vec<Object> = f
                    .options
                    .iter()
                    .enumerate()
                    .filter(|(_, o)| list.contains(&o.value))
                    .map(|(i, _)| Object::Integer(i as i64))
                    .collect();
                if idx.is_empty() {
                    remove_key(doc, field_ref, "I");
                } else {
                    set_key(doc, field_ref, "I", Object::Array(idx));
                }
            }
            for w in &f.widgets {
                let shown = if f.kind == FieldKind::Text {
                    vec![text.clone()]
                } else {
                    list.clone()
                };
                text_appearance(doc, f, ObjRef::new(w.object, 0), &shown)?;
            }
        }
        FieldKind::Checkbox | FieldKind::Radio => {
            let chosen: Option<String> = match value {
                FieldValue::Bool(true) => f
                    .widgets
                    .iter()
                    .find_map(|w| w.on_state.clone())
                    .or_else(|| Some("Yes".into())),
                FieldValue::Bool(false) => None,
                FieldValue::Text(s)
                    if s.is_empty()
                        || s.eq_ignore_ascii_case("off")
                        || s.eq_ignore_ascii_case("false")
                        || s == "0" =>
                {
                    None
                }
                FieldValue::Text(s) => {
                    if let Some(w) = f
                        .widgets
                        .iter()
                        .find(|w| w.on_state.as_deref() == Some(s.as_str()))
                    {
                        w.on_state.clone()
                    } else if let Some(i) = f
                        .options
                        .iter()
                        .position(|o| &o.value == s || &o.label == s)
                    {
                        // Radio groups with /Opt: the i-th widget is the i-th option.
                        f.widgets
                            .get(i)
                            .and_then(|w| w.on_state.clone())
                            .or_else(|| Some(s.clone()))
                    } else if truthy(s) {
                        f.widgets
                            .iter()
                            .find_map(|w| w.on_state.clone())
                            .or_else(|| Some("Yes".into()))
                    } else {
                        return Err(Error::Preset(format!(
                            "'{s}' is not a choice of '{}'",
                            f.name
                        )));
                    }
                }
                FieldValue::List(v) => v
                    .first()
                    .and_then(|s| {
                        f.widgets
                            .iter()
                            .find(|w| w.on_state.as_deref() == Some(s.as_str()))
                    })
                    .and_then(|w| w.on_state.clone()),
            };
            match &chosen {
                Some(on) => set_key(doc, field_ref, "V", Object::name(on)),
                None => set_key(doc, field_ref, "V", Object::name("Off")),
            }
            for w in &f.widgets {
                let wr = ObjRef::new(w.object, 0);
                ensure_check_appearance(doc, f, wr, w.on_state.as_deref().unwrap_or("Yes"))?;
                let on = w.on_state.as_deref().unwrap_or("Yes");
                let is_on = chosen.as_deref() == Some(on);
                set_key(doc, wr, "AS", Object::name(if is_on { on } else { "Off" }));
                if f.widgets.len() == 1 && wr != field_ref {
                    // Nothing else to do: V lives on the parent.
                }
            }
        }
        FieldKind::Button | FieldKind::Signature | FieldKind::Unknown => {
            return Err(Error::Preset(format!(
                "'{}' cannot be filled ({:?})",
                f.name, f.kind
            )));
        }
    }
    // Viewers should trust the appearances we generated.
    if let Some(cat) = doc.catalog_ref() {
        if let Some(af) = doc
            .get(cat)
            .as_dict()
            .and_then(|c| c.get("AcroForm"))
            .cloned()
        {
            match af {
                Object::Reference(r) => remove_key(doc, r, "NeedAppearances"),
                Object::Dict(_) => {
                    if let Some(d) = doc
                        .catalog_mut()
                        .get_mut("AcroForm")
                        .and_then(Object::as_dict_mut)
                    {
                        d.remove("NeedAppearances");
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// Parsed default appearance string: font resource name, size, colour ops.
struct DefaultAppearance {
    font: Option<String>,
    size: f64,
    color: Vec<u8>,
}

fn parse_da(da: &[u8]) -> DefaultAppearance {
    let mut lex = Lexer::new(da);
    let mut stack: Vec<Token> = Vec::new();
    let mut out = DefaultAppearance {
        font: None,
        size: 0.0,
        color: Vec::new(),
    };
    loop {
        let t = match lex.next_token() {
            Ok(Token::Eof) | Err(_) => break,
            Ok(t) => t,
        };
        match t {
            Token::Keyword(k) => {
                let k = String::from_utf8_lossy(&k).into_owned();
                match k.as_str() {
                    "Tf" if stack.len() >= 2 => {
                        if let (Token::Name(n), Some(sz)) =
                            (&stack[stack.len() - 2], num(&stack[stack.len() - 1]))
                        {
                            out.font = Some(n.as_str().into_owned());
                            out.size = sz;
                        }
                    }
                    "g" | "rg" | "k" => {
                        let n = match k.as_str() {
                            "g" => 1,
                            "rg" => 3,
                            _ => 4,
                        };
                        if stack.len() >= n {
                            let mut c = Vec::new();
                            for t in &stack[stack.len() - n..] {
                                if let Some(v) = num(t) {
                                    write_num(&mut c, v.clamp(0.0, 1.0));
                                    c.push(b' ');
                                }
                            }
                            c.extend_from_slice(k.as_bytes());
                            out.color = c;
                        }
                    }
                    _ => {}
                }
                stack.clear();
            }
            other => stack.push(other),
        }
    }
    out
}

fn num(t: &Token) -> Option<f64> {
    match t {
        Token::Integer(i) => Some(*i as f64),
        Token::Real(r) => Some(*r),
        _ => None,
    }
}

/// Finds (or installs) a standard-14 font in `/AcroForm /DR` for drawing
/// field text. Returns the resource name, its reference and the metrics.
fn field_font(doc: &mut Document, wanted: Option<&str>) -> (String, ObjRef, StandardFont) {
    let dr_font = |doc: &Document, name: &str| -> Option<(ObjRef, StandardFont)> {
        let af = acroform(doc)?;
        let dr = doc.dict_get(af, "DR").and_then(Object::as_dict)?;
        let fonts = doc.dict_get(dr, "Font").and_then(Object::as_dict)?;
        let r = fonts.get(name).and_then(Object::as_reference)?;
        let fd = doc.get(r).as_dict()?;
        let base = doc.dict_get(fd, "BaseFont").and_then(Object::as_name)?;
        let subtype = doc
            .dict_get(fd, "Subtype")
            .and_then(Object::as_name)
            .map(|n| n.as_str().into_owned())
            .unwrap_or_default();
        if subtype == "Type0" {
            return None;
        }
        let sf = StandardFont::by_name(&base.as_str())?;
        Some((r, sf))
    };
    if let Some(n) = wanted {
        if let Some((r, sf)) = dr_font(doc, n) {
            return (n.to_string(), r, sf);
        }
        // Common Acrobat aliases that may be missing from DR.
        if let Some(sf) = StandardFont::by_name(match n {
            "Helv" => "Helvetica",
            "HeBo" => "Helvetica-Bold",
            "TiRo" => "Times-Roman",
            "TiBo" => "Times-Bold",
            "Cour" => "Courier",
            "CoBo" => "Courier-Bold",
            other => other,
        }) {
            let r = install_dr_font(doc, n, sf);
            return (n.to_string(), r, sf);
        }
    }
    if let Some((r, sf)) = dr_font(doc, "Helv") {
        return ("Helv".into(), r, sf);
    }
    let r = install_dr_font(doc, "Helv", StandardFont::Helvetica);
    ("Helv".into(), r, StandardFont::Helvetica)
}

fn install_dr_font(doc: &mut Document, name: &str, sf: StandardFont) -> ObjRef {
    let font = doc.add(
        Dict::new()
            .with("Type", "Font")
            .with("Subtype", "Type1")
            .with("BaseFont", sf.base_font())
            .with("Encoding", "WinAnsiEncoding")
            .into(),
    );
    let mut af = acroform_owned(doc);
    let mut dr = af
        .get("DR")
        .map(|o| doc.resolve(o).clone())
        .and_then(Object::into_dict)
        .unwrap_or_default();
    let mut fonts = dr
        .get("Font")
        .map(|o| doc.resolve(o).clone())
        .and_then(Object::into_dict)
        .unwrap_or_default();
    fonts.set(name, font);
    dr.set("Font", fonts);
    af.set("DR", dr);
    store_acroform(doc, af);
    font
}

/// The AcroForm dictionary as an owned value (empty if absent).
fn acroform_owned(doc: &Document) -> Dict {
    acroform(doc).cloned().unwrap_or_default()
}

/// Writes the AcroForm dictionary back, keeping it indirect when it was.
fn store_acroform(doc: &mut Document, af: Dict) {
    let existing = doc.catalog().get("AcroForm").cloned();
    match existing {
        Some(Object::Reference(r)) => doc.set(r, af.into()),
        _ => {
            doc.catalog_mut().set("AcroForm", af);
        }
    }
}

fn zadb_font(doc: &mut Document) -> ObjRef {
    // ZapfDingbats for check marks; installed into DR as /ZaDb like Acrobat does.
    let af = acroform_owned(doc);
    if let Some(r) = af
        .get("DR")
        .map(|o| doc.resolve(o).clone())
        .and_then(Object::into_dict)
        .and_then(|dr| dr.get("Font").map(|o| doc.resolve(o).clone()))
        .and_then(Object::into_dict)
        .and_then(|f| f.get("ZaDb").and_then(Object::as_reference))
    {
        return r;
    }
    let font = doc.add(
        Dict::new()
            .with("Type", "Font")
            .with("Subtype", "Type1")
            .with("BaseFont", "ZapfDingbats")
            .into(),
    );
    let mut af = acroform_owned(doc);
    let mut dr = af
        .get("DR")
        .map(|o| doc.resolve(o).clone())
        .and_then(Object::into_dict)
        .unwrap_or_default();
    let mut fonts = dr
        .get("Font")
        .map(|o| doc.resolve(o).clone())
        .and_then(Object::into_dict)
        .unwrap_or_default();
    fonts.set("ZaDb", font);
    dr.set("Font", fonts);
    af.set("DR", dr);
    store_acroform(doc, af);
    font
}

/// Widget geometry for appearance generation: rect, rotation, and the
/// resulting BBox size and Matrix.
struct WidgetBox {
    w: f64,
    h: f64,
    matrix: Matrix,
    bg: Option<Vec<f64>>,
    bc: Option<Vec<f64>>,
    border_width: f64,
}

fn widget_box(doc: &Document, wr: ObjRef) -> Option<WidgetBox> {
    let d = doc.get(wr).as_dict()?;
    let rect = doc.dict_get(d, "Rect").and_then(Rect::from_object)?;
    let (rw, rh) = (rect.width(), rect.height());
    let mk = doc.dict_get(d, "MK").and_then(Object::as_dict);
    let rot = mk
        .and_then(|m| doc.dict_get(m, "R"))
        .and_then(Object::as_i64)
        .unwrap_or(0)
        .rem_euclid(360);
    let color = |key: &str| -> Option<Vec<f64>> {
        let a = doc.dict_get(mk?, key)?.as_array()?;
        let v: Vec<f64> = a.iter().filter_map(Object::as_f64).collect();
        (!v.is_empty()).then_some(v)
    };
    let border_width = doc
        .dict_get(d, "BS")
        .and_then(Object::as_dict)
        .and_then(|bs| doc.dict_get(bs, "W"))
        .and_then(Object::as_f64)
        .unwrap_or(1.0);
    let (w, h, matrix) = match rot {
        90 => (rh, rw, Matrix::new(0.0, 1.0, -1.0, 0.0, rw, 0.0)),
        180 => (rw, rh, Matrix::new(-1.0, 0.0, 0.0, -1.0, rw, rh)),
        270 => (rh, rw, Matrix::new(0.0, -1.0, 1.0, 0.0, 0.0, rh)),
        _ => (rw, rh, Matrix::IDENTITY),
    };
    Some(WidgetBox {
        w,
        h,
        matrix,
        bg: color("BG"),
        bc: color("BC"),
        border_width,
    })
}

fn set_color(cb: &mut ContentBuilder, c: &[f64], fill: bool) {
    match (c.len(), fill) {
        (1, true) => cb.fill_gray(c[0]),
        (1, false) => cb.stroke_gray(c[0]),
        (3, true) => cb.fill_rgb(c[0], c[1], c[2]),
        (3, false) => cb.stroke_rgb(c[0], c[1], c[2]),
        (4, true) => cb.fill_cmyk(c[0], c[1], c[2], c[3]),
        (4, false) => {
            let mut raw = Vec::new();
            for v in c {
                write_num(&mut raw, *v);
                raw.push(b' ');
            }
            raw.extend_from_slice(b"K");
            cb.raw(&raw)
        }
        _ => cb,
    };
}

fn paint_background(cb: &mut ContentBuilder, b: &WidgetBox) {
    if let Some(bg) = &b.bg {
        set_color(cb, bg, true);
        cb.rect(&Rect::new(0.0, 0.0, b.w, b.h)).fill();
    }
    if let Some(bc) = &b.bc {
        if b.border_width > 0.0 {
            set_color(cb, bc, false);
            let bw = b.border_width;
            cb.line_width(bw)
                .rect(&Rect::new(
                    bw / 2.0,
                    bw / 2.0,
                    b.w - bw / 2.0,
                    b.h - bw / 2.0,
                ))
                .stroke();
        }
    }
}

fn text_appearance(doc: &mut Document, f: &Field, wr: ObjRef, values: &[String]) -> Result<()> {
    let b = match widget_box(doc, wr) {
        Some(b) => b,
        None => return Ok(()),
    };
    let da_bytes = {
        let d = doc.get(wr).as_dict().cloned().unwrap_or_default();
        let field_da = doc
            .get(ObjRef::new(f.object, 0))
            .as_dict()
            .and_then(|fd| doc.dict_get(fd, "DA"))
            .and_then(Object::as_string)
            .map(|s| s.as_bytes().to_vec());
        doc.dict_get(&d, "DA")
            .and_then(Object::as_string)
            .map(|s| s.as_bytes().to_vec())
            .or(field_da)
            .or_else(|| {
                acroform(doc)
                    .and_then(|a| doc.dict_get(a, "DA"))
                    .and_then(Object::as_string)
                    .map(|s| s.as_bytes().to_vec())
            })
            .unwrap_or_else(|| b"/Helv 0 Tf 0 g".to_vec())
    };
    let da = parse_da(&da_bytes);
    let (font_name, font_ref, sf) = field_font(doc, da.font.as_deref());
    let font = Font::standard(sf);
    let q = {
        let d = doc.get(wr).as_dict().cloned().unwrap_or_default();
        doc.dict_get(&d, "Q")
            .and_then(Object::as_i64)
            .or_else(|| {
                doc.get(ObjRef::new(f.object, 0))
                    .as_dict()
                    .and_then(|fd| doc.dict_get(fd, "Q"))
                    .and_then(Object::as_i64)
            })
            .unwrap_or(0)
    };
    let pad = 2.0;
    let bw = if b.bc.is_some() { b.border_width } else { 0.0 };
    let inner_w = (b.w - 2.0 * (pad + bw)).max(1.0);
    let inner_h = (b.h - 2.0 * (pad + bw)).max(1.0);
    let comb = f.kind == FieldKind::Text && f.max_len.unwrap_or(0) > 0 && {
        let ff = doc
            .get(ObjRef::new(f.object, 0))
            .as_dict()
            .and_then(|fd| doc.dict_get(fd, "Ff"))
            .and_then(Object::as_i64)
            .unwrap_or(0);
        ff & (1 << 24) != 0
    };
    let display: Vec<String> = if f.password {
        values
            .iter()
            .map(|v| "*".repeat(v.chars().count()))
            .collect()
    } else if f.kind == FieldKind::Combo {
        values
            .iter()
            .map(|v| {
                f.options
                    .iter()
                    .find(|o| &o.value == v)
                    .map(|o| o.label.clone())
                    .unwrap_or_else(|| v.clone())
            })
            .collect()
    } else {
        values.to_vec()
    };
    let mut cb = ContentBuilder::new();
    paint_background(&mut cb, &b);
    cb.raw(b"/Tx BMC").save();
    cb.rect(&Rect::new(bw, bw, b.w - bw, b.h - bw))
        .clip()
        .end_path();
    let color_ops = if da.color.is_empty() {
        b"0 g".to_vec()
    } else {
        da.color.clone()
    };
    if f.kind == FieldKind::List {
        // List box: every option, selected ones on a blue band.
        let size = if da.size > 0.0 { da.size } else { 12.0 };
        let lead = size * 1.15;
        let mut y = b.h - bw - pad - lead;
        for o in &f.options {
            let selected = values.contains(&o.value);
            if selected {
                cb.fill_rgb(0.6, 0.75, 0.85)
                    .rect(&Rect::new(
                        bw,
                        y - size * 0.25,
                        b.w - bw,
                        y + lead - size * 0.25,
                    ))
                    .fill();
            }
            cb.begin_text()
                .raw(&color_ops)
                .font(&font_name, size)
                .text_matrix(&Matrix::translate(bw + pad, y))
                .show_literal(&Font::standard(sf).encode(&o.label))
                .end_text();
            y -= lead;
            if y < -lead {
                break;
            }
        }
    } else if comb {
        let n = f.max_len.unwrap_or(1).max(1);
        let cell = b.w / n as f64;
        let size = if da.size > 0.0 {
            da.size
        } else {
            (inner_h * 0.75).min(cell * 1.2).max(4.0)
        };
        let text = display.first().cloned().unwrap_or_default();
        let y = (b.h - size * 0.72) / 2.0;
        cb.begin_text().raw(&color_ops).font(&font_name, size);
        for (i, ch) in text.chars().enumerate().take(n) {
            let s = ch.to_string();
            let cw = font.measure(&s, size);
            cb.text_matrix(&Matrix::translate(cell * i as f64 + (cell - cw) / 2.0, y))
                .show_literal(&Font::standard(sf).encode(&s));
        }
        cb.end_text();
    } else {
        let text = display.first().cloned().unwrap_or_default();
        let multiline = f.multiline && f.kind == FieldKind::Text;
        let mut size = da.size;
        if size <= 0.0 {
            size = if multiline {
                12.0
            } else {
                (inner_h * 0.72).clamp(4.0, 48.0)
            };
            if !multiline {
                let w = font.measure(&text, size);
                if w > inner_w && w > 0.0 {
                    size = (size * inner_w / w).max(4.0);
                }
            }
        }
        let ascent = font.ascent() * size / 1000.0;
        let descent = font.descent() * size / 1000.0;
        let lines = if multiline {
            wrap_text(&font, size, &text, inner_w)
        } else {
            vec![text.replace(['\n', '\r'], " ")]
        };
        let lead = size * 1.15;
        let mut y = if multiline {
            b.h - bw - pad - ascent
        } else {
            (b.h - (ascent - descent)) / 2.0 - descent
        };
        cb.begin_text().raw(&color_ops).font(&font_name, size);
        for line in &lines {
            let w = font.measure(line, size);
            let x = match q {
                1 => (b.w - w) / 2.0,
                2 => b.w - bw - pad - w,
                _ => bw + pad,
            };
            cb.text_matrix(&Matrix::translate(x, y))
                .show_literal(&Font::standard(sf).encode(line));
            y -= lead;
            if y + ascent < 0.0 {
                break;
            }
        }
        cb.end_text();
    }
    cb.restore().raw(b"EMC");
    let mut res = Dict::new().with("Font", Dict::new().with(&font_name, font_ref));
    if font_name != "Helv" {
        // Keep the resource self-contained even if DR changes later.
        res = Dict::new().with("Font", Dict::new().with(&font_name, font_ref));
    }
    let form = make_form(doc, b.w, b.h, Some(b.matrix), res, cb.finish());
    set_key(doc, wr, "AP", Dict::new().with("N", form).into());
    Ok(())
}

/// Makes sure a check box / radio widget has `/AP /N << /<on> ... /Off ... >>`.
fn ensure_check_appearance(doc: &mut Document, f: &Field, wr: ObjRef, on: &str) -> Result<()> {
    let has = {
        let d = doc.get(wr).as_dict().cloned().unwrap_or_default();
        doc.dict_get(&d, "AP")
            .and_then(Object::as_dict)
            .and_then(|ap| doc.dict_get(ap, "N"))
            .and_then(Object::as_dict)
            .map(|n| {
                let is_stream = |k: &str| {
                    n.get(k)
                        .map(|o| doc.resolve(o).as_stream().is_some())
                        .unwrap_or(false)
                };
                is_stream(on) && is_stream("Off")
            })
            .unwrap_or(false)
    };
    if has {
        return Ok(());
    }
    let b = match widget_box(doc, wr) {
        Some(b) => b,
        None => return Ok(()),
    };
    let zadb = zadb_font(doc);
    let res = Dict::new().with("Font", Dict::new().with("ZaDb", zadb));
    let mut off = ContentBuilder::new();
    paint_background(&mut off, &b);
    let mut on_cb = ContentBuilder::new();
    paint_background(&mut on_cb, &b);
    // ZapfDingbats: '4' is a check mark (a20), 'l' is a filled circle (a71).
    let (glyph, gw): (&[u8], f64) = if f.kind == FieldKind::Radio {
        (b"l", 791.0)
    } else {
        (b"4", 756.0)
    };
    let size = (b.h.min(b.w) * 0.8).max(2.0);
    let w = gw * size / 1000.0;
    on_cb
        .begin_text()
        .fill_gray(0.0)
        .font("ZaDb", size)
        .text_matrix(&Matrix::translate(
            (b.w - w) / 2.0,
            (b.h - size * 0.69) / 2.0,
        ))
        .show_literal(glyph)
        .end_text();
    let off_form = make_form(doc, b.w, b.h, Some(b.matrix), res.clone(), off.finish());
    let on_form = make_form(doc, b.w, b.h, Some(b.matrix), res, on_cb.finish());
    let mut n = Dict::new();
    n.set(on, on_form);
    n.set("Off", off_form);
    set_key(doc, wr, "AP", Dict::new().with("N", n).into());
    Ok(())
}

// ---------------------------------------------------------------------------
// Creating fields
// ---------------------------------------------------------------------------

/// A field to create with [`add_field`]. Geometry is in display space.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct NewField {
    /// Field name (must be unique in the document).
    pub name: String,
    /// Text, check box, radio group, drop-down or list.
    pub kind: FieldKind,
    /// Where the widget goes. For radio groups without explicit `widgets`,
    /// the buttons are laid out left to right inside this box.
    pub rect: Rect,
    /// Initial value (text, export value, or `true`/`false` for a check box).
    pub value: Option<String>,
    /// Choices for radio groups, drop-downs and lists.
    pub options: Vec<String>,
    /// One rectangle per radio button (same order as `options`).
    pub widgets: Vec<Rect>,
    /// Wrap text onto several lines.
    pub multiline: bool,
    /// Mark as required.
    pub required: bool,
    /// Read-only.
    pub read_only: bool,
    /// Hide typed characters.
    pub password: bool,
    /// Maximum number of characters.
    pub max_len: Option<usize>,
    /// Spread characters into `max_len` equal cells.
    pub comb: bool,
    /// Font size; 0 fits the text to the box.
    pub font_size: f64,
    /// Text colour.
    pub color: [f64; 3],
    /// Text alignment.
    pub align: annot::Align,
    /// Fill colour of the box.
    pub background: Option<[f64; 3]>,
    /// Border colour.
    pub border: Option<[f64; 3]>,
    /// Border width in points.
    pub border_width: f64,
    /// Tooltip shown by viewers.
    pub tooltip: Option<String>,
}

impl Default for NewField {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: FieldKind::Text,
            rect: Rect::default(),
            value: None,
            options: Vec::new(),
            widgets: Vec::new(),
            multiline: false,
            required: false,
            read_only: false,
            password: false,
            max_len: None,
            comb: false,
            font_size: 0.0,
            color: [0.0, 0.0, 0.0],
            align: annot::Align::Left,
            background: None,
            border: None,
            border_width: 1.0,
            tooltip: None,
        }
    }
}

fn ensure_acroform(doc: &mut Document) {
    let mut af = acroform_owned(doc);
    let has_helv = af
        .get("DR")
        .map(|o| doc.resolve(o).clone())
        .and_then(Object::into_dict)
        .and_then(|dr| dr.get("Font").map(|o| doc.resolve(o).clone()))
        .and_then(Object::into_dict)
        .map(|f| f.contains("Helv"))
        .unwrap_or(false);
    if !af.contains("DA") {
        af.set("DA", PdfString::new(b"/Helv 0 Tf 0 g".to_vec()));
    }
    if !af.contains("Fields") {
        af.set("Fields", Object::Array(Vec::new()));
    }
    store_acroform(doc, af);
    if !has_helv {
        install_dr_font(doc, "Helv", StandardFont::Helvetica);
    }
}

fn add_root_field(doc: &mut Document, r: ObjRef) {
    let mut af = acroform_owned(doc);
    let mut fields = af
        .get("Fields")
        .map(|o| doc.resolve(o).clone())
        .and_then(Object::into_array)
        .unwrap_or_default();
    fields.push(Object::Reference(r));
    af.set("Fields", Object::Array(fields));
    store_acroform(doc, af);
}

fn mk_dict(spec: &NewField, rotation: i64) -> Dict {
    let mut mk = Dict::new();
    if let Some(bg) = spec.background {
        mk.set(
            "BG",
            Object::Array(bg.iter().map(|&v| Object::Real(v)).collect()),
        );
    }
    if let Some(bc) = spec.border {
        mk.set(
            "BC",
            Object::Array(bc.iter().map(|&v| Object::Real(v)).collect()),
        );
    }
    if rotation != 0 {
        mk.set("R", rotation);
    }
    mk
}

fn da_string(spec: &NewField) -> PdfString {
    let mut da = Vec::new();
    da.extend_from_slice(b"/Helv ");
    write_num(&mut da, spec.font_size.max(0.0));
    da.extend_from_slice(b" Tf ");
    for c in spec.color {
        write_num(&mut da, c.clamp(0.0, 1.0));
        da.push(b' ');
    }
    da.extend_from_slice(b"rg");
    PdfString::new(da)
}

/// Creates a form field on `page`. Returns the field object.
pub fn add_field(doc: &mut Document, page: usize, spec: &NewField) -> Result<ObjRef> {
    if spec.name.trim().is_empty() {
        return Err(Error::Preset("field name must not be empty".into()));
    }
    if list_fields(doc).iter().any(|f| f.name == spec.name) {
        return Err(Error::Preset(format!(
            "a field named '{}' already exists",
            spec.name
        )));
    }
    if spec.rect.width() <= 0.0 || spec.rect.height() <= 0.0 {
        return Err(Error::Preset(
            "field rectangle must have a positive size".into(),
        ));
    }
    ensure_acroform(doc);
    let info = doc.page_info(page)?;
    let rotation = info.rotation.degrees();
    let user_rect = annot::to_user_rect(&info, &spec.rect);
    let mut ff: i64 = 0;
    if spec.read_only {
        ff |= 1;
    }
    if spec.required {
        ff |= 2;
    }
    let base = |ft: &str| -> Dict {
        let mut d = Dict::new()
            .with("Type", "Annot")
            .with("Subtype", "Widget")
            .with("FT", ft)
            .with("T", PdfString::from_text(&spec.name))
            .with("Rect", user_rect.to_object())
            .with("F", 4)
            .with("P", info.obj)
            .with("MK", mk_dict(spec, rotation))
            .with(
                "BS",
                Dict::new()
                    .with("W", spec.border_width.max(0.0))
                    .with("S", "S"),
            );
        if let Some(t) = &spec.tooltip {
            d.set("TU", PdfString::from_text(t));
        }
        d
    };
    let field = match spec.kind {
        FieldKind::Text => {
            if spec.multiline {
                ff |= 1 << 12;
            }
            if spec.password {
                ff |= 1 << 13;
            }
            if spec.comb && spec.max_len.unwrap_or(0) > 0 {
                ff |= 1 << 24;
            }
            let mut d = base("Tx").with("Ff", ff).with("DA", da_string(spec)).with(
                "Q",
                match spec.align {
                    annot::Align::Left => 0,
                    annot::Align::Center => 1,
                    annot::Align::Right => 2,
                },
            );
            if let Some(m) = spec.max_len {
                d.set("MaxLen", m as i64);
            }
            let r = doc.add(d.into());
            doc.push_annot(page, r)?;
            add_root_field(doc, r);
            let value = spec.value.clone().unwrap_or_default();
            set_field(doc, &spec.name, &FieldValue::Text(value))?;
            r
        }
        FieldKind::Combo | FieldKind::List => {
            if spec.kind == FieldKind::Combo {
                ff |= 1 << 17;
            }
            let opts: Vec<Object> = spec
                .options
                .iter()
                .map(|o| PdfString::from_text(o).into())
                .collect();
            let d = base("Ch")
                .with("Ff", ff)
                .with("DA", da_string(spec))
                .with("Opt", Object::Array(opts));
            let r = doc.add(d.into());
            doc.push_annot(page, r)?;
            add_root_field(doc, r);
            let value = spec.value.clone().unwrap_or_default();
            set_field(doc, &spec.name, &FieldValue::Text(value))?;
            r
        }
        FieldKind::Checkbox => {
            let d = base("Btn")
                .with("Ff", ff)
                .with("V", Object::name("Off"))
                .with("AS", Object::name("Off"));
            let r = doc.add(d.into());
            doc.push_annot(page, r)?;
            add_root_field(doc, r);
            let on = spec
                .value
                .as_deref()
                .map(|v| truthy(v) || v == "Yes")
                .unwrap_or(false);
            set_field(doc, &spec.name, &FieldValue::Bool(on))?;
            r
        }
        FieldKind::Radio => {
            if spec.options.is_empty() {
                return Err(Error::Preset("a radio group needs options".into()));
            }
            ff |= 1 << 15;
            let parent = doc.add(
                Dict::new()
                    .with("FT", "Btn")
                    .with("Ff", ff)
                    .with("T", PdfString::from_text(&spec.name))
                    .with("V", Object::name("Off"))
                    .into(),
            );
            let n = spec.options.len();
            let rects: Vec<Rect> = if spec.widgets.len() == n {
                spec.widgets.clone()
            } else {
                let cell = spec.rect.width() / n as f64;
                let size = cell.min(spec.rect.height());
                (0..n)
                    .map(|i| {
                        let x = spec.rect.x0 + cell * i as f64 + (cell - size) / 2.0;
                        let y = spec.rect.y0 + (spec.rect.height() - size) / 2.0;
                        Rect::from_xywh(x, y, size, size)
                    })
                    .collect()
            };
            let mut kids = Vec::with_capacity(n);
            for (opt, r) in spec.options.iter().zip(rects.iter()) {
                let mut w = Dict::new()
                    .with("Type", "Annot")
                    .with("Subtype", "Widget")
                    .with("Parent", parent)
                    .with("Rect", annot::to_user_rect(&info, r).to_object())
                    .with("F", 4)
                    .with("P", info.obj)
                    .with("AS", Object::name("Off"))
                    .with("MK", mk_dict(spec, rotation))
                    .with(
                        "BS",
                        Dict::new()
                            .with("W", spec.border_width.max(0.0))
                            .with("S", "S"),
                    );
                if let Some(t) = &spec.tooltip {
                    w.set("TU", PdfString::from_text(t));
                }
                let wr = doc.add(w.into());
                // Name the on-state after the option so the export value is meaningful.
                let placeholder = Dict::new().with(
                    "N",
                    Dict::new()
                        .with(opt.as_str(), Object::Null)
                        .with("Off", Object::Null),
                );
                set_key(doc, wr, "AP", placeholder.into());
                doc.push_annot(page, wr)?;
                kids.push(Object::Reference(wr));
            }
            set_key(doc, parent, "Kids", Object::Array(kids));
            add_root_field(doc, parent);
            let value = spec.value.clone().unwrap_or_default();
            set_field(doc, &spec.name, &FieldValue::Text(value))?;
            parent
        }
        FieldKind::Button | FieldKind::Signature | FieldKind::Unknown => {
            return Err(Error::Preset(format!(
                "cannot create a {:?} field",
                spec.kind
            )));
        }
    };
    Ok(field)
}

/// Removes a field (and its widgets) by name. Returns whether it existed.
pub fn remove_field(doc: &mut Document, name: &str) -> Result<bool> {
    let fields = list_fields(doc);
    let f = match fields.iter().find(|f| f.name == name) {
        Some(f) => f.clone(),
        None => return Ok(false),
    };
    let mut gone: HashSet<u32> = f.widgets.iter().map(|w| w.object).collect();
    gone.insert(f.object);
    for w in &f.widgets {
        if let Some(p) = w.page {
            let keep: Vec<ObjRef> = doc
                .page_annots(p)?
                .into_iter()
                .filter(|r| !gone.contains(&r.num))
                .collect();
            doc.set_page_annots(p, &keep)?;
        }
    }
    prune_fields(doc, &gone);
    for n in gone {
        doc.remove_object(ObjRef::new(n, 0));
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// Flatten and prune
// ---------------------------------------------------------------------------

/// Paints every field's appearance into its page and removes the form.
/// Returns the number of widgets flattened.
pub fn flatten_fields(doc: &mut Document) -> Result<usize> {
    let pages: Vec<usize> = (0..doc.page_count()).collect();
    let n = annot::flatten_annotations(
        doc,
        &pages,
        &annot::FlattenOptions {
            subtypes: Some(vec!["Widget".into()]),
            ..Default::default()
        },
    )?;
    doc.catalog_mut().remove("AcroForm");
    Ok(n)
}

/// Drops references to removed widgets from the field tree, and fields
/// that have nothing left. Called after annotations are removed.
pub(crate) fn prune_fields(doc: &mut Document, removed: &HashSet<u32>) {
    let mut af = acroform_owned(doc);
    if af.is_empty() {
        return;
    }
    let roots = refs(doc, af.get("Fields"));
    let mut keep_roots = Vec::new();
    for r in roots {
        if prune_node(doc, r, removed) {
            keep_roots.push(Object::Reference(r));
        }
    }
    if keep_roots.is_empty() {
        doc.catalog_mut().remove("AcroForm");
        return;
    }
    af.set("Fields", Object::Array(keep_roots));
    store_acroform(doc, af);
}

/// Returns whether the node should be kept.
fn prune_node(doc: &mut Document, r: ObjRef, removed: &HashSet<u32>) -> bool {
    if removed.contains(&r.num) {
        return false;
    }
    let kids = refs(doc, doc.get(r).as_dict().and_then(|d| d.get("Kids")));
    if kids.is_empty() {
        return true;
    }
    let mut keep = Vec::new();
    for k in kids {
        if prune_node(doc, k, removed) {
            keep.push(Object::Reference(k));
        }
    }
    if keep.is_empty() {
        let is_widget = doc
            .get(r)
            .as_dict()
            .map(|d| is_widget(doc, d))
            .unwrap_or(false);
        if !is_widget {
            return false;
        }
        remove_key(doc, r, "Kids");
        return true;
    }
    set_key(doc, r, "Kids", Object::Array(keep));
    true
}

/// Names of the root fields in `/AcroForm /Fields`.
pub(crate) fn root_names(doc: &Document) -> HashMap<String, u32> {
    let mut out = HashMap::new();
    if let Some(af) = acroform(doc) {
        for r in refs(doc, af.get("Fields")) {
            if let Some(t) = doc.get(r).as_dict().and_then(|d| text(doc, d, "T")) {
                out.insert(t, r.num);
            }
        }
    }
    out
}

/// Registers `roots` as top-level fields of this document's form (creating
/// the form if needed), renaming on name clashes, and merges default
/// resources from `dr`. Used by [`Document::import_pages`].
pub(crate) fn attach_roots(
    doc: &mut Document,
    roots: &[ObjRef],
    dr: Option<Dict>,
    da: Option<Object>,
    need_appearances: bool,
) {
    if roots.is_empty() {
        return;
    }
    let existing = root_names(doc);
    let mut af = acroform_owned(doc);
    let mut fields: Vec<Object> = af
        .get("Fields")
        .map(|o| doc.resolve(o).clone())
        .and_then(Object::into_array)
        .unwrap_or_default();
    let mut names: HashSet<String> = existing.keys().cloned().collect();
    for &r in roots {
        if fields.iter().any(|o| o.as_reference() == Some(r)) {
            continue;
        }
        if let Some(t) = doc.get(r).as_dict().and_then(|d| text(doc, d, "T")) {
            if names.contains(&t) {
                let mut n = 2;
                let fresh = loop {
                    let cand = format!("{t}-{n}");
                    if !names.contains(&cand) {
                        break cand;
                    }
                    n += 1;
                };
                set_key(doc, r, "T", PdfString::from_text(&fresh).into());
                names.insert(fresh);
            } else {
                names.insert(t);
            }
        }
        fields.push(Object::Reference(r));
    }
    af.set("Fields", Object::Array(fields));
    if let Some(src_dr) = dr {
        let mut dst = af
            .get("DR")
            .map(|o| doc.resolve(o).clone())
            .and_then(Object::into_dict)
            .unwrap_or_default();
        for (cat, val) in src_dr.iter() {
            let src_cat = doc.resolve(val).clone().into_dict().unwrap_or_default();
            let mut dst_cat = dst
                .get(&cat.as_str())
                .map(|o| doc.resolve(o).clone())
                .and_then(Object::into_dict)
                .unwrap_or_default();
            for (k, v) in src_cat.iter() {
                if !dst_cat.contains(&k.as_str()) {
                    dst_cat.0.insert(k.clone(), v.clone());
                }
            }
            dst.0.insert(cat.clone(), dst_cat.into());
        }
        af.set("DR", dst);
    }
    if !af.contains("DA") {
        if let Some(da) = da {
            af.set("DA", da);
        }
    }
    if need_appearances {
        af.set("NeedAppearances", true);
    }
    store_acroform(doc, af);
}

/// Whether the dictionary is a widget annotation (helper for `Document`).
pub(crate) fn dict_is_widget(doc: &Document, d: &Dict) -> bool {
    is_widget(doc, d)
}

/// Removes every trace of the interactive form while keeping widget
/// appearances on the page (they become static annotations).
pub fn remove_form(doc: &mut Document) {
    doc.catalog_mut().remove("AcroForm");
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::annot::normal_appearance;
    use crate::page::PageSize;

    /// A one-page form: text field, check box, radio group with two buttons,
    /// and a drop-down.
    pub(crate) fn sample_form() -> Document {
        let mut d = Document::new();
        let page = d.add_page(PageSize::LETTER);
        let helv = d.add(
            Dict::new()
                .with("Type", "Font")
                .with("Subtype", "Type1")
                .with("BaseFont", "Helvetica")
                .with("Encoding", "WinAnsiEncoding")
                .into(),
        );
        let text = d.add(
            Dict::new()
                .with("Type", "Annot")
                .with("Subtype", "Widget")
                .with("FT", "Tx")
                .with("T", PdfString::from_text("name"))
                .with("Rect", Rect::new(72.0, 700.0, 300.0, 720.0).to_object())
                .with("DA", PdfString::new(b"/Helv 0 Tf 0 0 1 rg".to_vec()))
                .with(
                    "MK",
                    Dict::new()
                        .with(
                            "BG",
                            Object::Array(vec![0.9.into(), 0.9.into(), 1.0.into()]),
                        )
                        .with("BC", Object::Array(vec![0.0.into()])),
                )
                .with("P", page)
                .into(),
        );
        let check = d.add(
            Dict::new()
                .with("Type", "Annot")
                .with("Subtype", "Widget")
                .with("FT", "Btn")
                .with("T", PdfString::from_text("agree"))
                .with("Rect", Rect::new(72.0, 660.0, 90.0, 678.0).to_object())
                .with("V", Object::name("Off"))
                .with("AS", Object::name("Off"))
                .with("P", page)
                .into(),
        );
        let radio = d.add(
            Dict::new()
                .with("FT", "Btn")
                .with("Ff", 1 << 15)
                .with("T", PdfString::from_text("colour"))
                .with("V", Object::name("Off"))
                .into(),
        );
        let mut kids = Vec::new();
        for (i, on) in ["Red", "Blue"].iter().enumerate() {
            let k = d.add(
                Dict::new()
                    .with("Type", "Annot")
                    .with("Subtype", "Widget")
                    .with("Parent", radio)
                    .with(
                        "Rect",
                        Rect::new(72.0 + 30.0 * i as f64, 620.0, 90.0 + 30.0 * i as f64, 638.0)
                            .to_object(),
                    )
                    .with("AS", Object::name("Off"))
                    .with(
                        "AP",
                        Dict::new().with(
                            "N",
                            Dict::new().with(on, Object::Null).with("Off", Object::Null),
                        ),
                    )
                    .with("P", page)
                    .into(),
            );
            kids.push(Object::Reference(k));
        }
        if let Some(rd) = d.get_mut(radio).and_then(Object::as_dict_mut) {
            rd.set("Kids", Object::Array(kids.clone()));
        }
        let combo = d.add(
            Dict::new()
                .with("Type", "Annot")
                .with("Subtype", "Widget")
                .with("FT", "Ch")
                .with("Ff", 1 << 17)
                .with("T", PdfString::from_text("size"))
                .with(
                    "Opt",
                    Object::Array(vec![
                        Object::Array(vec![
                            PdfString::from_text("s").into(),
                            PdfString::from_text("Small").into(),
                        ]),
                        PdfString::from_text("Large").into(),
                    ]),
                )
                .with("Rect", Rect::new(72.0, 580.0, 200.0, 600.0).to_object())
                .with("DA", PdfString::new(b"/Helv 10 Tf 0 g".to_vec()))
                .with("P", page)
                .into(),
        );
        let mut annots = vec![
            Object::Reference(text),
            Object::Reference(check),
            Object::Reference(combo),
        ];
        annots.extend(kids);
        if let Some(pd) = d.get_mut(page).and_then(Object::as_dict_mut) {
            pd.set("Annots", Object::Array(annots));
        }
        let af = Dict::new()
            .with(
                "Fields",
                Object::Array(vec![text.into(), check.into(), radio.into(), combo.into()]),
            )
            .with(
                "DR",
                Dict::new().with("Font", Dict::new().with("Helv", helv)),
            )
            .with("DA", PdfString::new(b"/Helv 0 Tf 0 g".to_vec()))
            .with("NeedAppearances", true);
        d.catalog_mut().set("AcroForm", af);
        d
    }

    #[test]
    fn lists_fields() {
        let d = sample_form();
        let fields = list_fields(&d);
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["name", "agree", "colour", "size"]);
        assert_eq!(fields[0].kind, FieldKind::Text);
        assert_eq!(fields[1].kind, FieldKind::Checkbox);
        assert_eq!(fields[2].kind, FieldKind::Radio);
        assert_eq!(
            fields[2]
                .options
                .iter()
                .map(|o| o.value.as_str())
                .collect::<Vec<_>>(),
            ["Red", "Blue"]
        );
        assert_eq!(fields[2].widgets.len(), 2);
        assert_eq!(fields[3].kind, FieldKind::Combo);
        assert_eq!(fields[3].options[0].label, "Small");
        assert_eq!(fields[0].page, Some(0));
        assert!((fields[0].rect.unwrap().y1 - 720.0).abs() < 1e-6);
    }

    #[test]
    fn fills_and_flattens() {
        let mut d = sample_form();
        set_field(&mut d, "name", &FieldValue::Text("Ada Lovelace".into())).unwrap();
        set_field(&mut d, "agree", &FieldValue::Bool(true)).unwrap();
        set_field(&mut d, "colour", &FieldValue::Text("Blue".into())).unwrap();
        set_field(&mut d, "size", &FieldValue::Text("s".into())).unwrap();
        let fields = list_fields(&d);
        assert_eq!(fields[0].value.as_deref(), Some("Ada Lovelace"));
        assert_eq!(fields[1].value.as_deref(), Some("Yes"));
        assert_eq!(fields[2].value.as_deref(), Some("Blue"));
        assert_eq!(fields[3].value.as_deref(), Some("s"));
        // Appearance streams were generated.
        let w = ObjRef::new(fields[0].widgets[0].object, 0);
        let ap = normal_appearance(&d, d.get(w).as_dict().unwrap()).unwrap();
        let content =
            String::from_utf8_lossy(&d.stream_data(d.get(ap).as_stream().unwrap()).unwrap())
                .into_owned();
        assert!(content.contains("(Ada Lovelace) Tj"), "{content}");
        assert!(content.contains("0 0 1 rg"), "colour from DA: {content}");
        assert!(content.starts_with("0.9 0.9 1 rg"), "background: {content}");
        let blue = ObjRef::new(fields[2].widgets[1].object, 0);
        assert_eq!(
            d.get(blue)
                .as_dict()
                .unwrap()
                .get("AS")
                .and_then(Object::as_name)
                .unwrap(),
            "Blue"
        );
        let red = ObjRef::new(fields[2].widgets[0].object, 0);
        assert_eq!(
            d.get(red)
                .as_dict()
                .unwrap()
                .get("AS")
                .and_then(Object::as_name)
                .unwrap(),
            "Off"
        );
        assert!(!acroform(&d).unwrap().contains("NeedAppearances"));
        // Round trip.
        let bytes = d.save(&Default::default()).unwrap();
        let mut d2 = Document::load(&bytes).unwrap();
        assert_eq!(list_fields(&d2)[0].value.as_deref(), Some("Ada Lovelace"));
        let n = flatten_fields(&mut d2).unwrap();
        assert_eq!(n, 5);
        assert!(list_fields(&d2).is_empty());
        assert!(d2.page_annots(0).unwrap().is_empty());
        assert!(d2.catalog().get("AcroForm").is_none());
        let content = String::from_utf8_lossy(&d2.page_content(0).unwrap()).into_owned();
        assert!(content.contains("Do"), "{content}");
    }

    #[test]
    fn unknown_field_and_bad_choice() {
        let mut d = sample_form();
        assert!(set_field(&mut d, "nope", &FieldValue::Text("x".into())).is_err());
        assert!(set_field(&mut d, "colour", &FieldValue::Text("Green".into())).is_err());
        let missing = set_fields(
            &mut d,
            &[
                ("name".into(), FieldValue::Text("a".into())),
                ("zzz".into(), FieldValue::Bool(true)),
            ],
        )
        .unwrap();
        assert_eq!(missing, vec!["zzz".to_string()]);
    }

    #[test]
    fn multiline_and_maxlen() {
        let mut d = sample_form();
        let fields = list_fields(&d);
        let fr = ObjRef::new(fields[0].object, 0);
        set_key(&mut d, fr, "Ff", Object::Integer(1 << 12));
        set_key(&mut d, fr, "MaxLen", Object::Integer(10));
        set_field(
            &mut d,
            "name",
            &FieldValue::Text("one two three four five six".into()),
        )
        .unwrap();
        assert_eq!(list_fields(&d)[0].value.as_deref(), Some("one two th"));
    }

    #[test]
    fn creates_fields() {
        let mut d = Document::new();
        d.add_page(PageSize::LETTER);
        add_field(
            &mut d,
            0,
            &NewField {
                name: "first".into(),
                rect: Rect::new(72.0, 700.0, 300.0, 724.0),
                value: Some("Ada".into()),
                background: Some([0.95, 0.95, 1.0]),
                border: Some([0.0, 0.0, 0.0]),
                ..Default::default()
            },
        )
        .unwrap();
        add_field(
            &mut d,
            0,
            &NewField {
                name: "ok".into(),
                kind: FieldKind::Checkbox,
                rect: Rect::new(72.0, 660.0, 90.0, 678.0),
                value: Some("true".into()),
                ..Default::default()
            },
        )
        .unwrap();
        add_field(
            &mut d,
            0,
            &NewField {
                name: "colour".into(),
                kind: FieldKind::Radio,
                rect: Rect::new(72.0, 620.0, 200.0, 640.0),
                options: vec!["Red".into(), "Green".into(), "Blue".into()],
                value: Some("Green".into()),
                ..Default::default()
            },
        )
        .unwrap();
        add_field(
            &mut d,
            0,
            &NewField {
                name: "size".into(),
                kind: FieldKind::Combo,
                rect: Rect::new(72.0, 580.0, 200.0, 600.0),
                options: vec!["S".into(), "M".into()],
                value: Some("M".into()),
                font_size: 10.0,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(
            add_field(
                &mut d,
                0,
                &NewField {
                    name: "first".into(),
                    rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                    ..Default::default()
                }
            )
            .is_err(),
            "duplicate name"
        );
        let bytes = d.save(&Default::default()).unwrap();
        let mut d2 = Document::load(&bytes).unwrap();
        let f = list_fields(&d2);
        assert_eq!(
            f.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(),
            ["first", "ok", "colour", "size"]
        );
        assert_eq!(f[0].value.as_deref(), Some("Ada"));
        assert_eq!(f[1].value.as_deref(), Some("Yes"));
        assert_eq!(f[2].value.as_deref(), Some("Green"));
        assert_eq!(f[2].widgets.len(), 3);
        assert_eq!(f[2].widgets[1].on_state.as_deref(), Some("Green"));
        assert_eq!(f[3].value.as_deref(), Some("M"));
        assert!(remove_field(&mut d2, "colour").unwrap());
        assert_eq!(list_fields(&d2).len(), 3);
        assert_eq!(d2.page_annots(0).unwrap().len(), 3);
        assert!(!remove_field(&mut d2, "colour").unwrap());
    }

    #[test]
    fn da_parsing() {
        let da = parse_da(b"/HeBo 9.5 Tf 0.2 0.4 0.6 rg");
        assert_eq!(da.font.as_deref(), Some("HeBo"));
        assert_eq!(da.size, 9.5);
        assert_eq!(String::from_utf8(da.color).unwrap(), "0.2 0.4 0.6 rg");
        let da = parse_da(b"0 g /Helv 0 Tf");
        assert_eq!(da.size, 0.0);
        assert_eq!(String::from_utf8(da.color).unwrap(), "0 g");
    }

    #[test]
    fn merge_carries_forms_and_renames() {
        let a = sample_form();
        let b = sample_form();
        let merged = crate::ops::merge(&[&a, &b]).unwrap();
        let fields = list_fields(&merged);
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            ["name", "agree", "colour", "size", "name-2", "agree-2", "colour-2", "size-2"]
        );
        assert_eq!(fields[6].widgets.len(), 2);
        assert_eq!(fields[6].page, Some(1));
        let bytes = merged.clone().save(&Default::default()).unwrap();
        let re = Document::load(&bytes).unwrap();
        assert_eq!(list_fields(&re).len(), 8);
    }

    #[test]
    fn extract_prunes_widgets_on_other_pages() {
        let mut a = sample_form();
        a.add_page(PageSize::LETTER);
        // Move the "Blue" radio button to page 2.
        let fields = list_fields(&a);
        let blue = ObjRef::new(fields[2].widgets[1].object, 0);
        let mut p0 = a.page_annots(0).unwrap();
        p0.retain(|r| *r != blue);
        a.set_page_annots(0, &p0).unwrap();
        a.push_annot(1, blue).unwrap();
        let only_first = crate::ops::extract(&a, &[0]).unwrap();
        let f = list_fields(&only_first);
        let colour = f.iter().find(|f| f.name == "colour").unwrap();
        assert_eq!(colour.widgets.len(), 1, "stray widget pruned");
        assert_eq!(colour.widgets[0].on_state.as_deref(), Some("Red"));
    }
}
