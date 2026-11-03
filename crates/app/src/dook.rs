//! Dook: a configurable bottom taskbar with live Explorer pins and system actions.

use crate::config::{DookConfig, DookEdge, DookPosition, DookStyle, DookTarget};
use crossbeam_channel::{Receiver, Sender, unbounded};
use egui::{
    Align, Align2, Color32, Context, CornerRadius, Id, Layout, Rect, Sense, Stroke, TextureHandle,
    TextureOptions, Ui, UiBuilder, ViewportBuilder, ViewportClass, ViewportId, WindowLevel, pos2,
    vec2,
};
use nook_platform::monitor::{Monitor, Rect as MonitorRect};
use nook_platform::system::{self, Battery};
use nook_ui::theme::{Theme, parse_hex};
use nook_ui::{anim, icons, paint};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const TITLE: &str = "Dook - Nook";
const FLOATING_HEIGHT: f32 = 56.0;
const BAR_HEIGHT: f32 = 48.0;
const EDGE: f32 = 10.0;
const BUTTON_SIZE: f32 = 44.0;
const STATUS_SIZE: f32 = 28.0;
const ITEM_GAP: f32 = 6.0;
const DEFAULT_TASKBAR_COLOR: &str = "#17171A";
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct AppBarKey {
    monitor_id: isize,
    bounds: MonitorRect,
    edge: DookEdge,
    height: i32,
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
    quitting: bool,
    native_window: Option<isize>,
    viewport_open: bool,
    appbar: Option<nook_platform::appbar::AppBar>,
    appbar_key: Option<AppBarKey>,
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
            quitting: false,
            native_window: None,
            viewport_open: false,
            appbar: None,
            appbar_key: None,
        }
    }
}

