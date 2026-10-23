//! Controls shared by the tool pages (QR code, calculator, calendar, system
//! monitor, quick launch, voice memo), drawn in the Nook style: pill
//! buttons, chips, glass text fields, section labels and sparklines.

use egui::epaint::{Mesh, PathShape, PathStroke};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align, Align2, Color32, FontId, Id, Layout, Pos2, Rect, Response, Sense, Shape, Stroke,
    TextEdit, Ui, UiBuilder, pos2, vec2,
};
use nook_ui::module::TileCtx;
use nook_ui::theme::{SIZE_ICON, SIZE_LABEL, Theme, mix, with_alpha};
use nook_ui::{anim, paint, widgets};

/// A faint fill for wells and inactive controls.
pub fn wash(theme: &Theme) -> Color32 {
    if theme.dark {
        Color32::from_white_alpha(14)
    } else {
        Color32::from_black_alpha(10)
    }
}

/// A text button, accent-filled when `primary`.
pub fn button(
    ui: &mut Ui,
    rect: Rect,
    label: &str,
    icon: Option<&str>,
    primary: bool,
    theme: &Theme,
) -> Response {
    let resp = ui.interact(
        rect,
        ui.id()
            .with(("kit-btn", label, rect.min.x as i32, rect.min.y as i32)),
        Sense::click(),
    );
    let enabled = ui.is_enabled();
    let hover = anim::tween(ui.ctx(), resp.id.with("h"), resp.hovered() && enabled, 0.12);
    let press = if resp.is_pointer_button_down_on() {
        0.97
    } else {
        1.0
    };
    let r = Rect::from_center_size(rect.center(), rect.size() * press);
    let painter = ui.painter();
    let (fill, fg) = if primary {
        (
            mix(theme.accent, Color32::WHITE, 0.12 * hover),
            Color32::WHITE,
        )
    } else {
        (
            with_alpha(
                theme.text,
                if theme.dark { 0.10 } else { 0.07 } + 0.06 * hover,
            ),
            theme.text,
        )
    };
    painter.rect_filled(r, r.height() * 0.5, fill);
    let font = Theme::font_semibold(SIZE_LABEL);
    let galley = painter.layout_no_wrap(label.to_owned(), font, fg);
    let icon_w = match (icon, label.is_empty()) {
        (Some(_), true) => 0.0,
        (Some(_), false) => 18.0,
        (None, _) => 0.0,
    };
    let x0 = r.center().x - (galley.size().x + icon_w) * 0.5;
    if let Some(i) = icon {
        let c = if label.is_empty() {
            r.center()
        } else {
            pos2(x0 + 7.0, r.center().y)
        };
        paint::icon(painter, c, i, 13.0, fg, primary);
    }
    painter.galley(
        pos2(x0 + icon_w, r.center().y - galley.size().y * 0.5),
        galley,
        fg,
    );
    resp
}

/// A selectable chip; the selected one is lit with the accent.
pub fn chip(ui: &mut Ui, rect: Rect, label: &str, selected: bool, theme: &Theme) -> Response {
    let resp = ui.interact(
        rect,
        ui.id()
            .with(("kit-chip", label, rect.min.x as i32, rect.min.y as i32)),
        Sense::click(),
    );
    let ctx = ui.ctx();
    let hover = anim::tween(ctx, resp.id.with("h"), resp.hovered(), 0.12);
    let on = anim::tween(ctx, resp.id.with("on"), selected, 0.16);
    let painter = ui.painter();
    let idle = with_alpha(wash(theme), 1.0 + 0.8 * hover);
    let fill = mix(
        idle,
        with_alpha(theme.accent, if theme.dark { 0.32 } else { 0.18 }),
        on,
    );
    painter.rect_filled(rect, rect.height() * 0.5, fill);
    if on > 0.01 {
        painter.rect_stroke(
            rect,
            rect.height() * 0.5,
            Stroke::new(1.0, with_alpha(theme.accent, 0.7 * on)),
            egui::StrokeKind::Inside,
        );
    }
    let color = mix(
        mix(theme.text_dim, theme.text, hover),
        theme.accent_fg(),
        on,
    );
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        Theme::font_semibold(SIZE_LABEL),
        color,
    );
    resp
}

