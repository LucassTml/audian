//! Renders a contact sheet of the artwork at several sizes on dark and light backgrounds.
//! usage: cargo run -p audian-art --example preview -- out.png

use tiny_skia::*;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "preview.png".into());
    let sizes = [256u32, 128, 64, 48, 32, 24, 16];
    let (w, h) = (900u32, 640u32);
    let mut sheet = Pixmap::new(w, h).unwrap();
    sheet.fill(Color::from_rgba8(32, 32, 36, 255));
    let mut light = Paint::default();
    light.set_color(Color::from_rgba8(243, 243, 243, 255));
    sheet.fill_rect(Rect::from_xywh(0.0, 320.0, w as f32, 320.0).unwrap(), &light, Transform::identity(), None);
    for (row, y0) in [(0, 30.0f32), (1, 350.0)] {
        let mut x = 20.0;
        for &s in &sizes {
            let logo = audian_art::draw_logo(s);
            sheet.draw_pixmap(x as i32, y0 as i32, logo.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            x += s as f32 + 20.0;
        }
        // Tray glyphs (white on dark row, near-black on light row, plus state colours).
        let glyph_colors = if row == 0 {
            [Color::from_rgba8(245, 245, 247, 255), Color::from_rgba8(255, 69, 58, 255), Color::from_rgba8(90, 160, 255, 255)]
        } else {
            [Color::from_rgba8(30, 30, 34, 255), Color::from_rgba8(230, 50, 40, 255), Color::from_rgba8(40, 110, 230, 255)]
        };
        let mut gx = 620.0;
        for c in glyph_colors {
            for (i, &s) in [32u32, 20, 16].iter().enumerate() {
                let g = audian_art::draw_glyph(s, c);
                sheet.draw_pixmap(gx as i32, (y0 + 200.0 + i as f32 * 36.0) as i32, g.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
            }
            gx += 50.0;
        }
    }
    sheet.save_png(&out).unwrap();
    println!("wrote {out}");
}