impl Runtime {
    pub fn sync(&mut self, cfg: &DookConfig, monitor: Option<Monitor>, fullscreen_hidden: bool) {
        if self.quitting {
            self.restore_taskbar();
            return;
        }
        if !cfg.enabled || fullscreen_hidden || cfg.style != DookStyle::Bar {
            self.release_appbar();
        }
        let target =
            monitor.filter(|_| cfg.enabled && !fullscreen_hidden && cfg.hide_windows_taskbar);
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

    fn release_appbar(&mut self) {
        self.appbar.take();
        self.appbar_key = None;
    }

    /// Closes the dock viewport before the primary Nook viewport exits.
    pub fn shutdown(&mut self, ctx: &Context) {
        self.quitting = true;
        self.restore_taskbar();
        self.taskbar_requested = Some(false);
        self.taskbar_monitor = None;
        self.viewport_open = false;
        self.release_appbar();
        ctx.send_viewport_cmd_to(
            ViewportId::from_hash_of(TITLE),
            egui::ViewportCommand::Close,
        );
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
                    let image = match nook_platform::files::icon(&PathBuf::from(&worker_path), 64) {
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
        let area = if cfg.style == DookStyle::Bar || cfg.hide_windows_taskbar {
            monitor.bounds
        } else {
            monitor.work
        };
        let edge_top = cfg.edge == DookEdge::Top;
        let sensor_height = if self.auto_hidden {
            (7.0 * scale) as i32
        } else {
            ((if cfg.style == DookStyle::Bar {
                BAR_HEIGHT
            } else {
                FLOATING_HEIGHT + EDGE
            }) * scale) as i32
        };
        let pointer_inside = nook_platform::window::cursor_position().is_some_and(|(x, y)| {
            x >= area.left
                && x < area.right
                && if edge_top {
                    y >= area.top && y < area.top + sensor_height
                } else {
                    y >= area.bottom - sensor_height && y < area.bottom
                }
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
    fullscreen_hidden: bool,
) -> Option<Action> {
    if runtime.quitting {
        return None;
    }
    if !cfg.enabled || fullscreen_hidden {
        runtime.auto_hidden = false;
        runtime.native_window = None;
        if runtime.viewport_open {
            ctx.send_viewport_cmd_to(
                ViewportId::from_hash_of(TITLE),
                egui::ViewportCommand::Close,
            );
            runtime.viewport_open = false;
        }
        return None;
    }
    if monitor.is_none() {
        runtime.auto_hidden = false;
        runtime.native_window = None;
        if runtime.viewport_open {
            ctx.send_viewport_cmd_to(
                ViewportId::from_hash_of(TITLE),
                egui::ViewportCommand::Close,
            );
            runtime.viewport_open = false;
        }
    }
    let monitor = monitor?;
    runtime.viewport_open = true;
    runtime.refresh_pins(ctx);

    let scale = monitor.dpi as f32 / 96.0;
    let area = if cfg.style == DookStyle::Bar || cfg.hide_windows_taskbar {
        monitor.bounds
    } else {
        monitor.work
    };
    let width = area.width() as f32 / scale;
    let height = if cfg.style == DookStyle::Bar {
        BAR_HEIGHT
    } else {
        FLOATING_HEIGHT
    };
    let visible = runtime.sync_auto_hide(ctx, cfg, monitor);
    let edge_inset = if cfg.style == DookStyle::Floating {
        EDGE
    } else {
        0.0
    };
    let window_height = 7.0 + (height + edge_inset - 7.0) * visible;
    let reserve_appbar = cfg.style == DookStyle::Bar && (!cfg.auto_hide || !runtime.auto_hidden);
    let appbar_height = (height * scale).round() as i32;
    let desired_appbar_key = AppBarKey {
        monitor_id: monitor.id,
        bounds: monitor.bounds,
        edge: cfg.edge,
        height: appbar_height,
    };
    let assigned_bar = reserve_appbar
        .then(|| {
            (runtime.appbar_key == Some(desired_appbar_key))
                .then(|| {
                    runtime
                        .appbar
                        .as_ref()
                        .map(nook_platform::appbar::AppBar::rect)
                })
                .flatten()
        })
        .flatten();
    let viewport_width = assigned_bar.map_or(width, |rect| rect.width() as f32 / scale);
    let viewport_height = assigned_bar.map_or(window_height, |rect| rect.height() as f32 / scale);
    let x = assigned_bar.map_or(area.left as f32 / scale, |rect| rect.left as f32 / scale);
    let y = assigned_bar.map_or_else(
        || {
            if cfg.edge == DookEdge::Top {
                area.top as f32 / scale
            } else {
                (area.bottom as f32 / scale) - window_height
            }
        },
        |rect| rect.top as f32 / scale,
    );
    let position = pos2(x, y);
    let builder = ViewportBuilder::default()
        .with_title(TITLE)
        .with_position(position)
        .with_inner_size(vec2(viewport_width, viewport_height))
        .with_min_inner_size(vec2(viewport_width, viewport_height))
        .with_max_inner_size(vec2(viewport_width, viewport_height))
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_has_shadow(false)
        .with_taskbar(false)
        .with_active(false)
        .with_window_level(WindowLevel::AlwaysOnTop);
    runtime.load_pin_icons(ctx);
    let icons = &runtime.icons;
    let pins = &runtime.pins;
    let native_window = &mut runtime.native_window;
    let appbar = &mut runtime.appbar;
    let appbar_key = &mut runtime.appbar_key;
    ctx.show_viewport_immediate(ViewportId::from_hash_of(TITLE), builder, |ctx, class| {
        if class == ViewportClass::EmbeddedWindow {
            return None;
        }
        let hwnd = prepare_native_window(native_window);
        if sync_appbar(
            appbar,
            appbar_key,
            hwnd,
            monitor,
            cfg.edge,
            appbar_height,
            reserve_appbar,
        ) {
            ctx.request_repaint();
        }
        ui(ctx, cfg, theme, battery, pins, icons)
    })
}

fn prepare_native_window(cached: &mut Option<isize>) -> Option<nook_platform::Hwnd> {
    let hwnd = nook_platform::window::find_own(TITLE)?;
    if *cached == Some(hwnd.raw()) {
        return Some(hwnd);
    }
    if let Err(error) = nook_platform::window::ensure_overlay(hwnd, false) {
        tracing::warn!("taskbar window style: {error}");
    }
    if let Err(error) = nook_platform::window::fill_client(hwnd) {
        tracing::warn!("taskbar window client area: {error}");
    }
    *cached = Some(hwnd.raw());
    Some(hwnd)
}

fn sync_appbar(
    appbar: &mut Option<nook_platform::appbar::AppBar>,
    key: &mut Option<AppBarKey>,
    hwnd: Option<nook_platform::Hwnd>,
    monitor: Monitor,
    edge: DookEdge,
    height: i32,
    needed: bool,
) -> bool {
    if !needed {
        let changed = appbar.take().is_some();
        *key = None;
        return changed;
    }
    let desired = AppBarKey {
        monitor_id: monitor.id,
        bounds: monitor.bounds,
        edge,
        height,
    };
    if *key == Some(desired) && appbar.is_some() {
        return false;
    }
    let Some(hwnd) = hwnd else {
        return false;
    };
    appbar.take();
    *key = None;
    match nook_platform::appbar::AppBar::reserve(
        hwnd,
        monitor.bounds,
        edge == DookEdge::Top,
        height,
    ) {
        Ok(reservation) => {
            let rect = reservation.rect();
            if let Err(error) = nook_platform::window::set_rect(
                hwnd,
                rect.left,
                rect.top,
                rect.width(),
                rect.height(),
            ) {
                tracing::warn!("positioning taskbar AppBar: {error}");
            }
            *appbar = Some(reservation);
            *key = Some(desired);
            true
        }
        Err(error) => {
            tracing::warn!("registering taskbar AppBar: {error}");
            false
        }
    }
}

fn ui(
    ctx: &Context,
    cfg: &DookConfig,
    theme: &Theme,
    battery: Option<Battery>,
    pins: &[PinnedApp],
    app_icons: &HashMap<String, TextureHandle>,
) -> Option<Action> {
    let background = taskbar_fill(cfg, theme);
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
    let stroke = theme.tile_rim;
    let mut action = None;
    egui::Area::new(Id::new("dook-root"))
        .order(egui::Order::Background)
        .fixed_pos(viewport.min)
        .show(ctx, |ui| {
            ui.set_min_size(viewport.size());
            let full = ui.max_rect();
            let taskbar = taskbar_rect(full, cfg);
            let (apps_rect, tray_rect) = content_rects(taskbar, cfg);
            paint_surface(ui, taskbar, cfg, radius, background, stroke);
            if cfg.auto_hide && full.height() < taskbar.height() - 0.5 {
                let sensor = if cfg.edge == DookEdge::Top {
                    Rect::from_min_max(full.min, pos2(full.max.x, full.min.y + full.height()))
                } else {
                    Rect::from_min_max(pos2(full.min.x, full.max.y - full.height()), full.max)
                };
                ui.painter()
                    .rect_filled(sensor, CornerRadius::same(3), theme.glass_bottom);
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
                tray_rect,
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

fn taskbar_rect(full: Rect, cfg: &DookConfig) -> Rect {
    let height = if cfg.style == DookStyle::Bar {
        BAR_HEIGHT
    } else {
        FLOATING_HEIGHT
    };
    let width = if cfg.style == DookStyle::Bar {
        full.width()
    } else {
        (full.width() - EDGE * 2.0).min(960.0)
    };
    let x = if cfg.style == DookStyle::Bar {
        full.min.x
    } else {
        match cfg.apps_position {
            DookPosition::Left => full.min.x + EDGE,
            DookPosition::Center | DookPosition::Right => full.center().x - width * 0.5,
        }
    };
    let edge_inset = if cfg.style == DookStyle::Floating {
        EDGE
    } else {
        0.0
    };
    let y = match cfg.edge {
        DookEdge::Top => full.min.y + edge_inset,
        DookEdge::Bottom => full.max.y - height - edge_inset,
    };
    Rect::from_min_size(pos2(x, y), vec2(width, height))
}

fn content_rects(taskbar: Rect, cfg: &DookConfig) -> (Rect, Rect) {
    let nav_buttons = [cfg.show_start, cfg.show_search, cfg.show_task_view]
        .into_iter()
        .filter(|show| *show)
        .count() as f32;
    let nav_width = nav_buttons * BUTTON_SIZE + if nav_buttons > 0.0 { 17.0 } else { 0.0 };
    let status_buttons = [
        cfg.show_quick_settings,
        cfg.show_wifi,
        cfg.show_volume,
        cfg.show_battery,
        cfg.show_emoji,
        cfg.show_notifications,
    ]
    .into_iter()
    .filter(|show| *show)
    .count() as f32;
    let clock_width = if cfg.show_clock || cfg.show_date {
        72.0
    } else {
        0.0
    };
    let show_desktop_width = if cfg.show_desktop { 9.0 } else { 0.0 };
    let tray_width =
        (status_buttons * (STATUS_SIZE + ITEM_GAP) + clock_width + show_desktop_width + 12.0)
            .min((taskbar.width() - nav_width - 12.0).max(0.0));
    let content_min = pos2(taskbar.min.x + 8.0, taskbar.min.y);
    let content_max = pos2(taskbar.max.x - 8.0, taskbar.max.y);
    let app_min = pos2(content_min.x, content_min.y);
    let app_max = pos2(
        (content_max.x - tray_width - 6.0).max(app_min.x),
        content_max.y,
    );
    let tray_min = pos2(app_max.x + 6.0, content_min.y);
    let apps = Rect::from_min_max(app_min, app_max);
    let tray = Rect::from_min_max(tray_min, content_max);
    (apps, tray)
}

fn taskbar_fill(cfg: &DookConfig, theme: &Theme) -> Color32 {
    if cfg.oled {
        Color32::BLACK
    } else if cfg
        .background_hex
        .eq_ignore_ascii_case(DEFAULT_TASKBAR_COLOR)
    {
        theme.glass_bottom
    } else {
        parse_hex(&cfg.background_hex)
            .unwrap_or(theme.glass_bottom)
            .gamma_multiply(0.94)
    }
}

fn paint_surface(
    ui: &Ui,
    rect: Rect,
    cfg: &DookConfig,
    radius: u8,
    fill: Color32,
    stroke: Color32,
) {
    let cap = (rect.height() * 0.5).round() as u8;
    let radius = radius.min(cap);
    let rounding = if cfg.style == DookStyle::Bar {
        match cfg.edge {
            DookEdge::Top => CornerRadius {
                nw: 0,
                ne: 0,
                sw: radius,
                se: radius,
            },
            DookEdge::Bottom => CornerRadius {
                nw: radius,
                ne: radius,
                sw: 0,
                se: 0,
            },
        }
    } else {
        CornerRadius::same(radius)
    };
    ui.painter().rect_filled(rect, rounding, fill);
    ui.painter().rect_stroke(
        rect,
        rounding,
        Stroke::new(1.0, stroke),
        egui::StrokeKind::Inside,
    );
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
    let row = rect.shrink2(vec2(8.0, 2.0));
    let mut content = ui.new_child(
        UiBuilder::new()
            .max_rect(row)
            .layout(Layout::left_to_right(Align::Center)),
    );
    content.set_clip_rect(row);
    let mut action = None;
    if cfg.show_start
        && taskbar_button(
            &mut content,
            "start",
            icons::SQUARES_FOUR,
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
    if cfg.show_search
        && taskbar_button(
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
    if cfg.show_task_view
        && taskbar_button(
            &mut content,
            "task-view",
            icons::LAYOUT,
            "Task View",
            icon_color,
            button_radius,
        )
        .clicked()
    {
        nook_platform::shell::show_task_view();
    }
    if cfg.show_start || cfg.show_search || cfg.show_task_view {
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
                ui.spacing_mut().item_spacing.x = ITEM_GAP;
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
    let row = rect.shrink2(vec2(4.0, 2.0));
    let mut ui = ui.new_child(
        UiBuilder::new()
            .max_rect(row)
            .layout(Layout::left_to_right(Align::Center)),
    );
    ui.set_clip_rect(row);
    ui.spacing_mut().item_spacing.x = ITEM_GAP;
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
    if cfg.show_notifications
        && task_button(
            &mut ui,
            "notifications",
            icons::BELL,
            "Notifications",
            look.icon_color,
            look.button_radius,
        )
        .clicked()
    {
        nook_platform::shell::show_notifications();
    }
    if cfg.show_desktop {
        let (rect, _) = ui.allocate_exact_size(vec2(8.0, 40.0), Sense::hover());
        let response = ui.interact(rect, ui.id().with("show-desktop"), Sense::click());
        ui.painter().vline(
            rect.center().x,
            rect.y_range(),
            Stroke::new(1.0, look.theme.text_faint),
        );
        if response.clicked() {
            nook_platform::shell::toggle_desktop();
        }
    }
    action
}

fn taskbar_button(
    ui: &mut Ui,
    id: &'static str,
    glyph: &str,
    tip: &str,
    color: Color32,
    radius: u8,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(BUTTON_SIZE, BUTTON_SIZE), Sense::hover());
    let response = ui.interact(rect, ui.id().with(id), Sense::click());
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(radius),
            Color32::from_white_alpha(if response.is_pointer_button_down_on() {
                32
            } else {
                20
            }),
        );
    }
    paint::icon(ui.painter(), rect.center(), glyph, 26.0, color, false);
    response.on_hover_text(tip)
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
                20
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
    let (rect, _) = ui.allocate_exact_size(vec2(BUTTON_SIZE, BUTTON_SIZE), Sense::hover());
    let response = ui.interact(rect, ui.id().with(("dook-pin", index)), Sense::click());
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(radius),
            Color32::from_white_alpha(if response.is_pointer_button_down_on() {
                38
            } else {
                20
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
        paint::icon(
            ui.painter(),
            rect.center(),
            icons::SQUARES_FOUR,
            22.0,
            tint,
            false,
        );
    }
    response.on_hover_text(label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floating_taskbar_keeps_its_edge_margin() {
        let full = Rect::from_min_size(pos2(0.0, 0.0), vec2(1920.0, 56.0));
        let cfg = DookConfig::default();
        let rect = taskbar_rect(full, &cfg);
        assert_eq!(rect.width(), 960.0);
        assert_eq!(rect.height(), FLOATING_HEIGHT);
        assert_eq!(rect.max.y, 46.0);
    }

    #[test]
    fn bar_taskbar_touches_selected_screen_edge() {
        let full = Rect::from_min_size(pos2(0.0, 0.0), vec2(1920.0, 48.0));
        let cfg = DookConfig {
            style: DookStyle::Bar,
            edge: DookEdge::Top,
            ..Default::default()
        };
        let rect = taskbar_rect(full, &cfg);
        assert_eq!(rect.width(), full.width());
        assert_eq!(rect.min.y, full.min.y);
        assert_eq!(rect.height(), BAR_HEIGHT);
    }

    #[test]
    fn content_groups_do_not_overlap() {
        let taskbar = Rect::from_min_size(pos2(0.0, 0.0), vec2(960.0, FLOATING_HEIGHT));
        let cfg = DookConfig::default();
        let (apps, tray) = content_rects(taskbar, &cfg);
        assert!(apps.max.x < tray.min.x);
        assert!(apps.min.x >= taskbar.min.x);
        assert!(tray.max.x <= taskbar.max.x);
    }
}
