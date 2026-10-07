//! Custom, animated widgets for the Audian window.

use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, Id, Layout, Margin, Rect, Response, RichText, Sense, Stroke, StrokeKind, Ui, pos2, vec2,
};

use crate::theme::{self, *};

/// iOS/Windows-style switch with a sliding knob.
pub fn toggle(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(44.0, 24.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.16);
    let hover = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let r = rect.height() / 2.0;
        let off = lerp_color(Color32::from_rgb(46, 47, 61), Color32::from_rgb(58, 59, 76), hover);
        p.rect_filled(rect, r, lerp_color(off, accent(), t));
        if t < 0.99 {
            p.rect_stroke(rect, r, Stroke::new(1.0, Color32::from_rgb(64, 65, 84).linear_multiply(1.0 - t)), StrokeKind::Inside);
        }
        let x = egui::lerp(rect.left() + r..=rect.right() - r, ease_out(t));
        let knob = r - 4.0 + hover * 0.8;
        p.circle_filled(pos2(x, rect.center().y + 0.6), knob + 0.6, Color32::from_black_alpha(40));
        // White knob, or the on-accent colour once on (dark on light accents such as Ivory).
        p.circle_filled(pos2(x, rect.center().y), knob, lerp_color(Color32::WHITE, on_accent(), t));
    }
    resp
}

/// Segmented control with a highlight that glides to the selected option.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, id: Id, value: &mut T, options: &[(T, &str)]) -> bool {
    let font = theme::body(13.5);
    let pad = 14.0;
    let h = 32.0;
    let widths: Vec<f32> = options
        .iter()
        .map(|(_, l)| ui.painter().layout_no_wrap(l.to_string(), font.clone(), TEXT).size().x + 2.0 * pad)
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, h), Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(rect, 10.0, INSET);
    p.rect_stroke(rect, 10.0, Stroke::new(1.0, BORDER), StrokeKind::Inside);

    let mut x = rect.left() + 3.0;
    let mut slots = Vec::with_capacity(options.len());
    for w in &widths {
        slots.push(Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(*w, h - 6.0)));
        x += w;
    }
    let selected = options.iter().position(|(v, _)| v == value).unwrap_or(0);
    let target = slots[selected];
    let ax = ui.ctx().animate_value_with_time(id.with("x"), target.left(), 0.18);
    let aw = ui.ctx().animate_value_with_time(id.with("w"), target.width(), 0.18);
    let hl = Rect::from_min_size(pos2(ax, target.top()), vec2(aw, target.height()));
    p.rect_filled(hl, 8.0, tint(Color32::from_rgb(40, 41, 54), 0.1));
    p.rect_stroke(hl, 8.0, Stroke::new(1.0, accent().linear_multiply(0.55)), StrokeKind::Inside);

    let mut changed = false;
    for (i, ((v, label), slot)) in options.iter().zip(&slots).enumerate() {
        let resp = ui.interact(*slot, id.with(i), Sense::click());
        if resp.clicked() && *value != *v {
            *value = *v;
            changed = true;
        }
        let color = if i == selected {
            TEXT
        } else if resp.hovered() {
            Color32::from_rgb(210, 210, 222)
        } else {
            TEXT_DIM
        };
        p.text(slot.center(), Align2::CENTER_CENTER, *label, font.clone(), color);
    }
    changed
}

/// Rounded, softly shadowed container.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(14))
        .inner_margin(Margin::symmetric(20, 16))
        .shadow(egui::Shadow { offset: [0, 6], blur: 22, spread: 0, color: Color32::from_black_alpha(70) })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Card heading with an icon.
pub fn card_title(ui: &mut Ui, icon: &str, title: &str, subtitle: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).font(theme::icons(17.0)).color(accent()));
        ui.add_space(2.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(title).font(theme::semibold(16.0)).color(TEXT));
            if !subtitle.is_empty() {
                ui.label(RichText::new(subtitle).size(12.5).color(TEXT_DIM));
            }
        });
    });
    ui.add_space(6.0);
}

/// A setting: title + optional explanation on the left, control on the right.
pub fn row(ui: &mut Ui, title: &str, subtitle: &str, control: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        let w = ui.available_width();
        ui.allocate_ui_with_layout(vec2((w * 0.52).max(220.0), 0.0), Layout::top_down(Align::Min), |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(RichText::new(title).size(14.5).color(TEXT));
            if !subtitle.is_empty() {
                ui.add(egui::Label::new(RichText::new(subtitle).size(12.5).color(TEXT_DIM)).wrap());
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
}

pub fn divider(ui: &mut Ui) {
    ui.add_space(4.0);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, BORDER);
    ui.add_space(4.0);
}

pub fn hint(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).size(12.5).color(TEXT_FAINT)).wrap());
}

