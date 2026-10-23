//! Font detector: drag over some text on screen and see which installed
//! fonts it looks like.
//!
//! The screen freezes (`nook_platform::snip`), the selection is matched
//! against every installed font (`nook_platform::fontmatch`), the best
//! match's name is copied, and the panel opens on a page listing the top
//! matches with the sample that was compared. Click a match to copy its
//! name. Ctrl+Alt+F starts it from anywhere.

use egui::{Align2, Color32, ColorImage, Rect, Sense, TextureHandle, Ui, pos2, vec2};
use nook_platform::clipboard;
use nook_platform::fontmatch::{self, Detection};
use nook_platform::snip::{SnipMode, SnipOutcome};
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme, with_alpha};
use nook_ui::{icons, widgets};
use std::time::Duration;

use crate::snipper::Snipper;

const HINT: &str = "Drag over a line of text to find its font  \u{00B7}  Esc to cancel";
/// Matches listed on the page.
const TOP: usize = 5;
/// How long "Copied" shows on a row.
const COPIED_FOR: f64 = 1.2;

type Outcome = Result<Detection, String>;

#[derive(Default)]
pub struct FontDetector {
    snipper: Snipper<Outcome>,
    last: Option<Detection>,
    error: Option<String>,
    texture: Option<TextureHandle>,
    show: bool,
    open_page: bool,
    /// Match index copied from the page, and when.
    copied: Option<(usize, f64)>,
}

fn detect(outcome: SnipOutcome) -> Option<Outcome> {
    let SnipOutcome::Region { frame, .. } = outcome else {
        return None;
    };
    let started = std::time::Instant::now();
    let result = fontmatch::detect(&frame, TOP).map_err(|e| e.to_string());
    match &result {
        Ok(d) => {
            tracing::info!(
                "font detector: {:?} -> {} ({:.2}) in {:?}",
                d.text,
                d.matches[0].family,
                d.matches[0].score,
                started.elapsed()
            );
            if let Err(e) = clipboard::copy_text(&d.matches[0].family) {
                tracing::warn!("font detector: copying: {e}");
            }
        }
        Err(e) => tracing::warn!("font detector: {e}"),
    }
    Some(result)
}

impl FontDetector {
    fn start(&mut self) {
        self.snipper.toggle(SnipMode::Region, HINT, detect);
    }

    fn copy(&mut self, index: usize, now: f64) {
        let Some(m) = self.last.as_ref().and_then(|d| d.matches.get(index)) else {
            return;
        };
        match clipboard::copy_text(&m.family) {
            Ok(()) => self.copied = Some((index, now)),
            Err(e) => tracing::warn!("font detector: copying: {e}"),
        }
    }

    fn sample_texture(&mut self, ctx: &egui::Context) -> Option<&TextureHandle> {
        if self.texture.is_none() {
            let s = &self.last.as_ref()?.sample;
            let rgba: Vec<u8> = s
                .pixels
                .iter()
                .flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8, 255])
                .collect();
            let image =
                ColorImage::from_rgba_unmultiplied([s.width as usize, s.height as usize], &rgba);
            self.texture =
                Some(ctx.load_texture("nook-font-sample", image, egui::TextureOptions::LINEAR));
        }
        self.texture.as_ref()
    }
}

