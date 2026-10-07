//! Appearance: colour theme of this window and the recording indicator.

use audian_common::config::{IndicatorStyle, OverlayPosition, ThemeId};
use eframe::egui::{self, Align2, Color32, RichText, Sense, pos2, vec2};

use super::super::theme::{self, icon, *};
use super::super::widgets as w;
use super::super::SettingsApp;
use super::{tile_width, tiles_per_row};
use crate::overlay::{self, Phase};

pub fn show(app: &mut SettingsApp, ui: &mut egui::Ui) {
    w::page_header(ui, "Appearance", "Colours for this window, the logo and the recording indicator.");
    w::card(ui, |ui| {
        w::card_title(ui, icon::PALETTE, "Theme", "Applies instantly.");
        let per_row = tiles_per_row(ui);
        let tw = tile_width(ui, per_row);
        for row in ThemeId::ALL.chunks(per_row) {
            ui.horizontal(|ui| {
                for &t in row {
                    let pal = t.palette();
                    let selected = app.draft.appearance.theme == t;
                    let resp = w::theme_tile(ui, vec2(tw, 92.0), t.label(), t.description(), theme::rgb(pal.accent), theme::rgb(pal.accent_2), selected);
                    if resp.clicked() {
                        app.draft.appearance.theme = t;
                    }
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                }
            });
        }
    });
    ui.add_space(12.0);
    w::card(ui, |ui| {
        w::card_title(ui, icon::MIC, "Recording indicator", "The pill that appears while you dictate.");
        indicator_preview(app, ui);
        ui.add_space(6.0);
        w::row(ui, "Style", "Dark glass, or light with a waveform in your theme's colour.", |ui| {
            w::segmented(ui, egui::Id::new("ov-style"), &mut app.draft.overlay.style, &[(IndicatorStyle::Dark, "Dark"), (IndicatorStyle::Light, "Light")]);
        });
        w::divider(ui);
        ui.label(RichText::new("Position").size(14.5).color(TEXT));
        w::segmented(
            ui,
            egui::Id::new("ov-pos"),
            &mut app.draft.overlay.position,
            &[(OverlayPosition::Auto, "Near text cursor"), (OverlayPosition::BottomCenter, "Bottom"), (OverlayPosition::TopCenter, "Top")],
        );
        w::hint(ui, "Near the text cursor when the app reports it; otherwise at the bottom of the screen.");
        w::divider(ui);
        w::row(ui, "Show mode and status text", "", |ui| {
            w::toggle(ui, &mut app.draft.overlay.show_label);
        });
    });
}

/// Both indicator states side by side, over a desktop-like backdrop.
fn indicator_preview(app: &mut SettingsApp, ui: &mut egui::Ui) {
    let key = (app.draft.appearance.theme, app.draft.overlay.style, app.draft.overlay.show_label);
    let ppp = ui.ctx().pixels_per_point();
    if app.indicator_preview.as_ref().map(|(k, _)| *k) != Some(key) {
        let textures = [Phase::Recording, Phase::Processing]
            .into_iter()
            .filter_map(|phase| overlay::preview(key.0, key.1, key.2, phase, phase == Phase::Recording, ppp))
            .enumerate()
            .map(|(i, pm)| {
                let img = egui::ColorImage::from_rgba_unmultiplied([pm.width() as usize, pm.height() as usize], &audian_art::to_rgba(&pm));
                ui.ctx().load_texture(format!("indicator-{i}"), img, egui::TextureOptions::LINEAR)
            })
            .collect();
        app.indicator_preview = Some((key, textures));
    }
    let Some((_, textures)) = &app.indicator_preview else { return };

    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 118.0), Sense::hover());
    // Backdrop: a mid-tone, desktop-like surface on which both styles read well.
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 12.0, Color32::from_rgb(58, 61, 78));
    painter.rect_filled(rect.shrink2(vec2(0.0, rect.height() * 0.5)).translate(vec2(0.0, rect.height() * 0.25)), 12.0, Color32::from_white_alpha(6));
    painter.rect_stroke(rect, 12.0, egui::Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);

    let sizes: Vec<egui::Vec2> = textures.iter().map(|t| t.size_vec2() / ppp).collect();
    let gap = 28.0;
    let total: f32 = sizes.iter().map(|s| s.x).sum::<f32>() + gap * (sizes.len().saturating_sub(1)) as f32;
    let mut x = rect.center().x - total / 2.0;
    for ((tex, size), caption) in textures.iter().zip(&sizes).zip(["While you speak", "While processing"]) {
        let r = egui::Rect::from_min_size(pos2(x, rect.top() + 8.0), *size);
        painter.image(tex.id(), r, egui::Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        painter.text(pos2(r.center().x, rect.bottom() - 14.0), Align2::CENTER_CENTER, caption, theme::body(12.0), Color32::from_rgb(214, 216, 228));
        x += size.x + gap;
    }
}
