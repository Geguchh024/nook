//! Resizable Wi-Fi tile and an on-demand network browser.

use crate::kit;
use crossbeam_channel::{Receiver, Sender, unbounded};
use nook_platform::controls::RadioKind;
use nook_platform::wireless::{self, WifiNetwork, WifiSnapshot};
use nook_ui::module::{Badge, Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme};
use nook_ui::{icons, paint, widgets};
use std::thread::JoinHandle;

enum Command {
    Refresh,
    Radio(bool),
    Connect(String, Option<String>),
    Stop,
}

enum ResultEvent {
    Snapshot(Result<WifiSnapshot, String>),
    Radio(bool, Result<bool, String>),
    Connected(String, Result<(), String>),
    Notice(String),
}

struct Worker {
    tx: Sender<Command>,
    rx: Receiver<ResultEvent>,
    join: Option<JoinHandle<()>>,
}

impl Worker {
    fn spawn(ctx: egui::Context) -> std::io::Result<Self> {
        let (tx, commands) = unbounded();
        let (results, rx) = unbounded();
        let join = std::thread::Builder::new()
            .name("nook-wifi".into())
            .spawn(move || {
                if let Err(error) = wireless::initialize_worker() {
                    let _ = results.send(ResultEvent::Snapshot(Err(error.to_string())));
                    return;
                }
                let (notice_tx, notice_rx) = unbounded();
                let _notice_keepalive = notice_tx.clone();
                let _watcher = match wireless::watch_wifi(notice_tx) {
                    Ok(watcher) => Some(watcher),
                    Err(error) => {
                        tracing::debug!("Wi-Fi event watcher unavailable: {error}");
                        None
                    }
                };
                let initial = WifiSnapshot {
                    enabled: nook_platform::controls::radio_enabled(RadioKind::Wifi),
                    ..WifiSnapshot::default()
                };
                let _ = results.send(ResultEvent::Snapshot(Ok(initial)));
                ctx.request_repaint();
                loop {
                    crossbeam_channel::select! {
                        recv(commands) -> command => {
                            let Ok(command) = command else { break };
                            if matches!(command, Command::Stop) { break; }
                            let event = match command {
                                Command::Stop => break,
                                Command::Refresh => ResultEvent::Snapshot(
                                    wireless::scan_wifi().map_err(|e| e.to_string()),
                                ),
                                Command::Radio(on) => ResultEvent::Radio(
                                    on,
                                    wireless::set_radio(RadioKind::Wifi, on).map_err(|e| e.to_string()),
                                ),
                                Command::Connect(ssid, password) => ResultEvent::Connected(
                                    ssid.clone(),
                                    wireless::connect_wifi(&ssid, password.as_deref())
                                        .map_err(|e| e.to_string()),
                                ),
                            };
                            if results.send(event).is_err() { break; }
                            ctx.request_repaint();
                        }
                        recv(notice_rx) -> notice => {
                            if let Ok(notice) = notice {
                                if results.send(ResultEvent::Notice(notice)).is_err() { break; }
                                ctx.request_repaint();
                            }
                        }
                    }
                }
                wireless::uninitialize_worker();
            })?;
        Ok(Self {
            tx,
            rx,
            join: Some(join),
        })
    }