/// A row of equal chips filling `rect`; returns the index clicked.
pub fn chips(
    ui: &mut Ui,
    rect: Rect,
    labels: &[&str],
    selected: usize,
    theme: &Theme,
) -> Option<usize> {
    let gap = 6.0;
    let w = (rect.width() - gap * (labels.len() as f32 - 1.0)) / labels.len() as f32;
    let mut picked = None;
    for (i, l) in labels.iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(rect.min.x + i as f32 * (w + gap), rect.min.y),
            vec2(w, rect.height()),
        );
        if chip(ui, r, l, i == selected, theme).clicked() {
            picked = Some(i);
        }
    }
    picked
}

/// A small label over a group of controls.
pub fn section(ui: &Ui, pos: Pos2, text: &str, theme: &Theme) {
    ui.painter().text(
        pos,
        Align2::LEFT_TOP,
        text,
        Theme::font_semibold(SIZE_LABEL - 1.0),
        theme.text_faint,
    );
}

/// A single-line glass text field. The rim lights up while it has focus.
pub fn field(
    ui: &mut Ui,
    rect: Rect,
    text: &mut String,
    hint: &str,
    id: Id,
    font: FontId,
    theme: &Theme,
) -> Response {
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    let t = anim::tween(ui.ctx(), id.with("focus"), focused, 0.15);
    let painter = ui.painter();
    let fill = if theme.dark {
        Color32::from_black_alpha(60)
    } else {
        Color32::from_white_alpha(200)
    };
    painter.rect_filled(rect, 10.0, fill);
    painter.rect_stroke(
        rect,
        10.0,
        Stroke::new(1.0, mix(theme.tile_rim, with_alpha(theme.accent, 0.85), t)),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink2(vec2(10.0, 0.0));
    let mut child = ui.new_child(
        UiBuilder::new()
            .max_rect(inner)
            .layout(Layout::left_to_right(Align::Center)),
    );
    child.add(
        TextEdit::singleline(text)
            .id(id)
            .frame(egui::Frame::NONE)
            .font(font)
            .text_color(theme.text)
            .hint_text(egui::RichText::new(hint).color(theme.text_faint))
            .desired_width(inner.width())
            .vertical_align(Align::Center),
    )
}

/// A round icon button centered on `center`.
pub fn icon_at(
    ui: &mut Ui,
    center: Pos2,
    size: f32,
    glyph: &'static str,
    active: bool,
    theme: &Theme,
) -> Response {
    let rect = Rect::from_center_size(center, vec2(size, size));
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).id_salt((
        "kit-icon",
        glyph,
        center.x as i32,
        center.y as i32,
    )));
    widgets::icon_button(&mut child, glyph, size, active, theme)
}

/// A clickable row with a hover wash (and a stronger one when selected).
pub fn row(ui: &mut Ui, rect: Rect, id: Id, selected: bool, theme: &Theme) -> Response {
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let hover = anim::tween(ui.ctx(), id.with("hover"), resp.hovered(), 0.12);
    let t = if selected { 1.0 } else { hover * 0.7 };
    if t > 0.0 {
        let fill = if theme.dark {
            Color32::from_white_alpha(22)
        } else {
            Color32::from_black_alpha(14)
        };
        ui.painter().rect_filled(rect, 10.0, with_alpha(fill, t));
    }
    resp
}

