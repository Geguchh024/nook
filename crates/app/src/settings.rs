//! Settings: the gear tile in the bento grid and the in-notch settings page.

use crate::config::{AccentSource, Config, GrainSize, Material, MonitorMode};
use crate::update::{self, Status};
use egui::{
    Align, Align2, Area, Color32, Id, Layout, Order, Rect, Sense, Stroke, Ui, UiBuilder, pos2, vec2,
};
use nook_ui::bento::PANEL_PADDING;
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::shell::{ContentCtx, Placement};
use nook_ui::theme::{
    FALLBACK_ACCENT, SIZE_BODY, SIZE_ICON, SIZE_LABEL, Theme, ThemeMode, parse_hex, to_hex,
    with_alpha,
};
use nook_ui::{icons, paint, widgets};

pub const SETTINGS_ID: &str = "settings";

const GAP: f32 = 8.0;
const HEADER: f32 = 28.0;
const ROW: f32 = 28.0;
/// Height of the Appearance and Behavior cards (title + four rows).
const TOP_CARD: f32 = 154.0;
/// Height of a collapsible card's clickable header.
const DISCLOSURE: f32 = 44.0;
/// Revealed height of each collapsible section's body.
const BODY: [f32; 2] = [4.0 * ROW + 14.0, 2.0 * ROW + 14.0];
/// Page height with every section collapsed, panel padding included.
const BASE: f32 =
    2.0 * PANEL_PADDING + HEADER + GAP + TOP_CARD + GAP + DISCLOSURE + GAP + DISCLOSURE;

/// Collapsible sections, in page order.
const CUSTOMIZE: usize = 0;
const ADVANCED: usize = 1;

/// Which collapsible section is open (at most one at a time).
fn open_section(ctx: &egui::Context) -> Option<usize> {
    ctx.data(|d| d.get_temp(Id::new("nook-settings-open")))
        .flatten()
}

fn set_open_section(ctx: &egui::Context, section: Option<usize>) {
    ctx.data_mut(|d| d.insert_temp(Id::new("nook-settings-open"), section));
}

/// How much taller than the regular panel (`regular`) the settings page
/// is; follows the open section, so the panel springs along with it.
pub fn extra_height(ctx: &egui::Context, regular: f32) -> f32 {
    let body = open_section(ctx).map_or(0.0, |i| BODY[i]);
    (BASE + body - regular).max(0.0)
}

/// Spring for a section's 0..1 reveal, tuned like the panel height so the
/// card and the panel move together.
fn reveal(ctx: &egui::Context, section: usize, open: bool) -> f32 {
    nook_ui::anim::spring(
        ctx,
        Id::new("nook-settings-reveal").with(section),
        if open { 1.0 } else { 0.0 },
        nook_ui::spring::SpringParams::with_overshoot(220.0, 0.04),
    )
    .max(0.0)
}

/// Background tints offered in the Style card; `None` is neutral glass.
const BACKGROUNDS: [Option<Color32>; 7] = [
    None,
    Some(Color32::from_rgb(0x4A, 0x6F, 0xA5)),
    Some(Color32::from_rgb(0x2A, 0x9D, 0x8F)),
    Some(Color32::from_rgb(0x4F, 0x7D, 0x4A)),
    Some(Color32::from_rgb(0xD0, 0x8C, 0x3C)),
    Some(Color32::from_rgb(0xC8, 0x56, 0x6E)),
    Some(Color32::from_rgb(0x8A, 0x5C, 0xC2)),
];

/// Accent colors for the "Custom" accent.
const ACCENTS: [Option<Color32>; 7] = [
    Some(FALLBACK_ACCENT),
    Some(Color32::from_rgb(0x3D, 0x8B, 0xFF)),
    Some(Color32::from_rgb(0x18, 0xB4, 0xA6)),
    Some(Color32::from_rgb(0x3D, 0xBE, 0x6A)),
    Some(Color32::from_rgb(0xFF, 0x8A, 0x3D)),
    Some(Color32::from_rgb(0xFF, 0x4F, 0x8B)),
    Some(Color32::from_rgb(0xF0, 0x4A, 0x4A)),
];

/// Glass opacity slider range, in percent.
const OPACITY_MIN: u8 = 40;
const OPACITY_MAX: u8 = 100;

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
    /// "Check for updates" was clicked.
    pub check_update: bool,
    /// The Update button was clicked.
    pub install_update: bool,
}

