//! Clipboard history: text and images you copy, newest first. The tile
//! shows the latest three; the expand button opens the whole history as a
//! full-panel page of cards. Click an entry to copy it again.
//!
//! History stays in memory only and skips content apps mark as private
//! (see `nook_platform::clipboard`).

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align2, Color32, ColorImage, Id, Pos2, Rect, Response, ScrollArea, Sense, TextureHandle,
    TextureOptions, Ui, UiBuilder, pos2, vec2,
};
use nook_platform::clipboard::{ClipContent, ClipImage, ClipboardWatcher};
use nook_ui::bento::tile_header;
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_LABEL, TILE_RADIUS, Theme, with_alpha};
use nook_ui::{anim, icons, paint, widgets};
use std::time::{Duration, Instant};

/// Entries kept.
const MAX_ENTRIES: usize = 40;
/// Images kept (each holds its full bitmap for pasting back).
const MAX_IMAGES: usize = 8;
/// How long the "Copied" confirmation shows.
const COPIED_FOR: f64 = 1.2;

struct Entry {
    id: u64,
    content: ClipContent,
    at: Instant,
    texture: Option<TextureHandle>,
}

impl Entry {
    fn texture(&mut self, ctx: &egui::Context) -> Option<(&TextureHandle, &ClipImage)> {
        let ClipContent::Image(img) = &self.content else {
            return None;
        };
        if self.texture.is_none() {
            let image = ColorImage::from_rgba_unmultiplied(img.thumb_size, &img.thumb);
            self.texture = Some(ctx.load_texture(
                format!("nook-clip-{}", self.id),
                image,
                TextureOptions::LINEAR,
            ));
        }
        self.texture.as_ref().map(|t| (t, img.as_ref()))
    }
}

/// Newest-first history with de-duplication and caps.
#[derive(Default)]
struct History {
    entries: Vec<Entry>,
    next_id: u64,
}

impl History {
    /// Adds `content` on top; an identical entry moves up instead.
    fn push(&mut self, content: ClipContent) {
        if let Some(i) = self.entries.iter().position(|e| e.content == content) {
            let mut e = self.entries.remove(i);
            e.at = Instant::now();
            self.entries.insert(0, e);
            return;
        }
        self.next_id += 1;
        self.entries.insert(
            0,
            Entry {
                id: self.next_id,
                content,
                at: Instant::now(),
                texture: None,
            },
        );
        let mut images = 0;
        self.entries.retain(|e| {
            if matches!(e.content, ClipContent::Image(_)) {
                images += 1;
                images <= MAX_IMAGES
            } else {
                true
            }
        });
        self.entries.truncate(MAX_ENTRIES);
    }

    fn remove(&mut self, id: u64) {
        self.entries.retain(|e| e.id != id);
    }
}

#[derive(Default)]
pub struct Clipboard {
    watcher: Option<ClipboardWatcher>,
    history: History,
    /// Entry last copied back, and when (egui time).
    copied: Option<(u64, f64)>,
    open_page: bool,
}

impl Clipboard {
    fn copy(&mut self, id: u64, now: f64) {
        let Some(e) = self.history.entries.iter().find(|e| e.id == id) else {
            return;
        };
        if let Some(w) = &self.watcher {
            w.set(e.content.clone());
        }
        self.copied = Some((id, now));
    }

    fn just_copied(&self, id: u64, now: f64) -> bool {
        self.copied
            .is_some_and(|(c, at)| c == id && now - at < COPIED_FOR)
    }
}

/// "now", "5 min", "2 h", "3 d".
fn age(at: Instant) -> String {
    let s = at.elapsed().as_secs();
    match s {
        0..60 => "now".into(),
        60..3600 => format!("{} min", s / 60),
        3600..86400 => format!("{} h", s / 3600),
        _ => format!("{} d", s / 86400),
    }
}

/// Whitespace collapsed to single spaces, for one-line previews.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn text_block(
    ui: &Ui,
    pos: Pos2,
    text: &str,
    width: f32,
    rows: usize,
    font: egui::FontId,
    color: Color32,
) {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, color));
    job.wrap = TextWrapping {
        max_width: width,
        max_rows: rows,
        break_anywhere: rows == 1,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(pos, galley, color);
}

/// The image fitted (aspect kept) and centered in `area`.
fn fit(area: Rect, size: [usize; 2]) -> Rect {
    let (w, h) = (size[0].max(1) as f32, size[1].max(1) as f32);
    let scale = (area.width() / w).min(area.height() / h);
    Rect::from_center_size(area.center(), vec2(w * scale, h * scale))
}

