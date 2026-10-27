//! The eframe app: owns the shell, modules, tray, hotkeys and system
//! watcher, and keeps the OS window (position, region, material) in sync
//! with what the shell draws.

use crate::config::{AccentSource, Config, Material, MonitorMode};
use crate::settings::{self, SETTINGS_ID, SettingsTile};
use crate::stats::{self, Stats};
use crate::tray::{Tray, TrayCommand};
use crate::update::{self, Updater};
use crate::watcher::{self, SystemState, WatcherControl};
use anyhow::{Context as _, Result};
use crossbeam_channel::{Receiver, unbounded};
use egui::{Color32, FontFamily, FontId, Rect, Ui, pos2, vec2};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use nook_platform::backdrop::{Backdrop, Tint};
use nook_platform::hittest::{HitShape, HitTester};
use nook_platform::material::{self, Applied};
use nook_platform::single_instance::InstanceGuard;
use nook_platform::system::Battery;
use nook_platform::{Hwnd, startup, system, wallpaper, window};
use nook_ui::anim::{self, Motion};
use nook_ui::bento;
use nook_ui::geometry::Squircle;
use nook_ui::module::{Badge, Module, Registry};
use nook_ui::paint;
use nook_ui::shell::{Shell, ShellMetrics, ShellOutput};
use nook_ui::shell_state::ShellTimings;
use nook_ui::theme::{self, FALLBACK_ACCENT, SIZE_BODY, SIZE_LABEL, Theme, ThemeMode};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Bento,
    Settings,
    /// A module's full-panel page (e.g. clipboard history), by module id.
    Module(&'static str),
}

/// Shell outline (window px) and debug-overlay rect last sent to the OS.
type PushedShape = (Vec<(f32, f32)>, Option<Rect>);

struct Hotkeys {
    manager: GlobalHotKeyManager,
    toggle_id: Option<u32>,
    /// Module hotkeys and the module each belongs to.
    modules: Vec<(HotKey, &'static str)>,
    rx: Receiver<GlobalHotKeyEvent>,
}

impl Hotkeys {
    /// Registers a module's own shortcut, if it has one.
    fn add_module(&mut self, m: &dyn Module) {
        let Some(accel) = m.default_hotkey() else {
            return;
        };
        match HotKey::from_str(accel).map(|hk| (self.manager.register(hk), hk)) {
            Ok((Ok(()), hk)) => self.modules.push((hk, m.id())),
            Ok((Err(e), _)) => tracing::warn!("hotkey {accel} for {} unavailable: {e}", m.id()),
            Err(e) => tracing::warn!("bad hotkey {accel:?} for {}: {e}", m.id()),
        }
    }