    fn send(&self, command: Command) {
        if self.tx.send(command).is_err() {
            tracing::warn!("Wi-Fi worker stopped");
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub struct Wifi {
    worker: Option<Worker>,
    state: WifiSnapshot,
    page_request: bool,
    refresh_on_page: bool,
    loading: bool,
    selected: Option<String>,
    password: String,
    error: Option<String>,
    notice: Option<String>,
    notice_until: f64,
    last_now: f64,
    ctx: Option<egui::Context>,
}

impl Default for Wifi {
    fn default() -> Self {
        Self {
            worker: None,
            state: WifiSnapshot::default(),
            page_request: false,
            refresh_on_page: true,
            loading: false,
            selected: None,
            password: String::new(),
            error: None,
            notice: None,
            notice_until: 0.0,
            last_now: 0.0,
            ctx: None,
        }
    }
}

impl Wifi {
    fn send(&self, command: Command) {
        if let Some(worker) = &self.worker {
            worker.send(command);
        }
    }

    fn notify(&mut self, message: String) {
        self.notice = Some(message);
        self.notice_until = self.last_now + 5.0;
    }

    fn tile_summary(&self) -> &str {
        if self.state.enabled == Some(false) {
            "Off"
        } else {
            self.state
                .current_ssid
                .as_deref()
                .unwrap_or("Not connected")
        }
    }

    fn network_row(&mut self, ui: &mut egui::Ui, network: &WifiNetwork, theme: &Theme) {
        let width = ui.available_width();
        let (row, _) = ui.allocate_exact_size(egui::vec2(width, 42.0), egui::Sense::hover());
        widgets::card(ui.painter(), row, theme);
        let response = ui.interact(
            row,
            ui.id().with(("wifi-network", &network.ssid)),
            egui::Sense::click(),
        );
        if self.selected.as_deref() == Some(network.ssid.as_str()) {
            ui.painter().rect_stroke(
                row,
                nook_ui::theme::TILE_RADIUS,
                egui::Stroke::new(1.0, theme.accent),
                egui::StrokeKind::Inside,
            );
        }
        paint::icon(
            ui.painter(),
            egui::pos2(row.min.x + 18.0, row.center().y),
            icons::WIFI_HIGH,
            17.0,
            if network.connected {
                theme.accent
            } else {
                theme.text_dim
            },
            network.connected,
        );
        let trailing = if network.connected {
            84.0
        } else if network.secured {
            38.0
        } else {
            12.0
        };
        let label_rect = egui::Rect::from_min_max(
            egui::pos2(row.min.x + 34.0, row.min.y),
            egui::pos2((row.max.x - trailing).max(row.min.x + 40.0), row.max.y),
        );
        // The WLAN API's 0–100 field is an adapter signal-quality score.
        // Showing it as a percent looked like a battery/connection value.
        let quality = match network.signal {
            80..=100 => "Excellent signal",
            60..=79 => "Good signal",
            35..=59 => "Fair signal",
            _ => "Weak signal",
        };
        let label = format!("{}  ·  {quality}", network.ssid);
        ui.put(
            label_rect,
            egui::Label::new(egui::RichText::new(label).font(Theme::font(SIZE_LABEL))).truncate(),
        );
        if network.connected {
            ui.painter().text(
                egui::pos2(row.max.x - 12.0, row.center().y),
                egui::Align2::RIGHT_CENTER,
                "Connected",
                Theme::font(SIZE_LABEL - 1.0),
                theme.accent,
            );
        } else if network.secured {
            paint::icon(
                ui.painter(),
                egui::pos2(row.max.x - 18.0, row.center().y),
                icons::LOCK_KEY,
                15.0,
                theme.text_faint,
                false,
            );
        }
        if response.clicked() {
            self.error = None;
            if network.connected {
                self.selected = None;
            } else if network.secured
                && network.saved_profile.is_none()
                && !network.password_supported
            {
                if let Err(error) = nook_platform::shell::open("ms-settings:network-wifi") {
                    self.error = Some(error.to_string());
                }
            } else if network.secured
                && network.saved_profile.is_none()
                && network.password_supported
            {
                self.selected = Some(network.ssid.clone());
                self.password.clear();
            } else {
                self.selected = None;
                self.loading = true;
                self.send(Command::Connect(network.ssid.clone(), None));
            }
        }
    }
}

impl Module for Wifi {
    fn id(&self) -> &'static str {
        "wifi"
    }
    fn name(&self) -> &str {
        "Wi-Fi"
    }
    fn icon(&self) -> &'static str {
        icons::WIFI_HIGH
    }
    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }
    fn page_size(&self) -> Option<egui::Vec2> {
        Some(egui::vec2(620.0, 440.0))
    }
    fn is_active(&self) -> bool {
        self.state.enabled == Some(true)
    }

    fn uses_active_tile_fill(&self) -> bool {
        false
    }

    fn collapsed_badge(&self) -> Option<Badge> {
        (self.notice_until > self.last_now)
            .then(|| {
                self.notice.clone().map(|label| Badge::Notice {
                    label,
                    until: self.notice_until,
                })
            })
            .flatten()
    }

