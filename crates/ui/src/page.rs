//! A full-panel page: back button and title on top, content below. Used by
//! modules that expand beyond their tile (clipboard history).

use crate::bento::PANEL_PADDING;
use crate::shell::ContentCtx;
use crate::theme::{SIZE_BODY, Theme};
use crate::{icons, widgets};
use egui::{Align, Area, Id, Layout, Order, Rect, Sense, Ui, UiBuilder, pos2, vec2};

/// Height of the header row.
pub const HEADER: f32 = 28.0;

/// What happened on the page this frame.
#[derive(Default)]
pub struct PageOutput {
    /// The back button was clicked.
    pub back: bool,
    /// Empty page space (not a control) was clicked. The page covers the
    /// glass, so the app treats this like a click on the glass: pin / unpin.
    pub background_clicked: bool,
}

/// Draws the page; `content` gets the body rect below the header and may
/// add widgets to the right of the title through the second `Ui`.
pub fn show(
    ui: &mut Ui,
    cctx: &ContentCtx,
    theme: &Theme,
    id: &str,
    title: &str,
    content: impl FnOnce(&mut Ui, &mut Ui, Rect),
) -> PageOutput {
    let ctx = ui.ctx().clone();
    let alpha = cctx.alpha * cctx.stagger(0);
    if alpha <= 0.002 {
        return PageOutput::default();
    }
    let panel = cctx.panel.shrink(PANEL_PADDING);
    let mut back = false;
    let area = Area::new(Id::new(("nook-page", id)))
        .order(Order::Middle)
        .fixed_pos(panel.min)
        .movable(false)
        .constrain(false)
        // Controls drawn inside sit above the area's own background, so
        // this only reports clicks on empty space.
        .sense(Sense::click())
        .show(&ctx, |ui| {
            ui.multiply_opacity(alpha);
            ui.set_min_size(panel.size());
            let header = Rect::from_min_size(panel.min, vec2(panel.width(), HEADER));
            let mut hui = ui.new_child(
                UiBuilder::new()
                    .max_rect(header)
                    .layout(Layout::left_to_right(Align::Center)),
            );
            if widgets::icon_button(&mut hui, icons::ARROW_LEFT, HEADER, false, theme).clicked() {
                back = true;
            }
            hui.add_space(4.0);
            hui.label(
                egui::RichText::new(title)
                    .font(Theme::font_semibold(SIZE_BODY + 1.0))
                    .color(theme.text),
            );
            let mut actions = ui.new_child(
                UiBuilder::new()
                    .max_rect(header)
                    .layout(Layout::right_to_left(Align::Center)),
            );
            let body = Rect::from_min_max(pos2(panel.min.x, header.max.y + 8.0), panel.max);
            let mut bui = ui.new_child(UiBuilder::new().max_rect(body));
            // A tall page unrolls with the glass instead of floating past it.
            bui.set_clip_rect(body.expand(2.0).intersect(cctx.shape));
            content(&mut bui, &mut actions, body);
        });
    ctx.set_transform_layer(area.response.layer_id, cctx.transform);
    PageOutput {
        back,
        background_clicked: area.response.clicked(),
    }
}
