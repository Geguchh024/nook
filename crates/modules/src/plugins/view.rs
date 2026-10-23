//! Draws a plugin's view tree with Nook's own controls, and turns
//! interaction into `event` messages.
//!
//! Controls are optimistic: a toggle flips, a slider moves and a text field
//! keeps what was typed at once, while the plugin hears about it. When the
//! plugin later sends a different `value` for the same id, that wins.

use super::icons;
use super::protocol::{Align, Kind, ListItem, Node, Shape, TextSize, msg_event};
use crate::kit;
use base64::Engine;
use egui::emath::GuiRounding as _;
use egui::epaint::{PathShape, PathStroke};
use egui::{
    Align2, Color32, FontFamily, FontId, Id, Key, Layout, Pos2, Rect, Response, RichText, Sense,
    Stroke, TextureHandle, Ui, UiBuilder, pos2, vec2,
};
use nook_ui::theme::{SIZE_BIG, SIZE_BODY, SIZE_LABEL, Theme, mix, with_alpha};
use nook_ui::{paint, widgets};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::Path;

const DEFAULT_GAP: f32 = 6.0;
const CONTROL_H: f32 = 26.0;
/// Larger images are scaled down when loaded.
const MAX_IMAGE: u32 = 1024;

/// What a control last showed, so a plugin's new value can override the
/// user's edit and an unchanged one does not.
enum Local {
    Bool { seen: bool, now: bool },
    Num { seen: f64, now: f64 },
    Text { seen: String, now: String },
    Index { seen: usize, now: usize },
}

/// State that outlives a frame: control values and decoded images.
#[derive(Default)]
pub struct ViewState {
    local: HashMap<String, Local>,
    images: HashMap<String, Option<TextureHandle>>,
}

impl ViewState {
    /// Forget images when the plugin restarts (files may have changed).
    pub fn clear_images(&mut self) {
        self.images.clear();
    }
}

pub struct Renderer<'a> {
    pub theme: &'a Theme,
    pub state: &'a mut ViewState,
    /// Outgoing `event` messages.
    pub events: &'a mut Vec<Value>,
    /// Plugin folder, for relative image paths.
    pub dir: &'a Path,
    /// Scopes egui ids (`plugin id` + `tile`/`page`).
    pub salt: &'a str,
}

/// A named or hex color, else `fallback`.
pub fn color(spec: Option<&str>, fallback: Color32, theme: &Theme) -> Color32 {
    let Some(s) = spec else { return fallback };
    match s.trim() {
        "text" => theme.text,
        "dim" => theme.text_dim,
        "faint" => theme.text_faint,
        "accent" => theme.accent,
        "on_accent" => theme.accent_fg(),
        "red" => Color32::from_rgb(0xF0, 0x5A, 0x5A),
        "orange" => Color32::from_rgb(0xF5, 0x9E, 0x3B),
        "yellow" => Color32::from_rgb(0xF2, 0xC9, 0x4C),
        "green" => Color32::from_rgb(0x3D, 0xC9, 0x7A),
        "teal" => Color32::from_rgb(0x2E, 0xC4, 0xB6),
        "blue" => Color32::from_rgb(0x4C, 0x9A, 0xFF),
        "purple" => Color32::from_rgb(0xA3, 0x7C, 0xFF),
        "pink" => Color32::from_rgb(0xF2, 0x6D, 0xB5),
        "white" => Color32::WHITE,
        "black" => Color32::BLACK,
        "transparent" => Color32::TRANSPARENT,
        hex => parse_hex(hex).unwrap_or(fallback),
    }
}

/// `#RGB`, `#RRGGBB` or `#RRGGBBAA`.
fn parse_hex(s: &str) -> Option<Color32> {
    let h = s.strip_prefix('#')?;
    let n = |i: usize, len: usize| u8::from_str_radix(h.get(i..i + len)?, 16).ok();
    match h.len() {
        3 => Some(Color32::from_rgb(
            n(0, 1)? * 17,
            n(1, 1)? * 17,
            n(2, 1)? * 17,
        )),
        6 => Some(Color32::from_rgb(n(0, 2)?, n(2, 2)?, n(4, 2)?)),
        8 => Some(Color32::from_rgba_unmultiplied(
            n(0, 2)?,
            n(2, 2)?,
            n(4, 2)?,
            n(6, 2)?,
        )),
        _ => None,
    }
}

