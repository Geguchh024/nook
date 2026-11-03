//! Clipboard history: text and images you copy, newest first. The tile
//! shows the latest three; the expand button opens the whole history as a
//! full-panel page of cards. Click an entry to copy it again.
//!
//! History stays in memory only and skips content apps mark as private
//! (see `nook_platform::clipboard`).

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align2, Color32, ColorImage, Id, Pos2, Rect, Response, ScrollArea, Sense, TextureHandle,
    TextureOptions, Ui, pos2, vec2,
};
use nook_platform::clipboard::{self, ClipContent, ClipImage, ClipboardWatcher};
use nook_ui::bento::tile_header;
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_LABEL, TILE_RADIUS, Theme, parse_hex, with_alpha};
use nook_ui::{anim, icons, paint, widgets};
use std::time::{Duration, Instant};

/// Entries kept.
const MAX_ENTRIES: usize = 40;
/// Images kept (each holds its full bitmap for pasting back).
const MAX_IMAGES: usize = 8;
/// How long the "Copied" confirmation shows.
const COPIED_FOR: f64 = 1.2;

fn clipboard_color(text: &str) -> Option<Color32> {
    let value = text.trim();
    (value.starts_with('#') && matches!(value.len(), 4 | 7 | 9))
        .then(|| parse_hex(value))
        .flatten()
}

fn entry_color(entries: &[Entry], id: u64) -> Option<Color32> {
    entries
        .iter()
        .find(|entry| entry.id == id)
        .and_then(|entry| {
            if let ClipContent::Text(text) = &entry.content {
                clipboard_color(text)
            } else {
                None
            }
        })
}

struct Entry {
    id: u64,
    content: ClipContent,
    at: Instant,
    day: i64,
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
            let local = nook_platform::system::local_time();
            e.day = crate::calendar::days_from_civil(local.year, local.month, local.day);
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
                day: {
                    let local = nook_platform::system::local_time();
                    crate::calendar::days_from_civil(local.year, local.month, local.day)
                },
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

    fn expire(&mut self, max_age: Duration) {
        self.entries.retain(|entry| entry.at.elapsed() < max_age);
    }
}

#[derive(Default)]
pub struct Clipboard {
    watcher: Option<ClipboardWatcher>,
    history: History,
    /// Entry last copied back, and when (egui time).
    copied: Option<(u64, f64)>,
    open_page: bool,
    editing: Option<(u64, String, bool)>,
    edit_session: u64,
    edit_focus_pending: bool,
    retention_days: Option<u32>,
    message: Option<String>,
    filter: crate::kit::HistoryFilter,
}

impl Clipboard {
    fn copy(&mut self, id: u64, now: f64) {
        let Some(content) = self
            .history
            .entries
            .iter()
            .find(|e| e.id == id)
            .map(|entry| entry.content.clone())
        else {
            return;
        };
        let result = if let Some(watcher) = &self.watcher {
            watcher.set(content.clone())
        } else {
            clipboard::copy_content(&content)
        };
        match result {
            Ok(()) => {
                self.copied = Some((id, now));
                self.message = None;
            }
            Err(error) => self.message = Some(format!("Could not copy: {error}")),
        }
    }

    fn just_copied(&self, id: u64, now: f64) -> bool {
        self.copied
            .is_some_and(|(c, at)| c == id && now - at < COPIED_FOR)
    }
}

