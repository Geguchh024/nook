//! Screenshot: capture a region, a window, a monitor or every monitor,
//! then mark it up, copy it or save it. Like ShareX or the Snipping Tool,
//! from the notch.
//!
//! The screen freezes first (`nook_platform::snip`, screenshot mode): drag
//! a region, click the highlighted window, press F for the monitor under
//! the cursor or A for all of them. What happens next is a setting: open
//! the editor (`shot_editor`), copy straight to the clipboard, or save a
//! PNG to the Screenshots folder (and copy it). A delay of 3, 5 or 10
//! seconds lets you open a menu first.
//!
//! Click the tile (or Ctrl+Alt+S) to capture; right click it for the page
//! with those settings and the last few captures, which reopen in the
//! editor.

use crate::shot_editor::{self, Editor};
use crate::snipper::Snipper;
use egui::{
    Align2, Color32, ColorImage, Id, Rect, Sense, TextureHandle, TextureOptions, Ui, pos2, vec2,
};
use nook_platform::capture::Frame;
use nook_platform::snip::{SnipMode, SnipOutcome};
use nook_platform::{clipboard, monitor, shell};
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BIG, SIZE_LABEL, Theme, with_alpha};
use nook_ui::{anim, icons, paint, widgets};
use std::sync::Arc;
use std::time::{Duration, Instant};

const HINT: &str = "Drag a region  \u{00B7}  Click a window  \u{00B7}  F screen  \u{00B7}  A all screens  \u{00B7}  Esc to cancel";
const DELAYS: [u64; 4] = [0, 3, 5, 10];
const DELAY_LABELS: [&str; 4] = ["Off", "3 s", "5 s", "10 s"];
/// Captures kept for the page.
const RECENT: usize = 4;
/// Thumbnail edge, pixels.
const THUMB: u32 = 240;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum After {
    #[default]
    Edit,
    Copy,
    Save,
}

impl After {
    const ALL: [After; 3] = [After::Edit, After::Copy, After::Save];
    const LABELS: [&str; 3] = ["Edit", "Copy", "Save"];
}

struct Recent {
    frame: Arc<Frame>,
    thumb: Option<TextureHandle>,
}

#[derive(Default)]
pub struct Screenshot {
    snipper: Snipper<Arc<Frame>>,
    delay: usize,
    after: After,
    /// A delayed capture starts at this instant.
    pending: Option<Instant>,
    editors: Vec<Editor>,
    next_editor: u64,
    recent: Vec<Recent>,
    open_page: bool,
    repaint: Option<egui::Context>,
}

/// A small copy of `frame` (box filtered) for the page.
fn thumbnail(frame: &Frame) -> ColorImage {
    let (w, h) = (frame.width.max(1), frame.height.max(1));
    let step = w.max(h).div_ceil(THUMB).max(1);
    let (tw, th) = (w.div_ceil(step), h.div_ceil(step));
    let mut rgba = Vec::with_capacity((tw * th * 4) as usize);
    for ty in 0..th {
        for tx in 0..tw {
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in ty * step..((ty + 1) * step).min(h) {
                for x in tx * step..((tx + 1) * step).min(w) {
                    let p = frame.pixels[(y * w + x) as usize];
                    r += (p >> 16) & 0xFF;
                    g += (p >> 8) & 0xFF;
                    b += p & 0xFF;
                    n += 1;
                }
            }
            let n = n.max(1);
            rgba.extend_from_slice(&[(r / n) as u8, (g / n) as u8, (b / n) as u8, 255]);
        }
    }
    ColorImage::from_rgba_unmultiplied([tw as usize, th as usize], &rgba)
}

impl Screenshot {
    fn start(&mut self) {
        if self.pending.take().is_some() {
            // A second press cancels the countdown.
            return;
        }
        match DELAYS[self.delay] {
            0 => self.capture(),
            s => {
                self.pending = Some(Instant::now() + Duration::from_secs(s));
                if let Some(ctx) = &self.repaint {
                    ctx.request_repaint();
                }
            }
        }
    }

