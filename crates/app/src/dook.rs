//! Dook: a configurable bottom taskbar with live Explorer pins and system actions.

use crate::config::{DookConfig, DookPosition, DookStyle, DookTarget};
use crossbeam_channel::{Receiver, Sender, unbounded};
use egui::{
    Align, Align2, Color32, Context, CornerRadius, Id, Layout, Rect, Sense, Stroke, TextureHandle,
    TextureOptions, Ui, UiBuilder, ViewportBuilder, ViewportClass, ViewportId, WindowLevel, pos2,
    vec2,
};
use nook_platform::monitor::Monitor;
use nook_platform::system::{self, Battery};
use nook_ui::theme::{Theme, parse_hex};
use nook_ui::{anim, icons, paint};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const TITLE: &str = "Dook - Nook";
const HEIGHT: f32 = 84.0;
const EDGE: f32 = 10.0;
const PIN_SIZE: f32 = 40.0;
const PIN_GAP: f32 = 4.0;
const STATUS_SIZE: f32 = 32.0;
const PIN_REFRESH: Duration = Duration::from_secs(8);

type IconResult = (String, Option<(u32, u32, Vec<u8>)>);

struct TrayLook<'a> {
    theme: &'a Theme,
    battery: Option<Battery>,
    clock: &'a system::Clock,
    icon_color: Color32,
    button_radius: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PinnedApp {
    path: String,
    label: String,
}

/// Actions that must be routed through Nook's primary app state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Start,
    Search,
    QuickSettings,
}

/// Owns Explorer visibility and cached shell icons for the current pin list.
pub struct Runtime {
    taskbar_requested: Option<bool>,
    taskbar_monitor: Option<isize>,
    next_taskbar_retry: Instant,
    taskbars: Option<nook_platform::taskbar::VisibilitySnapshot>,
    pins: Vec<PinnedApp>,
    next_pin_refresh: Instant,
    icons: HashMap<String, TextureHandle>,
    requested_icons: HashSet<String>,
    icon_tx: Sender<IconResult>,
    icon_rx: Receiver<IconResult>,
    auto_hidden: bool,
    left_since: Option<Instant>,
}

impl Default for Runtime {
    fn default() -> Self {
        let (icon_tx, icon_rx) = unbounded();
        Self {
            taskbar_requested: None,
            taskbar_monitor: None,
            next_taskbar_retry: Instant::now(),
            taskbars: None,
            pins: Vec::new(),
            next_pin_refresh: Instant::now(),
            icons: HashMap::new(),
            requested_icons: HashSet::new(),
            icon_tx,
            icon_rx,
            auto_hidden: false,
            left_since: None,
        }
    }
}

impl Runtime {
    pub fn sync(&mut self, cfg: &DookConfig, monitor: Option<Monitor>) {
        let target = monitor.filter(|_| cfg.enabled && cfg.hide_windows_taskbar);
        if let Some(target) = target {
            if self.taskbar_requested == Some(true) && self.taskbar_monitor == Some(target.id) {
                return;
            }
            if self.taskbar_requested.is_none() && Instant::now() < self.next_taskbar_retry {
                return;
            }
            self.restore_taskbar();
            match nook_platform::taskbar::hide(target) {
                Ok(snapshot) => {
                    self.taskbars = Some(snapshot);
                    self.taskbar_requested = Some(true);
                    self.taskbar_monitor = Some(target.id);
                }
                Err(error) => {
                    tracing::warn!("Windows taskbar visibility: {error}");
                    self.taskbar_requested = None;
                    self.taskbar_monitor = None;
                    self.next_taskbar_retry = Instant::now() + Duration::from_secs(2);
                }
            }
        } else {
            if self.taskbar_requested == Some(false) {
                return;
            }
            self.restore_taskbar();
            self.taskbar_requested = Some(false);
            self.taskbar_monitor = None;
        }
    }

    fn restore_taskbar(&mut self) {
        if let Some(snapshot) = self.taskbars.take() {
            nook_platform::taskbar::restore(snapshot);
        }
    }

    fn refresh_pins(&mut self, ctx: &Context) {
        if Instant::now() < self.next_pin_refresh {
            return;
        }
        self.next_pin_refresh = Instant::now() + PIN_REFRESH;
        match nook_platform::taskbar::pinned_shortcuts() {
            Ok(paths) => {
                let pins = paths.into_iter().map(pinned_app).collect::<Vec<_>>();
                if pins != self.pins {
                    self.pins = pins;
                    let retained = self
                        .pins
                        .iter()
                        .map(|pin| pin.path.as_str())
                        .collect::<HashSet<_>>();
                    self.icons
                        .retain(|path, _| retained.contains(path.as_str()));
                    self.requested_icons
                        .retain(|path| retained.contains(path.as_str()));
                }
            }
            Err(error) => tracing::debug!("reading Windows taskbar pins: {error}"),
        }
        self.load_pin_icons(ctx);
    }

