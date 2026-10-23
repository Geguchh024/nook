//! Draw on screen: a transparent ink layer over every monitor, for circling
//! and marking things on top of whatever is on screen. Pen, highlighter and
//! eraser in five colors, with undo and clear. The toolbar opens on the
//! monitor under the cursor.
//!
//! Open it from the tile or with Ctrl+Alt+D; Esc closes it and the ink goes
//! away. Ctrl+Z undoes the last stroke.
//!
//! The canvas is native (`nook_platform::ink`, a per-pixel-alpha layered
//! window), because a second OpenGL window does not composite transparently.
//! The toolbar is a small egui window owned by the canvas, so it always
//! floats above the ink; drag it by its grip to move it out of the way.

use egui::{
    Color32, Key, Pos2, Rect, Sense, Ui, UiBuilder, ViewportBuilder, ViewportClass,
    ViewportCommand, ViewportId, WindowLevel, pos2, vec2,
};
use nook_platform::ink::{InkCanvas, InkTool};
use nook_platform::{Hwnd, monitor, window};
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_ICON, SIZE_LABEL, Theme, mix};
use nook_ui::{icons, paint, widgets};

const TOOLBAR_TITLE: &str = "Nook Draw";
const TOOLBAR: egui::Vec2 = egui::vec2(404.0, 48.0);

const COLORS: [Color32; 5] = [
    Color32::from_rgb(255, 69, 58),
    Color32::from_rgb(255, 214, 10),
    Color32::from_rgb(48, 209, 88),
    Color32::from_rgb(10, 132, 255),
    Color32::WHITE,
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Tool {
    #[default]
    Pen,
    Highlighter,
    Eraser,
}

#[derive(Default)]
pub struct Draw {
    canvas: Option<InkCanvas>,
    tool: Tool,
    color: usize,
    /// The monitor the toolbar opened on.
    toolbar_monitor: Option<monitor::Monitor>,
    /// The toolbar window once found and styled.
    toolbar: Option<Hwnd>,
    repaint: Option<egui::Context>,
}

/// An opaque take on the panel glass for the toolbar window, which cannot
/// be translucent.
fn solid_bg(theme: &Theme) -> Color32 {
    let base = if theme.dark {
        Color32::from_rgb(28, 28, 33)
    } else {
        Color32::from_rgb(246, 246, 249)
    };
    mix(base, theme.accent, 0.04)
}

fn button_at(
    ui: &mut Ui,
    center: Pos2,
    size: f32,
    glyph: &'static str,
    active: bool,
    theme: &Theme,
) -> egui::Response {
    let rect = Rect::from_center_size(center, vec2(size, size));
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(glyph));
    widgets::icon_button(&mut child, glyph, size, active, theme)
}

/// Where the toolbar starts on `m`: bottom center, physical pixels
/// (x, y, width, height) at the monitor's scale.
fn toolbar_rect(m: &monitor::Monitor) -> (i32, i32, i32, i32) {
    let scale = m.dpi as f32 / 96.0;
    let (w, h) = (
        (TOOLBAR.x * scale).round() as i32,
        (TOOLBAR.y * scale).round() as i32,
    );
    let b = m.bounds;
    let x = b.left + (b.width() - w) / 2;
    let y = b.bottom - h - (72.0 * scale).round() as i32;
    (x, y, w, h)
}

impl Draw {
    fn ink_tool(&self) -> InkTool {
        let [r, g, b, _] = COLORS[self.color].to_array();
        match self.tool {
            Tool::Pen => InkTool::Pen {
                color: [r, g, b, 255],
                width: 4.0,
            },
            Tool::Highlighter => InkTool::Pen {
                color: [r, g, b, 90],
                width: 18.0,
            },
            Tool::Eraser => InkTool::Eraser,
        }
    }

    fn is_open(&self) -> bool {
        self.canvas.as_ref().is_some_and(InkCanvas::is_open)
    }

