//! Quick system controls. All Windows work runs on one worker and returns
//! snapshots through a channel drained by `on_tick`.

use crossbeam_channel::{Receiver, Sender, unbounded};
use nook_platform::controls::{self, RadioKind, Snapshot};
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme};
use nook_ui::{icons, paint, widgets};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

enum Command {
    Refresh,
    Radio(RadioKind, bool),
    Volume(f32),
    Brightness(f32),
    Mute(bool),
    Notifications(bool),
    Stop,
}

struct Worker {
    tx: Sender<Command>,
    rx: Receiver<Result<Option<Snapshot>, String>>,
    join: Option<JoinHandle<()>>,
}

impl Worker {
    fn spawn(ctx: egui::Context) -> std::io::Result<Self> {
        let (tx, commands) = unbounded();
        let (results, rx) = unbounded();
        let join = std::thread::Builder::new()
            .name("nook-controls".into())
            .spawn(move || {
                if let Err(error) = controls::initialize_worker() {
                    let _ = results.send(Err(error.to_string()));
                    return;
                }
                while let Ok(command) = commands.recv() {
                    let result = match command {
                        Command::Stop => break,
                        Command::Volume(value) => controls::set_volume(value).map(|()| None),
                        command => {
                            let action = match command {
                                Command::Refresh => Ok(()),
                                Command::Radio(kind, on) => {
                                    controls::set_radio(kind, on).map(|_| ())
                                }
                                Command::Brightness(value) => {
                                    controls::set_brightness(value).map(|_| ())
                                }
                                Command::Mute(muted) => controls::set_muted(muted),
                                Command::Notifications(muted) => {
                                    controls::set_notifications_muted(muted)
                                }
                                Command::Volume(_) | Command::Stop => continue,
                            };
                            action.map(|()| Some(controls::snapshot()))
                        }
                    };
                    if results.send(result.map_err(|e| e.to_string())).is_err() {
                        break;
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

    fn send(&self, command: Command) {
        if self.tx.send(command).is_err() {
            tracing::warn!("quick controls worker stopped");
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

pub struct Controls {
    worker: Option<Worker>,
    state: Snapshot,
    page_request: bool,
    refresh_on_page: bool,
    volume_due: Option<Instant>,
    volume_pending: Option<f32>,
    volume_display: Option<f32>,
    brightness_display: Option<f32>,
}

impl Default for Controls {
    fn default() -> Self {
        Self {
            worker: None,
            state: Snapshot::default(),
            page_request: false,
            refresh_on_page: true,
            volume_due: None,
            volume_pending: None,
            volume_display: None,
            brightness_display: None,
        }
    }
}

impl Controls {
    fn send(&self, command: Command) {
        if let Some(worker) = &self.worker {
            worker.send(command);
        }
    }

    fn row_label(ui: &mut egui::Ui, label: &str, theme: &Theme) {
        ui.label(
            egui::RichText::new(label)
                .font(Theme::font(SIZE_LABEL))
                .color(theme.text_dim),
        );
    }

    fn radio_row(
        ui: &mut egui::Ui,
        label: &str,
        kind: RadioKind,
        state: Option<bool>,
        theme: &Theme,
        tx: &Sender<Command>,
    ) {
        if let Some(mut enabled) = state {
            ui.horizontal(|ui| {
                Self::row_label(ui, label, theme);
                let response = widgets::toggle(ui, &mut enabled, theme);
                if response.changed() {
                    let _ = tx.send(Command::Radio(kind, enabled));
                }
            });
        }
    }
}

impl Module for Controls {
    fn id(&self) -> &'static str {
        "controls"
    }
    fn name(&self) -> &str {
        "Controls"
    }
    fn icon(&self) -> &'static str {
        icons::SLIDERS_HORIZONTAL
    }
    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn attach(&mut self, ctx: &egui::Context) {
        match Worker::spawn(ctx.clone()) {
            Ok(worker) => self.worker = Some(worker),
            Err(error) => tracing::warn!("quick controls worker: {error}"),
        }
    }

    fn on_tick(&mut self) {
        if self
            .volume_due
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.volume_due = None;
            if let Some(value) = self.volume_pending.take() {
                self.send(Command::Volume(value));
            }
        }
        if let Some(worker) = &self.worker {
            while let Ok(result) = worker.rx.try_recv() {
                match result {
                    Ok(Some(state)) => {
                        self.state = state;
                        if self.volume_pending.is_none() {
                            self.volume_display = state.volume;
                        }
                        self.brightness_display = state.brightness;
                    }
                    Ok(None) => {}
                    Err(error) => tracing::warn!("quick controls: {error}"),
                }
            }
        }
    }

    fn ui_tile(&mut self, ui: &mut egui::Ui, ctx: &mut TileCtx<'_>) {
        let rect = ui.max_rect();
        let clicked = ui.interact(rect, ui.id().with("controls-open"), egui::Sense::click());
        let painter = ui.painter();
        let center = rect.center();
        paint::icon(
            painter,
            center - egui::vec2(0.0, 13.0),
            self.icon(),
            26.0,
            ctx.theme.text,
            false,
        );
        painter.text(
            egui::pos2(center.x, center.y + 15.0),
            egui::Align2::CENTER_CENTER,
            "Quick Controls",
            Theme::font(SIZE_BODY),
            ctx.theme.text,
        );
        if clicked.clicked() {
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
        let Some(worker) = &self.worker else {
            ui.put(body, egui::Label::new("System controls are unavailable."));
            return false;
        };
        let tx = worker.tx.clone();
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            ui.set_width(body.width());
            ui.set_height(body.height());
            ui.vertical(|ui| {
                let columns = if body.width() >= 600.0 { 2 } else { 1 };
                egui::Grid::new("controls-grid")
                    .num_columns(columns)
                    .spacing(egui::vec2(24.0, 16.0))
                    .show(ui, |ui| {
                        Self::radio_row(ui, "Wi-Fi", RadioKind::Wifi, self.state.wifi, theme, &tx);
                        Self::radio_row(
                            ui,
                            "Bluetooth",
                            RadioKind::Bluetooth,
                            self.state.bluetooth,
                            theme,
                            &tx,
                        );
                        ui.end_row();

                        ui.horizontal(|ui| {
                            Self::row_label(ui, "Mute notifications", theme);
                            let mut muted = self.state.notifications_muted;
                            if widgets::toggle(ui, &mut muted, theme)
                                .on_hover_text(
                                    "Experimental: changes the Windows toast notification setting",
                                )
                                .changed()
                            {
                                let _ = tx.send(Command::Notifications(muted));
                                self.state.notifications_muted = muted;
                            }
                        });
                        ui.end_row();

                        if let Some(mut volume) = self.volume_display.or(self.state.volume) {
                            ui.horizontal(|ui| {
                                Self::row_label(ui, "System volume", theme);
                                let available = (ui.available_width() - 20.0).clamp(72.0, 320.0);
                                let response =
                                    widgets::slider(ui, &mut volume, available, 0, theme);
                                if response.changed() {
                                    nook_ui::sound::scrub(ui.ctx(), response.id);
                                    self.volume_display = Some(volume);
                                    self.volume_pending = Some(volume);
                                    self.volume_due =
                                        Some(Instant::now() + Duration::from_millis(50));
                                    ui.ctx().request_repaint_after(Duration::from_millis(55));
                                }
                                if response.drag_stopped() {
                                    self.volume_due = None;
                                    if let Some(value) = self.volume_pending.take() {
                                        let _ = tx.send(Command::Volume(value));
                                    }
                                }
                                if self.state.muted == Some(true) {
                                    let button = widgets::icon_button(
                                        ui,
                                        icons::SPEAKER_SLASH,
                                        26.0,
                                        true,
                                        theme,
                                    );
                                    if button.clicked() {
                                        let _ = tx.send(Command::Mute(false));
                                    }
                                } else {
                                    let button = widgets::icon_button(
                                        ui,
                                        icons::SPEAKER_HIGH,
                                        26.0,
                                        false,
                                        theme,
                                    );
                                    if button.clicked() {
                                        let _ = tx.send(Command::Mute(true));
                                    }
                                }
                            });
                        } else {
                            ui.label("System volume is unavailable");
                        }
                        if columns == 2 {
                            ui.end_row();
                        }
                        if let Some(mut brightness) =
                            self.brightness_display.or(self.state.brightness)
                        {
                            ui.horizontal(|ui| {
                                Self::row_label(ui, "Display brightness", theme);
                                let available = (ui.available_width() - 20.0).clamp(72.0, 320.0);
                                let response =
                                    widgets::slider(ui, &mut brightness, available, 0, theme);
                                if response.changed() {
                                    nook_ui::sound::scrub(ui.ctx(), response.id);
                                    self.brightness_display = Some(brightness);
                                }
                                if response.drag_stopped() || response.clicked() {
                                    let _ = tx.send(Command::Brightness(brightness));
                                }
                            });
                        } else {
                            ui.label("This display does not expose brightness control");
                        }
                        ui.end_row();
                    });
            });
        });
        false
    }

    fn shutdown(&mut self) {
        self.worker = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_tile_defaults_to_small() {
        assert_eq!(Controls::default().size_hint(), TileSize::Small);
    }
}