fn text_font(size: TextSize, bold: bool, mono: bool) -> FontId {
    let pts = match size {
        TextSize::Small => SIZE_LABEL - 1.0,
        TextSize::Body => SIZE_BODY,
        TextSize::Title => SIZE_BODY + 3.0,
        TextSize::Big => SIZE_BIG,
        TextSize::Huge => SIZE_BIG * 1.6,
    };
    if mono {
        FontId::new(pts, FontFamily::Monospace)
    } else if bold || matches!(size, TextSize::Title | TextSize::Big | TextSize::Huge) {
        Theme::font_semibold(pts)
    } else {
        Theme::font(pts)
    }
}

fn cross(align: Align) -> egui::Align {
    match align {
        Align::Start => egui::Align::Min,
        Align::Center => egui::Align::Center,
        Align::End => egui::Align::Max,
    }
}

impl Renderer<'_> {
    fn id(&self, node_id: &str) -> Id {
        Id::new(("nook-plugin", self.salt, node_id))
    }

    fn emit(&mut self, id: &str, kind: &str, value: Value) {
        self.events.push(msg_event(id, kind, value));
    }

    /// Draws `root` filling `ui`'s max rect.
    pub fn root(&mut self, ui: &mut Ui, root: &Node) {
        // Whole points keep text crisp (and egui's debug build quiet).
        let rect = ui.max_rect().round_ui();
        let mut child = ui.new_child(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::top_down(egui::Align::Min)),
        );
        child.spacing_mut().item_spacing = vec2(DEFAULT_GAP, DEFAULT_GAP);
        self.node(&mut child, root);
    }

    pub fn node(&mut self, ui: &mut Ui, node: &Node) -> Response {
        let resp = self.kind(ui, &node.kind);
        match &node.tooltip {
            Some(t) if !t.is_empty() => resp.on_hover_text(t),
            _ => resp,
        }
    }

    fn kind(&mut self, ui: &mut Ui, kind: &Kind) -> Response {
        let theme = self.theme;
        match kind {
            Kind::Column {
                children,
                gap,
                align,
                justify,
                scroll,
            } => {
                if *scroll {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .id_salt(self.id("scroll"))
                        .show(ui, |ui| {
                            self.column(ui, children, *gap, *align, Align::Start)
                        })
                        .inner
                } else {
                    self.column(ui, children, *gap, *align, *justify)
                }
            }
            Kind::Row {
                children,
                gap,
                align,
            } => self.row(ui, children, *gap, *align),
            Kind::Grid {
                columns,
                children,
                gap,
            } => self.grid(ui, (*columns).max(1), children, gap.unwrap_or(DEFAULT_GAP)),
            Kind::Card {
                children,
                padding,
                gap,
            } => self.card(ui, children, padding.unwrap_or(10.0), *gap),
            Kind::Spacer { size } => {
                let s = size.unwrap_or(0.0);
                ui.allocate_exact_size(vec2(s, s), Sense::hover()).1
            }
            Kind::Divider => {
                widgets::separator(ui, theme);
                ui.allocate_exact_size(vec2(0.0, 0.0), Sense::hover()).1
            }
            Kind::Text {
                text,
                size,
                color: c,
                bold,
                wrap,
                mono,
            } => {
                let rich = RichText::new(text)
                    .font(text_font(*size, *bold, *mono))
                    .color(color(c.as_deref(), theme.text, theme));
                let label = egui::Label::new(rich).selectable(false);
                ui.add(if *wrap {
                    label.wrap()
                } else {
                    label.truncate()
                })
            }
            Kind::Icon {
                icon,
                size,
                color: c,
                fill,
            } => {
                let s = size.unwrap_or(18.0);
                let (rect, resp) = ui.allocate_exact_size(vec2(s, s), Sense::hover());
                paint::icon(
                    ui.painter(),
                    rect.center(),
                    icons::glyph(icon),
                    s,
                    color(c.as_deref(), theme.text, theme),
                    *fill,
                );
                resp
            }
            Kind::Image {
                path,
                base64,
                width,
                height,
                rounding,
            } => self.image(
                ui,
                path.as_deref(),
                base64.as_deref(),
                *width,
                *height,
                *rounding,
            ),
            Kind::Progress {
                value,
                color: c,
                width,
            } => {
                let w = width.unwrap_or_else(|| ui.available_width()).max(8.0);
                let (rect, resp) = ui.allocate_exact_size(vec2(w, 6.0), Sense::hover());
                kit::meter(
                    ui.painter(),
                    rect,
                    *value,
                    color(c.as_deref(), theme.accent, theme),
                    theme,
                );
                resp
            }
            Kind::Ring {
                value,
                label,
                size,
                color: c,
            } => {
                let s = size.unwrap_or(40.0);
                let (rect, resp) = ui.allocate_exact_size(vec2(s, s), Sense::hover());
                let stroke = (s * 0.09).clamp(2.5, 6.0);
                paint::ring(
                    ui.painter(),
                    rect.center(),
                    s * 0.5 - stroke,
                    stroke,
                    *value,
                    kit::wash(theme),
                    color(c.as_deref(), theme.accent, theme),
                );
                if !label.is_empty() {
                    ui.painter().text(
                        rect.center(),
                        Align2::CENTER_CENTER,
                        label,
                        Theme::font_semibold((s * 0.26).clamp(9.0, 22.0)),
                        theme.text,
                    );
                }
                resp
            }
            Kind::Sparkline {
                values,
                max,
                color: c,
                width,
                height,
            } => {
                let w = width.unwrap_or_else(|| ui.available_width()).max(8.0);
                let (rect, resp) =
                    ui.allocate_exact_size(vec2(w, height.unwrap_or(32.0)), Sense::hover());
                let top = max.unwrap_or_else(|| values.iter().copied().fold(0.0, f32::max));
                kit::sparkline(
                    ui.painter(),
                    rect,
                    values,
                    top,
                    color(c.as_deref(), theme.accent, theme),
                );
                resp
            }
            Kind::List { items } => self.list(ui, items),
            Kind::Canvas {
                id,
                width,
                height,
                shapes,
            } => self.canvas(ui, id.as_deref(), *width, *height, shapes),
            Kind::Button {
                id,
                label,
                icon,
                primary,
                width,
            } => self.button(ui, id, label, icon.as_deref(), *primary, *width),
            Kind::Toggle { id, value, label } => {
                let mut on = match self.state.local.get_mut(id) {
                    Some(Local::Bool { seen, now }) => {
                        if *seen != *value {
                            *seen = *value;
                            *now = *value;
                        }
                        *now
                    }
                    _ => *value,
                };
                let resp = ui
                    .horizontal(|ui| {
                        if let Some(l) = label {
                            ui.label(
                                RichText::new(l)
                                    .font(Theme::font(SIZE_BODY))
                                    .color(theme.text),
                            );
                        }
                        widgets::toggle(ui, &mut on, theme)
                    })
                    .inner;
                if resp.changed() {
                    self.emit(id, "change", json!(on));
                }
                self.state.local.insert(
                    id.clone(),
                    Local::Bool {
                        seen: *value,
                        now: on,
                    },
                );
                resp
            }
            Kind::Slider {
                id,
                value,
                min,
                max,
                step,
                width,
            } => self.slider(ui, id, *value, *min, *max, *step, *width),
            Kind::Input {
                id,
                value,
                hint,
                width,
                live,
            } => {
                let mut text = match self.state.local.get_mut(id) {
                    Some(Local::Text { seen, now }) => {
                        if seen != value {
                            seen.clone_from(value);
                            now.clone_from(value);
                        }
                        now.clone()
                    }
                    _ => value.clone(),
                };
                let w = width.unwrap_or_else(|| ui.available_width()).max(40.0);
                let (rect, _) = ui.allocate_exact_size(vec2(w, 28.0), Sense::hover());
                let wid = self.id(id);
                let resp = kit::field(
                    ui,
                    rect,
                    &mut text,
                    hint,
                    wid,
                    Theme::font(SIZE_BODY),
                    theme,
                );
                if resp.changed() && *live {
                    self.emit(id, "change", json!(text));
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    self.emit(id, "submit", json!(text));
                }
                self.state.local.insert(
                    id.clone(),
                    Local::Text {
                        seen: value.clone(),
                        now: text,
                    },
                );
                resp
            }
            Kind::Chips {
                id,
                options,
                selected,
            } => {
                let mut sel = match self.state.local.get_mut(id) {
                    Some(Local::Index { seen, now }) => {
                        if *seen != *selected {
                            *seen = *selected;
                            *now = *selected;
                        }
                        *now
                    }
                    _ => *selected,
                };
                let font = Theme::font_semibold(SIZE_LABEL);
                let resp = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        for (i, o) in options.iter().enumerate() {
                            let w = ui
                                .painter()
                                .layout_no_wrap(o.clone(), font.clone(), theme.text)
                                .size()
                                .x
                                + 22.0;
                            let (rect, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
                            if kit::chip(ui, rect, o, i == sel, theme).clicked() && i != sel {
                                sel = i;
                                self.events.push(msg_event(
                                    id,
                                    "change",
                                    json!({ "index": i, "label": o }),
                                ));
                            }
                        }
                    })
                    .response;
                self.state.local.insert(
                    id.clone(),
                    Local::Index {
                        seen: *selected,
                        now: sel,
                    },
                );
                resp
            }
            Kind::Unknown => ui.allocate_exact_size(vec2(0.0, 0.0), Sense::hover()).1,
        }
    }

    fn column(
        &mut self,
        ui: &mut Ui,
        children: &[Node],
        gap: Option<f32>,
        align: Align,
        justify: Align,
    ) -> Response {
        let gap = gap.unwrap_or(DEFAULT_GAP);
        // A spacer without size splits the column: the rest hugs the bottom.
        let split = children
            .iter()
            .position(|c| matches!(c.kind, Kind::Spacer { size: None }));
        let avail = ui.available_rect_before_wrap();
        // Centering needs the content height: use last frame's.
        let memo = ui.id().with(("col-h", children.len()));
        let offset = match (justify, split) {
            (Align::Start, _) | (_, Some(_)) => 0.0,
            (j, None) => {
                let h: f32 = ui
                    .ctx()
                    .data(|d| d.get_temp(memo))
                    .unwrap_or(avail.height());
                let room = (avail.height() - h).max(0.0);
                if j == Align::Center { room * 0.5 } else { room }
            }
        };
        let rect = Rect::from_min_max(avail.min + vec2(0.0, offset), avail.max);
        let resp = ui
            .scope_builder(
                UiBuilder::new()
                    .max_rect(rect)
                    .layout(Layout::top_down(cross(align))),
                |ui| {
                    ui.spacing_mut().item_spacing.y = gap;
                    match split {
                        None => {
                            for c in children {
                                self.node(ui, c);
                            }
                        }
                        Some(at) => {
                            for c in &children[..at] {
                                self.node(ui, c);
                            }
                            let rest = ui.available_rect_before_wrap();
                            ui.scope_builder(
                                UiBuilder::new()
                                    .max_rect(rest)
                                    .layout(Layout::bottom_up(cross(align))),
                                |ui| {
                                    for c in children[at + 1..].iter().rev() {
                                        self.node(ui, c);
                                    }
                                },
                            );
                        }
                    }
                },
            )
            .response;
        if justify != Align::Start {
            let h = resp.rect.height();
            let old: Option<f32> = ui.ctx().data(|d| d.get_temp(memo));
            if old.is_none_or(|o| (o - h).abs() > 0.5) {
                ui.ctx().data_mut(|d| d.insert_temp(memo, h));
                ui.ctx().request_repaint();
            }
        }
        resp
    }

    fn row(&mut self, ui: &mut Ui, children: &[Node], gap: Option<f32>, align: Align) -> Response {
        let gap = gap.unwrap_or(DEFAULT_GAP);
        let split = children
            .iter()
            .position(|c| matches!(c.kind, Kind::Spacer { size: None }));
        let valign = match align {
            Align::Start => egui::Align::Min,
            Align::Center => egui::Align::Center,
            Align::End => egui::Align::Max,
        };
        match split {
            None => {
                ui.with_layout(Layout::left_to_right(valign), |ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for c in children {
                        self.node(ui, c);
                    }
                })
                .response
            }
            Some(at) => {
                // Left part, then the rest packed from the right edge.
                let width = ui.available_width();
                ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::left_to_right(valign), |ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for c in &children[..at] {
                        self.node(ui, c);
                    }
                    ui.with_layout(Layout::right_to_left(valign), |ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for c in children[at + 1..].iter().rev() {
                            self.node(ui, c);
                        }
                    });
                })
                .response
            }
        }
    }

    fn grid(&mut self, ui: &mut Ui, columns: usize, children: &[Node], gap: f32) -> Response {
        let width = ui.available_width();
        let cell = ((width - gap * (columns - 1) as f32) / columns as f32).max(1.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = gap;
            for chunk in children.chunks(columns) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = gap;
                    for c in chunk {
                        ui.allocate_ui_with_layout(
                            vec2(cell, 0.0),
                            Layout::top_down(egui::Align::Min),
                            |ui| {
                                ui.set_width(cell);
                                self.node(ui, c);
                            },
                        );
                    }
                });
            }
        })
        .response
    }

    fn card(&mut self, ui: &mut Ui, children: &[Node], pad: f32, gap: Option<f32>) -> Response {
        let theme = self.theme;
        let bg = ui.painter().add(egui::Shape::Noop);
        let width = ui.available_width();
        let resp = ui
            .allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(egui::Align::Min), |ui| {
                ui.set_width(width);
                egui::Frame::NONE.inner_margin(pad).show(ui, |ui| {
                    ui.set_width(width - 2.0 * pad);
                    ui.spacing_mut().item_spacing.y = gap.unwrap_or(DEFAULT_GAP);
                    for c in children {
                        self.node(ui, c);
                    }
                })
            })
            .response;
        ui.painter().set(
            bg,
            egui::Shape::Vec(vec![
                egui::Shape::rect_filled(resp.rect, 12.0, theme.tile_top),
                egui::Shape::rect_stroke(
                    resp.rect,
                    12.0,
                    Stroke::new(1.0, theme.tile_rim),
                    egui::StrokeKind::Inside,
                ),
            ]),
        );
        resp
    }

    fn button(
        &mut self,
        ui: &mut Ui,
        id: &str,
        label: &str,
        icon: Option<&str>,
        primary: bool,
        width: Option<f32>,
    ) -> Response {
        let theme = self.theme;
        let glyph = icon.map(icons::glyph);
        let resp = if label.is_empty() && width.is_none() {
            // Icon only: the round header-style button.
            let g = glyph.unwrap_or(icons::glyph("circle"));
            let mut child = ui.new_child(
                UiBuilder::new()
                    .max_rect(ui.available_rect_before_wrap())
                    .id_salt(("btn", id)),
            );
            let r = widgets::icon_button(&mut child, g, CONTROL_H, primary, theme);
            ui.allocate_rect(r.rect, Sense::hover());
            r
        } else {
            let text_w = ui
                .painter()
                .layout_no_wrap(
                    label.to_owned(),
                    Theme::font_semibold(SIZE_LABEL),
                    theme.text,
                )
                .size()
                .x;
            let natural = text_w + 26.0 + if glyph.is_some() { 18.0 } else { 0.0 };
            let w = width.map_or(natural, |w| if w <= 0.0 { ui.available_width() } else { w });
            let (rect, _) = ui.allocate_exact_size(vec2(w, CONTROL_H), Sense::hover());
            kit::button(ui, rect, label, glyph, primary, theme)
        };
        if resp.clicked() {
            self.emit(id, "click", Value::Null);
        }
        resp
    }

    #[allow(clippy::too_many_arguments)]
    fn slider(
        &mut self,
        ui: &mut Ui,
        id: &str,
        value: f64,
        min: f64,
        max: f64,
        step: Option<f64>,
        width: Option<f32>,
    ) -> Response {
        let theme = self.theme;
        let mut v = match self.state.local.get_mut(id) {
            Some(Local::Num { seen, now }) => {
                if *seen != value {
                    *seen = value;
                    *now = value;
                }
                *now
            }
            _ => value,
        };
        let w = width.unwrap_or_else(|| ui.available_width()).max(40.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 20.0), Sense::click_and_drag());
        let span = (max - min).max(f64::EPSILON);
        if (resp.dragged() || resp.clicked())
            && let Some(p) = resp.interact_pointer_pos()
        {
            let t = ((p.x - rect.min.x - 8.0) / (rect.width() - 16.0)).clamp(0.0, 1.0) as f64;
            let mut nv = min + t * span;
            if let Some(s) = step.filter(|s| *s > 0.0) {
                nv = min + ((nv - min) / s).round() * s;
            }
            nv = nv.clamp(min.min(max), max.max(min));
            if (nv - v).abs() > f64::EPSILON {
                v = nv;
                self.emit(id, "change", json!(v));
            }
        }
        if resp.drag_stopped() {
            self.emit(id, "commit", json!(v));
        }
        let t = ((v - min) / span).clamp(0.0, 1.0) as f32;
        let track = Rect::from_center_size(rect.center(), vec2(rect.width() - 16.0, 4.0));
        kit::meter(ui.painter(), track, t, theme.accent, theme);
        let knob = pos2(track.min.x + track.width() * t, rect.center().y);
        let hover = resp.hovered() || resp.dragged();
        ui.painter()
            .circle_filled(knob + vec2(0.0, 1.0), 8.0, Color32::from_black_alpha(50));
        ui.painter()
            .circle_filled(knob, if hover { 8.0 } else { 7.0 }, Color32::WHITE);
        self.state.local.insert(
            id.to_owned(),
            Local::Num {
                seen: value,
                now: v,
            },
        );
        resp
    }

    fn list(&mut self, ui: &mut Ui, items: &[ListItem]) -> Response {
        let theme = self.theme;
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for item in items {
                let h = if item.subtitle.is_empty() { 30.0 } else { 40.0 };
                let w = ui.available_width();
                let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
                let resp = kit::row(ui, rect, self.id(&item.id), item.selected, theme);
                let mut x = rect.min.x + 10.0;
                if let Some(icon) = &item.icon {
                    paint::icon(
                        ui.painter(),
                        pos2(x + 8.0, rect.center().y),
                        icons::glyph(icon),
                        16.0,
                        theme.text_dim,
                        false,
                    );
                    x += 26.0;
                }
                let trailing_w = if item.trailing.is_empty() {
                    0.0
                } else {
                    let g = ui.painter().layout_no_wrap(
                        item.trailing.clone(),
                        Theme::font(SIZE_LABEL),
                        theme.text_dim,
                    );
                    let gw = g.size().x;
                    ui.painter().galley(
                        pos2(rect.max.x - 10.0 - gw, rect.center().y - g.size().y * 0.5),
                        g,
                        theme.text_dim,
                    );
                    gw + 10.0
                };
                let text_w = rect.max.x - 10.0 - trailing_w - x;
                if item.subtitle.is_empty() {
                    kit::one_line(
                        ui,
                        pos2(x, rect.center().y - 8.0),
                        &item.title,
                        text_w,
                        Theme::font(SIZE_BODY),
                        theme.text,
                    );
                } else {
                    kit::one_line(
                        ui,
                        pos2(x, rect.min.y + 4.0),
                        &item.title,
                        text_w,
                        Theme::font(SIZE_BODY),
                        theme.text,
                    );
                    kit::one_line(
                        ui,
                        pos2(x, rect.min.y + 21.0),
                        &item.subtitle,
                        text_w,
                        Theme::font(SIZE_LABEL),
                        theme.text_dim,
                    );
                }
                if resp.clicked() {
                    self.emit(&item.id, "click", Value::Null);
                }
            }
        })
        .response
    }

    fn canvas(
        &mut self,
        ui: &mut Ui,
        id: Option<&str>,
        width: Option<f32>,
        height: f32,
        shapes: &[Shape],
    ) -> Response {
        let theme = self.theme;
        let w = width.unwrap_or_else(|| ui.available_width()).max(1.0);
        let sense = if id.is_some() {
            Sense::click()
        } else {
            Sense::hover()
        };
        let (rect, resp) = ui.allocate_exact_size(vec2(w, height.max(1.0)), sense);
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let o = rect.min.to_vec2();
        let at = |x: f32, y: f32| pos2(x, y) + o;
        let pts = |p: &[[f32; 2]]| -> Vec<Pos2> { p.iter().map(|[x, y]| at(*x, *y)).collect() };
        for s in shapes {
            match s {
                Shape::Rect {
                    x,
                    y,
                    w,
                    h,
                    radius,
                    fill,
                    stroke,
                    width,
                } => {
                    let r = Rect::from_min_size(at(*x, *y), vec2(*w, *h));
                    if let Some(f) = fill {
                        painter.rect_filled(r, *radius, color(Some(f), theme.text, theme));
                    }
                    if let Some(st) = stroke {
                        painter.rect_stroke(
                            r,
                            *radius,
                            Stroke::new(*width, color(Some(st), theme.text, theme)),
                            egui::StrokeKind::Inside,
                        );
                    }
                }
                Shape::Circle {
                    x,
                    y,
                    r,
                    fill,
                    stroke,
                    width,
                } => {
                    if let Some(f) = fill {
                        painter.circle_filled(at(*x, *y), *r, color(Some(f), theme.text, theme));
                    }
                    if let Some(st) = stroke {
                        painter.circle_stroke(
                            at(*x, *y),
                            *r,
                            Stroke::new(*width, color(Some(st), theme.text, theme)),
                        );
                    }
                }
                Shape::Line {
                    points,
                    color: c,
                    width,
                    closed,
                } if points.len() >= 2 => {
                    let stroke = PathStroke::new(*width, color(c.as_deref(), theme.text, theme));
                    painter.add(if *closed {
                        PathShape::closed_line(pts(points), stroke)
                    } else {
                        PathShape::line(pts(points), stroke)
                    });
                }
                Shape::Polygon { points, fill } if points.len() >= 3 => {
                    painter.add(PathShape::convex_polygon(
                        pts(points),
                        color(fill.as_deref(), theme.text, theme),
                        PathStroke::NONE,
                    ));
                }
                Shape::Arc {
                    x,
                    y,
                    r,
                    start,
                    end,
                    color: c,
                    width,
                } => {
                    let sweep = (end - start).clamp(-360.0, 360.0);
                    let n = ((sweep.abs() / 6.0).ceil() as usize).max(2);
                    let points: Vec<Pos2> = (0..=n)
                        .map(|i| {
                            let deg = start + sweep * i as f32 / n as f32 - 90.0;
                            let a = deg.to_radians();
                            at(*x, *y) + vec2(a.cos(), a.sin()) * *r
                        })
                        .collect();
                    painter.add(PathShape::line(
                        points,
                        PathStroke::new(*width, color(c.as_deref(), theme.accent, theme)),
                    ));
                }
                Shape::Text {
                    x,
                    y,
                    text,
                    size,
                    color: c,
                    anchor,
                    bold,
                } => {
                    let align = match anchor {
                        Align::Start => Align2::LEFT_CENTER,
                        Align::Center => Align2::CENTER_CENTER,
                        Align::End => Align2::RIGHT_CENTER,
                    };
                    let font = if *bold {
                        Theme::font_semibold(*size)
                    } else {
                        Theme::font(*size)
                    };
                    painter.text(
                        at(*x, *y),
                        align,
                        text,
                        font,
                        color(c.as_deref(), theme.text, theme),
                    );
                }
                Shape::Icon {
                    x,
                    y,
                    icon,
                    size,
                    color: c,
                    fill,
                } => paint::icon(
                    &painter,
                    at(*x, *y),
                    icons::glyph(icon),
                    *size,
                    color(c.as_deref(), theme.text, theme),
                    *fill,
                ),
                _ => {}
            }
        }
        if let Some(id) = id
            && resp.clicked()
            && let Some(p) = resp.interact_pointer_pos()
        {
            let local = p - rect.min;
            self.emit(
                id,
                "click",
                json!({ "x": local.x.round(), "y": local.y.round() }),
            );
        }
        resp
    }

    fn image(
        &mut self,
        ui: &mut Ui,
        path: Option<&str>,
        b64: Option<&str>,
        width: Option<f32>,
        height: Option<f32>,
        rounding: f32,
    ) -> Response {
        let key = match (path, b64) {
            (Some(p), _) => {
                let full = self.dir.join(p);
                // Reload when the file changes.
                let stamp = std::fs::metadata(&full)
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis());
                format!("p:{}:{stamp}", full.display())
            }
            (None, Some(b)) => format!("b:{}:{}", b.len(), &b[..b.len().min(64)]),
            (None, None) => String::new(),
        };
        if !self.state.images.contains_key(&key) {
            let bytes = match (path, b64) {
                (Some(p), _) => std::fs::read(self.dir.join(p)).map_err(|e| e.to_string()),
                (None, Some(b)) => base64::engine::general_purpose::STANDARD
                    .decode(b.trim())
                    .map_err(|e| e.to_string()),
                (None, None) => Err("no path or base64".into()),
            };
            let tex = bytes
                .and_then(|b| image::load_from_memory(&b).map_err(|e| e.to_string()))
                .map(|img| {
                    let img = if img.width() > MAX_IMAGE || img.height() > MAX_IMAGE {
                        img.thumbnail(MAX_IMAGE, MAX_IMAGE)
                    } else {
                        img
                    };
                    let rgba = img.to_rgba8();
                    let size = [rgba.width() as usize, rgba.height() as usize];
                    let ci = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
                    ui.ctx()
                        .load_texture(format!("plugin-{key}"), ci, egui::TextureOptions::LINEAR)
                });
            if let Err(e) = &tex {
                tracing::warn!("plugin image {:?}: {e}", path.unwrap_or("base64"));
            }
            // Keep the cache from growing without bound under streamed images.
            if self.state.images.len() > 64 {
                self.state.images.clear();
            }
            self.state.images.insert(key.clone(), tex.ok());
        }
        let tex = self.state.images.get(&key).and_then(Option::as_ref);
        let natural = tex.map_or(vec2(32.0, 32.0), |t| t.size_vec2());
        let aspect = natural.x / natural.y.max(1.0);
        let size = match (width, height) {
            (Some(w), Some(h)) => vec2(w, h),
            (Some(w), None) => vec2(w, w / aspect),
            (None, Some(h)) => vec2(h * aspect, h),
            (None, None) => {
                let w = natural.x.min(ui.available_width());
                vec2(w, w / aspect)
            }
        };
        match tex {
            Some(t) => ui.add(egui::Image::new((t.id(), size)).corner_radius(rounding)),
            None => {
                let (rect, resp) = ui.allocate_exact_size(size, Sense::hover());
                ui.painter()
                    .rect_filled(rect, rounding, kit::wash(self.theme));
                paint::icon(
                    ui.painter(),
                    rect.center(),
                    icons::glyph("image-broken"),
                    16.0,
                    self.theme.text_faint,
                    false,
                );
                resp
            }
        }
    }
}

