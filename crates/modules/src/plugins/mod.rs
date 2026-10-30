//! Plugins: tiles anyone can write, in any language, without rebuilding
//! Nook.
//!
//! A plugin is a folder with a `plugin.toml` ([`manifest`]) and a program.
//! Nook starts the program and the two talk in JSON lines ([`protocol`]): the
//! program describes its tile and page as view trees, Nook draws them in its
//! own style ([`view`]) and sends back clicks, edits, hotkeys and dropped
//! files. Each plugin is an ordinary [`Module`] to the rest of the app, so it
//! can be moved, resized and hidden in the tile editor like the built-ins.
//!
//! See `docs/plugins.html` for the guide and `examples/plugins` for samples.

mod examples;
mod host;
mod icon_names;
pub mod icons;
pub mod install;
mod manager;
pub mod manifest;
pub mod protocol;
mod view;

pub use manager::PluginsTile;
pub use view::keyboard_focused;

/// Module id of the Plugins tile and page.
pub const PLUGINS_ID: &str = "plugins";

use egui::emath::GuiRounding as _;
use egui::{Color32, Rect, Sense, Ui, UiBuilder, pos2, vec2};
use host::{Event, Host, Tail};
use manifest::{Found, Manifest};
use nook_ui::module::{Badge, Module, Registry, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme, to_hex};
use nook_ui::{page::HEADER, widgets};
use protocol::{Action, BadgeSpec, Node, Out};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use view::{Renderer, ViewState};

/// Prefix of plugin ids in Nook's config, so they never clash with
/// built-in modules.
pub const ID_PREFIX: &str = "plugin:";
/// Automatic restarts allowed within [`RESTART_WINDOW`] before giving up.
const MAX_RESTARTS: usize = 3;
const RESTART_WINDOW: Duration = Duration::from_secs(60);
const RESTART_DELAY: Duration = Duration::from_secs(2);

/// Dev mode as set in Settings.
static DEV: AtomicBool = AtomicBool::new(false);
/// The plugin folder scanner is running.
static SCANNING: AtomicBool = AtomicBool::new(false);
/// The scanner saw plugin folders appear, go or change their manifest.
static FOLDERS_CHANGED: AtomicBool = AtomicBool::new(false);

/// Plugins in the registry: (plugin id, folder).
fn loaded() -> &'static Mutex<Vec<(String, PathBuf)>> {
    static L: OnceLock<Mutex<Vec<(String, PathBuf)>>> = OnceLock::new();
    L.get_or_init(Mutex::default)
}

/// Turns dev mode on or off (the Settings toggle). On: each plugin restarts
/// when a file in its folder is saved. Takes effect on the next frame.
pub fn set_dev_mode(on: bool) {
    DEV.store(on, Ordering::Relaxed);
}

/// True when plugins should reload as their files change: the Settings
/// toggle, `NOOK_PLUGIN_DEV=1` or `--plugin-dev`.
pub fn dev_mode() -> bool {
    static FORCED: OnceLock<bool> = OnceLock::new();
    DEV.load(Ordering::Relaxed)
        || *FORCED.get_or_init(|| {
            std::env::var_os("NOOK_PLUGIN_DEV").is_some_and(|v| v != "0")
                || std::env::args().any(|a| a == "--plugin-dev")
        })
}

/// Finds plugins in the usual folders and registers each as a module.
pub fn register(reg: &mut Registry) {
    let dirs = manifest::search_dirs();
    let found = manifest::discover(&dirs);
    if found.is_empty() {
        tracing::info!("no plugins (looked in {} folders)", dirs.len());
    }
    for f in found {
        tracing::info!(
            "plugin {} {} from {}",
            f.manifest.id,
            f.manifest.version,
            f.dir.display()
        );
        add(reg, f, None);
    }
}

/// Registers a plugin; with `ctx` (after start-up) it is started at once.
fn add(reg: &mut Registry, found: Found, ctx: Option<&egui::Context>) -> &'static str {
    if let Ok(mut l) = loaded().lock() {
        l.retain(|(id, _)| *id != found.manifest.id);
        l.push((found.manifest.id.clone(), found.dir.clone()));
    }
    let mut plugin = Plugin::new(found);
    if let Some(ctx) = ctx {
        plugin.attach(ctx);
    }
    let id = plugin.id();
    reg.register(Box::new(plugin));
    id
}

