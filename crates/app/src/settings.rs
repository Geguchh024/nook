//! Settings: the gear tile in the bento grid and the in-notch settings page.

use crate::config::{AccentSource, Config, Material, MonitorMode};
use egui::{Align, Align2, Area, Id, Layout, Order, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use nook_ui::bento::PANEL_PADDING;
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::shell::{ContentCtx, Placement};
use nook_ui::theme::{SIZE_BODY, SIZE_ICON, SIZE_LABEL, Theme, ThemeMode};
use nook_ui::{icons, paint, widgets};

pub const SETTINGS_ID: &str = "settings";

/// Height of the Plugins card under the two columns.
const PLUGINS_CARD: f32 = 52.0;
const GAP: f32 = 8.0;
/// How much taller than the regular panel the settings page is.
pub const EXTRA_HEIGHT: f32 = PLUGINS_CARD + GAP;

/// The gear tile. Clicking it opens the settings page (handled by the app).
pub struct SettingsTile;

impl Module for SettingsTile {
    fn id(&self) -> &'static str {
        SETTINGS_ID
    }

    fn name(&self) -> &str {
        "Settings"
    }

    fn icon(&self) -> &'static str {
        icons::GEAR_SIX
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn can_hide(&self) -> bool {
        false
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let c = ui.max_rect().center();
        let color = if ctx.hovered {
            ctx.theme.text
        } else {
            ctx.theme.text_dim
        };
        paint::icon(
            ui.painter(),
            c,
            icons::GEAR_SIX,
            SIZE_ICON + 2.0,
            color,
            false,
        );
    }
}

#[derive(Default)]
pub struct PageOutput {
    pub back: bool,
    pub changed: bool,
    pub startup_toggled: bool,
    /// "Manage plugins" was clicked.
    pub open_plugins: bool,
}

/// The settings page, laid out in the expanded panel.
pub fn page(ui: &mut Ui, cctx: &ContentCtx, theme: &Theme, cfg: &mut Config) -> PageOutput {
    let mut out = PageOutput::default();
    let ctx = ui.ctx().clone();
    let alpha = cctx.alpha * cctx.stagger(0);
    if alpha <= 0.002 {
        return out;
    }
    let panel = cctx.panel.shrink(PANEL_PADDING);
    let area = Area::new(Id::new("nook-settings"))
        .order(Order::Middle)
        .fixed_pos(panel.min)
        .movable(false)
        .constrain(false)
        .sense(Sense::hover())
        .show(&ctx, |ui| {
            ui.multiply_opacity(alpha);
            ui.set_min_size(panel.size());

            // Header: back button + title.
            let header = Rect::from_min_size(panel.min, vec2(panel.width(), 28.0));
            let mut hui = ui.new_child(
                UiBuilder::new()
                    .max_rect(header)
                    .layout(Layout::left_to_right(Align::Center)),
            );
            if widgets::icon_button(&mut hui, icons::ARROW_LEFT, 28.0, false, theme).clicked() {
                out.back = true;
            }
            hui.add_space(4.0);
            hui.label(
                egui::RichText::new("Settings")
                    .font(Theme::font_semibold(SIZE_BODY + 1.0))
                    .color(theme.text),
            );

            // Two cards, and the Plugins card across the bottom.
            let full = Rect::from_min_max(pos2(panel.min.x, header.max.y + 8.0), panel.max);
            let body = Rect::from_min_max(full.min, pos2(full.max.x, full.max.y - EXTRA_HEIGHT));
            let plugins = Rect::from_min_max(pos2(full.min.x, body.max.y + GAP), full.max);
            let gap = GAP;
            let col_w = (body.width() - gap) * 0.5;
            let left = Rect::from_min_size(body.min, vec2(col_w, body.height()));
            let right = Rect::from_min_size(
                pos2(body.min.x + col_w + gap, body.min.y),
                vec2(col_w, body.height()),
            );

            widgets::card(ui.painter(), left, theme);
            widgets::card(ui.painter(), right, theme);

            let mut lui = card_ui(ui, left);
            section_title(&mut lui, "Appearance", theme);
            let mut theme_idx = match cfg.theme {
                ThemeMode::Auto => 0,
                ThemeMode::Dark => 1,
                ThemeMode::Light => 2,
            };
            if row(&mut lui, "Theme", theme, |ui| {
                widgets::segmented(ui, &["Auto", "Dark", "Light"], &mut theme_idx, theme)
            }) {
                cfg.theme = [ThemeMode::Auto, ThemeMode::Dark, ThemeMode::Light][theme_idx];
                out.changed = true;
            }
            let mut accent_idx = match cfg.accent {
                AccentSource::Wallpaper => 0,
                AccentSource::System => 1,
                AccentSource::Custom => 2,
            };
            if row(&mut lui, "Accent", theme, |ui| {
                widgets::segmented(
                    ui,
                    &["Wallpaper", "Windows", "Violet"],
                    &mut accent_idx,
                    theme,
                )
            }) {
                cfg.accent = [
                    AccentSource::Wallpaper,
                    AccentSource::System,
                    AccentSource::Custom,
                ][accent_idx];
                if cfg.accent == AccentSource::Custom {
                    cfg.accent_hex = "#7C5CFF".into();
                }
                out.changed = true;
            }
            let mut mat_idx = match cfg.material {
                Material::Acrylic => 0,
                Material::Solid => 1,
            };
            if row(&mut lui, "Material", theme, |ui| {
                widgets::segmented(ui, &["Glass", "Solid"], &mut mat_idx, theme)
            }) {
                cfg.material = [Material::Acrylic, Material::Solid][mat_idx];
                out.changed = true;
            }
            let mut pos_idx = Placement::ALL
                .iter()
                .position(|&p| p == cfg.placement)
                .unwrap_or(1);
            if row(&mut lui, "Position", theme, |ui| {
                widgets::segmented(ui, &["Left", "Center", "Right"], &mut pos_idx, theme)
            }) {
                cfg.placement = Placement::ALL[pos_idx];
                out.changed = true;
            }

            let mut rui = card_ui(ui, right);
            section_title(&mut rui, "Behavior", theme);
            let mut mon_idx = match cfg.monitor {
                MonitorMode::Primary => 0,
                MonitorMode::Cursor => 1,
            };
            if row(&mut rui, "Monitor", theme, |ui| {
                widgets::segmented(ui, &["Primary", "Follow cursor"], &mut mon_idx, theme)
            }) {
                cfg.monitor = [MonitorMode::Primary, MonitorMode::Cursor][mon_idx];
                out.changed = true;
            }
            const DELAYS: [u32; 3] = [60, 120, 250];
            let mut delay_idx = DELAYS
                .iter()
                .position(|&d| d == cfg.hover_delay_ms)
                .unwrap_or(1);
            if row(&mut rui, "Hover delay", theme, |ui| {
                widgets::segmented(ui, &["Fast", "Normal", "Calm"], &mut delay_idx, theme)
            }) {
                cfg.hover_delay_ms = DELAYS[delay_idx];
                out.changed = true;
            }
            let mut on_top = cfg.on_top;
            if row(&mut rui, "Always on top", theme, |ui| {
                widgets::toggle(ui, &mut on_top, theme)
            }) {
                cfg.on_top = on_top;
                out.changed = true;
            }
            let mut startup = cfg.launch_at_startup;
            if row(&mut rui, "Launch at startup", theme, |ui| {
                widgets::toggle(ui, &mut startup, theme)
            }) {
                cfg.launch_at_startup = startup;
                out.startup_toggled = true;
                out.changed = true;
            }

            widgets::card(ui.painter(), plugins, theme);
            plugins_card(ui, plugins, theme, cfg, &mut out);
        });
    let slide = 8.0 * (1.0 - cctx.stagger(0));
    ctx.set_transform_layer(
        area.response.layer_id,
        cctx.transform * egui::emath::TSTransform::from_translation(vec2(0.0, slide)),
    );
    out
}

