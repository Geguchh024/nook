//! Resizable Bluetooth tile, device discovery and pairing.

use crate::kit;
use crossbeam_channel::{Receiver, Sender, unbounded};
use nook_platform::controls::RadioKind;
use nook_platform::wireless::{self, BluetoothDeviceInfo, BluetoothSnapshot};
use nook_ui::module::{Badge, Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme};
use nook_ui::{icons, paint, widgets};
use std::thread::JoinHandle;

enum Command {
    Refresh,
    Radio(bool),
    Pair(String, String, bool),
    Stop,
}

enum ResultEvent {
    Snapshot(Result<BluetoothSnapshot, String>),
    Radio(bool, Result<bool, String>),
    Pair(String, bool, Result<(), String>),
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
            .name("nook-bluetooth".into())
            .spawn(move || {
                if let Err(error) = wireless::initialize_worker() {
                    let _ = results.send(ResultEvent::Snapshot(Err(error.to_string())));
                    return;
                }
                let (notice_tx, notice_rx) = unbounded();
                let _notice_keepalive = notice_tx.clone();
                let _watcher = match wireless::watch_bluetooth(notice_tx) {
                    Ok(watcher) => Some(watcher),
                    Err(error) => {
                        tracing::debug!("Bluetooth event watcher unavailable: {error}");
                        None
                    }
                };
                let initial = BluetoothSnapshot {
                    enabled: nook_platform::controls::radio_enabled(RadioKind::Bluetooth),
                    devices: Vec::new(),
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
                                    wireless::scan_bluetooth().map_err(|e| e.to_string()),
                                ),
                                Command::Radio(on) => ResultEvent::Radio(
                                    on,
                                    wireless::set_radio(RadioKind::Bluetooth, on).map_err(|e| e.to_string()),
                                ),
                                Command::Pair(id, name, paired) => ResultEvent::Pair(
                                    name,
                                    paired,
                                    wireless::set_bluetooth_pairing(&id, paired).map_err(|e| e.to_string()),
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
            tracing::warn!("Bluetooth worker stopped");
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

pub struct Bluetooth {
    worker: Option<Worker>,
    devices: Vec<BluetoothDeviceInfo>,
    enabled: Option<bool>,
    page_request: bool,
    refresh_on_page: bool,
    loading: bool,
    error: Option<String>,
    notice: Option<String>,
    notice_until: f64,
    last_now: f64,
    ctx: Option<egui::Context>,
}

impl Default for Bluetooth {
    fn default() -> Self {
        Self {
            worker: None,
            devices: Vec::new(),
            enabled: None,
            page_request: false,
            refresh_on_page: true,
            loading: false,
            error: None,
            notice: None,
            notice_until: 0.0,
            last_now: 0.0,
            ctx: None,
        }
    }
}

impl Bluetooth {
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
        if self.enabled == Some(false) {
            "Off"
        } else if let Some(device) = self.devices.iter().find(|device| device.connected) {
            &device.name
        } else {
            "Not connected"
        }
    }

    fn device_row(&mut self, ui: &mut egui::Ui, device: &BluetoothDeviceInfo, theme: &Theme) {
        ui.horizontal(|ui| {
            paint::icon(
                ui.painter(),
                ui.cursor().left_center() + egui::vec2(10.0, 0.0),
                if device.connected {
                    icons::BLUETOOTH_CONNECTED
                } else {
                    icons::BLUETOOTH
                },
                18.0,
                if device.connected {
                    theme.accent
                } else {
                    theme.text_dim
                },
                device.connected,
            );
            ui.vertical(|ui| {
                ui.label(egui::RichText::new(&device.name).font(Theme::font(SIZE_LABEL)));
                let state = if device.connected {
                    "Connected"
                } else if device.paired {
                    "Paired"
                } else {
                    "Nearby"
                };
                ui.label(
                    egui::RichText::new(state)
                        .font(Theme::font(SIZE_LABEL - 1.0))
                        .color(theme.text_dim),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if device.paired {
                    if kit::button(
                        ui,
                        egui::Rect::from_min_size(ui.cursor().min, egui::vec2(72.0, 30.0)),
                        "Remove",
                        None,
                        false,
                        theme,
                    )
                    .clicked()
                    {
                        self.send(Command::Pair(device.id.clone(), device.name.clone(), true));
                        self.loading = true;
                    }
                    if !device.connected
                        && kit::button(
                            ui,
                            egui::Rect::from_min_size(ui.cursor().min, egui::vec2(82.0, 30.0)),
                            "Connect",
                            None,
                            true,
                            theme,
                        )
                        .clicked()
                        && let Err(error) = wireless::open_bluetooth_settings()
                    {
                        self.error = Some(error.to_string());
                    }
                } else if device.can_pair
                    && kit::button(
                        ui,
                        egui::Rect::from_min_size(ui.cursor().min, egui::vec2(72.0, 30.0)),
                        "Pair",
                        None,
                        true,
                        theme,
                    )
                    .clicked()
                {
                    self.send(Command::Pair(device.id.clone(), device.name.clone(), false));
                    self.loading = true;
                }
            });
        });
    }
}

impl Module for Bluetooth {
    fn id(&self) -> &'static str {
        "bluetooth"
    }
    fn name(&self) -> &str {
        "Bluetooth"
    }
    fn icon(&self) -> &'static str {
        icons::BLUETOOTH
    }
    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }
    fn page_size(&self) -> Option<egui::Vec2> {
        Some(egui::vec2(620.0, 440.0))
    }
    fn is_active(&self) -> bool {
        self.enabled == Some(true)
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
            Err(error) => tracing::warn!("Bluetooth worker: {error}"),
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
                ResultEvent::Snapshot(Ok(devices)) => {
                    self.devices = devices.devices;
                    self.enabled = devices.enabled;
                    self.loading = false;
                    self.error = None;
                }
                ResultEvent::Snapshot(Err(error)) => {
                    self.loading = false;
                    self.error = Some(error);
                }
                ResultEvent::Radio(on, Ok(true)) => {
                    self.enabled = Some(on);
                    if on {
                        self.loading = true;
                        self.send(Command::Refresh);
                    } else {
                        self.devices.clear();
                        self.loading = false;
                    }
                }
                ResultEvent::Radio(_, Ok(false)) => {
                    self.error = Some("Windows did not allow changing Bluetooth".into());
                    self.loading = false;
                }
                ResultEvent::Radio(_, Err(error)) | ResultEvent::Pair(_, _, Err(error)) => {
                    self.error = Some(error);
                    self.loading = false;
                }
                ResultEvent::Pair(name, paired, Ok(())) => {
                    self.notify(if paired {
                        format!("{name} unpaired")
                    } else {
                        format!("Paired with {name}")
                    });
                    self.loading = true;
                    self.send(Command::Refresh);
                }
                ResultEvent::Notice(notice) => self.notify(notice),
            }
        }
    }

    fn ui_tile(&mut self, ui: &mut egui::Ui, ctx: &mut TileCtx<'_>) {
        let rect = ui.max_rect();
        let response = ui.interact(rect, ui.id().with("bluetooth-page"), egui::Sense::click());
        let active = self.enabled == Some(true);
        let color = if active {
            ctx.theme.accent
        } else {
            ctx.theme.text_faint
        };
        let painter = ui.painter();
        let glyph = if active {
            icons::BLUETOOTH
        } else {
            icons::BLUETOOTH_SLASH
        };
        if rect.width() < 150.0 {
            paint::icon(painter, rect.center(), glyph, 28.0, color, active);
        } else {
            paint::icon(
                painter,
                rect.center() - egui::vec2(0.0, 15.0),
                glyph,
                25.0,
                color,
                active,
            );
            painter.text(
                egui::pos2(rect.center().x, rect.center().y + 7.0),
                egui::Align2::CENTER_CENTER,
                "Bluetooth",
                Theme::font(SIZE_BODY),
                ctx.theme.text,
            );
            painter.text(
                egui::pos2(rect.center().x, rect.center().y + 25.0),
                egui::Align2::CENTER_CENTER,
                self.tile_summary(),
                Theme::font(SIZE_LABEL - 1.0),
                ctx.theme.text_dim,
            );
            if rect.height() >= 150.0 {
                painter.text(
                    egui::pos2(rect.center().x, rect.center().y + 43.0),
                    egui::Align2::CENTER_CENTER,
                    format!("{} nearby or paired", self.devices.len()),
                    Theme::font(SIZE_LABEL - 1.0),
                    ctx.theme.text_faint,
                );
            }
        }
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
            ui.put(
                body,
                egui::Label::new("Bluetooth controls are unavailable."),
            );
            return false;
        };
        let tx = worker.tx.clone();
        ui.scope_builder(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)), |ui| {
            ui.set_width(body.width());
            ui.set_height(body.height());
            ui.horizontal(|ui| {
                let mut enabled = self.enabled.unwrap_or(false);
                if widgets::toggle(ui, &mut enabled, theme).changed() {
                    self.enabled = Some(enabled);
                    tx.send(Command::Radio(enabled)).ok();
                }
                ui.label(egui::RichText::new(if enabled { "Bluetooth on" } else { "Bluetooth off" }).font(Theme::font(SIZE_BODY)));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if kit::button(ui, egui::Rect::from_min_size(ui.cursor().min, egui::vec2(98.0, 30.0)), "Refresh", Some(icons::ARROW_CLOCKWISE), false, theme).clicked() {
                        self.loading = true;
                        tx.send(Command::Refresh).ok();
                    }
                });
            });
            if self.enabled != Some(true) {
                ui.label(egui::RichText::new("Turn Bluetooth on to discover nearby devices.").color(theme.text_dim));
                if ui.link("Open Windows Bluetooth settings").clicked() {
                    let _ = wireless::open_bluetooth_settings();
                }
            } else {
                ui.label(egui::RichText::new(if self.loading { "Looking for devices…" } else { "Nearby and paired devices" }).font(Theme::font(SIZE_LABEL)).color(theme.text_dim));
                egui::ScrollArea::vertical().id_salt("bluetooth-devices").max_height(270.0).show(ui, |ui| {
                    for device in self.devices.clone() {
                        self.device_row(ui, &device, theme);
                    }
                });
                if self.devices.is_empty() && !self.loading {
                    ui.label(egui::RichText::new("No Bluetooth devices found. Put a device in pairing mode and refresh.").color(theme.text_dim));
                }
            }
            if let Some(error) = &self.error {
                ui.label(egui::RichText::new(error).color(theme.text_dim));
                if ui.link("Open Windows Bluetooth settings").clicked() {
                    let _ = wireless::open_bluetooth_settings();
                }
            }
        });
        false
    }

    fn next_repaint(&self, _tile_visible: bool) -> Option<std::time::Duration> {
        (self.notice_until > self.last_now).then(|| {
            std::time::Duration::from_secs_f64((self.notice_until - self.last_now).max(0.02))
        })
    }
}
