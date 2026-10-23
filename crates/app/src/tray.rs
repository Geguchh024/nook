//! System tray icon and menu.

use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use nook_ui::geometry::Squircle;
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrayCommand {
    Settings,
    TogglePin,
    ToggleOnTop,
    ToggleStartup,
    Quit,
}

pub struct Tray {
    _icon: TrayIcon,
    pin: CheckMenuItem,
    on_top: CheckMenuItem,
    startup: CheckMenuItem,
}

impl Tray {
    /// Must run on the thread that pumps Win32 messages (the UI thread).
    pub fn new(
        tx: Sender<TrayCommand>,
        ctx: egui::Context,
        startup_on: bool,
        on_top_on: bool,
    ) -> Result<Self> {
        let settings = MenuItem::with_id("settings", "Settings", true, None);
        let pin = CheckMenuItem::with_id("pin", "Pin open", true, false, None);
        let on_top = CheckMenuItem::with_id("ontop", "Always on top", true, on_top_on, None);
        let startup =
            CheckMenuItem::with_id("startup", "Launch at startup", true, startup_on, None);
        let quit = MenuItem::with_id("quit", "Quit Nook", true, None);
        let menu = Menu::new();
        menu.append_items(&[
            &settings,
            &pin,
            &on_top,
            &startup,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .context("building tray menu")?;

        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let cmd = match event.id.as_ref() {
                "settings" => TrayCommand::Settings,
                "pin" => TrayCommand::TogglePin,
                "ontop" => TrayCommand::ToggleOnTop,
                "startup" => TrayCommand::ToggleStartup,
                "quit" => TrayCommand::Quit,
                _ => return,
            };
            if tx.send(cmd).is_ok() {
                ctx.request_repaint();
            }
        }));

        let icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Nook")
            .with_icon(tray_glyph()?)
            .build()
            .context("creating tray icon")?;
        Ok(Self {
            _icon: icon,
            pin,
            on_top,
            startup,
        })
    }

    pub fn set_pinned(&self, pinned: bool) {
        self.pin.set_checked(pinned);
    }

    pub fn set_on_top(&self, on: bool) {
        self.on_top.set_checked(on);
    }

    pub fn set_startup(&self, on: bool) {
        self.startup.set_checked(on);
    }
}

/// A small white pill on a transparent background, drawn in code so the
/// binary needs no icon assets.
fn tray_glyph() -> Result<Icon> {
    const N: u32 = 32;
    let pill = Squircle::new(
        egui::Rect::from_min_max(egui::pos2(3.0, 10.0), egui::pos2(29.0, 22.0)),
        6.0,
    );
    let mut rgba = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            // 4x4 supersampling for smooth edges.
            let mut hits = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let p = egui::pos2(
                        x as f32 + (sx as f32 + 0.5) / 4.0,
                        y as f32 + (sy as f32 + 0.5) / 4.0,
                    );
                    if pill.contains(p) {
                        hits += 1;
                    }
                }
            }
            let a = (hits * 255 / 16) as u8;
            rgba.extend_from_slice(&[255, 255, 255, a]);
        }
    }
    Icon::from_rgba(rgba, N, N).context("tray icon pixels")
}
