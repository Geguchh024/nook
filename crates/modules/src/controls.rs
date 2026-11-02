//! Quick system controls. All Windows work runs on one worker and returns
//! snapshots through a channel drained by `on_tick`.

use crossbeam_channel::{Receiver, Sender, unbounded};
use nook_platform::controls::{self, Snapshot};
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme};
use nook_ui::{icons, paint, widgets};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

enum Command {
    Refresh,
    Volume(f32),
    Brightness(f32),
    BrightnessPreview(f32),
    Mute(bool),
    MicrophoneMute(bool),
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
                let mut deferred = VecDeque::new();
                loop {
                    let command = match deferred.pop_front() {
                        Some(command) => command,
                        None => match commands.recv() {
                            Ok(command) => command,
                            Err(_) => break,
                        },
                    };
                    // Slider samples are latest-wins while adjacent in the
                    // queue; stale positions should never delay the thumb.
                    let command = match command {
                        Command::Volume(mut value) => {
                            while let Ok(next) = commands.try_recv() {
                                match next {
                                    Command::Volume(latest) => value = latest,
                                    other => {
                                        deferred.push_back(other);
                                        break;
                                    }
                                }
                            }
                            Command::Volume(value)
                        }
                        Command::Brightness(mut value) => {
                            while let Ok(next) = commands.try_recv() {
                                match next {
                                    Command::Brightness(latest) => value = latest,
                                    other => {
                                        deferred.push_back(other);
                                        break;
                                    }
                                }
                            }
                            Command::Brightness(value)
                        }
                        other => other,
                    };
                    let result = match command {
                        Command::Stop => break,
                        Command::Volume(value) => controls::set_volume(value).map(|()| None),
                        Command::Brightness(value) => controls::set_brightness(value).map(|_| None),
                        Command::BrightnessPreview(value) => {
                            controls::set_brightness_preview(value).map(|()| None)
                        }
                        command => {
                            let action = match command {
                                Command::Refresh => Ok(()),
                                Command::Mute(muted) => controls::set_muted(muted),
                                Command::MicrophoneMute(muted) => {
                                    controls::set_microphone_muted(muted)
                                }
                                Command::Notifications(muted) => {
                                    controls::set_notifications_muted(muted)
                                }
                                Command::Stop => continue,
                                Command::Volume(_) => continue,
                                Command::Brightness(_) => continue,
                                Command::BrightnessPreview(_) => continue,
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
    brightness_due: Option<Instant>,
    brightness_pending: Option<f32>,
    volume_display: Option<f32>,
    brightness_display: Option<f32>,
    volume_overridden: bool,
    brightness_overridden: bool,
    audio_page: bool,
    audio_textures: HashMap<String, egui::TextureHandle>,
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
            brightness_due: None,
            brightness_pending: None,
            volume_display: None,
            brightness_display: None,
            volume_overridden: false,
            brightness_overridden: false,
            audio_page: false,
            audio_textures: HashMap::new(),
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

    fn page_size(&self) -> Option<egui::Vec2> {
        Some(egui::vec2(600.0, 382.0))
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
        // Brightness previews target DDC/CI only. Laptop-panel WMI writes are
        // sent once on release, avoiding slow stale updates during a drag.
        if self
            .brightness_due
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.brightness_due = None;
            if let Some(value) = self.brightness_pending {
                self.send(Command::BrightnessPreview(value));
            }
        }
        if let Some(worker) = &self.worker {
            while let Ok(result) = worker.rx.try_recv() {
                match result {
                    Ok(Some(state)) => {
                        let snapshot_volume = state.volume;
                        let snapshot_brightness = state.brightness;
                        self.state = state;
                        if !self.volume_overridden {
                            self.volume_display = snapshot_volume;
                        }
                        if !self.brightness_overridden {
                            self.brightness_display = snapshot_brightness;
                        }
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
        let wide = rect.width() >= 150.0;
        if wide {
            paint::icon(
                painter,
                center - egui::vec2(0.0, 12.0),
                self.icon(),
                26.0,
                ctx.theme.text,
                false,
            );
            painter.text(
                egui::pos2(center.x, center.y + 16.0),
                egui::Align2::CENTER_CENTER,
                "Quick Controls",
                Theme::font(SIZE_BODY),
                ctx.theme.text,
            );
            if rect.height() >= 150.0 {
                let volume = self
                    .volume_display
                    .or(self.state.volume)
                    .map(|value| format!("Volume {}%", (value * 100.0).round() as u32));
                let brightness = self
                    .brightness_display
                    .or(self.state.brightness)
                    .map(|value| format!("Brightness {}%", (value * 100.0).round() as u32));
                let summary = match (volume, brightness) {
                    (Some(volume), Some(brightness)) => format!("{volume}  ·  {brightness}"),
                    (Some(value), None) | (None, Some(value)) => value,
                    (None, None) => "System sliders".to_owned(),
                };
                painter.text(
                    egui::pos2(center.x, center.y + 33.0),
                    egui::Align2::CENTER_CENTER,
                    summary,
                    Theme::font(SIZE_LABEL - 1.0),
                    ctx.theme.text_dim,
                );
            }
        } else {
            paint::icon(painter, center, self.icon(), 28.0, ctx.theme.text, false);
        }
        if clicked.clicked() {
            self.page_request = true;
            self.refresh_on_page = true;
            self.volume_display = None;
            self.brightness_display = None;
            self.volume_overridden = false;
            self.brightness_overridden = false;
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
        if self.audio_page {
            let ctx = ui.ctx().clone();
            for app in &self.state.audio_apps {
                let key = app.icon_path.as_deref().unwrap_or(&app.name).to_owned();
                if !self.audio_textures.contains_key(&key)
                    && let Some(icon) = &app.icon
                {
                    self.audio_textures.insert(
                        key.clone(),
                        ctx.load_texture(
                            format!("nook-audio-app-{key}"),
                            egui::ColorImage::from_rgba_premultiplied(
                                [icon.width as usize, icon.height as usize],
                                &icon.rgba,
                            ),
                            egui::TextureOptions::LINEAR,
                        ),
                    );
                }
            }
            ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                ui.set_width(body.width());
                ui.horizontal(|ui| {
                    if ui.button("←  Sound").clicked() {
                        self.audio_page = false;
                    }
                    ui.heading("Output and apps");
                });
                egui::ScrollArea::vertical()
                    .id_salt("controls-audio-routing")
                    .max_height((body.height() - 44.0).max(100.0))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new("Output devices")
                                .font(Theme::font(SIZE_LABEL))
                                .color(theme.text_dim),
                        );
                        if self.state.outputs.is_empty() {
                            ui.label("No active output devices were found.");
                        } else {
                            for output in &self.state.outputs {
                                ui.horizontal(|ui| {
                                    paint::icon(
                                        ui.painter(),
                                        ui.cursor().min + egui::vec2(12.0, 12.0),
                                        icons::SPEAKER_HIGH,
                                        18.0,
                                        if output.is_default { theme.accent } else { theme.text_dim },
                                        output.is_default,
                                    );
                                    ui.add_space(30.0);
                                    ui.label(&output.name);
                                    if output.is_default {
                                        ui.label(egui::RichText::new("Default").color(theme.accent));
                                    }
                                });
                            }
                        }
                        if ui.button("Choose output in Windows Settings").clicked()
                            && let Err(error) = nook_platform::shell::open("ms-settings:sound")
                        {
                            tracing::warn!("opening Windows sound settings: {error}");
                        }
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new("Apps currently using audio")
                                .font(Theme::font(SIZE_LABEL))
                                .color(theme.text_dim),
                        );
                        if self.state.audio_apps.is_empty() {
                            ui.label("No active audio sessions were found.");
                        } else {
                            for app in &self.state.audio_apps {
                                ui.horizontal(|ui| {
                                    let key = app.icon_path.as_deref().unwrap_or(&app.name);
                                    if let Some(icon) = self.audio_textures.get(key) {
                                        ui.add(
                                            egui::Image::new(icon)
                                                .fit_to_exact_size(egui::vec2(28.0, 28.0)),
                                        );
                                    } else {
                                        paint::icon(
                                            ui.painter(),
                                            ui.cursor().min + egui::vec2(12.0, 12.0),
                                            icons::APP_WINDOW,
                                            20.0,
                                            theme.text_dim,
                                            false,
                                        );
                                        ui.add_space(30.0);
                                    }
                                    let label = ui.label(&app.name);
                                    if let Some(path) = app.icon_path.as_deref() {
                                        label.on_hover_text(path);
                                    }
                                    if ui.button("Choose speaker").clicked()
                                        && let Err(error) =
                                            nook_platform::shell::open("ms-settings:apps-volume")
                                    {
                                        tracing::warn!("opening Windows app volume settings: {error}");
                                    }
                                });
                            }
                        }
                        ui.label(
                            egui::RichText::new(
                                "Windows applies output choices per device and app in its sound settings.",
                            )
                            .small()
                            .color(theme.text_faint),
                        );
                    });
            });
            return false;
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            ui.set_width(body.width());
            ui.set_height(body.height());
            ui.spacing_mut().item_spacing.y = 14.0;
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    Self::row_label(ui, "System volume", theme);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let muted = self.state.muted == Some(true);
                        let glyph = if muted {
                            icons::SPEAKER_SLASH
                        } else {
                            icons::SPEAKER_HIGH
                        };
                        if widgets::icon_button(ui, glyph, 28.0, muted, theme).clicked() {
                            let _ = tx.send(Command::Mute(!muted));
                        }
                        if let Some(value) = self.volume_display.or(self.state.volume) {
                            ui.label(
                                egui::RichText::new(format!("{}%", (value * 100.0).round() as u32))
                                    .color(theme.text_dim),
                            );
                        }
                    });
                });
                if let Some(mut volume) = self.volume_display.or(self.state.volume) {
                    let available = ui.available_width().max(120.0);
                    let response = widgets::slider(ui, &mut volume, available, 0, theme);
                    if response.changed() {
                        nook_ui::sound::scrub(ui.ctx(), response.id);
                        self.volume_display = Some(volume);
                        self.volume_overridden = true;
                        self.volume_pending = Some(volume);
                        self.volume_due = Some(Instant::now() + Duration::from_millis(50));
                        ui.ctx().request_repaint_after(Duration::from_millis(55));
                    }
                    if response.drag_stopped() {
                        self.volume_due = None;
                        if let Some(value) = self.volume_pending.take() {
                            let _ = tx.send(Command::Volume(value));
                        }
                    }
                } else {
                    ui.label("System volume is unavailable");
                }