    fn capture(&mut self) {
        let after = self.after;
        self.snipper
            .toggle(SnipMode::Screenshot, HINT, move |outcome| {
                let SnipOutcome::Region { frame, .. } = outcome else {
                    return None;
                };
                tracing::info!("screenshot: {}x{}", frame.width, frame.height);
                match after {
                    After::Edit => {}
                    After::Copy => {
                        if let Err(e) = clipboard::copy_image(&frame) {
                            tracing::warn!("screenshot: copying: {e}");
                        }
                    }
                    After::Save => match shot_editor::save(&frame) {
                        Ok(path) => {
                            tracing::info!("screenshot: saved {}", path.display());
                            let _ = clipboard::copy_image(&frame);
                        }
                        Err(e) => tracing::warn!("screenshot: saving: {e}"),
                    },
                }
                Some(Arc::new(frame))
            });
    }

    fn open_editor(&mut self, frame: Arc<Frame>) {
        let Some(ctx) = &self.repaint else {
            return;
        };
        // Room on the monitor under the cursor, in the main window's points.
        let ppp = ctx.pixels_per_point();
        let room = monitor::under_cursor()
            .map(|m| vec2(m.work.width() as f32, m.work.height() as f32) * 0.8 / ppp)
            .unwrap_or(vec2(1200.0, 800.0));
        self.next_editor += 1;
        self.editors
            .push(Editor::new(self.next_editor, frame, room, ppp));
    }

    fn remember(&mut self, frame: Arc<Frame>) {
        self.recent.insert(0, Recent { frame, thumb: None });
        self.recent.truncate(RECENT);
    }

    fn left(&self) -> Option<Duration> {
        self.pending
            .map(|at| at.saturating_duration_since(Instant::now()))
    }
}