    fn set_open(&mut self, open: bool) {
        if open == self.is_open() {
            return;
        }
        self.toolbar = None;
        if !open {
            self.canvas = None;
            tracing::info!("draw on screen: closed");
            return;
        }
        let (Ok(b), Ok(here)) = (
            monitor::virtual_bounds(),
            monitor::under_cursor().or_else(|_| monitor::primary()),
        ) else {
            tracing::warn!("draw on screen: no monitor");
            return;
        };
        let repaint = self.repaint.clone();
        match InkCanvas::open(
            (b.left, b.top, b.width(), b.height()),
            self.ink_tool(),
            move || {
                if let Some(ctx) = &repaint {
                    ctx.request_repaint();
                }
            },
        ) {
            Ok(c) => {
                self.canvas = Some(c);
                self.toolbar_monitor = Some(here);
                tracing::info!("draw on screen: open");
            }
            Err(e) => tracing::warn!("draw on screen: {e}"),
        }
    }

    fn apply_tool(&self) {
        if let Some(c) = &self.canvas {
            c.set_tool(self.ink_tool());
        }
    }

    /// Styles the toolbar window once it exists: frameless, rounded, owned
    /// by the canvas so it stays above the ink.
    fn sync_toolbar(&mut self) {
        if self.toolbar.is_some() {
            return;
        }
        let (Some(h), Some(canvas)) = (window::find_own(TOOLBAR_TITLE), &self.canvas) else {
            return;
        };
        if let Err(e) = window::ensure_popup(h) {
            tracing::debug!("draw toolbar styles: {e}");
        }
        if let Err(e) = window::fill_client(h) {
            tracing::debug!("draw toolbar client area: {e}");
        }
        window::round_corners(h);
        window::set_owner(h, canvas.hwnd());
        // egui places new windows with the main window's scale, which is
        // wrong on a monitor with another one: put it at the bottom center
        // of the cursor's monitor in physical pixels instead.
        if let Some(m) = self.toolbar_monitor {
            let (x, y, w, hgt) = toolbar_rect(&m);
            if let Err(e) = window::set_rect(h, x, y, w, hgt) {
                tracing::debug!("draw toolbar placement: {e}");
            }
        }
        self.toolbar = Some(h);
    }

    fn toolbar_ui(&mut self, ctx: &egui::Context, theme: &Theme) {
        if ctx.input(|i| i.key_pressed(Key::Escape) || i.viewport().close_requested()) {
            self.set_open(false);
            return;
        }
        if ctx.input(|i| i.modifiers.command && i.key_pressed(Key::Z))
            && let Some(c) = &self.canvas
        {
            c.undo();
        }
        let bar = ctx.content_rect();
        egui::Area::new(egui::Id::new("nook-draw-toolbar"))
            .fixed_pos(bar.min)
            .movable(false)
            .show(ctx, |ui| {
                ui.set_min_size(bar.size());
                ui.painter().rect_filled(bar, 0.0, solid_bg(theme));
                let cy = bar.center().y;

                // Grip: drag to move the toolbar anywhere.
                let grip = Rect::from_center_size(pos2(bar.min.x + 16.0, cy), vec2(24.0, 40.0));
                let resp = ui.interact(grip, ui.id().with("grip"), Sense::drag());
                if resp.drag_started() {
                    ctx.send_viewport_cmd(ViewportCommand::StartDrag);
                }
                if resp.hovered() {
                    ctx.set_cursor_icon(egui::CursorIcon::Grab);
                }
                paint::icon(
                    ui.painter(),
                    grip.center(),
                    icons::DOTS_SIX_VERTICAL,
                    18.0,
                    if resp.hovered() {
                        theme.text
                    } else {
                        theme.text_faint
                    },
                    false,
                );

                let mut x = bar.min.x + 48.0;
                let tools = [
                    (Tool::Pen, icons::PEN, "Pen"),
                    (Tool::Highlighter, icons::HIGHLIGHTER, "Highlighter"),
                    (Tool::Eraser, icons::ERASER, "Eraser"),
                ];
                for (tool, glyph, tip) in tools {
                    if button_at(ui, pos2(x, cy), 32.0, glyph, self.tool == tool, theme)
                        .on_hover_text(tip)
                        .clicked()
                    {
                        self.tool = tool;
                        self.apply_tool();
                    }
                    x += 36.0;
                }
                x += 8.0;
                for (i, c) in COLORS.iter().enumerate() {
                    let center = pos2(x, cy);
                    let rect = Rect::from_center_size(center, vec2(24.0, 24.0));
                    let resp = ui.interact(rect, ui.id().with(("draw-color", i)), Sense::click());
                    if resp.clicked() {
                        self.color = i;
                        if self.tool == Tool::Eraser {
                            self.tool = Tool::Pen;
                        }
                        self.apply_tool();
                    }
                    let selected = self.color == i;
                    if selected {
                        ui.painter().circle_stroke(
                            center,
                            11.0,
                            egui::Stroke::new(2.0, theme.text),
                        );
                    }
                    let r = if resp.hovered() || selected { 8.0 } else { 7.0 };
                    ui.painter().circle_filled(center, r, *c);
                    x += 28.0;
                }
                x += 6.0;
                if button_at(ui, pos2(x, cy), 32.0, icons::ARROW_U_UP_LEFT, false, theme)
                    .on_hover_text("Undo (Ctrl+Z)")
                    .clicked()
                    && let Some(c) = &self.canvas
                {
                    c.undo();
                }
                x += 36.0;
                if button_at(ui, pos2(x, cy), 32.0, icons::TRASH, false, theme)
                    .on_hover_text("Clear")
                    .clicked()
                    && let Some(c) = &self.canvas
                {
                    c.clear();
                }
                // Esc is the way out; say so.
                ui.painter().text(
                    pos2(bar.max.x - 14.0, cy),
                    egui::Align2::RIGHT_CENTER,
                    "Esc",
                    Theme::font(SIZE_LABEL - 1.0),
                    theme.text_faint,
                );
            });
    }
}

