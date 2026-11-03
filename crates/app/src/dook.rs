//! Dook: a small native dock viewport with live system indicators.

use crate::config::{DookConfig, DookPosition};
use crossbeam_channel::{Receiver, Sender, unbounded};
use egui::{
    Align, Align2, Color32, Context, CornerRadius, Id, Layout, Rect, Sense, Stroke, TextureHandle,
    TextureOptions, Ui, UiBuilder, ViewportBuilder, ViewportClass, ViewportId, WindowLevel, pos2,
    vec2,
};
use nook_platform::monitor::Monitor;
use nook_platform::system::{self, Battery};
use nook_ui::theme::{Theme, parse_hex};
use nook_ui::{icons, paint};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::Duration;

const TITLE: &str = "Dook - Nook";
const HEIGHT: f32 = 64.0;
const EDGE: f32 = 8.0;

type IconResult = (String, Option<(u32, u32, Vec<u8>)>);

/// Synchronizes the user's replacement-taskbar choice and restores Explorer
/// when Dook is disabled or Nook exits.
pub struct Runtime {
    taskbar_requested: Option<bool>,
    taskbars: Option<nook_platform::taskbar::VisibilitySnapshot>,
    icons: HashMap<String, TextureHandle>,
    requested_icons: HashSet<String>,
    icon_tx: Sender<IconResult>,
    icon_rx: Receiver<IconResult>,
}

impl Default for Runtime {
    fn default() -> Self {
        let (icon_tx, icon_rx) = unbounded();
        Self {
            taskbar_requested: None,
            taskbars: None,
            icons: HashMap::new(),
            requested_icons: HashSet::new(),
            icon_tx,
            icon_rx,
        }
    }
}

impl Runtime {
    pub fn sync(&mut self, cfg: &DookConfig) {
        let wanted = cfg.enabled && cfg.hide_windows_taskbar;
        if self.taskbar_requested == Some(wanted) {
            return;
        }
        if wanted {
            match nook_platform::taskbar::hide() {
                Ok(snapshot) => {
                    self.taskbars = Some(snapshot);
                    self.taskbar_requested = Some(true);
                }
                Err(error) => {
                    // Retry on a later frame if Explorer was restarting.
                    tracing::warn!("Windows taskbar visibility: {error}");
                    self.taskbar_requested = None;
                }
            }
        } else {
            self.restore();
            self.taskbar_requested = Some(false);
        }
    }

    pub fn restore(&mut self) {
        if let Some(snapshot) = self.taskbars.take() {
            nook_platform::taskbar::restore(snapshot);
        }
    }