/// Page title and subtitle.
pub fn page_header(ui: &mut Ui, title: &str, subtitle: &str) {
    ui.label(RichText::new(title).font(theme::semibold(28.0)).color(TEXT));
    if !subtitle.is_empty() {
        ui.add_space(-4.0);
        ui.label(RichText::new(subtitle).size(14.0).color(TEXT_DIM));
    }
    ui.add_space(14.0);
}

fn button_impl(ui: &mut Ui, text: &str, icon: Option<&str>, fill: Color32, hover_fill: Color32, text_color: Color32, stroke: Option<Color32>) -> Response {
    let font = theme::semibold(14.0);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font.clone(), text_color);
    let icon_w = if icon.is_some() { 22.0 } else { 0.0 };
    let size = vec2(galley.size().x + icon_w + 32.0, 36.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let enabled = ui.is_enabled();
    let hover = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered() && enabled, 0.12);
    let press = if resp.is_pointer_button_down_on() { 0.92 } else { 1.0 };
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let fill = lerp_color(fill, hover_fill, hover).linear_multiply(if enabled { press } else { 0.45 });
        p.rect_filled(rect, 10.0, fill);
        if let Some(s) = stroke {
            p.rect_stroke(rect, 10.0, Stroke::new(1.0, s), StrokeKind::Inside);
        }
        let color = if enabled { text_color } else { text_color.linear_multiply(0.5) };
        let mut x = rect.left() + 16.0;
        if let Some(icon) = icon {
            p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, icon, theme::icons(14.0), color);
            x += icon_w;
        }
        p.galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, color);
    }
    resp
}

pub fn primary(ui: &mut Ui, text: &str) -> Response {
    button_impl(ui, text, None, accent(), accent_hover(), on_accent(), None)
}

pub fn primary_icon(ui: &mut Ui, icon: &str, text: &str) -> Response {
    button_impl(ui, text, Some(icon), accent(), accent_hover(), on_accent(), None)
}

pub fn secondary(ui: &mut Ui, text: &str) -> Response {
    button_impl(ui, text, None, Color32::from_rgb(34, 35, 46), Color32::from_rgb(44, 45, 60), TEXT, Some(BORDER))
}

pub fn secondary_icon(ui: &mut Ui, icon: &str, text: &str) -> Response {
    button_impl(ui, text, Some(icon), Color32::from_rgb(34, 35, 46), Color32::from_rgb(44, 45, 60), TEXT, Some(BORDER))
}

pub fn danger(ui: &mut Ui, text: &str) -> Response {
    button_impl(ui, text, None, Color32::from_rgb(58, 28, 32), Color32::from_rgb(80, 34, 40), DANGER, Some(Color32::from_rgb(96, 44, 50)))
}

/// Small square icon button.
pub fn icon_button(ui: &mut Ui, icon: &str, tooltip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
    let p = ui.painter();
    p.rect_filled(rect, 8.0, Color32::from_rgb(44, 45, 60).linear_multiply(hover));
    p.text(rect.center(), Align2::CENTER_CENTER, icon, theme::icons(14.0), lerp_color(TEXT_DIM, TEXT, hover));
    resp.on_hover_text(tooltip)
}

pub fn badge(ui: &mut Ui, text: &str, color: Color32) {
    let font = theme::semibold(11.5);
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, color);
    let size = galley.size() + vec2(16.0, 6.0);
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(rect, size.y / 2.0, color.linear_multiply(0.14));
    ui.painter().galley(rect.center() - galley.size() / 2.0, galley, color);
}

