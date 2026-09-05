//! Builds sample PDFs exercising forms and annotations, for visual checks.
//! `cargo run --release -p foliopdf --example makeform -- OUTDIR`
use foliopdf::annot::{self, Align, Annotation, AnnotationMeta, NoteIcon};
use foliopdf::forms::{self, FieldKind, FieldValue, NewField};
use foliopdf::geometry::{Point, Rect};
use foliopdf::ops::{self, TextStamp};
use foliopdf::{Document, PageSize};

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    // --- form ---
    let mut d = Document::new();
    d.add_page(PageSize::LETTER);
    d.add_page(PageSize::LETTER);
    d.rotate_page(1, 90).unwrap();
    let label = |d: &mut Document, page: usize, text: &str, x: f64, y: f64| {
        ops::stamp_text(
            d,
            &[page],
            &TextStamp {
                text: text.into(),
                size: 10.0,
                color: [0.2, 0.2, 0.2],
                opacity: 1.0,
                position: ops::Position::BottomLeft,
                margin: 0.0,
                ..Default::default()
            },
        )
        .unwrap();
        let _ = (x, y);
    };
    let _ = label;
    let f = |name: &str, kind: FieldKind, rect: Rect| NewField {
        name: name.into(),
        kind,
        rect,
        border: Some([0.4, 0.4, 0.4]),
        background: Some([0.96, 0.97, 1.0]),
        ..Default::default()
    };
    forms::add_field(
        &mut d,
        0,
        &NewField {
            value: Some("Ada Lovelace".into()),
            ..f(
                "name",
                FieldKind::Text,
                Rect::new(72.0, 690.0, 320.0, 714.0),
            )
        },
    )
    .unwrap();
    forms::add_field(
        &mut d,
        0,
        &NewField {
            font_size: 9.0,
            align: Align::Right,
            value: Some("right aligned 9pt".into()),
            ..f(
                "right",
                FieldKind::Text,
                Rect::new(340.0, 690.0, 540.0, 714.0),
            )
        },
    )
    .unwrap();
    forms::add_field(&mut d, 0, &NewField { multiline: true, value: Some("A multi-line field. The quick brown fox jumps over the lazy dog, then keeps going until the text wraps onto several lines inside the box.".into()), ..f("notes", FieldKind::Text, Rect::new(72.0, 600.0, 320.0, 670.0)) }).unwrap();
    forms::add_field(
        &mut d,
        0,
        &NewField {
            comb: true,
            max_len: Some(9),
            value: Some("123456789".into()),
            ..f(
                "ssn",
                FieldKind::Text,
                Rect::new(340.0, 640.0, 540.0, 670.0),
            )
        },
    )
    .unwrap();
    forms::add_field(
        &mut d,
        0,
        &NewField {
            password: true,
            value: Some("secret".into()),
            ..f("pw", FieldKind::Text, Rect::new(340.0, 600.0, 540.0, 624.0))
        },
    )
    .unwrap();
    forms::add_field(
        &mut d,
        0,
        &NewField {
            value: Some("true".into()),
            ..f(
                "agree",
                FieldKind::Checkbox,
                Rect::new(72.0, 560.0, 90.0, 578.0),
            )
        },
    )
    .unwrap();
    forms::add_field(
        &mut d,
        0,
        &f(
            "no",
            FieldKind::Checkbox,
            Rect::new(100.0, 560.0, 118.0, 578.0),
        ),
    )
    .unwrap();
    forms::add_field(
        &mut d,
        0,
        &NewField {
            options: vec!["Red".into(), "Green".into(), "Blue".into()],
            value: Some("Green".into()),
            ..f(
                "colour",
                FieldKind::Radio,
                Rect::new(140.0, 556.0, 260.0, 582.0),
            )
        },
    )
    .unwrap();
    forms::add_field(
        &mut d,
        0,
        &NewField {
            options: vec!["Small".into(), "Medium".into(), "Large".into()],
            value: Some("Medium".into()),
            font_size: 11.0,
            ..f(
                "size",
                FieldKind::Combo,
                Rect::new(340.0, 556.0, 540.0, 580.0),
            )
        },
    )
    .unwrap();
    forms::add_field(
        &mut d,
        0,
        &NewField {
            options: vec![
                "Alpha".into(),
                "Beta".into(),
                "Gamma".into(),
                "Delta".into(),
            ],
            value: Some("Beta".into()),
            font_size: 10.0,
            ..f(
                "list",
                FieldKind::List,
                Rect::new(340.0, 480.0, 540.0, 540.0),
            )
        },
    )
    .unwrap();
    forms::add_field(
        &mut d,
        0,
        &NewField {
            value: Some("auto-sized tall field".into()),
            ..f(
                "tall",
                FieldKind::Text,
                Rect::new(72.0, 480.0, 320.0, 540.0),
            )
        },
    )
    .unwrap();
    // Rotated page: field should read upright.
    forms::add_field(
        &mut d,
        1,
        &NewField {
            value: Some("On a rotated page".into()),
            ..f("rot", FieldKind::Text, Rect::new(72.0, 500.0, 400.0, 530.0))
        },
    )
    .unwrap();
    forms::add_field(
        &mut d,
        1,
        &NewField {
            value: Some("yes".into()),
            ..f(
                "rotcheck",
                FieldKind::Checkbox,
                Rect::new(72.0, 450.0, 96.0, 474.0),
            )
        },
    )
    .unwrap();
    std::fs::write(
        format!("{out}/gen-form.pdf"),
        d.save(&Default::default()).unwrap(),
    )
    .unwrap();
    forms::set_field(&mut d, "name", &FieldValue::Text("Changed later".into())).unwrap();
    forms::set_field(&mut d, "colour", &FieldValue::Text("Blue".into())).unwrap();
    forms::set_field(&mut d, "no", &FieldValue::Bool(true)).unwrap();
    std::fs::write(
        format!("{out}/gen-form-filled.pdf"),
        d.save(&Default::default()).unwrap(),
    )
    .unwrap();
    forms::flatten_fields(&mut d).unwrap();
    std::fs::write(
        format!("{out}/gen-form-flat.pdf"),
        d.save(&Default::default()).unwrap(),
    )
    .unwrap();

    // --- annotations ---
    let mut d = Document::new();
    d.add_page(PageSize::LETTER);
    d.add_page(PageSize::LETTER);
    d.rotate_page(1, 90).unwrap();
    for p in 0..2 {
        ops::stamp_text(
            &mut d,
            &[p],
            &TextStamp {
                text: "The quick brown fox jumps over the lazy dog".into(),
                size: 14.0,
                color: [0.0, 0.0, 0.0],
                opacity: 1.0,
                position: ops::Position::TopLeft,
                margin: 72.0,
                ..Default::default()
            },
        )
        .unwrap();
        let m = AnnotationMeta {
            author: Some("Ada".into()),
            contents: Some("A comment".into()),
            ..Default::default()
        };
        let (w, h) = {
            let i = d.page_info(p).unwrap();
            (i.display_width(), i.display_height())
        };
        let line = Rect::new(72.0, h - 72.0 - 14.0, 72.0 + 300.0, h - 72.0 + 4.0);
        annot::add_annotation(
            &mut d,
            p,
            &Annotation::Highlight {
                quads: vec![Rect::new(line.x0, line.y0, line.x0 + 120.0, line.y1)],
                color: [1.0, 0.92, 0.23],
                opacity: 1.0,
            },
            &m,
        )
        .unwrap();
        annot::add_annotation(
            &mut d,
            p,
            &Annotation::Underline {
                quads: vec![Rect::new(
                    line.x0 + 130.0,
                    line.y0,
                    line.x0 + 200.0,
                    line.y1,
                )],
                color: [0.0, 0.5, 0.0],
                opacity: 1.0,
            },
            &m,
        )
        .unwrap();
        annot::add_annotation(
            &mut d,
            p,
            &Annotation::StrikeOut {
                quads: vec![Rect::new(
                    line.x0 + 210.0,
                    line.y0,
                    line.x0 + 300.0,
                    line.y1,
                )],
                color: [0.9, 0.0, 0.0],
                opacity: 1.0,
            },
            &m,
        )
        .unwrap();
        annot::add_annotation(
            &mut d,
            p,
            &Annotation::Square {
                rect: Rect::new(72.0, h - 250.0, 200.0, h - 150.0),
                stroke: Some([0.0, 0.0, 0.8]),
                fill: Some([0.85, 0.9, 1.0]),
                width: 3.0,
                opacity: 0.8,
            },
            &m,
        )
        .unwrap();
        annot::add_annotation(
            &mut d,
            p,
            &Annotation::Circle {
                rect: Rect::new(220.0, h - 250.0, 320.0, h - 150.0),
                stroke: Some([0.8, 0.0, 0.0]),
                fill: None,
                width: 2.0,
                opacity: 1.0,
            },
            &m,
        )
        .unwrap();
        annot::add_annotation(
            &mut d,
            p,
            &Annotation::Line {
                from: Point::new(340.0, h - 250.0),
                to: Point::new(w - 72.0, h - 150.0),
                color: [0.0, 0.6, 0.0],
                width: 2.0,
                opacity: 1.0,
            },
            &m,
        )
        .unwrap();
        let mut path = Vec::new();
        for i in 0..60 {
            let t = i as f64 / 59.0;
            path.push(Point::new(
                72.0 + t * 250.0,
                h - 320.0 + (t * 12.0).sin() * 20.0,
            ));
        }
        annot::add_annotation(
            &mut d,
            p,
            &Annotation::Ink {
                paths: vec![path],
                color: [0.5, 0.0, 0.5],
                width: 3.0,
                opacity: 1.0,
            },
            &m,
        )
        .unwrap();
        annot::add_annotation(&mut d, p, &Annotation::FreeText { rect: Rect::new(340.0, h - 360.0, w - 72.0, h - 280.0), text: "Free text box, centred, with a background and a border. It wraps when the line is too long.".into(), font: "Times-Roman".into(), size: 11.0, color: [0.1, 0.1, 0.4], align: Align::Center, background: Some([1.0, 1.0, 0.85]), border: Some([0.6, 0.5, 0.0]), opacity: 1.0 }, &m).unwrap();
        for (i, icon) in [
            NoteIcon::Comment,
            NoteIcon::Key,
            NoteIcon::Help,
            NoteIcon::Paragraph,
            NoteIcon::Insert,
        ]
        .iter()
        .enumerate()
        {
            annot::add_annotation(
                &mut d,
                p,
                &Annotation::Note {
                    at: Point::new(72.0 + 30.0 * i as f64, h - 380.0),
                    icon: *icon,
                    color: [1.0, 0.85, 0.2],
                },
                &m,
            )
            .unwrap();
        }
        annot::add_annotation(
            &mut d,
            p,
            &Annotation::Link {
                rect: Rect::new(72.0, h - 72.0 - 14.0, 372.0, h - 72.0 + 4.0),
                uri: Some("https://example.com".into()),
                page: None,
            },
            &Default::default(),
        )
        .unwrap();
    }
    std::fs::write(
        format!("{out}/gen-annots.pdf"),
        d.save(&Default::default()).unwrap(),
    )
    .unwrap();
    annot::flatten_annotations(&mut d, &[0, 1], &Default::default()).unwrap();
    std::fs::write(
        format!("{out}/gen-annots-flat.pdf"),
        d.save(&Default::default()).unwrap(),
    )
    .unwrap();
    println!("ok");
}