    /// Frees the shortcut of a module that was taken out (a plugin).
    fn remove_module(&mut self, id: &str) {
        for (hk, _) in self.modules.iter().filter(|(_, m)| *m == id) {
            if let Err(e) = self.manager.unregister(*hk) {
                tracing::warn!("releasing hotkey for {id}: {e}");
            }
        }
        self.modules.retain(|(_, m)| *m != id);
    }
}

pub struct NookApp {
    cfg: Config,
    hwnd: Hwnd,
    shell: Shell,
    registry: Registry,
    page: Page,
    /// The grid is in edit mode (pencil in the gutter).
    editing: bool,
    pinned_before_edit: bool,
    tray: Option<Tray>,
    tray_rx: Receiver<TrayCommand>,
    hotkeys: Option<Hotkeys>,
    sys: SystemState,
    sys_rx: Receiver<SystemState>,
    watcher: Arc<WatcherControl>,
    accent_tx: crossbeam_channel::Sender<Option<[u8; 3]>>,
    accent_rx: Receiver<Option<[u8; 3]>>,
    wallpaper_accent: Option<Color32>,
    system_accent: Option<Color32>,
    material: Applied,
    material_key: Option<(bool, bool, Material)>,
    /// Created lazily the first time the glass material is wanted.
    backdrop: Option<Backdrop>,
    backdrop_failed: bool,
    /// Click-through outside the shape (the window has no region).
    hit: Option<HitTester>,
    /// Last shape pushed to the compositor and the hit tester (window px).
    pushed: Option<PushedShape>,
    /// Forces the next frame to push its shape to the OS.
    shape_dirty: bool,
    /// Whether the cursor was inside the shape at the previous frame.
    was_inside: bool,
    /// Z-layer last applied (`cfg.on_top`).
    applied_on_top: Option<bool>,
    /// Frames drawn; the window stays fully transparent for the first few
    /// so no uninitialized surface ever shows.
    frames: u32,
    /// When the shell last settled collapsed, and whether memory was trimmed since.
    collapsed_since: Option<f64>,
    trimmed: bool,
    /// Window rect in physical pixels: (x, y, w, h).
    anchor: Option<(i32, i32, i32, i32)>,
    activatable: bool,
    /// GitHub release checks and the in-place update.
    updater: Updater,
    /// The anonymous daily usage ping.
    stats: Stats,
    _instance: InstanceGuard,
    #[cfg(feature = "debug-overlay")]
    overlay: crate::overlay::DebugOverlay,
}

impl NookApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        cfg: Config,
        instance: InstanceGuard,
    ) -> Result<Self> {
        let ctx = cc.egui_ctx.clone();
        theme::install_fonts(&ctx);

        let hwnd = match cc.window_handle().context("window handle")?.as_raw() {
            RawWindowHandle::Win32(h) => Hwnd::from_raw(h.hwnd.get()),
            other => anyhow::bail!("unexpected window handle {other:?}"),
        };
        window::ensure_overlay(hwnd, false).context("overlay window styles")?;
        if let Err(e) = window::fill_client(hwnd) {
            tracing::warn!("frameless client area: {e}");
        }
        // Invisible until the first frames are drawn.
        window::set_opacity(hwnd, 0).context("initial window opacity")?;

        let repaint = ctx.clone();
        let hit = match HitTester::spawn(move |_| repaint.request_repaint()) {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!("hit testing unavailable, the notch will not take clicks: {e}");
                None
            }
        };

        let (tray_tx, tray_rx) = unbounded();
        let tray = match Tray::new(tray_tx, ctx.clone(), startup::is_enabled(), cfg.on_top) {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::error!("tray icon unavailable: {e:#}");
                None
            }
        };

        let watcher = Arc::new(WatcherControl::default());
        watcher.set_monitor_mode(cfg.monitor);
        let sys = watcher::sample(hwnd, cfg.monitor == MonitorMode::Cursor);
        let (sys_tx, sys_rx) = unbounded();
        watcher::spawn(hwnd, watcher.clone(), sys_tx, ctx.clone()).context("spawning watcher")?;

        let (accent_tx, accent_rx) = unbounded();
        spawn_wallpaper_accent(accent_tx.clone(), ctx.clone());

        nook_modules::plugins::set_dev_mode(cfg.plugin_dev);
        let mut registry = Registry::default();
        nook_modules::register_all(&mut registry);
        registry.register(Box::new(SettingsTile));
        registry.arrange(&cfg.modules.order, &cfg.modules.disabled);
        registry.set_sizes(&cfg.modules.sizes);
        for m in registry.all_mut() {
            m.attach(&ctx);
        }
        let hotkeys = match register_hotkeys(&cfg, &registry, &ctx) {
            Ok(h) => Some(h),
            Err(e) => {
                tracing::error!("global hotkeys unavailable: {e:#}");
                None
            }
        };

        // Keep config and the Run key in agreement (the key is the truth).
        let mut cfg = cfg;
        cfg.launch_at_startup = startup::is_enabled();

        let timings = timings(&cfg);
        let mut shell = Shell::new(ShellMetrics::default(), timings);
        shell.placement = cfg.placement;
        // Dev flags (screenshots/demos): `--pinned` starts expanded and
        // pinned, `--settings` additionally opens the settings page.
        // `--page=<module id>` opens that module's page.
        let flag = |f: &str| std::env::args().any(|a| a == f);
        let module_page = std::env::args()
            .find_map(|a| a.strip_prefix("--page=").map(str::to_owned))
            .and_then(|id| registry.all().find(|m| m.id() == id).map(|m| m.id()));
        let page = if flag("--settings") {
            Page::Settings
        } else if let Some(id) = module_page {
            Page::Module(id)
        } else {
            Page::Bento
        };
        if flag("--pinned") || page != Page::Bento {
            shell.state.set_pinned(true, 0.0);
        }
        let updater = Updater::start(ctx.clone(), cfg.check_updates);
        // A missing id means a new install (or a config from before stats).
        let new_install = !stats::valid_id(&cfg.install_id);
        if new_install {
            cfg.install_id = stats::new_install_id();
            if let Err(e) = cfg.save() {
                tracing::error!("saving config: {e:#}");
            }
        }
        let stats = Stats::start(cfg.install_id.clone(), new_install, cfg.usage_stats);
        Ok(Self {
            cfg,
            hwnd,
            shell,
            registry,
            page,
            editing: false,
            pinned_before_edit: false,
            tray,
            tray_rx,
            hotkeys,
            sys,
            sys_rx,
            watcher,
            accent_tx,
            accent_rx,
            wallpaper_accent: None,
            system_accent: system::system_accent()
                .ok()
                .map(|[r, g, b]| Color32::from_rgb(r, g, b)),
            material: Applied::Opaque,
            material_key: None,
            backdrop: None,
            backdrop_failed: false,
            hit,
            pushed: None,
            shape_dirty: true,
            was_inside: false,
            applied_on_top: None,
            frames: 0,
            collapsed_since: None,
            trimmed: false,
            anchor: None,
            activatable: false,
            updater,
            stats,
            _instance: instance,
            #[cfg(feature = "debug-overlay")]
            overlay: Default::default(),
        })
    }

    fn dark(&self) -> bool {
        match self.cfg.theme {
            ThemeMode::Dark => true,
            ThemeMode::Light => false,
            ThemeMode::Auto => !self.sys.light_theme,
        }
    }

    fn accent(&self) -> Color32 {
        match self.cfg.accent {
            AccentSource::Wallpaper => self.wallpaper_accent.unwrap_or(FALLBACK_ACCENT),
            AccentSource::System => self.system_accent.unwrap_or(FALLBACK_ACCENT),
            AccentSource::Custom => {
                theme::parse_hex(&self.cfg.accent_hex).unwrap_or(FALLBACK_ACCENT)
            }
        }
    }

    fn save_config(&self) {
        if let Err(e) = self.cfg.save() {
            tracing::error!("saving config: {e:#}");
        }
    }

    /// Enters or leaves the grid's edit mode. Editing pins the panel open
    /// (a drag near the edge must not fold it); leaving restores the pin.
    fn set_editing(&mut self, on: bool, ctx: &egui::Context) {
        if on == self.editing {
            return;
        }
        self.editing = on;
        let now = Self::now(ctx);
        if on {
            self.pinned_before_edit = self.shell.state.is_pinned();
            self.shell.state.set_pinned(true, now);
        } else {
            self.shell.state.set_pinned(self.pinned_before_edit, now);
        }
    }

    /// Applies edit-mode changes to the config and the grid, and saves.
    fn apply_layout_edits(&mut self, edits: Vec<bento::Edit>) {
        let modules = &mut self.cfg.modules;
        for edit in edits {
            match edit {
                bento::Edit::Reorder(ids) => {
                    modules.order = ids.into_iter().map(str::to_owned).collect();
                }
                bento::Edit::ToggleShown(id) => {
                    if modules.disabled.iter().any(|d| d == id) {
                        modules.disabled.retain(|d| d != id);
                    } else {
                        modules.disabled.push(id.to_owned());
                    }
                }
                bento::Edit::Resize(id, size) => {
                    let native = self
                        .registry
                        .all()
                        .find(|m| m.id() == id)
                        .map(|m| m.size_hint());
                    if Some(size) == native {
                        modules.sizes.remove(id);
                    } else {
                        modules.sizes.insert(id.to_owned(), size);
                    }
                }
            }
        }
        self.registry.arrange(&modules.order, &modules.disabled);
        self.registry.set_sizes(&modules.sizes);
        self.save_config();
    }

    fn now(ctx: &egui::Context) -> f64 {
        ctx.input(|i| i.time)
    }

    /// After the shell has sat collapsed and untouched for a few seconds,
    /// hand the working set back to the OS. Repeats after each idle repaint
    /// (the pill clock ticks once a minute), never while the pointer is on it.
    fn trim_when_idle(&mut self, ctx: &egui::Context, progress: f32) {
        const AFTER: f64 = 3.0;
        let touched = ctx.input(|i| i.pointer.hover_pos().is_some())
            || self.registry.all().any(|m| m.overlay_open());
        if self.shell.state.is_open() || progress > 0.001 || touched {
            self.collapsed_since = None;
            self.trimmed = false;
            return;
        }
        let now = Self::now(ctx);
        let since = *self.collapsed_since.get_or_insert(now);
        if now - since >= AFTER {
            if let Err(e) = nook_platform::process::trim_working_set() {
                tracing::debug!("trim: {e}");
            }
            self.trimmed = true;
        } else if !self.trimmed {
            ctx.request_repaint_after(Duration::from_secs_f64(AFTER - (now - since) + 0.01));
        }
    }

    fn handle_tray(&mut self, ctx: &egui::Context, cmd: TrayCommand) {
        let now = Self::now(ctx);
        match cmd {
            TrayCommand::Settings => {
                self.page = Page::Settings;
                self.shell.state.set_pinned(true, now);
            }
            TrayCommand::TogglePin => self.shell.state.toggle_pin(now),
            TrayCommand::ToggleOnTop => {
                self.cfg.on_top = !self.cfg.on_top;
                self.save_config();
            }
            TrayCommand::ToggleStartup => {
                self.cfg.launch_at_startup = !self.cfg.launch_at_startup;
                self.apply_startup();
            }
            TrayCommand::Update => self.updater.install(),
            TrayCommand::Quit => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        }
    }

    fn apply_startup(&mut self) {
        if let Err(e) = startup::set_enabled(self.cfg.launch_at_startup) {
            tracing::error!("launch at startup: {e}");
            self.cfg.launch_at_startup = startup::is_enabled();
        }
        if let Some(t) = &self.tray {
            t.set_startup(self.cfg.launch_at_startup);
        }
        self.save_config();
    }

    /// Spans the window across the top of the target monitor's work area.
    /// The shell positions itself inside it (left, center or right).
    fn anchor_window(&mut self) {
        let Some(mon) = self.sys.monitor else {
            return;
        };
        let scale = window::dpi(self.hwnd) as f32 / 96.0;
        let h = (self.shell.window_size().y * scale).ceil() as i32;
        let rect = (mon.work.left, mon.work.top, mon.work.width(), h);
        if self.anchor != Some(rect) {
            match window::set_rect(self.hwnd, rect.0, rect.1, rect.2, rect.3) {
                Ok(()) => {
                    self.anchor = Some(rect);
                    self.shape_dirty = true;
                }
                Err(e) => tracing::warn!("anchoring window: {e}"),
            }
        }
    }

    /// Lets clicks through everywhere except on the shell (and keeps input
    /// while a button pressed on it is held). The hit tester decides; the window
    /// style can only be changed safely from this thread.
    fn apply_click_through(&mut self) {
        let through = !self.hit.as_ref().is_none_or(HitTester::inside);
        // Checked against the real style every frame (one read, a write only
        // when it differs): winit rewrites the extended style when the
        // window is activated, which used to drop the bit while the cached
        // state said it was set, leaving the whole strip catching clicks.
        if let Err(e) = window::set_click_through(self.hwnd, through) {
            tracing::warn!("click-through: {e}");
        }
    }

    /// Keeps the z-order in line with `cfg.on_top`, and re-applies it if
    /// something (winit, the shell) changed the topmost bit behind our back
    /// or another always-on-top window came up over the notch.
    fn apply_layer(&mut self) {
        let drift = window::is_topmost(self.hwnd) != self.cfg.on_top
            || (self.cfg.on_top && self.sys.covered);
        if self.applied_on_top == Some(self.cfg.on_top) && !drift {
            return;
        }
        let host = self.backdrop.as_ref().map(Backdrop::host);
        match window::set_layer(self.hwnd, host, self.cfg.on_top) {
            Ok(()) => {
                self.applied_on_top = Some(self.cfg.on_top);
                self.sys.covered = false;
                tracing::info!("on top: {}", self.cfg.on_top);
            }
            Err(e) => tracing::warn!("window layer: {e}"),
        }
        if let Some(t) = &self.tray {
            t.set_on_top(self.cfg.on_top);
        }
    }

    fn apply_material(&mut self) {
        let dark = self.dark();
        let key = (dark, self.sys.transparency, self.cfg.material);
        if self.material_key == Some(key) {
            return;
        }
        self.material_key = Some(key);
        material::prepare(self.hwnd, dark);
        // The backdrop window also paints the glass tint, so it is used for
        // both materials; "Solid" only turns its blur layer off.
        if self.backdrop.is_none() && !self.backdrop_failed {
            match Backdrop::new(self.hwnd) {
                Ok(b) => self.backdrop = Some(b),
                Err(e) => {
                    tracing::warn!("composition backdrop unavailable, painting glass in GL: {e}");
                    self.backdrop_failed = true;
                }
            }
        }
        let blur = self.cfg.material == Material::Acrylic && self.sys.transparency;
        self.material = if blur && self.backdrop.is_some() {
            Applied::Acrylic
        } else {
            Applied::Opaque
        };
        self.shape_dirty = true;
        tracing::info!("backdrop material: {:?}", self.material);
    }

    /// Pushes this frame's glass tint to the compositor.
    fn update_tint(&mut self, ctx: &egui::Context, theme: &Theme, out: &ShellOutput) {
        let Some(b) = &mut self.backdrop else {
            return;
        };
        let ppp = ctx.pixels_per_point();
        let (top, bottom) = theme.glass_at(out.progress);
        let unmult = |c: Color32| theme::with_alpha(c, out.opacity).to_srgba_unmultiplied();
        let rect = out.shape.rect;
        let r = out.shape.radius;
        let tint = Tint {
            top: unmult(top),
            bottom: unmult(bottom),
            y0: rect.min.y * ppp,
            y1: rect.max.y * ppp,
            blur: self.material == Applied::Acrylic,
            rim: unmult(theme.rim_at(out.progress)),
            rim_width: 2.0 * ppp,
            highlight: unmult(theme.highlight_at(out.progress)),
            highlight_x0: (rect.min.x + r) * ppp,
            highlight_x1: (rect.max.x - r) * ppp,
            highlight_y: (rect.min.y + 1.0) * ppp,
        };
        if let Err(e) = b.set_tint(tint) {
            tracing::warn!("backdrop tint: {e}");
        }
    }

    /// Pushes the shell's current shape to the compositor (sub-pixel clip
    /// of the glass) and to the hit tester (screen pixels, plus the debug
    /// overlay if enabled). Only does work when the shape changed.
    fn update_shape(&mut self, ctx: &egui::Context, shape: Option<Squircle>, extra: Option<Rect>) {
        let ppp = ctx.pixels_per_point();
        let outline: Vec<(f32, f32)> = shape
            .map(|s| s.outline().iter().map(|p| (p.x * ppp, p.y * ppp)).collect())
            .unwrap_or_default();
        let key = (outline, extra);
        if self.pushed.as_ref() == Some(&key) && !self.shape_dirty {
            return;
        }
        let (outline, extra) = key;
        if let Some(b) = &mut self.backdrop
            && let Err(e) = b.set_shape(&outline)
        {
            tracing::warn!("backdrop clip: {e}");
        }
        if let Some(hit) = &self.hit {
            let (ox, oy) = self
                .anchor
                .map(|(x, y, _, _)| (x as f32, y as f32))
                .unwrap_or_default();
            let mut polygons = Vec::with_capacity(2);
            if let Some(s) = shape {
                // A point of slack so the anti-aliased edge still counts.
                let padded =
                    Squircle::new(s.rect.expand(1.0), s.radius + 1.0).with_exponent(s.exponent);
                polygons.push(
                    padded
                        .outline()
                        .iter()
                        .map(|p| (ox + p.x * ppp, oy + p.y * ppp))
                        .collect(),
                );
            }
            if let Some(r) = extra {
                let (x0, y0) = (ox + r.min.x * ppp, oy + r.min.y * ppp);
                let (x1, y1) = (ox + r.max.x * ppp, oy + r.max.y * ppp);
                polygons.push(vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]);
            }
            hit.set_shape(HitShape { polygons });
        }
        self.pushed = Some((outline, extra));
        self.shape_dirty = false;
    }

    /// Re-asserts the overlay styles (winit resets them on some state
    /// changes). Text input needs an activatable window; otherwise never
    /// steal focus.
    fn sync_window_styles(&mut self, ctx: &egui::Context) {
        // Only a text field needs the keyboard; any clicked widget holds
        // egui focus, and taking the foreground for those would steal it
        // from other windows (the screenshot editor, the browser).
        let wanted = ctx.text_edit_focused();
        let gained = wanted && !self.activatable;
        self.activatable = wanted;
        match window::ensure_overlay(self.hwnd, self.activatable) {
            Ok(true) => tracing::debug!("re-applied overlay window styles"),
            Ok(false) => {}
            Err(e) => tracing::warn!("overlay styles: {e}"),
        }
        // A text field just took focus: the window must be the foreground
        // one for the keys to arrive (the click that focused it allows it).
        if gained {
            window::activate(self.hwnd);
        }
    }

    /// Panel size the open page asks for, kept inside the monitor's work area.
    fn page_size(&self, ctx: &egui::Context) -> Option<egui::Vec2> {
        let m = self.shell.metrics;
        let want = match self.page {
            Page::Bento => return None,
            Page::Settings => m.expanded + vec2(0.0, settings::extra_height(ctx, m.expanded.y)),
            Page::Module(id) => self.registry.all().find(|m| m.id() == id)?.page_size()?,
        };
        let fit = self.sys.monitor.map_or(want, |mon| {
            let ppp = ctx.pixels_per_point();
            let room_y = mon.work.height() as f32 / ppp / 1.08 - m.top_margin - 24.0;
            let room_x = mon.work.width() as f32 / ppp - 2.0 * m.side_margin - 24.0;
            vec2(want.x.min(room_x), want.y.min(room_y))
        });
        Some(fit.max(m.expanded))
    }

    /// Puts plugin installs, updates and removals into effect while Nook
    /// runs: the registry, the grid layout and the hotkeys follow.
    fn apply_plugin_changes(&mut self, ctx: &egui::Context) {
        let applied = nook_modules::plugins::apply_changes(&mut self.registry, ctx);
        if applied.added.is_empty() && applied.removed.is_empty() {
            return;
        }
        if let Some(hk) = &mut self.hotkeys {
            for id in &applied.removed {
                hk.remove_module(id);
            }
        }
        let modules = &mut self.cfg.modules;
        for &id in &applied.added {
            // A plugin hidden before and installed again should show.
            modules.disabled.retain(|d| d != id);
            if let (Some(hk), Some(m)) = (
                &mut self.hotkeys,
                self.registry.all().find(|m| m.id() == id),
            ) {
                hk.add_module(m);
            }
        }
        self.registry.arrange(&modules.order, &modules.disabled);
        self.registry.set_sizes(&modules.sizes);
        self.save_config();
    }

    /// Hands files dropped on the notch to the first visible module that
    /// takes them.
    fn route_dropped_files(&mut self, ctx: &egui::Context) {
        let paths: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .collect()
        });
        if paths.is_empty() {
            return;
        }
        match self
            .registry
            .visible_mut()
            .into_iter()
            .find(|m| m.accepts_files())
        {
            Some(m) => {
                tracing::info!("{} file(s) dropped on {}", paths.len(), m.id());
                m.drop_files(paths);
            }
            None => tracing::info!("{} file(s) dropped, no module takes files", paths.len()),
        }
    }
}