/// Keyboard keys rendered as keycaps, e.g. "Ctrl + Shift + Space".
pub fn keycaps(ui: &mut Ui, shortcut: &str, size: f32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (i, key) in shortcut.split('+').map(str::trim).filter(|k| !k.is_empty()).enumerate() {
            if i > 0 {
                ui.label(RichText::new("+").size(size * 0.8).color(TEXT_FAINT));
            }
            let font = theme::semibold(size);
            let galley = ui.painter().layout_no_wrap(key.to_string(), font, TEXT);
            let w = (galley.size().x + size * 1.3).max(size * 2.0);
            let h = size * 2.0;
            let (rect, _) = ui.allocate_exact_size(vec2(w, h + 3.0), Sense::hover());
            let cap = Rect::from_min_size(rect.min, vec2(w, h));
            let p = ui.painter();
            p.rect_filled(cap.translate(vec2(0.0, 3.0)), 8.0, Color32::from_rgb(12, 12, 16));
            p.rect_filled(cap, 8.0, Color32::from_rgb(40, 41, 54));
            p.rect_stroke(cap, 8.0, Stroke::new(1.0, Color32::from_rgb(62, 63, 82)), StrokeKind::Inside);
            p.galley(cap.center() - galley.size() / 2.0, galley, TEXT);
        }
    });
}

/// Rounded progress bar in the accent colour.
pub fn progress(ui: &mut Ui, fraction: f32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 8.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, INSET);
    let f = fraction.clamp(0.0, 1.0);
    if f > 0.0 {
        let fill = Rect::from_min_size(rect.min, vec2((rect.width() * f).max(8.0), rect.height()));
        p.rect_filled(fill, 4.0, lerp_color(accent(), accent2(), f));
    }
}

/// Live level meter with a smoothed bar and a threshold marker.
pub fn level_meter(ui: &mut Ui, level: f32, threshold: f32, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 12.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 6.0, INSET);
    let map = |l: f32| (l / 0.14).clamp(0.0, 1.0).powf(0.62);
    let v = map(level);
    if v > 0.0 {
        let fill = Rect::from_min_size(rect.min, vec2((rect.width() * v).max(12.0), rect.height()));
        let color = if level < threshold { TEXT_FAINT } else { lerp_color(accent(), accent2(), v) };
        p.rect_filled(fill, 6.0, color);
    }
    let tx = rect.left() + rect.width() * map(threshold);
    p.line_segment([pos2(tx, rect.top() - 3.0), pos2(tx, rect.bottom() + 3.0)], Stroke::new(1.5, WARN.linear_multiply(0.8)));
}

/// A selectable tile (mode / provider choice) with icon, title and description.
pub fn choice_tile(ui: &mut Ui, size: egui::Vec2, icon: &str, title: &str, desc: &str, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
    let sel = ui.ctx().animate_bool_with_time(resp.id.with("sel"), selected, 0.18);
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let lift = hover * 1.5;
        let r = rect.translate(vec2(0.0, -lift));
        p.rect_filled(r.translate(vec2(0.0, 4.0)), 12.0, Color32::from_black_alpha((30.0 + 30.0 * hover) as u8));
        let accent = accent();
        p.rect_filled(r, 12.0, lerp_color(lerp_color(CARD, CARD_HOVER, hover), tint(CARD, 0.08), sel));
        p.rect_stroke(r, 12.0, Stroke::new(1.0 + sel, lerp_color(BORDER, accent, sel)), StrokeKind::Inside);
        let pad = 14.0;
        p.text(pos2(r.left() + pad, r.top() + pad + 9.0), Align2::LEFT_CENTER, icon, theme::icons(18.0), lerp_color(TEXT_DIM, accent, sel.max(hover * 0.6)));
        p.text(pos2(r.left() + pad + 28.0, r.top() + pad + 9.0), Align2::LEFT_CENTER, title, theme::semibold(14.5), TEXT);
        if sel > 0.01 {
            p.text(pos2(r.right() - pad, r.top() + pad + 9.0), Align2::RIGHT_CENTER, theme::icon::CHECK, theme::icons(13.0), accent.linear_multiply(sel));
        }
        let galley = p.layout(desc.to_string(), theme::body(12.5), TEXT_DIM, r.width() - 2.0 * pad);
        p.galley(pos2(r.left() + pad, r.top() + pad + 26.0), galley, TEXT_DIM);
    }
    resp
}

/// Big number with a caption.
pub fn stat_tile(ui: &mut Ui, width: f32, icon: &str, value: &str, label: &str, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 96.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect.translate(vec2(0.0, 4.0)), 14.0, Color32::from_black_alpha(50));
    p.rect_filled(rect, 14.0, CARD);
    p.rect_stroke(rect, 14.0, Stroke::new(1.0, BORDER), StrokeKind::Inside);
    let icon_rect = Rect::from_min_size(rect.min + vec2(16.0, 16.0), vec2(30.0, 30.0));
    p.rect_filled(icon_rect, 9.0, color.linear_multiply(0.16));
    p.text(icon_rect.center(), Align2::CENTER_CENTER, icon, theme::icons(15.0), color);
    p.text(pos2(rect.left() + 16.0, rect.bottom() - 30.0), Align2::LEFT_CENTER, value, theme::semibold(24.0), TEXT);
    p.text(pos2(rect.left() + 16.0, rect.bottom() - 12.0), Align2::LEFT_CENTER, label, theme::body(12.5), TEXT_DIM);
}

