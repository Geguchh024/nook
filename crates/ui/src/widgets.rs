//! Small custom controls in the Nook style: spring toggle, segmented
//! control and round icon button.

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