                if ui.button("Output and app speakers  ›").clicked() {
                    self.audio_page = true;
                    self.send(Command::Refresh);
                }

                ui.horizontal(|ui| {
                    Self::row_label(ui, "Display brightness", theme);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(value) = self.brightness_display.or(self.state.brightness) {
                            ui.label(
                                egui::RichText::new(format!("{}%", (value * 100.0).round() as u32))
                                    .color(theme.text_dim),
                            );
                        }
                    });
                });
                if let Some(mut brightness) = self.brightness_display.or(self.state.brightness) {
                    let available = ui.available_width().max(120.0);
                    let response = widgets::slider(ui, &mut brightness, available, 0, theme);
                    if response.changed() {
                        nook_ui::sound::scrub(ui.ctx(), response.id);
                        self.brightness_display = Some(brightness);
                        self.brightness_overridden = true;
                        self.brightness_pending = Some(brightness);
                        self.brightness_due = Some(Instant::now() + Duration::from_millis(55));
                        ui.ctx().request_repaint_after(Duration::from_millis(60));
                    }
                    if response.drag_stopped() || response.clicked() {
                        self.brightness_due = None;
                        if let Some(value) = self.brightness_pending.take() {
                            let _ = tx.send(Command::Brightness(value));
                        }
                    }
                } else {
                    ui.label("Brightness is unavailable on this display");
                }

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

                ui.horizontal(|ui| {
                    Self::row_label(ui, "Microphone mute", theme);
                    if let Some(mut muted) = self.state.microphone_muted {
                        if widgets::toggle(ui, &mut muted, theme).changed() {
                            let _ = tx.send(Command::MicrophoneMute(muted));
                        }
                    } else {
                        ui.label(
                            egui::RichText::new("Unavailable")
                                .font(Theme::font(SIZE_LABEL))
                                .color(theme.text_faint),
                        );
                    }
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
