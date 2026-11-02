//! Background watcher for slow-changing system state: fullscreen apps, the
//! target monitor, theme, animation and transparency preferences, and the
//! wallpaper.
//!
//! Polls once per second on its own thread (each check is a handful of
//! cheap syscalls) and wakes the UI only when something changed, so the UI
//! thread stays fully idle.

use crate::config::MonitorMode;
use crossbeam_channel::Sender;
use nook_platform::monitor::{self, Monitor};
use nook_platform::{Hwnd, fullscreen, system};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SystemState {
    pub fullscreen: bool,
    /// Another topmost window is above the notch (or it lost topmost).
    pub covered: bool,
    pub monitor: Option<Monitor>,
    pub light_theme: bool,
    pub animations: bool,
    pub transparency: bool,
    pub battery: Option<system::Battery>,
    /// Hash of the wallpaper's path; changes when the wallpaper does.
    pub wallpaper: u64,
}

/// Shared knobs the UI can change without restarting the thread.
#[derive(Default)]
pub struct WatcherControl {
    pub monitor_mode: AtomicU8,
    pub monitor_display: AtomicUsize,
    pub stop: AtomicBool,
}

impl WatcherControl {
    pub fn set_target(&self, mode: MonitorMode, display: u8) {
        self.monitor_mode.store(
            match mode {
                MonitorMode::Primary => 0,
                MonitorMode::Cursor => 1,
                MonitorMode::Display => 2,
            },
            Ordering::Relaxed,
        );
        self.monitor_display
            .store(display as usize, Ordering::Relaxed);
    }
}

pub fn sample(own: Hwnd, mode: MonitorMode, display: u8) -> SystemState {
    let monitor = match mode {
        MonitorMode::Primary => monitor::primary(),
        MonitorMode::Cursor => monitor::under_cursor(),
        MonitorMode::Display => monitor::all()
            .get(display as usize)
            .copied()
            .ok_or_else(|| {
                nook_platform::PlatformError::Other("selected display is not connected".into())
            }),
    };
    SystemState {
        fullscreen: fullscreen::foreground_is_fullscreen(own),
        covered: nook_platform::window::is_covered(own),
        monitor: monitor.ok(),
        light_theme: system::apps_use_light_theme(),
        animations: system::animations_enabled(),
        transparency: system::transparency_enabled(),
        battery: system::battery(),
        wallpaper: {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            system::wallpaper_path().hash(&mut h);
            h.finish()
        },
    }
}

pub fn spawn(
    own: Hwnd,
    control: Arc<WatcherControl>,
    tx: Sender<SystemState>,
    ctx: egui::Context,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::thread::Builder::new()
        .name("nook-watcher".into())
        .spawn(move || {
            let mut last: Option<SystemState> = None;
            while !control.stop.load(Ordering::Relaxed) {
                let mode = match control.monitor_mode.load(Ordering::Relaxed) {
                    1 => MonitorMode::Cursor,
                    2 => MonitorMode::Display,
                    _ => MonitorMode::Primary,
                };
                let display = control.monitor_display.load(Ordering::Relaxed) as u8;
                let now = sample(own, mode, display);
                if last != Some(now) {
                    last = Some(now);
                    if tx.send(now).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
                std::thread::sleep(Duration::from_millis(1000));
            }
        })
}
