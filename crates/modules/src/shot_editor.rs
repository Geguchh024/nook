//! The screenshot editor: a normal window (taskbar, Alt+Tab, resizable)
//! showing the capture with annotation tools on top.
//!
//! Tools: arrow, box, pen, marker, text and pixelate (to hide what should
//! not be shared), in six colors and three sizes, with undo. Ctrl+C copies,
//! Ctrl+S saves a PNG to the Screenshots folder, Enter copies and closes,
//! Esc closes. Number keys 1 to 6 pick the tools.
//!
//! Shapes live in image pixels. While editing they are drawn with egui on
//! top of the image texture; copying or saving burns them into the pixels
//! with `nook_platform::raster`, in the order they were drawn, so the file
//! matches what you saw.

use egui::{
    Align, Align2, Color32, ColorImage, FontId, Key, Layout, Pos2, Rect, Sense, Stroke,
    TextureHandle, TextureOptions, Ui, ViewportBuilder, ViewportClass, ViewportId, pos2, vec2,
};
use nook_platform::capture::Frame;
use nook_platform::{clipboard, raster, shell, system};
use nook_ui::theme::{SIZE_LABEL, Theme, mix};
use nook_ui::{icons, widgets};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

const COLORS: [[u8; 3]; 6] = [
    [255, 59, 48],
    [255, 204, 0],
    [52, 199, 89],
    [10, 132, 255],
    [255, 255, 255],
    [20, 20, 24],
];
/// Stroke size multipliers for S, M, L.
const SIZES: [f32; 3] = [1.0, 1.75, 3.0];
const TOOLBAR_H: f32 = 52.0;
/// How long a status message stays.
const STATUS_FOR: Duration = Duration::from_secs(4);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Arrow,
    Rect,
    Pen,
    Marker,
    Text,
    Pixelate,
}

impl Tool {
    const ALL: [Tool; 6] = [
        Tool::Arrow,
        Tool::Rect,
        Tool::Pen,
        Tool::Marker,
        Tool::Text,
        Tool::Pixelate,
    ];

    fn icon(self) -> &'static str {
        match self {
            Tool::Arrow => icons::ARROW_UP_RIGHT,
            Tool::Rect => icons::SQUARE,
            Tool::Pen => icons::PEN,
            Tool::Marker => icons::HIGHLIGHTER,
            Tool::Text => icons::TEXT_T,
            Tool::Pixelate => icons::GRID_FOUR,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Tool::Arrow => "Arrow (1)",
            Tool::Rect => "Box (2)",
            Tool::Pen => "Pen (3)",
            Tool::Marker => "Marker (4)",
            Tool::Text => "Text (5)",
            Tool::Pixelate => "Pixelate (6)",
        }
    }

    /// Stroke width (or text size) in image pixels at size multiplier 1.
    fn base(self) -> f32 {
        match self {
            Tool::Arrow => 5.0,
            Tool::Rect | Tool::Pen => 4.0,
            Tool::Marker => 18.0,
            Tool::Text => 20.0,
            Tool::Pixelate => 0.0,
        }
    }
}

type P = (f32, f32);

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Arrow {
        a: P,
        b: P,
        rgb: [u8; 3],
        width: f32,
    },
    Rect {
        a: P,
        b: P,
        rgb: [u8; 3],
        width: f32,
    },
    Pen {
        points: Vec<P>,
        rgb: [u8; 3],
        width: f32,
    },
    Marker {
        points: Vec<P>,
        rgb: [u8; 3],
        width: f32,
    },
    Text {
        pos: P,
        text: String,
        rgb: [u8; 3],
        size: f32,
    },
    /// Blocks of `block` pixels filling `x0, y0` to `x1, y1`, `cols` per
    /// row, averaged from the original capture.
    Pixelate {
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        block: i32,
        cols: i32,
        cells: Vec<[u8; 3]>,
    },
}

