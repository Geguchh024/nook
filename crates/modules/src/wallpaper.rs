//! Wallpaper: browse a folder of pictures and set one as the desktop
//! wallpaper.
//!
//! The tile shows the current wallpaper; click it for the page. The page is
//! one large preview with a filmstrip of the folder along its bottom edge.
//! The strip scrolls sideways (wheel, trackpad or drag) on a spring, and
//! thumbnails fade out toward its edges. Hovering a thumbnail crossfades it
//! into the preview; clicking it sets it as the wallpaper on every monitor.
//!
//! Thumbnails and previews come from the shell's thumbnail cache (the same
//! pictures Explorer shows, so any format Windows can read works) on two
//! worker threads: small ones for the strip, only for what is in view, and
//! large ones for the preview, newest request first. The folder is kept in
//! `%APPDATA%\Nook\wallpaper.txt`.

use egui::epaint::RectShape;
use egui::{
    Align2, Color32, ColorImage, Context, Id, MouseWheelUnit, Rect, Sense, TextureHandle,
    TextureOptions, Ui, Vec2, pos2, vec2,
};
use nook_platform::{files, system, wallpaper};
use nook_ui::geometry::Squircle;
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::spring::{Spring, SpringParams};
use nook_ui::theme::{SIZE_ICON, SIZE_LABEL, TILE_RADIUS, Theme, with_alpha};
use nook_ui::{anim, icons, paint, widgets};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

const EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jfif", "png", "bmp", "gif", "webp", "tif", "tiff", "heic", "avif",
];
/// Pictures listed from one folder (subfolders two levels deep included).
const MAX_FILES: usize = 600;
/// Strip thumbnail size requested from the shell, pixels.
const THUMB_PX: i32 = 200;
/// Preview size requested from the shell, pixels.
const PREVIEW_PX: i32 = 1280;
/// Large previews kept as textures (about 3 MB each).
const PREVIEW_CACHE: usize = 6;

const THUMB: Vec2 = vec2(84.0, 52.0);
const GAP: f32 = 8.0;
/// Inset of the strip from the preview's edges.
const INSET: f32 = 10.0;
/// Width over which thumbnails fade out at the strip's edges.
const EDGE_FADE: f32 = 36.0;
/// Preview crossfade, seconds.
const FADE: f32 = 0.32;

enum Event {
    /// The folder picker closed; `None` if it was cancelled.
    Picked(Option<PathBuf>),
    Listed(PathBuf, Vec<PathBuf>),
    Thumb(PathBuf, ColorImage),
    Preview(PathBuf, ColorImage),
    Applied(PathBuf, Result<(), String>),
}

fn config_file() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Nook").join("wallpaper.txt"))
}