/// Paints `text` so its ink (not its line box) starts at `x`, or ends at `x`
/// when `from_right`, and is vertically centered on `cy`. Segoe UI's line
/// box carries descender space and Phosphor glyphs are narrower than their
/// em box, so aligning line boxes leaves the pill visibly off-center.
fn ink_text(
    painter: &egui::Painter,
    text: &str,
    font: FontId,
    color: Color32,
    x: f32,
    cy: f32,
    from_right: bool,
) -> Rect {
    let galley = painter.layout_no_wrap(text.to_owned(), font, color);
    let ink = galley.mesh_bounds;
    let dx = if from_right {
        x - ink.max.x
    } else {
        x - ink.min.x
    };
    let offset = vec2(dx, cy - ink.center().y);
    painter.galley(pos2(offset.x, offset.y), galley, color);
    ink.translate(offset)
}

fn icon_font(size: f32, fill: bool) -> FontId {
    let family = if fill {
        theme::FAMILY_ICON_FILL
    } else {
        theme::FAMILY_ICON_REGULAR
    };
    FontId::new(size, FontFamily::Name(family.into()))
}

/// Collapsed pill: live badges from modules (ring + label on the left,
/// glyphs and pulse dots on the right), inset by the same ink margin on
/// both sides.
fn draw_pill(badges: Vec<Badge>, battery: Option<Battery>, ui: &mut Ui, rect: Rect, theme: &Theme) {
    const PAD: f32 = 15.0;
    let painter = ui.painter();
    let cy = rect.center().y;
    let left = rect.min.x + PAD;
    let mut right = rect.max.x - PAD;
    let now = ui.input(|i| i.time);
    let clock = system::clock();

    // Left: a running timer's ring + countdown, otherwise the clock.
    let ring = badges.iter().find_map(|b| match b {
        Badge::Ring { progress, label } => Some((*progress, label.clone())),
        _ => None,
    });
    match ring {
        Some((progress, label)) => {
            let r = 6.0;
            paint::ring(
                painter,
                pos2(left + r, cy),
                r,
                2.0,
                progress,
                theme::with_alpha(theme.pill_text_dim, 0.35),
                theme.accent,
            );
            ink_text(
                painter,
                &label,
                Theme::font_semibold(SIZE_BODY),
                theme.pill_text,
                left + 2.0 * r + 7.0,
                cy,
                false,
            );
        }
        None => {
            ink_text(
                painter,
                &clock.time,
                Theme::font_semibold(SIZE_BODY),
                theme.pill_text,
                left,
                cy,
                false,
            );
        }
    }

    // Right: live glyphs (coffee, phone pulse...), otherwise battery or date.
    let mut any_right = false;
    for badge in &badges {
        match badge {
            Badge::Ring { .. } => {}
            Badge::Glyph { icon } => {
                let ink = ink_text(
                    painter,
                    icon,
                    icon_font(14.0, true),
                    theme.accent_fg(),
                    right,
                    cy,
                    true,
                );
                right = ink.min.x - 6.0;
                any_right = true;
            }
            Badge::Pulse => {
                let phase = ((now * 1.6).sin() * 0.5 + 0.5) as f32;
                let c = pos2(right - 3.0, cy);
                painter.circle_filled(
                    c,
                    3.0 + 3.0 * phase,
                    theme::with_alpha(theme.accent, 0.35 * (1.0 - phase)),
                );
                painter.circle_filled(c, 3.0, theme.accent);
                right -= 12.0;
                any_right = true;
                ui.ctx().request_repaint_after(Duration::from_millis(33));
            }
        }
    }
    if !any_right {
        match battery {
            Some(b) => {
                let text = ink_text(
                    painter,
                    &format!("{}%", b.percent),
                    Theme::font(SIZE_LABEL + 1.0),
                    theme.pill_text_dim,
                    right,
                    cy,
                    true,
                );
                let (icon, color) = battery_icon(b, theme);
                ink_text(
                    painter,
                    icon,
                    icon_font(15.0, b.charging),
                    color,
                    text.min.x - 4.0,
                    cy,
                    true,
                );
            }
            None => {
                ink_text(
                    painter,
                    &clock.date,
                    Theme::font(SIZE_LABEL + 1.0),
                    theme.pill_text_dim,
                    right,
                    cy,
                    true,
                );
            }
        }
    }

    // One wake-up when the minute changes; nothing in between.
    ui.ctx()
        .request_repaint_after(Duration::from_secs(u64::from(clock.secs_to_next_minute)));
}