/// The two lines of an arrow head at `b`, pointing away from `a`.
fn arrow_head(a: P, b: P, width: f32) -> [P; 3] {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let (ux, uy) = (dx / len, dy / len);
    let head = (width * 3.5).max(12.0).min(len * 0.6);
    let (s, c) = 28f32.to_radians().sin_cos();
    let left = (
        b.0 - head * (ux * c - uy * s),
        b.1 - head * (uy * c + ux * s),
    );
    let right = (
        b.0 - head * (ux * c + uy * s),
        b.1 - head * (uy * c - ux * s),
    );
    [left, b, right]
}

fn rect_points(a: P, b: P) -> [P; 5] {
    [a, (b.0, a.1), b, (a.0, b.1), a]
}

/// Averages `frame` in blocks over the rectangle between `a` and `b`.
fn pixelate(frame: &Frame, a: P, b: P) -> Option<Shape> {
    let (w, h) = (frame.width as i32, frame.height as i32);
    let x0 = (a.0.min(b.0).floor() as i32).clamp(0, w);
    let y0 = (a.1.min(b.1).floor() as i32).clamp(0, h);
    let x1 = (a.0.max(b.0).ceil() as i32).clamp(0, w);
    let y1 = (a.1.max(b.1).ceil() as i32).clamp(0, h);
    if x1 - x0 < 3 || y1 - y0 < 3 {
        return None;
    }
    let block = (w.min(h) / 60).clamp(8, 24);
    let cols = (x1 - x0 + block - 1) / block;
    let rows = (y1 - y0 + block - 1) / block;
    let mut cells = Vec::with_capacity((cols * rows) as usize);
    for r in 0..rows {
        for c in 0..cols {
            let bx = x0 + c * block;
            let by = y0 + r * block;
            cells.push(raster::average(
                &frame.pixels,
                frame.width,
                frame.height,
                (bx, by, (bx + block).min(x1), (by + block).min(y1)),
            ));
        }
    }
    Some(Shape::Pixelate {
        x0,
        y0,
        x1,
        y1,
        block,
        cols,
        cells,
    })
}

/// Burns `shapes` into a copy of `frame`.
pub fn render(frame: &Frame, shapes: &[Shape]) -> Frame {
    let mut out = frame.clone();
    let mut canvas = raster::Canvas {
        pixels: &mut out.pixels,
        width: frame.width,
        height: frame.height,
    };
    for s in shapes {
        match s {
            Shape::Arrow { a, b, rgb, width } => {
                canvas.stroke(&[*a, *b], *width, *rgb, 1.0);
                canvas.stroke(&arrow_head(*a, *b, *width), *width, *rgb, 1.0);
            }
            Shape::Rect { a, b, rgb, width } => {
                canvas.stroke(&rect_points(*a, *b), *width, *rgb, 1.0);
            }
            Shape::Pen { points, rgb, width } => canvas.stroke(points, *width, *rgb, 1.0),
            Shape::Marker { points, rgb, width } => canvas.stroke(points, *width, *rgb, 0.4),
            Shape::Text {
                pos,
                text,
                rgb,
                size,
            } => {
                if let Err(e) = canvas.text(*pos, text, *size, *rgb) {
                    tracing::warn!("screenshot text: {e}");
                }
            }
            Shape::Pixelate {
                x0,
                y0,
                x1,
                y1,
                block,
                cols,
                cells,
            } => {
                for (i, rgb) in cells.iter().enumerate() {
                    let (c, r) = (i as i32 % cols, i as i32 / cols);
                    let (bx, by) = (x0 + c * block, y0 + r * block);
                    canvas.fill((bx, by, (bx + block).min(*x1), (by + block).min(*y1)), *rgb);
                }
            }
        }
    }
    out
}

/// Saves `frame` as a PNG in the Screenshots folder; returns the path.
pub fn save(frame: &Frame) -> nook_platform::Result<PathBuf> {
    let dir = shell::screenshots_dir()
        .ok_or_else(|| nook_platform::PlatformError::Other("no Screenshots folder".into()))?;
    std::fs::create_dir_all(&dir).map_err(|e| {
        nook_platform::PlatformError::Other(format!("creating {}: {e}", dir.display()))
    })?;
    let stamp = system::file_timestamp();
    let mut path = dir.join(format!("Screenshot {stamp}.png"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("Screenshot {stamp} ({n}).png"));
        n += 1;
    }
    let png = frame.encode_png()?;
    std::fs::write(&path, png).map_err(|e| {
        nook_platform::PlatformError::Other(format!("writing {}: {e}", path.display()))
    })?;
    Ok(path)
}

