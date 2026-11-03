//! Experimental Airplane tile: turns available Wi-Fi/Bluetooth radios off
//! and restores the previous state when switched back on.

use crossbeam_channel::{Receiver, Sender, unbounded};
use nook_platform::controls::{self, RadioKind};
use nook_ui::module::{Module, RadioRestore, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme};
use nook_ui::{icons, widgets};
use std::thread::JoinHandle;

enum Command {
    Refresh,
    Set(bool, Option<RadioRestore>),
    Stop,
}

enum Event {
    Snapshot(Option<bool>, Option<bool>),
    Action(Option<bool>, Result<ActionState, String>),
}

struct ActionState {
    wifi: Option<bool>,
    bluetooth: Option<bool>,
    restore: Option<RadioRestore>,
}

struct Worker {
    tx: Sender<Command>,
    rx: Receiver<Event>,
    join: Option<JoinHandle<()>>,
}

impl Worker {
    fn spawn(ctx: egui::Context) -> std::io::Result<Self> {
        let (tx, commands) = unbounded();
        let (events, rx) = unbounded();
        let join = std::thread::Builder::new()
            .name("nook-airplane".into())
            .spawn(move || {
                if let Err(error) = controls::initialize_worker() {
                    let _ = events.send(Event::Action(None, Err(error.to_string())));
                    return;
                }
                while let Ok(command) = commands.recv() {
                    match command {
                        Command::Stop => break,
                        Command::Refresh => {
                            let snapshot = (
                                controls::radio_enabled(RadioKind::Wifi),
                                controls::radio_enabled(RadioKind::Bluetooth),
                            );
                            if events
                                .send(Event::Snapshot(snapshot.0, snapshot.1))
                                .is_err()
                            {
                                break;
                            }
                        }
                        Command::Set(enabled, restore) => {
                            let result = set_airplane(enabled, restore);
                            if events.send(Event::Action(Some(enabled), result)).is_err() {
                                break;
                            }
                        }
                    }
                    ctx.request_repaint();
                }
                controls::uninitialize_worker();
            })?;
        Ok(Self {
            tx,
            rx,
            join: Some(join),
        })
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

fn set_airplane(enabled: bool, restore: Option<RadioRestore>) -> Result<ActionState, String> {
    let (wifi, bluetooth) = (
        controls::radio_enabled(RadioKind::Wifi),
        controls::radio_enabled(RadioKind::Bluetooth),
    );
    if wifi.is_none() && bluetooth.is_none() {
        return Err("No available radios were found".into());
    }

    if enabled {
        let saved = RadioRestore {
            wifi: wifi == Some(true),
            bluetooth: bluetooth == Some(true),
        };
        if wifi == Some(true)
            && !controls::set_radio(RadioKind::Wifi, false).map_err(|e| e.to_string())?
        {
            return Err("Windows did not allow Wi-Fi to be turned off".into());
        }
        if bluetooth == Some(true)
            && !controls::set_radio(RadioKind::Bluetooth, false).map_err(|e| e.to_string())?
        {
            return Err("Windows did not allow Bluetooth to be turned off".into());
        }
        Ok(ActionState {
            wifi: controls::radio_enabled(RadioKind::Wifi),
            bluetooth: controls::radio_enabled(RadioKind::Bluetooth),
            restore: Some(saved),
        })
    } else {
        let saved = restore.unwrap_or(RadioRestore {
            wifi: true,
            bluetooth: true,
        });
        if saved.wifi
            && wifi.is_some()
            && !controls::set_radio(RadioKind::Wifi, true).map_err(|e| e.to_string())?
        {
            return Err("Windows did not allow Wi-Fi to be restored".into());
        }
        if saved.bluetooth
            && bluetooth.is_some()
            && !controls::set_radio(RadioKind::Bluetooth, true).map_err(|e| e.to_string())?
        {
            return Err("Windows did not allow Bluetooth to be restored".into());
        }
        Ok(ActionState {
            wifi: controls::radio_enabled(RadioKind::Wifi),
            bluetooth: controls::radio_enabled(RadioKind::Bluetooth),
            restore: None,
        })
    }
}

fn all_radios_off(wifi: Option<bool>, bluetooth: Option<bool>) -> Option<bool> {
    (wifi.is_some() || bluetooth.is_some())
        .then_some(!wifi.unwrap_or(false) && !bluetooth.unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::all_radios_off;

    #[test]
    fn airplane_state_uses_only_radios_available_on_the_device() {
        assert_eq!(all_radios_off(Some(false), Some(false)), Some(true));
        assert_eq!(all_radios_off(Some(false), Some(true)), Some(false));
        assert_eq!(all_radios_off(Some(false), None), Some(true));
        assert_eq!(all_radios_off(None, None), None);
    }
}

#[derive(Default)]
pub struct Airplane {
    worker: Option<Worker>,
    wifi: Option<bool>,
    bluetooth: Option<bool>,
    restore: Option<RadioRestore>,
    /// State shown immediately while Windows updates the radio adapters.
    pending: Option<bool>,
    restore_update: Option<Option<RadioRestore>>,
    page_request: bool,
    refresh_on_page: bool,
    error: Option<String>,
}

impl Airplane {
    fn send(&self, command: Command) {
        if let Some(worker) = &self.worker {
            let _ = worker.tx.send(command);
        }
    }
}

impl Module for Airplane {
    fn id(&self) -> &'static str {
        "airplane"
    }
    fn name(&self) -> &str {
        "Airplane"
    }
    fn icon(&self) -> &'static str {
        icons::AIRPLANE_TILT
    }
    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn is_active(&self) -> bool {
        self.pending
            .or_else(|| all_radios_off(self.wifi, self.bluetooth))
            == Some(true)
    }

    fn uses_active_tile_fill(&self) -> bool {
        false
    }

    fn set_radio_restore_state(&mut self, state: Option<RadioRestore>) {
        self.restore = state;
    }

    fn take_radio_restore_update(&mut self) -> Option<Option<RadioRestore>> {
        self.restore_update.take()
    }

    fn attach(&mut self, ctx: &egui::Context) {
        match Worker::spawn(ctx.clone()) {
            Ok(worker) => self.worker = Some(worker),
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    fn on_tick(&mut self) {
        while let Some(event) = self
            .worker
            .as_ref()
            .and_then(|worker| worker.rx.try_recv().ok())
        {
            match event {
                Event::Snapshot(wifi, bluetooth) => {
                    self.wifi = wifi;
                    self.bluetooth = bluetooth;
                }
                Event::Action(target, Ok(state)) => {
                    self.wifi = state.wifi;
                    self.bluetooth = state.bluetooth;
                    self.restore = state.restore;
                    self.restore_update = Some(state.restore);
                    if target.is_none() || self.pending == target {
                        self.pending = None;
                    }
                    self.error = None;
                }
                Event::Action(target, Err(error)) => {
                    if target.is_none() || self.pending == target {
                        self.pending = None;
                    }
                    self.error = Some(error);
                }
            }
        }
    }

    fn ui_tile(&mut self, ui: &mut egui::Ui, ctx: &mut TileCtx<'_>) {
        let rect = ui.max_rect();
        let response = ui.interact(rect, ui.id().with("airplane-page"), egui::Sense::click());
        let enabled = self
            .pending
            .or_else(|| all_radios_off(self.wifi, self.bluetooth));
        let summary = match enabled {
            Some(true) => "Radios off",
            Some(false) => "Airplane off",
            None => "Not available",
        };
        widgets::wireless_tile(
            ui,
            self.icon(),
            "Airplane",
            summary,
            enabled == Some(true),
            None,
            ctx,
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
        if self.refresh_on_page {
            self.send(Command::Refresh);
            self.refresh_on_page = false;
        }
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.set_width(body.width());
                ui.horizontal_wrapped(|ui| {
                    let mut enabled = self
                        .pending
                        .or_else(|| all_radios_off(self.wifi, self.bluetooth))
                        .unwrap_or(false);
                    if widgets::toggle(ui, &mut enabled, theme).changed() {
                        self.pending = Some(enabled);
                        self.send(Command::Set(enabled, self.restore));
                    }
                    ui.label(
                        egui::RichText::new(if all_radios_off(self.wifi, self.bluetooth).is_none() {
                            "No radios available"
                        } else if enabled {
                            "Airplane mode on · available radios are off"
                        } else {
                            "Airplane mode off"
                        })
                        .font(Theme::font(SIZE_BODY)),
                    );
                });
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("This turns off available Wi-Fi and Bluetooth radios, then restores the radios that were on when switched off. Windows’ system airplane indicator may not change.")
                            .font(Theme::font(SIZE_LABEL))
                            .color(theme.text_dim),
                    )
                    .wrap(),
                );
                if let Some(error) = &self.error {
                    ui.add_space(8.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(error)
                                .font(Theme::font(SIZE_LABEL))
                                .color(theme.text_dim),
                        )
                        .wrap(),
                    );
                }
            },
        );
        false
    }
}