/// Small status dot; `glow` adds a soft halo (static, so an idle window doesn't keep
/// repainting itself).
pub fn status_dot(ui: &mut Ui, color: Color32, glow: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
    let c = rect.center();
    if glow {
        ui.painter().circle_filled(c, 7.0, color.linear_multiply(0.16));
    }
    ui.painter().circle_filled(c, 4.0, color);
}

/// A theme choice: a miniature window drawn in the theme's colours, its name and a short
/// description. The tile uses its own accent (not the current one) for the preview and border.
pub fn theme_tile(ui: &mut Ui, size: egui::Vec2, name: &str, desc: &str, accent: Color32, accent2: Color32, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let hover = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
    let sel = ui.ctx().animate_bool_with_time(resp.id.with("sel"), selected, 0.18);
    if ui.is_rect_visible(rect) {
        let p = ui.painter();
        let r = rect.translate(vec2(0.0, -hover * 1.5));
        p.rect_filled(r.translate(vec2(0.0, 4.0)), 12.0, Color32::from_black_alpha((30.0 + 30.0 * hover) as u8));
        p.rect_filled(r, 12.0, lerp_color(lerp_color(CARD, CARD_HOVER, hover), lerp_color(CARD, accent, 0.08), sel));
        p.rect_stroke(r, 12.0, Stroke::new(1.0 + sel, lerp_color(BORDER, accent, sel)), StrokeKind::Inside);

        // Miniature window: sidebar with a selected item, a button and a switch.
        let pad = 12.0;
        let mini = Rect::from_min_size(pos2(r.left() + pad, r.top() + pad), vec2(78.0, r.height() - 2.0 * pad));
        p.rect_filled(mini, 8.0, BG);
        p.rect_stroke(mini, 8.0, Stroke::new(1.0, BORDER), StrokeKind::Inside);
        let side = Rect::from_min_max(mini.min, pos2(mini.left() + 24.0, mini.bottom()));
        p.rect_filled(side, CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }, SIDEBAR);
        for i in 0..3 {
            let y = mini.top() + 10.0 + i as f32 * 9.0;
            let c = if i == 1 { accent } else { Color32::from_rgb(58, 59, 72) };
            p.rect_filled(Rect::from_min_size(pos2(side.left() + 6.0, y), vec2(12.0, 3.0)), 1.5, c);
        }
        let button = Rect::from_min_size(pos2(side.right() + 8.0, mini.top() + 9.0), vec2(30.0, 10.0));
        p.rect_filled(button, 5.0, accent);
        let line = |y: f32, w: f32| Rect::from_min_size(pos2(side.right() + 8.0, y), vec2(w, 3.0));
        p.rect_filled(line(mini.top() + 26.0, 38.0), 1.5, Color32::from_rgb(70, 71, 84));
        p.rect_filled(line(mini.top() + 33.0, 28.0), 1.5, Color32::from_rgb(56, 57, 68));
        let sw = Rect::from_min_size(pos2(side.right() + 8.0, mini.bottom() - 16.0), vec2(18.0, 10.0));
        p.rect_filled(sw, 5.0, lerp_color(accent, accent2, 0.35));
        p.circle_filled(pos2(sw.right() - 5.0, sw.center().y), 3.2, Color32::WHITE);

        let tx = mini.right() + 14.0;
        p.text(pos2(tx, r.top() + pad + 9.0), Align2::LEFT_CENTER, name, theme::semibold(14.5), TEXT);
        if sel > 0.01 {
            p.text(pos2(r.right() - pad, r.top() + pad + 9.0), Align2::RIGHT_CENTER, theme::icon::CHECK, theme::icons(13.0), accent.linear_multiply(sel));
        }
        let galley = p.layout(desc.to_string(), theme::body(12.5), TEXT_DIM, r.right() - pad - tx);
        p.galley(pos2(tx, r.top() + pad + 22.0), galley, TEXT_DIM);
    }
    resp
}
