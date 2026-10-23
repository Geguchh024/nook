//! Quick launch: a dock for the apps, folders, files, websites and Settings
//! pages you open all the time.
//!
//! Drop files or folders on the notch while the page is open, paste a path
//! or a URL, or browse for one. Each item shows the icon Explorer would;
//! websites get a lettered badge (Nook makes no network calls, so no
//! favicons). Click to open. The pencil lets you drag items into a new
//! order or remove them. Made Wide or Large, the tile itself shows the
//! first few for one-click launching. The list is kept in
//! `%APPDATA%\Nook\launch.txt`.

use egui::{
    Align2, Color32, ColorImage, Id, Rect, Sense, TextureHandle, TextureOptions, Ui, pos2, vec2,
};
use nook_platform::{files, shell};
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme, mix, with_alpha};
use nook_ui::{anim, icons, paint, widgets};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Instant;

use crate::kit;

const PAGE: egui::Vec2 = vec2(660.0, 420.0);
const COLS: usize = 6;
const CARD_H: f32 = 92.0;
const GAP: f32 = 10.0;
const ICON_PX: i32 = 96;
const FIELD_ID: &str = "nook-launch-add";

struct Item {
    name: String,
    target: String,
    icon: Option<TextureHandle>,
    asked: bool,
}

impl Item {
    fn new(name: impl Into<String>, target: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            target: target.into(),
            icon: None,
            asked: false,
        }
    }
}

fn list_file() -> Option<PathBuf> {
    kit::settings_path("launch.txt")
}

fn windir() -> PathBuf {
    PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into()))
}

fn is_url(t: &str) -> bool {
    let t = t.trim().to_lowercase();
    t.starts_with("http://") || t.starts_with("https://") || t.starts_with("www.")
}

/// The file whose icon stands for `target`, if any.
fn icon_source(target: &str) -> Option<PathBuf> {
    let p = Path::new(target);
    if p.is_absolute() {
        return p.exists().then(|| p.to_path_buf());
    }
    if target.to_lowercase().ends_with(".exe") {
        let candidates = [
            windir().join("System32").join(target),
            windir().join(target),
        ];
        return candidates.into_iter().find(|c| c.exists());
    }
    None
}

/// The glyph used when there is no icon.
fn glyph(target: &str) -> &'static str {
    let t = target.to_lowercase();
    if is_url(&t) {
        icons::GLOBE
    } else if t.starts_with("ms-settings:") {
        icons::GEAR_SIX
    } else if t.contains("wt.exe") || t.contains("cmd.exe") || t.contains("powershell") {
        icons::TERMINAL_WINDOW
    } else if Path::new(target).is_dir() {
        icons::FOLDER
    } else {
        icons::APP_WINDOW
    }
}

/// A friendly name: the domain of a URL, the file name of a path.
fn name_for(target: &str) -> String {
    let t = target.trim();
    if is_url(t) {
        let host = t
            .split("://")
            .nth(1)
            .unwrap_or(t)
            .trim_start_matches("www.")
            .split(['/', '?', '#'])
            .next()
            .unwrap_or(t);
        let base = host.split('.').next().unwrap_or(host);
        let mut c = base.chars();
        return match c.next() {
            Some(f) => f.to_uppercase().chain(c).collect(),
            None => host.to_owned(),
        };
    }
    if t.to_lowercase().starts_with("ms-settings:") {
        return "Settings".into();
    }
    let p = Path::new(t);
    if p.parent().is_none() || t.ends_with(":\\") {
        return t.to_owned();
    }
    p.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| t.to_owned())
}

fn defaults() -> Vec<Item> {
    let mut items = vec![Item::new(
        "Explorer",
        windir().join("explorer.exe").display().to_string(),
    )];
    if let Some(d) = shell::downloads_dir() {
        items.push(Item::new("Downloads", d.display().to_string()));
    }
    items.push(Item::new("Settings", "ms-settings:"));
    items.push(Item::new("Task Manager", "taskmgr.exe"));
    let wt = std::env::var_os("LOCALAPPDATA").map(|a| {
        PathBuf::from(a)
            .join("Microsoft")
            .join("WindowsApps")
            .join("wt.exe")
    });
    if wt.is_some_and(|p| p.exists()) {
        items.push(Item::new("Terminal", "wt.exe"));
    } else {
        items.push(Item::new("Terminal", "cmd.exe"));
    }
    items.push(Item::new("Notepad", "notepad.exe"));
    items
}

