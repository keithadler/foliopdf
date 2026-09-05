//! Writes a test JPEG (gradient with a white frame). `-- out.jpg [W H]`
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (w, h): (u16, u16) = (
        a.get(1).and_then(|x| x.parse().ok()).unwrap_or(640),
        a.get(2).and_then(|x| x.parse().ok()).unwrap_or(480),
    );
    let mut px = Vec::with_capacity(w as usize * h as usize * 3);
    for y in 0..h {
        for x in 0..w {
            let edge = x < 16 || y < 16 || x >= w - 16 || y >= h - 16;
            if edge {
                px.extend_from_slice(&[255, 255, 255]);
            } else {
                px.extend_from_slice(&[
                    (x as u32 * 255 / w as u32) as u8,
                    (y as u32 * 255 / h as u32) as u8,
                    128,
                ]);
            }
        }
    }
    let mut out = Vec::new();
    jpeg_encoder::Encoder::new(&mut out, 85)
        .encode(&px, w, h, jpeg_encoder::ColorType::Rgb)
        .unwrap();
    std::fs::write(&a[0], out).unwrap();
}
