//! Small custom controls in the Nook style: spring toggle, segmented
//! control, slider, color swatches and round icon button.

use crate::anim;
use crate::geometry::Squircle;
use crate::paint;
use crate::spring::SpringParams;
use crate::theme::{SIZE_LABEL, Theme, mix, with_alpha};
use egui::{Align2, Color32, Response, Sense, Stroke, Ui, pos2, vec2};

/// Size of [`toggle`].
pub const TOGGLE_SIZE: egui::Vec2 = vec2(36.0, 22.0);

/// iOS-style switch whose knob glides on a spring. The knob sits inset in
/// the track with a soft shadow and stretches toward its travel while
/// pressed; the off track keeps a rim so the switch reads on any glass.
pub fn toggle(ui: &mut Ui, on: &mut bool, theme: &Theme) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(TOGGLE_SIZE, Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let ctx = ui.ctx();
    let t = anim::spring(
        ctx,
        resp.id.with("knob"),
        if *on { 1.0 } else { 0.0 },
        SpringParams::with_overshoot(500.0, 0.06),
    );
    let lit = t.clamp(0.0, 1.0);
    let hover = anim::tween(ctx, resp.id.with("hover"), resp.hovered(), 0.12);
    let press = anim::spring(
        ctx,
        resp.id.with("press"),
        if resp.is_pointer_button_down_on() {
            1.0
        } else {
            0.0
        },
        SpringParams::critical(900.0),
    );
    let painter = ui.painter();
    let radius = rect.height() * 0.5;

    // Track: a visible off state that brightens on hover, then the accent.
    let (off, off_hover, rim) = if theme.dark {
        (
            Color32::from_white_alpha(40),
            Color32::from_white_alpha(56),
            Color32::from_white_alpha(22),
        )
    } else {
        (
            Color32::from_black_alpha(26),
            Color32::from_black_alpha(38),
            Color32::from_black_alpha(18),
        )
    };
    let track = mix(mix(off, off_hover, hover), theme.accent, lit);
    painter.rect_filled(rect, radius, track);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, with_alpha(rim, 1.0 - lit)),
        egui::StrokeKind::Inside,
    );

    // Knob: inset 3 px, up to 5 px wider while pressed.
    let inset = 3.0;
    let d = rect.height() - 2.0 * inset;
    let w = d + 5.0 * press;
    let travel = rect.width() - 2.0 * inset - w;
    let knob = egui::Rect::from_min_size(
        pos2(rect.min.x + inset + travel * t, rect.min.y + inset),
        vec2(w, d),
    );
    let shadow = Color32::from_black_alpha(if theme.dark { 70 } else { 40 });
    painter.rect_filled(knob.translate(vec2(0.0, 1.0)).expand(0.5), d, shadow);
    painter.rect_filled(knob, d * 0.5, Color32::WHITE);
    resp
}

/// Segmented control; the highlight slides between options on a spring.
pub fn segmented(ui: &mut Ui, options: &[&str], selected: &mut usize, theme: &Theme) -> Response {
    let font = Theme::font(SIZE_LABEL);
    let seg_w = options
        .iter()
        .map(|o| {
            ui.painter()
                .layout_no_wrap((*o).to_owned(), font.clone(), theme.text)
                .size()
                .x
        })
        .fold(0.0_f32, f32::max)
        + 14.0;
    let size = vec2(seg_w * options.len() as f32 + 4.0, 24.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    let track = if theme.dark {
        Color32::from_white_alpha(14)
    } else {
        Color32::from_black_alpha(14)
    };
    painter.rect_filled(rect, 12.0, track);

    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let i = (((p.x - rect.min.x - 2.0) / seg_w).floor() as usize).min(options.len() - 1);
        if i != *selected {
            *selected = i;
            resp.mark_changed();
        }
    }
    let pos = anim::spring(
        ui.ctx(),
        resp.id.with("seg"),
        *selected as f32,
        SpringParams::critical(520.0),
    );
    let hl = egui::Rect::from_min_size(
        pos2(rect.min.x + 2.0 + pos * seg_w, rect.min.y + 2.0),
        vec2(seg_w, size.y - 4.0),
    );
    let hl_fill = if theme.dark {
        Color32::from_white_alpha(36)
    } else {
        Color32::WHITE
    };
    painter.rect_filled(hl, 10.0, hl_fill);
    for (i, o) in options.iter().enumerate() {
        let c = pos2(rect.min.x + 2.0 + seg_w * (i as f32 + 0.5), rect.center().y);
        let near = 1.0 - (pos - i as f32).abs().min(1.0);
        let color = mix(theme.text_dim, theme.text, near);
        painter.text(c, Align2::CENTER_CENTER, *o, font.clone(), color);
    }
    resp
}