/// Text cut to one line (`…` at the end); returns the rect it took.
pub fn one_line(ui: &Ui, pos: Pos2, text: &str, width: f32, font: FontId, color: Color32) -> Rect {
    let mut job =
        LayoutJob::single_section(text.replace('\n', " "), TextFormat::simple(font, color));
    job.wrap = TextWrapping {
        max_width: width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    let rect = Rect::from_min_size(pos, galley.size());
    ui.painter().galley(pos, galley, color);
    rect
}

/// A filled area chart of `values` (oldest first) scaled to `max`, with the
/// newest value at the right edge.
pub fn sparkline(painter: &egui::Painter, rect: Rect, values: &[f32], max: f32, color: Color32) {
    if values.len() < 2 || max <= 0.0 {
        return;
    }
    let n = values.len();
    let step = rect.width() / (n - 1) as f32;
    let points: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let t = (v / max).clamp(0.0, 1.0);
            pos2(rect.min.x + i as f32 * step, rect.max.y - t * rect.height())
        })
        .collect();
    // Area: a strip from the line down to the baseline, fading out.
    let mut mesh = Mesh::default();
    let top = with_alpha(color, 0.30);
    let bottom = with_alpha(color, 0.0);
    for p in &points {
        mesh.colored_vertex(*p, top);
        mesh.colored_vertex(pos2(p.x, rect.max.y), bottom);
    }
    for i in 0..(n as u32 - 1) {
        let a = i * 2;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 1, a + 2, a + 3);
    }
    painter.add(Shape::mesh(mesh));
    painter.add(PathShape::line(points, PathStroke::new(1.5, color)));
}

/// A thin horizontal meter.
pub fn meter(painter: &egui::Painter, rect: Rect, t: f32, color: Color32, theme: &Theme) {
    painter.rect_filled(rect, rect.height() * 0.5, wash(theme));
    let w = rect.width() * t.clamp(0.0, 1.0);
    if w > 0.5 {
        let fill = Rect::from_min_size(rect.min, vec2(w.max(rect.height()), rect.height()));
        painter.rect_filled(fill, rect.height() * 0.5, color);
    }
}

/// The face of a small tool tile: its glyph, lit on hover. Returns the
/// tile's click response.
pub fn tool_tile(ui: &mut Ui, ctx: &TileCtx<'_>, glyph: &'static str, active: bool) -> Response {
    let theme = ctx.theme;
    let rect = ui.max_rect();
    let resp = ui.interact(rect.expand(8.0), ui.id().with("tool-tile"), Sense::click());
    let color = if active {
        theme.accent_fg()
    } else if ctx.hovered {
        theme.text
    } else {
        theme.text_dim
    };
    paint::icon(
        ui.painter(),
        rect.center(),
        glyph,
        SIZE_ICON + 2.0,
        color,
        active,
    );
    resp
}

/// An empty state: a big faint glyph over a line of text.
pub fn empty(ui: &Ui, center: Pos2, glyph: &str, text: &str, theme: &Theme) {
    paint::icon(
        ui.painter(),
        center - vec2(0.0, 14.0),
        glyph,
        28.0,
        theme.text_faint,
        false,
    );
    ui.painter().text(
        center + vec2(0.0, 16.0),
        Align2::CENTER_CENTER,
        text,
        Theme::font(SIZE_LABEL + 1.0),
        theme.text_dim,
    );
}

/// Mouse wheel travel this frame, in lines (up is positive).
pub fn wheel_lines(ctx: &egui::Context) -> f32 {
    ctx.input(|i| {
        i.events
            .iter()
            .map(|e| match e {
                egui::Event::MouseWheel { unit, delta, .. } => match unit {
                    egui::MouseWheelUnit::Line => delta.y,
                    egui::MouseWheelUnit::Point => delta.y / 40.0,
                    egui::MouseWheelUnit::Page => delta.y * 3.0,
                },
                _ => 0.0,
            })
            .sum()
    })
}

/// Persistent `key=value` settings for a module, one per line.
pub fn settings_path(file: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("APPDATA").map(|a| std::path::PathBuf::from(a).join("Nook").join(file))
}

pub fn read_settings(file: &str) -> Vec<(String, String)> {
    settings_path(file)
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| {
            s.lines()
                .filter_map(|l| l.split_once('='))
                .map(|(k, v)| (k.trim().to_owned(), v.trim().to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

pub fn write_settings(file: &str, pairs: &[(&str, String)]) {
    let Some(path) = settings_path(file) else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let text: String = pairs
        .iter()
        .map(|(k, v)| format!("{k}={}\n", v.replace('\n', " ")))
        .collect();
    if let Err(e) = std::fs::write(&path, text) {
        tracing::warn!("saving {}: {e}", path.display());
    }
}