enum Status {
    Copied,
    Saved(PathBuf),
    Failed(String),
}

struct TextDraft {
    pos: P,
    text: String,
    focused: bool,
}

pub struct Editor {
    id: u64,
    frame: Arc<Frame>,
    texture: Option<TextureHandle>,
    shapes: Vec<Shape>,
    draft: Option<Shape>,
    text: Option<TextDraft>,
    tool: Tool,
    color: usize,
    size: usize,
    open: bool,
    /// Initial window size (points), fixed so egui does not resize it.
    initial: egui::Vec2,
    status: Option<(Status, Instant)>,
    results: (Sender<Status>, Receiver<Status>),
    close_after_copy: bool,
}

impl Editor {
    /// An editor for `frame`, sized to fit within `room` (points).
    pub fn new(id: u64, frame: Arc<Frame>, room: egui::Vec2, ppp: f32) -> Self {
        let img = vec2(frame.width as f32, frame.height as f32) / ppp;
        let fit = ((room.x - 48.0) / img.x)
            .min((room.y - TOOLBAR_H - 48.0) / img.y)
            .min(1.0);
        let initial = vec2(
            (img.x * fit + 48.0).max(640.0),
            (img.y * fit + TOOLBAR_H + 48.0).max(360.0),
        );
        Self {
            id,
            frame,
            texture: None,
            shapes: Vec::new(),
            draft: None,
            text: None,
            tool: Tool::default(),
            color: 0,
            size: 1,
            open: true,
            initial,
            status: None,
            results: mpsc::channel(),
            close_after_copy: false,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    fn rgb(&self) -> [u8; 3] {
        COLORS[self.color]
    }

    fn width(&self) -> f32 {
        self.tool.base() * SIZES[self.size]
    }

    fn commit_text(&mut self) {
        if let Some(t) = self.text.take()
            && !t.text.trim().is_empty()
        {
            self.shapes.push(Shape::Text {
                pos: t.pos,
                text: t.text,
                rgb: self.rgb(),
                size: Tool::Text.base() * SIZES[self.size],
            });
        }
    }

    /// Renders and copies or saves on a worker thread.
    fn export(&mut self, save_file: bool, ctx: &egui::Context) {
        self.commit_text();
        let frame = self.frame.clone();
        let shapes = self.shapes.clone();
        let tx = self.results.0.clone();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("nook-shot-export".into())
            .spawn(move || {
                let out = render(&frame, &shapes);
                let status = if save_file {
                    match save(&out) {
                        Ok(path) => {
                            let _ = clipboard::copy_image(&out);
                            Status::Saved(path)
                        }
                        Err(e) => Status::Failed(e.to_string()),
                    }
                } else {
                    match clipboard::copy_image(&out) {
                        Ok(()) => Status::Copied,
                        Err(e) => Status::Failed(e.to_string()),
                    }
                };
                let _ = tx.send(status);
                ctx.request_repaint();
            });
        if let Err(e) = spawned {
            self.status = Some((Status::Failed(e.to_string()), Instant::now()));
        }
    }

    /// Shows the editor window. Call every frame while open.
    pub fn show(&mut self, ctx: &egui::Context, theme: &Theme) {
        if !self.open {
            return;
        }
        while let Ok(s) = self.results.1.try_recv() {
            let ok = !matches!(s, Status::Failed(_));
            self.status = Some((s, Instant::now()));
            if ok && self.close_after_copy {
                self.open = false;
                return;
            }
            self.close_after_copy = false;
        }
        let builder = ViewportBuilder::default()
            .with_title("Screenshot - Nook")
            .with_inner_size(self.initial)
            .with_min_inner_size([560.0, 320.0])
            .with_resizable(true)
            .with_decorations(true)
            .with_taskbar(true)
            .with_active(true);
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of(("nook-shot-editor", self.id)),
            builder,
            |ui, class| {
                if class == ViewportClass::EmbeddedWindow {
                    return;
                }
                if ui.input(|i| i.viewport().close_requested()) {
                    self.open = false;
                    return;
                }
                self.ui(ui, theme);
            },
        );
    }

    fn ui(&mut self, ui: &mut Ui, theme: &Theme) {
        let ctx = ui.ctx().clone();
        let bg = if theme.dark {
            Color32::from_rgb(24, 24, 28)
        } else {
            Color32::from_rgb(238, 238, 242)
        };
        self.shortcuts(&ctx);
        if !self.open {
            return;
        }
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(bg))
            .show(ui, |ui| {
                let full = ui.max_rect();
                let bar = Rect::from_min_size(full.min, vec2(full.width(), TOOLBAR_H));
                let mut bui = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(bar.shrink2(vec2(12.0, 0.0)))
                        .layout(Layout::left_to_right(Align::Center)),
                );
                self.toolbar(&mut bui, theme);
                let canvas = Rect::from_min_max(pos2(full.min.x, bar.max.y), full.max);
                self.canvas(ui, canvas, theme);
            });
        if self.status.is_some() || self.text.is_some() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let typing = self.text.is_some();
        let (undo, copy, save, enter, esc, digit) = ctx.input(|i| {
            let digit = [
                Key::Num1,
                Key::Num2,
                Key::Num3,
                Key::Num4,
                Key::Num5,
                Key::Num6,
            ]
            .iter()
            .position(|k| i.key_pressed(*k) && !i.modifiers.any());
            (
                i.modifiers.command && i.key_pressed(Key::Z),
                i.modifiers.command && i.key_pressed(Key::C),
                i.modifiers.command && i.key_pressed(Key::S),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
                digit,
            )
        });
        if typing {
            if esc {
                self.text = None;
            }
            return;
        }
        if undo {
            self.shapes.pop();
        }
        if copy {
            self.export(false, ctx);
        }
        if save {
            self.export(true, ctx);
        }
        if enter {
            self.close_after_copy = true;
            self.export(false, ctx);
        }
        if esc {
            self.open = false;
        }
        if let Some(d) = digit {
            self.tool = Tool::ALL[d];
        }
    }

    fn toolbar(&mut self, ui: &mut Ui, theme: &Theme) {
        ui.spacing_mut().item_spacing.x = 4.0;
        for tool in Tool::ALL {
            if widgets::icon_button(ui, tool.icon(), 32.0, self.tool == tool, theme)
                .on_hover_text(tool.label())
                .clicked()
            {
                self.commit_text();
                self.tool = tool;
            }
        }
        ui.add_space(10.0);
        for (i, c) in COLORS.iter().enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 32.0), Sense::click());
            let color = Color32::from_rgb(c[0], c[1], c[2]);
            let selected = self.color == i;
            if selected {
                ui.painter()
                    .circle_stroke(rect.center(), 11.0, Stroke::new(2.0, theme.text));
            }
            let r = if resp.hovered() || selected { 8.0 } else { 7.0 };
            ui.painter().circle(
                rect.center(),
                r,
                color,
                Stroke::new(1.0, mix(color, theme.text_faint, 0.5)),
            );
            if resp.clicked() {
                self.color = i;
            }
        }
        ui.add_space(10.0);
        widgets::segmented(ui, &["S", "M", "L"], &mut self.size, theme).on_hover_text("Size");
        ui.add_space(6.0);
        if widgets::icon_button(ui, icons::ARROW_U_UP_LEFT, 32.0, false, theme)
            .on_hover_text("Undo (Ctrl+Z)")
            .clicked()
        {
            self.text = None;
            self.shapes.pop();
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let ctx = ui.ctx().clone();
            if widgets::icon_button(ui, icons::CHECK, 32.0, true, theme)
                .on_hover_text("Copy and close (Enter)")
                .clicked()
            {
                self.close_after_copy = true;
                self.export(false, &ctx);
            }
            if widgets::icon_button(ui, icons::FLOPPY_DISK, 32.0, false, theme)
                .on_hover_text("Save to Screenshots (Ctrl+S)")
                .clicked()
            {
                self.export(true, &ctx);
            }
            if widgets::icon_button(ui, icons::COPY, 32.0, false, theme)
                .on_hover_text("Copy (Ctrl+C)")
                .clicked()
            {
                self.export(false, &ctx);
            }
            self.status_label(ui, theme);
        });
    }

    fn status_label(&mut self, ui: &mut Ui, theme: &Theme) {
        if self
            .status
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() > STATUS_FOR)
        {
            self.status = None;
        }
        let Some((status, _)) = &self.status else {
            return;
        };
        let font = Theme::font(SIZE_LABEL + 1.0);
        match status {
            Status::Copied => {
                ui.label(
                    egui::RichText::new("Copied")
                        .font(font)
                        .color(theme.accent_fg()),
                );
            }
            Status::Saved(path) => {
                let resp = ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new("Saved · Show")
                                .font(font)
                                .color(theme.accent_fg()),
                        )
                        .sense(Sense::click()),
                    )
                    .on_hover_text(path.display().to_string());
                if resp.clicked()
                    && let Err(e) = shell::reveal(path)
                {
                    tracing::warn!("show screenshot: {e}");
                }
            }
            Status::Failed(e) => {
                ui.label(
                    egui::RichText::new("Failed")
                        .font(font)
                        .color(theme.text_dim),
                )
                .on_hover_text(e.as_str());
            }
        }
    }

    fn canvas(&mut self, ui: &mut Ui, area: Rect, theme: &Theme) {
        let ctx = ui.ctx().clone();
        let ppp = ctx.pixels_per_point();
        let frame = self.frame.clone();
        let texture = self.texture.get_or_insert_with(|| {
            let rgba: Vec<u8> = frame
                .pixels
                .iter()
                .flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8, 255])
                .collect();
            let image = ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &rgba,
            );
            ctx.load_texture(
                format!("nook-shot-{}", self.id),
                image,
                TextureOptions::LINEAR,
            )
        });
        let room = area.shrink(20.0);
        let img = vec2(frame.width as f32, frame.height as f32);
        // Points per image pixel: fit, but never past 1:1 physical pixels.
        let scale = (room.width() / img.x)
            .min(room.height() / img.y)
            .min(1.0 / ppp);
        let shown = Rect::from_center_size(room.center(), img * scale);
        ui.painter()
            .rect_filled(shown.expand(1.0), 2.0, theme.tile_rim);
        ui.painter().image(
            texture.id(),
            shown,
            Rect::from_min_max(Pos2::ZERO, pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        let to_img = |p: Pos2| ((p.x - shown.min.x) / scale, (p.y - shown.min.y) / scale);
        let to_screen = |p: P| pos2(shown.min.x + p.0 * scale, shown.min.y + p.1 * scale);

        let resp = ui.interact(area, ui.id().with("shot-canvas"), Sense::click_and_drag());
        if resp.hovered() {
            ctx.set_cursor_icon(if self.tool == Tool::Text {
                egui::CursorIcon::Text
            } else {
                egui::CursorIcon::Crosshair
            });
        }
        let rgb = self.rgb();
        let width = self.width();
        if let Some(p) = resp.interact_pointer_pos() {
            let ip = to_img(p);
            if resp.drag_started() && self.tool != Tool::Text {
                self.commit_text();
                self.draft = Some(match self.tool {
                    Tool::Arrow => Shape::Arrow {
                        a: ip,
                        b: ip,
                        rgb,
                        width,
                    },
                    Tool::Rect | Tool::Pixelate => Shape::Rect {
                        a: ip,
                        b: ip,
                        rgb,
                        width,
                    },
                    Tool::Pen => Shape::Pen {
                        points: vec![ip],
                        rgb,
                        width,
                    },
                    Tool::Marker | Tool::Text => Shape::Marker {
                        points: vec![ip],
                        rgb,
                        width,
                    },
                });
            } else if resp.dragged() {
                match &mut self.draft {
                    Some(Shape::Arrow { b, .. } | Shape::Rect { b, .. }) => *b = ip,
                    Some(Shape::Pen { points, .. } | Shape::Marker { points, .. }) => {
                        let last = points.last().copied().unwrap_or(ip);
                        if (last.0 - ip.0).abs() + (last.1 - ip.1).abs() >= 1.0 {
                            points.push(ip);
                        }
                    }
                    _ => {}
                }
            }
            if resp.clicked() && self.tool == Tool::Text {
                self.commit_text();
                self.text = Some(TextDraft {
                    pos: ip,
                    text: String::new(),
                    focused: false,
                });
            }
        }
        if resp.drag_stopped()
            && let Some(draft) = self.draft.take()
        {
            let big = |a: P, b: P| (a.0 - b.0).abs().max((a.1 - b.1).abs()) >= 3.0;
            let done = match draft {
                Shape::Rect { a, b, .. } if self.tool == Tool::Pixelate => pixelate(&frame, a, b),
                Shape::Arrow { a, b, .. } | Shape::Rect { a, b, .. } if !big(a, b) => None,
                other => Some(other),
            };
            if let Some(s) = done {
                self.shapes.push(s);
            }
        }

        let painter = ui.painter_at(shown.expand(2.0));
        for s in &self.shapes {
            paint_shape(&painter, s, scale, &to_screen, false);
        }
        if let Some(d) = &self.draft {
            // A pixelate drag is a plain outline until it is let go.
            paint_shape(&painter, d, scale, &to_screen, self.tool == Tool::Pixelate);
        }
        self.text_draft(ui, scale, &to_screen);
    }

    fn text_draft(&mut self, ui: &mut Ui, scale: f32, to_screen: &impl Fn(P) -> Pos2) {
        let rgb = self.rgb();
        let size = Tool::Text.base() * SIZES[self.size];
        let Some(draft) = &mut self.text else {
            return;
        };
        let at = to_screen(draft.pos);
        let font = FontId::new(
            size * scale,
            egui::FontFamily::Name(nook_ui::theme::FAMILY_SEMIBOLD.into()),
        );
        let rect = Rect::from_min_size(at, vec2(600.0, size * scale * 1.5));
        let edit = egui::TextEdit::singleline(&mut draft.text)
            .id(ui.id().with(("shot-text", self.id)))
            .frame(egui::Frame::NONE)
            .font(font)
            .text_color(Color32::from_rgb(rgb[0], rgb[1], rgb[2]))
            .hint_text("Type, then Enter")
            .margin(egui::Margin::ZERO)
            .desired_width(600.0);
        let resp = ui.put(rect, edit);
        if !draft.focused {
            resp.request_focus();
            draft.focused = true;
        }
        let enter = ui.input(|i| i.key_pressed(Key::Enter));
        if resp.lost_focus() || enter {
            self.commit_text();
        }
    }
}

