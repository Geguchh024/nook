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
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

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
    pub monitor_displays: AtomicU64,
    pub stop: AtomicBool,
}

impl WatcherControl {
    pub fn set_target(&self, mode: MonitorMode, display: u8, displays: &[u8]) {
        self.monitor_mode.store(
            match mode {
                MonitorMode::Primary => 0,
                MonitorMode::Cursor => 1,
                MonitorMode::Display => 2,
                MonitorMode::SelectedDisplays => 3,
            },
            Ordering::Relaxed,
        );
        self.monitor_display
            .store(display as usize, Ordering::Relaxed);
        let mask = displays.iter().fold(0_u64, |mask, index| {
            if *index < 64 {
                mask | (1_u64 << index)
            } else {
                mask
            }
        });
        self.monitor_displays.store(mask, Ordering::Relaxed);
    }
}

pub fn sample(own: Hwnd, mode: MonitorMode, display: u8, displays: &[u8]) -> SystemState {
    let (fullscreen, covered, monitor) = sample_fast(own, mode, display, displays);
    let slow = sample_slow();
    SystemState {
        fullscreen,
        covered,
        monitor,
        light_theme: slow.light_theme,
        animations: slow.animations,
        transparency: slow.transparency,
        battery: slow.battery,
        wallpaper: slow.wallpaper,
    }
}

fn sample_fast(
    own: Hwnd,
    mode: MonitorMode,
    display: u8,
    displays: &[u8],
) -> (bool, bool, Option<Monitor>) {
    let monitor = match mode {
        MonitorMode::Primary => monitor::primary(),
        MonitorMode::Cursor => monitor::under_cursor(),
        MonitorMode::Display => monitor::all()
            .get(display as usize)
            .copied()
            .ok_or_else(|| {
                nook_platform::PlatformError::Other("selected display is not connected".into())
            }),
        MonitorMode::SelectedDisplays => {
            let all = monitor::all();
            let cursor = monitor::under_cursor().ok();
            choose_selected_display(&all, cursor, displays)
                .ok_or_else(|| nook_platform::PlatformError::Other("no connected displays".into()))
        }
    };
    (
        fullscreen::foreground_is_fullscreen(own),
        nook_platform::window::is_covered(own),
        monitor.ok(),
    )
}

fn choose_selected_display(
    all: &[Monitor],
    cursor: Option<Monitor>,
    selected_indices: &[u8],
) -> Option<Monitor> {
    let selected = all
        .iter()
        .enumerate()
        .filter(|(index, _)| selected_indices.contains(&(*index as u8)))
        .map(|(_, monitor)| *monitor)
        .collect::<Vec<_>>();
    cursor
        .filter(|at| selected.iter().any(|monitor| monitor.id == at.id))
        .or_else(|| selected.first().copied())
        .or_else(|| all.first().copied())
}

#[derive(Clone, Copy)]
struct SlowState {
    light_theme: bool,
    animations: bool,
    transparency: bool,
    battery: Option<system::Battery>,
    wallpaper: u64,
}

fn sample_slow() -> SlowState {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    system::wallpaper_path().hash(&mut h);
    SlowState {
        light_theme: system::apps_use_light_theme(),
        animations: system::animations_enabled(),
        transparency: system::transparency_enabled(),
        battery: system::battery(),
        wallpaper: h.finish(),
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
            let mut slow = sample_slow();
            let mut slow_sampled = Instant::now();
            while !control.stop.load(Ordering::Relaxed) {
                let mode = match control.monitor_mode.load(Ordering::Relaxed) {
                    1 => MonitorMode::Cursor,
                    2 => MonitorMode::Display,
                    3 => MonitorMode::SelectedDisplays,
                    _ => MonitorMode::Primary,
                };
                let display = control.monitor_display.load(Ordering::Relaxed) as u8;
                let mask = control.monitor_displays.load(Ordering::Relaxed);
                let displays = (0..64)
                    .filter(|index| mask & (1_u64 << index) != 0)
                    .map(|index| index as u8)
                    .collect::<Vec<_>>();
                let (fullscreen, covered, monitor) = sample_fast(own, mode, display, &displays);
                if slow_sampled.elapsed() >= Duration::from_secs(10) {
                    slow = sample_slow();
                    slow_sampled = Instant::now();
                }
                let now = SystemState {
                    fullscreen,
                    covered,
                    monitor,
                    light_theme: slow.light_theme,
                    animations: slow.animations,
                    transparency: slow.transparency,
                    battery: slow.battery,
                    wallpaper: slow.wallpaper,
                };
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

#[cfg(test)]
mod tests {
    use super::*;
    use nook_platform::monitor::Rect;

    fn monitor(id: isize, left: i32) -> Monitor {
        let bounds = Rect {
            left,
            top: 0,
            right: left + 1920,
            bottom: 1080,
        };
        Monitor {
            id,
            bounds,
            work: bounds,
            dpi: 96,
        }
    }

    #[test]
    fn selection_tracks_cursor_only_on_chosen_displays() {
        let displays = [monitor(1, 0), monitor(2, 1920), monitor(3, 3840)];
        assert_eq!(
            choose_selected_display(&displays, Some(displays[1]), &[0, 1]).map(|m| m.id),
            Some(2)
        );
        assert_eq!(
            choose_selected_display(&displays, Some(displays[2]), &[0, 1]).map(|m| m.id),
            Some(1)
        );
    }

    #[test]
    fn selection_falls_back_to_first_chosen_display() {
        let displays = [monitor(1, 0), monitor(2, 1920)];
        assert_eq!(
            choose_selected_display(&displays, None, &[1]).map(|m| m.id),
            Some(2)
        );
        assert_eq!(
            choose_selected_display(&displays, None, &[]).map(|m| m.id),
            Some(1)
        );
    }
}