    fn load_pin_icons(&mut self, ctx: &Context) {
        while let Ok((path, image)) = self.icon_rx.try_recv() {
            if let Some((width, height, rgba)) = image {
                let image = egui::ColorImage::from_rgba_premultiplied(
                    [width as usize, height as usize],
                    &rgba,
                );
                self.icons.insert(
                    path.clone(),
                    ctx.load_texture(format!("dook-pin-{path}"), image, TextureOptions::LINEAR),
                );
            }
        }
        for pin in &self.pins {
            if self.icons.contains_key(&pin.path) || self.requested_icons.contains(&pin.path) {
                continue;
            }
            self.requested_icons.insert(pin.path.clone());
            let tx = self.icon_tx.clone();
            let worker_path = pin.path.clone();
            let repaint = ctx.clone();
            let spawned = std::thread::Builder::new()
                .name("nook-dook-icon".into())
                .spawn(move || {
                    nook_platform::files::init_thumbnail_thread();
                    let image =
                        match nook_platform::files::thumbnail(&PathBuf::from(&worker_path), 64) {
                            Ok(image) => Some(image),
                            Err(error) => {
                                tracing::debug!("Dook pinned app icon {worker_path}: {error}");
                                None
                            }
                        };
                    if tx.send((worker_path, image)).is_ok() {
                        repaint.request_repaint();
                    }
                });
            if let Err(error) = spawned {
                self.requested_icons.remove(&pin.path);
                tracing::warn!("starting Dook icon loader: {error}");
            }
        }
    }