/// Draws a shape with egui (image pixels mapped through `to_screen`).
fn paint_shape(
    painter: &egui::Painter,
    shape: &Shape,
    scale: f32,
    to_screen: &impl Fn(P) -> Pos2,
    pixelating: bool,
) {
    let c = |rgb: &[u8; 3]| Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
    let poly = |pts: &[P], width: f32, color: Color32, caps: bool| {
        let w = (width * scale).max(1.0);
        let screen: Vec<Pos2> = pts.iter().map(|p| to_screen(*p)).collect();
        painter.add(egui::Shape::line(screen.clone(), Stroke::new(w, color)));
        if caps {
            for p in screen {
                painter.circle_filled(p, w * 0.5, color);
            }
        }
    };
    match shape {
        Shape::Arrow { a, b, rgb, width } => {
            poly(&[*a, *b], *width, c(rgb), true);
            poly(&arrow_head(*a, *b, *width), *width, c(rgb), true);
        }
        Shape::Rect { a, b, rgb, width } if !pixelating => {
            let pts: Vec<Pos2> = rect_points(*a, *b)[..4]
                .iter()
                .map(|p| to_screen(*p))
                .collect();
            painter.add(egui::Shape::closed_line(
                pts,
                Stroke::new((width * scale).max(1.0), c(rgb)),
            ));
        }
        Shape::Rect { a, b, .. } => {
            // The pixelate tool's drag rectangle.
            let r = Rect::from_two_pos(to_screen(*a), to_screen(*b));
            painter.rect_stroke(
                r,
                0.0,
                Stroke::new(1.5, Color32::WHITE),
                egui::StrokeKind::Middle,
            );
        }
        Shape::Pen { points, rgb, width } => poly(points, *width, c(rgb), true),
        Shape::Marker { points, rgb, width } => {
            let [r, g, b] = *rgb;
            poly(
                points,
                *width,
                Color32::from_rgba_unmultiplied(r, g, b, 102),
                false,
            );
        }
        Shape::Text {
            pos,
            text,
            rgb,
            size,
        } => {
            painter.text(
                to_screen(*pos),
                Align2::LEFT_TOP,
                text,
                FontId::new(
                    size * scale,
                    egui::FontFamily::Name(nook_ui::theme::FAMILY_SEMIBOLD.into()),
                ),
                c(rgb),
            );
        }
        Shape::Pixelate {
            x0,
            y0,
            x1,
            y1,
            block,
            cols,
            cells,
        } => {
            for (i, rgb) in cells.iter().enumerate() {
                let (cx, cy) = (i as i32 % cols, i as i32 / cols);
                let (bx, by) = (x0 + cx * block, y0 + cy * block);
                let r = Rect::from_min_max(
                    to_screen((bx as f32, by as f32)),
                    to_screen(((bx + block).min(*x1) as f32, (by + block).min(*y1) as f32)),
                );
                painter.rect_filled(r, 0.0, c(rgb));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32, p: u32) -> Frame {
        Frame {
            origin: (0, 0),
            width: w,
            height: h,
            pixels: vec![p; (w * h) as usize],
        }
    }

    #[test]
    fn arrow_head_points_back_along_the_shaft() {
        let [l, tip, r] = arrow_head((0.0, 0.0), (100.0, 0.0), 4.0);
        assert_eq!(tip, (100.0, 0.0));
        assert!(l.0 < 100.0 && r.0 < 100.0);
        assert!((l.1 + r.1).abs() < 0.001, "symmetric");
    }

    #[test]
    fn pixelate_averages_blocks() {
        let f = frame(64, 64, 0xFF10_2030);
        let Some(Shape::Pixelate {
            cols, cells, block, ..
        }) = pixelate(&f, (0.0, 0.0), (20.0, 10.0))
        else {
            panic!("expected pixelate");
        };
        assert_eq!(block, 8);
        assert_eq!(cols, 3);
        assert_eq!(cells.len(), 6);
        assert!(cells.iter().all(|c| *c == [0x10, 0x20, 0x30]));
        assert!(pixelate(&f, (5.0, 5.0), (6.0, 6.0)).is_none());
    }

    #[test]
    fn render_burns_shapes_in_order() {
        let f = frame(40, 40, 0xFF00_0000);
        let shapes = vec![
            Shape::Rect {
                a: (5.0, 5.0),
                b: (30.0, 30.0),
                rgb: [255, 0, 0],
                width: 3.0,
            },
            Shape::Pixelate {
                x0: 0,
                y0: 0,
                x1: 8,
                y1: 8,
                block: 8,
                cols: 1,
                cells: vec![[0, 0, 255]],
            },
        ];
        let out = render(&f, &shapes);
        // Pixelate drawn last covers the box corner.
        assert_eq!(out.pixels[5 * 40 + 5], 0xFF00_00FF);
        // The box edge elsewhere is red.
        assert_eq!(out.pixels[30 * 40 + 20], 0xFFFF_0000);
        // The original is untouched.
        assert!(f.pixels.iter().all(|&p| p == 0xFF00_0000));
    }
}