/// Horizontal slider over `0..=1` with an accent fill and a white knob.
/// `steps > 0` snaps the value to that many intervals.
pub fn slider(ui: &mut Ui, value: &mut f32, width: f32, steps: u32, theme: &Theme) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 22.0), Sense::click_and_drag());
    let inset = 8.0;
    if (resp.dragged() || resp.clicked())
        && let Some(p) = resp.interact_pointer_pos()
    {
        let mut t = ((p.x - rect.min.x - inset) / (rect.width() - 2.0 * inset)).clamp(0.0, 1.0);
        if steps > 0 {
            t = (t * steps as f32).round() / steps as f32;
        }
        if (t - *value).abs() > f32::EPSILON {
            *value = t;
            resp.mark_changed();
        }
    }
    let t = value.clamp(0.0, 1.0);
    let hover = anim::tween(
        ui.ctx(),
        resp.id.with("hover"),
        resp.hovered() || resp.dragged(),
        0.12,
    );
    let painter = ui.painter();
    let track = egui::Rect::from_center_size(rect.center(), vec2(rect.width() - 2.0 * inset, 4.0));
    let empty = if theme.dark {
        Color32::from_white_alpha(30)
    } else {
        Color32::from_black_alpha(22)
    };
    painter.rect_filled(track, 2.0, empty);
    let fill = egui::Rect::from_min_max(
        track.min,
        pos2(track.min.x + track.width() * t, track.max.y),
    );
    painter.rect_filled(fill, 2.0, theme.accent);
    let knob = pos2(track.min.x + track.width() * t, rect.center().y);
    let r = 7.0 + hover;
    painter.circle_filled(
        knob + vec2(0.0, 1.0),
        r + 0.5,
        Color32::from_black_alpha(if theme.dark { 70 } else { 40 }),
    );
    painter.circle_filled(knob, r, Color32::WHITE);
    resp
}

/// A row of round color swatches; `None` is drawn as a "no color" swatch.
/// `selected` is the index of the chosen swatch, if any.
pub fn swatches(
    ui: &mut Ui,
    colors: &[Option<Color32>],
    selected: &mut Option<usize>,
    theme: &Theme,
) -> Response {
    const D: f32 = 16.0;
    const GAP: f32 = 6.0;
    let n = colors.len() as f32;
    let size = vec2(n * D + (n - 1.0).max(0.0) * GAP, 22.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, Sense::click());
    let hovered = resp
        .hover_pos()
        .map(|p| (((p.x - rect.min.x) / (D + GAP)) as usize).min(colors.len() - 1));
    if resp.clicked()
        && let Some(i) = hovered
        && *selected != Some(i)
    {
        *selected = Some(i);
        resp.mark_changed();
    }
    let painter = ui.painter();
    let rim = if theme.dark {
        Color32::from_white_alpha(40)
    } else {
        Color32::from_black_alpha(30)
    };
    for (i, color) in colors.iter().enumerate() {
        let c = pos2(rect.min.x + D * 0.5 + i as f32 * (D + GAP), rect.center().y);
        let grow = if hovered == Some(i) { 1.0 } else { 0.0 };
        let r = D * 0.5 + grow;
        match color {
            Some(color) => {
                painter.circle_filled(c, r, *color);
            }
            None => {
                // Neutral swatch with a diagonal slash.
                let [r8, g8, b8, _] =
                    mix(theme.glass_top, theme.glass_bottom, 0.5).to_srgba_unmultiplied();
                painter.circle_filled(c, r, Color32::from_rgb(r8, g8, b8));
                let d = vec2(r * 0.6, -r * 0.6);
                painter.line_segment([c - d, c + d], Stroke::new(1.5, theme.text_dim));
            }
        }
        painter.circle_stroke(c, r, Stroke::new(1.0, rim));
        if *selected == Some(i) {
            painter.circle_stroke(c, r + 2.5, Stroke::new(1.5, theme.text));
        }
    }
    resp
}