fn battery_icon(b: Battery, theme: &Theme) -> (&'static str, Color32) {
    use nook_ui::icons;
    if b.charging {
        return (icons::BATTERY_CHARGING, Color32::from_rgb(98, 214, 132));
    }
    let icon = match b.percent {
        90.. => icons::BATTERY_FULL,
        60.. => icons::BATTERY_HIGH,
        30.. => icons::BATTERY_MEDIUM,
        12.. => icons::BATTERY_LOW,
        _ => icons::BATTERY_WARNING,
    };
    let color = if b.percent < 12 {
        Color32::from_rgb(255, 99, 88)
    } else {
        theme.pill_text_dim
    };
    (icon, color)
}

fn timings(cfg: &Config) -> ShellTimings {
    ShellTimings {
        hover_delay: cfg.hover_delay_ms as f64 / 1000.0,
        collapse_grace: cfg.collapse_grace_ms as f64 / 1000.0,
        ..ShellTimings::default()
    }
}

fn register_hotkeys(cfg: &Config, registry: &Registry, ctx: &egui::Context) -> Result<Hotkeys> {
    let manager = GlobalHotKeyManager::new().context("creating hotkey manager")?;
    let toggle_id = match HotKey::from_str(&cfg.toggle_hotkey) {
        Ok(hk) => match manager.register(hk) {
            Ok(()) => Some(hk.id()),
            Err(e) => {
                tracing::warn!("hotkey {} unavailable: {e}", cfg.toggle_hotkey);
                None
            }
        },
        Err(e) => {
            tracing::warn!("bad hotkey {:?}: {e}", cfg.toggle_hotkey);
            None
        }
    };
    let (tx, rx) = unbounded();
    let ctx = ctx.clone();
    GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
        if tx.send(e).is_ok() {
            ctx.request_repaint();
        }
    }));
    let mut hotkeys = Hotkeys {
        manager,
        toggle_id,
        modules: Vec::new(),
        rx,
    };
    for m in registry.all() {
        hotkeys.add_module(m);
    }
    Ok(hotkeys)
}

