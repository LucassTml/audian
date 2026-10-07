//! Audian's artwork, drawn as vectors so every size (16 px tray icon to 256 px installer
//! art) is rendered crisply from one source. Used by build scripts (to bake the .ico) and at
//! runtime (tray, settings window, installer).

use tiny_skia::*;

type Rgb = (u8, u8, u8);

/// Colours of the application icon. The default is the brand's off-white ("Ivory") look;
/// the Audian window draws its logo in the user's theme.
#[derive(Clone, Copy, Debug)]
pub struct LogoColors {
    pub bg_top: Rgb,
    pub bg_bottom: Rgb,
    pub glow: Rgb,
    pub mark: [Rgb; 3],
}

impl LogoColors {
    pub const BRAND: LogoColors = LogoColors {
        bg_top: (42, 42, 46),
        bg_bottom: (14, 14, 16),
        glow: (255, 244, 225),
        mark: [(252, 250, 245), (236, 230, 218), (204, 196, 182)],
    };
}

fn rgba((r, g, b): Rgb, a: u8) -> Color {
    Color::from_rgba8(r, g, b, a)
}

/// Superellipse ("squircle") outline, the continuous-curvature shape modern app icons use.
pub fn squircle(x: f32, y: f32, size: f32, exponent: f32) -> Option<Path> {
    let (cx, cy, r) = (x + size / 2.0, y + size / 2.0, size / 2.0);
    let mut pb = PathBuilder::new();
    let steps = 256;
    for i in 0..=steps {
        let t = i as f32 / steps as f32 * std::f32::consts::TAU;
        let (s, c) = t.sin_cos();
        let px = cx + r * c.signum() * c.abs().powf(2.0 / exponent);
        let py = cy + r * s.signum() * s.abs().powf(2.0 / exponent);
        if i == 0 { pb.move_to(px, py) } else { pb.line_to(px, py) }
    }
    pb.close();
    pb.finish()
}

pub fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = 0.552_284_8 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish()
}

fn linear(p0: (f32, f32), p1: (f32, f32), stops: &[(f32, Color)]) -> Shader<'static> {
    LinearGradient::new(
        Point::from_xy(p0.0, p0.1),
        Point::from_xy(p1.0, p1.1),
        stops.iter().map(|&(t, c)| GradientStop::new(t, c)).collect(),
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap_or(Shader::SolidColor(stops[0].1))
}

fn radial(center: (f32, f32), radius: f32, stops: &[(f32, Color)]) -> Shader<'static> {
    RadialGradient::new(
        Point::from_xy(center.0, center.1),
        0.0,
        Point::from_xy(center.0, center.1),
        radius,
        stops.iter().map(|&(t, c)| GradientStop::new(t, c)).collect(),
        SpreadMode::Pad,
        Transform::identity(),
    )
    .unwrap_or(Shader::SolidColor(stops[0].1))
}

/// The Audian mark: an "A" whose crossbar is a sound wave, in unit coordinates scaled to
/// `size` and offset by (ox, oy). Returns the stroke path and recommended stroke width.
pub fn mark_path(ox: f32, oy: f32, size: f32) -> (Option<Path>, f32) {
    let p = |x: f32, y: f32| (ox + x * size, oy + y * size);
    let mut pb = PathBuilder::new();
    // Legs of the A (one continuous stroke through the apex for a smooth round join).
    let (lx, ly) = p(0.215, 0.835);
    let (ax, ay) = p(0.5, 0.165);
    let (rx, ry) = p(0.785, 0.835);
    pb.move_to(lx, ly);
    pb.line_to(ax, ay);
    pb.line_to(rx, ry);
    // Wave crossbar: one full sine period spanning the legs.
    let y0 = 0.565;
    let left = 0.5 - (0.5 - 0.215) * (y0 - 0.165) / (0.835 - 0.165);
    let right = 1.0 - left;
    let w = right - left;
    let amp = 0.07;
    let (sx, sy) = p(left, y0);
    pb.move_to(sx, sy);
    let q = w / 4.0;
    let (c1x, c1y) = p(left + q * 0.55, y0 - amp * 1.33);
    let (c2x, c2y) = p(left + q * 1.45, y0 - amp * 1.33);
    let (mx, my) = p(left + 2.0 * q, y0);
    pb.cubic_to(c1x, c1y, c2x, c2y, mx, my);
    let (c3x, c3y) = p(left + q * 2.55, y0 + amp * 1.33);
    let (c4x, c4y) = p(left + q * 3.45, y0 + amp * 1.33);
    let (ex, ey) = p(right, y0);
    pb.cubic_to(c3x, c3y, c4x, c4y, ex, ey);
    (pb.finish(), size * 0.105)
}