/// Round icon button with a hover wash and press scale.
pub fn icon_button(
    ui: &mut Ui,
    glyph: &'static str,
    size: f32,
    active: bool,
    theme: &Theme,
) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let hover = anim::spring(
        ui.ctx(),
        resp.id.with("hover"),
        if resp.hovered() { 1.0 } else { 0.0 },
        SpringParams::snappy(),
    );
    let press = anim::spring(
        ui.ctx(),
        resp.id.with("press"),
        if resp.is_pointer_button_down_on() {
            1.0
        } else {
            0.0
        },
        SpringParams::critical(900.0),
    );
    let painter = ui.painter();
    let r = rect.width() * 0.5 * (1.0 - 0.06 * press);
    let wash = if theme.dark {
        Color32::from_white_alpha(22)
    } else {
        Color32::from_black_alpha(16)
    };
    if hover > 0.01 {
        painter.circle_filled(rect.center(), r, with_alpha(wash, hover));
    }
    let color = if active {
        theme.accent_fg()
    } else {
        theme.text
    };
    paint::icon(
        painter,
        rect.center(),
        glyph,
        size * 0.55 * (1.0 - 0.06 * press),
        color,
        active,
    );
    resp
}

/// Clickable header of a collapsible card: icon, title, a dim summary and a
/// chevron that turns as `open` (the section's 0..1 reveal) moves. `rect` is
/// the whole header strip; returns its response (clicked = toggle).
pub fn disclosure_header(
    ui: &mut Ui,
    rect: egui::Rect,
    glyph: &'static str,
    title: &str,
    summary: &str,
    open: f32,
    theme: &Theme,
) -> Response {
    let resp = ui.interact(rect, ui.id().with(("disclosure", title)), Sense::click());
    let hover = anim::tween(ui.ctx(), resp.id.with("hover"), resp.hovered(), 0.12);
    let painter = ui.painter();
    if hover > 0.01 {
        let wash = if theme.dark {
            Color32::from_white_alpha(8)
        } else {
            Color32::from_black_alpha(6)
        };
        let s = Squircle::new(rect.shrink(1.0), crate::theme::TILE_RADIUS - 1.0);
        painter.add(egui::epaint::PathShape::convex_polygon(
            s.outline(),
            with_alpha(wash, hover),
            Stroke::NONE,
        ));
    }
    let cy = rect.center().y;
    let x = rect.min.x + 14.0;
    paint::icon(
        painter,
        pos2(x + 10.0, cy),
        glyph,
        crate::theme::SIZE_ICON,
        mix(theme.text_dim, theme.accent, open.clamp(0.0, 1.0)),
        false,
    );
    let title_g = painter.layout_no_wrap(
        title.to_owned(),
        Theme::font(crate::theme::SIZE_BODY),
        theme.text,
    );
    let tx = x + 30.0;
    let title_w = title_g.size().x;
    painter.galley(pos2(tx, cy - title_g.size().y * 0.5), title_g, theme.text);
    // The summary fades out as the section opens: the controls say it then.
    painter.text(
        pos2(tx + title_w + 10.0, cy),
        Align2::LEFT_CENTER,
        summary,
        Theme::font(SIZE_LABEL),
        with_alpha(theme.text_faint, 1.0 - open.clamp(0.0, 1.0)),
    );
    // Chevron: a caret drawn as two strokes, rotating from right to down.
    let c = pos2(rect.max.x - 22.0, cy);
    let angle = (1.0 - open) * -std::f32::consts::FRAC_PI_2;
    let rot = egui::emath::Rot2::from_angle(angle);
    let arm = 4.0;
    let pts = [
        vec2(-arm, -arm * 0.5),
        vec2(0.0, arm * 0.5),
        vec2(arm, -arm * 0.5),
    ]
    .map(|p| c + rot * p);
    painter.add(egui::epaint::PathShape::line(
        pts.to_vec(),
        egui::epaint::PathStroke::new(1.5, mix(theme.text_dim, theme.text, hover)),
    ));
    resp
}

/// A thin row separator.
pub fn separator(ui: &mut Ui, theme: &Theme) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 1.0), Sense::hover());
    ui.painter().hline(
        rect.x_range(),
        rect.center().y,
        Stroke::new(1.0, theme.tile_rim),
    );
}

/// A glass card (same look as a bento tile) behind arbitrary content.
pub fn card(painter: &egui::Painter, rect: egui::Rect, theme: &Theme) {
    let s = Squircle::new(rect, crate::theme::TILE_RADIUS);
    let outline = s.outline();
    paint::fill_gradient(painter, &outline, theme.tile_top, theme.tile_bottom);
    painter.add(egui::epaint::PathShape::closed_line(
        outline,
        egui::epaint::PathStroke::new(1.0, theme.tile_rim),
    ));
}
