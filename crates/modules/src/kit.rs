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
use nook_ui::{anim, icons, paint, widgets};

/// Search and local-calendar date filter shared by history-style pages.
#[derive(Clone, Debug)]
pub struct HistoryFilter {
    pub query: String,
    pub day: Option<i64>,
    month: i64,
    calendar_open: bool,
}

impl Default for HistoryFilter {
    fn default() -> Self {
        let now = nook_platform::system::local_time();
        let day = crate::calendar::days_from_civil(now.year, now.month, now.day);
        let (year, month, _) = crate::calendar::civil_from_days(day);
        Self {
            query: String::new(),
            day: None,
            month: crate::calendar::days_from_civil(year, month, 1),
            calendar_open: false,
        }
    }
}

impl HistoryFilter {
    pub fn matches(&self, text: &str, day: i64) -> bool {
        (self.query.trim().is_empty()
            || text
                .to_lowercase()
                .contains(&self.query.trim().to_lowercase()))
            && self.day.is_none_or(|selected| selected == day)
    }

    /// Extra content height requested while the inline date picker is open.
    pub fn picker_height(&self) -> f32 {
        if !self.calendar_open {
            return 0.0;
        }
        let (_, month, _) = crate::calendar::civil_from_days(self.month);
        let offset = crate::calendar::weekday(self.month);
        let days =
            crate::calendar::days_in_month(crate::calendar::civil_from_days(self.month).0, month);
        let rows = (offset + days).div_ceil(7).clamp(4, 6) as f32;
        90.0 + (rows - 1.0) * 21.0
    }
}

/// Maps a file timestamp to its local calendar day without waking a worker.
pub fn local_day(timestamp: std::time::SystemTime) -> i64 {
    let now = nook_platform::system::local_time();
    let current = std::time::SystemTime::now();
    let utc_now = current
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let local_seconds = (now.hour * 3600 + now.minute * 60 + now.second) as i64;
    let mut offset = local_seconds - utc_now.rem_euclid(86_400);
    if offset > 43_200 {
        offset -= 86_400;
    } else if offset < -43_200 {
        offset += 86_400;
    }
    let utc_stamp = timestamp
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    (utc_stamp + offset).div_euclid(86_400)
}

