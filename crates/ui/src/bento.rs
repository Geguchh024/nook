//! The expanded panel's bento grid: packs module tiles, draws their glass
//! tiles with hover lift, press scale and accent state, and hosts each
//! module's `ui_tile`.
//!
//! Two extras live here. **Edit mode** (the pencil in the bottom gutter)
//! turns the grid into a live preview of its own layout: drag a tile and the
//! others spring out of its way as the grid repacks; the tick under each
//! icon shows or hides it and the shapes below pick its size. **Expanding**:
//! a tile made smaller than its layout needs draws a compact face, and a
//! click opens it: the grid repacks with it at full size, so it grows in
//! place while its neighbours glide out of the way. A click anywhere else
//! (or Esc) folds it back.

use crate::anim;
use crate::geometry::{GridSpec, Squircle, TileSize, pack_pages};
use crate::icons;
use crate::module::{Badge, GridTile, Module, ModuleStatus, TileCtx};
use crate::paint;
use crate::shell::ContentCtx;
use crate::spring::SpringParams;
use crate::theme::{self, SIZE_ICON, SIZE_LABEL, TILE_PADDING, TILE_RADIUS, Theme, with_alpha};
use egui::emath::TSTransform;
use egui::{Align2, Area, Color32, Id, Order, Rect, Sense, Ui, UiBuilder, Vec2, pos2, vec2};

pub const GRID: GridSpec = GridSpec {
    columns: 8,
    rows: 3,
    cell: 60.0,
    gap: 8.0,
};

/// Inset of the grid inside the panel (top/left/right). The bottom gutter is
/// taller and holds the caption line.
pub const PANEL_PADDING: f32 = 16.0;

