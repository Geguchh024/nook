//! Nook: a floating notch utility hub for Windows 10/11.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
#[cfg(feature = "debug-overlay")]
mod overlay;
mod settings;
mod tray;
mod watcher;

use anyhow::{Context, Result};
use nook_ui::shell::ShellMetrics;

fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_env("NOOK_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    // Release builds have no console: log to %APPDATA%\Nook\nook.log.
    let file = config::dir().ok().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::File::create(dir.join("nook.log")).ok()
    });
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false);
    match file {
        Some(f) if !cfg!(debug_assertions) => builder.with_writer(std::sync::Mutex::new(f)).init(),
        _ => builder.init(),
    }
}

fn main() -> Result<()> {
    // Checked before logging starts: a second launch must not truncate the
    // running Nook's log.
    let Some(instance) = nook_platform::single_instance::acquire("Nook.SingleInstance")
        .context("single-instance check")?
    else {
        // An install link from the browser goes to the running Nook.
        if let Some(url) = std::env::args().find(|a| a.to_ascii_lowercase().starts_with("nook:"))
            && !nook_modules::plugins::install::forward(&url)
        {
            anyhow::bail!("could not hand {url} to the running Nook");
        }
        return Ok(());
    };
    init_logging();
    let cfg = config::Config::load().context("loading config")?;

    let size = ShellMetrics::default().window_size();
    let viewport = egui::ViewportBuilder::default()
        .with_title("Nook")
        .with_app_id("Nook")
        .with_inner_size(size)
        .with_position([0.0, 0.0])
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_has_shadow(false)
        .with_taskbar(false)
        .with_active(false)
        .with_drag_and_drop(true)
        .with_window_level(egui::WindowLevel::AlwaysOnTop);
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        centered: false,
        persist_window: false,
        ..Default::default()
    };

    eframe::run_native(
        "Nook",
        options,
        Box::new(move |cc| Ok(Box::new(app::NookApp::new(cc, cfg, instance)?))),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {e}"))
}