/// Stops a plugin and takes it out of the registry. Returns its module id.
fn take_out(reg: &mut Registry, id: &str) -> Option<String> {
    if let Ok(mut l) = loaded().lock() {
        l.retain(|(l, _)| l != id);
    }
    let module_id = format!("{ID_PREFIX}{id}");
    let mut old = reg.remove(&module_id)?;
    old.shutdown();
    Some(module_id)
}

/// `plugin.toml` files in the plugin folders and when each last changed.
fn folder_signature() -> Vec<(PathBuf, Option<std::time::SystemTime>)> {
    let mut sig = Vec::new();
    for root in manifest::search_dirs() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for e in entries.filter_map(Result::ok) {
            let toml = e.path().join("plugin.toml");
            if let Ok(meta) = std::fs::metadata(&toml) {
                sig.push((toml, meta.modified().ok()));
            }
        }
    }
    sig.sort();
    sig
}

/// Watches the plugin folders (on its own thread) for plugins that appear,
/// go, or change id: once a second in dev mode, every few seconds otherwise.
fn ensure_scanner(ctx: &egui::Context) {
    if SCANNING.swap(true, Ordering::Relaxed) {
        return;
    }
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new()
        .name("nook-plugin-scan".into())
        .spawn(move || {
            let mut last = folder_signature();
            loop {
                let every = if dev_mode() { 1 } else { 3 };
                std::thread::sleep(Duration::from_secs(every));
                let now = folder_signature();
                if now != last {
                    last = now;
                    FOLDERS_CHANGED.store(true, Ordering::Relaxed);
                    ctx.request_repaint();
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("spawning plugin folder scanner: {e}");
        SCANNING.store(false, Ordering::Relaxed);
    }
}

/// Loads plugin folders that appeared and unloads those that went (or now
/// hold another id), so a new plugin shows up without a restart.
fn resync(reg: &mut Registry, ctx: &egui::Context, out: &mut Applied) {
    let found = manifest::discover(&manifest::search_dirs());
    let before = loaded().lock().map(|l| l.clone()).unwrap_or_default();
    for (id, dir) in &before {
        if !found.iter().any(|f| f.manifest.id == *id && f.dir == *dir)
            && let Some(m) = take_out(reg, id)
        {
            tracing::info!("unloaded plugin {id} (folder went)");
            out.removed.push(m);
        }
    }
    for f in found {
        if !before
            .iter()
            .any(|(id, dir)| *id == f.manifest.id && *dir == f.dir)
        {
            tracing::info!(
                "loaded new plugin {} from {}",
                f.manifest.id,
                f.dir.display()
            );
            out.added.push(add(reg, f, Some(ctx)));
        }
    }
}

/// What [`apply_changes`] did, so the app can update hotkeys.
#[derive(Default)]
pub struct Applied {
    /// Module ids of plugins that were loaded.
    pub added: Vec<&'static str>,
    /// Module ids of plugins that were taken out.
    pub removed: Vec<String>,
}

/// Puts finished installs and uninstalls into effect, live: the old
/// plugin's process stops, its folder is swapped, and the new one starts.
/// Call from the UI thread once per frame.
pub fn apply_changes(reg: &mut Registry, ctx: &egui::Context) -> Applied {
    let mut out = Applied::default();
    for change in install::take_changes() {
        match change {
            install::Change::Install(ready) => {
                out.removed.extend(take_out(reg, &ready.id));
                match install::commit(&ready.staging, &ready.id) {
                    Ok(found) => {
                        let name = found.manifest.name.clone();
                        tracing::info!(
                            "plugin {} installed in {}",
                            found.manifest.id,
                            found.dir.display()
                        );
                        out.added.push(add(reg, found, Some(ctx)));
                        install::finish(ready.request, Ok(name));
                    }
                    Err(e) => {
                        let _ = std::fs::remove_dir_all(&ready.staging);
                        install::finish(ready.request, Err(e));
                    }
                }
            }
            install::Change::Uninstall { id } => {
                out.removed.extend(take_out(reg, &id));
                match install::trash(&id) {
                    Ok(()) => {
                        tracing::info!("plugin {id} removed");
                        install::set_remove_error(&id, None);
                    }
                    Err(e) => {
                        tracing::warn!("removing plugin {id}: {e}");
                        install::set_remove_error(&id, Some(e));
                        // The folder is still there: keep the plugin running.
                        let dir = install::plugins_root().join(&id);
                        if let Some(f) = manifest::discover(&manifest::search_dirs())
                            .into_iter()
                            .find(|f| f.manifest.id == id && f.dir == dir)
                        {
                            out.added.push(add(reg, f, Some(ctx)));
                        }
                    }
                }
            }
        }
    }
    ensure_scanner(ctx);
    if FOLDERS_CHANGED.swap(false, Ordering::Relaxed) {
        resync(reg, ctx, &mut out);
    }
    out
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

enum Status {
    Running,
    Stopped(String),
}

pub struct Plugin {
    found: Found,
    id: &'static str,
    icon: &'static str,
    hotkey: Option<&'static str>,
    ctx: Option<egui::Context>,
    host: Option<Host>,
    /// Dev mode's file watcher, while dev mode is on.
    watch: Option<host::Watch>,
    status: Status,
    restarts: Vec<Instant>,
    restart_at: Option<Instant>,
    stderr: Tail,
    last_bad: Option<String>,

    tile: Option<Node>,
    page: Option<Node>,
    actions: Vec<Action>,
    badge: Option<Badge>,
    active: bool,
    state: ViewState,
    outbox: Vec<Value>,

    page_request: bool,
    show_request: bool,
    close_request: bool,
    page_open: bool,
    /// The page was drawn since the last `on_tick`.
    page_drawn: bool,
    /// What the plugin was last told, to send only changes.
    theme_sent: Option<(bool, Color32)>,
    tile_layout: Option<(TileSize, egui::Vec2)>,
    page_layout: Option<egui::Vec2>,
}

impl Plugin {
    pub fn new(found: Found) -> Self {
        let m = &found.manifest;
        Self {
            id: leak(format!("{ID_PREFIX}{}", m.id)),
            icon: icons::glyph(&m.icon),
            hotkey: m.hotkey.clone().map(leak),
            found,
            ctx: None,
            host: None,
            watch: None,
            status: Status::Running,
            restarts: Vec::new(),
            restart_at: None,
            stderr: Tail::default(),
            last_bad: None,
            tile: None,
            page: None,
            actions: Vec::new(),
            badge: None,
            active: false,
            state: ViewState::default(),
            outbox: Vec::new(),
            page_request: false,
            show_request: false,
            close_request: false,
            page_open: false,
            page_drawn: false,
            theme_sent: None,
            tile_layout: None,
            page_layout: None,
        }
    }

    fn manifest(&self) -> &Manifest {
        &self.found.manifest
    }

    fn start(&mut self) {
        let Some(ctx) = self.ctx.clone() else { return };
        self.host = None;
        let (dark, accent) = self
            .theme_sent
            .unwrap_or((true, nook_ui::theme::FALLBACK_ACCENT));
        let accent = to_hex(accent);
        let host = Host::start(&self.found, dark, &accent, ctx);
        let data = host::data_dir(&self.manifest().id);
        host.send(protocol::msg_hello(&protocol::Hello {
            id: &self.manifest().id,
            data_dir: &data.display().to_string(),
            dark,
            accent: &accent,
        }));
        self.host = Some(host);
        self.status = Status::Running;
        self.restart_at = None;
        self.stderr.clear();
        self.last_bad = None;
        self.state.clear_images();
        // Layout and theme are re-sent to the new process.
        self.tile_layout = None;
        self.page_layout = None;
        if self.page_open {
            self.send(protocol::msg_simple("page_open"));
        }
    }

    fn restart_by_user(&mut self) {
        self.restarts.clear();
        self.start();
    }

    fn send(&mut self, msg: Value) {
        if let Some(h) = &self.host {
            h.send(msg);
        }
    }

    fn flush(&mut self) {
        for m in std::mem::take(&mut self.outbox) {
            self.send(m);
        }
    }

    /// Re-reads `plugin.toml` (dev mode) and restarts.
    fn reload(&mut self) {
        let file = self.found.dir.join("plugin.toml");
        match std::fs::read_to_string(&file)
            .map_err(|e| e.to_string())
            .and_then(|t| Manifest::parse(&t))
        {
            Ok(m) if m.id == self.manifest().id => {
                self.icon = icons::glyph(&m.icon);
                self.found.manifest = m;
            }
            Ok(_) => tracing::warn!("plugin {}: id changed, restart Nook", self.id),
            Err(e) => tracing::warn!("plugin {}: {e}", file.display()),
        }
        tracing::info!("plugin {} reloaded", self.id);
        self.restarts.clear();
        self.start();
    }

    fn handle(&mut self, msg: Out) {
        match msg {
            Out::Tile { view } => self.tile = Some(view),
            Out::Page { view, actions } => {
                self.page = Some(view);
                self.actions = actions;
            }
            Out::Badge { badge } => {
                self.badge = badge.map(|b| match b {
                    BadgeSpec::Ring { progress, label } => Badge::Ring { progress, label },
                    BadgeSpec::Glyph { icon } => Badge::Glyph {
                        icon: icons::glyph(&icon),
                    },
                    BadgeSpec::Pulse => Badge::Pulse,
                });
            }
            Out::Active { active } => self.active = active,
            Out::OpenPage => self.page_request = true,
            Out::ClosePage => self.close_request = true,
            Out::Show => self.show_request = true,
            Out::Copy { text } => {
                if let Err(e) = nook_platform::clipboard::copy_text(&text) {
                    tracing::warn!("plugin {} copy: {e}", self.id);
                }
            }
            Out::Open { target } => {
                if let Err(e) = nook_platform::shell::open(&target) {
                    tracing::warn!("plugin {} open {target:?}: {e}", self.id);
                }
            }
            Out::Log { level, message } => match level.as_str() {
                "error" => tracing::error!(target: "plugin", "[{}] {message}", self.id),
                "warn" => tracing::warn!(target: "plugin", "[{}] {message}", self.id),
                "debug" => tracing::debug!(target: "plugin", "[{}] {message}", self.id),
                _ => tracing::info!(target: "plugin", "[{}] {message}", self.id),
            },
        }
    }

    fn sync_theme(&mut self, theme: &Theme) {
        let now = (theme.dark, theme.accent);
        if self.theme_sent != Some(now) {
            let first = self.theme_sent.is_none();
            self.theme_sent = Some(now);
            if !first || self.host.is_some() {
                self.send(protocol::msg_theme(theme.dark, &to_hex(theme.accent)));
            }
        }
    }

    fn error_text(&self) -> String {
        let mut s = match &self.status {
            Status::Stopped(why) => format!("Stopped: {why}"),
            Status::Running => String::new(),
        };
        if let Some(bad) = &self.last_bad {
            s.push_str(&format!("\nLast bad message: {bad}"));
        }
        let tail = self.stderr.text();
        if !tail.is_empty() {
            s.push_str(&format!("\n\n{tail}"));
        }
        s.trim().to_owned()
    }

    fn render(&mut self, ui: &mut Ui, view: &Node, theme: &Theme, salt: &str) {
        let mut events = Vec::new();
        let mut drag = None;
        Renderer {
            theme,
            state: &mut self.state,
            events: &mut events,
            drag: &mut drag,
            drags: 0,
            dir: &self.found.dir,
            salt,
        }
        .root(ui, view);
        self.outbox.extend(events);
        if let Some(paths) = drag {
            self.drag_out(paths);
        }
    }

    /// Drags a `drag` node's files out of Nook with the shell's own drag (as
    /// from Explorer); blocks until the drop, like the file shelf does.
    fn drag_out(&mut self, paths: Vec<String>) {
        let files: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        let result = nook_platform::files::drag_out(&files);
        // The drag loop ate the button release; tell egui it happened.
        if let Some(main) = nook_platform::window::find_own("Nook") {
            nook_platform::window::post_left_button_up(main);
        }
        use nook_platform::files::DropResult;
        let msg = match result {
            Ok(r) => protocol::msg_dragged(
                &paths,
                match r {
                    DropResult::Copied => "copied",
                    DropResult::Moved => "moved",
                    DropResult::Linked => "linked",
                    DropResult::Cancelled => "cancelled",
                },
                None,
            ),
            Err(e) => {
                tracing::warn!("plugin {} drag out: {e}", self.id);
                protocol::msg_dragged(&paths, "error", Some(&e.to_string()))
            }
        };
        self.outbox.push(msg);
    }
}

impl Module for Plugin {
    fn id(&self) -> &'static str {
        self.id
    }

    fn name(&self) -> &str {
        &self.found.manifest.name
    }

    fn icon(&self) -> &'static str {
        self.icon
    }

    fn size_hint(&self) -> TileSize {
        self.manifest().size
    }

    fn is_active(&self) -> bool {
        self.active
    }

    fn collapsed_badge(&self) -> Option<Badge> {
        self.badge.clone()
    }

    fn attach(&mut self, ctx: &egui::Context) {
        self.ctx = Some(ctx.clone());
        self.start();
    }

    fn on_tick(&mut self) {
        // Dev mode can be switched on and off while Nook runs.
        if dev_mode() != self.watch.is_some() {
            self.watch = match &self.ctx {
                Some(ctx) if dev_mode() => Some(host::watch(self.found.dir.clone(), ctx.clone())),
                _ => None,
            };
        }
        let mut changed = false;
        if let Some(w) = &self.watch {
            while w.changes.try_recv().is_ok() {
                changed = true;
            }
        }
        if changed {
            self.reload();
        }
        let mut events = Vec::new();
        if let Some(h) = &self.host {
            while let Ok(e) = h.events.try_recv() {
                events.push(e);
            }
        }
        for e in events {
            match e {
                Event::Msg(m) => self.handle(m),
                Event::Bad(e) => {
                    tracing::warn!("plugin {}: {e}", self.id);
                    self.last_bad = Some(e);
                }
                Event::Stderr(l) => self.stderr.push(l),
                Event::Exited(why) => {
                    let code = self.host.as_mut().and_then(Host::reap);
                    let why = match code {
                        Some(c) if c != 0 => format!("{why} (exit code {c})"),
                        _ => why,
                    };
                    tracing::warn!("plugin {}: {why}", self.id);
                    // Poll mode keeps running; only a live process stops.
                    if self.manifest().mode == manifest::Mode::Live {
                        self.restarts.retain(|t| t.elapsed() < RESTART_WINDOW);
                        if self.restarts.len() < MAX_RESTARTS {
                            self.restarts.push(Instant::now());
                            self.restart_at = Some(Instant::now() + RESTART_DELAY);
                        }
                    }
                    self.status = Status::Stopped(why);
                }
                Event::Changed => {}
            }
        }
        if self.restart_at.is_some_and(|t| Instant::now() >= t) {
            self.start();
        }
        // The page counts as closed once a frame went by without drawing it.
        // Not by time: an idle page is only redrawn every 350 ms (see
        // `next_repaint`), and a time limit read before the draw flapped it
        // closed and open again, which reset games waiting on a timer.
        if self.page_open && !self.page_drawn {
            self.page_open = false;
            self.send(protocol::msg_simple("page_close"));
        }
        self.page_drawn = false;
    }

    fn next_repaint(&self, _tile_visible: bool) -> Option<Duration> {
        let restart = self
            .restart_at
            .map(|t| t.saturating_duration_since(Instant::now()));
        let page_close = self.page_open.then_some(Duration::from_millis(350));
        match (restart, page_close) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        self.sync_theme(theme);
        let rect = ui.max_rect();
        let size = self.manifest().size;
        if self.tile_layout != Some((size, rect.size())) {
            self.tile_layout = Some((size, rect.size()));
            let name = match size {
                TileSize::Small => "small",
                TileSize::Wide => "wide",
                TileSize::Large => "large",
            };
            self.send(protocol::msg_layout(
                "tile",
                name,
                rect.width(),
                rect.height(),
            ));
        }
        // Under the view, so the view's own controls win.
        let resp = ui.interact(
            rect.expand(8.0),
            ui.id().with("plugin-tile"),
            Sense::click(),
        );
        match (&self.status, self.tile.clone()) {
            (Status::Stopped(_), _) if self.restart_at.is_none() => {
                view::default_tile(
                    ui,
                    self.icon,
                    self.name(),
                    "Stopped · click to restart",
                    theme,
                    ctx.hovered,
                );
                let err = self.error_text();
                if resp.clicked() {
                    self.restart_by_user();
                }
                resp.on_hover_text(err);
                return;
            }
            (_, Some(view)) => {
                let mut child = ui.new_child(UiBuilder::new().max_rect(rect.round_ui()));
                child.set_clip_rect(rect.expand(6.0).intersect(ui.clip_rect()));
                self.render(&mut child, &view, theme, "tile");
            }
            (_, None) => {
                view::default_tile(ui, self.icon, self.name(), "", theme, ctx.hovered);
            }
        }
        if resp.clicked() {
            self.send(protocol::msg_simple("tile_click"));
            if self.page.is_some() {
                self.page_request = true;
            }
        }
        self.flush();
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.page_request)
    }

    fn take_show_request(&mut self) -> bool {
        std::mem::take(&mut self.show_request)
    }

    fn page_size(&self) -> Option<egui::Vec2> {
        self.manifest().page_size.map(|[w, h]| vec2(w, h))
    }

    fn ui_page(&mut self, ui: &mut Ui, header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        self.sync_theme(theme);
        self.page_drawn = true;
        if !self.page_open {
            self.page_open = true;
            self.close_request = false;
            self.send(protocol::msg_simple("page_open"));
        }
        if self.page_layout != Some(body.size()) {
            self.page_layout = Some(body.size());
            self.send(protocol::msg_layout(
                "page",
                "page",
                body.width(),
                body.height(),
            ));
        }
        for a in self.actions.clone() {
            let r = widgets::icon_button(header, icons::glyph(&a.icon), HEADER, false, theme);
            let r = if a.tooltip.is_empty() {
                r
            } else {
                r.on_hover_text(&a.tooltip)
            };
            if r.clicked() {
                self.outbox
                    .push(protocol::msg_event(&a.id, "click", Value::Null));
            }
        }
        let mut child = ui.new_child(UiBuilder::new().max_rect(body.round_ui()));
        if let Status::Stopped(_) = self.status {
            let text = self.error_text();
            child.label(
                egui::RichText::new(text)
                    .font(egui::FontId::monospace(SIZE_LABEL))
                    .color(theme.text_dim),
            );
            child.add_space(8.0);
            let w = 120.0;
            let r = Rect::from_min_size(child.next_widget_position(), vec2(w, 28.0));
            if crate::kit::button(
                &mut child,
                r,
                "Restart",
                Some(icons::glyph("arrow-clockwise")),
                true,
                theme,
            )
            .clicked()
            {
                self.restart_by_user();
            }
        } else if let Some(view) = self.page.clone() {
            self.render(&mut child, &view, theme, "page");
        } else {
            child.painter().text(
                pos2(body.center().x, body.center().y),
                egui::Align2::CENTER_CENTER,
                format!("{} has no page", self.name()),
                Theme::font(SIZE_BODY),
                theme.text_dim,
            );
        }
        self.flush();
        std::mem::take(&mut self.close_request)
    }

    fn default_hotkey(&self) -> Option<&'static str> {
        self.hotkey
    }

    fn on_hotkey(&mut self) {
        self.send(protocol::msg_simple("hotkey"));
    }

    fn accepts_files(&self) -> bool {
        self.manifest().accepts_files && self.page_open
    }

    fn drop_files(&mut self, paths: Vec<PathBuf>) {
        self.send(protocol::msg_files(&paths));
    }

    fn shutdown(&mut self) {
        if let Some(mut h) = self.host.take() {
            h.stop();
        }
    }
}
