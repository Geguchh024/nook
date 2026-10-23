//! Color picker: pick any pixel on screen and its code lands on the
//! clipboard, like PowerToys Color Picker.
//!
//! The screen freezes under a loupe (`nook_platform::snip`, pick mode):
//! scroll to zoom, arrows nudge by a pixel (Shift: ten), click or Enter
//! picks, Esc cancels. The color is copied in the chosen format (HEX, RGB,
//! HSL, HSV or OKLCH, written as modern CSS) and kept in a short history.
//! The latest color sits as a dot on the tile; right click opens the
//! history page, where any color copies again in the current format.
//! Ctrl+Alt+P starts it from anywhere. The history lives in memory only.

use egui::{Align2, Color32, Id, Rect, ScrollArea, Sense, Stroke, Ui, pos2, vec2};
use nook_platform::clipboard;
use nook_platform::snip::{SnipMode, SnipOutcome};
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_LABEL, TILE_RADIUS, Theme, with_alpha};
use nook_ui::{anim, icons, paint, widgets};
use std::time::Duration;

use crate::snipper::Snipper;

const HINT: &str =
    "Click to copy  \u{00B7}  Scroll to zoom  \u{00B7}  Arrows to nudge  \u{00B7}  Esc to cancel";
/// Colors kept.
const MAX_HISTORY: usize = 12;
/// How long the "Copied" confirmation shows on the page.
const COPIED_FOR: f64 = 1.2;
/// History page grid.
const COLS: usize = 4;
const GAP: f32 = 8.0;
const CARD_H: f32 = 76.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    #[default]
    Hex,
    Rgb,
    Hsl,
    Hsv,
    Oklch,
}

impl Format {
    const ALL: [Format; 5] = [
        Format::Hex,
        Format::Rgb,
        Format::Hsl,
        Format::Hsv,
        Format::Oklch,
    ];
    const LABELS: [&str; 5] = ["HEX", "RGB", "HSL", "HSV", "OKLCH"];

    pub fn format(self, [r, g, b]: [u8; 3]) -> String {
        match self {
            Format::Hex => format!("#{r:02X}{g:02X}{b:02X}"),
            Format::Rgb => format!("rgb({r} {g} {b})"),
            Format::Hsl => {
                let (h, s, l) = hsl([r, g, b]);
                format!("hsl({h:.0} {:.0}% {:.0}%)", s * 100.0, l * 100.0)
            }
            Format::Hsv => {
                let (h, s, v) = hsv([r, g, b]);
                format!("hsv({h:.0} {:.0}% {:.0}%)", s * 100.0, v * 100.0)
            }
            Format::Oklch => {
                let (l, c, h) = oklch([r, g, b]);
                format!("oklch({:.1}% {c:.3} {h:.1})", l * 100.0)
            }
        }
    }
}

fn unit([r, g, b]: [u8; 3]) -> (f32, f32, f32) {
    (
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    )
}