/// Draws a search field and a compact expandable month picker.
pub fn history_filter(ui: &mut Ui, filter: &mut HistoryFilter, theme: &Theme) -> f32 {
    let search_id = ui.id().with("nook-history-search");
    ui.horizontal(|ui| {
        let clear_width = if filter.day.is_some() { 34.0 } else { 0.0 };
        let search_width = (ui.available_width() - 126.0 - clear_width).max(120.0);
        let (field, _) = ui.allocate_exact_size(vec2(search_width, 40.0), Sense::hover());
        let focused = ui.ctx().memory(|memory| memory.has_focus(search_id));
        let focus_t = anim::tween(ui.ctx(), search_id.with("focus"), focused, 0.16);
        ui.painter().rect_filled(
            field,
            field.height() * 0.5,
            if theme.dark {
                Color32::from_black_alpha(74)
            } else {
                Color32::from_white_alpha(210)
            },
        );
        ui.painter().rect_stroke(
            field,
            field.height() * 0.5,
            egui::Stroke::new(
                1.0,
                mix(theme.tile_rim, with_alpha(theme.accent, 0.9), focus_t),
            ),
            egui::StrokeKind::Inside,
        );
        paint::icon(
            ui.painter(),
            pos2(field.min.x + 16.0, field.center().y),
            icons::MAGNIFYING_GLASS,
            14.0,
            theme.text_faint,
            false,
        );
        let mut input = ui.new_child(
            UiBuilder::new()
                .max_rect(Rect::from_min_max(
                    pos2(field.min.x + 34.0, field.min.y),
                    pos2(field.max.x - 14.0, field.max.y),
                ))
                .id_salt("history-search-input"),
        );
        input.add(
            TextEdit::singleline(&mut filter.query)
                .id(search_id)
                .frame(egui::Frame::NONE)
                .hint_text(egui::RichText::new("Search content").color(theme.text_faint))
                .font(Theme::font(SIZE_LABEL))
                .text_color(theme.text)
                .vertical_align(Align::Center)
                .desired_width(search_width - 48.0),
        );
        let label = filter.day.map_or_else(
            || "All dates".to_owned(),
            |day| {
                let (y, m, d) = crate::calendar::civil_from_days(day);
                format!("{y:04}-{m:02}-{d:02}")
            },
        );
        let (date_rect, _) = ui.allocate_exact_size(vec2(112.0, 40.0), Sense::hover());
        if button(
            ui,
            date_rect,
            &label,
            Some(icons::CALENDAR_BLANK),
            filter.day.is_some(),
            theme,
        )
        .clicked()
        {
            filter.calendar_open = !filter.calendar_open;
        }
        if filter.day.is_some() {
            let (clear_rect, _) = ui.allocate_exact_size(vec2(30.0, 40.0), Sense::hover());
            if icon_at(ui, clear_rect.center(), 28.0, icons::X, false, theme)
                .on_hover_text("Clear date filter")
                .clicked()
            {
                filter.day = None;
            }
        }
    });

    if !filter.calendar_open {
        return 44.0;
    }
    let calendar_height = filter.picker_height();
    let (calendar, _) =
        ui.allocate_exact_size(vec2(ui.available_width(), calendar_height), Sense::hover());
    widgets::card(ui.painter(), calendar, theme);
    let inner = calendar.shrink2(vec2(12.0, 8.0));
    let (year, month, _) = crate::calendar::civil_from_days(filter.month);
    let mut picker = ui.new_child(UiBuilder::new().max_rect(inner).id_salt("history-calendar"));
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), 30.0));
    if icon_at(
        &mut picker,
        pos2(header.min.x + 14.0, header.center().y),
        25.0,
        icons::CARET_LEFT,
        false,
        theme,
    )
    .clicked()
    {
        let (y, m) = if month == 1 {
            (year - 1, 12)
        } else {
            (year, month - 1)
        };
        filter.month = crate::calendar::days_from_civil(y, m, 1);
    }
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    picker.painter().text(
        pos2(header.center().x, header.center().y),
        Align2::CENTER_CENTER,
        format!("{} {year}", MONTHS[(month - 1) as usize]),
        Theme::font_semibold(SIZE_LABEL + 1.0),
        theme.text,
    );
    if icon_at(
        &mut picker,
        pos2(header.max.x - 78.0, header.center().y),
        25.0,
        icons::CARET_RIGHT,
        false,
        theme,
    )
    .clicked()
    {
        let (y, m) = if month == 12 {
            (year + 1, 1)
        } else {
            (year, month + 1)
        };
        filter.month = crate::calendar::days_from_civil(y, m, 1);
    }
    if button(
        &mut picker,
        Rect::from_center_size(
            pos2(header.max.x - 34.0, header.center().y),
            vec2(58.0, 24.0),
        ),
        "Today",
        None,
        false,
        theme,
    )
    .clicked()
    {
        let now = nook_platform::system::local_time();
        filter.day = Some(crate::calendar::days_from_civil(
            now.year, now.month, now.day,
        ));
        filter.calendar_open = false;
    }
    let weekdays = ["M", "T", "W", "T", "F", "S", "S"];
    let cell_w = inner.width() / 7.0;
    let weekday_y = inner.min.y + 42.0;
    for (i, label) in weekdays.iter().enumerate() {
        picker.painter().text(
            pos2(inner.min.x + (i as f32 + 0.5) * cell_w, weekday_y),
            Align2::CENTER_CENTER,
            *label,
            Theme::font_semibold(SIZE_LABEL - 1.0),
            theme.text_faint,
        );
    }
    let offset = crate::calendar::weekday(filter.month);
    let count = crate::calendar::days_in_month(year, month);
    let cell_h = 21.0;
    for cell in 0..42 {
        let day = cell as i64 - i64::from(offset) + 1;
        if !(1..=i64::from(count)).contains(&day) {
            continue;
        }
        let stamp = filter.month + day - 1;
        let row = cell / 7;
        let col = cell % 7;
        let center = pos2(
            inner.min.x + (col as f32 + 0.5) * cell_w,
            weekday_y + 20.0 + row as f32 * cell_h,
        );
        let rect = Rect::from_center_size(center, vec2((cell_w - 3.0).min(34.0), 20.0));
        let response =
            picker.interact(rect, Id::new(("history-filter-day", stamp)), Sense::click());
        if filter.day == Some(stamp) {
            picker
                .painter()
                .rect_filled(rect, 10.0, with_alpha(theme.accent, 0.85));
        } else if response.hovered() {
            picker
                .painter()
                .rect_filled(rect, 10.0, with_alpha(theme.text, 0.10));
        }
        picker.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            day.to_string(),
            Theme::font(if filter.day == Some(stamp) {
                SIZE_LABEL + 1.0
            } else {
                SIZE_LABEL
            }),
            if filter.day == Some(stamp) {
                theme.accent_fg()
            } else {
                theme.text
            },
        );
        if response.clicked() {
            filter.day = Some(stamp);
            filter.calendar_open = false;
        }
    }
    50.0 + calendar_height
}

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