    fn sync_auto_hide(&mut self, ctx: &Context, cfg: &DookConfig, monitor: Monitor) -> f32 {
        if !cfg.auto_hide {
            self.auto_hidden = false;
            self.left_since = None;
            return 1.0;
        }
        let scale = monitor.dpi as f32 / 96.0;
        let area = if cfg.hide_windows_taskbar {
            monitor.bounds
        } else {
            monitor.work
        };
        let sensor_height = if self.auto_hidden {
            (7.0 * scale) as i32
        } else {
            (HEIGHT * scale) as i32
        };
        let pointer_inside = nook_platform::window::cursor_position().is_some_and(|(x, y)| {
            x >= area.left && x < area.right && y >= area.bottom - sensor_height && y < area.bottom
        });
        if pointer_inside {
            self.auto_hidden = false;
            self.left_since = None;
        } else if !self.auto_hidden {
            let since = *self.left_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_millis(550) {
                self.auto_hidden = true;
                self.left_since = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }
        let t = anim::tween(ctx, Id::new("dook-auto-hide"), !self.auto_hidden, 0.22);
        t.clamp(0.0, 1.0)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        self.restore_taskbar();
    }
}

fn pinned_app(path: PathBuf) -> PinnedApp {
    let label = path
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("App")
        .to_owned();
    PinnedApp {
        path: path.to_string_lossy().into_owned(),
        label,
    }
}

pub fn show(
    runtime: &mut Runtime,
    ctx: &Context,
    cfg: &DookConfig,
    theme: &Theme,
    monitor: Option<Monitor>,
    battery: Option<Battery>,
) -> Option<Action> {
    if !cfg.enabled {
        runtime.auto_hidden = false;
        return None;
    }
    let monitor = monitor?;
    runtime.refresh_pins(ctx);

    let scale = monitor.dpi as f32 / 96.0;
    let area = if cfg.hide_windows_taskbar {
        monitor.bounds
    } else {
        monitor.work
    };
    let width = area.width() as f32 / scale;
    let height = HEIGHT;
    let visible = runtime.sync_auto_hide(ctx, cfg, monitor);
    let window_height = 7.0 + (height - 7.0) * visible;
    let position = pos2(
        area.left as f32 / scale,
        (area.bottom as f32 / scale) - window_height,
    );
    let builder = ViewportBuilder::default()
        .with_title(TITLE)
        .with_position(position)
        .with_inner_size(vec2(width, window_height))
        .with_min_inner_size(vec2(width, window_height))
        .with_max_inner_size(vec2(width, window_height))
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_has_shadow(false)
        .with_taskbar(false)
        .with_window_level(WindowLevel::AlwaysOnTop);
    runtime.load_pin_icons(ctx);
    let icons = &runtime.icons;
    let pins = &runtime.pins;
    ctx.show_viewport_immediate(ViewportId::from_hash_of(TITLE), builder, |ctx, class| {
        if class == ViewportClass::EmbeddedWindow {
            return None;
        }
        ui(ctx, cfg, theme, battery, pins, icons)
    })
}

fn ui(
    ctx: &Context,
    cfg: &DookConfig,
    theme: &Theme,
    battery: Option<Battery>,
    pins: &[PinnedApp],
    app_icons: &HashMap<String, TextureHandle>,
) -> Option<Action> {
    let background = if cfg.oled {
        Color32::BLACK
    } else {
        parse_hex(&cfg.background_hex).unwrap_or(Color32::from_rgb(16, 16, 19))
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
    let radius = cfg.radius.min(40);
    let button_radius = cfg.button_radius.min(20);
    let stroke = if theme.dark {
        Color32::from_white_alpha(28)
    } else {
        Color32::from_black_alpha(26)
    };
    let mut action = None;
    egui::Area::new(Id::new("dook-root"))
        .order(egui::Order::Background)
        .fixed_pos(viewport.min)
        .show(ctx, |ui| {
            ui.set_min_size(viewport.size());
            let full = ui.max_rect();
            let (apps_rect, tray_rect) = match cfg.style {
                DookStyle::Pill => pill_rects(full, cfg),
                DookStyle::Notch => notch_rects(full, cfg.apps_position),
            };
            if cfg.style == DookStyle::Notch {
                paint_surface(
                    ui,
                    full,
                    apps_rect,
                    DookStyle::Notch,
                    radius,
                    background,
                    stroke,
                );
            } else {
                paint_surface(
                    ui,
                    full,
                    apps_rect,
                    DookStyle::Pill,
                    radius,
                    background,
                    stroke,
                );
                if let Some(tray_rect) = tray_rect {
                    paint_surface(
                        ui,
                        full,
                        tray_rect,
                        DookStyle::Pill,
                        radius,
                        background,
                        stroke,
                    );
                }
            }
            if cfg.auto_hide && full.height() < HEIGHT - 0.5 {
                ui.painter()
                    .rect_filled(full, CornerRadius::same(3), background);
            }
            action = draw_apps(
                ui,
                apps_rect,
                cfg,
                pins,
                app_icons,
                icon_color,
                button_radius,
            );
            let tray_action = draw_tray(
                ui,
                tray_rect.unwrap_or(apps_rect),
                cfg,
                TrayLook {
                    theme,
                    battery,
                    clock: &clock,
                    icon_color,
                    button_radius,
                },
            );
            if action.is_none() {
                action = tray_action;
            }
        });
    action
}

fn pill_rects(full: Rect, cfg: &DookConfig) -> (Rect, Option<Rect>) {
    let position = cfg.apps_position;
    let controls = [
        cfg.show_quick_settings,
        cfg.show_emoji,
        cfg.show_battery,
        cfg.show_volume,
        cfg.show_wifi,
    ]
    .into_iter()
    .filter(|show| *show)
    .count() as f32;
    let clock_width = if cfg.show_clock || cfg.show_date {
        86.0
    } else {
        0.0
    };
    let battery_width = if cfg.show_battery { 28.0 } else { 0.0 };
    let tray_width = (32.0 + controls * (STATUS_SIZE + PIN_GAP) + clock_width + battery_width)
        .min(full.width() * 0.62);
    let dock_top = full.max.y - 69.0;
    let tray = Rect::from_min_size(
        pos2(full.max.x - tray_width - EDGE, dock_top),
        vec2(tray_width, 58.0),
    );
    let preferred_width = (full.width() * 0.44).clamp(220.0, 760.0);
    let left_space = (tray.min.x - full.min.x - EDGE * 2.0).max(1.0);
    let center_space = tray.min.x - full.center().x - EDGE;
    let app_width = match position {
        DookPosition::Left => preferred_width.min(left_space),
        DookPosition::Center if center_space < 80.0 => preferred_width.min(left_space),
        DookPosition::Center => preferred_width.min(center_space * 2.0),
        DookPosition::Right => preferred_width.min(left_space),
    };
    let x = match position {
        DookPosition::Left => full.min.x + EDGE,
        DookPosition::Center if center_space < 80.0 => full.min.x + EDGE,
        DookPosition::Center => full.center().x - app_width * 0.5,
        DookPosition::Right => tray.min.x - app_width - EDGE,
    };
    (
        Rect::from_min_size(pos2(x, dock_top), vec2(app_width, 58.0)),
        Some(tray),
    )
}

fn notch_rects(full: Rect, position: DookPosition) -> (Rect, Option<Rect>) {
    let island_width = 480.0_f32.min((full.width() - EDGE * 2.0).max(1.0));
    let x = match position {
        DookPosition::Left => full.min.x + EDGE,
        DookPosition::Center | DookPosition::Right => full.center().x - island_width * 0.5,
    };
    let island = Rect::from_min_max(
        pos2(x, full.max.y - HEIGHT + 4.0),
        pos2(x + island_width, full.max.y),
    );
    (island, Some(full))
}

fn paint_surface(
    ui: &Ui,
    full: Rect,
    rect: Rect,
    style: DookStyle,
    radius: u8,
    fill: Color32,
    stroke: Color32,
) {
    match style {
        DookStyle::Pill => {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(radius), fill);
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(radius),
                Stroke::new(1.0, stroke),
                egui::StrokeKind::Inside,
            );
        }
        DookStyle::Notch => {
            let rail = Rect::from_min_max(pos2(full.min.x, full.max.y - 34.0), full.max);
            let strip_rounding = CornerRadius {
                nw: radius,
                ne: radius,
                sw: 0,
                se: 0,
            };
            ui.painter().rect_filled(rail, strip_rounding, fill);
            ui.painter().rect_stroke(
                rail,
                strip_rounding,
                Stroke::new(1.0, stroke),
                egui::StrokeKind::Inside,
            );
            let island_rounding = CornerRadius {
                nw: radius,
                ne: radius,
                sw: 0,
                se: 0,
            };
            ui.painter().rect_filled(rect, island_rounding, fill);
            ui.painter().rect_stroke(
                rect,
                island_rounding,
                Stroke::new(1.0, stroke),
                egui::StrokeKind::Inside,
            );
        }
    }
}