    fn load_app_icons(&mut self, ctx: &Context, paths: impl Iterator<Item = String>) {
        while let Ok((path, image)) = self.icon_rx.try_recv() {
            if let Some((width, height, rgba)) = image {
                let image = egui::ColorImage::from_rgba_premultiplied(
                    [width as usize, height as usize],
                    &rgba,
                );
                self.icons.insert(
                    path.clone(),
                    ctx.load_texture(format!("dook-app-{path}"), image, TextureOptions::LINEAR),
                );
            }
        }
        for path in paths {
            if self.icons.contains_key(&path) || !self.requested_icons.insert(path.clone()) {
                continue;
            }
            let tx = self.icon_tx.clone();
            let worker_path = path.clone();
            let repaint = ctx.clone();
            let spawned = std::thread::Builder::new()
                .name("nook-dook-icon".into())
                .spawn(move || {
                    nook_platform::files::init_thumbnail_thread();
                    let image = match nook_platform::files::thumbnail(Path::new(&worker_path), 64) {
                        Ok(image) => Some(image),
                        Err(error) => {
                            tracing::debug!("Dook app icon {worker_path}: {error}");
                            None
                        }
                    };
                    if tx.send((worker_path, image)).is_ok() {
                        repaint.request_repaint();
                    }
                });
            if let Err(error) = spawned {
                self.requested_icons.remove(&path);
                tracing::warn!("starting Dook icon loader: {error}");
            }
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.restore();
    }
}

pub fn show(
    runtime: &mut Runtime,
    ctx: &Context,
    cfg: &DookConfig,
    theme: &Theme,
    monitor: Option<Monitor>,
    battery: Option<Battery>,
) {
    if !cfg.enabled {
        return;
    }
    let Some(monitor) = monitor else {
        return;
    };
    let scale = monitor.dpi as f32 / 96.0;
    let bounds = if cfg.hide_windows_taskbar {
        monitor.bounds
    } else {
        monitor.work
    };
    runtime.load_app_icons(ctx, cfg.apps.iter().map(|app| app.path.clone()));
    let status_count = [
        cfg.show_wifi,
        cfg.show_volume,
        cfg.show_battery,
        cfg.show_emoji,
    ]
    .into_iter()
    .filter(|show| *show)
    .count() as f32;
    let desired_width = 44.0
        + if cfg.show_logo { 40.0 } else { 0.0 }
        + if cfg.show_clock || cfg.show_date {
            112.0
        } else {
            0.0
        }
        + cfg.apps.len().min(6) as f32 * 40.0
        + (status_count + 1.0) * 36.0;
    let max_width = (bounds.width() as f32 / scale - EDGE * 2.0).max(1.0);
    let width = desired_width.min(720.0).min(max_width).max(1.0);
    let x = match cfg.position {
        DookPosition::Left => bounds.left as f32 / scale + EDGE,
        DookPosition::Center => {
            (bounds.left as f32 + (bounds.width() as f32 - width * scale) * 0.5) / scale
        }
        DookPosition::Right => (bounds.right as f32 - width * scale) / scale - EDGE,
    };
    let y = (bounds.bottom as f32 - HEIGHT * scale) / scale - EDGE;
    let builder = ViewportBuilder::default()
        .with_title(TITLE)
        .with_position(pos2(x, y))
        .with_inner_size(vec2(width, HEIGHT))
        .with_min_inner_size(vec2(width, HEIGHT))
        .with_max_inner_size(vec2(width, HEIGHT))
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_has_shadow(false)
        .with_taskbar(false)
        .with_window_level(WindowLevel::AlwaysOnTop);
    let icons = &runtime.icons;
    ctx.show_viewport_immediate(ViewportId::from_hash_of(TITLE), builder, |ctx, class| {
        if class == ViewportClass::EmbeddedWindow {
            return;
        }
        ui(ctx, cfg, theme, battery, icons);
    });
}

fn ui(
    ctx: &Context,
    cfg: &DookConfig,
    theme: &Theme,
    battery: Option<Battery>,
    app_icons: &HashMap<String, TextureHandle>,
) {
    let background = if cfg.oled {
        Color32::BLACK
    } else {
        parse_hex(&cfg.background_hex).unwrap_or(Color32::from_rgb(24, 24, 28))
    };
    let icon_color = if cfg.monochrome_icons {
        parse_hex(&cfg.icon_hex).unwrap_or(theme.text)
    } else {
        theme.accent
    };
    let clock = system::clock();
    ctx.request_repaint_after(Duration::from_secs(u64::from(
        clock.secs_to_next_minute.max(1),
    )));

    let viewport = ctx.content_rect();
    egui::Area::new(Id::new("dook-root"))
        .order(egui::Order::Background)
        .fixed_pos(viewport.min)
        .show(ctx, |ui| {
            ui.set_min_size(viewport.size());
            let full = ui.max_rect();
            let dock = full.shrink(3.0);
            let dock_radius = cfg.radius.min(36);
            let button_radius = cfg.button_radius.min(18);
            let stroke = if theme.dark {
                Color32::from_white_alpha(28)
            } else {
                Color32::from_black_alpha(24)
            };
            ui.painter()
                .rect_filled(dock, CornerRadius::same(dock_radius), background);
            ui.painter().rect_stroke(
                dock,
                CornerRadius::same(dock_radius),
                Stroke::new(1.0, stroke),
                egui::StrokeKind::Inside,
            );
            let row = dock.shrink2(vec2(12.0, 4.0));
            let mut row_ui = ui.new_child(
                UiBuilder::new()
                    .max_rect(row)
                    .layout(Layout::left_to_right(Align::Center)),
            );

            if cfg.show_logo {
                let (logo, _) = row_ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
                paint::icon(
                    row_ui.painter(),
                    logo.center(),
                    icons::SQUARES_FOUR,
                    17.0,
                    icon_color,
                    false,
                );
                row_ui.add_space(4.0);
            }

            for (index, app) in cfg.apps.iter().take(6).enumerate() {
                let texture = app_icons.get(&app.path);
                let icon_tint = if cfg.monochrome_icons {
                    icon_color
                } else {
                    Color32::WHITE
                };
                if app_button(
                    &mut row_ui,
                    index,
                    &app.label,
                    texture,
                    icon_tint,
                    button_radius,
                )
                .clicked()
                    && let Err(error) = nook_platform::shell::open(&app.path)
                {
                    tracing::warn!("launching Dook app {}: {error}", app.label);
                }
            }

            if cfg.show_clock || cfg.show_date {
                let (text_rect, _) = row_ui.allocate_exact_size(vec2(112.0, 44.0), Sense::hover());
                let painter = row_ui.painter();
                if cfg.show_clock {
                    painter.text(
                        pos2(
                            text_rect.min.x,
                            text_rect.center().y - if cfg.show_date { 7.0 } else { 0.0 },
                        ),
                        Align2::LEFT_CENTER,
                        &clock.time,
                        egui::FontId::proportional(14.0),
                        theme.text,
                    );
                }
                if cfg.show_date {
                    painter.text(
                        pos2(
                            text_rect.min.x,
                            text_rect.center().y + if cfg.show_clock { 9.0 } else { 0.0 },
                        ),
                        Align2::LEFT_CENTER,
                        &clock.date,
                        egui::FontId::proportional(10.0),
                        theme.text_dim,
                    );
                }
            }

            row_ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let overflow_id = Id::new("dook-overflow-open");
                let mut overflow = ui
                    .ctx()
                    .data(|data| data.get_temp::<bool>(overflow_id))
                    .unwrap_or(false);
                let overflow_button = button(
                    ui,
                    "overflow",
                    if overflow {
                        icons::CARET_DOWN
                    } else {
                        icons::CARET_UP
                    },
                    "Show more indicators",
                    icon_color,
                    button_radius,
                );
                if overflow_button.clicked() {
                    overflow = !overflow;
                    ui.ctx()
                        .data_mut(|data| data.insert_temp(overflow_id, overflow));
                }
                if cfg.show_emoji
                    && overflow
                    && icon_button(
                        ui,
                        "emoji",
                        icons::SMILEY,
                        "Open Windows emoji picker",
                        icon_color,
                        button_radius,
                    )
                    .clicked()
                {
                    nook_platform::shell::show_emoji_picker();
                }
                if cfg.show_battery
                    && let Some(battery) = battery
                {
                    let glyph = if battery.charging {
                        icons::BATTERY_CHARGING
                    } else {
                        icons::BATTERY_FULL
                    };
                    let label = format!("{}%", battery.percent);
                    if icon_button(ui, "battery", glyph, &label, icon_color, button_radius)
                        .clicked()
                    {
                        let _ = nook_platform::shell::open("ms-settings:powersleep");
                    }
                    ui.label(egui::RichText::new(label).size(10.0).color(theme.text_dim));
                }
                if cfg.show_volume
                    && icon_button(
                        ui,
                        "volume",
                        icons::SPEAKER_HIGH,
                        "Open sound settings",
                        icon_color,
                        button_radius,
                    )
                    .clicked()
                    && let Err(error) = nook_platform::shell::open("ms-settings:sound")
                {
                    tracing::warn!("opening sound settings from Dook: {error}");
                }
                if cfg.show_wifi
                    && icon_button(
                        ui,
                        "wifi",
                        icons::WIFI_HIGH,
                        "Open Wi-Fi settings",
                        icon_color,
                        button_radius,
                    )
                    .clicked()
                    && let Err(error) = nook_platform::shell::open("ms-settings:network-wifi")
                {
                    tracing::warn!("opening Wi-Fi settings from Dook: {error}");
                }
            });
        });
}