/// The settings page, laid out in the expanded panel.
pub fn page(
    ui: &mut Ui,
    cctx: &ContentCtx,
    theme: &Theme,
    cfg: &mut Config,
    update: &Status,
) -> PageOutput {
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
            update_header(ui, header, theme, update, &mut out);

            // Appearance and Behavior side by side, then the collapsible
            // Customize and Advanced cards.
            let top = Rect::from_min_size(
                pos2(panel.min.x, header.max.y + GAP),
                vec2(panel.width(), TOP_CARD),
            );
            let col_w = (top.width() - GAP) * 0.5;
            let left = Rect::from_min_size(top.min, vec2(col_w, top.height()));
            let right = Rect::from_min_size(
                pos2(top.min.x + col_w + GAP, top.min.y),
                vec2(col_w, top.height()),
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
                    &["Wallpaper", "Windows", "Custom"],
                    &mut accent_idx,
                    theme,
                )
            }) {
                cfg.accent = [
                    AccentSource::Wallpaper,
                    AccentSource::System,
                    AccentSource::Custom,
                ][accent_idx];
                if parse_hex(&cfg.accent_hex).is_none() {
                    cfg.accent_hex = to_hex(FALLBACK_ACCENT);
                }
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
            let mut hide = cfg.hide_in_fullscreen;
            if row(&mut lui, "Hide in fullscreen", theme, |ui| {
                widgets::toggle(ui, &mut hide, theme)
                    .on_hover_text("Fade the notch out while a game or video is fullscreen")
            }) {
                cfg.hide_in_fullscreen = hide;
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

            let open = open_section(&ctx);
            let mut y = top.max.y + GAP;
            for section in [CUSTOMIZE, ADVANCED] {
                let t = reveal(&ctx, section, open == Some(section));
                let card = Rect::from_min_size(
                    pos2(panel.min.x, y),
                    vec2(panel.width(), DISCLOSURE + BODY[section] * t),
                );
                y = card.max.y + GAP;
                widgets::card(ui.painter(), card, theme);
                let head = Rect::from_min_size(card.min, vec2(card.width(), DISCLOSURE));
                let (glyph, title, summary) = match section {
                    CUSTOMIZE => (icons::PALETTE, "Customize", customize_summary(cfg)),
                    _ => (icons::SLIDERS_HORIZONTAL, "Advanced", advanced_summary(cfg)),
                };
                if widgets::disclosure_header(ui, head, glyph, title, &summary, t, theme).clicked()
                {
                    let next = (open != Some(section)).then_some(section);
                    set_open_section(&ctx, next);
                }
                if section == ADVANCED {
                    manage_plugins_button(ui, head, theme, &mut out);
                }
                if t > 0.01 {
                    // Hairline under the header, then the controls fading
                    // in and settling down from slightly above.
                    let fade = t.clamp(0.0, 1.0);
                    ui.painter().hline(
                        (card.min.x + 14.0)..=(card.max.x - 14.0),
                        head.max.y,
                        Stroke::new(1.0, with_alpha(theme.tile_rim, fade)),
                    );
                    let body = Rect::from_min_size(
                        pos2(card.min.x, head.max.y - 6.0 * (1.0 - fade)),
                        vec2(card.width(), BODY[section]),
                    );
                    let mut bui = ui.new_child(UiBuilder::new().max_rect(body));
                    bui.set_clip_rect(card.intersect(ui.clip_rect()));
                    bui.multiply_opacity(fade * fade);
                    match section {
                        CUSTOMIZE => customize_body(&mut bui, body, theme, cfg, &mut out),
                        _ => advanced_body(&mut bui, body, theme, cfg, &mut out),
                    }
                }
            }
        });
    let slide = 8.0 * (1.0 - cctx.stagger(0));
    ctx.set_transform_layer(
        area.response.layer_id,
        cctx.transform * egui::emath::TSTransform::from_translation(vec2(0.0, slide)),
    );
    out
}

