//! The floating recording indicator: a small pill with a live waveform while listening, the
//! current stage while processing, and a brief check / error mark at the end.
//!
//! Implementation: a layered, click-through, never-activated topmost window. Frames are drawn
//! into a small pixmap with `tiny-skia` (text via `fontdue` with the system's Segoe UI) and
//! pushed with `UpdateLayeredWindow` (per-pixel alpha, soft shadow, no GPU context). The
//! ~30 fps animation timer runs only while the pill is visible.
//!
//! Colours follow the user's theme and indicator style (dark glass or light). The same drawing
//! code renders the static previews on the Appearance page ([`preview`]).

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use audian_art::rounded_rect;
use audian_common::config::OverlayPosition;
use audian_common::theme::{IndicatorStyle, Rgb, ThemeId};
use tiny_skia::{Color, FillRule, LineCap, LineJoin, Paint, PathBuilder, Pixmap, Stroke, Transform};
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

use crate::audio::LevelMeter;
use crate::inject::Target;

const PILL_W_LABEL: f32 = 236.0;
const PILL_W_PLAIN: f32 = 168.0;
const PILL_H: f32 = 46.0;
/// Transparent margin around the pill for the drop shadow.
const MARGIN: f32 = 14.0;
const BARS: usize = 22;
const FRAME: Duration = Duration::from_millis(33);
const APPEAR: f32 = 0.16;
const FADE: f32 = 0.18;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Hidden,
    Recording,
    Processing,
    Done,
    Error,
    NoSpeech,
}

impl Phase {
    /// How long transient states stay before fading out.
    fn hold(self) -> Option<f32> {
        match self {
            Phase::Done => Some(0.75),
            Phase::NoSpeech => Some(1.0),
            Phase::Error => Some(1.9),
            _ => None,
        }
    }
}

/// Minimal glyph cache + compositor over fontdue.
///
/// fontdue parses every glyph of a font up front (~20 MB for Segoe UI), so the font is only
/// loaded while the overlay is visible and released when it hides; the few rendered glyphs
/// stay cached (a few KB).
struct TextRenderer {
    font: Option<fontdue::Font>,
    cache: HashMap<(char, u32), (fontdue::Metrics, Vec<u8>)>,
}

fn load_font() -> Option<fontdue::Font> {
    let fonts = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into()).join("Fonts");
    let bytes = ["seguisb.ttf", "segoeui.ttf"].iter().find_map(|f| std::fs::read(fonts.join(f)).ok())?;
    fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
}

impl TextRenderer {
    fn load() -> Option<TextRenderer> {
        let fonts = std::env::var_os("SystemRoot").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into()).join("Fonts");
        let available = ["seguisb.ttf", "segoeui.ttf"].iter().any(|f| fonts.join(f).is_file());
        available.then(|| TextRenderer { font: None, cache: HashMap::new() })
    }

    fn release_font(&mut self) {
        self.font = None;
    }

    fn glyph(&mut self, c: char, px: f32) -> &(fontdue::Metrics, Vec<u8>) {
        let key = (c, (px * 10.0) as u32);
        if !self.cache.contains_key(&key) {
            if self.font.is_none() {
                self.font = load_font();
            }
            let g = match &self.font {
                Some(f) => f.rasterize(c, px),
                None => (fontdue::Metrics::default(), Vec::new()),
            };
            self.cache.insert(key, g);
        }
        &self.cache[&key]
    }

    fn width(&mut self, text: &str, px: f32) -> f32 {
        text.chars().map(|c| self.glyph(c, px).0.advance_width).sum()
    }

    /// Draws `text` with its baseline at `y`, compositing coverage into premultiplied RGBA.
    fn draw(&mut self, pm: &mut Pixmap, text: &str, mut x: f32, y: f32, px: f32, rgb: (u8, u8, u8), alpha: f32) {
        let (w, h) = (pm.width() as i32, pm.height() as i32);
        for c in text.chars() {
            let (m, cov) = self.glyph(c, px).clone();
            let gx = (x + m.xmin as f32).round() as i32;
            let gy = (y - m.height as f32 - m.ymin as f32).round() as i32;
            let data = pm.data_mut();
            for row in 0..m.height as i32 {
                for col in 0..m.width as i32 {
                    let (px_x, px_y) = (gx + col, gy + row);
                    if px_x < 0 || px_y < 0 || px_x >= w || px_y >= h {
                        continue;
                    }
                    let a = cov[(row * m.width as i32 + col) as usize] as f32 / 255.0 * alpha;
                    if a <= 0.0 {
                        continue;
                    }
                    let i = ((px_y * w + px_x) * 4) as usize;
                    let inv = 1.0 - a;
                    data[i] = (rgb.0 as f32 * a + data[i] as f32 * inv) as u8;
                    data[i + 1] = (rgb.1 as f32 * a + data[i + 1] as f32 * inv) as u8;
                    data[i + 2] = (rgb.2 as f32 * a + data[i + 2] as f32 * inv) as u8;
                    data[i + 3] = (255.0 * a + data[i + 3] as f32 * inv) as u8;
                }
            }
            x += m.advance_width;
        }
    }
}