fn is_picture(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Pictures in `dir` and up to `depth` levels of subfolders, sorted by path
/// (case-insensitive) and capped at [`MAX_FILES`].
fn list_pictures(dir: &Path, depth: u32) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        let mut subdirs = Vec::new();
        for e in entries.flatten() {
            let path = e.path();
            match e.file_type() {
                Ok(t) if t.is_dir() => subdirs.push(path),
                Ok(_) if is_picture(&path) => out.push(path),
                _ => {}
            }
        }
        if depth > 0 {
            for d in subdirs {
                if out.len() >= MAX_FILES {
                    break;
                }
                walk(&d, depth - 1, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, depth, &mut out);
    out.sort_by_cached_key(|p| p.to_string_lossy().to_lowercase());
    out.truncate(MAX_FILES);
    out
}

/// UV rect that crops a `tex`-shaped image to fill a `rect`-shaped area,
/// zoomed in by `zoom` (1 = no zoom) around the center.
fn cover_uv(tex: Vec2, rect: Vec2, zoom: f32) -> Rect {
    let (ta, ra) = (tex.x / tex.y.max(1.0), rect.x / rect.y.max(1.0));
    let (w, h) = if ta > ra {
        (ra / ta, 1.0)
    } else {
        (1.0, ta / ra)
    };
    let half = vec2(w, h) * (0.5 / zoom.max(1.0));
    Rect::from_min_max(pos2(0.5, 0.5) - half, pos2(0.5, 0.5) + half)
}

/// Draws `tex` cropped to fill `rect` with rounded corners.
fn image_cover(ui: &Ui, rect: Rect, tex: &TextureHandle, radius: f32, zoom: f32, alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    let uv = cover_uv(tex.size_vec2(), rect.size(), zoom);
    ui.painter().add(
        RectShape::filled(rect, radius, Color32::WHITE.gamma_multiply(alpha))
            .with_texture(tex.id(), uv),
    );
}

fn file_name(path: &Path) -> String {
    path.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn same_file(a: &Path, b: &Path) -> bool {
    a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
}

/// A worker that turns paths into shell thumbnails of `size` pixels. With
/// `latest_only`, a backlog is skipped down to the newest request (the
/// preview only cares about what is hovered now).
fn spawn_thumbnailer(
    name: &str,
    size: i32,
    latest_only: bool,
    tx: Sender<Event>,
    ctx: Context,
) -> Option<Sender<PathBuf>> {
    let (req_tx, req_rx) = mpsc::channel::<PathBuf>();
    let spawned = std::thread::Builder::new()
        .name(name.into())
        .spawn(move || {
            files::init_thumbnail_thread();
            while let Ok(mut path) = req_rx.recv() {
                if latest_only && let Some(last) = req_rx.try_iter().last() {
                    path = last;
                }
                match files::thumbnail(&path, size) {
                    Ok((w, h, rgba)) => {
                        let img =
                            ColorImage::from_rgba_premultiplied([w as usize, h as usize], &rgba);
                        let event = if latest_only {
                            Event::Preview(path, img)
                        } else {
                            Event::Thumb(path, img)
                        };
                        if tx.send(event).is_err() {
                            break;
                        }
                        ctx.request_repaint();
                    }
                    Err(e) => tracing::debug!("wallpaper thumbnail {}: {e}", path.display()),
                }
            }
        });
    match spawned {
        Ok(_) => Some(req_tx),
        Err(e) => {
            tracing::warn!("spawning {name} thread: {e}");
            None
        }
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
        tracing::warn!("spawning {name} thread: {e}");
    }
}

#[derive(Default)]
pub struct Wallpaper {
    folder: Option<PathBuf>,
    items: Vec<PathBuf>,
    /// Strip thumbnails; `None` while one is on its way.
    thumbs: HashMap<PathBuf, Option<TextureHandle>>,
    /// Large previews, most recently used last.
    previews: Vec<(PathBuf, TextureHandle)>,
    preview_asked: Option<PathBuf>,
    /// The desktop wallpaper right now.
    current: Option<PathBuf>,
    /// What the preview shows, what it is fading from, and since when.
    shown: Option<PathBuf>,
    fading_from: Option<PathBuf>,
    shown_since: f64,
    /// Thumbnail under the pointer, held while the pointer is on the strip.
    hovered: Option<PathBuf>,
    scroll: Option<Spring>,
    /// The page was drawn last at this time; a gap means it just opened.
    last_page_frame: f64,
    opened_at: f64,
    listing: bool,
    picking: bool,
    applying: Option<PathBuf>,
    /// Applied, and when (for the check mark's pop).
    applied: Option<(PathBuf, f64)>,
    error: Option<String>,
    open_page: bool,
    show_page: bool,
    tx: Option<Sender<Event>>,
    rx: Option<Receiver<Event>>,
    thumb_req: Option<Sender<PathBuf>>,
    preview_req: Option<Sender<PathBuf>>,
    ctx: Option<Context>,
}

impl Wallpaper {
    fn save(&self) {
        let (Some(file), Some(folder)) = (config_file(), &self.folder) else {
            return;
        };
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&file, folder.display().to_string()) {
            tracing::warn!("wallpaper: saving {}: {e}", file.display());
        }
    }

    fn set_folder(&mut self, folder: PathBuf) {
        self.folder = Some(folder.clone());
        self.items.clear();
        self.thumbs.clear();
        self.scroll = None;
        self.listing = true;
        let Some(tx) = self.tx.clone() else {
            return;
        };
        let ctx = self.ctx.clone();
        spawn("nook-wallpaper-list", move || {
            let list = list_pictures(&folder, 2);
            let _ = tx.send(Event::Listed(folder, list));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
    }

    fn pick_folder(&mut self) {
        if self.picking {
            return;
        }
        let Some(tx) = self.tx.clone() else {
            return;
        };
        self.picking = true;
        let start = self
            .folder
            .clone()
            .or_else(|| std::env::var_os("USERPROFILE").map(|u| PathBuf::from(u).join("Pictures")));
        let ctx = self.ctx.clone();
        spawn("nook-wallpaper-pick", move || {
            let dir = wallpaper::pick_folder("Choose your wallpaper folder", start.as_deref())
                .unwrap_or_else(|e| {
                    tracing::warn!("wallpaper: folder picker: {e}");
                    None
                });
            let _ = tx.send(Event::Picked(dir));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
    }

    fn apply(&mut self, path: PathBuf) {
        if self.applying.is_some() {
            return;
        }
        let Some(tx) = self.tx.clone() else {
            return;
        };
        self.applying = Some(path.clone());
        self.error = None;
        let ctx = self.ctx.clone();
        spawn("nook-wallpaper-set", move || {
            let result = wallpaper::set_desktop(&path).map_err(|e| e.to_string());
            let _ = tx.send(Event::Applied(path, result));
            if let Some(ctx) = ctx {
                ctx.request_repaint();
            }
        });
    }

    fn request_thumb(&mut self, path: &Path) {
        if self.thumbs.contains_key(path) {
            return;
        }
        if let Some(tx) = &self.thumb_req {
            self.thumbs.insert(path.to_path_buf(), None);
            let _ = tx.send(path.to_path_buf());
        }
    }

    fn thumb(&self, path: &Path) -> Option<&TextureHandle> {
        self.thumbs.get(path).and_then(Option::as_ref)
    }

    /// The sharpest texture there is for `path`: the preview if it has
    /// arrived, else the strip thumbnail. With `fetch`, asks for the preview
    /// if it is missing.
    fn best_texture(&mut self, path: &Path, fetch: bool) -> Option<TextureHandle> {
        if let Some(i) = self.previews.iter().position(|(p, _)| p == path) {
            let entry = self.previews.remove(i);
            let tex = entry.1.clone();
            self.previews.push(entry);
            return Some(tex);
        }
        if fetch
            && self.preview_asked.as_deref() != Some(path)
            && let Some(tx) = &self.preview_req
        {
            self.preview_asked = Some(path.to_path_buf());
            let _ = tx.send(path.to_path_buf());
        }
        self.request_thumb(path);
        self.thumb(path).cloned()
    }

    /// Points the preview at `target`, starting a crossfade if it changed.
    fn show(&mut self, target: Option<PathBuf>, now: f64) {
        if target != self.shown {
            self.fading_from = self.shown.take();
            self.shown = target;
            self.shown_since = now;
        }
    }

    fn scroll_spring(&mut self) -> &mut Spring {
        self.scroll
            .get_or_insert_with(|| Spring::new(0.0, SpringParams::critical(180.0)))
    }

    /// Scrolls so `index` sits in the middle of a strip `width` wide.
    fn center_on(&mut self, index: usize, width: f32, snap: bool) {
        let content = self.items.len() as f32 * (THUMB.x + GAP) - GAP;
        let max = (content - width).max(0.0);
        let target = (index as f32 * (THUMB.x + GAP) + THUMB.x * 0.5 - width * 0.5).clamp(0.0, max);
        let s = self.scroll_spring();
        if snap {
            s.snap(target);
        } else {
            s.set_target(target);
        }
    }

    fn upload(ctx: &Context, path: &Path, img: ColorImage) -> TextureHandle {
        ctx.load_texture(
            format!("nook-wallpaper-{}", path.display()),
            img,
            TextureOptions::LINEAR,
        )
    }

    fn page_body(&mut self, ui: &mut Ui, body: Rect, theme: &Theme) {
        let ctx = ui.ctx().clone();
        let now = ctx.input(|i| i.time);
        if now - self.last_page_frame > 0.25 {
            // Just opened: replay the strip's entrance and bring the current
            // wallpaper into view.
            self.opened_at = now;
            self.hovered = None;
            // It may have been changed elsewhere since.
            if self.applying.is_none() {
                self.current = system::wallpaper_path().filter(|p| p.exists());
            }
            if let Some(i) = self.current_index() {
                self.center_on(i, body.width() - 2.0 * INSET, true);
            }
        }
        self.last_page_frame = now;

        let painter = ui.painter_at(body);
        let shape = Squircle::new(body, TILE_RADIUS);
        widgets::card(&painter, body, theme);

        // Preview: the hovered picture, else the current wallpaper.
        let target = self.hovered.clone().or_else(|| self.current.clone());
        self.show(target, now);
        let t = if anim::motion(&ctx).reduced {
            1.0
        } else {
            ((now - self.shown_since) as f32 / FADE).clamp(0.0, 1.0)
        };
        if t < 1.0 {
            ctx.request_repaint();
        }
        let e = anim::ease_out_cubic(t);
        if t < 1.0
            && let Some(from) = self.fading_from.clone()
            && let Some(tex) = self.best_texture(&from, false)
        {
            image_cover(ui, body, &tex, TILE_RADIUS, 1.0, 1.0);
        }
        if let Some(shown) = self.shown.clone() {
            match self.best_texture(&shown, true) {
                // The incoming picture settles from a slight zoom as it fades in.
                Some(tex) => image_cover(ui, body, &tex, TILE_RADIUS, 1.0 + 0.05 * (1.0 - e), e),
                None => paint::icon(
                    &painter,
                    body.center() - vec2(0.0, 24.0),
                    icons::IMAGE,
                    30.0,
                    theme.text_faint,
                    false,
                ),
            }
        }
        // Darken toward the bottom so the strip and labels read on any picture.
        paint::fill_gradient(
            &painter,
            &shape.outline(),
            Color32::from_black_alpha(10),
            Color32::from_black_alpha(150),
        );

        self.caption(ui, body, theme);

        if self.items.is_empty() {
            self.empty_state(ui, body, theme);
            return;
        }
        self.strip(ui, body, now, theme);
    }

    /// Name of what the preview shows, top left, in a small dark pill.
    fn caption(&self, ui: &Ui, body: Rect, theme: &Theme) {
        let Some(shown) = &self.shown else {
            return;
        };
        let is_current = self.current.as_deref().is_some_and(|c| same_file(c, shown));
        let applying = self.applying.as_deref() == Some(shown.as_path());
        let label = if applying {
            format!("Setting {}…", file_name(shown))
        } else if is_current {
            format!("{}  \u{00B7}  Current", file_name(shown))
        } else {
            format!("{}  \u{00B7}  Click to set", file_name(shown))
        };
        let font = Theme::font(SIZE_LABEL);
        let galley = ui
            .painter()
            .layout_no_wrap(label, font, Color32::from_white_alpha(235));
        let size = galley.size() + vec2(18.0, 8.0);
        let pill = Rect::from_min_size(body.min + vec2(INSET, INSET), size);
        let pill = Rect::from_min_size(
            pill.min,
            vec2(pill.width().min(body.width() - 2.0 * INSET), size.y),
        );
        ui.painter()
            .rect_filled(pill, size.y * 0.5, Color32::from_black_alpha(110));
        ui.painter().with_clip_rect(pill.shrink(4.0)).galley(
            pill.min + vec2(9.0, 4.0),
            galley,
            Color32::WHITE,
        );
        if let Some(err) = &self.error {
            ui.painter().text(
                pos2(body.max.x - INSET, body.min.y + INSET + size.y * 0.5),
                Align2::RIGHT_CENTER,
                err,
                Theme::font(SIZE_LABEL),
                theme.accent_fg(),
            );
        }
    }

    fn empty_state(&mut self, ui: &mut Ui, body: Rect, theme: &Theme) {
        let text = if self.listing {
            "Looking for pictures…"
        } else if self.folder.is_some() {
            "No pictures in this folder."
        } else {
            "Choose a folder with your wallpapers."
        };
        ui.painter().text(
            pos2(body.center().x, body.max.y - 58.0),
            Align2::CENTER_CENTER,
            text,
            Theme::font(SIZE_LABEL + 1.0),
            Color32::from_white_alpha(220),
        );
        let button =
            Rect::from_center_size(pos2(body.center().x, body.max.y - 28.0), vec2(132.0, 28.0));
        let resp = ui.interact(button, Id::new("nook-wallpaper-choose"), Sense::click());
        let hover = anim::tween(ui.ctx(), resp.id.with("hover"), resp.hovered(), 0.12);
        ui.painter()
            .rect_filled(button, 14.0, with_alpha(theme.accent, 0.85 + 0.15 * hover));
        paint::icon(
            ui.painter(),
            button.left_center() + vec2(20.0, 0.0),
            icons::FOLDER_OPEN,
            14.0,
            Color32::WHITE,
            true,
        );
        ui.painter().text(
            button.left_center() + vec2(32.0, 0.0),
            Align2::LEFT_CENTER,
            "Choose folder",
            Theme::font_semibold(SIZE_LABEL + 1.0),
            Color32::WHITE,
        );
        if resp.clicked() {
            self.pick_folder();
        }
    }

    /// The filmstrip along the bottom of the preview.
    fn strip(&mut self, ui: &mut Ui, body: Rect, now: f64, theme: &Theme) {
        let ctx = ui.ctx().clone();
        let reduced = anim::motion(&ctx).reduced;
        let lane = Rect::from_min_max(
            pos2(body.min.x + INSET, body.max.y - INSET - THUMB.y - 6.0),
            pos2(body.max.x - INSET, body.max.y - INSET + 2.0),
        );
        let n = self.items.len();
        let content = n as f32 * (THUMB.x + GAP) - GAP;
        let max = (content - lane.width()).max(0.0);
        let overflow = max > 0.0;

        // Wheel (either axis) and drag move the target; the strip follows on
        // a spring.
        let lane_resp = ui.interact(lane, Id::new("nook-wallpaper-lane"), Sense::drag());
        let pointer_in = ui.rect_contains_pointer(lane);
        if pointer_in && overflow {
            let d: f32 = ctx.input(|i| {
                i.events
                    .iter()
                    .map(|e| match e {
                        egui::Event::MouseWheel { unit, delta, .. } => {
                            let d = if delta.x.abs() > delta.y.abs() {
                                delta.x
                            } else {
                                delta.y
                            };
                            match unit {
                                MouseWheelUnit::Point => d,
                                MouseWheelUnit::Line => d * 48.0,
                                MouseWheelUnit::Page => d * lane.width(),
                            }
                        }
                        _ => 0.0,
                    })
                    .sum()
            });
            if d != 0.0 {
                let s = self.scroll_spring();
                s.set_target((s.target - d).clamp(0.0, max));
            }
        }
        let dt = ctx.input(|i| i.stable_dt);
        let s = self.scroll_spring();
        s.set_target(s.target.clamp(0.0, max));
        if s.value > max {
            s.snap(max);
        }
        if reduced {
            let t = s.target;
            s.snap(t);
        } else if s.step(dt) {
            ctx.request_repaint();
        }
        let offset = s.value;

        let start_x = if overflow {
            lane.min.x - offset
        } else {
            lane.center().x - content * 0.5
        };
        let clip = lane.expand2(vec2(0.0, 8.0));
        let painter = ui.painter_at(clip);
        let entrance = if reduced {
            1.0
        } else {
            ((now - self.opened_at) as f32 / 0.5).clamp(0.0, 1.0)
        };
        let any_hovered = self.hovered.is_some();
        let mut hovered = None;
        let mut clicked = None;
        let mut drag_dx = lane_resp.drag_delta().x;

        let first = (((lane.min.x - start_x) / (THUMB.x + GAP)).floor().max(0.0)) as usize;
        let last = ((((lane.max.x - start_x) / (THUMB.x + GAP)).ceil()) as usize + 1).min(n);
        for i in first..last {
            let path = self.items[i].clone();
            let x = start_x + i as f32 * (THUMB.x + GAP);
            // Entrance: thumbnails slide in from the right one after another.
            let local = ((entrance - (i - first) as f32 * 0.06) / 0.6).clamp(0.0, 1.0);
            let enter = anim::ease_out_cubic(local);
            if enter < 1.0 {
                ctx.request_repaint();
            }
            let base = Rect::from_min_size(
                pos2(x + (1.0 - enter) * 48.0, lane.max.y - 2.0 - THUMB.y),
                THUMB,
            );
            let id = Id::new(("nook-wallpaper-thumb", &path));
            let resp = ui.interact(base.intersect(lane), id, Sense::click_and_drag());
            if resp.hovered() {
                hovered = Some(path.clone());
            }
            if resp.dragged() {
                drag_dx = resp.drag_delta().x;
            }
            if resp.clicked() {
                clicked = Some(path.clone());
            }
            let lift = anim::spring(
                &ctx,
                id.with("lift"),
                if resp.hovered() { 1.0 } else { 0.0 },
                SpringParams::with_overshoot(420.0, 0.12),
            );
            let press = anim::spring(
                &ctx,
                id.with("press"),
                if resp.is_pointer_button_down_on() {
                    1.0
                } else {
                    0.0
                },
                SpringParams::critical(900.0),
            );
            // Grow from the bottom edge and rise a little.
            let scale = 1.0 + 0.08 * lift - 0.04 * press;
            let size = THUMB * scale;
            let rect = Rect::from_min_size(
                pos2(
                    base.center().x - size.x * 0.5,
                    base.max.y - size.y - 4.0 * lift,
                ),
                size,
            );
            // Fade toward the strip's edges, and recede while another is hovered.
            let edge = if overflow {
                let l = (rect.center().x - lane.min.x) / EDGE_FADE;
                let r = (lane.max.x - rect.center().x) / EDGE_FADE;
                let k = l.min(r).clamp(0.0, 1.0);
                k * k * (3.0 - 2.0 * k)
            } else {
                1.0
            };
            let focus = if any_hovered { 0.62 + 0.38 * lift } else { 1.0 };
            let alpha = (edge * focus * enter).clamp(0.0, 1.0);
            if alpha <= 0.01 {
                continue;
            }
            self.request_thumb(&path);
            let radius = 10.0;
            painter.rect_filled(
                rect.translate(vec2(0.0, 2.0 + 2.0 * lift)).expand(1.0),
                radius + 1.0,
                Color32::from_black_alpha((70.0 * alpha) as u8),
            );
            match self.thumb(&path) {
                Some(tex) => {
                    let uv = cover_uv(tex.size_vec2(), rect.size(), 1.0);
                    painter.add(
                        RectShape::filled(rect, radius, Color32::WHITE.gamma_multiply(alpha))
                            .with_texture(tex.id(), uv),
                    );
                }
                None => {
                    painter.rect_filled(
                        rect,
                        radius,
                        Color32::from_white_alpha((28.0 * alpha) as u8),
                    );
                }
            }
            let is_current = self.current.as_deref().is_some_and(|c| same_file(c, &path));
            let ring = if is_current {
                Some((theme.accent, 2.0))
            } else if lift > 0.01 {
                Some((Color32::from_white_alpha((230.0 * lift) as u8), 1.5))
            } else {
                None
            };
            if let Some((color, width)) = ring {
                painter.rect_stroke(
                    rect.expand(1.5),
                    radius + 1.5,
                    egui::Stroke::new(width, with_alpha(color, alpha)),
                    egui::StrokeKind::Outside,
                );
            }
            if is_current {
                // A check badge that pops in when the wallpaper was just set.
                let pop = match &self.applied {
                    Some((p, at)) if same_file(p, &path) && !reduced => {
                        let k = ((now - at) as f32 / 0.35).clamp(0.0, 1.0);
                        if k < 1.0 {
                            ctx.request_repaint();
                        }
                        1.0 + 0.35 * (k * std::f32::consts::PI).sin()
                    }
                    _ => 1.0,
                };
                let c = rect.right_top() + vec2(-9.0, 9.0);
                painter.circle_filled(c, 7.0 * pop, with_alpha(theme.accent, alpha));
                paint::icon(
                    &painter,
                    c,
                    icons::CHECK,
                    9.0 * pop,
                    with_alpha(Color32::WHITE, alpha),
                    true,
                );
            }
            if self.applying.as_deref() == Some(path.as_path()) {
                let spin = (now * 6.0) as f32;
                ctx.request_repaint();
                painter.add(egui::Shape::circle_filled(
                    rect.center(),
                    13.0,
                    Color32::from_black_alpha(120),
                ));
                let p = rect.center() + vec2(spin.cos(), spin.sin()) * 7.0;
                painter.circle_filled(p, 2.5, Color32::WHITE);
            }
        }

        if drag_dx != 0.0 && overflow {
            // Follow the finger one to one while dragging.
            let s = self.scroll_spring();
            let v = (s.value - drag_dx).clamp(0.0, max);
            s.snap(v);
        }
        // Hold the hovered picture while the pointer is anywhere on the strip,
        // so crossing a gap does not flash back to the current wallpaper.
        if hovered.is_some() {
            self.hovered = hovered;
        } else if !pointer_in {
            self.hovered = None;
        }
        if let Some(path) = clicked {
            self.apply(path);
        }

        // Chevrons hint at more pictures off either edge.
        if overflow {
            let s = self.scroll_spring();
            let (at_start, at_end) = (s.value <= 1.0, s.value >= max - 1.0);
            for (show, glyph, x) in [
                (!at_start, icons::CARET_LEFT, lane.min.x - 2.0),
                (!at_end, icons::CARET_RIGHT, lane.max.x + 2.0),
            ] {
                let a = anim::tween(&ctx, Id::new(("nook-wallpaper-chevron", glyph)), show, 0.15);
                if a > 0.0 {
                    paint::icon(
                        ui.painter(),
                        pos2(x, lane.center().y),
                        glyph,
                        12.0,
                        Color32::from_white_alpha((200.0 * a) as u8),
                        true,
                    );
                }
            }
        }
    }

    fn current_index(&self) -> Option<usize> {
        let current = self.current.as_deref()?;
        self.items.iter().position(|p| same_file(p, current))
    }
}

impl Module for Wallpaper {
    fn id(&self) -> &'static str {
        "wallpaper"
    }

    fn name(&self) -> &str {
        "Wallpaper"
    }

    fn icon(&self) -> &'static str {
        icons::IMAGE
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn attach(&mut self, ctx: &Context) {
        let (tx, rx) = mpsc::channel();
        self.thumb_req = spawn_thumbnailer(
            "nook-wallpaper-thumbs",
            THUMB_PX,
            false,
            tx.clone(),
            ctx.clone(),
        );
        self.preview_req = spawn_thumbnailer(
            "nook-wallpaper-preview",
            PREVIEW_PX,
            true,
            tx.clone(),
            ctx.clone(),
        );
        self.tx = Some(tx);
        self.rx = Some(rx);
        self.ctx = Some(ctx.clone());
        self.current = system::wallpaper_path().filter(|p| p.exists());
        let saved = config_file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .map(|s| PathBuf::from(s.trim()))
            .filter(|p| p.is_dir());
        if let Some(folder) = saved {
            self.set_folder(folder);
        }
    }

    fn on_tick(&mut self) {
        let Some(rx) = &self.rx else {
            return;
        };
        let events: Vec<Event> = rx.try_iter().collect();
        let Some(ctx) = self.ctx.clone() else {
            return;
        };
        for event in events {
            match event {
                Event::Picked(dir) => {
                    self.picking = false;
                    if let Some(dir) = dir {
                        self.set_folder(dir);
                        self.save();
                        // The panel folded while the dialog was up; bring it
                        // back on this page.
                        self.show_page = true;
                    }
                }
                Event::Listed(dir, list) => {
                    if self.folder.as_deref() == Some(dir.as_path()) {
                        tracing::info!("wallpaper: {} pictures in {}", list.len(), dir.display());
                        self.items = list;
                        self.listing = false;
                        self.opened_at = ctx.input(|i| i.time);
                    }
                }
                Event::Thumb(path, img) => {
                    let tex = Self::upload(&ctx, &path, img);
                    self.thumbs.insert(path, Some(tex));
                }
                Event::Preview(path, img) => {
                    let tex = Self::upload(&ctx, &path, img);
                    self.previews.retain(|(p, _)| p != &path);
                    self.previews.push((path, tex));
                    if self.previews.len() > PREVIEW_CACHE {
                        drop(self.previews.remove(0));
                    }
                    self.preview_asked = None;
                }
                Event::Applied(path, result) => {
                    self.applying = None;
                    match result {
                        Ok(()) => {
                            tracing::info!("wallpaper: set {}", path.display());
                            self.applied = Some((path.clone(), ctx.input(|i| i.time)));
                            self.current = Some(path);
                        }
                        Err(e) => {
                            tracing::warn!("wallpaper: set {}: {e}", path.display());
                            self.error = Some("Could not set the wallpaper".into());
                        }
                    }
                }
            }
        }
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.open_page)
    }

    fn take_show_request(&mut self) -> bool {
        std::mem::take(&mut self.show_page)
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        let rect = ui.max_rect();
        let resp = ui.interact(
            rect.expand(8.0),
            ui.id().with("wallpaper-tile"),
            Sense::click(),
        );
        // Until a folder is chosen the tile keeps its plain icon.
        let current = self.folder.as_ref().and(self.current.clone());
        let tex = current.as_deref().and_then(|p| {
            self.request_thumb(p);
            self.thumb(p).cloned()
        });
        match tex {
            Some(tex) => {
                let grow = anim::spring(
                    ui.ctx(),
                    resp.id.with("grow"),
                    if ctx.hovered { 1.0 } else { 0.0 },
                    SpringParams::with_overshoot(420.0, 0.1),
                );
                // A bigger tile shows the whole picture; a small one a swatch.
                let base = if rect.width() > 44.0 {
                    rect
                } else {
                    Rect::from_center_size(rect.center(), vec2(34.0, 34.0))
                };
                let r = base.expand(2.0 * grow);
                image_cover(ui, r, &tex, 9.0, 1.0 + 0.1 * grow, 1.0);
                ui.painter().rect_stroke(
                    r,
                    9.0,
                    egui::Stroke::new(1.0, theme.tile_rim),
                    egui::StrokeKind::Inside,
                );
            }
            None => paint::icon(
                ui.painter(),
                rect.center(),
                icons::IMAGE,
                SIZE_ICON + 2.0,
                if ctx.hovered {
                    theme.text
                } else {
                    theme.text_dim
                },
                false,
            ),
        }
        if resp.clicked() || resp.secondary_clicked() {
            self.open_page = true;
        }
        resp.on_hover_text("Browse and set your wallpaper");
    }

    fn ui_page(&mut self, ui: &mut Ui, header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        if widgets::icon_button(header, icons::FOLDER_OPEN, 26.0, self.picking, theme)
            .on_hover_text(match &self.folder {
                Some(f) => format!("{}\nClick to choose another folder", f.display()),
                None => "Choose your wallpaper folder".to_owned(),
            })
            .clicked()
        {
            self.pick_folder();
        }
        if self.folder.is_some()
            && widgets::icon_button(header, icons::ARROWS_CLOCKWISE, 26.0, false, theme)
                .on_hover_text("Look for new pictures")
                .clicked()
            && let Some(folder) = self.folder.clone()
        {
            self.set_folder(folder);
        }
        if !self.items.is_empty() {
            header.label(
                egui::RichText::new(format!("{}", self.items.len()))
                    .font(Theme::font(SIZE_LABEL))
                    .color(theme.text_faint),
            );
        }
        self.page_body(ui, body, theme);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_pictures() {
        assert!(is_picture(Path::new("C:\\a\\b.JPG")));
        assert!(is_picture(Path::new("x.webp")));
        assert!(!is_picture(Path::new("x.txt")));
        assert!(!is_picture(Path::new("jpg")));
    }

    #[test]
    fn cover_crops_the_long_side() {
        // A wide image in a square: crop left and right.
        let uv = cover_uv(vec2(200.0, 100.0), vec2(50.0, 50.0), 1.0);
        assert!((uv.width() - 0.5).abs() < 1e-5 && (uv.height() - 1.0).abs() < 1e-5);
        assert!((uv.center().x - 0.5).abs() < 1e-5);
        // A tall image in a wide area: crop top and bottom.
        let uv = cover_uv(vec2(100.0, 200.0), vec2(100.0, 50.0), 1.0);
        assert!((uv.width() - 1.0).abs() < 1e-5 && (uv.height() - 0.25).abs() < 1e-5);
        // Zoom shrinks the window around the center.
        let uv = cover_uv(vec2(100.0, 100.0), vec2(100.0, 100.0), 2.0);
        assert!((uv.width() - 0.5).abs() < 1e-5 && (uv.center().y - 0.5).abs() < 1e-5);
    }

    #[test]
    fn lists_pictures_sorted_and_nested() {
        let dir = std::env::temp_dir().join(format!("nook-wallpaper-test-{}", std::process::id()));
        let sub = dir.join("sub");
        std::fs::create_dir_all(&sub).expect("temp dir");
        for f in [
            dir.join("b.png"),
            dir.join("A.jpg"),
            dir.join("notes.txt"),
            sub.join("c.webp"),
        ] {
            std::fs::write(&f, b"x").expect("write");
        }
        let names: Vec<String> = list_pictures(&dir, 2)
            .iter()
            .map(|p| {
                p.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert_eq!(names, ["A.jpg", "b.png", "c.webp"]);
        assert_eq!(list_pictures(&dir, 0).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
