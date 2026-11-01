//! Notes: quick plain-text notes that live in the notch.
//!
//! The tile shows the latest note; clicking it opens the notes page with
//! the list on the left and the editor on the right, ready to type. The
//! panel stays open while you type (the shell holds it open while a text
//! field has focus) and folds away once you click elsewhere.
//!
//! Each note is a Markdown file in `%APPDATA%\Nook\notes`, so they are easy
//! to back up, sync or open in another editor. Changes save on their own a
//! moment after you stop typing, and on exit. A note that is still empty
//! is never written; deleting one moves its file to the Recycle Bin.

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, Color32, Id, Rect, ScrollArea, Sense, TextEdit, Ui, UiBuilder, pos2, vec2};
use nook_platform::shell;
use nook_ui::bento::tile_header;
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme, with_alpha};
use nook_ui::{anim, icons, widgets};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Quiet time after the last keystroke before a note is written.
const SAVE_AFTER: Duration = Duration::from_millis(600);
/// Width of the note list on the page.
const LIST_W: f32 = 150.0;

struct Note {
    path: PathBuf,
    text: String,
    modified: SystemTime,
    /// Unsaved since this instant.
    dirty: Option<Instant>,
}

impl Note {
    fn title(&self) -> String {
        title(&self.text)
    }

    fn save(&mut self) {
        if self.text.trim().is_empty() {
            // Never write an empty note; drop one that emptied out.
            if self.path.exists() {
                let _ = std::fs::remove_file(&self.path);
            }
            self.dirty = None;
            return;
        }
        if let Some(dir) = self.path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            tracing::warn!("notes: creating {}: {e}", dir.display());
            return;
        }
        match std::fs::write(&self.path, &self.text) {
            Ok(()) => {
                self.dirty = None;
                self.modified = SystemTime::now();
            }
            Err(e) => tracing::warn!("notes: saving {}: {e}", self.path.display()),
        }
    }
}

/// First non-empty line, without Markdown heading marks.
fn title(text: &str) -> String {
    text.lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .unwrap_or("New note")
        .to_owned()
}

fn notes_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Nook").join("notes"))
}

fn load(dir: &Path) -> Vec<Note> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut notes: Vec<Note> = entries
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .filter_map(|e| {
            let path = e.path();
            let text = std::fs::read_to_string(&path).ok()?;
            let modified = e.metadata().and_then(|m| m.modified()).ok()?;
            Some(Note {
                path,
                text,
                modified,
                dirty: None,
            })
        })
        .collect();
    notes.sort_by_key(|n| std::cmp::Reverse(n.modified));
    notes
}

#[derive(Default)]
pub struct Notes {
    dir: Option<PathBuf>,
    /// Newest first (re-sorted when the page opens, not while typing).
    notes: Vec<Note>,
    current: usize,
    open_page: bool,
    /// Put the cursor in the editor on the next page frame.
    focus_editor: bool,
    retention_days: Option<u32>,
    filter: crate::kit::HistoryFilter,
}

impl Notes {
    fn new_note(&mut self) {
        self.prune_empty();
        let Some(dir) = &self.dir else {
            return;
        };
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();
        self.notes.insert(
            0,
            Note {
                path: dir.join(format!("note-{stamp}.md")),
                text: String::new(),
                modified: SystemTime::now(),
                dirty: None,
            },
        );
        self.current = 0;
        self.focus_editor = true;
    }

    /// Drops empty notes other than the one being edited.
    fn prune_empty(&mut self) {
        let current = self.notes.get(self.current).map(|n| n.path.clone());
        self.notes.retain_mut(|n| {
            let keep = !n.text.trim().is_empty() || Some(&n.path) == current.as_ref();
            if !keep {
                n.save();
            }
            keep
        });
        self.current = current
            .and_then(|p| self.notes.iter().position(|n| n.path == p))
            .unwrap_or(0);
    }

    fn select(&mut self, i: usize) {
        if i == self.current {
            return;
        }
        let target = self.notes.get(i).map(|n| n.path.clone());
        if let Some(n) = self.notes.get_mut(self.current) {
            n.save();
        }
        self.notes
            .retain(|n| !n.text.trim().is_empty() || Some(&n.path) == target.as_ref());
        self.current = target
            .and_then(|p| self.notes.iter().position(|n| n.path == p))
            .unwrap_or(0);
        self.focus_editor = true;
    }

