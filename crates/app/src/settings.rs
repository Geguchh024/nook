//! Settings: the gear tile in the bento grid and the in-notch settings page.

use crate::config::{
    AccentSource, Config, GrainSize, Material, MonitorMode, NotchSize, Renderer, RetentionPeriod,
    ShellStyle,
};
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
/// Height of a collapsible card's clickable header.
const DISCLOSURE: f32 = 44.0;
/// Revealed height of each collapsible section's body.
const BODY: [f32; 5] = [
    6.0 * ROW + 12.0,
    7.0 * ROW + 16.0,
    9.0 * ROW + 16.0,
    6.0 * ROW + 12.0,
    5.0 * ROW + 36.0,
];
/// Collapsible sections, in page order.
const APPEARANCE: usize = 0;
const BEHAVIOR: usize = 1;
const CUSTOMIZE: usize = 2;
const MOTION: usize = 3;
const ADVANCED: usize = 4;

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
pub fn extra_height(ctx: &egui::Context, regular: f32, panel_width: f32) -> f32 {
    let body = open_section(ctx).map_or(0.0, |i| BODY[i]);
    (base_height(panel_width) + body - regular).max(0.0)
}

fn top_height(_width: f32, open: Option<usize>) -> f32 {
    2.0 * DISCLOSURE + GAP + open.filter(|i| *i < 2).map_or(0.0, |i| BODY[i])
}

