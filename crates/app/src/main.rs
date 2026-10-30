//! Nook: a floating notch utility hub for Windows 10/11.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod diag;
#[cfg(feature = "debug-overlay")]
mod overlay;
mod settings;
mod stats;
mod tray;
mod update;
mod watcher;

use anyhow::{Context, Result};
use nook_ui::shell::ShellMetrics;

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
    diag::init();
    // A GUI build has no console: a startup error is only seen in the log.
    start(instance).inspect_err(|e| tracing::error!("Nook stopped: {e:#}"))
}

fn start(instance: nook_platform::single_instance::InstanceGuard) -> Result<()> {
    let cfg = config::Config::load().context("loading config")?;
    diag::log_environment(&cfg);

    let forced_gl = std::env::args().any(|a| a == GL_FLAG);
    let renderer = if forced_gl || cfg.renderer == config::Renderer::OpenGl {
        eframe::Renderer::Glow
    } else {
        eframe::Renderer::Wgpu
    };
    let auto = cfg.renderer == config::Renderer::Auto;
    tracing::info!("renderer: {renderer:?} (setting {:?})", cfg.renderer);
    match run(cfg, instance, renderer) {
        // DirectX could not start: try again on OpenGL in a fresh process (an
        // event loop can only be made once). The single-instance guard was
        // dropped with the app, so the new process can take it.
        Err(e) if renderer == eframe::Renderer::Wgpu && auto => {
            tracing::warn!("DirectX renderer failed, restarting on OpenGL: {e}");
            let exe = std::env::current_exe().context("current exe")?;
            std::process::Command::new(exe)
                .arg(GL_FLAG)
                .spawn()
                .context("restarting on OpenGL")?;
            Ok(())
        }
        other => other,
    }
}

/// Starts Nook on OpenGL whatever the config says (set by the fallback).
const GL_FLAG: &str = "--renderer=gl";

fn run(
    cfg: config::Config,
    instance: nook_platform::single_instance::InstanceGuard,
    renderer: eframe::Renderer,
) -> Result<()> {
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
        renderer,
        wgpu_options: wgpu_options(),
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

/// DirectX 12 presenting through a DirectComposition visual: the only DXGI
/// swapchain that keeps per-pixel alpha, so the window is transparent without
/// relying on the OpenGL driver honoring DWM blur-behind.
fn wgpu_options() -> eframe::egui_wgpu::WgpuConfiguration {
    use eframe::egui_wgpu::{SurfaceConfig, WgpuConfiguration, WgpuSetup, WgpuSetupCreateNew};
    let mut setup = WgpuSetupCreateNew::without_display_handle();
    setup.instance_descriptor.backends = wgpu::Backends::DX12;
    setup.instance_descriptor.backend_options.dx12.presentation_system =
        wgpu::Dx12SwapchainKind::DxgiFromVisual;
    // The integrated GPU on hybrid laptops: a notch never needs the
    // discrete one, and waking it costs battery.
    setup.power_preference = wgpu::PowerPreference::LowPower;
    setup.device_descriptor = std::sync::Arc::new(|_adapter| wgpu::DeviceDescriptor {
        label: Some("nook"),
        required_limits: wgpu::Limits {
            // Font atlas and plugin images.
            max_texture_dimension_2d: 8192,
            ..wgpu::Limits::downlevel_defaults()
        },
        // Smaller allocator blocks: egui needs a few MB of buffers at most.
        memory_hints: wgpu::MemoryHints::MemoryUsage,
        ..Default::default()
    });
    WgpuConfiguration {
        // One queued frame: the panel follows the cursor with the least lag.
        surface: SurfaceConfig::LOW_LATENCY,
        wgpu_setup: WgpuSetup::CreateNew(setup),
        ..Default::default()
    }
}
