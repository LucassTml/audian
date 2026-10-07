//! Writes the logo as PNG: `export_logo <out.png> [size]`.
fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "logo.png".into());
    let size = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(512);
    audian_art::draw_logo(size).save_png(&out).unwrap();
}