fn draw_apps(
    ui: &mut Ui,
    rect: Rect,
    cfg: &DookConfig,
    pins: &[PinnedApp],
    app_icons: &HashMap<String, TextureHandle>,
    icon_color: Color32,
    button_radius: u8,
) -> Option<Action> {
    let row = rect.shrink2(vec2(10.0, 7.0));
    let mut content = ui.new_child(
        UiBuilder::new()
            .max_rect(row)
            .layout(Layout::left_to_right(Align::Center)),
    );
    let mut action = None;
    if cfg.show_start {
        let glyph = if cfg.start_target == DookTarget::Nook {
            icons::SQUARES_FOUR
        } else {
            icons::WINDOWS_LOGO
        };
        if task_button(
            &mut content,
            "start",
            glyph,
            "Open Start",
            icon_color,
            button_radius,
        )
        .clicked()
        {
            if cfg.start_target == DookTarget::Nook {
                action = Some(Action::Start);
            } else {
                nook_platform::shell::show_start_menu();
            }
        }
    }
    if cfg.show_search
        && task_button(
            &mut content,
            "search",
            icons::MAGNIFYING_GLASS,
            "Search",
            icon_color,
            button_radius,
        )
        .clicked()
    {
        if cfg.search_target == DookTarget::Windows {
            nook_platform::shell::windows_shortcut(b'S');
        } else {
            action = Some(Action::Search);
        }
    }
    if cfg.show_start || cfg.show_search {
        content.separator();
    }

    let icon_tint = if cfg.monochrome_icons {
        icon_color
    } else {
        Color32::WHITE
    };
    egui::ScrollArea::horizontal()
        .id_salt("dook-pinned-apps")
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(&mut content, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = PIN_GAP;
                for (index, pin) in pins.iter().enumerate() {
                    let response = app_button(
                        ui,
                        index,
                        &pin.label,
                        app_icons.get(&pin.path),
                        icon_tint,
                        button_radius,
                    );
                    if response.clicked()
                        && let Err(error) = nook_platform::shell::open(&pin.path)
                    {
                        tracing::warn!("launching pinned taskbar app {}: {error}", pin.label);
                    }
                }
            });
        });
    action
}