impl Module for Screenshot {
    fn id(&self) -> &'static str {
        "screenshot"
    }

    fn name(&self) -> &str {
        "Screenshot"
    }

    fn icon(&self) -> &'static str {
        icons::SELECTION
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn attach(&mut self, ctx: &egui::Context) {
        self.snipper.attach(ctx);
        self.repaint = Some(ctx.clone());
    }

    fn is_active(&self) -> bool {
        self.snipper.busy() || self.pending.is_some()
    }

    fn overlay_open(&self) -> bool {
        self.snipper.busy() || !self.editors.is_empty() || self.pending.is_some()
    }

    fn on_tick(&mut self) {
        if self.left() == Some(Duration::ZERO) {
            self.pending = None;
            self.capture();
        }
        for frame in self.snipper.poll() {
            self.remember(frame.clone());
            match self.after {
                After::Edit => self.open_editor(frame),
                After::Copy | After::Save => self.snipper.flash(true),
            }
        }
        self.editors.retain(Editor::is_open);
    }

    fn next_repaint(&self, _tile_visible: bool) -> Option<Duration> {
        let countdown = self
            .left()
            .map(|left| Duration::from_millis(left.as_millis() as u64 % 1000 + 5));
        match (countdown, self.snipper.next_repaint()) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn default_hotkey(&self) -> Option<&'static str> {
        Some("ctrl+alt+KeyS")
    }

    fn on_hotkey(&mut self) {
        self.start();
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.open_page)
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        if let Some(left) = self.left() {
            let rect = ui.max_rect();
            let resp = ui.interact(rect.expand(8.0), ui.id().with("shot-count"), Sense::click());
            if resp.clicked() {
                self.pending = None;
            }
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                left.as_secs() + 1,
                Theme::font_semibold(SIZE_BIG - 6.0),
                theme.accent_fg(),
            );
            resp.on_hover_text("Click to cancel");
            return;
        }
        let resp = self
            .snipper
            .tile(ui, ctx, icons::SELECTION, "shot-tile")
            .on_hover_text("Click to capture  \u{00B7}  Right click for options");
        if resp.clicked() {
            self.start();
        }
        if resp.secondary_clicked() {
            self.open_page = true;
        }
    }

    fn ui_overlay(&mut self, ctx: &egui::Context, theme: &Theme) {
        for e in &mut self.editors {
            e.show(ctx, theme);
        }
    }

    fn ui_page(&mut self, ui: &mut Ui, header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        if widgets::icon_button(header, icons::FOLDER_OPEN, 26.0, false, theme)
            .on_hover_text("Open the Screenshots folder")
            .clicked()
            && let Some(dir) = shell::screenshots_dir()
        {
            let _ = std::fs::create_dir_all(&dir);
            if let Err(e) = shell::open(&dir.to_string_lossy()) {
                tracing::warn!("open screenshots folder: {e}");
            }
        }
        if widgets::icon_button(header, icons::SELECTION, 26.0, false, theme)
            .on_hover_text("Capture (Ctrl+Alt+S)")
            .clicked()
        {
            self.start();
        }

        let label = |ui: &Ui, y: f32, text: &str| {
            ui.painter().text(
                pos2(body.min.x, y),
                Align2::LEFT_CENTER,
                text,
                Theme::font(SIZE_LABEL + 1.0),
                theme.text_dim,
            );
        };
        let row =
            |y: f32| Rect::from_min_size(pos2(body.min.x + 110.0, y - 12.0), vec2(300.0, 24.0));
        let y0 = body.min.y + 14.0;
        label(ui, y0, "Delay");
        let mut rui = ui.new_child(egui::UiBuilder::new().max_rect(row(y0)));
        widgets::segmented(&mut rui, &DELAY_LABELS, &mut self.delay, theme);
        let y1 = y0 + 34.0;
        label(ui, y1, "After capture");
        let mut after = After::ALL
            .iter()
            .position(|a| *a == self.after)
            .unwrap_or(0);
        let mut rui = ui.new_child(egui::UiBuilder::new().max_rect(row(y1)));
        if widgets::segmented(&mut rui, &After::LABELS, &mut after, theme).changed() {
            self.after = After::ALL[after];
        }
        ui.painter().text(
            pos2(body.max.x, y1),
            Align2::RIGHT_CENTER,
            match self.after {
                After::Edit => "Mark up, then copy or save",
                After::Copy => "Straight to the clipboard",
                After::Save => "PNG in Screenshots, and copied",
            },
            Theme::font(SIZE_LABEL),
            theme.text_faint,
        );

        // Recent captures.
        let top = y1 + 26.0;
        let area = Rect::from_min_max(pos2(body.min.x, top), body.max);
        if self.recent.is_empty() {
            ui.painter().text(
                area.center(),
                Align2::CENTER_CENTER,
                "Captures show up here.",
                Theme::font(SIZE_LABEL + 1.0),
                theme.text_faint,
            );
            return false;
        }
        let gap = 8.0;
        let card_w = (area.width() - gap * (RECENT as f32 - 1.0)) / RECENT as f32;
        let ctx = ui.ctx().clone();
        let mut open = None;
        for (i, r) in self.recent.iter_mut().enumerate() {
            let card = Rect::from_min_size(
                pos2(area.min.x + i as f32 * (card_w + gap), area.min.y),
                vec2(card_w, area.height()),
            );
            widgets::card(ui.painter(), card, theme);
            let id = Id::new(("shot-recent", i));
            let resp = ui.interact(card, id, Sense::click());
            let hover = anim::tween(&ctx, id.with("hover"), resp.hovered(), 0.12);
            let tex = r.thumb.get_or_insert_with(|| {
                ctx.load_texture(
                    format!("nook-shot-thumb-{i}-{:p}", Arc::as_ptr(&r.frame)),
                    thumbnail(&r.frame),
                    TextureOptions::LINEAR,
                )
            });
            let inner = card.shrink(6.0);
            let size = tex.size_vec2();
            let s = (inner.width() / size.x).min(inner.height() / size.y);
            let img = Rect::from_center_size(inner.center(), size * s);
            ui.painter().image(
                tex.id(),
                img,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
            if hover > 0.0 {
                ui.painter().rect_filled(
                    img,
                    4.0,
                    with_alpha(Color32::from_black_alpha(110), hover),
                );
                paint::icon(
                    ui.painter(),
                    img.center(),
                    icons::PENCIL_SIMPLE,
                    20.0,
                    with_alpha(Color32::WHITE, hover),
                    false,
                );
            }
            if resp.on_hover_text("Open in the editor").clicked() {
                open = Some(r.frame.clone());
            }
        }
        if let Some(f) = open {
            self.open_editor(f);
        }
        false
    }

    fn shutdown(&mut self) {
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumbnails_box_filter_and_fit() {
        let f = Frame {
            origin: (0, 0),
            width: 1000,
            height: 500,
            pixels: vec![0xFF20_4060; 500_000],
        };
        let t = thumbnail(&f);
        assert!(t.size[0] <= THUMB as usize && t.size[1] <= THUMB as usize);
        assert_eq!(t.size, [200, 100]);
        assert_eq!(t.pixels[0], Color32::from_rgb(0x20, 0x40, 0x60));
    }
}