/// Full-colour application icon in the brand colours.
pub fn draw_logo(size: u32) -> Pixmap {
    draw_logo_with(size, &LogoColors::BRAND)
}

/// Full-colour application icon.
pub fn draw_logo_with(size: u32, colors: &LogoColors) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("logo size");
    let s = size as f32;
    let small = size <= 24;
    let inset = if small { 0.0 } else { s * 0.02 };
    let bg = squircle(inset, inset, s - 2.0 * inset, 5.0);
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    if let Some(bg) = &bg {
        paint.shader = linear((0.0, 0.0), (s, s), &[(0.0, rgba(colors.bg_top, 255)), (1.0, rgba(colors.bg_bottom, 255))]);
        pm.fill_path(bg, &paint, FillRule::Winding, Transform::identity(), None);
        // Soft glow behind the mark.
        paint.shader = radial((s * 0.5, s * 0.52), s * 0.55, &[(0.0, rgba(colors.glow, 70)), (1.0, rgba(colors.glow, 0))]);
        pm.fill_path(bg, &paint, FillRule::Winding, Transform::identity(), None);
        // Hairline highlight along the top edge for depth.
        if !small {
            paint.shader = linear(
                (0.0, 0.0),
                (0.0, s),
                &[(0.0, Color::from_rgba8(255, 255, 255, 60)), (0.35, Color::from_rgba8(255, 255, 255, 0))],
            );
            let stroke = Stroke { width: (s * 0.008).max(1.0), ..Stroke::default() };
            pm.stroke_path(bg, &paint, &stroke, Transform::identity(), None);
        }
    }
    let mark_size = s * if small { 0.86 } else { 0.66 };
    let off = (s - mark_size) / 2.0;
    let (path, width) = mark_path(off, off + s * 0.01, mark_size);
    if let Some(path) = path {
        let width = if small { width * 1.3 } else { width };
        // Glow: a few wide, faint strokes under the mark.
        if !small {
            for i in 0..12 {
                let k = 4.2 - i as f32 * 0.26;
                let mut glow = Paint { anti_alias: true, ..Paint::default() };
                glow.set_color(rgba(colors.glow, 6));
                let stroke = Stroke { width: width * k, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
                pm.stroke_path(&path, &glow, &stroke, Transform::identity(), None);
            }
        }
        // Small sizes use only the light end of the gradient, for contrast.
        let [m0, m1, m2] = colors.mark;
        let stops: Vec<(f32, Color)> = if small {
            vec![(0.0, rgba(m0, 255)), (1.0, rgba(m1, 255))]
        } else {
            vec![(0.0, rgba(m0, 255)), (0.55, rgba(m1, 255)), (1.0, rgba(m2, 255))]
        };
        paint.shader = linear((off, off), (off + mark_size, off + mark_size), &stops);
        let stroke = Stroke { width, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
    pm
}

/// Monochrome mark for the tray / small UI, in `color`.
pub fn draw_glyph(size: u32, color: Color) -> Pixmap {
    let mut pm = Pixmap::new(size, size).expect("glyph size");
    let s = size as f32;
    let (path, width) = mark_path(s * 0.02, s * 0.03, s * 0.96);
    if let Some(path) = path {
        let mut paint = Paint { anti_alias: true, ..Paint::default() };
        paint.set_color(color);
        let stroke = Stroke { width: width * 1.3, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
        pm.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
    pm
}

/// Encodes a multi-resolution .ico (PNG-compressed entries) of the logo.
pub fn logo_ico() -> Vec<u8> {
    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 96, 128, 256];
    let pngs: Vec<Vec<u8>> = sizes.iter().map(|&s| draw_logo(s).encode_png().expect("png")).collect();
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&s, png) in sizes.iter().zip(&pngs) {
        let dim = if s >= 256 { 0 } else { s as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes());
        ico.extend_from_slice(&32u16.to_le_bytes());
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in &pngs {
        ico.extend_from_slice(png);
    }
    ico
}

/// Straight (non-premultiplied) RGBA bytes, e.g. for window icons and GUI textures.
pub fn to_rgba(pm: &Pixmap) -> Vec<u8> {
    pm.pixels()
        .iter()
        .flat_map(|p| {
            let c = p.demultiply();
            [c.red(), c.green(), c.blue(), c.alpha()]
        })
        .collect()
}