    fn delete_current(&mut self) {
        if self.current >= self.notes.len() {
            return;
        }
        let note = self.notes.remove(self.current);
        if note.path.exists()
            && let Err(e) = shell::recycle(&note.path)
        {
            tracing::warn!("notes: deleting {}: {e}", note.path.display());
        }
        self.current = self.current.min(self.notes.len().saturating_sub(1));
    }

    fn save_all(&mut self) {
        for n in &mut self.notes {
            if n.dirty.is_some() {
                n.save();
            }
        }
    }

    fn list(&mut self, ui: &mut Ui, area: Rect, theme: &Theme) {
        let now = SystemTime::now();
        let row_h = 38.0;
        let mut pick = None;
        let filtered: Vec<usize> = self
            .notes
            .iter()
            .enumerate()
            .filter(|(_, note)| {
                self.filter
                    .matches(&note.text, crate::kit::local_day(note.modified))
            })
            .map(|(index, _)| index)
            .collect();
        ScrollArea::vertical()
            .id_salt("nook-notes-list")
            .max_height(area.height())
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let (rows, _) = ui.allocate_exact_size(
                    vec2(area.width(), filtered.len() as f32 * row_h),
                    Sense::hover(),
                );
                for (row_index, i) in filtered.iter().copied().enumerate() {
                    let Some(note) = self.notes.get(i) else {
                        continue;
                    };
                    let row = Rect::from_min_size(
                        pos2(rows.min.x, rows.min.y + row_index as f32 * row_h),
                        vec2(rows.width(), row_h - 2.0),
                    );
                    if !ui.is_rect_visible(row) {
                        continue;
                    }
                    let id = Id::new(("note-row", i));
                    let resp = ui.interact(row, id, Sense::click());
                    let hover = anim::tween(ui.ctx(), id.with("hover"), resp.hovered(), 0.12);
                    let selected = i == self.current;
                    let wash = if theme.dark {
                        Color32::from_white_alpha(if selected { 26 } else { 16 })
                    } else {
                        Color32::from_black_alpha(if selected { 18 } else { 10 })
                    };
                    let t = if selected { 1.0 } else { hover };
                    if t > 0.0 {
                        ui.painter().rect_filled(row, 10.0, with_alpha(wash, t));
                    }
                    if resp.clicked() {
                        pick = Some(i);
                    }
                    one_line(
                        ui,
                        pos2(row.min.x + 10.0, row.min.y + 6.0),
                        &note.title(),
                        row.width() - 20.0,
                        Theme::font_semibold(SIZE_LABEL + 1.0),
                        theme.text,
                    );
                    ui.painter().text(
                        pos2(row.min.x + 10.0, row.max.y - 9.0),
                        Align2::LEFT_CENTER,
                        if note.dirty.is_some() {
                            "Editing".to_owned()
                        } else {
                            retention_left(note.modified, now, self.retention_days)
                                .unwrap_or_default()
                        },
                        Theme::font(SIZE_LABEL - 1.0),
                        theme.text_faint,
                    );
                }
            });
        if let Some(i) = pick {
            self.select(i);
        }
    }

    fn editor(&mut self, ui: &mut Ui, area: Rect, theme: &Theme) {
        widgets::card(ui.painter(), area, theme);
        let Some(note) = self.notes.get_mut(self.current) else {
            return;
        };
        let inner = area.shrink2(vec2(14.0, 10.0));
        let mut eui = ui.new_child(UiBuilder::new().max_rect(inner).id_salt("note-editor"));
        let focus = std::mem::take(&mut self.focus_editor);
        ScrollArea::vertical()
            .id_salt(("nook-note-scroll", self.current))
            .max_height(inner.height())
            .auto_shrink([false, false])
            .show(&mut eui, |ui| {
                let edit = TextEdit::multiline(&mut note.text)
                    .id(Id::new(("nook-note", note.path.clone())))
                    .frame(egui::Frame::NONE)
                    .font(Theme::font(SIZE_BODY))
                    .text_color(theme.text)
                    .hint_text(
                        egui::RichText::new("Start typing. The first line is the title.")
                            .color(theme.text_faint),
                    )
                    .desired_width(inner.width())
                    .desired_rows(8)
                    .lock_focus(true);
                let resp = ui.add(edit);
                if focus {
                    resp.request_focus();
                }
                if resp.changed() {
                    note.dirty = Some(Instant::now());
                }
            });
    }
}