/// A stable color for a name (for lettered badges).
fn hue_color(name: &str) -> Color32 {
    const PALETTE: [Color32; 6] = [
        Color32::from_rgb(124, 92, 255),
        Color32::from_rgb(14, 165, 233),
        Color32::from_rgb(16, 185, 129),
        Color32::from_rgb(245, 158, 11),
        Color32::from_rgb(236, 72, 153),
        Color32::from_rgb(239, 68, 68),
    ];
    let h = name
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    PALETTE[h as usize % PALETTE.len()]
}

type Icon = (String, ColorImage);
/// Requests (target, icon file) and the icons that come back.
type Iconer = (Sender<(String, PathBuf)>, Receiver<Icon>);

fn spawn_iconer(ctx: egui::Context) -> Option<Iconer> {
    let (req_tx, req_rx) = mpsc::channel::<(String, PathBuf)>();
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("nook-launch-icons".into())
        .spawn(move || {
            files::init_thumbnail_thread();
            for (target, path) in req_rx {
                match files::thumbnail(&path, ICON_PX) {
                    Ok((w, h, rgba)) => {
                        let img =
                            ColorImage::from_rgba_premultiplied([w as usize, h as usize], &rgba);
                        if tx.send((target, img)).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                    Err(e) => tracing::debug!("launch icon {}: {e}", path.display()),
                }
            }
        })
        .ok()
        .map(|_| (req_tx, rx))
}

#[derive(Default)]
pub struct QuickLaunch {
    items: Vec<Item>,
    iconer: Option<Iconer>,
    pending: Vec<Icon>,
    draft: String,
    editing: bool,
    /// Index being dragged in edit mode and the pointer offset.
    drag: Option<(usize, egui::Vec2)>,
    open_page: bool,
    page_seen: Option<Instant>,
    picker: Option<Receiver<Vec<PathBuf>>>,
    ctx: Option<egui::Context>,
    toast: Option<(String, Instant)>,
}

impl QuickLaunch {
    fn save(&self) {
        let Some(path) = list_file() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let text: String = self
            .items
            .iter()
            .map(|i| format!("{}\t{}\n", i.name.replace('\t', " "), i.target))
            .collect();
        if let Err(e) = std::fs::write(&path, text) {
            tracing::warn!("quick launch: saving {}: {e}", path.display());
        }
    }

    fn add(&mut self, target: &str) {
        let target = target.trim().trim_matches('"').to_owned();
        if target.is_empty()
            || self
                .items
                .iter()
                .any(|i| i.target.eq_ignore_ascii_case(&target))
        {
            return;
        }
        let target = if target.to_lowercase().starts_with("www.") {
            format!("https://{target}")
        } else {
            target
        };
        self.items.push(Item::new(name_for(&target), target));
        self.save();
    }

    fn launch(&mut self, i: usize) {
        let Some(item) = self.items.get(i) else {
            return;
        };
        let res = shell::open(&item.target);
        let name = item.name.clone();
        self.toast = Some((
            match res {
                Ok(()) => format!("Opening {name}"),
                Err(e) => format!("Could not open {name}: {e}"),
            },
            Instant::now(),
        ));
    }

    /// Uploads arrived icons and asks for missing ones.
    fn icons(&mut self, ctx: &egui::Context) {
        if self.iconer.is_none() {
            self.iconer = spawn_iconer(ctx.clone());
        }
        if let Some((_, rx)) = &self.iconer {
            self.pending.extend(rx.try_iter());
        }
        for (target, img) in std::mem::take(&mut self.pending) {
            for item in self.items.iter_mut().filter(|i| i.target == target) {
                item.icon = Some(ctx.load_texture(
                    format!("nook-launch-{target}"),
                    img.clone(),
                    TextureOptions::LINEAR,
                ));
            }
        }
        if let Some((tx, _)) = &self.iconer {
            for item in self.items.iter_mut().filter(|i| !i.asked) {
                item.asked = true;
                if let Some(src) = icon_source(&item.target) {
                    let _ = tx.send((item.target.clone(), src));
                }
            }
        }
    }

    fn page_visible(&self) -> bool {
        self.page_seen
            .is_some_and(|t| t.elapsed().as_secs_f32() < 0.5)
    }

    fn browse(&mut self) {
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("nook-launch-pick".into())
            .spawn(move || {
                let picked = files::pick_files("Add to Quick launch").unwrap_or_default();
                let _ = tx.send(picked);
                if let Some(c) = ctx {
                    c.request_repaint();
                }
            });
        if spawned.is_ok() {
            self.picker = Some(rx);
        }
    }

    fn grid(&mut self, ui: &mut Ui, area: Rect, theme: &Theme) {
        let w = (area.width() - GAP * (COLS as f32 - 1.0)) / COLS as f32;
        let cell = |i: usize| {
            Rect::from_min_size(
                pos2(
                    area.min.x + (i % COLS) as f32 * (w + GAP),
                    area.min.y + (i / COLS) as f32 * (CARD_H + GAP),
                ),
                vec2(w, CARD_H),
            )
        };
        let now = ui.input(|i| i.time);
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let mut launch = None;
        let mut remove = None;
        let mut drop_at = None;
        for i in 0..self.items.len() {
            let mut r = cell(i);
            if r.max.y > area.max.y + 1.0 {
                break;
            }
            let id = Id::new(("launch-card", i));
            let sense = if self.editing {
                Sense::click_and_drag()
            } else {
                Sense::click()
            };
            let resp = ui.interact(r, id, sense);
            let dragging = self.drag.is_some_and(|(d, _)| d == i);
            if self.editing {
                if resp.drag_started()
                    && let Some(p) = pointer
                {
                    self.drag = Some((i, p - r.min));
                }
                if dragging && resp.drag_stopped() {
                    drop_at = pointer;
                }
                if dragging && let (Some(p), Some((_, grab))) = (pointer, self.drag) {
                    r = Rect::from_min_size(p - grab, r.size());
                } else {
                    // A gentle wobble says the items can move.
                    let phase = i as f64 * 1.7;
                    let dx = ((now * 9.0 + phase).sin() * 0.8) as f32;
                    r = r.translate(vec2(dx, 0.0));
                    ui.ctx().request_repaint();
                }
            }
            let hover = anim::tween(ui.ctx(), id.with("h"), resp.hovered() || dragging, 0.12);
            let lift = if dragging { 1.0 } else { hover };
            let painter = ui.painter();
            if lift > 0.01 {
                painter.rect_filled(
                    r.expand(2.0 * lift),
                    18.0,
                    with_alpha(theme.accent, 0.10 * lift),
                );
            }
            widgets::card(painter, r, theme);
            let icon_r =
                Rect::from_center_size(pos2(r.center().x, r.min.y + 32.0), vec2(40.0, 40.0));
            draw_icon(
                ui,
                icon_r.translate(vec2(0.0, -2.0 * lift)),
                &self.items[i],
                theme,
            );
            let name = &self.items[i].name;
            let galley_w = ui
                .painter()
                .layout_no_wrap(name.clone(), Theme::font(SIZE_LABEL), theme.text)
                .size()
                .x
                .min(r.width() - 12.0);
            kit::one_line(
                ui,
                pos2(r.center().x - galley_w * 0.5, r.max.y - 26.0),
                name,
                r.width() - 12.0,
                Theme::font(SIZE_LABEL),
                if hover > 0.5 {
                    theme.text
                } else {
                    mix(theme.text, theme.text_dim, 0.3)
                },
            );
            if self.editing {
                let c = pos2(r.min.x + 12.0, r.min.y + 12.0);
                let badge = Rect::from_center_size(c, vec2(20.0, 20.0));
                let bresp = ui.interact(badge, id.with("remove"), Sense::click());
                ui.painter()
                    .circle_filled(c, 9.0, Color32::from_rgb(239, 68, 68));
                ui.painter().hline(
                    c.x - 4.0..=c.x + 4.0,
                    c.y,
                    egui::Stroke::new(2.0, Color32::WHITE),
                );
                if bresp.on_hover_text("Remove").clicked() {
                    remove = Some(i);
                }
            } else if resp.clicked() {
                launch = Some(i);
            }
            if !self.editing {
                resp.on_hover_text(&self.items[i].target);
            }
        }

        // Dropping a dragged item moves it to the cell under the pointer.
        if let (Some(p), Some((from, _))) = (drop_at, self.drag) {
            let col = ((p.x - area.min.x) / (w + GAP))
                .floor()
                .clamp(0.0, COLS as f32 - 1.0) as usize;
            let row = ((p.y - area.min.y) / (CARD_H + GAP)).floor().max(0.0) as usize;
            let to = (row * COLS + col).min(self.items.len() - 1);
            if to != from {
                let item = self.items.remove(from);
                self.items.insert(to, item);
                self.save();
            }
            self.drag = None;
        }
        if self.drag.is_some() && !ui.input(|i| i.pointer.any_down()) {
            self.drag = None;
        }
        if let Some(i) = remove {
            self.items.remove(i);
            self.save();
        }
        if let Some(i) = launch {
            self.launch(i);
        }
    }

    fn add_bar(&mut self, ui: &mut Ui, bar: Rect, theme: &Theme) {
        let browse = Rect::from_min_size(
            pos2(bar.max.x - 196.0, bar.min.y),
            vec2(100.0, bar.height()),
        );
        let add = Rect::from_min_size(pos2(bar.max.x - 88.0, bar.min.y), vec2(88.0, bar.height()));
        let field = Rect::from_min_max(bar.min, pos2(browse.min.x - 8.0, bar.max.y));
        let id = Id::new(FIELD_ID);
        let resp = kit::field(
            ui,
            field,
            &mut self.draft,
            "Paste a path, a website or ms-settings: link",
            id,
            Theme::font(SIZE_BODY),
            theme,
        );
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if kit::button(ui, browse, "Browse", Some(icons::FOLDER_OPEN), false, theme).clicked() {
            self.browse();
        }
        let ready = !self.draft.trim().is_empty();
        let clicked = ui
            .add_enabled_ui(ready, |ui| {
                kit::button(ui, add, "Add", Some(icons::PLUS), true, theme).clicked()
            })
            .inner;
        if (enter || clicked) && ready {
            let d = std::mem::take(&mut self.draft);
            self.add(&d);
        }
    }
}

/// An item's icon: its shell icon, a lettered badge for websites, or a glyph.
fn draw_icon(ui: &Ui, rect: Rect, item: &Item, theme: &Theme) {
    if let Some(tex) = &item.icon {
        let size = tex.size_vec2();
        let k = (rect.width() / size.x).min(rect.height() / size.y);
        let r = Rect::from_center_size(rect.center(), size * k);
        egui::Image::new(tex).paint_at(ui, r);
        return;
    }
    let painter = ui.painter();
    if is_url(&item.target) {
        let c = hue_color(&item.name);
        let bottom = mix(c, Color32::BLACK, 0.25);
        let s = nook_ui::geometry::Squircle::new(rect.shrink(2.0), 10.0);
        paint::fill_gradient(painter, &s.outline(), c, bottom);
        let letter = item
            .name
            .chars()
            .next()
            .unwrap_or('?')
            .to_uppercase()
            .to_string();
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            letter,
            Theme::font_semibold(19.0),
            Color32::WHITE,
        );
        return;
    }
    let s = nook_ui::geometry::Squircle::new(rect.shrink(2.0), 10.0);
    let (a, b) = theme.active_tile();
    paint::fill_gradient(painter, &s.outline(), a, b);
    paint::icon(
        painter,
        rect.center(),
        glyph(&item.target),
        20.0,
        theme.accent_fg(),
        true,
    );
}