impl Module for Draw {
    fn id(&self) -> &'static str {
        "draw"
    }

    fn name(&self) -> &str {
        "Draw on screen"
    }

    fn icon(&self) -> &'static str {
        icons::SCRIBBLE
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn attach(&mut self, ctx: &egui::Context) {
        self.repaint = Some(ctx.clone());
    }

    fn is_active(&self) -> bool {
        self.is_open()
    }

    fn overlay_open(&self) -> bool {
        self.is_open()
    }

    fn on_tick(&mut self) {
        // Esc on the canvas closes it from its own thread.
        if self.canvas.is_some() && !self.is_open() {
            self.canvas = None;
            self.toolbar = None;
        }
    }

    fn default_hotkey(&self) -> Option<&'static str> {
        Some("ctrl+alt+KeyD")
    }

    fn on_hotkey(&mut self) {
        let open = !self.is_open();
        self.set_open(open);
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        let rect = ui.max_rect();
        let open = self.is_open();
        let resp = ui.interact(rect.expand(8.0), ui.id().with("draw-tile"), Sense::click());
        if resp.clicked() {
            self.set_open(!open);
        }
        let color = if open {
            theme.accent_fg()
        } else if ctx.hovered {
            theme.text
        } else {
            theme.text_dim
        };
        paint::icon(
            ui.painter(),
            rect.center(),
            icons::SCRIBBLE,
            SIZE_ICON + 2.0,
            color,
            open,
        );
    }

    fn ui_overlay(&mut self, ctx: &egui::Context, theme: &Theme) {
        if !self.is_open() {
            return;
        }
        // Starts at the bottom center of the monitor (corrected in physical
        // pixels once the window exists); drag it by the grip.
        let ppp = ctx.pixels_per_point();
        let pos = self.toolbar_monitor.map_or(Pos2::ZERO, |m| {
            let (x, y, _, _) = toolbar_rect(&m);
            pos2(x as f32 / ppp, y as f32 / ppp)
        });
        let builder = ViewportBuilder::default()
            .with_title(TOOLBAR_TITLE)
            .with_position(pos)
            .with_inner_size(TOOLBAR)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_window_level(WindowLevel::AlwaysOnTop);
        ctx.show_viewport_immediate(
            ViewportId::from_hash_of(TOOLBAR_TITLE),
            builder,
            |ctx, class| {
                if class == ViewportClass::EmbeddedWindow {
                    return;
                }
                self.sync_toolbar();
                self.toolbar_ui(ctx, theme);
            },
        );
    }

    fn shutdown(&mut self) {
        self.set_open(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toolbar_starts_bottom_center_of_its_monitor() {
        let r = monitor::Rect {
            left: 1920,
            top: 0,
            right: 3520,
            bottom: 900,
        };
        let m = monitor::Monitor {
            id: 1,
            bounds: r,
            work: r,
            dpi: 120,
        };
        let (x, y, w, h) = toolbar_rect(&m);
        assert_eq!((w, h), (505, 60));
        assert_eq!(x, 1920 + (1600 - 505) / 2);
        assert_eq!(y, 900 - 60 - 90);
    }
}