fn image(ui: &Ui, rect: Rect, tex: &TextureHandle, radius: f32) {
    ui.painter().add(
        egui::epaint::RectShape::filled(rect, radius, Color32::WHITE)
            .with_texture(tex.id(), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
    );
}

/// Hover wash + click for a row or card.
fn hover_box(ui: &mut Ui, rect: Rect, id: Id, radius: f32, theme: &Theme) -> Response {
    let resp = ui.interact(rect, id, Sense::click());
    let t = anim::tween(ui.ctx(), id.with("hover"), resp.hovered(), 0.12);
    if t > 0.0 {
        let wash = if theme.dark {
            Color32::from_white_alpha(18)
        } else {
            Color32::from_black_alpha(12)
        };
        ui.painter().rect_filled(rect, radius, with_alpha(wash, t));
    }
    resp
}

fn button_at(ui: &mut Ui, center: Pos2, size: f32, glyph: &'static str, theme: &Theme) -> Response {
    let rect = Rect::from_center_size(center, vec2(size, size));
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(glyph));
    widgets::icon_button(&mut child, glyph, size, false, theme)
}

impl Module for Clipboard {
    fn id(&self) -> &'static str {
        "clipboard"
    }

    fn name(&self) -> &str {
        "Clipboard"
    }

    fn icon(&self) -> &'static str {
        icons::CLIPBOARD_TEXT
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Large
    }

    fn attach(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        match ClipboardWatcher::spawn(move || ctx.request_repaint()) {
            Ok(w) => self.watcher = Some(w),
            Err(e) => tracing::warn!("clipboard history unavailable: {e}"),
        }
    }

    fn on_tick(&mut self) {
        let Some(w) = &self.watcher else {
            return;
        };
        for content in w.poll() {
            self.history.push(content);
        }
    }

    fn next_repaint(&self, tile_visible: bool) -> Option<Duration> {
        // Let the "Copied" confirmation expire on time.
        (tile_visible && self.copied.is_some()).then(|| Duration::from_millis(250))
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.open_page)
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        let now = ctx.now;
        let rect = ui.max_rect();
        tile_header(ui, icons::CLIPBOARD_TEXT, "Clipboard", false, theme);
        if self.copied.is_some_and(|(_, at)| now - at >= COPIED_FOR) {
            self.copied = None;
        }

        if button_at(
            ui,
            pos2(rect.max.x - 8.0, rect.min.y + 9.0),
            20.0,
            icons::ARROWS_OUT_SIMPLE,
            theme,
        )
        .on_hover_text("Show all")
        .clicked()
        {
            self.open_page = true;
        }

        if self.history.entries.is_empty() {
            ui.painter().text(
                rect.center() + vec2(0.0, 10.0),
                Align2::CENTER_CENTER,
                "Copy something",
                Theme::font(SIZE_LABEL),
                theme.text_faint,
            );
            return;
        }

        // Latest three entries as rows.
        let row_h = 24.0;
        let top = rect.min.y + 26.0;
        let mut clicked = None;
        let ctx_egui = ui.ctx().clone();
        let rows: Vec<u64> = self.history.entries.iter().take(3).map(|e| e.id).collect();
        for (i, id) in rows.into_iter().enumerate() {
            let row = Rect::from_min_size(
                pos2(rect.min.x - 4.0, top + i as f32 * (row_h + 2.0)),
                vec2(rect.width() + 8.0, row_h),
            );
            let resp = hover_box(ui, row, ui.id().with(("clip-row", id)), 8.0, theme);
            if resp.clicked() {
                clicked = Some(id);
            }
            let copied = self.just_copied(id, now);
            let Some(entry) = self.history.entries.iter_mut().find(|e| e.id == id) else {
                continue;
            };
            let text_x = row.min.x + 6.0;
            if copied {
                paint::icon(
                    ui.painter(),
                    pos2(text_x + 6.0, row.center().y),
                    icons::CHECK,
                    13.0,
                    theme.accent_fg(),
                    true,
                );
                ui.painter().text(
                    pos2(text_x + 18.0, row.center().y),
                    Align2::LEFT_CENTER,
                    "Copied",
                    Theme::font(SIZE_LABEL),
                    theme.accent_fg(),
                );
                continue;
            }
            match entry.content.clone() {
                ClipContent::Text(t) => text_block(
                    ui,
                    pos2(text_x, row.center().y - 7.0),
                    &one_line(&t),
                    row.width() - 12.0,
                    1,
                    Theme::font(SIZE_LABEL),
                    theme.text,
                ),
                ClipContent::Image(_) => {
                    if let Some((tex, img)) = entry.texture(&ctx_egui) {
                        let thumb = Rect::from_min_size(
                            pos2(text_x, row.min.y + 3.0),
                            vec2(row_h - 6.0, row_h - 6.0),
                        );
                        image(ui, fit(thumb, img.thumb_size), tex, 3.0);
                        ui.painter().text(
                            pos2(thumb.max.x + 6.0, row.center().y),
                            Align2::LEFT_CENTER,
                            format!("{}×{}", img.width, img.height),
                            Theme::font(SIZE_LABEL),
                            theme.text_dim,
                        );
                    }
                }
            }
        }
        if let Some(id) = clicked {
            self.copy(id, now);
        }
    }

    fn ui_page(&mut self, ui: &mut Ui, header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        let now = ui.input(|i| i.time);
        if !self.history.entries.is_empty()
            && widgets::icon_button(header, icons::TRASH, 26.0, false, theme)
                .on_hover_text("Clear history")
                .clicked()
        {
            self.history.entries.clear();
        }

        if self.history.entries.is_empty() {
            ui.painter().text(
                body.center(),
                Align2::CENTER_CENTER,
                "Nothing copied yet. Text and images you copy show up here.",
                Theme::font(SIZE_LABEL + 1.0),
                theme.text_dim,
            );
            return false;
        }

        const COLS: usize = 4;
        const GAP: f32 = 8.0;
        let card_w = (body.width() - GAP * (COLS - 1) as f32) / COLS as f32;
        let card_h = 76.0;
        let ctx = ui.ctx().clone();
        let mut copy = None;
        let mut delete = None;
        let ids: Vec<u64> = self.history.entries.iter().map(|e| e.id).collect();
        let rows = ids.len().div_ceil(COLS);

        ScrollArea::vertical()
            .id_salt("nook-clip-scroll")
            .max_height(body.height())
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let (area, _) = ui.allocate_exact_size(
                    vec2(body.width(), rows as f32 * (card_h + GAP) - GAP),
                    Sense::hover(),
                );
                for (i, id) in ids.iter().copied().enumerate() {
                    let (r, c) = (i / COLS, i % COLS);
                    let card = Rect::from_min_size(
                        pos2(
                            area.min.x + c as f32 * (card_w + GAP),
                            area.min.y + r as f32 * (card_h + GAP),
                        ),
                        vec2(card_w, card_h),
                    );
                    if !ui.is_rect_visible(card) {
                        continue;
                    }
                    widgets::card(ui.painter(), card, theme);
                    let resp = hover_box(ui, card, Id::new(("clip-card", id)), TILE_RADIUS, theme);
                    if resp.clicked() {
                        copy = Some(id);
                    }
                    let copied = self.just_copied(id, now);
                    let Some(entry) = self.history.entries.iter_mut().find(|e| e.id == id) else {
                        continue;
                    };
                    let inner = card.shrink(10.0);
                    let footer_y = inner.max.y - 5.0;
                    let label = match entry.content.clone() {
                        ClipContent::Text(t) => {
                            text_block(
                                ui,
                                inner.min,
                                t.trim(),
                                inner.width(),
                                3,
                                Theme::font(SIZE_LABEL),
                                theme.text,
                            );
                            format!("{} chars", t.chars().count())
                        }
                        ClipContent::Image(_) => match entry.texture(&ctx) {
                            Some((tex, img)) => {
                                let area = Rect::from_min_max(
                                    inner.min,
                                    pos2(inner.max.x, footer_y - 9.0),
                                );
                                image(ui, fit(area, img.thumb_size), tex, 4.0);
                                format!("{}×{}", img.width, img.height)
                            }
                            None => "Image".into(),
                        },
                    };
                    let (footer, color) = if copied {
                        ("Copied".to_owned(), theme.accent_fg())
                    } else {
                        (format!("{label} · {}", age(entry.at)), theme.text_faint)
                    };
                    ui.painter().text(
                        pos2(inner.min.x, footer_y),
                        Align2::LEFT_CENTER,
                        footer,
                        Theme::font(SIZE_LABEL - 1.0),
                        color,
                    );
                    if resp.hovered()
                        && button_at(
                            ui,
                            pos2(card.max.x - 13.0, card.max.y - 13.0),
                            20.0,
                            icons::X,
                            theme,
                        )
                        .on_hover_text("Remove")
                        .clicked()
                    {
                        delete = Some(id);
                        copy = None;
                    }
                }
            });

        if let Some(id) = delete {
            self.history.remove(id);
        } else if let Some(id) = copy {
            self.copy(id, now);
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(s: &str) -> ClipContent {
        ClipContent::Text(s.into())
    }

    fn texts(h: &History) -> Vec<String> {
        h.entries
            .iter()
            .filter_map(|e| match &e.content {
                ClipContent::Text(t) => Some(t.clone()),
                ClipContent::Image(_) => None,
            })
            .collect()
    }

    #[test]
    fn newest_first_and_dedup_moves_up() {
        let mut h = History::default();
        h.push(text("a"));
        h.push(text("b"));
        h.push(text("a"));
        assert_eq!(texts(&h), ["a", "b"]);
    }

    #[test]
    fn capped() {
        let mut h = History::default();
        for i in 0..MAX_ENTRIES + 5 {
            h.push(text(&i.to_string()));
        }
        assert_eq!(h.entries.len(), MAX_ENTRIES);
        assert_eq!(texts(&h)[0], (MAX_ENTRIES + 4).to_string());
    }

    #[test]
    fn one_line_previews() {
        assert_eq!(one_line("  fn main() {\n    x\n}"), "fn main() { x }");
    }
}