fn draw_tray(ui: &mut Ui, rect: Rect, cfg: &DookConfig, look: TrayLook<'_>) -> Option<Action> {
    let row = if cfg.style == DookStyle::Notch {
        Rect::from_min_max(pos2(rect.min.x + 12.0, rect.max.y - 35.0), rect.max)
    } else {
        rect.shrink2(vec2(8.0, 7.0))
    };
    let mut ui = ui.new_child(
        UiBuilder::new()
            .max_rect(row)
            .layout(Layout::left_to_right(Align::Center)),
    );
    ui.spacing_mut().item_spacing.x = PIN_GAP;
    let mut action = None;
    if cfg.show_quick_settings
        && task_button(
            &mut ui,
            "quick-settings",
            icons::CARET_UP,
            "Open quick settings",
            look.icon_color,
            look.button_radius,
        )
        .clicked()
    {
        if cfg.quick_settings_target == DookTarget::Windows {
            nook_platform::shell::windows_shortcut(b'A');
        } else {
            action = Some(Action::QuickSettings);
        }
    }
    if cfg.show_emoji
        && task_button(
            &mut ui,
            "emoji",
            icons::SMILEY,
            "Open emoji picker",
            look.icon_color,
            look.button_radius,
        )
        .clicked()
    {
        nook_platform::shell::show_emoji_picker();
    }
    if cfg.show_battery
        && let Some(battery) = look.battery
    {
        let glyph = if battery.charging {
            icons::BATTERY_CHARGING
        } else {
            icons::BATTERY_FULL
        };
        let label = format!("{}%", battery.percent);
        if task_button(
            &mut ui,
            "battery",
            glyph,
            &format!("Battery {label}"),
            look.icon_color,
            look.button_radius,
        )
        .clicked()
            && let Err(error) = nook_platform::shell::open("ms-settings:powersleep")
        {
            tracing::warn!("opening battery settings from Dook: {error}");
        }
        ui.label(
            egui::RichText::new(label)
                .size(11.0)
                .color(look.theme.text_dim),
        );
    }
    if cfg.show_volume
        && task_button(
            &mut ui,
            "volume",
            icons::SPEAKER_HIGH,
            "Open sound settings",
            look.icon_color,
            look.button_radius,
        )
        .clicked()
        && let Err(error) = nook_platform::shell::open("ms-settings:sound")
    {
        tracing::warn!("opening sound settings from Dook: {error}");
    }
    if cfg.show_wifi
        && task_button(
            &mut ui,
            "wifi",
            icons::WIFI_HIGH,
            "Open Wi-Fi settings",
            look.icon_color,
            look.button_radius,
        )
        .clicked()
        && let Err(error) = nook_platform::shell::open("ms-settings:network-wifi")
    {
        tracing::warn!("opening Wi-Fi settings from Dook: {error}");
    }
    if cfg.show_date || cfg.show_clock {
        let (time_rect, _) = ui.allocate_exact_size(vec2(78.0, 42.0), Sense::hover());
        if ui
            .interact(time_rect, ui.id().with("clock"), Sense::click())
            .on_hover_text("Open notifications and calendar")
            .clicked()
        {
            nook_platform::shell::windows_shortcut(b'N');
        }
        let painter = ui.painter();
        if cfg.show_clock {
            painter.text(
                time_rect.center_top() + vec2(0.0, 9.0),
                Align2::CENTER_CENTER,
                &look.clock.time,
                egui::FontId::proportional(13.0),
                look.theme.text,
            );
        }
        if cfg.show_date {
            painter.text(
                time_rect.center_bottom() - vec2(0.0, 8.0),
                Align2::CENTER_CENTER,
                &look.clock.date,
                egui::FontId::proportional(10.0),
                look.theme.text_dim,
            );
        }
    }
    action
}

fn task_button(
    ui: &mut Ui,
    id: &'static str,
    glyph: &str,
    tip: &str,
    color: Color32,
    radius: u8,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(STATUS_SIZE, STATUS_SIZE), Sense::hover());
    let response = ui.interact(rect, ui.id().with(id), Sense::click());
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(radius),
            Color32::from_white_alpha(if response.is_pointer_button_down_on() {
                38
            } else {
                22
            }),
        );
    }
    paint::icon(ui.painter(), rect.center(), glyph, 18.0, color, false);
    response.on_hover_text(tip)
}

fn app_button(
    ui: &mut Ui,
    index: usize,
    label: &str,
    texture: Option<&TextureHandle>,
    tint: Color32,
    radius: u8,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(PIN_SIZE, PIN_SIZE), Sense::hover());
    let response = ui.interact(rect, ui.id().with(("dook-pin", index)), Sense::click());
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(radius),
            Color32::from_white_alpha(if response.is_pointer_button_down_on() {
                38
            } else {
                22
            }),
        );
    }
    if let Some(texture) = texture {
        ui.painter().image(
            texture.id(),
            rect.shrink(5.0),
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            tint,
        );
    } else {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            label
                .chars()
                .next()
                .unwrap_or('A')
                .to_uppercase()
                .to_string(),
            egui::FontId::proportional(16.0),
            tint,
        );
    }
    response.on_hover_text(label)
}