fn base_height(width: f32) -> f32 {
    2.0 * PANEL_PADDING
        + HEADER
        + GAP
        + top_height(width, None)
        + GAP
        + 5.0 * DISCLOSURE
        + 4.0 * GAP
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

            // Appearance and Behavior use one-at-a-time disclosures so the
            // controls never overflow the available panel width.
            let open = open_section(&ctx);
            let top = Rect::from_min_size(
                pos2(panel.min.x, header.max.y + GAP),
                vec2(panel.width(), top_height(panel.width(), open)),
            );
            let appearance_rect = Rect::from_min_size(top.min, vec2(top.width(), DISCLOSURE));
            let behavior_rect = Rect::from_min_size(
                pos2(
                    top.min.x,
                    appearance_rect.max.y
                        + if open == Some(APPEARANCE) {
                            BODY[APPEARANCE]
                        } else {
                            0.0
                        }
                        + GAP,
                ),
                vec2(top.width(), DISCLOSURE),
            );
            let appearance_body = Rect::from_min_size(
                pos2(top.min.x, appearance_rect.max.y),
                vec2(
                    top.width(),
                    BODY[APPEARANCE] * reveal(&ctx, APPEARANCE, open == Some(APPEARANCE)),
                ),
            );
            let behavior_body = Rect::from_min_size(
                pos2(top.min.x, behavior_rect.max.y),
                vec2(
                    top.width(),
                    BODY[BEHAVIOR] * reveal(&ctx, BEHAVIOR, open == Some(BEHAVIOR)),
                ),
            );
            widgets::card(
                ui.painter(),
                Rect::from_min_max(appearance_rect.min, appearance_body.max),
                theme,
            );
            widgets::card(
                ui.painter(),
                Rect::from_min_max(behavior_rect.min, behavior_body.max),
                theme,
            );
            if widgets::disclosure_header(
                ui,
                appearance_rect,
                icons::PALETTE,
                "Appearance",
                "Style · clock · theme · accent · position",
                reveal(&ctx, APPEARANCE, open == Some(APPEARANCE)),
                theme,
            )
            .clicked()
            {
                set_open_section(&ctx, (open != Some(APPEARANCE)).then_some(APPEARANCE));
            }
            if widgets::disclosure_header(
                ui,
                behavior_rect,
                icons::SLIDERS_HORIZONTAL,
                "Behavior",
                "Displays · startup · sound · fullscreen",
                reveal(&ctx, BEHAVIOR, open == Some(BEHAVIOR)),
                theme,
            )
            .clicked()
            {
                set_open_section(&ctx, (open != Some(BEHAVIOR)).then_some(BEHAVIOR));
            }

            if open == Some(APPEARANCE) {
                let mut lui = card_ui(ui, appearance_body);
                let mut style_idx = usize::from(cfg.shell_style == ShellStyle::Notch);
                if row(&mut lui, "Style", theme, |ui| {
                    widgets::segmented(ui, &["Pill", "Notch"], &mut style_idx, theme)
                }) {
                    cfg.shell_style = [ShellStyle::Pill, ShellStyle::Notch][style_idx];
                    out.changed = true;
                }
                let mut notch_size_idx = NotchSize::ALL
                    .iter()
                    .position(|&size| size == cfg.notch_size)
                    .unwrap_or(1);
                if row(&mut lui, "Notch size", theme, |ui| {
                    if cfg.shell_style != ShellStyle::Notch {
                        ui.disable();
                    }
                    widgets::segmented(
                        ui,
                        &["Compact", "Standard", "Large"],
                        &mut notch_size_idx,
                        theme,
                    )
                }) {
                    cfg.notch_size = NotchSize::ALL[notch_size_idx];
                    out.changed = true;
                }
                let mut clock_idx = match cfg.clock_style {
                    crate::config::ClockStyle::Classic => 0,
                    crate::config::ClockStyle::Digital => 1,
                    crate::config::ClockStyle::DotMatrix => 2,
                };
                if row(&mut lui, "Clock", theme, |ui| {
                    widgets::segmented(
                        ui,
                        &["Classic", "Digital", "Dot matrix"],
                        &mut clock_idx,
                        theme,
                    )
                }) {
                    cfg.clock_style = [
                        crate::config::ClockStyle::Classic,
                        crate::config::ClockStyle::Digital,
                        crate::config::ClockStyle::DotMatrix,
                    ][clock_idx];
                    out.changed = true;
                }
                let mut theme_idx = match cfg.theme {
                    ThemeMode::Auto => 0,
                    ThemeMode::Dark => 1,
                    ThemeMode::Light => 2,
                };
                if row(&mut lui, "Theme", theme, |ui| {
                    if cfg.material == Material::Oled {
                        ui.disable();
                    }
                    widgets::segmented(ui, &["Auto", "Dark", "Light"], &mut theme_idx, theme)
                        .on_hover_text(if cfg.material == Material::Oled {
                            "OLED material uses a fixed black theme"
                        } else {
                            "Choose the interface theme"
                        })
                }) && cfg.material != Material::Oled
                {
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
            }
            if open == Some(BEHAVIOR) {
                let mut rui = card_ui(ui, behavior_body);
                let mut mon_idx = match cfg.monitor {
                    MonitorMode::Primary => 0,
                    MonitorMode::Cursor => 1,
                    MonitorMode::Display | MonitorMode::SelectedDisplays => 2,
                };
                if row(&mut rui, "Monitor", theme, |ui| {
                    egui::ComboBox::from_id_salt("nook-monitor-mode")
                        .width(ui.available_width())
                        .selected_text(["Primary", "Follow cursor", "Choose displays…"][mon_idx])
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut mon_idx, 0, "Primary display");
                            ui.selectable_value(&mut mon_idx, 1, "Follow cursor");
                            ui.selectable_value(&mut mon_idx, 2, "Choose displays…");
                        })
                        .response
                }) {
                    cfg.monitor = [
                        MonitorMode::Primary,
                        MonitorMode::Cursor,
                        MonitorMode::SelectedDisplays,
                    ][mon_idx];
                    if cfg.monitor == MonitorMode::SelectedDisplays
                        && cfg.monitor_displays.is_empty()
                    {
                        cfg.monitor_displays.push(cfg.monitor_display);
                    }
                    out.changed = true;
                }
                if matches!(
                    cfg.monitor,
                    MonitorMode::Display | MonitorMode::SelectedDisplays
                ) {
                    let displays = nook_platform::monitor::all();
                    if !displays.is_empty() {
                        let display_idx = (cfg.monitor_display as usize).min(displays.len() - 1);
                        let names: Vec<String> = displays
                            .iter()
                            .enumerate()
                            .map(|(index, display)| {
                                format!(
                                    "Display {} · {}×{} · {}, {}",
                                    index + 1,
                                    display.bounds.width(),
                                    display.bounds.height(),
                                    display.bounds.left,
                                    display.bounds.top,
                                )
                            })
                            .collect();
                        if cfg.monitor_displays.is_empty() {
                            cfg.monitor_displays.push(display_idx as u8);
                            out.changed = true;
                        }
                        if cfg.monitor == MonitorMode::Display {
                            cfg.monitor = MonitorMode::SelectedDisplays;
                            out.changed = true;
                        }
                        {
                            let mut selection = cfg.monitor_displays.clone();
                            let mut selection_changed = false;
                            row(&mut rui, "Displays", theme, |ui| {
                                let count = selection.len();
                                egui::ComboBox::from_id_salt("nook-target-displays")
                                    .width(ui.available_width())
                                    .selected_text(format!("{} selected · follows cursor", count))
                                    .show_ui(ui, |ui| {
                                        ui.set_min_width(260.0);
                                        ui.label("Follow the cursor on selected displays");
                                        ui.separator();
                                        egui::ScrollArea::vertical().max_height(220.0).show(
                                            ui,
                                            |ui| {
                                                for (index, name) in names.iter().enumerate() {
                                                    let mut checked =
                                                        selection.contains(&(index as u8));
                                                    if ui.checkbox(&mut checked, name).changed() {
                                                        if checked {
                                                            selection.push(index as u8);
                                                        } else if selection.len() > 1 {
                                                            selection.retain(|i| *i != index as u8);
                                                        } else {
                                                            continue;
                                                        }
                                                        selection_changed = true;
                                                    }
                                                }
                                            },
                                        );
                                    })
                                    .response
                            });
                            if selection_changed {
                                selection.sort_unstable();
                                selection.dedup();
                                cfg.monitor_displays = selection;
                                out.changed = true;
                            }
                        }
                    } else {
                        hint(&mut rui, "No connected displays are available", theme);
                    }
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
                let mut sounds = cfg.sound_effects;
                if row(&mut rui, "Sound effects", theme, |ui| {
                    widgets::toggle(ui, &mut sounds, theme)
                }) {
                    cfg.sound_effects = sounds;
                    out.changed = true;
                }
                let mut hide = cfg.hide_in_fullscreen;
                if row(&mut rui, "Hide in fullscreen", theme, |ui| {
                    widgets::toggle(ui, &mut hide, theme)
                        .on_hover_text("Fade the notch out while a game or video is fullscreen")
                }) {
                    cfg.hide_in_fullscreen = hide;
                    out.changed = true;
                }
            }

            let open = open_section(&ctx);
            let mut y = top.max.y + GAP;
            for section in [CUSTOMIZE, MOTION, ADVANCED] {
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
                    MOTION => (
                        icons::ARROW_CLOCKWISE,
                        "Motion",
                        format!(
                            "{} ms · {} ms open delay",
                            cfg.motion_duration_ms, cfg.hover_delay_ms
                        ),
                    ),
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
                        MOTION => motion_body(&mut bui, body, theme, cfg, &mut out),
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
        Material::Oled => "OLED black".to_owned(),
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
        Material::Oled => 2,
    };
    if row(&mut lui, "Material", theme, |ui| {
        widgets::segmented(ui, &["Glass", "Solid", "OLED"], &mut mat_idx, theme)
    }) {
        cfg.material = [Material::Acrylic, Material::Solid, Material::Oled][mat_idx];
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
    let mut background = parse_hex(&cfg.background_hex).unwrap_or(theme.glass_top);
    if row(&mut lui, "Custom background", theme, |ui| {
        egui::color_picker::color_edit_button_srgba(
            ui,
            &mut background,
            egui::color_picker::Alpha::Opaque,
        )
    }) {
        cfg.background_hex = to_hex(background);
        out.changed = true;
    }
    for (label, value) in [
        ("Sections", &mut cfg.surface_hex),
        ("Buttons", &mut cfg.control_hex),
    ] {
        let mut color = parse_hex(value).unwrap_or(theme.tile_top);
        if row(&mut lui, label, theme, |ui| {
            egui::color_picker::color_edit_button_srgba(
                ui,
                &mut color,
                egui::color_picker::Alpha::Opaque,
            )
        }) {
            *value = to_hex(color);
            out.changed = true;
        }
    }
    let glass = cfg.material == Material::Acrylic;
    // With Windows "Transparency effects" off there is no blur to see
    // through, so the glass is drawn opaque and opacity has no effect. Only
    // read while this card is open.
    let transparency = nook_platform::system::transparency_enabled();
    let span = (OPACITY_MAX - OPACITY_MIN) as f32;
    let mut t = (cfg.glass_opacity.clamp(OPACITY_MIN, OPACITY_MAX) - OPACITY_MIN) as f32 / span;
    if row(&mut lui, "Glass opacity", theme, |ui| {
        if !glass || !transparency {
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
    let mut margin = cfg.pill_margin as f32 / 24.0;
    if row(&mut lui, "Pill top margin", theme, |ui| {
        if cfg.shell_style == ShellStyle::Notch {
            ui.disable();
        }
        let resp = widgets::slider(ui, &mut margin, 110.0, 12, theme);
        value_label(ui, &format!("{}", cfg.pill_margin), theme);
        resp
    }) {
        cfg.pill_margin = (margin * 24.0).round() as u8;
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
    let mut border = cfg.shell_border.min(100) as f32 / 100.0;
    if row(&mut rui, "Shell border", theme, |ui| {
        let resp = widgets::slider(ui, &mut border, 110.0, 18, theme);
        value_label(ui, &format!("{}%", cfg.shell_border), theme);
        resp
    }) {
        cfg.shell_border = (border * 100.0).round() as u8;
        out.changed = true;
    }
    let mut shadow = cfg.shell_shadow.min(100) as f32 / 100.0;
    if row(&mut rui, "Shell shadow", theme, |ui| {
        let resp = widgets::slider(ui, &mut shadow, 110.0, 19, theme);
        let label = if cfg.shell_shadow == 0 {
            "Off".to_owned()
        } else {
            format!("{}%", cfg.shell_shadow)
        };
        value_label(ui, &label, theme);
        resp
    }) {
        cfg.shell_shadow = (shadow * 100.0).round() as u8;
        out.changed = true;
    }
    let mut media_closed = cfg.media_in_closed_notch;
    if row(&mut rui, "Music in closed notch", theme, |ui| {
        widgets::toggle(ui, &mut media_closed, theme)
    }) {
        cfg.media_in_closed_notch = media_closed;
        out.changed = true;
    }
    let mut component_radius = cfg.component_radius.min(24) as f32 / 24.0;
    if row(&mut rui, "Button corner", theme, |ui| {
        let resp = widgets::slider(ui, &mut component_radius, 110.0, 24, theme);
        value_label(ui, &format!("{}", cfg.component_radius), theme);
        resp
    }) {
        cfg.component_radius = (component_radius * 24.0).round() as u8;
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

    if !transparency {
        let (note, _) = rui.allocate_exact_size(vec2(w, ROW), Sense::hover());
        ui.painter().text(
            pos2(note.min.x, note.center().y),
            Align2::LEFT_CENTER,
            "Windows transparency is off",
            Theme::font(SIZE_LABEL),
            theme.text_dim,
        );
        let turn_on = Rect::from_min_size(
            pos2(note.max.x - 96.0, note.center().y - 13.0),
            vec2(96.0, 26.0),
        );
        if nook_modules::kit::button(ui, turn_on, "Turn on", Some(icons::ARROW_SQUARE_OUT), false, theme)
            .on_hover_text(
                "Glass and opacity need Transparency effects in Windows Settings > Personalization > Colors",
            )
            .clicked()
            && let Err(e) = nook_platform::shell::open("ms-settings:colors")
        {
            tracing::warn!("opening color settings: {e}");
        }
    }
}

/// Shell hover timing and spring feel, grouped below Customize.
fn motion_body(ui: &mut Ui, rect: Rect, theme: &Theme, cfg: &mut Config, out: &mut PageOutput) {
    let inner = rect.shrink2(vec2(14.0, 6.0));
    let gap = 20.0;
    let col_w = (inner.width() - gap) * 0.5;
    let mut left = ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_size(inner.min, vec2(col_w, inner.height())))
            .layout(Layout::top_down(Align::Min)),
    );
    let mut right = ui.new_child(
        UiBuilder::new()
            .max_rect(Rect::from_min_size(
                pos2(inner.min.x + col_w + gap, inner.min.y),
                vec2(col_w, inner.height()),
            ))
            .layout(Layout::top_down(Align::Min)),
    );

    let mut open_delay = cfg.hover_delay_ms.min(600) as f32 / 600.0;
    if row(&mut left, "Open delay", theme, |ui| {
        let resp = widgets::slider(ui, &mut open_delay, 105.0, 30, theme);
        value_label(ui, &format!("{} ms", cfg.hover_delay_ms), theme);
        resp
    }) {
        cfg.hover_delay_ms = (open_delay * 600.0).round() as u32;
        out.changed = true;
    }
    let mut close_delay = cfg.collapse_grace_ms.min(800) as f32 / 800.0;
    if row(&mut left, "Close delay", theme, |ui| {
        let resp = widgets::slider(ui, &mut close_delay, 105.0, 31, theme);
        value_label(ui, &format!("{} ms", cfg.collapse_grace_ms), theme);
        resp
    }) {
        cfg.collapse_grace_ms = (close_delay * 800.0).round() as u32;
        out.changed = true;
    }
    let mut glow = cfg.hover_glow.min(100) as f32 / 100.0;
    if row(&mut left, "Hover glow", theme, |ui| {
        let resp = widgets::slider(ui, &mut glow, 105.0, 32, theme);
        let label = if cfg.hover_glow == 0 {
            "Off".to_owned()
        } else {
            format!("{}%", cfg.hover_glow)
        };
        value_label(ui, &label, theme);
        resp
    }) {
        cfg.hover_glow = (glow * 100.0).round() as u8;
        out.changed = true;
    }

    let mut duration = cfg.motion_duration_ms.saturating_sub(150).min(650) as f32 / 650.0;
    if row(&mut right, "Duration", theme, |ui| {
        let resp = widgets::slider(ui, &mut duration, 105.0, 33, theme);
        value_label(ui, &format!("{} ms", cfg.motion_duration_ms), theme);
        resp
    }) {
        cfg.motion_duration_ms = 150 + (duration * 650.0).round() as u16;
        out.changed = true;
    }
    let mut easing = match cfg.motion_easing {
        nook_ui::shell_state::MotionEasing::Smooth => 0,
        nook_ui::shell_state::MotionEasing::Spring => 1,
        nook_ui::shell_state::MotionEasing::Bouncy => 2,
    };
    if row(&mut right, "Easing", theme, |ui| {
        widgets::segmented(ui, &["Smooth", "Spring", "Bouncy"], &mut easing, theme)
    }) {
        cfg.motion_easing = [
            nook_ui::shell_state::MotionEasing::Smooth,
            nook_ui::shell_state::MotionEasing::Spring,
            nook_ui::shell_state::MotionEasing::Bouncy,
        ][easing];
        out.changed = true;
    }
    let width = right.available_width();
    let (reset_row, _) = right.allocate_exact_size(vec2(width, ROW), Sense::hover());
    let reset = Rect::from_min_size(
        pos2(reset_row.max.x - 124.0, reset_row.center().y - 13.0),
        vec2(124.0, 26.0),
    );
    if nook_modules::kit::button(
        ui,
        reset,
        "Reset motion",
        Some(icons::ARROW_COUNTER_CLOCKWISE),
        false,
        theme,
    )
    .clicked()
    {
        let defaults = Config::default();
        cfg.hover_delay_ms = defaults.hover_delay_ms;
        cfg.collapse_grace_ms = defaults.collapse_grace_ms;
        cfg.hover_glow = defaults.hover_glow;
        cfg.motion_duration_ms = defaults.motion_duration_ms;
        cfg.motion_easing = defaults.motion_easing;
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
        "Daily: version, opens, minutes, tools used",
        theme,
    );
    if retention_row(
        &mut rui,
        "Notes keep for",
        &mut cfg.notes_retention,
        "notes-retention",
        theme,
    ) {
        out.changed = true;
    }

    let mut renderer_idx = match cfg.renderer {
        Renderer::Auto => 0,
        Renderer::DirectX => 1,
        Renderer::OpenGl => 2,
    };
    if row(&mut lui, "Renderer", theme, |ui| {
        widgets::segmented(ui, &["Auto", "DirectX", "OpenGL"], &mut renderer_idx, theme)
            .on_hover_text("Try OpenGL if the panel flickers; DirectX if it shows a black box")
    }) {
        cfg.renderer = [Renderer::Auto, Renderer::DirectX, Renderer::OpenGl][renderer_idx];
        out.changed = true;
    }
    hint(
        &mut lui,
        &format!(
            "Using {} · applies after restart",
            crate::diag::active_renderer()
        ),
        theme,
    );
    if retention_row(
        &mut lui,
        "Clipboard keep for",
        &mut cfg.clipboard_retention,
        "clipboard-retention",
        theme,
    ) {
        out.changed = true;
    }

    // "Open log" at the right, level with the Renderer row.
    rui.add_space(ROW - 26.0);
    let w = rui.available_width();
    let (log_row, _) = rui.allocate_exact_size(vec2(w, ROW), Sense::hover());
    let open_log = Rect::from_min_size(
        pos2(log_row.max.x - 120.0, log_row.center().y - 13.0),
        vec2(120.0, 26.0),
    );
    if nook_modules::kit::button(
        ui,
        open_log,
        "Open log",
        Some(icons::FILE_TEXT),
        false,
        theme,
    )
    .on_hover_text("Shows nook.log in Explorer, to attach to a bug report")
    .clicked()
        && let Some(path) = crate::diag::log_path()
        && let Err(e) = nook_platform::shell::reveal(&path)
    {
        tracing::warn!("revealing log: {e}");
    }
}

fn retention_row(
    ui: &mut Ui,
    label: &str,
    value: &mut RetentionPeriod,
    id: &'static str,
    theme: &Theme,
) -> bool {
    row(ui, label, theme, |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(value.label())
            .width(100.0)
            .show_ui(ui, |ui| {
                for period in RetentionPeriod::ALL {
                    ui.selectable_value(value, period, period.label());
                }
            })
            .response
    })
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

/// Label on the left, control on the right. Returns whether it changed.
fn row(
    ui: &mut Ui,
    label: &str,
    theme: &Theme,
    control: impl FnOnce(&mut Ui) -> egui::Response,
) -> bool {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(w, ROW), Sense::hover());
    let label_width = (w * 0.38).clamp(68.0, 116.0).min((w - 72.0).max(0.0));
    let label_rect = Rect::from_min_size(rect.min, vec2(label_width, ROW));
    ui.put(
        label_rect,
        egui::Label::new(
            egui::RichText::new(label)
                .font(Theme::font(SIZE_BODY))
                .color(theme.text),
        )
        .truncate(),
    );
    let control_rect = Rect::from_min_max(pos2(rect.min.x + label_width, rect.min.y), rect.max);
    let mut cui = ui.new_child(
        UiBuilder::new()
            .max_rect(control_rect)
            .layout(Layout::right_to_left(Align::Center)),
    );
    let response = control(&mut cui);
    response.changed() || response.clicked()
}