/// Hue in degrees (0 for grays) from the channel extremes.
fn hue(r: f32, g: f32, b: f32, max: f32, delta: f32) -> f32 {
    if delta <= f32::EPSILON {
        return 0.0;
    }
    let h = if max == r {
        ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    (h * 60.0).rem_euclid(360.0)
}

/// (hue degrees, saturation 0..1, lightness 0..1).
fn hsl(c: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = unit(c);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let delta = max - min;
    let l = (max + min) / 2.0;
    let s = if delta <= f32::EPSILON {
        0.0
    } else {
        delta / (1.0 - (2.0 * l - 1.0).abs())
    };
    (hue(r, g, b, max, delta), s, l)
}

/// (hue degrees, saturation 0..1, value 0..1).
fn hsv(c: [u8; 3]) -> (f32, f32, f32) {
    let (r, g, b) = unit(c);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let delta = max - min;
    let s = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    (hue(r, g, b, max, delta), s, max)
}

/// OKLCH (Björn Ottosson's OKLab in polar form): lightness 0..1, chroma,
/// hue degrees (0 for grays).
fn oklch(c: [u8; 3]) -> (f32, f32, f32) {
    let lin = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    let (r, g, b) = unit(c);
    let (r, g, b) = (lin(r), lin(g), lin(b));
    let l = (0.412_221_47 * r + 0.536_332_54 * g + 0.051_445_99 * b).cbrt();
    let m = (0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b).cbrt();
    let s = (0.088_302_46 * r + 0.281_718_84 * g + 0.629_978_7 * b).cbrt();
    let ok_l = 0.210_454_26 * l + 0.793_617_8 * m - 0.004_072_047 * s;
    let ok_a = 1.977_998_5 * l - 2.428_592_2 * m + 0.450_593_7 * s;
    let ok_b = 0.025_904_037 * l + 0.782_771_77 * m - 0.808_675_77 * s;
    let chroma = ok_a.hypot(ok_b);
    let h = if chroma < 0.000_5 {
        0.0
    } else {
        ok_b.atan2(ok_a).to_degrees().rem_euclid(360.0)
    };
    (ok_l, if chroma < 0.000_5 { 0.0 } else { chroma }, h)
}

#[derive(Default)]
pub struct ColorPicker {
    snipper: Snipper<[u8; 3]>,
    /// Newest first.
    history: Vec<[u8; 3]>,
    format: Format,
    open_page: bool,
    /// History index copied from the page, and when.
    copied: Option<(usize, f64)>,
}

impl ColorPicker {
    fn start(&mut self) {
        let format = self.format;
        self.snipper.toggle(SnipMode::Pick, HINT, move |outcome| {
            let SnipOutcome::Color { rgb, at } = outcome else {
                return None;
            };
            let text = format.format(rgb);
            tracing::info!("color picker: {text} at {at:?}");
            if let Err(e) = clipboard::copy_text(&text) {
                tracing::warn!("color picker: copying: {e}");
            }
            Some(rgb)
        });
    }

    fn remember(&mut self, rgb: [u8; 3]) {
        self.history.retain(|&c| c != rgb);
        self.history.insert(0, rgb);
        self.history.truncate(MAX_HISTORY);
    }

    fn copy(&mut self, index: usize, now: f64) {
        let Some(&rgb) = self.history.get(index) else {
            return;
        };
        match clipboard::copy_text(&self.format.format(rgb)) {
            Ok(()) => self.copied = Some((index, now)),
            Err(e) => tracing::warn!("color picker: copying: {e}"),
        }
    }
}

fn color32([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

impl Module for ColorPicker {
    fn id(&self) -> &'static str {
        "color_picker"
    }

    fn name(&self) -> &str {
        "Color picker"
    }

    fn icon(&self) -> &'static str {
        icons::EYEDROPPER
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn attach(&mut self, ctx: &egui::Context) {
        self.snipper.attach(ctx);
    }

    fn is_active(&self) -> bool {
        self.snipper.busy()
    }

    fn overlay_open(&self) -> bool {
        self.snipper.busy()
    }

    fn on_tick(&mut self) {
        for rgb in self.snipper.poll() {
            self.remember(rgb);
            self.snipper.flash(true);
        }
    }

    fn next_repaint(&self, _tile_visible: bool) -> Option<Duration> {
        self.snipper.next_repaint()
    }

    fn default_hotkey(&self) -> Option<&'static str> {
        Some("ctrl+alt+KeyP")
    }

    fn on_hotkey(&mut self) {
        self.start();
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.open_page)
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        let rect = ui.max_rect();
        let resp = self
            .snipper
            .tile(ui, ctx, icons::EYEDROPPER, "color-tile")
            .on_hover_text("Click to pick  \u{00B7}  Right click for history");
        if resp.clicked() {
            self.start();
        }
        if resp.secondary_clicked() {
            self.open_page = true;
        }
        // The latest color as a dot in the corner.
        if let Some(&c) = self.history.first() {
            let center = rect.center() + vec2(11.0, 10.0);
            ui.painter()
                .circle(center, 5.0, color32(c), Stroke::new(1.5, theme.tile_bottom));
        }
    }

    fn ui_page(&mut self, ui: &mut Ui, header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        let now = ui.input(|i| i.time);
        if self.copied.is_some_and(|(_, at)| now - at >= COPIED_FOR) {
            self.copied = None;
        }
        if !self.history.is_empty()
            && widgets::icon_button(header, icons::TRASH, 26.0, false, theme)
                .on_hover_text("Clear history")
                .clicked()
        {
            self.history.clear();
        }
        if widgets::icon_button(header, icons::EYEDROPPER, 26.0, false, theme)
            .on_hover_text("Pick a color (Ctrl+Alt+P)")
            .clicked()
        {
            self.start();
        }
        let mut selected = Format::ALL
            .iter()
            .position(|&f| f == self.format)
            .unwrap_or(0);
        if widgets::segmented(header, &Format::LABELS, &mut selected, theme)
            .on_hover_text("Format to copy")
            .changed()
        {
            self.format = Format::ALL[selected];
        }

        if self.history.is_empty() {
            ui.painter().text(
                body.center(),
                Align2::CENTER_CENTER,
                "No colors yet. Pick one from anywhere on screen.",
                Theme::font(SIZE_LABEL + 1.0),
                theme.text_dim,
            );
            return false;
        }

        let mut copy = None;
        let rows = self.history.len().div_ceil(COLS);
        ScrollArea::vertical()
            .id_salt("nook-color-scroll")
            .max_height(body.height())
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let (area, _) = ui.allocate_exact_size(
                    vec2(body.width(), rows as f32 * (CARD_H + GAP) - GAP),
                    Sense::hover(),
                );
                copy = self.cards(ui, area, theme);
            });
        if let Some(i) = copy {
            self.copy(i, now);
        }
        false
    }
}

