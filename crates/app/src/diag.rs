//! Diagnostics for bug reports from machines we cannot touch.
//!
//! `%APPDATA%\Nook\nook.log` holds this session and `nook.prev.log` the one
//! before it (a crash or the OpenGL fallback restart would otherwise wipe the
//! evidence). The log opens with the system facts that explain most display
//! bugs, Rust panics are logged with their location, and native crashes
//! (GPU drivers) are appended by `nook_platform::crash`.

use crate::config::{self, Config};
use nook_platform::{monitor, system};
use std::path::PathBuf;

pub const LOG_FILE: &str = "nook.log";
const PREV_LOG_FILE: &str = "nook.prev.log";

/// Default filter: our info logs, but only warnings from the GPU stack,
/// which is chatty at info and the place most user-only bugs start.
const DEFAULT_FILTER: &str = "info,wgpu_core=warn,wgpu_hal=warn,naga=warn,egui_wgpu=info";

pub fn init() {
    use tracing_subscriber::EnvFilter;
    let filter =
        EnvFilter::try_from_env("NOOK_LOG").unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    // Release builds have no console: log to %APPDATA%\Nook\nook.log.
    let path = log_path();
    let file = path.as_ref().and_then(|path| {
        std::fs::create_dir_all(path.parent()?).ok()?;
        let _ = std::fs::rename(path, path.with_file_name(PREV_LOG_FILE));
        std::fs::File::create(path).ok()
    });
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_thread_names(true);
    match file {
        Some(f) if !cfg!(debug_assertions) => builder.with_writer(std::sync::Mutex::new(f)).init(),
        _ => builder.init(),
    }
    if let Some(path) = path {
        nook_platform::crash::install(path);
    }
    install_panic_hook();
}

static ACTIVE_RENDERER: std::sync::OnceLock<&'static str> = std::sync::OnceLock::new();

/// "DirectX 12" or "OpenGL": what this session draws with, for Settings.
pub fn active_renderer() -> &'static str {
    ACTIVE_RENDERER.get().copied().unwrap_or("?")
}

pub fn set_active_renderer(name: &'static str) {
    let _ = ACTIVE_RENDERER.set(name);
}

pub fn log_path() -> Option<PathBuf> {
    config::dir().ok().map(|d| d.join(LOG_FILE))
}

/// Panics go to the log with thread and location; the default hook only
/// writes to stderr, which a GUI build does not have.
fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        let msg = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "<non-string payload>".to_owned());
        let at = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        tracing::error!(
            "PANIC on thread '{}' at {at}: {msg}\n{}",
            thread.name().unwrap_or("?"),
            std::backtrace::Backtrace::force_capture()
        );
        default(info);
    }));
}

/// One block of facts at startup: version, Windows, display, power and the
/// settings that change how the window is drawn.
pub fn log_environment(cfg: &Config) {
    tracing::info!(
        "Nook {} on {}, exe {}",
        crate::update::CURRENT,
        system::windows_version(),
        std::env::current_exe()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    );
    tracing::info!(
        "system: transparency effects {}, animations {}, battery saver {}, battery {:?}",
        on(system::transparency_enabled()),
        on(system::animations_enabled()),
        on(system::battery_saver()),
        system::battery(),
    );
    for (i, m) in monitor::all().iter().enumerate() {
        tracing::info!(
            "monitor {i}: {}x{} at ({}, {}), {}% scale",
            m.bounds.width(),
            m.bounds.height(),
            m.bounds.left,
            m.bounds.top,
            m.dpi * 100 / 96,
        );
    }
    tracing::info!(
        "config: renderer {:?}, material {:?}, glass {}%, theme {:?}, accent {:?}, placement {:?}, on top {}, monitor {:?}",
        cfg.renderer,
        cfg.material,
        cfg.glass_opacity,
        cfg.theme,
        cfg.accent,
        cfg.placement,
        cfg.on_top,
        cfg.monitor,
    );
}

fn on(b: bool) -> &'static str {
    if b { "on" } else { "off" }
}