fn icon_hit(ui: &mut Ui, rect: Rect, id: Id) -> egui::Response {
    ui.interact(rect, id, Sense::click())
}

impl Module for QuickLaunch {
    fn id(&self) -> &'static str {
        "quick_launch"
    }

    fn name(&self) -> &str {
        "Quick launch"
    }

    fn icon(&self) -> &'static str {
        icons::ROCKET_LAUNCH
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn attach(&mut self, ctx: &egui::Context) {
        self.ctx = Some(ctx.clone());
        match list_file().and_then(|p| std::fs::read_to_string(p).ok()) {
            Some(text) => {
                self.items = text
                    .lines()
                    .filter_map(|l| l.split_once('\t'))
                    .map(|(n, t)| Item::new(n.trim(), t.trim()))
                    .collect();
            }
            None => self.items = defaults(),
        }
    }

    fn on_tick(&mut self) {
        if let Some(rx) = &self.picker
            && let Ok(paths) = rx.try_recv()
        {
            self.picker = None;
            for p in paths {
                self.add(&p.display().to_string());
            }
        }
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.open_page)
    }

    fn page_size(&self) -> Option<egui::Vec2> {
        Some(PAGE)
    }

    fn accepts_files(&self) -> bool {
        self.page_visible()
    }

    fn drop_files(&mut self, paths: Vec<PathBuf>) {
        for p in paths {
            self.add(&p.display().to_string());
        }
    }

    fn next_repaint(&self, _tile_visible: bool) -> Option<std::time::Duration> {
        self.toast
            .as_ref()
            .and_then(|(_, at)| std::time::Duration::from_secs(3).checked_sub(at.elapsed()))
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        let rect = ui.max_rect();
        let egui_ctx = ui.ctx().clone();
        // Wide and large tiles launch straight from the tile.
        let (cols, rows) = if rect.height() > 80.0 {
            (3, 3)
        } else if rect.width() > rect.height() * 1.5 {
            (3, 1)
        } else {
            (0, 0)
        };
        if cols > 0 && !self.items.is_empty() {
            self.icons(&egui_ctx);
            let back = ui.interact(
                rect.expand(10.0),
                ui.id().with("launch-tile"),
                Sense::click(),
            );
            let cw = rect.width() / cols as f32;
            let ch = rect.height() / rows as f32;
            let side = cw.min(ch) - 6.0;
            let mut launch = None;
            for (i, item) in self.items.iter().take(cols * rows).enumerate() {
                let c = pos2(
                    rect.min.x + cw * ((i % cols) as f32 + 0.5),
                    rect.min.y + ch * ((i / cols) as f32 + 0.5),
                );
                let r = Rect::from_center_size(c, vec2(side, side));
                let id = Id::new(("launch-tile-icon", i));
                let resp = icon_hit(ui, r, id);
                let h = anim::tween(ui.ctx(), id.with("h"), resp.hovered(), 0.1);
                if h > 0.0 {
                    ui.painter().rect_filled(
                        r.expand(2.0),
                        9.0,
                        with_alpha(kit::wash(theme), 2.0 * h),
                    );
                }
                draw_icon(ui, r.shrink(2.0 - 2.0 * h), item, theme);
                if resp.on_hover_text(&item.name).clicked() {
                    launch = Some(i);
                }
            }
            if let Some(i) = launch {
                self.launch(i);
            } else if back.clicked() {
                self.open_page = true;
            }
            return;
        }
        if kit::tool_tile(ui, ctx, icons::ROCKET_LAUNCH, false)
            .on_hover_text("Your apps, folders and sites, one click away")
            .clicked()
        {
            self.open_page = true;
        }
    }

    fn ui_page(&mut self, ui: &mut Ui, header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        self.page_seen = Some(Instant::now());
        self.icons(ui.ctx());
        let glyph = if self.editing {
            icons::CHECK
        } else {
            icons::PENCIL_SIMPLE
        };
        if widgets::icon_button(header, glyph, 26.0, self.editing, theme)
            .on_hover_text(if self.editing {
                "Done"
            } else {
                "Reorder or remove"
            })
            .clicked()
        {
            self.editing = !self.editing;
            self.drag = None;
        }
        if self.items.len() > 1
            && widgets::icon_button(header, icons::ARROW_COUNTER_CLOCKWISE, 26.0, false, theme)
                .on_hover_text("Reset to the starter set")
                .clicked()
        {
            self.items = defaults();
            self.save();
        }

        let bar_h = 34.0;
        let grid = Rect::from_min_max(body.min, pos2(body.max.x, body.max.y - bar_h - 26.0));
        if self.items.is_empty() {
            kit::empty(
                ui,
                grid.center(),
                icons::ROCKET_LAUNCH,
                "Drop apps, files or folders here",
                theme,
            );
        } else {
            self.grid(ui, grid, theme);
        }
        let hint_y = body.max.y - bar_h - 12.0;
        let (hint, color) = match &self.toast {
            Some((t, at)) if at.elapsed().as_secs_f32() < 3.0 => (t.clone(), theme.accent_fg()),
            _ if self.editing => (
                "Drag to reorder  \u{00B7}  the red badge removes".to_owned(),
                theme.text_faint,
            ),
            _ => (
                "Drop files or folders on the notch to add them".to_owned(),
                theme.text_faint,
            ),
        };
        ui.painter().text(
            pos2(body.min.x + 2.0, hint_y),
            Align2::LEFT_CENTER,
            hint,
            Theme::font(SIZE_LABEL - 1.0),
            color,
        );
        let bar = Rect::from_min_max(pos2(body.min.x, body.max.y - bar_h), body.max);
        self.add_bar(ui, bar, theme);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_kinds() {
        assert_eq!(name_for("https://www.github.com/rust"), "Github");
        assert_eq!(name_for("ms-settings:display"), "Settings");
        assert_eq!(
            name_for("C:\\Tools\\Paint.NET\\paintdotnet.exe"),
            "paintdotnet"
        );
        assert_eq!(name_for("D:\\"), "D:\\");
        assert!(is_url("www.example.com"));
        assert_eq!(glyph("https://x.y"), icons::GLOBE);
        assert_eq!(glyph("ms-settings:"), icons::GEAR_SIX);
        assert!(icon_source("notepad.exe").is_some());
        assert_eq!(hue_color("a"), hue_color("a"));
    }
}