pub struct Overlay {
    hwnd: HWND,
    phase: Phase,
    phase_started: Instant,
    shown_at: Instant,
    meter: Option<LevelMeter>,
    levels: VecDeque<f32>,
    display: [f32; BARS],
    smoothed: f32,
    label: String,
    show_label: bool,
    hands_free: bool,
    endpoint: Option<f32>,
    endpoint_display: f32,
    scale: f32,
    pos: POINT,
    surface: Option<Surface>,
    text: Option<TextRenderer>,
    theme: ThemeId,
    style: IndicatorStyle,
}

/// Colours for one theme + indicator style.
struct Colors {
    pill: Rgb,
    pill_alpha: f32,
    edge: Rgb,
    edge_alpha: f32,
    sheen_alpha: u8,
    wave: Rgb,
    text: Rgb,
    accent: Rgb,
    accent_2: Rgb,
}

impl Colors {
    fn new(theme: ThemeId, style: IndicatorStyle) -> Colors {
        let pal = theme.palette();
        match style {
            IndicatorStyle::Dark => Colors {
                pill: (20, 20, 27),
                pill_alpha: 0.95,
                edge: (255, 255, 255),
                edge_alpha: 0.10,
                sheen_alpha: 18,
                wave: pal.wave,
                text: (255, 255, 255),
                accent: pal.accent,
                accent_2: pal.accent_2,
            },
            IndicatorStyle::Light => Colors {
                pill: (248, 245, 239),
                pill_alpha: 0.97,
                edge: (0, 0, 0),
                edge_alpha: 0.09,
                sheen_alpha: 90,
                wave: pal.ink,
                text: (28, 28, 34),
                accent: pal.ink,
                accent_2: pal.ink_2,
            },
        }
    }
}

/// GDI DIB section the pixmap is copied into for UpdateLayeredWindow.
struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u8,
    width: i32,
    height: i32,
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

fn solid(r: u8, g: u8, b: u8, a: f32) -> Paint<'static> {
    let mut p = Paint { anti_alias: true, ..Paint::default() };
    p.set_color(Color::from_rgba8(r, g, b, (a.clamp(0.0, 1.0) * 255.0) as u8));
    p
}

impl Overlay {
    /// State without a window, for rendering previews.
    fn offscreen() -> Overlay {
        Overlay {
            hwnd: HWND::default(),
            phase: Phase::Hidden,
            phase_started: Instant::now(),
            shown_at: Instant::now(),
            meter: None,
            levels: VecDeque::from(vec![0.0; BARS]),
            display: [0.0; BARS],
            smoothed: 0.0,
            label: String::new(),
            show_label: true,
            hands_free: false,
            endpoint: None,
            endpoint_display: 0.0,
            scale: 1.0,
            pos: POINT::default(),
            surface: None,
            text: TextRenderer::load(),
            theme: ThemeId::default(),
            style: IndicatorStyle::default(),
        }
    }