/// Right end of the header: the running version and either the Update
/// chip or a "Check for updates" button with the last result.
fn update_header(ui: &mut Ui, header: Rect, theme: &Theme, status: &Status, out: &mut PageOutput) {
    let version = format!("Nook {}", update::CURRENT);
    let font = Theme::font(SIZE_LABEL);
    let right = pos2(header.max.x, header.center().y);
    if let Some(chip) = update::chip(ui, right, Align2::RIGHT_CENTER, status, theme) {
        out.install_update = chip.clicked;
        ui.painter().text(
            pos2(chip.rect.min.x - 10.0, header.center().y),
            Align2::RIGHT_CENTER,
            version,
            font,
            theme.text_faint,
        );
        return;
    }
    let button = Rect::from_min_size(
        pos2(header.max.x - 136.0, header.center().y - 13.0),
        vec2(136.0, 26.0),
    );
    let checking = *status == Status::Checking;
    let resp = ui
        .add_enabled_ui(!checking, |ui| {
            nook_modules::kit::button(
                ui,
                button,
                if checking {
                    "Checking…"
                } else {
                    "Check for updates"
                },
                Some(icons::ARROW_CLOCKWISE),
                false,
                theme,
            )
        })
        .inner;
    if resp.clicked() {
        out.check_update = true;
    }
    let note = match status {
        Status::UpToDate => format!("{version} · up to date"),
        Status::Failed(e) => e.clone(),
        _ => version,
    };
    let color = if matches!(status, Status::Failed(_)) {
        theme.text_dim
    } else {
        theme.text_faint
    };
    ui.painter().text(
        pos2(button.min.x - 10.0, header.center().y),
        Align2::RIGHT_CENTER,
        note,
        font,
        color,
    );
}

/// One line beside the Customize title while it is collapsed.
fn customize_summary(cfg: &Config) -> String {
    let material = match cfg.material {
        Material::Acrylic => format!("Glass {}%", cfg.glass_opacity),
        Material::Solid => "Solid".to_owned(),
    };
    let tint = if parse_hex(&cfg.background_hex).is_some() {
        "tinted"
    } else {
        "neutral"
    };
    let grain = if cfg.grain == 0 {
        "no grain".to_owned()
    } else {
        format!("grain {}%", cfg.grain)
    };
    format!("{material} · {tint} · {grain}")
}

fn advanced_summary(cfg: &Config) -> String {
    let on = |b: bool| if b { "on" } else { "off" };
    format!(
        "Dev mode {} · usage stats {}",
        on(cfg.plugin_dev),
        on(cfg.usage_stats)
    )
}

/// Material, background tint, glass opacity, accent color and film grain,
/// in two columns.
fn customize_body(ui: &mut Ui, rect: Rect, theme: &Theme, cfg: &mut Config, out: &mut PageOutput) {
    let inner = rect.shrink2(vec2(14.0, 6.0));
    let gap = 20.0;
    let col_w = (inner.width() - gap) * 0.5;
    let left = Rect::from_min_size(inner.min, vec2(col_w, inner.height()));
    let right = Rect::from_min_size(
        pos2(inner.min.x + col_w + gap, inner.min.y),
        vec2(col_w, inner.height()),
    );
    let column = |ui: &mut Ui, r: Rect| {
        ui.new_child(
            UiBuilder::new()
                .max_rect(r)
                .layout(Layout::top_down(Align::Min)),
        )
    };

    let mut lui = column(ui, left);
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
    let mut bg_idx = match parse_hex(&cfg.background_hex) {
        None => Some(0),
        Some(c) => BACKGROUNDS.iter().position(|b| *b == Some(c)),
    };
    if row(&mut lui, "Background", theme, |ui| {
        widgets::swatches(ui, &BACKGROUNDS, &mut bg_idx, theme)
    }) && let Some(i) = bg_idx
    {
        cfg.background_hex = BACKGROUNDS[i].map(to_hex).unwrap_or_default();
        out.changed = true;
    }
    let glass = cfg.material == Material::Acrylic;
    let span = (OPACITY_MAX - OPACITY_MIN) as f32;
    let mut t = (cfg.glass_opacity.clamp(OPACITY_MIN, OPACITY_MAX) - OPACITY_MIN) as f32 / span;
    if row(&mut lui, "Glass opacity", theme, |ui| {
        if !glass {
            // The solid material is always opaque.
            ui.disable();
        }
        let resp = widgets::slider(ui, &mut t, 110.0, 12, theme);
        value_label(ui, &format!("{}%", cfg.glass_opacity), theme);
        resp
    }) {
        cfg.glass_opacity = OPACITY_MIN + (t * span).round() as u8;
        out.changed = true;
    }
    let mut accent_idx = match cfg.accent {
        AccentSource::Custom => {
            parse_hex(&cfg.accent_hex).and_then(|c| ACCENTS.iter().position(|a| *a == Some(c)))
        }
        _ => None,
    };
    if row(&mut lui, "Accent color", theme, |ui| {
        widgets::swatches(ui, &ACCENTS, &mut accent_idx, theme)
    }) && let Some(Some(c)) = accent_idx.map(|i| ACCENTS[i])
    {
        cfg.accent = AccentSource::Custom;
        cfg.accent_hex = to_hex(c);
        out.changed = true;
    }

    let mut rui = column(ui, right);
    let mut grain = cfg.grain.min(100) as f32 / 100.0;
    if row(&mut rui, "Grain", theme, |ui| {
        let resp = widgets::slider(ui, &mut grain, 110.0, 20, theme);
        let text = if cfg.grain == 0 {
            "Off".to_owned()
        } else {
            format!("{}%", cfg.grain)
        };
        value_label(ui, &text, theme);
        resp
    }) {
        cfg.grain = (grain * 100.0).round() as u8;
        out.changed = true;
    }
    let mut size_idx = GrainSize::ALL
        .iter()
        .position(|&s| s == cfg.grain_size)
        .unwrap_or(0);
    if row(&mut rui, "Grain size", theme, |ui| {
        if cfg.grain == 0 {
            ui.disable();
        }
        widgets::segmented(ui, &["Fine", "Medium", "Coarse"], &mut size_idx, theme)
    }) {
        cfg.grain_size = GrainSize::ALL[size_idx];
        out.changed = true;
    }
    let w = rui.available_width();
    let (reset_row, _) = rui.allocate_exact_size(vec2(w, ROW), Sense::hover());
    let reset = Rect::from_min_size(
        pos2(reset_row.max.x - 120.0, reset_row.center().y - 13.0),
        vec2(120.0, 26.0),
    );
    if nook_modules::kit::button(
        ui,
        reset,
        "Reset style",
        Some(icons::ARROW_COUNTER_CLOCKWISE),
        false,
        theme,
    )
    .clicked()
    {
        cfg.reset_look();
        out.changed = true;
    }
}