/// Plugin dev mode and the way to the Plugins page.
fn plugins_card(ui: &mut Ui, rect: Rect, theme: &Theme, cfg: &mut Config, out: &mut PageOutput) {
    let inner = rect.shrink2(vec2(14.0, 0.0));
    let painter = ui.painter();
    paint::icon(
        painter,
        pos2(inner.min.x + 10.0, inner.center().y),
        icons::CODE,
        SIZE_ICON,
        theme.text_dim,
        false,
    );
    let x = inner.min.x + 30.0;
    painter.text(
        pos2(x, inner.center().y - 8.0),
        Align2::LEFT_CENTER,
        "Plugin dev mode",
        Theme::font(SIZE_BODY),
        theme.text,
    );
    painter.text(
        pos2(x, inner.center().y + 9.0),
        Align2::LEFT_CENTER,
        "Restarts a plugin each time you save one of its files",
        Theme::font(SIZE_LABEL - 1.0),
        theme.text_dim,
    );
    let manage = Rect::from_min_size(
        pos2(inner.max.x - 132.0, inner.center().y - 14.0),
        vec2(132.0, 28.0),
    );
    if nook_modules::kit::button(
        ui,
        manage,
        "Manage plugins",
        Some(icons::PUZZLE_PIECE),
        false,
        theme,
    )
    .clicked()
    {
        out.open_plugins = true;
    }
    let toggle = Rect::from_center_size(
        pos2(
            manage.min.x - 14.0 - widgets::TOGGLE_SIZE.x * 0.5,
            inner.center().y,
        ),
        widgets::TOGGLE_SIZE,
    );
    let mut tui = ui.new_child(UiBuilder::new().max_rect(toggle));
    let mut dev = cfg.plugin_dev;
    if widgets::toggle(&mut tui, &mut dev, theme).changed() {
        cfg.plugin_dev = dev;
        out.changed = true;
    }
}

fn card_ui(ui: &mut Ui, rect: Rect) -> Ui {
    ui.new_child(
        UiBuilder::new()
            .max_rect(rect.shrink2(vec2(14.0, 10.0)))
            .layout(Layout::top_down(Align::Min)),
    )
}

fn section_title(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.label(
        egui::RichText::new(text.to_uppercase())
            .font(Theme::font_semibold(SIZE_LABEL - 1.0))
            .color(theme.text_faint),
    );
    ui.add_space(2.0);
}

/// Label on the left, control on the right. Returns whether it changed.
fn row(
    ui: &mut Ui,
    label: &str,
    theme: &Theme,
    control: impl FnOnce(&mut Ui) -> egui::Response,
) -> bool {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
    ui.painter().text(
        pos2(rect.min.x, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        Theme::font(SIZE_BODY),
        theme.text,
    );
    let mut cui = ui.new_child(
        UiBuilder::new()
            .max_rect(rect)
            .layout(Layout::right_to_left(Align::Center)),
    );
    control(&mut cui).changed()
}