fn icon_button(
    ui: &mut Ui,
    id: &'static str,
    glyph: &str,
    tip: &str,
    color: Color32,
    radius: u8,
) -> egui::Response {
    button(ui, id, glyph, tip, color, radius)
}

fn app_button(
    ui: &mut Ui,
    index: usize,
    label: &str,
    texture: Option<&TextureHandle>,
    tint: Color32,
    radius: u8,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
    let response = ui.interact(rect, ui.id().with(("dook-app", index)), Sense::click());
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(radius.min(18)),
            Color32::from_white_alpha(if response.is_pointer_button_down_on() {
                36
            } else {
                20
            }),
        );
    }
    if let Some(texture) = texture {
        ui.painter().image(
            texture.id(),
            rect.shrink(7.0),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            tint,
        );
    } else {
        let initial = label
            .chars()
            .next()
            .unwrap_or('A')
            .to_uppercase()
            .to_string();
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            initial,
            egui::FontId::proportional(15.0),
            tint,
        );
    }
    response.on_hover_text(label)
}

fn button(
    ui: &mut Ui,
    id: &'static str,
    glyph: &str,
    tip: &str,
    color: Color32,
    radius: u8,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
    let response = ui.interact(rect, ui.id().with(id), Sense::click());
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(radius.min(18)),
            Color32::from_white_alpha(if response.is_pointer_button_down_on() {
                36
            } else {
                20
            }),
        );
    }
    paint::icon(ui.painter(), rect.center(), glyph, 17.0, color, false);
    response.on_hover_text(tip)
}