impl Module for FontDetector {
    fn id(&self) -> &'static str {
        "font_detector"
    }

    fn name(&self) -> &str {
        "Font detector"
    }

    fn icon(&self) -> &'static str {
        icons::TEXT_T
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
        for outcome in self.snipper.poll() {
            match outcome {
                Ok(d) => {
                    self.last = Some(d);
                    self.error = None;
                    self.texture = None;
                    self.copied = None;
                    self.show = true;
                    self.snipper.flash(true);
                }
                Err(e) => {
                    self.error = Some(e);
                    self.snipper.flash(false);
                }
            }
        }
    }

    fn next_repaint(&self, _tile_visible: bool) -> Option<Duration> {
        self.snipper.next_repaint()
    }

    fn default_hotkey(&self) -> Option<&'static str> {
        Some("ctrl+alt+KeyF")
    }

    fn on_hotkey(&mut self) {
        self.start();
    }

    fn take_show_request(&mut self) -> bool {
        std::mem::take(&mut self.show)
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.open_page)
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let resp = self.snipper.tile(ui, ctx, icons::TEXT_T, "font-tile");
        if resp.clicked() {
            self.start();
        }
        // Right click: the last results.
        if resp.secondary_clicked() && self.last.is_some() {
            self.open_page = true;
        }
    }

    fn ui_page(&mut self, ui: &mut Ui, header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        let now = ui.input(|i| i.time);
        if self.copied.is_some_and(|(_, at)| now - at >= COPIED_FOR) {
            self.copied = None;
        }
        if widgets::icon_button(header, icons::SELECTION, 26.0, false, theme)
            .on_hover_text("Detect again (Ctrl+Alt+F)")
            .clicked()
        {
            self.start();
        }
        let painter = ui.painter().clone();
        let Some(d) = self.last.clone() else {
            let msg = self
                .error
                .clone()
                .unwrap_or_else(|| "Drag over some text to find its font".into());
            painter.text(
                body.center(),
                Align2::CENTER_CENTER,
                msg,
                Theme::font(SIZE_LABEL + 1.0),
                theme.text_dim,
            );
            return false;
        };

        // Left: the sample that was compared and what it says.
        let gap = 10.0;
        let left = Rect::from_min_size(body.min, vec2(body.width() * 0.4, body.height()));
        let right = Rect::from_min_max(pos2(left.max.x + gap, body.min.y), body.max);
        widgets::card(&painter, left, theme);
        let inner = left.shrink(10.0);
        if let Some(tex) = self.sample_texture(ui.ctx()) {
            let size = tex.size_vec2();
            let room = vec2(inner.width(), inner.height() - 40.0);
            let scale = (room.x / size.x).min(room.y / size.y).min(2.0);
            let img = Rect::from_center_size(
                pos2(inner.center().x, inner.min.y + room.y * 0.5),
                size * scale,
            );
            painter.image(
                tex.id(),
                img,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        let quote = if d.text.is_empty() {
            "Text not readable, matched by shape only".to_owned()
        } else {
            format!("\u{201C}{}\u{201D}", d.text)
        };
        let galley = painter.layout(
            quote,
            Theme::font(SIZE_LABEL),
            theme.text_dim,
            inner.width(),
        );
        painter.galley(
            pos2(inner.min.x, inner.max.y - galley.size().y.min(32.0)),
            galley,
            theme.text_dim,
        );
        if let Some(e) = &self.error {
            painter.text(
                pos2(inner.min.x, inner.min.y),
                Align2::LEFT_TOP,
                e,
                Theme::font(SIZE_LABEL - 1.0),
                theme.text_faint,
            );
        }

        // Right: the matches, best first. Click to copy the name.
        let row_h = (right.height() / TOP as f32).min(32.0);
        let mut copy = None;
        for (i, m) in d.matches.iter().enumerate() {
            let row = Rect::from_min_size(
                pos2(right.min.x, right.min.y + i as f32 * row_h),
                vec2(right.width(), row_h - 4.0),
            );
            let resp = ui
                .interact(row, ui.id().with(("font-row", i)), Sense::click())
                .on_hover_text("Copy name");
            let fill = if i == 0 {
                with_alpha(theme.accent, 0.22)
            } else if resp.hovered() {
                with_alpha(theme.text, 0.08)
            } else {
                Color32::TRANSPARENT
            };
            painter.rect_filled(row, 8.0, fill);
            let cy = row.center().y;
            painter.text(
                pos2(row.min.x + 10.0, cy),
                Align2::LEFT_CENTER,
                &m.family,
                Theme::font_semibold(SIZE_BODY),
                theme.text,
            );
            let style_x = row.min.x
                + 16.0
                + painter
                    .layout_no_wrap(
                        m.family.clone(),
                        Theme::font_semibold(SIZE_BODY),
                        theme.text,
                    )
                    .size()
                    .x;
            // Lens also names open-source fonts that are not on this PC.
            let note = match (m.style, m.source) {
                (_, fontmatch::Source::Lens) => Some("Not installed"),
                (fontmatch::FontStyle::Regular, _) => None,
                (style, _) => Some(style.label()),
            };
            if let Some(note) = note {
                painter.text(
                    pos2(style_x, cy),
                    Align2::LEFT_CENTER,
                    note,
                    Theme::font(SIZE_LABEL),
                    theme.text_faint,
                );
            }
            let right_text = if self.copied.is_some_and(|(c, _)| c == i) {
                "Copied".to_owned()
            } else {
                format!("{:.0}%", m.score * 100.0)
            };
            painter.text(
                pos2(row.max.x - 10.0, cy),
                Align2::RIGHT_CENTER,
                right_text,
                Theme::font(SIZE_LABEL),
                theme.text_dim,
            );
            if resp.clicked() {
                copy = Some(i);
            }
        }
        if let Some(i) = copy {
            self.copy(i, now);
        }
        false
    }
}