impl ColorPicker {
    /// The history as a grid of cards; returns the index clicked.
    fn cards(&self, ui: &mut Ui, area: Rect, theme: &Theme) -> Option<usize> {
        let card_w = (area.width() - GAP * (COLS - 1) as f32) / COLS as f32;
        let mut copy = None;
        for (i, &c) in self.history.iter().enumerate() {
            let (row, col) = (i / COLS, i % COLS);
            let card = Rect::from_min_size(
                pos2(
                    area.min.x + col as f32 * (card_w + GAP),
                    area.min.y + row as f32 * (CARD_H + GAP),
                ),
                vec2(card_w, CARD_H),
            );
            if !ui.is_rect_visible(card) {
                continue;
            }
            widgets::card(ui.painter(), card, theme);
            let id = Id::new(("color-card", i));
            let resp = ui.interact(card, id, Sense::click());
            let hover = anim::tween(ui.ctx(), id.with("hover"), resp.hovered(), 0.12);
            if hover > 0.0 {
                let wash = if theme.dark {
                    Color32::from_white_alpha(18)
                } else {
                    Color32::from_black_alpha(12)
                };
                ui.painter()
                    .rect_filled(card, TILE_RADIUS, with_alpha(wash, hover));
            }
            if resp.clicked() {
                copy = Some(i);
            }
            let inner = card.shrink(8.0);
            let swatch = Rect::from_min_size(inner.min, vec2(inner.width(), 38.0));
            ui.painter().rect(
                swatch,
                8.0,
                color32(c),
                Stroke::new(1.0, theme.tile_rim),
                egui::StrokeKind::Inside,
            );
            let label_y = inner.max.y - 7.0;
            if self.copied.is_some_and(|(j, _)| j == i) {
                paint::icon(
                    ui.painter(),
                    pos2(inner.min.x + 6.0, label_y),
                    icons::CHECK,
                    13.0,
                    theme.accent_fg(),
                    true,
                );
                ui.painter().text(
                    pos2(inner.min.x + 18.0, label_y),
                    Align2::LEFT_CENTER,
                    "Copied",
                    Theme::font(SIZE_LABEL),
                    theme.accent_fg(),
                );
            } else {
                ui.painter().text(
                    pos2(inner.min.x + 2.0, label_y),
                    Align2::LEFT_CENTER,
                    self.format.format(c),
                    Theme::font(SIZE_LABEL),
                    theme.text,
                );
            }
        }
        copy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_like_css() {
        let blue = [0x12, 0x9A, 0xE3];
        assert_eq!(Format::Hex.format(blue), "#129AE3");
        assert_eq!(Format::Rgb.format(blue), "rgb(18 154 227)");
        assert_eq!(Format::Hsl.format(blue), "hsl(201 85% 48%)");
        assert_eq!(Format::Hsv.format(blue), "hsv(201 92% 89%)");
        assert_eq!(Format::Hsl.format([128, 128, 128]), "hsl(0 0% 50%)");
    }

    #[test]
    fn oklch_matches_reference_values() {
        // Reference: red is oklch(62.8% 0.2577 29.23), white is (100% 0 0).
        assert_eq!(Format::Oklch.format([255, 0, 0]), "oklch(62.8% 0.258 29.2)");
        assert_eq!(
            Format::Oklch.format([255, 255, 255]),
            "oklch(100.0% 0.000 0.0)"
        );
        assert_eq!(Format::Oklch.format([0, 0, 0]), "oklch(0.0% 0.000 0.0)");
    }

    #[test]
    fn history_dedupes_and_caps() {
        let mut p = ColorPicker::default();
        for i in 0..20u8 {
            p.remember([i, 0, 0]);
        }
        p.remember([15, 0, 0]);
        assert_eq!(p.history.len(), MAX_HISTORY);
        assert_eq!(p.history[0], [15, 0, 0]);
        assert_eq!(p.history.iter().filter(|c| c[0] == 15).count(), 1);
    }
}