/// The face of a plugin without a tile view (or that stopped): its icon and
/// name, and a status line.
pub fn default_tile(ui: &Ui, glyph: &str, name: &str, status: &str, theme: &Theme, hot: bool) {
    let rect = ui.max_rect();
    let wide = rect.width() > rect.height() * 1.5;
    let color = if hot { theme.text } else { theme.text_dim };
    if wide {
        paint::icon(
            ui.painter(),
            pos2(rect.min.x + 10.0, rect.center().y),
            glyph,
            20.0,
            color,
            false,
        );
        let x = rect.min.x + 28.0;
        let w = rect.max.x - x;
        kit::one_line(
            ui,
            pos2(x, rect.center().y - 15.0),
            name,
            w,
            Theme::font_semibold(SIZE_LABEL + 1.0),
            theme.text,
        );
        kit::one_line(
            ui,
            pos2(x, rect.center().y + 1.0),
            status,
            w,
            Theme::font(SIZE_LABEL - 1.0),
            mix(theme.text_dim, theme.text_faint, 0.3),
        );
    } else {
        paint::icon(ui.painter(), rect.center(), glyph, 20.0, color, false);
        if !status.is_empty() {
            ui.painter().circle_filled(
                rect.right_top() + vec2(-2.0, 2.0),
                3.0,
                with_alpha(Color32::from_rgb(0xF0, 0x5A, 0x5A), 0.9),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_colors() {
        assert_eq!(parse_hex("#fff"), Some(Color32::WHITE));
        assert_eq!(parse_hex("#102030"), Some(Color32::from_rgb(16, 32, 48)));
        assert_eq!(
            parse_hex("#10203080"),
            Some(Color32::from_rgba_unmultiplied(16, 32, 48, 128))
        );
        assert_eq!(parse_hex("102030"), None);
        assert_eq!(parse_hex("#12345"), None);
    }
}