/// Small dim value text next to a slider (right-to-left row layout).
fn value_label(ui: &mut Ui, text: &str, theme: &Theme) {
    let (rect, _) = ui.allocate_exact_size(vec2(32.0, 20.0), Sense::hover());
    ui.painter().text(
        pos2(rect.max.x - 4.0, rect.center().y),
        Align2::RIGHT_CENTER,
        text,
        Theme::font(SIZE_LABEL),
        theme.text_dim,
    );
}

/// "Manage plugins" at the right of the Advanced header, left of the chevron.
fn manage_plugins_button(ui: &mut Ui, head: Rect, theme: &Theme, out: &mut PageOutput) {
    let manage = Rect::from_min_size(
        pos2(head.max.x - 44.0 - 132.0, head.center().y - 13.0),
        vec2(132.0, 26.0),
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
}

/// Plugin dev mode and anonymous usage stats, side by side.
fn advanced_body(ui: &mut Ui, rect: Rect, theme: &Theme, cfg: &mut Config, out: &mut PageOutput) {
    let inner = rect.shrink2(vec2(14.0, 6.0));
    let gap = 20.0;
    let col_w = (inner.width() - gap) * 0.5;
    let mut lui = ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_size(inner.min, vec2(col_w, inner.height())))
            .layout(Layout::top_down(Align::Min)),
    );
    let mut rui = ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_size(
                pos2(inner.min.x + col_w + gap, inner.min.y),
                vec2(col_w, inner.height()),
            ))
            .layout(Layout::top_down(Align::Min)),
    );
    let mut dev = cfg.plugin_dev;
    if row(&mut lui, "Plugin dev mode", theme, |ui| {
        widgets::toggle(ui, &mut dev, theme)
    }) {
        cfg.plugin_dev = dev;
        out.changed = true;
    }
    hint(&mut lui, "Restarts a plugin when you save its files", theme);
    let mut usage = cfg.usage_stats;
    if row(&mut rui, "Anonymous usage stats", theme, |ui| {
        widgets::toggle(ui, &mut usage, theme)
    }) {
        cfg.usage_stats = usage;
        out.changed = true;
    }
    hint(
        &mut rui,
        "Install id, version and Windows build, daily",
        theme,
    );
}

/// Small faint line under a row.
fn hint(ui: &mut Ui, text: &str, theme: &Theme) {
    ui.label(
        egui::RichText::new(text)
            .font(Theme::font(SIZE_LABEL - 1.0))
            .color(theme.text_faint),
    );
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
    let (rect, _) = ui.allocate_exact_size(vec2(w, ROW), Sense::hover());
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