fn spawn_wallpaper_accent(tx: crossbeam_channel::Sender<Option<[u8; 3]>>, ctx: egui::Context) {
    let spawned = std::thread::Builder::new()
        .name("nook-accent".into())
        .spawn(move || {
            let accent = match wallpaper::accent_from_wallpaper() {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!("wallpaper accent: {e}");
                    None
                }
            };
            if tx.send(accent).is_ok() {
                ctx.request_repaint();
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("spawning accent thread: {e}");
    }
}

impl eframe::App for NookApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(cmd) = self.tray_rx.try_recv() {
            self.handle_tray(ctx, cmd);
        }
        if let Some(hk) = &self.hotkeys {
            let mut toggle = false;
            while let Ok(e) = hk.rx.try_recv() {
                if e.state != HotKeyState::Pressed {
                    continue;
                }
                if Some(e.id) == hk.toggle_id {
                    toggle = !toggle;
                } else if let Some((_, id)) = hk.modules.iter().find(|(k, _)| k.id() == e.id)
                    && let Some(m) = self.registry.get_mut(id)
                {
                    m.on_hotkey();
                }
            }
            if toggle {
                let now = Self::now(ctx);
                // The hotkey always brings a notch that was sent behind
                // other windows back on top.
                if !self.cfg.on_top {
                    self.cfg.on_top = true;
                    self.save_config();
                }
                let st = &mut self.shell.state;
                if st.is_open() && st.is_pinned() {
                    st.set_open(false, now);
                } else {
                    st.set_pinned(true, now);
                }
            }
        }
        while let Ok(s) = self.sys_rx.try_recv() {
            if s.light_theme != self.sys.light_theme {
                self.system_accent = system::system_accent()
                    .ok()
                    .map(|[r, g, b]| Color32::from_rgb(r, g, b));
            }
            if s.wallpaper != self.sys.wallpaper {
                // New wallpaper (from the wallpaper page or elsewhere): new accent.
                spawn_wallpaper_accent(self.accent_tx.clone(), ctx.clone());
            }
            self.sys = s;
        }
        while let Ok(a) = self.accent_rx.try_recv() {
            self.wallpaper_accent = a.map(|[r, g, b]| Color32::from_rgb(r, g, b));
        }
        self.apply_plugin_changes(ctx);
        let mut show = None;
        for m in self.registry.all_mut() {
            m.on_tick();
            if m.take_show_request() {
                show = Some(m.id());
            }
        }
        if let Some(id) = show {
            self.page = Page::Module(id);
            self.shell.state.set_pinned(true, Self::now(ctx));
        }
        anim::set_motion(
            ctx,
            Motion {
                reduced: !self.sys.animations,
            },
        );
        self.apply_material();
        self.anchor_window();
        self.apply_layer();
        self.apply_click_through();
        if let Some(b) = &mut self.backdrop
            && let Err(e) = b.follow(self.hwnd)
        {
            tracing::warn!("backdrop placement: {e}");
        }
        if let Some(t) = &self.tray {
            t.set_pinned(self.shell.state.is_pinned());
        }
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        #[cfg(feature = "debug-overlay")]
        self.overlay.begin();
        let ctx = ui.ctx().clone();
        let theme = Theme::styled(
            self.dark(),
            self.accent(),
            self.material.translucent(),
            &self.cfg.look(),
        );
        let hidden = self.cfg.hide_in_fullscreen && self.sys.fullscreen;

        let mut clicked = None;
        let mut page_request = None;
        let mut module_back = false;
        let mut settings_out = settings::PageOutput::default();
        let mut grid_out = bento::BentoOutput::default();
        let mut update_clicked = false;
        let update_status = self.updater.status();
        let editing = self.editing;
        let page = self.page;
        let badges: Vec<Badge> = self
            .registry
            .visible()
            .filter_map(|m| m.collapsed_badge())
            .collect();
        let battery = self.sys.battery;
        let composited = self.backdrop.is_some();
        // Stay open while typing in the panel (and the window has focus) or
        // while files are dragged over it.
        self.shell.hold_open = (ctx.text_edit_focused() && ctx.input(|i| i.focused))
            || ctx.input(|i| !i.raw.hovered_files.is_empty());
        self.route_dropped_files(&ctx);
        self.shell.expanded_override = self.page_size(&ctx);
        let registry = &mut self.registry;
        let cfg = &mut self.cfg;
        let out = self.shell.show(
            ui,
            &theme,
            hidden,
            composited,
            |ui, rect, _alpha| draw_pill(badges, battery, ui, rect, &theme),
            |ui, cctx| match page {
                Page::Bento => {
                    let mut tiles = registry.grid_mut(editing);
                    grid_out = bento::show(ui, cctx, &theme, &mut tiles, editing);
                    clicked = grid_out.clicked;
                    // The Update chip sits in the middle of the bottom
                    // gutter, between the pencil and the page dots.
                    if !editing && cctx.alpha > 0.5 {
                        let grid = bento::grid_rect(cctx.panel);
                        let y = grid.max.y + (cctx.panel.max.y - grid.max.y) * 0.5 - 1.0;
                        update_clicked = update::chip(
                            ui,
                            pos2(grid.center().x, y),
                            egui::Align2::CENTER_CENTER,
                            &update_status,
                            &theme,
                        )
                        .is_some_and(|c| c.clicked);
                    }
                    page_request = tiles
                        .iter_mut()
                        .filter(|t| !t.hidden)
                        .find_map(|t| t.module.take_page_request().then(|| t.module.id()));
                }
                Page::Module(id) => {
                    if let Some(m) = registry.get_mut(id) {
                        let title = m.name().to_owned();
                        let mut close = false;
                        module_back = nook_ui::page::show(
                            ui,
                            cctx,
                            &theme,
                            id,
                            &title,
                            |ui, header, body| {
                                close = m.ui_page(ui, header, body, &theme);
                            },
                        ) || close;
                    } else {
                        module_back = true;
                    }
                }
                Page::Settings => {
                    settings_out = settings::page(ui, cctx, &theme, cfg, &update_status);
                }
            },
        );

        if clicked == Some(SETTINGS_ID) {
            self.page = Page::Settings;
        }
        if settings_out.back || module_back {
            self.page = Page::Bento;
        }
        if settings_out.open_plugins {
            self.page = Page::Module(nook_modules::plugins::PLUGINS_ID);
        }
        if grid_out.toggle_edit {
            self.set_editing(!self.editing, &ctx);
        }
        if !grid_out.edits.is_empty() {
            self.apply_layout_edits(grid_out.edits);
        }
        if let Some(id) = page_request {
            self.page = Page::Module(id);
        }
        for m in self.registry.all_mut() {
            m.ui_overlay(&ctx, &theme);
        }
        if settings_out.startup_toggled {
            self.apply_startup();
        }
        if update_clicked || settings_out.install_update {
            self.updater.install();
        }
        if settings_out.check_update {
            self.updater.check_now();
        }
        if let Some(t) = &mut self.tray {
            t.set_update(update_status.offer().map(|r| r.version.as_str()));
        }
        // Setup is running and will start the new version; get out of its way.
        if update_status == update::Status::Installing {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if settings_out.changed {
            self.shell.timings = timings(&self.cfg);
            self.shell.placement = self.cfg.placement;
            self.watcher.set_monitor_mode(self.cfg.monitor);
            nook_modules::plugins::set_dev_mode(self.cfg.plugin_dev);
            self.stats.set_enabled(self.cfg.usage_stats);
            // Re-sample now so a monitor switch does not wait for the watcher.
            self.sys.monitor =
                watcher::sample(self.hwnd, self.cfg.monitor == MonitorMode::Cursor).monitor;
            self.save_config();
        }
        // Back to the grid once the panel has fully collapsed.
        if out.progress < 0.05 && !self.shell.state.is_open() {
            self.page = Page::Bento;
            self.editing = false;
            ctx.memory_mut(|m| m.stop_text_input());
        }

        // Live modules ask for their own repaint cadence.
        let expanded = self.shell.state.is_open();
        if let Some(next) = self
            .registry
            .visible()
            .filter_map(|m| m.next_repaint(expanded))
            .min()
        {
            ctx.request_repaint_after(next);
        }

        self.sync_window_styles(&ctx);
        self.trim_when_idle(&ctx, out.progress);

        #[cfg(feature = "debug-overlay")]
        let overlay_rect = Some(self.overlay.show(ui, &theme));
        #[cfg(not(feature = "debug-overlay"))]
        let overlay_rect: Option<Rect> = None;

        self.update_tint(&ctx, &theme, &out);
        self.update_shape(&ctx, out.visible.then_some(out.shape), overlay_rect);

        // Reveal the window once real frames have been presented.
        self.frames = self.frames.saturating_add(1);
        if self.frames == 3 {
            window::ensure_shown(self.hwnd);
            if let Err(e) = window::set_opacity(self.hwnd, 255) {
                tracing::warn!("window opacity: {e}");
            }
        } else if self.frames < 3 {
            ctx.request_repaint();
        }
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        // The hit tester made the window click-through as the cursor left
        // the shape, so no more mouse events arrive: tell egui it is gone.
        let inside = self.hit.as_ref().is_none_or(HitTester::inside);
        if self.was_inside && !inside {
            let clicking = raw
                .events
                .iter()
                .any(|e| matches!(e, egui::Event::PointerButton { .. }));
            if clicking {
                // Let egui finish the click at its position first.
                ctx.request_repaint();
                return;
            }
            raw.events.push(egui::Event::PointerGone);
        }
        // Becoming interactive sends no mouse move until the cursor moves
        // again; report where it already is so a resting cursor hovers too.
        if !self.was_inside
            && inside
            && let (Ok((x, y)), Some((ox, oy, _, _))) =
                (nook_platform::monitor::cursor_pos(), self.anchor)
        {
            let ppp = ctx.pixels_per_point();
            raw.events.push(egui::Event::PointerMoved(pos2(
                (x - ox) as f32 / ppp,
                (y - oy) as f32 / ppp,
            )));
        }
        self.was_inside = inside;
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        for m in self.registry.all_mut() {
            m.shutdown();
        }
        self.watcher
            .stop
            .store(true, std::sync::atomic::Ordering::Relaxed);
        self.save_config();
        tracing::info!("bye");
    }
}