fn one_line(ui: &Ui, pos: egui::Pos2, text: &str, width: f32, font: egui::FontId, color: Color32) {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, color));
    job.wrap = TextWrapping {
        max_width: width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(pos, galley, color);
}

impl Notes {
    /// Opens the page on the most recent note, or a new one.
    fn open_latest(&mut self) {
        self.prune_empty();
        self.notes.sort_by_key(|n| std::cmp::Reverse(n.modified));
        if self.notes.is_empty() {
            self.new_note();
        } else {
            self.current = 0;
            self.focus_editor = true;
        }
        self.open_page = true;
    }
}

impl Module for Notes {
    fn id(&self) -> &'static str {
        "notes"
    }

    fn name(&self) -> &str {
        "Notes"
    }

    fn icon(&self) -> &'static str {
        icons::NOTE_PENCIL
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Wide
    }

    fn attach(&mut self, _ctx: &egui::Context) {
        self.dir = notes_dir();
        if let Some(dir) = &self.dir {
            self.notes = load(dir);
            tracing::info!("notes: {} loaded from {}", self.notes.len(), dir.display());
        }
    }

    fn set_retention_days(&mut self, days: Option<u32>) {
        self.retention_days = days;
    }

    fn on_tick(&mut self) {
        for n in &mut self.notes {
            if n.dirty.is_some_and(|at| at.elapsed() >= SAVE_AFTER) {
                n.save();
            }
        }
        let Some(days) = self.retention_days else {
            return;
        };
        let max_age = Duration::from_secs(u64::from(days) * 86_400);
        let current_path = self.notes.get(self.current).map(|note| note.path.clone());
        let mut keep = Vec::with_capacity(self.notes.len());
        for mut note in self.notes.drain(..) {
            if SystemTime::now()
                .duration_since(note.modified)
                .is_ok_and(|age| age >= max_age)
                && note.dirty.is_none()
                && note.path.exists()
            {
                if let Err(error) = shell::recycle(&note.path) {
                    tracing::warn!("notes: recycling expired {}: {error}", note.path.display());
                    if let Err(remove_error) = std::fs::remove_file(&note.path) {
                        tracing::warn!(
                            "notes: removing expired {}: {remove_error}",
                            note.path.display()
                        );
                        note.modified = SystemTime::now();
                        keep.push(note);
                    }
                }
            } else {
                keep.push(note);
            }
        }
        self.notes = keep;
        self.current = current_path
            .and_then(|path| self.notes.iter().position(|note| note.path == path))
            .unwrap_or_else(|| self.current.min(self.notes.len().saturating_sub(1)));
    }

    fn next_repaint(&self, _tile_visible: bool) -> Option<Duration> {
        let dirty = self
            .notes
            .iter()
            .filter_map(|n| n.dirty)
            .map(|at| SAVE_AFTER.saturating_sub(at.elapsed()) + Duration::from_millis(5))
            .min();
        let expiry = self.retention_days.and_then(|days| {
            let max_age = Duration::from_secs(u64::from(days) * 86_400);
            let now = SystemTime::now();
            self.notes
                .iter()
                .filter(|note| note.dirty.is_none() && note.path.exists())
                .map(|note| {
                    max_age
                        .saturating_sub(now.duration_since(note.modified).unwrap_or(Duration::ZERO))
                })
                .min()
        });
        match (dirty, expiry) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(value), None) | (None, Some(value)) => Some(value),
            (None, None) => None,
        }
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.open_page)
    }

    fn shutdown(&mut self) {
        self.save_all();
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        let rect = ui.max_rect();
        tile_header(ui, icons::NOTE_PENCIL, "Notes", false, theme);
        let resp = ui.interact(
            rect.expand(12.0),
            ui.id().with("notes-tile"),
            Sense::click(),
        );
        let (preview, color) = match self.notes.iter().find(|n| !n.text.trim().is_empty()) {
            Some(n) => (n.title(), theme.text_dim),
            None => ("Write something".to_owned(), theme.text_faint),
        };
        one_line(
            ui,
            pos2(rect.min.x, rect.min.y + 21.0),
            &preview,
            rect.width(),
            Theme::font(SIZE_LABEL),
            if ctx.hovered { theme.text } else { color },
        );
        if resp.clicked() {
            self.open_latest();
        }
    }

    fn ui_page(&mut self, ui: &mut Ui, header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        if !self.notes.is_empty()
            && widgets::icon_button(header, icons::TRASH, 26.0, false, theme)
                .on_hover_text("Move this note to the Recycle Bin")
                .clicked()
        {
            self.delete_current();
        }
        if widgets::icon_button(header, icons::PLUS, 26.0, false, theme)
            .on_hover_text("New note")
            .clicked()
        {
            self.new_note();
        }
        let filter_area =
            Rect::from_min_size(body.min, vec2(body.width(), body.height().min(220.0)));
        let mut filter_ui = ui.new_child(
            UiBuilder::new()
                .max_rect(filter_area)
                .id_salt("notes-filter"),
        );
        let filter_height = crate::kit::history_filter(&mut filter_ui, &mut self.filter, theme);
        let content_top = (body.min.y + filter_height + 6.0).min(body.max.y);
        if self.notes.is_empty() {
            ui.painter().text(
                body.center(),
                Align2::CENTER_CENTER,
                "No notes. Press + to start one.",
                Theme::font(SIZE_LABEL + 1.0),
                theme.text_dim,
            );
            return false;
        }
        let content = Rect::from_min_max(pos2(body.min.x, content_top), body.max);
        let list = Rect::from_min_size(content.min, vec2(LIST_W, content.height()));
        let editor = Rect::from_min_max(pos2(list.max.x + 8.0, content.min.y), content.max);
        let mut lui = ui.new_child(UiBuilder::new().max_rect(list).id_salt("notes-list"));
        self.list(&mut lui, list, theme);
        let current_matches = self.notes.get(self.current).is_some_and(|note| {
            self.filter
                .matches(&note.text, crate::kit::local_day(note.modified))
        });
        if current_matches {
            self.editor(ui, editor, theme);
        } else {
            ui.painter().text(
                editor.center(),
                Align2::CENTER_CENTER,
                "No notes match this search or date.",
                Theme::font(SIZE_LABEL),
                theme.text_dim,
            );
        }
        false
    }
}