    fn attach(&mut self, ctx: &egui::Context) {
        self.ctx = Some(ctx.clone());
        match Worker::spawn(ctx.clone()) {
            Ok(worker) => self.worker = Some(worker),
            Err(error) => tracing::warn!("Wi-Fi worker: {error}"),
        }
    }

    fn on_tick(&mut self) {
        if let Some(ctx) = &self.ctx {
            self.last_now = ctx.input(|input| input.time);
        }
        let events: Vec<_> = self
            .worker
            .as_ref()
            .map(|worker| worker.rx.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                ResultEvent::Snapshot(Ok(snapshot)) => {
                    self.state = snapshot;
                    self.loading = false;
                    self.error = None;
                }
                ResultEvent::Snapshot(Err(error)) => {
                    self.loading = false;
                    self.error = Some(error);
                }
                ResultEvent::Radio(on, Ok(true)) => {
                    self.state.enabled = Some(on);
                    self.loading = true;
                    if on {
                        self.send(Command::Refresh);
                    } else {
                        self.state.current_ssid = None;
                        self.state.networks.clear();
                        self.loading = false;
                    }
                }
                ResultEvent::Radio(_, Ok(false)) => {
                    self.error = Some("Windows did not allow changing Wi-Fi".into());
                    self.loading = false;
                }
                ResultEvent::Radio(_, Err(error)) | ResultEvent::Connected(_, Err(error)) => {
                    self.error = Some(error);
                    self.loading = false;
                }
                ResultEvent::Connected(ssid, Ok(())) => {
                    self.selected = None;
                    self.password.clear();
                    self.notify(format!("Connecting to {ssid}"));
                    self.loading = true;
                    self.send(Command::Refresh);
                }
                ResultEvent::Notice(notice) => self.notify(notice),
            }
        }
    }

    fn ui_tile(&mut self, ui: &mut egui::Ui, ctx: &mut TileCtx<'_>) {
        let rect = ui.max_rect();
        let response = ui.interact(rect, ui.id().with("wifi-page"), egui::Sense::click());
        widgets::wireless_tile(
            ui,
            self.icon(),
            "Wi-Fi",
            self.tile_summary(),
            self.state.enabled == Some(true),
            Some(&format!("{} networks nearby", self.state.networks.len())),
            ctx.theme,
        );
        if response.clicked() {
            self.page_request = true;
            self.refresh_on_page = true;
        }
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.page_request)
    }

    fn ui_page(
        &mut self,
        ui: &mut egui::Ui,
        _header: &mut egui::Ui,
        body: egui::Rect,
        theme: &Theme,
    ) -> bool {
        self.last_now = ui.input(|input| input.time);
        if self.refresh_on_page {
            self.send(Command::Refresh);
            self.refresh_on_page = false;
            self.loading = true;
        }
        let Some(worker) = &self.worker else {
            ui.put(body, egui::Label::new("Wi-Fi controls are unavailable."));
            return false;
        };
        let tx = worker.tx.clone();
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(body)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.set_width(body.width());
                ui.set_height(body.height());
                // Keep the connection state and actions in separate rows so
                // the controls reflow instead of colliding on narrow panels.
                ui.horizontal_wrapped(|ui| {
                    let detected_on =
                        self.state.current_ssid.is_some() || !self.state.networks.is_empty();
                    let mut enabled = self.state.enabled.unwrap_or(detected_on);
                    if widgets::toggle(ui, &mut enabled, theme).changed() {
                        self.state.enabled = Some(enabled);
                        self.loading = true;
                        tx.send(Command::Radio(enabled)).ok();
                    }
                    ui.label(
                        egui::RichText::new(if self.state.enabled.is_none() && !detected_on {
                            "Wi-Fi status unavailable"
                        } else if enabled {
                            "Wi-Fi on"
                        } else {
                            "Wi-Fi off"
                        })
                        .font(Theme::font(SIZE_BODY)),
                    );
                    if let Some(ssid) = &self.state.current_ssid {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(format!("Connected · {ssid}"))
                                    .color(theme.text_dim),
                            )
                            .truncate(),
                        );
                    }
                });
                ui.horizontal_wrapped(|ui| {
                    let (more_rect, _) =
                        ui.allocate_exact_size(egui::vec2(124.0, 30.0), egui::Sense::hover());
                    let more = kit::button(
                        ui,
                        more_rect,
                        "More settings",
                        Some(icons::GEAR_SIX),
                        false,
                        theme,
                    );
                    if more.clicked()
                        && let Err(error) = nook_platform::shell::open("ms-settings:network-wifi")
                    {
                        self.error = Some(error.to_string());
                    }
                    let (refresh_rect, _) =
                        ui.allocate_exact_size(egui::vec2(98.0, 30.0), egui::Sense::hover());
                    if kit::button(
                        ui,
                        refresh_rect,
                        "Refresh",
                        Some(icons::ARROW_CLOCKWISE),
                        false,
                        theme,
                    )
                    .clicked()
                    {
                        self.loading = true;
                        tx.send(Command::Refresh).ok();
                    }
                });
                if self.state.enabled == Some(false) {
                    ui.label(
                        egui::RichText::new("Turn Wi-Fi on to see nearby networks.")
                            .color(theme.text_dim),
                    );
                } else {
                    ui.label(
                        egui::RichText::new(if self.loading {
                            "Scanning for networks…"
                        } else {
                            "Available networks"
                        })
                        .font(Theme::font(SIZE_LABEL))
                        .color(theme.text_dim),
                    );
                    let reserved = if self.selected.is_some() { 104.0 } else { 54.0 };
                    let list_height = (ui.available_height() - reserved).max(64.0);
                    egui::ScrollArea::vertical()
                        .id_salt("wifi-networks")
                        .max_height(list_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            for network in self.state.networks.clone() {
                                self.network_row(ui, &network, theme);
                            }
                        });
                    if let Some(selected) = self.selected.clone() {
                        let network = self
                            .state
                            .networks
                            .iter()
                            .find(|network| network.ssid == selected);
                        ui.horizontal_wrapped(|ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(selected.clone())
                                        .font(Theme::font(SIZE_LABEL)),
                                )
                                .truncate(),
                            );
                            if network.is_some_and(|network| {
                                network.secured
                                    && network.saved_profile.is_none()
                                    && network.password_supported
                            }) {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.password)
                                        .password(true)
                                        .hint_text("Password")
                                        .desired_width(ui.available_width().clamp(90.0, 170.0)),
                                );
                            }
                            let ready = network.is_some_and(|network| {
                                network.connected
                                    || !network.secured
                                    || network.saved_profile.is_some()
                                    || (network.password_supported && !self.password.is_empty())
                            });
                            let unsupported = network.is_some_and(|network| {
                                network.secured
                                    && !network.password_supported
                                    && network.saved_profile.is_none()
                            });
                            let label = if network.is_some_and(|network| network.connected) {
                                "Connected"
                            } else if unsupported {
                                "Settings"
                            } else {
                                "Connect"
                            };
                            if kit::button(
                                ui,
                                egui::Rect::from_min_size(ui.cursor().min, egui::vec2(98.0, 32.0)),
                                label,
                                Some(if unsupported {
                                    icons::GEAR_SIX
                                } else {
                                    icons::ARROW_RIGHT
                                }),
                                unsupported,
                                theme,
                            )
                            .clicked()
                            {
                                if unsupported {
                                    if let Err(error) =
                                        nook_platform::shell::open("ms-settings:network-wifi")
                                    {
                                        self.error = Some(error.to_string());
                                    }
                                } else if ready {
                                    self.loading = true;
                                    let password =
                                        (!self.password.is_empty()).then(|| self.password.clone());
                                    tx.send(Command::Connect(selected, password)).ok();
                                }
                            }
                        });
                    }
                }
                if let Some(error) = &self.error {
                    ui.label(egui::RichText::new(error).color(theme.text_dim));
                    if error.contains("Windows Settings")
                        && ui.link("Open Windows Wi-Fi settings").clicked()
                    {
                        let _ = nook_platform::shell::open("ms-settings:network-wifi");
                    }
                }
            },
        );
        false
    }

    fn next_repaint(&self, _tile_visible: bool) -> Option<std::time::Duration> {
        (self.notice_until > self.last_now).then(|| {
            std::time::Duration::from_secs_f64((self.notice_until - self.last_now).max(0.02))
        })
    }
}
