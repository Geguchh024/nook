//! Debug overlay (feature `debug-overlay`): CPU time of the last frame, the
//! interval since the previous frame, a frame counter and why egui
//! repainted. It never requests repaints itself, so an idle app shows a
//! frozen counter.

use egui::{Align2, Color32, Rect, Ui, pos2, vec2};
use nook_ui::theme::{SIZE_LABEL, Theme};
use std::time::Instant;

#[derive(Default)]
pub struct DebugOverlay {
    frame_start: Option<Instant>,
    last_start: Option<Instant>,
    last_cpu_ms: f32,
    interval_ms: f32,
    frames: u64,
}

impl DebugOverlay {
    pub fn begin(&mut self) {
        let now = Instant::now();
        if let Some(prev) = self.last_start {
            self.interval_ms = (now - prev).as_secs_f32() * 1000.0;
        }
        self.last_start = Some(now);
        self.frame_start = Some(now);
        self.frames += 1;
    }

    /// Draws the overlay at the bottom-left of the window; returns its rect
    /// so the app can add it to the window region.
    pub fn show(&mut self, ui: &mut Ui, theme: &Theme) -> Rect {
        let ctx = ui.ctx().clone();
        let window = ctx.content_rect();
        let rect = Rect::from_min_size(
            pos2(window.min.x + 8.0, window.max.y - 30.0),
            vec2(300.0, 22.0),
        );
        let causes = ctx.repaint_causes();
        let cause = causes
            .first()
            .map(|c| {
                let file = c.file.rsplit(['/', '\\']).next().unwrap_or(c.file);
                format!("{file}:{} {}", c.line, c.reason)
            })
            .unwrap_or_else(|| "input".into());
        let text = format!(
            "#{}  cpu {:.1} ms  Δ {:.0} ms  {}",
            self.frames, self.last_cpu_ms, self.interval_ms, cause
        );
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("nook-debug"),
        ));
        painter.rect_filled(rect, 6.0, Color32::from_black_alpha(200));
        painter.text(
            pos2(rect.min.x + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            text,
            Theme::font(SIZE_LABEL),
            theme::overlay_text(theme),
        );
        if let Some(start) = self.frame_start.take() {
            self.last_cpu_ms = start.elapsed().as_secs_f32() * 1000.0;
        }
        rect
    }
}

mod theme {
    use egui::Color32;
    use nook_ui::theme::Theme;

    pub fn overlay_text(_theme: &Theme) -> Color32 {
        Color32::from_rgb(160, 255, 170)
    }
}