/// A layout change made in edit mode.
#[derive(Clone, Debug, PartialEq)]
pub enum Edit {
    /// New order of every tile shown in edit mode.
    Reorder(Vec<&'static str>),
    /// Show a hidden tile or hide a shown one.
    ToggleShown(&'static str),
    Resize(&'static str, TileSize),
}

#[derive(Default)]
pub struct BentoOutput {
    /// Id of the tile that was clicked this frame, if any.
    pub clicked: Option<&'static str>,
    /// Layout changes made in edit mode this frame.
    pub edits: Vec<Edit>,
    /// The pencil (or the Done check) was clicked.
    pub toggle_edit: bool,
}

/// Grid origin within a panel rect.
pub fn grid_rect(panel: Rect) -> Rect {
    let size = GRID.size();
    let min = pos2(panel.center().x - size.x * 0.5, panel.min.y + PANEL_PADDING);
    Rect::from_min_size(min, size)
}

/// Grid page shown, kept in egui memory.
#[derive(Clone, Copy, Default)]
struct PageState {
    page: usize,
    /// Wheel travel (in lines) not yet turned into a page flip.
    wheel: f32,
    /// egui time of the last wheel event and of the last flip.
    last_wheel: f64,
    last_flip: f64,
}

/// A tile held in edit mode, kept in egui memory.
#[derive(Clone)]
struct Drag {
    id: &'static str,
    /// Pointer offset from the tile's corner when it was picked up.
    grab: Vec2,
    /// The order being previewed, held tile included.
    order: Vec<&'static str>,
    /// When the held tile last changed places (debounces swaps).
    last_move: f64,
}

/// The compact tile opened to full size, kept in egui memory.
#[derive(Clone, Copy, Default)]
struct Expanded {
    id: Option<&'static str>,
    /// egui time it opened, so the opening click does not close it.
    since: f64,
    /// Show the page it lands on (it may move when it grows).
    reveal: bool,
}

/// Wheel travel, in lines, that flips a page.
const FLIP_AT: f32 = 1.0;
/// Pause after a flip so one fling does not run through every page.
const FLIP_COOLDOWN: f64 = 0.35;
/// Hovering a page dot with a held tile flips after this long.
const DOT_FLIP: f64 = 0.4;

fn wheel_lines(ctx: &egui::Context) -> f32 {
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

/// A tile drawn smaller than `size_hint` gets the compact face.
fn is_compact(tile: &GridTile<'_>) -> bool {
    tile.module.status() == ModuleStatus::Ready && tile.size.area() < tile.module.size_hint().area()
}

/// Glass tile background: gradient, rim and highlight, with hover glow.
fn tile_glass(painter: &egui::Painter, rect: Rect, theme: &Theme, active: bool, lift: f32) {
    let shape = Squircle::new(rect, TILE_RADIUS);
    let outline = shape.outline();
    if lift > 0.01 {
        paint::glow(painter, &shape, theme.accent, lift * 0.6);
    }
    let (top, bottom) = if active {
        theme.active_tile()
    } else {
        (theme.tile_top, theme.tile_bottom)
    };
    // Hover brightens the top of the gradient a touch.
    let top = theme::mix(top, with_alpha(theme.text, 0.16), lift * 0.4);
    paint::fill_gradient(painter, &outline, top, bottom);
    painter.add(egui::epaint::PathShape::closed_line(
        outline,
        egui::epaint::PathStroke::new(1.0, theme.tile_rim),
    ));
    paint::top_highlight(painter, &shape, with_alpha(theme.highlight, 0.5));
}

fn tile_inner(rect: Rect, size: TileSize) -> Rect {
    rect.shrink(match size {
        TileSize::Small => 8.0,
        _ => TILE_PADDING,
    })
}

pub fn show(
    ui: &mut Ui,
    cctx: &ContentCtx,
    theme: &Theme,
    tiles: &mut [GridTile<'_>],
    editing: bool,
) -> BentoOutput {
    let ctx = ui.ctx().clone();
    let mut out = BentoOutput::default();
    let grid = grid_rect(cctx.panel);
    let now = ctx.input(|i| i.time);
    let mut caption: Option<(String, bool)> = None;
    let ids: Vec<&'static str> = tiles.iter().map(|t| t.module.id()).collect();

    let drag_id = Id::new("nook-bento-drag");
    let expand_id = Id::new("nook-bento-expand");
    let mut drag: Option<Drag> = if editing {
        ctx.data(|d| d.get_temp(drag_id))
    } else {
        None
    };
    let mut expanded: Expanded = if editing {
        Expanded::default()
    } else {
        ctx.data(|d| d.get_temp(expand_id)).unwrap_or_default()
    };
    if cctx.closing {
        expanded.id = None;
    }
    // The open tile is packed at its full size.
    let open_id = expanded.id;
    let size_of = move |t: &GridTile<'_>| {
        if open_id == Some(t.module.id()) {
            t.module.size_hint()
        } else {
            t.size
        }
    };

    // Display order: the previewed order while a tile is held.
    let order: Vec<usize> = match &drag {
        Some(d) => {
            let mut o: Vec<usize> = d
                .order
                .iter()
                .filter_map(|id| ids.iter().position(|x| x == id))
                .collect();
            let missing: Vec<usize> = (0..ids.len()).filter(|i| !o.contains(i)).collect();
            o.extend(missing);
            o
        }
        None => (0..ids.len()).collect(),
    };
    let sizes: Vec<TileSize> = order.iter().map(|&i| size_of(&tiles[i])).collect();
    let pages = pack_pages(&GRID, &sizes);

    // Pointer in content space (the content scales while the shell opens).
    let pointer = ctx
        .input(|i| i.pointer.interact_pos())
        .map(|p| cctx.transform.inverse() * p);

    // Paging: the wheel over the panel scrolls between grids.
    let state_id = Id::new("nook-bento-page");
    let mut st: PageState = ctx.data(|d| d.get_temp(state_id)).unwrap_or_default();
    st.page = st.page.min(pages.len() - 1);
    if expanded.reveal {
        expanded.reveal = false;
        if let Some(page) = pages
            .iter()
            .position(|pl| pl.iter().any(|p| Some(ids[order[p.index]]) == expanded.id))
        {
            st.page = page;
        }
    }
    let over_panel = pointer.is_some_and(|p| cctx.panel.contains(p));
    let wheel_taken = pointer.is_some_and(|p| {
        pages[st.page].iter().any(|pl| {
            let t = &tiles[order[pl.index]];
            GRID.cell_rect(grid.min, pl.col, pl.row, size_of(t))
                .contains(p)
                && !editing
                && (!is_compact(t) || expanded.id == Some(t.module.id()))
                && t.module.captures_wheel()
        })
    });
    let lines = wheel_lines(&ctx);
    if now - st.last_wheel > 0.25 {
        st.wheel = 0.0;
    }
    if lines != 0.0 && over_panel && !wheel_taken && pages.len() > 1 && cctx.alpha > 0.5 {
        st.wheel += lines;
        st.last_wheel = now;
        if st.wheel.abs() >= FLIP_AT && now - st.last_flip > FLIP_COOLDOWN {
            // Wheel down (negative y) goes to the next page.
            st.page = if st.wheel < 0.0 {
                (st.page + 1).min(pages.len() - 1)
            } else {
                st.page.saturating_sub(1)
            };
            st.wheel = 0.0;
            st.last_flip = now;
        }
    }

    // Bottom gutter: edit pencil on the left, page dots on the right.
    let gutter_y = grid.max.y + (cctx.panel.max.y - grid.max.y) * 0.5 - 1.0;
    {
        let c = pos2(grid.min.x + 8.0, gutter_y);
        let hit = Rect::from_center_size(c, vec2(22.0, 18.0));
        let resp = ui.interact(hit, Id::new("nook-bento-edit"), Sense::click());
        let (glyph, color) = if editing {
            (icons::CHECK, theme.accent_fg())
        } else if resp.hovered() {
            (icons::PENCIL_SIMPLE, theme.text_dim)
        } else {
            (icons::PENCIL_SIMPLE, theme.text_faint)
        };
        paint::icon(
            ui.painter(),
            c,
            glyph,
            13.0,
            with_alpha(color, cctx.alpha),
            editing,
        );
        if resp
            .on_hover_text(if editing { "Done" } else { "Edit the layout" })
            .clicked()
        {
            out.toggle_edit = true;
        }
    }
    if pages.len() > 1 {
        let n = pages.len();
        for i in 0..n {
            let x = grid.max.x - 6.0 - (n - 1 - i) as f32 * 12.0;
            let c = pos2(x, gutter_y);
            let hit = Rect::from_center_size(c, vec2(12.0, 14.0));
            let resp = ui.interact(hit, state_id.with(i), Sense::click());
            if resp.clicked() {
                st.page = i;
                st.last_flip = now;
            }
            // A held tile flips pages by resting on a dot.
            if drag.is_some()
                && pointer.is_some_and(|p| hit.expand(4.0).contains(p))
                && st.page != i
                && now - st.last_flip > DOT_FLIP
            {
                st.page = i;
                st.last_flip = now;
            }
            let on = i == st.page;
            let color = if on || resp.hovered() {
                theme.text_dim
            } else {
                theme.text_faint
            };
            ui.painter().circle_filled(
                c,
                if on { 3.0 } else { 2.5 },
                with_alpha(color, cctx.alpha),
            );
        }
    }
    ctx.data_mut(|d| d.insert_temp(state_id, st));

    // A held tile takes the place of the tile under the pointer; past the
    // last tile of the last page, it goes to the end.
    if let (Some(d), Some(p)) = (drag.as_mut(), pointer)
        && now - d.last_move > 0.12
        && grid.contains(p)
    {
        let held_pos = order.iter().position(|&i| ids[i] == d.id);
        let mut target = None;
        for pl in &pages[st.page] {
            let i = order[pl.index];
            if ids[i] == d.id {
                continue;
            }
            let r = GRID.cell_rect(grid.min, pl.col, pl.row, tiles[i].size);
            if r.shrink2(r.size() * 0.2).contains(p) {
                target = Some(pl.index);
            }
        }
        if target.is_none() && st.page + 1 == pages.len() {
            let right = pages[st.page]
                .iter()
                .filter(|pl| ids[order[pl.index]] != d.id)
                .map(|pl| {
                    GRID.cell_rect(grid.min, pl.col, pl.row, tiles[order[pl.index]].size)
                        .max
                        .x
                })
                .fold(f32::MIN, f32::max);
            if p.x > right + GRID.gap * 0.5 {
                target = Some(order.len() - 1);
            }
        }
        if let (Some(to), Some(from)) = (target, held_pos)
            && to != from
        {
            let held = d.order.remove(from);
            d.order.insert(to.min(d.order.len()), held);
            d.last_move = now;
        }
    }

    let scroll = anim::spring(
        &ctx,
        state_id.with("scroll"),
        st.page as f32,
        SpringParams::snappy(),
    );
    let step = GRID.size().y + PANEL_PADDING * 2.0;
    let clip = grid.expand(PANEL_PADDING - 2.0);
    let files_hovering = ctx.input(|i| !i.raw.hovered_files.is_empty());
    // Where the open tile is drawn, to tell clicks outside it.
    let mut open_rect: Option<Rect> = None;
    let mut new_drag: Option<Drag> = None;

    for (page_idx, placements) in pages.iter().enumerate() {
        let distance = page_idx as f32 - scroll;
        let shift = vec2(0.0, distance * step);
        for (slot, p) in placements.iter().enumerate() {
            let ti = order[p.index];
            let size = size_of(&tiles[ti]);
            let tile = &mut tiles[ti];
            let id = Id::new(("nook-tile", tile.module.id()));
            let held = drag.as_ref().is_some_and(|d| d.id == tile.module.id());
            let target = GRID.cell_rect(grid.min, p.col, p.row, size);
            let is_open = expanded.id == Some(tile.module.id());
            // Tiles glide to new places and sizes when the layout changes.
            let (rect, page_alpha) = match (held, pointer, &drag) {
                (true, Some(ptr), Some(d)) => {
                    let r = Rect::from_min_size(ptr - d.grab, target.size());
                    anim::spring_vec2(&ctx, id.with("pos"), r.min - grid.min - shift, true);
                    anim::spring_vec2(&ctx, id.with("size"), r.size(), true);
                    (r, 1.0)
                }
                _ => {
                    if distance.abs() >= 1.0 {
                        continue;
                    }
                    let pos = anim::spring_vec2(&ctx, id.with("pos"), target.min - grid.min, false);
                    let sz = anim::spring_vec2(&ctx, id.with("size"), target.size(), false);
                    (
                        Rect::from_min_size(grid.min + pos, sz).translate(shift),
                        1.0 - distance.abs(),
                    )
                }
            };
            if is_open {
                open_rect = Some(rect);
            }
            let ready = tile.module.status() == ModuleStatus::Ready;
            // An opening tile keeps its compact face until it has nearly
            // reached full size, then shows its full layout.
            let grown =
                rect.width() >= target.width() * 0.92 && rect.height() >= target.height() * 0.92;
            let compact = is_compact(tile) && !(is_open && grown);
            let stagger = cctx.stagger(slot);
            let mut alpha = cctx.alpha * stagger * page_alpha;
            if tile.hidden {
                alpha *= 0.5;
            }
            if alpha <= 0.002 {
                continue;
            }
            let interactive = page_idx == st.page || held;

            let area = Area::new(id)
                .order(if held {
                    Order::Foreground
                } else {
                    Order::Middle
                })
                .fixed_pos(rect.min)
                .movable(false)
                .constrain(false)
                .sense(Sense::hover())
                .show(&ctx, |ui| {
                    if !held {
                        ui.set_clip_rect(clip);
                    }
                    ui.multiply_opacity(alpha);
                    let sense = match (interactive, editing) {
                        (false, _) => Sense::hover(),
                        (true, true) => Sense::click_and_drag(),
                        (true, false) => Sense::click(),
                    };
                    // The area must cover the tile, or egui never finds its
                    // layer under the pointer and nothing in it hovers.
                    ui.expand_to_include_rect(rect);
                    let resp = ui.interact(rect, id.with("tile"), sense);
                    let hovered = interactive && ui.rect_contains_pointer(rect);
                    let pressed = hovered && ctx.input(|i| i.pointer.primary_down());
                    if hovered && drag.is_none() {
                        caption = Some((tile.module.name().to_owned(), ready || editing));
                    }
                    let lift = anim::spring(
                        &ctx,
                        id.with("lift"),
                        if (hovered && (ready || editing)) || held {
                            1.0
                        } else {
                            0.0
                        },
                        SpringParams::snappy(),
                    );
                    let press = anim::spring(
                        &ctx,
                        id.with("press"),
                        if pressed && ready && !editing {
                            1.0
                        } else {
                            0.0
                        },
                        SpringParams::critical(900.0),
                    );

                    let painter = ui.painter().clone();
                    if held {
                        painter.rect_filled(
                            rect.translate(vec2(0.0, 5.0)).expand(2.0),
                            TILE_RADIUS + 2.0,
                            Color32::from_black_alpha(70),
                        );
                    }
                    let active = ready && !editing && tile.module.is_active();
                    tile_glass(&painter, rect, theme, active, lift);
                    if is_open {
                        painter.rect_stroke(
                            rect.shrink(0.5),
                            TILE_RADIUS,
                            egui::Stroke::new(1.0, with_alpha(theme.accent, 0.6)),
                            egui::StrokeKind::Inside,
                        );
                    }
                    if editing {
                        painter.rect_stroke(
                            rect.shrink(0.5),
                            TILE_RADIUS,
                            egui::Stroke::new(1.0, with_alpha(theme.accent, 0.45)),
                            egui::StrokeKind::Inside,
                        );
                    }

                    let inner = tile_inner(rect, size);
                    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).id_salt(id));
                    if !held {
                        child.set_clip_rect(rect.intersect(clip));
                    }
                    if !interactive {
                        child.disable();
                    }
                    if editing {
                        if let Some(e) = edit_face(&mut child, rect, tile, theme) {
                            out.edits.push(e);
                        }
                    } else if compact {
                        compact_tile(&mut child, inner, tile.module.as_ref(), theme, hovered);
                    } else if ready {
                        let mut tctx = TileCtx {
                            theme,
                            now,
                            hovered,
                            files_hovering,
                        };
                        tile.module.ui_tile(&mut child, &mut tctx);
                    } else {
                        coming_soon(&mut child, inner, tile.module.icon(), theme, hovered, id);
                    }

                    if editing {
                        if resp.drag_started()
                            && let Some(ptr) = pointer
                        {
                            new_drag = Some(Drag {
                                id: tile.module.id(),
                                grab: ptr - rect.min,
                                order: order.iter().map(|&i| ids[i]).collect(),
                                last_move: now,
                            });
                        }
                    } else if resp.clicked() && ready {
                        if compact && !is_open {
                            expanded = Expanded {
                                id: Some(tile.module.id()),
                                since: now,
                                reveal: true,
                            };
                        } else {
                            out.clicked = Some(tile.module.id());
                        }
                    }
                    (lift, press)
                });

            let (lift, press) = area.inner;
            let scale = 1.0 - 0.03 * press + 0.05 * if held { lift } else { 0.0 };
            let c = rect.center().to_vec2();
            let slide = 8.0 * (1.0 - stagger) - 2.0 * lift;
            let tile_t = TSTransform::new(c * (1.0 - scale) + vec2(0.0, slide), scale);
            ctx.set_transform_layer(area.response.layer_id, cctx.transform * tile_t);
        }
    }

    // Picking up, and putting down: the new order is the one previewed.
    if let Some(d) = new_drag {
        drag = Some(d);
    }
    if let Some(d) = &drag
        && !ctx.input(|i| i.pointer.any_down())
    {
        if d.order != ids {
            out.edits.push(Edit::Reorder(d.order.clone()));
        }
        drag = None;
    }
    if drag.is_some() {
        ctx.request_repaint();
    }
    ctx.data_mut(|data| match drag {
        Some(d) => {
            data.insert_temp(drag_id, d);
        }
        None => {
            data.remove::<Drag>(drag_id);
        }
    });

    // A click anywhere but the open tile (another tile included, which
    // still gets its click) folds it back, and so does Esc.
    if let Some(id) = expanded.id {
        let clicked_outside = ctx.input(|i| i.pointer.primary_clicked())
            && pointer.is_some_and(|p| open_rect.is_none_or(|r| !r.contains(p)));
        if (now - expanded.since > 0.1 && clicked_outside)
            || ctx.input(|i| i.key_pressed(egui::Key::Escape))
        {
            expanded.id = None;
        } else if caption.is_none()
            && let Some(t) = tiles.iter().find(|t| t.module.id() == id)
        {
            caption = Some((
                format!("{}  ·  Click outside to fold", t.module.name()),
                true,
            ));
        }
    }
    ctx.data_mut(|d| d.insert_temp(expand_id, expanded));

    // Caption line in the bottom gutter: hovered tile name, or the edit hint.
    let caption = caption.or_else(|| {
        editing.then(|| {
            (
                "Drag to move  ·  tick to show  ·  pick a size".to_owned(),
                true,
            )
        })
    });
    if let Some((name, ready)) = caption {
        let text = if ready {
            name
        } else {
            format!("{name}  ·  Coming soon")
        };
        ui.painter().text(
            pos2(cctx.panel.center().x, gutter_y),
            Align2::CENTER_CENTER,
            text,
            Theme::font(SIZE_LABEL),
            with_alpha(theme.text_dim, cctx.alpha),
        );
    }
    out
}

/// The face of a tile in edit mode: its icon, the show tick under it, and
/// the size picker under that. Returns the edit made, if any.
fn edit_face(ui: &mut Ui, rect: Rect, tile: &GridTile<'_>, theme: &Theme) -> Option<Edit> {
    let ctx = ui.ctx().clone();
    let id = Id::new(("nook-edit", tile.module.id()));
    let mut edit = None;
    let shown = !tile.hidden;
    let on = anim::tween(&ctx, id.with("on"), shown, 0.15);
    let c = rect.center();
    let painter = ui.painter().clone();
    paint::icon(
        &painter,
        c - vec2(0.0, 14.0),
        tile.module.icon(),
        SIZE_ICON + 2.0,
        with_alpha(theme.text, 0.5 + 0.5 * on),
        false,
    );

    // The tick: filled accent when shown, an empty ring when hidden.
    let tick_c = c + vec2(0.0, 4.0);
    let tick = ui.interact(
        Rect::from_center_size(tick_c, vec2(20.0, 16.0)),
        id.with("tick"),
        Sense::click(),
    );
    let locked = !tile.module.can_hide();
    let hover = anim::tween(&ctx, id.with("tick-hover"), tick.hovered() && !locked, 0.1);
    let r = 6.0 + hover;
    painter.circle_stroke(
        tick_c,
        r,
        egui::Stroke::new(1.2, with_alpha(theme.text_dim, 1.0 - on)),
    );
    if on > 0.0 {
        let pop = 0.6 + 0.4 * anim::ease_out_cubic(on);
        painter.circle_filled(tick_c, r * pop, with_alpha(theme.accent, on));
        paint::icon(
            &painter,
            tick_c,
            icons::CHECK,
            8.5 * pop,
            with_alpha(Color32::WHITE, on),
            true,
        );
    }
    let tip = if locked {
        "Always shown"
    } else if shown {
        "Shown. Click to hide"
    } else {
        "Hidden. Click to show"
    };
    if tick.on_hover_text(tip).clicked() && !locked {
        edit = Some(Edit::ToggleShown(tile.module.id()));
    }

    // Size picker: 1x1, 2x1 and 2x2 drawn to scale.
    let unit = 5.0;
    let shape = |s: TileSize| {
        let (w, h) = s.cells();
        vec2(
            w as f32 * unit + (w - 1) as f32,
            h as f32 * unit + (h - 1) as f32,
        )
    };
    let total: f32 = TileSize::ALL.iter().map(|&s| shape(s).x).sum::<f32>() + 2.0 * 6.0;
    let mut x = c.x - total * 0.5;
    let y = c.y + 20.0;
    let native = tile.module.size_hint();
    for s in TileSize::ALL {
        let size = shape(s);
        let glyph = Rect::from_min_size(pos2(x, y - size.y * 0.5), size);
        x += size.x + 6.0;
        let hit = Rect::from_center_size(glyph.center(), vec2(size.x + 5.0, 16.0));
        let resp = ui.interact(hit, id.with(("size", s.area())), Sense::click());
        let selected = s == tile.size;
        let color = if selected {
            theme.accent
        } else if resp.hovered() {
            theme.text
        } else {
            theme.text_faint
        };
        if selected || resp.hovered() {
            painter.rect_filled(glyph, 1.5, color);
        } else {
            painter.rect_stroke(
                glyph,
                1.5,
                egui::Stroke::new(1.0, color),
                egui::StrokeKind::Inside,
            );
        }
        let label = match s {
            TileSize::Small => "Small",
            TileSize::Wide => "Wide",
            TileSize::Large => "Large",
        };
        let tip = if s.area() < native.area() {
            format!("{label}: compact, click it to open")
        } else {
            label.to_owned()
        };
        if resp.on_hover_text(tip).clicked() && !selected {
            edit = Some(Edit::Resize(tile.module.id(), s));
        }
    }
    edit
}

/// A module shown smaller than its layout needs: its icon (filled while
/// active), its name when there is room, and the live text of its badge
/// (e.g. the timer's time).
fn compact_tile(ui: &mut Ui, inner: Rect, module: &dyn Module, theme: &Theme, hovered: bool) {
    let active = module.is_active();
    let color = if active {
        theme.accent_fg()
    } else if hovered {
        theme.text
    } else {
        theme.text_dim
    };
    let label = match module.collapsed_badge() {
        Some(Badge::Ring { label, .. }) => Some(label),
        _ => None,
    };
    let painter = ui.painter();
    if inner.width() > inner.height() * 1.5 {
        // Wide: icon on the left, name and badge text beside it.
        let c = pos2(inner.min.x + 10.0, inner.center().y);
        paint::icon(painter, c, module.icon(), SIZE_ICON, color, active);
        let x = inner.min.x + 26.0;
        let (name_y, sub) = match &label {
            Some(l) => (inner.center().y - 7.0, Some(l.as_str())),
            None => (inner.center().y, None),
        };
        painter.text(
            pos2(x, name_y),
            Align2::LEFT_CENTER,
            module.name(),
            Theme::font_semibold(SIZE_LABEL + 1.0),
            theme.text,
        );
        if let Some(sub) = sub {
            painter.text(
                pos2(x, inner.center().y + 8.0),
                Align2::LEFT_CENTER,
                sub,
                Theme::font(SIZE_LABEL - 1.0),
                theme.text_dim,
            );
        }
    } else {
        let c = inner.center() - vec2(0.0, if label.is_some() { 6.0 } else { 0.0 });
        paint::icon(painter, c, module.icon(), SIZE_ICON + 2.0, color, active);
        if let Some(l) = label {
            painter.text(
                inner.center() + vec2(0.0, 13.0),
                Align2::CENTER_CENTER,
                l,
                Theme::font(SIZE_LABEL - 2.0),
                theme.text_dim,
            );
        }
    }
}

/// Disabled tile body: dim icon that crossfades to a "Soon" tag on hover.
fn coming_soon(ui: &mut Ui, inner: Rect, icon: &'static str, theme: &Theme, hovered: bool, id: Id) {
    let ctx = ui.ctx().clone();
    let t = anim::tween(&ctx, id.with("soon"), hovered, 0.12);
    let painter = ui.painter();
    let c = inner.center();
    paint::icon(
        painter,
        c,
        icon,
        SIZE_ICON + 2.0,
        with_alpha(theme.text_faint, 1.0 - t),
        false,
    );
    if t > 0.0 {
        painter.text(
            c,
            Align2::CENTER_CENTER,
            "Soon",
            Theme::font_semibold(SIZE_LABEL),
            with_alpha(theme.text_dim, t),
        );
    }
}

/// Standard tile header for modules: icon (regular/fill by state) and name.
pub fn tile_header(ui: &mut Ui, icon: &'static str, name: &str, active: bool, theme: &Theme) {
    let rect = ui.max_rect();
    let painter = ui.painter();
    let color = if active {
        theme.accent_fg()
    } else {
        theme.text_dim
    };
    let icon_center = pos2(rect.min.x + 9.0, rect.min.y + 9.0);
    let (prev, t) = anim::icon_crossfade(ui.ctx(), ui.id().with("hdr"), icon);
    if let Some(prev) = prev {
        paint::icon(
            painter,
            icon_center,
            prev,
            SIZE_ICON,
            with_alpha(color, 1.0 - t),
            !active,
        );
    }
    paint::icon(
        painter,
        icon_center,
        icon,
        SIZE_ICON,
        with_alpha(color, t),
        active,
    );
    painter.text(
        pos2(rect.min.x + 24.0, rect.min.y + 9.0),
        Align2::LEFT_CENTER,
        name,
        Theme::font_semibold(SIZE_LABEL),
        theme.text_dim,
    );
}