    pub fn create(instance: HINSTANCE) -> windows::core::Result<Overlay> {
        unsafe {
            let class = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(overlay_proc),
                hInstance: instance,
                lpszClassName: w!("AudianOverlay"),
                ..Default::default()
            };
            RegisterClassExW(&class);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
                w!("AudianOverlay"),
                w!("Audian"),
                WS_POPUP,
                0,
                0,
                1,
                1,
                None,
                None,
                Some(instance),
                None,
            )?;
            Ok(Overlay { hwnd, ..Overlay::offscreen() })
        }
    }

    pub fn set_appearance(&mut self, theme: ThemeId, style: IndicatorStyle) {
        self.theme = theme;
        self.style = style;
    }

    pub fn is_visible(&self) -> bool {
        self.phase != Phase::Hidden
    }

    pub fn frame_interval() -> Duration {
        FRAME
    }

    fn pill_width(&self) -> f32 {
        if self.show_label && self.text.is_some() { PILL_W_LABEL } else { PILL_W_PLAIN }
    }

    /// Shows the pill for a new recording, positioned relative to the target window.
    pub fn show_recording(&mut self, meter: LevelMeter, target: Option<&Target>, position: OverlayPosition, label: &str, show_label: bool) {
        self.meter = Some(meter);
        self.levels.iter_mut().for_each(|l| *l = 0.0);
        self.display = [0.0; BARS];
        self.smoothed = 0.0;
        self.hands_free = false;
        self.endpoint = None;
        self.endpoint_display = 0.0;
        self.show_label = show_label;
        self.label = label.to_string();
        let was_visible = self.is_visible();
        self.place(target, position);
        if !was_visible {
            self.shown_at = Instant::now();
        }
        self.set_phase(Phase::Recording, label);
    }

    /// Positions the pill at the bottom of the active monitor (used when showing a status
    /// such as an error without a preceding recording).
    pub fn place_default(&mut self) {
        self.place(None, OverlayPosition::BottomCenter);
        self.shown_at = Instant::now();
    }

    pub fn set_hands_free(&mut self, on: bool) {
        self.hands_free = on;
    }

    /// Progress (0..1) towards auto-stop during a pause, or None while speaking.
    pub fn set_endpoint_progress(&mut self, progress: Option<f32>) {
        self.endpoint = progress;
    }

    pub fn set_label(&mut self, label: &str) {
        self.label = label.to_string();
    }

    pub fn set_phase(&mut self, phase: Phase, label: &str) {
        if phase != Phase::Recording {
            self.meter = None;
            self.endpoint = None;
        }
        self.phase = phase;
        self.label = label.to_string();
        self.phase_started = Instant::now();
        if phase == Phase::Hidden {
            unsafe {
                let _ = ShowWindow(self.hwnd, SW_HIDE);
            }
            if let Some(t) = self.text.as_mut() {
                t.release_font();
            }
        } else {
            self.render();
        }
    }

    pub fn hide(&mut self) {
        self.set_phase(Phase::Hidden, "");
    }

    /// Advances the animation. Returns false once the overlay has hidden itself.
    pub fn tick(&mut self) -> bool {
        if self.phase == Phase::Hidden {
            return false;
        }
        if let Some(hold) = self.phase.hold() {
            if self.phase_started.elapsed().as_secs_f32() > hold + FADE {
                self.hide();
                return false;
            }
        }
        if let Some(meter) = &self.meter {
            let level = meter.take();
            // Fast attack, slower release, for a lively but not jittery waveform.
            self.smoothed = if level > self.smoothed { level } else { self.smoothed * 0.55 + level * 0.45 };
            self.levels.pop_front();
            self.levels.push_back(self.smoothed);
        }
        for (d, &l) in self.display.iter_mut().zip(self.levels.iter()) {
            *d += (l - *d) * 0.55;
        }
        let target = self.endpoint.unwrap_or(0.0);
        self.endpoint_display += (target - self.endpoint_display) * 0.35;
        self.render();
        true
    }

    fn place(&mut self, target: Option<&Target>, position: OverlayPosition) {
        unsafe {
            let caret = target.and_then(|t| t.caret);
            let monitor = match (caret, target) {
                (Some(r), _) if position == OverlayPosition::Auto => {
                    MonitorFromPoint(POINT { x: r.left, y: r.bottom }, MONITOR_DEFAULTTONEAREST)
                }
                (_, Some(t)) => MonitorFromWindow(t.hwnd, MONITOR_DEFAULTTONEAREST),
                _ => {
                    let mut pt = POINT::default();
                    let _ = GetCursorPos(&mut pt);
                    MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST)
                }
            };
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            let _ = GetMonitorInfoW(monitor, &mut info);
            let work = info.rcWork;
            let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
            let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y);
            self.scale = dpi_x as f32 / 96.0;
            let s = self.scale;
            let pill_w = (self.pill_width() * s).round() as i32;
            let pill_h = (PILL_H * s).round() as i32;
            let m = (MARGIN * s).round() as i32;
            let gap = (12.0 * s) as i32;

            let (x, y) = match (position, caret) {
                (OverlayPosition::Auto, Some(r)) => {
                    let below = r.bottom + gap;
                    let y = if below + pill_h <= work.bottom { below } else { r.top - gap - pill_h };
                    (r.left - pill_w / 2, y)
                }
                (OverlayPosition::TopCenter, _) => ((work.left + work.right - pill_w) / 2, work.top + 3 * gap),
                _ => ((work.left + work.right - pill_w) / 2, work.bottom - pill_h - 5 * gap),
            };
            let x = x.clamp(work.left + gap, (work.right - pill_w - gap).max(work.left));
            let y = y.clamp(work.top + gap, (work.bottom - pill_h - gap).max(work.top));
            self.pos = POINT { x: x - m, y: y - m };
            self.ensure_surface(pill_w + 2 * m, pill_h + 2 * m);
        }
    }

    fn ensure_surface(&mut self, w: i32, h: i32) {
        if self.surface.as_ref().is_some_and(|s| s.width == w && s.height == h) {
            return;
        }
        self.surface = None;
        unsafe {
            let screen = GetDC(None);
            let dc = CreateCompatibleDC(Some(screen));
            ReleaseDC(None, screen);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let Ok(bitmap) = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) else {
                let _ = DeleteDC(dc);
                return;
            };
            let old = SelectObject(dc, bitmap.into());
            self.surface = Some(Surface { dc, bitmap, old, bits: bits as *mut u8, width: w, height: h });
        }
    }

    fn render(&mut self) {
        let Some((w, h)) = self.surface.as_ref().map(|s| (s.width as u32, s.height as u32)) else { return };
        let t = self.phase_started.elapsed().as_secs_f32();
        let since_shown = self.shown_at.elapsed().as_secs_f32();
        let Some((pm, alpha)) = self.paint(w, h, t, since_shown) else { return };
        self.present(&pm, alpha);
    }

    /// Draws the current state, `t` seconds into the phase. Returns the pixmap and the
    /// window opacity (entrance / exit fades).
    fn paint(&mut self, w: u32, h: u32, t: f32, since_shown: f32) -> Option<(Pixmap, f32)> {
        let mut pm = Pixmap::new(w, h)?;
        let s = self.scale;
        let colors = Colors::new(self.theme, self.style);

        // Entrance (scale + fade) and exit (fade + slight shrink).
        let appear = ease_out(since_shown / APPEAR);
        let exit = match self.phase.hold() {
            Some(hold) if t > hold => ((t - hold) / FADE).clamp(0.0, 1.0),
            _ => 0.0,
        };
        let alpha = appear * (1.0 - exit);
        let zoom = (0.94 + 0.06 * appear) * (1.0 - 0.03 * exit);
        let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
        let tf = Transform::from_translate(cx, cy).pre_scale(zoom, zoom).pre_translate(-cx, -cy);

        let m = MARGIN * s;
        let (pw, ph) = (self.pill_width() * s, PILL_H * s);
        draw_pill(&mut pm, m, m, pw, ph, s, tf, &colors);

        // Content crossfades in on every phase change.
        let content = ease_out(t / 0.14);
        let ox = m;
        let oy = m;
        match self.phase {
            Phase::Recording => self.draw_recording(&mut pm, ox, oy, pw, ph, s, t, tf, content, &colors),
            Phase::Processing => self.draw_processing(&mut pm, ox, oy, pw, ph, s, t, tf, content, &colors),
            Phase::Done => self.draw_status(&mut pm, ox, oy, ph, s, tf, content, StatusIcon::Check, &colors),
            Phase::Error => self.draw_status(&mut pm, ox, oy, ph, s, tf, content, StatusIcon::Alert, &colors),
            Phase::NoSpeech => self.draw_status(&mut pm, ox, oy, ph, s, tf, content, StatusIcon::Muted, &colors),
            Phase::Hidden => {}
        }
        Some((pm, alpha))
    }

    fn present(&self, pm: &Pixmap, alpha: f32) {
        let Some(surface) = self.surface.as_ref() else { return };
        let (w, h) = (pm.width(), pm.height());
        if (w, h) != (surface.width as u32, surface.height as u32) {
            return;
        }
        unsafe {
            // tiny-skia: premultiplied RGBA; GDI wants premultiplied BGRA.
            let dst = std::slice::from_raw_parts_mut(surface.bits, (w * h * 4) as usize);
            for (d, src) in dst.chunks_exact_mut(4).zip(pm.data().chunks_exact(4)) {
                d[0] = src[2];
                d[1] = src[1];
                d[2] = src[0];
                d[3] = src[3];
            }
            let size = SIZE { cx: surface.width, cy: surface.height };
            let src = POINT::default();
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: (alpha * 255.0) as u8,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                self.hwnd,
                None,
                Some(&self.pos),
                Some(&size),
                Some(surface.dc),
                Some(&src),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_SHOWWINDOW | SWP_NOSIZE | SWP_NOMOVE,
            );
        }
    }

    fn label_x(&mut self, ox: f32, pw: f32, s: f32, px: f32) -> Option<f32> {
        if !self.show_label || self.label.is_empty() {
            return None;
        }
        let label = self.label.clone();
        let tr = self.text.as_mut()?;
        Some(ox + pw - 18.0 * s - tr.width(&label, px))
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_recording(&mut self, pm: &mut Pixmap, ox: f32, oy: f32, pw: f32, ph: f32, s: f32, t: f32, tf: Transform, content: f32, c: &Colors) {
        let cy = oy + ph / 2.0;
        // Live dot with a breathing halo (slower breathing in hands-free mode).
        let speed = if self.hands_free { 2.2 } else { 3.6 };
        let breathe = 0.5 + 0.5 * (t * speed).sin();
        if let Some(halo) = PathBuilder::from_circle(ox + 22.0 * s, cy, (6.0 + 3.0 * breathe) * s) {
            pm.fill_path(&halo, &solid(255, 69, 58, 0.18 * content), FillRule::Winding, tf, None);
        }
        if let Some(dot) = PathBuilder::from_circle(ox + 22.0 * s, cy, 4.6 * s) {
            pm.fill_path(&dot, &solid(255, 69, 58, content), FillRule::Winding, tf, None);
        }
        if self.hands_free {
            // A thin ring signals "hands-free: I'll stop when you pause".
            if let Some(ring) = PathBuilder::from_circle(ox + 22.0 * s, cy, 8.5 * s) {
                let stroke = Stroke { width: 1.2 * s, ..Stroke::default() };
                pm.stroke_path(&ring, &solid(255, 120, 110, 0.55 * content), &stroke, tf, None);
            }
        }

        let px = 12.5 * s;
        let label_x = self.label_x(ox, pw, s, px);
        let left = ox + 40.0 * s;
        let right = label_x.map(|x| x - 14.0 * s).unwrap_or(ox + pw - 20.0 * s);
        let step = (right - left) / BARS as f32;
        let bar_w = (step * 0.52).max(1.5 * s);
        let max_h = ph * 0.56;
        let min_h = 3.0 * s;
        let paint = solid(c.wave.0, c.wave.1, c.wave.2, 0.92 * content);
        for (i, &level) in self.display.iter().enumerate() {
            // Speech RMS mostly lives in 0.01..0.2; compress into 0..1, taper the edges.
            let v = (level / 0.13).clamp(0.0, 1.0).powf(0.62);
            let edge = 1.0 - ((i as f32 + 0.5) / BARS as f32 * 2.0 - 1.0).abs().powi(4) * 0.35;
            let bh = min_h + (max_h - min_h) * v * edge;
            let x = left + i as f32 * step + (step - bar_w) / 2.0;
            if let Some(p) = rounded_rect(x, cy - bh / 2.0, bar_w, bh, bar_w / 2.0) {
                pm.fill_path(&p, &paint, FillRule::Winding, tf, None);
            }
        }
        if let (Some(x), Some(tr)) = (label_x, self.text.as_mut()) {
            let label = self.label.clone();
            tr.draw(pm, &label, x, cy + 4.3 * s, px, c.text, 0.6 * content);
        }
        // Auto-stop countdown: an accent line filling along the bottom of the pill.
        if self.endpoint_display > 0.02 {
            let inset = ph / 2.0;
            let len = (pw - 2.0 * inset) * self.endpoint_display.min(1.0);
            let mut pb = PathBuilder::new();
            pb.move_to(ox + inset, oy + ph - 3.0 * s);
            pb.line_to(ox + inset + len.max(0.5), oy + ph - 3.0 * s);
            if let Some(p) = pb.finish() {
                let stroke = Stroke { width: 2.0 * s, line_cap: LineCap::Round, ..Stroke::default() };
                pm.stroke_path(&p, &solid(c.accent.0, c.accent.1, c.accent.2, 0.9 * content), &stroke, tf, None);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_processing(&mut self, pm: &mut Pixmap, ox: f32, oy: f32, pw: f32, ph: f32, s: f32, t: f32, tf: Transform, content: f32, c: &Colors) {
        let cy = oy + ph / 2.0;
        // A compact travelling wave in the theme's gradient.
        let n = 7;
        for i in 0..n {
            let phase = t * 7.5 - i as f32 * 0.55;
            let v = 0.5 + 0.5 * phase.sin();
            let bh = (4.0 + v * v * 14.0) * s;
            let x = ox + (18.0 + i as f32 * 5.2) * s;
            let k = i as f32 / (n - 1) as f32;
            let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * k) as u8;
            let (r, g, b) = (mix(c.accent.0, c.accent_2.0), mix(c.accent.1, c.accent_2.1), mix(c.accent.2, c.accent_2.2));
            if let Some(p) = rounded_rect(x, cy - bh / 2.0, 3.0 * s, bh, 1.5 * s) {
                pm.fill_path(&p, &solid(r, g, b, content), FillRule::Winding, tf, None);
            }
        }
        if let Some(tr) = self.text.as_mut() {
            let dots = ((t * 2.5) as usize) % 4;
            let label = format!("{}{}", self.label, ".".repeat(dots));
            tr.draw(pm, &label, ox + 64.0 * s, cy + 4.6 * s, 13.5 * s, c.text, 0.88 * content);
        }
        let _ = pw;
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_status(&mut self, pm: &mut Pixmap, ox: f32, oy: f32, ph: f32, s: f32, tf: Transform, content: f32, icon: StatusIcon, c: &Colors) {
        let cy = oy + ph / 2.0;
        let cx = ox + 26.0 * s;
        let (r, g, b) = match icon {
            StatusIcon::Check => (52, 199, 89),
            StatusIcon::Alert => (255, 159, 10),
            StatusIcon::Muted => (150, 150, 160),
        };
        // Icon pops in slightly larger, then settles.
        let pop = 1.0 + 0.18 * (1.0 - content);
        if let Some(c) = PathBuilder::from_circle(cx, cy, 11.0 * s * pop) {
            pm.fill_path(&c, &solid(r, g, b, 0.18 * content), FillRule::Winding, tf, None);
        }
        let stroke = Stroke { width: 2.4 * s, line_cap: LineCap::Round, line_join: LineJoin::Round, ..Stroke::default() };
        let mut pb = PathBuilder::new();
        match icon {
            StatusIcon::Check => {
                pb.move_to(cx - 5.0 * s, cy + 0.2 * s);
                pb.line_to(cx - 1.5 * s, cy + 3.8 * s);
                pb.line_to(cx + 5.5 * s, cy - 4.0 * s);
            }
            StatusIcon::Alert => {
                pb.move_to(cx, cy - 5.0 * s);
                pb.line_to(cx, cy + 1.0 * s);
                pb.move_to(cx, cy + 5.0 * s);
                pb.line_to(cx, cy + 5.1 * s);
            }
            StatusIcon::Muted => {
                pb.move_to(cx - 5.0 * s, cy);
                pb.line_to(cx + 5.0 * s, cy);
            }
        }
        if let Some(p) = pb.finish() {
            pm.stroke_path(&p, &solid(r, g, b, content), &stroke, tf, None);
        }
        if let Some(tr) = self.text.as_mut() {
            let label = self.label.clone();
            tr.draw(pm, &label, ox + 46.0 * s, cy + 4.6 * s, 13.5 * s, c.text, 0.9 * content);
        }
    }
}

#[derive(Clone, Copy)]
enum StatusIcon {
    Check,
    Alert,
    Muted,
}

#[allow(clippy::too_many_arguments)]
fn draw_pill(pm: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, s: f32, tf: Transform, c: &Colors) {
    // Soft shadow: stacked, expanding translucent shapes, offset downwards.
    for i in (1..=10).rev() {
        let e = i as f32 * 1.2 * s;
        if let Some(p) = rounded_rect(x - e, y - e + 3.0 * s, w + 2.0 * e, h + 2.0 * e, h / 2.0 + e) {
            pm.fill_path(&p, &solid(0, 0, 0, 0.028), FillRule::Winding, tf, None);
        }
    }
    if let Some(p) = rounded_rect(x, y, w, h, h / 2.0) {
        pm.fill_path(&p, &solid(c.pill.0, c.pill.1, c.pill.2, c.pill_alpha), FillRule::Winding, tf, None);
        // Top sheen.
        let mut sheen = Paint { anti_alias: true, ..Paint::default() };
        if let Some(shader) = tiny_skia::LinearGradient::new(
            tiny_skia::Point::from_xy(0.0, y),
            tiny_skia::Point::from_xy(0.0, y + h),
            vec![
                tiny_skia::GradientStop::new(0.0, Color::from_rgba8(255, 255, 255, c.sheen_alpha)),
                tiny_skia::GradientStop::new(0.5, Color::from_rgba8(255, 255, 255, 0)),
            ],
            tiny_skia::SpreadMode::Pad,
            Transform::identity(),
        ) {
            sheen.shader = shader;
            pm.fill_path(&p, &sheen, FillRule::Winding, tf, None);
        }
        let stroke = Stroke { width: 1.0, ..Stroke::default() };
        pm.stroke_path(&p, &solid(c.edge.0, c.edge.1, c.edge.2, c.edge_alpha), &stroke, tf, None);
    }
}

/// Renders a still of the indicator in `phase` (recording with a frozen speech-like waveform,
/// processing, or done), at `scale` pixels per point. `countdown` shows the auto-stop line
/// that fills during a pause.
pub fn preview(theme: ThemeId, style: IndicatorStyle, show_label: bool, phase: Phase, countdown: bool, scale: f32) -> Option<Pixmap> {
    let mut o = Overlay::offscreen();
    o.set_appearance(theme, style);
    o.show_label = show_label;
    o.scale = scale;
    o.phase = phase;
    o.label = match phase {
        Phase::Processing => "Polishing".into(),
        Phase::Done => "18 words".into(),
        _ => "Natural".into(),
    };
    for (i, d) in o.display.iter_mut().enumerate() {
        let x = i as f32 / BARS as f32;
        *d = 0.015 + 0.12 * (0.5 + 0.5 * (x * 9.0).sin()) * (x * 3.1 + 0.4).sin().abs();
    }
    if countdown {
        o.endpoint_display = 0.45;
    }
    let w = ((o.pill_width() + 2.0 * MARGIN) * scale).ceil() as u32;
    let h = ((PILL_H + 2.0 * MARGIN) * scale).ceil() as u32;
    // 1.25 s into the phase: fully faded in, and "Polishing..." shows all three dots.
    o.paint(w, h, 1.25, 10.0).map(|(pm, _)| pm)
}