fn retention_left(modified: SystemTime, now: SystemTime, days: Option<u32>) -> Option<String> {
    let period = Duration::from_secs(u64::from(days?) * 86_400);
    let elapsed = now.duration_since(modified).unwrap_or(Duration::ZERO);
    let remaining = period.saturating_sub(elapsed);
    let left = remaining.as_secs().div_ceil(86_400).max(1);
    Some(format!(
        "{left} {} left",
        if left == 1 { "day" } else { "days" }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_skip_blank_lines_and_heading_marks() {
        assert_eq!(title("\n\n## Groceries\nmilk"), "Groceries");
        assert_eq!(title("  \n"), "New note");
    }

    #[test]
    fn retention_countdown_uses_remaining_days() {
        let now = SystemTime::now();
        assert_eq!(
            retention_left(now - Duration::from_secs(2 * 86_400), now, Some(3)).as_deref(),
            Some("1 day left")
        );
        assert_eq!(retention_left(now, now, None), None);
    }

    #[test]
    fn history_filter_matches_content_and_selected_day() {
        let day = crate::calendar::days_from_civil(2026, 11, 1);
        let mut filter = crate::kit::HistoryFilter::default();
        filter.query = "meeting".into();
        filter.day = Some(day);
        assert!(filter.matches("Team Meeting notes", day));
        assert!(!filter.matches("Team Meeting notes", day - 1));
        assert!(!filter.matches("Shopping list", day));
    }

    #[test]
    fn file_timestamp_maps_to_windows_local_day() {
        let local = nook_platform::system::local_time();
        assert_eq!(
            crate::kit::local_day(SystemTime::now()),
            crate::calendar::days_from_civil(local.year, local.month, local.day)
        );
    }

    #[test]
    fn saves_loads_and_skips_empty_notes() {
        let dir = std::env::temp_dir().join(format!("nook-notes-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = Note {
            path: dir.join("note-1.md"),
            text: "Hello\nworld".into(),
            modified: SystemTime::now(),
            dirty: Some(Instant::now()),
        };
        a.save();
        let mut empty = Note {
            path: dir.join("note-2.md"),
            text: "  ".into(),
            modified: SystemTime::now(),
            dirty: Some(Instant::now()),
        };
        empty.save();
        let loaded = load(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].title(), "Hello");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