fn retention_left(at: Instant, days: Option<u32>) -> Option<String> {
    let duration = Duration::from_secs(u64::from(days?) * 86_400);
    let remaining = duration.saturating_sub(at.elapsed());
    let days = remaining.as_secs().div_ceil(86_400).max(1);
    Some(format!(
        "{days} {} left",
        if days == 1 { "day" } else { "days" }
    ))
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

fn button_at(
    ui: &mut Ui,
    center: Pos2,
    size: f32,
    glyph: &'static str,
    id: Id,
    visible: bool,
    theme: &Theme,
) -> Response {
    let rect = Rect::from_center_size(center, vec2(size, size));
    let response = ui.interact(rect, id, Sense::click());
    if visible || response.hovered() {
        let painter = ui.painter();
        let fill = if theme.dark {
            Color32::from_white_alpha(28)
        } else {
            Color32::from_black_alpha(20)
        };
        if response.hovered() {
            painter.circle_filled(rect.center(), size * 0.5, fill);
        }
        paint::icon(
            painter,
            rect.center(),
            glyph,
            size * 0.55,
            if response.hovered() {
                theme.accent
            } else {
                theme.text_dim
            },
            false,
        );
    }
    if response.clicked() {
        nook_ui::sound::play(ui.ctx(), nook_ui::sound::Cue::Tap);
    }
    response
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

    fn collapsed_badge(&self) -> Option<nook_ui::module::Badge> {
        let (id, copied_at) = self.copied?;
        let entry = self.history.entries.iter().find(|entry| entry.id == id)?;
        let label = match &entry.content {
            ClipContent::Image(_) => "Copied image".to_owned(),
            ClipContent::Text(text) => {
                let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
                let preview: String = compact.chars().take(28).collect();
                let suffix = if compact.chars().count() > 28 {
                    "…"
                } else {
                    ""
                };
                if preview.is_empty() {
                    "Copied text".to_owned()
                } else {
                    format!("Copied: {preview}{suffix}")
                }
            }
        };
        Some(nook_ui::module::Badge::Notice {
            label,
            until: copied_at + COPIED_FOR,
        })
    }

    fn page_size(&self) -> Option<egui::Vec2> {
        let picker = self.filter.picker_height();
        (picker > 0.0).then_some(vec2(760.0, 430.0 + picker + 14.0))
    }

    fn attach(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        match ClipboardWatcher::spawn(move || ctx.request_repaint()) {
            Ok(w) => self.watcher = Some(w),
            Err(e) => tracing::warn!("clipboard history unavailable: {e}"),
        }
    }

    fn set_retention_days(&mut self, days: Option<u32>) {
        self.retention_days = days;
    }

    fn on_tick(&mut self) {
        if let Some(watcher) = &self.watcher {
            for content in watcher.poll() {
                self.history.push(content);
            }
        }
        if let Some(days) = self.retention_days {
            let max_age = Duration::from_secs(u64::from(days) * 86_400);
            let before = self.history.entries.len();
            self.history.expire(max_age);
            if before != self.history.entries.len()
                && let Some((id, _, _)) = self.editing.as_ref()
                && !self.history.entries.iter().any(|entry| entry.id == *id)
            {
                self.editing = None;
                self.edit_focus_pending = false;
            }
        }
    }

    fn next_repaint(&self, tile_visible: bool) -> Option<Duration> {
        // Let the "Copied" confirmation expire on time.
        let copy_feedback =
            (tile_visible && self.copied.is_some()).then_some(Duration::from_millis(250));
        let expiry = self.retention_days.and_then(|days| {
            let max_age = Duration::from_secs(u64::from(days) * 86_400);
            self.history
                .entries
                .iter()
                .map(|entry| max_age.saturating_sub(entry.at.elapsed()))
                .min()
        });
        match (copy_feedback, expiry) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(value), None) | (None, Some(value)) => Some(value),
            (None, None) => None,
        }
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
            ui.id().with("clip-show-all"),
            true,
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
            if let ClipContent::Text(text) = &entry.content
                && let Some(color) = clipboard_color(text)
            {
                ui.painter().rect_filled(
                    row,
                    8.0,
                    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 112),
                );
            }
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

        if let Some(message) = &self.message {
            ui.painter().text(
                pos2(body.min.x + 4.0, body.max.y - 4.0),
                Align2::LEFT_BOTTOM,
                message,
                Theme::font(SIZE_LABEL - 1.0),
                theme.text_dim,
            );
        }

        let filter_area =
            Rect::from_min_size(body.min, vec2(body.width(), body.height().min(260.0)));
        let mut filter_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(filter_area)
                .id_salt("clipboard-filter"),
        );
        let filter_height = crate::kit::history_filter(&mut filter_ui, &mut self.filter, theme);
        let content_top = (body.min.y + filter_height + 6.0).min(body.max.y);
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

        const GAP: f32 = 8.0;
        const CARD_H: f32 = 76.0;
        let ctx = ui.ctx().clone();
        let mut copy = None;
        let mut delete = None;
        let mut edit = None;
        let ids: Vec<u64> = self
            .history
            .entries
            .iter()
            .filter(|entry| {
                let text = match &entry.content {
                    ClipContent::Text(text) => text.as_str(),
                    ClipContent::Image(_) => "",
                };
                self.filter.matches(text, entry.day)
            })
            .map(|e| e.id)
            .collect();

        if ids.is_empty() {
            ui.painter().text(
                pos2(body.center().x, content_top + 44.0),
                Align2::CENTER_CENTER,
                "No clipboard items match this search or date.",
                Theme::font(SIZE_LABEL),
                theme.text_dim,
            );
            return false;
        }

        let list_area = Rect::from_min_max(pos2(body.min.x, content_top), body.max);
        let mut list_ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(list_area)
                .id_salt("clipboard-results"),
        );
        ScrollArea::vertical()
            .id_salt("nook-clip-scroll")
            .max_height((body.height() - (content_top - body.min.y) - 16.0).max(80.0))
            .auto_shrink([false, false])
            .show(&mut list_ui, |ui| {
                let (area, _) = ui.allocate_exact_size(
                    vec2(body.width(), ids.len() as f32 * (CARD_H + GAP) - GAP),
                    Sense::hover(),
                );
                for (i, id) in ids.iter().copied().enumerate() {
                    let card = Rect::from_min_size(
                        pos2(area.min.x, area.min.y + i as f32 * (CARD_H + GAP)),
                        vec2(area.width(), CARD_H),
                    );
                    if !ui.is_rect_visible(card) {
                        continue;
                    }
                    widgets::card(ui.painter(), card, theme);
                    if let Some(color) = entry_color(&self.history.entries, id) {
                        ui.painter().rect_filled(
                            card,
                            TILE_RADIUS,
                            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), 112),
                        );
                    }
                    let resp = hover_box(ui, card, Id::new(("clip-card", id)), TILE_RADIUS, theme);
                    let copied = self.just_copied(id, now);
                    let Some(entry) = self.history.entries.iter_mut().find(|e| e.id == id) else {
                        continue;
                    };
                    let actions_w = 92.0;
                    let inner = card.shrink2(vec2(12.0, 8.0));
                    let content = Rect::from_min_max(
                        inner.min,
                        pos2((inner.max.x - actions_w).max(inner.min.x), inner.max.y),
                    );
                    let action_y = inner.center().y;
                    let label = match entry.content.clone() {
                        ClipContent::Text(t) => {
                            text_block(
                                ui,
                                pos2(content.min.x, content.min.y + 1.0),
                                t.trim(),
                                content.width(),
                                2,
                                Theme::font(SIZE_LABEL),
                                theme.text,
                            );
                            format!("{} chars", t.chars().count())
                        }
                        ClipContent::Image(_) => match entry.texture(&ctx) {
                            Some((tex, img)) => {
                                let area = Rect::from_min_max(
                                    content.min,
                                    pos2(content.max.x, content.max.y),
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
                        let remaining = retention_left(entry.at, self.retention_days)
                            .map(|text| format!(" · {text}"))
                            .unwrap_or_default();
                        (
                            format!("{label} · {}{remaining}", age(entry.at)),
                            theme.text_faint,
                        )
                    };
                    ui.painter().text(
                        pos2(content.min.x, content.max.y - 1.0),
                        Align2::LEFT_CENTER,
                        footer,
                        Theme::font(SIZE_LABEL - 1.0),
                        color,
                    );
                    let copy_resp = button_at(
                        ui,
                        pos2(inner.max.x - 72.0, action_y),
                        24.0,
                        icons::COPY_SIMPLE,
                        Id::new(("clip-copy", id)),
                        resp.hovered(),
                        theme,
                    )
                    .on_hover_text("Copy")
                    .clicked();
                    let mut action_clicked = copy_resp;
                    if copy_resp {
                        copy = Some(id);
                    }
                    if let ClipContent::Text(text) = &entry.content {
                        let long = text.chars().count() > 180;
                        let edit_resp = button_at(
                            ui,
                            pos2(inner.max.x - 44.0, action_y),
                            24.0,
                            if long {
                                icons::ARROWS_OUT_SIMPLE
                            } else {
                                icons::PENCIL_SIMPLE
                            },
                            Id::new(("clip-edit", id)),
                            resp.hovered(),
                            theme,
                        )
                        .on_hover_text(if long {
                            "Enlarge and edit text"
                        } else {
                            "Edit text"
                        })
                        .clicked();
                        if edit_resp {
                            action_clicked = true;
                            edit = Some((id, text.clone(), long));
                        }
                    }
                    let delete_resp = button_at(
                        ui,
                        pos2(inner.max.x - 14.0, action_y),
                        24.0,
                        icons::TRASH,
                        Id::new(("clip-delete", id)),
                        resp.hovered(),
                        theme,
                    )
                    .on_hover_text("Delete")
                    .clicked();
                    if delete_resp {
                        action_clicked = true;
                        delete = Some(id);
                        copy = None;
                        edit = None;
                    }
                    if resp.clicked() && !action_clicked {
                        copy = Some(id);
                    }
                }
            });

        if let Some((id, text, enlarged)) = edit {
            self.editing = Some((id, text, enlarged));
            self.edit_session = self.edit_session.wrapping_add(1);
            self.edit_focus_pending = true;
        }
        let mut save_edit = None;
        let mut cancel_edit = false;
        let mut editor_response = None;
        if let Some((id, text, enlarged)) = &mut self.editing {
            egui::Window::new("Edit clipboard text")
                .id(Id::new("nook-clipboard-edit").with(self.edit_session))
                .collapsible(false)
                .resizable(true)
                .default_width(if *enlarged { 640.0 } else { 440.0 })
                .default_height(if *enlarged { 500.0 } else { 340.0 })
                .show(ui.ctx(), |ui| {
                    let editor_size = vec2(
                        ui.available_width(),
                        (ui.available_height() - 38.0).max(120.0),
                    );
                    editor_response = Some(
                        ui.add_sized(
                            editor_size,
                            egui::TextEdit::multiline(text)
                                .desired_width(ui.available_width())
                                .desired_rows(if *enlarged { 20 } else { 8 })
                                .cursor_at_end(true),
                        ),
                    );
                    ui.horizontal(|ui| {
                        if widgets::icon_button(ui, icons::CHECK, 26.0, true, theme)
                            .on_hover_text("Save and copy")
                            .clicked()
                        {
                            save_edit = Some((*id, text.clone()));
                        }
                        ui.label(egui::RichText::new("Save and copy").color(theme.text_dim));
                        if ui.button("Cancel").clicked() {
                            cancel_edit = true;
                        }
                    });
                });
        }
        if self.edit_focus_pending
            && let Some(response) = editor_response
        {
            response.request_focus();
            self.edit_focus_pending = false;
        }
        if let Some((id, text)) = save_edit {
            if let Some(entry) = self.history.entries.iter_mut().find(|entry| entry.id == id) {
                entry.content = ClipContent::Text(text.clone());
                entry.at = Instant::now();
                let local = nook_platform::system::local_time();
                entry.day = crate::calendar::days_from_civil(local.year, local.month, local.day);
                let content = ClipContent::Text(text);
                let result = if let Some(watcher) = &self.watcher {
                    watcher.set(content.clone())
                } else {
                    clipboard::copy_content(&content)
                };
                match result {
                    Ok(()) => {
                        self.copied = Some((id, now));
                        self.message = None;
                    }
                    Err(error) => self.message = Some(format!("Could not copy: {error}")),
                }
            }
            self.editing = None;
            self.edit_focus_pending = false;
        } else if cancel_edit {
            self.editing = None;
            self.edit_focus_pending = false;
        }

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

    #[test]
    fn clipboard_hex_colors_are_recognized_without_matching_other_text() {
        assert_eq!(
            clipboard_color("#ff89fa"),
            Some(Color32::from_rgb(255, 137, 250))
        );
        assert!(clipboard_color("notes #ff89fa").is_none());
        assert!(clipboard_color("#nothex").is_none());
    }

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
