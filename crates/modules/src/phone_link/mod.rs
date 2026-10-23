//! Phone link: your Android phone inside the notch.
//!
//! - **Files and text** both ways over the LAN with the LocalSend protocol
//!   ([`localsend`]): the phone only needs the open-source LocalSend app.
//!   Drop files on the open page (or choose them) to send; incoming
//!   transfers ask first, unless auto-accept is on.
//! - **Screen mirroring** with scrcpy ([`mirror`]), shown inside the page.
//!   Click the mirror (or the cursor button) to control the phone with the
//!   mouse and keyboard; **Esc** gives them back.
//! - **Notifications** read over adb ([`adb`]); the pill pulses when new
//!   ones arrive. Click one to open its app on the phone and mirror it.
//! - Back, home, recents, volume and power keys; USB or Wi-Fi (adb connect,
//!   Android 11+ pairing, or switching a USB phone to Wi-Fi in one click).
//!
//! Settings live in `%APPDATA%\Nook\phone.txt`.

mod adb;
mod localsend;
mod mirror;

use crate::file_shelf::human_size;
use adb::{Adb, Device, DeviceInfo, DeviceState, Notif};
use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Align2, Color32, FontId, Id, Pos2, Rect, Response, Sense, Ui, UiBuilder, pos2, vec2};
use localsend::{Direction, LocalSend, Peer, State, Transfer};
use mirror::{Mirror, Status};
use nook_platform::{Hwnd, clipboard, files, shell, window};
use nook_ui::module::{Badge, Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_ICON, SIZE_LABEL, Theme, mix, with_alpha};
use nook_ui::{anim, icons, paint, widgets};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// The page is taller than the regular panel: room for a phone screen.
const PAGE: egui::Vec2 = vec2(800.0, 560.0);
const PHONE_W: f32 = 244.0;
const GAP: f32 = 12.0;
/// A hidden mirror (page closed) is stopped after this long.
const MIRROR_IDLE: Duration = Duration::from_secs(20);

const KEY_BACK: u32 = 4;
const KEY_HOME: u32 = 3;
const KEY_RECENTS: u32 = 187;
const KEY_VOL_UP: u32 = 24;
const KEY_VOL_DOWN: u32 = 25;
const KEY_POWER: u32 = 26;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Files,
    Alerts,
    Setup,
}

/// Persisted choices.
#[derive(Clone, Debug)]
struct Settings {
    fingerprint: String,
    save_dir: PathBuf,
    auto_accept: bool,
    audio: bool,
    screen_off: bool,
    wireless: String,
    target: String,
}

impl Settings {
    fn file() -> Option<PathBuf> {
        std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("Nook").join("phone.txt"))
    }

    fn load() -> Self {
        let text = Self::file()
            .and_then(|f| std::fs::read_to_string(f).ok())
            .unwrap_or_default();
        let get = |key: &str| {
            text.lines()
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| k.trim() == key)
                .map(|(_, v)| v.trim().to_owned())
        };
        let flag = |key: &str, default: bool| get(key).map_or(default, |v| v == "true");
        let s = Self {
            fingerprint: get("fingerprint")
                .filter(|f| !f.is_empty())
                .unwrap_or_else(|| localsend::random_hex(16)),
            save_dir: get("save_dir")
                .filter(|d| !d.is_empty())
                .map(PathBuf::from)
                .or_else(|| shell::downloads_dir().map(|d| d.join("Nook")))
                .unwrap_or_else(|| PathBuf::from(".")),
            auto_accept: flag("auto_accept", false),
            audio: flag("audio", false),
            screen_off: flag("screen_off", false),
            wireless: get("wireless").unwrap_or_default(),
            target: get("target").unwrap_or_default(),
        };
        s.save();
        s
    }

    fn save(&self) {
        let Some(file) = Self::file() else {
            return;
        };
        let text = format!(
            "fingerprint={}\nsave_dir={}\nauto_accept={}\naudio={}\nscreen_off={}\nwireless={}\ntarget={}\n",
            self.fingerprint,
            self.save_dir.display(),
            self.auto_accept,
            self.audio,
            self.screen_off,
            self.wireless,
            self.target,
        );
        if let Some(dir) = file.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&file, text) {
            tracing::warn!("phone link: saving {}: {e}", file.display());
        }
    }
}

struct Incoming {
    transfer: Arc<Transfer>,
    reply: Sender<bool>,
}

struct Message {
    from: String,
    text: String,
}

enum Picked {
    Files(Vec<PathBuf>),
    Folder(Option<PathBuf>),
}

pub struct PhoneLink {
    ctx: Option<egui::Context>,
    settings: Settings,
    ls: Option<LocalSend>,
    peers: Vec<Peer>,
    transfers: Vec<Arc<Transfer>>,
    incoming: Vec<Incoming>,
    messages: Vec<Message>,
    /// A send that needs the receiver's PIN.
    pin: Option<(Peer, Vec<PathBuf>, String)>,
    /// Files dropped before a device was chosen.
    queued: Vec<PathBuf>,
    adb: Option<Adb>,
    adb_missing: bool,
    scrcpy: Option<PathBuf>,
    devices: Vec<Device>,
    serial: Option<String>,
    info: Option<DeviceInfo>,
    notifs: Vec<Notif>,
    known: HashSet<String>,
    notifs_loaded: bool,
    unseen: usize,
    /// Something arrived while the page was closed (files, a message).
    attention: bool,
    mirror: Option<Mirror>,
    mirror_error: Option<String>,
    want_mirror: bool,
    hidden_since: Option<Instant>,
    owner: Option<Hwnd>,
    tab: Tab,
    connect_addr: String,
    pair_code: String,
    toast: Option<(String, Instant)>,
    picker: Option<Receiver<Picked>>,
    /// Pass in which the page was last drawn.
    page_pass: Option<u64>,
    fast: bool,
    open_page: bool,
    show: bool,
}

impl Default for PhoneLink {
    fn default() -> Self {
        let settings = Settings::load();
        Self {
            connect_addr: settings.wireless.clone(),
            settings,
            ctx: None,
            ls: None,
            peers: Vec::new(),
            transfers: Vec::new(),
            incoming: Vec::new(),
            messages: Vec::new(),
            pin: None,
            queued: Vec::new(),
            adb: None,
            adb_missing: false,
            scrcpy: None,
            devices: Vec::new(),
            serial: None,
            info: None,
            notifs: Vec::new(),
            known: HashSet::new(),
            notifs_loaded: false,
            unseen: 0,
            attention: false,
            mirror: None,
            mirror_error: None,
            want_mirror: false,
            hidden_since: None,
            owner: None,
            tab: Tab::default(),
            pair_code: String::new(),
            toast: None,
            picker: None,
            page_pass: None,
            fast: false,
            open_page: false,
            show: false,
        }
    }
}

impl PhoneLink {
    fn page_visible(&self) -> bool {
        match (&self.ctx, self.page_pass) {
            (Some(ctx), Some(p)) => ctx.cumulative_pass_nr().saturating_sub(p) <= 1,
            _ => false,
        }
    }

    fn device(&self) -> Option<&Device> {
        let serial = self.serial.as_ref()?;
        self.devices.iter().find(|d| &d.serial == serial)
    }

    fn ready(&self) -> bool {
        self.device().is_some_and(|d| d.state == DeviceState::Ready)
    }

    fn adb_send(&self, cmd: adb::Cmd) {
        if let Some(a) = &self.adb {
            a.send(cmd);
        }
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.toast = Some((msg.into(), Instant::now()));
    }

    fn target(&self) -> Option<&Peer> {
        self.peers
            .iter()
            .find(|p| p.fingerprint == self.settings.target)
    }

    fn send_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        match (self.target().cloned(), &self.ls) {
            (Some(peer), Some(ls)) => {
                ls.send_files(peer, paths, None);
                self.tab = Tab::Files;
            }
            _ => {
                self.queued.extend(paths);
                self.say("Choose a device to send to");
            }
        }
    }

    fn set_target(&mut self, fingerprint: String) {
        self.settings.target = fingerprint;
        self.settings.save();
        if !self.queued.is_empty() {
            let q = std::mem::take(&mut self.queued);
            self.send_paths(q);
        }
    }

    fn rescan(&mut self) {
        self.peers.clear();
        if let Some(ls) = &self.ls {
            ls.announce();
            ls.deep_scan();
        }
        self.adb_send(adb::Cmd::Refresh);
    }

    fn pick_files(&mut self) {
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("nook-phone-pick".into())
            .spawn(move || {
                let paths = files::pick_files("Send to your phone").unwrap_or_else(|e| {
                    tracing::warn!("phone link: file picker: {e}");
                    Vec::new()
                });
                let _ = tx.send(Picked::Files(paths));
                if let Some(c) = ctx {
                    c.request_repaint();
                }
            });
        if spawned.is_ok() {
            self.picker = Some(rx);
        }
    }

    fn pick_folder(&mut self) {
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        let start = self.settings.save_dir.clone();
        let spawned = std::thread::Builder::new()
            .name("nook-phone-pick".into())
            .spawn(move || {
                let dir =
                    nook_platform::wallpaper::pick_folder("Save received files to", Some(&start))
                        .unwrap_or(None);
                let _ = tx.send(Picked::Folder(dir));
                if let Some(c) = ctx {
                    c.request_repaint();
                }
            });
        if spawned.is_ok() {
            self.picker = Some(rx);
        }
    }

    fn drain_localsend(&mut self) {
        let Some(ls) = &self.ls else {
            return;
        };
        let events: Vec<localsend::Event> = ls.events.try_iter().collect();
        for e in events {
            match e {
                localsend::Event::Peer(p) => {
                    match self
                        .peers
                        .iter_mut()
                        .find(|x| x.fingerprint == p.fingerprint)
                    {
                        Some(x) => *x = p,
                        None => {
                            tracing::info!("phone link: found {} at {}", p.alias, p.ip);
                            self.peers.push(p);
                        }
                    }
                    // Phones first, then by name.
                    self.peers.sort_by(|a, b| {
                        b.is_mobile()
                            .cmp(&a.is_mobile())
                            .then_with(|| a.alias.cmp(&b.alias))
                    });
                    if self.target().is_none()
                        && let Some(first) = self
                            .peers
                            .iter()
                            .find(|p| p.is_mobile())
                            .or(self.peers.first())
                    {
                        let f = first.fingerprint.clone();
                        self.set_target(f);
                    }
                }
                localsend::Event::Incoming { transfer, reply } => {
                    self.transfers.insert(0, transfer.clone());
                    self.incoming.push(Incoming { transfer, reply });
                    self.tab = Tab::Files;
                    self.show = true;
                }
                localsend::Event::Message { from, text } => {
                    self.messages.insert(0, Message { from, text });
                    self.messages.truncate(20);
                    self.tab = Tab::Files;
                    self.attention = true;
                    self.show = true;
                }
                localsend::Event::Transfer(t) => {
                    if t.direction == Direction::Receive {
                        self.attention = true;
                    }
                    self.transfers.insert(0, t);
                }
                localsend::Event::NeedPin { peer, paths } => {
                    self.pin = Some((peer, paths, String::new()));
                    self.tab = Tab::Files;
                }
            }
        }
        // Keep the list short: finished transfers beyond the newest 30 go.
        let mut finished = 0;
        self.transfers.retain(|t| {
            if t.state().finished() {
                finished += 1;
                finished <= 30
            } else {
                true
            }
        });
        self.incoming
            .retain(|i| i.transfer.state() == State::Waiting);
    }

    fn drain_adb(&mut self) {
        let Some(a) = &self.adb else {
            return;
        };
        let events: Vec<adb::Event> = a.events.try_iter().collect();
        for e in events {
            match e {
                adb::Event::Devices(list) => {
                    self.adb_missing = false;
                    self.devices = list;
                    let keep = self
                        .serial
                        .as_ref()
                        .is_some_and(|s| self.devices.iter().any(|d| &d.serial == s));
                    if !keep {
                        // Prefer a ready phone, USB over Wi-Fi.
                        let mut ready: Vec<&Device> = self
                            .devices
                            .iter()
                            .filter(|d| d.state == DeviceState::Ready)
                            .collect();
                        ready.sort_by_key(|d| d.wireless());
                        let pick = ready
                            .first()
                            .map(|d| d.serial.clone())
                            .or_else(|| self.devices.first().map(|d| d.serial.clone()));
                        self.select(pick);
                    } else if self.ready() {
                        // Became authorized: read it now.
                        self.adb_send(adb::Cmd::Refresh);
                    }
                    if self
                        .mirror
                        .as_ref()
                        .is_some_and(|m| !self.devices.iter().any(|d| d.serial == m.serial))
                    {
                        self.mirror = None;
                    }
                }
                adb::Event::Info(serial, info) => {
                    if Some(&serial) == self.serial.as_ref() {
                        self.info = Some(info);
                    }
                }
                adb::Event::Notifications(serial, list) => {
                    if Some(&serial) != self.serial.as_ref() {
                        continue;
                    }
                    let fresh = list.iter().filter(|n| !self.known.contains(&n.key)).count();
                    if self.notifs_loaded && fresh > 0 {
                        self.unseen += fresh;
                    }
                    self.known.extend(list.iter().map(|n| n.key.clone()));
                    self.notifs_loaded = true;
                    self.notifs = list;
                }
                adb::Event::Message(m) => {
                    if m.contains("connected to") {
                        self.settings.wireless = self.connect_addr.trim().to_owned();
                        self.settings.save();
                    }
                    self.say(m);
                }
                adb::Event::Unavailable => self.adb_missing = true,
            }
        }
    }

    fn select(&mut self, serial: Option<String>) {
        if serial == self.serial {
            return;
        }
        self.serial = serial.clone();
        self.info = None;
        self.notifs.clear();
        self.known.clear();
        self.notifs_loaded = false;
        self.unseen = 0;
        self.adb_send(adb::Cmd::Select(serial));
    }

    /// Screen rect (physical pixels) of a rect in the page.
    fn to_screen(&self, ctx: &egui::Context, r: Rect) -> Option<(i32, i32, i32, i32)> {
        let owner = self.owner?;
        let (ox, oy, _, _) = window::window_rect(owner).ok()?;
        let ppp = ctx.pixels_per_point();
        let x = ox + (r.min.x * ppp).round() as i32;
        let y = oy + (r.min.y * ppp).round() as i32;
        Some((
            x,
            y,
            (r.width() * ppp).round() as i32,
            (r.height() * ppp).round() as i32,
        ))
    }

    fn start_mirror(&mut self, ctx: &egui::Context, area: Rect) {
        let (Some(scrcpy), Some(serial)) = (self.scrcpy.clone(), self.serial.clone()) else {
            return;
        };
        if self.owner.is_none() {
            self.owner = window::find_own("Nook");
        }
        let aspect = self.aspect();
        let rect = fit(area, aspect);
        let Some(screen) = self.to_screen(ctx, rect) else {
            return;
        };
        let adb = self.adb.as_ref().and_then(|a| a.exe.clone());
        let opts = mirror::Options {
            audio: self.settings.audio,
            screen_off: self.settings.screen_off,
        };
        match Mirror::start(&scrcpy, adb.as_deref(), &serial, opts, screen, aspect, ctx) {
            Ok(m) => {
                self.mirror = Some(m);
                self.mirror_error = None;
                // Mirroring keeps the panel open.
                self.show = true;
            }
            Err(e) => self.mirror_error = Some(format!("Could not start scrcpy: {e}")),
        }
    }

    fn aspect(&self) -> f32 {
        if let Some(m) = &self.mirror {
            return m.aspect;
        }
        self.info
            .as_ref()
            .and_then(|i| i.screen)
            .map_or(9.0 / 20.0, |(w, h)| w as f32 / h as f32)
    }

    fn running(&self) -> Vec<&Arc<Transfer>> {
        self.transfers
            .iter()
            .filter(|t| t.state() == State::Running)
            .collect()
    }

    fn key(&self, code: u32) {
        self.adb_send(adb::Cmd::Key(code));
    }
}

/// The largest rect of `aspect` (w / h) centered in `area`.
fn fit(area: Rect, aspect: f32) -> Rect {
    let (w, h) = if area.width() / area.height() > aspect {
        (area.height() * aspect, area.height())
    } else {
        (area.width(), area.width() / aspect)
    };
    Rect::from_center_size(area.center(), vec2(w, h))
}

fn wash(theme: &Theme) -> Color32 {
    if theme.dark {
        Color32::from_white_alpha(14)
    } else {
        Color32::from_black_alpha(10)
    }
}

/// A text button: accent-filled when `primary`.
fn button(
    ui: &mut Ui,
    rect: Rect,
    label: &str,
    icon: Option<&str>,
    primary: bool,
    theme: &Theme,
) -> Response {
    let resp = ui.interact(
        rect,
        ui.id()
            .with(("btn", label, rect.min.x as i32, rect.min.y as i32)),
        Sense::click(),
    );
    let hover = anim::tween(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.12);
    let press = if resp.is_pointer_button_down_on() {
        0.97
    } else {
        1.0
    };
    let r = Rect::from_center_size(rect.center(), rect.size() * press);
    let painter = ui.painter();
    let (fill, fg) = if primary {
        (
            mix(theme.accent, Color32::WHITE, 0.12 * hover),
            Color32::WHITE,
        )
    } else {
        (
            with_alpha(
                theme.text,
                if theme.dark { 0.10 } else { 0.07 } + 0.06 * hover,
            ),
            theme.text,
        )
    };
    painter.rect_filled(r, r.height() * 0.5, fill);
    let font = Theme::font_semibold(SIZE_LABEL);
    let galley = painter.layout_no_wrap(label.to_owned(), font, fg);
    let icon_w = if icon.is_some() { 18.0 } else { 0.0 };
    let x0 = r.center().x - (galley.size().x + icon_w) * 0.5;
    if let Some(i) = icon {
        paint::icon(painter, pos2(x0 + 7.0, r.center().y), i, 13.0, fg, primary);
    }
    painter.galley(
        pos2(x0 + icon_w, r.center().y - galley.size().y * 0.5),
        galley,
        fg,
    );
    resp
}

/// Text cut to one line (`…` at the end).
fn one_line(ui: &Ui, pos: Pos2, text: &str, width: f32, font: FontId, color: Color32) -> Rect {
    let mut job =
        LayoutJob::single_section(text.replace('\n', " "), TextFormat::simple(font, color));
    job.wrap = TextWrapping {
        max_width: width.max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    let rect = Rect::from_min_size(pos, galley.size());
    ui.painter().galley(pos, galley, color);
    rect
}

/// Wrapped text, at most `rows` lines.
fn lines(
    ui: &Ui,
    pos: Pos2,
    text: &str,
    width: f32,
    rows: usize,
    font: FontId,
    color: Color32,
) -> f32 {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, color));
    job.wrap = TextWrapping {
        max_width: width.max(1.0),
        max_rows: rows,
        break_anywhere: false,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    let h = galley.size().y;
    ui.painter().galley(pos, galley, color);
    h
}

fn progress_bar(ui: &Ui, rect: Rect, t: f32, theme: &Theme) {
    let p = ui.painter();
    p.rect_filled(rect, rect.height() * 0.5, wash(theme));
    let w = rect.width() * t.clamp(0.0, 1.0);
    if w > 0.5 {
        let fill = Rect::from_min_size(rect.min, vec2(w.max(rect.height()), rect.height()));
        p.rect_filled(fill, rect.height() * 0.5, theme.accent);
    }
}

fn icon_at(
    ui: &mut Ui,
    center: Pos2,
    size: f32,
    glyph: &'static str,
    active: bool,
    theme: &Theme,
) -> Response {
    let rect = Rect::from_center_size(center, vec2(size, size));
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).id_salt((
        glyph,
        center.x as i32,
        center.y as i32,
    )));
    widgets::icon_button(&mut child, glyph, size, active, theme)
}

fn ago(when_ms: i64) -> String {
    if when_ms <= 0 {
        return String::new();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64);
    let s = ((now - when_ms) / 1000).max(0);
    match s {
        0..=59 => "now".into(),
        60..=3599 => format!("{}m", s / 60),
        3600..=86399 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86400),
    }
}

fn speed(bps: f64) -> String {
    format!("{}/s", human_size(bps as u64))
}

impl PhoneLink {
    // ---------------------------------------------------------- phone column

    fn phone_column(&mut self, ui: &mut Ui, col: Rect, theme: &Theme) {
        let ctx = ui.ctx().clone();
        let controls_h = 34.0;
        let screen = Rect::from_min_max(col.min, pos2(col.max.x, col.max.y - controls_h - 8.0));
        widgets::card(ui.painter(), screen, theme);
        let area = screen.shrink(10.0);

        if self.want_mirror && self.ready() && self.mirror.is_none() && self.scrcpy.is_some() {
            self.want_mirror = false;
            self.start_mirror(&ctx, area);
        }

        if self.mirror.is_some() {
            self.mirror_view(ui, area, theme);
        } else {
            self.device_card(ui, area, theme);
        }
        let row = Rect::from_min_max(pos2(col.min.x, col.max.y - controls_h), col.max);
        self.controls(ui, row, theme);
    }

    fn mirror_view(&mut self, ui: &mut Ui, area: Rect, theme: &Theme) {
        let ctx = ui.ctx().clone();
        let aspect = self.aspect();
        let rect = fit(area, aspect);
        let radius = 14.0;
        ui.painter().rect_filled(rect, radius, Color32::BLACK);
        let controlling = self.mirror.as_ref().is_some_and(Mirror::controlling);
        if controlling {
            ui.painter().rect_stroke(
                rect.expand(2.5),
                radius + 2.5,
                egui::Stroke::new(2.0, theme.accent),
                egui::StrokeKind::Middle,
            );
        }
        // Only on full glass: not while fading, nor before the tall page has
        // unrolled down to the phone.
        let settled = ui.opacity() > 0.99 && ui.clip_rect().contains_rect(rect);
        let screen = self.to_screen(&ctx, rect);
        let ppp = ctx.pixels_per_point();
        let Some(m) = self.mirror.as_mut() else {
            return;
        };
        if !m.ready() {
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                "Starting mirror…",
                Theme::font(SIZE_LABEL),
                Color32::from_gray(170),
            );
            return;
        }
        match (settled, screen) {
            (true, Some(s)) => m.place(s, (radius * ppp).round() as i32),
            _ => m.hide(),
        }
        // The window lets clicks through while view-only: a click on it
        // lands here and starts control.
        let resp = ui.interact(rect, Id::new("phone-mirror-surface"), Sense::click());
        if resp.clicked() && !controlling {
            m.set_control(true, Some(&ctx));
        }
        resp.on_hover_text("Click to control your phone with the mouse and keyboard");
    }

    fn device_card(&mut self, ui: &mut Ui, area: Rect, theme: &Theme) {
        let c = area.center();
        let painter = ui.painter().clone();
        let center_text = |y: f32, text: &str, font: FontId, color: Color32| {
            painter.text(pos2(c.x, y), Align2::CENTER_CENTER, text, font, color);
        };
        if self.adb_missing && self.scrcpy.is_none() {
            paint::icon(
                &painter,
                c - vec2(0.0, 70.0),
                icons::PLUG,
                38.0,
                theme.text_faint,
                false,
            );
            center_text(
                c.y - 22.0,
                "Mirroring needs scrcpy",
                Theme::font_semibold(SIZE_BODY),
                theme.text,
            );
            lines(
                ui,
                pos2(area.min.x + 8.0, c.y - 4.0),
                "Free and open source. It brings adb along, for notifications and the phone keys too.",
                area.width() - 16.0,
                4,
                Theme::font(SIZE_LABEL),
                theme.text_dim,
            );
            let b = Rect::from_center_size(pos2(c.x, c.y + 62.0), vec2(150.0, 30.0));
            if button(
                ui,
                b,
                "Install scrcpy",
                Some(icons::DOWNLOAD_SIMPLE),
                true,
                theme,
            )
            .clicked()
            {
                match mirror::install_with_winget() {
                    Ok(()) => self.say("Installing… press Check again when it is done"),
                    Err(e) => self.say(format!("Could not run winget: {e}")),
                }
            }
            let b2 = Rect::from_center_size(pos2(c.x, c.y + 98.0), vec2(150.0, 28.0));
            if button(ui, b2, "Check again", None, false, theme).clicked() {
                self.find_tools();
            }
            return;
        }
        let Some(dev) = self.device().cloned() else {
            paint::icon(
                &painter,
                c - vec2(0.0, 60.0),
                icons::DEVICE_MOBILE_SLASH,
                40.0,
                theme.text_faint,
                false,
            );
            center_text(
                c.y - 10.0,
                "No phone connected",
                Theme::font_semibold(SIZE_BODY),
                theme.text,
            );
            lines(
                ui,
                pos2(area.min.x + 10.0, c.y + 10.0),
                "Plug it in with USB and turn on USB debugging (Developer options), or connect over Wi-Fi in Setup.\n\nFiles work without this: open LocalSend on the phone.",
                area.width() - 20.0,
                9,
                Theme::font(SIZE_LABEL),
                theme.text_dim,
            );
            return;
        };
        match dev.state {
            DeviceState::Unauthorized => {
                paint::icon(
                    &painter,
                    c - vec2(0.0, 50.0),
                    icons::SHIELD_CHECK,
                    40.0,
                    theme.accent_fg(),
                    false,
                );
                center_text(
                    c.y,
                    "Check your phone",
                    Theme::font_semibold(SIZE_BODY),
                    theme.text,
                );
                lines(
                    ui,
                    pos2(area.min.x + 10.0, c.y + 20.0),
                    "Allow USB debugging for this computer on the phone's screen.",
                    area.width() - 20.0,
                    4,
                    Theme::font(SIZE_LABEL),
                    theme.text_dim,
                );
                return;
            }
            DeviceState::Offline | DeviceState::Other(_) => {
                center_text(
                    c.y,
                    "Phone is offline",
                    Theme::font_semibold(SIZE_BODY),
                    theme.text,
                );
                center_text(
                    c.y + 20.0,
                    "Reconnect the cable or Wi-Fi",
                    Theme::font(SIZE_LABEL),
                    theme.text_dim,
                );
                return;
            }
            DeviceState::Ready => {}
        }
        // A phone silhouette, then what we know about it.
        let aspect = self.aspect();
        let frame = fit(
            Rect::from_center_size(c - vec2(0.0, 58.0), vec2(80.0, 150.0)),
            aspect,
        );
        painter.rect_stroke(
            frame,
            12.0,
            egui::Stroke::new(2.0, with_alpha(theme.text, 0.35)),
            egui::StrokeKind::Middle,
        );
        painter.circle_filled(
            pos2(frame.center().x, frame.min.y + 8.0),
            2.5,
            with_alpha(theme.text, 0.35),
        );
        paint::icon(
            &painter,
            frame.center(),
            icons::SCREENCAST,
            26.0,
            theme.text_faint,
            false,
        );
        let name = self
            .info
            .as_ref()
            .map(DeviceInfo::name)
            .or(dev.model.clone())
            .unwrap_or_else(|| dev.serial.clone());
        center_text(
            frame.max.y + 22.0,
            &name,
            Theme::font_semibold(SIZE_BODY + 1.0),
            theme.text,
        );
        let mut facts = Vec::new();
        if let Some(i) = &self.info {
            if !i.android.is_empty() {
                facts.push(format!("Android {}", i.android));
            }
            if let Some(b) = i.battery {
                facts.push(format!("{b}%{}", if i.charging { " ⚡" } else { "" }));
            }
        }
        facts.push(if dev.wireless() {
            "Wi-Fi".into()
        } else {
            "USB".into()
        });
        center_text(
            frame.max.y + 42.0,
            &facts.join("  ·  "),
            Theme::font(SIZE_LABEL),
            theme.text_dim,
        );
        let b = Rect::from_center_size(pos2(c.x, frame.max.y + 78.0), vec2(160.0, 32.0));
        if self.scrcpy.is_some() {
            if button(ui, b, "Mirror screen", Some(icons::SCREENCAST), true, theme).clicked() {
                let ctx = ui.ctx().clone();
                self.start_mirror(&ctx, area);
            }
        } else if button(
            ui,
            b,
            "Get scrcpy to mirror",
            Some(icons::DOWNLOAD_SIMPLE),
            false,
            theme,
        )
        .clicked()
        {
            match mirror::install_with_winget() {
                Ok(()) => self.say("Installing… press Check again in Setup when it is done"),
                Err(e) => self.say(format!("Could not run winget: {e}")),
            }
        }
        if let Some(err) = &self.mirror_error {
            lines(
                ui,
                pos2(area.min.x + 6.0, frame.max.y + 102.0),
                err,
                area.width() - 12.0,
                3,
                Theme::font(SIZE_LABEL - 1.0),
                Color32::from_rgb(255, 120, 110),
            );
        }
    }

    fn controls(&mut self, ui: &mut Ui, row: Rect, theme: &Theme) {
        if !self.ready() {
            return;
        }
        let ctx = ui.ctx().clone();
        let mirroring = self.mirror.as_ref().is_some_and(Mirror::ready);
        let controlling = self.mirror.as_ref().is_some_and(Mirror::controlling);
        let keys: [(&'static str, &'static str, u32); 6] = [
            (icons::CARET_LEFT, "Back", KEY_BACK),
            (icons::CIRCLE, "Home", KEY_HOME),
            (icons::SQUARE, "Recent apps", KEY_RECENTS),
            (icons::SPEAKER_LOW, "Volume down", KEY_VOL_DOWN),
            (icons::SPEAKER_HIGH, "Volume up", KEY_VOL_UP),
            (icons::POWER, "Screen on / off", KEY_POWER),
        ];
        let n = keys.len() + if mirroring { 2 } else { 0 };
        let step = row.width() / n as f32;
        let size = 28.0f32.min(step);
        let cy = row.center().y;
        for (i, (glyph, tip, code)) in keys.iter().enumerate() {
            let cx = row.min.x + step * (i as f32 + 0.5);
            if icon_at(ui, pos2(cx, cy), size, glyph, false, theme)
                .on_hover_text(*tip)
                .clicked()
            {
                self.key(*code);
            }
        }
        if mirroring {
            let cx = row.min.x + step * (keys.len() as f32 + 0.5);
            let tip = if controlling {
                "Controlling: press Esc to give the mouse and keyboard back"
            } else {
                "Control with mouse and keyboard (Esc to stop)"
            };
            if icon_at(
                ui,
                pos2(cx, cy),
                size,
                icons::CURSOR_CLICK,
                controlling,
                theme,
            )
            .on_hover_text(tip)
            .clicked()
                && let Some(m) = self.mirror.as_mut()
            {
                m.set_control(!controlling, Some(&ctx));
            }
            let cx = row.min.x + step * (keys.len() as f32 + 1.5);
            if icon_at(ui, pos2(cx, cy), size, icons::STOP, false, theme)
                .on_hover_text("Stop mirroring")
                .clicked()
            {
                self.mirror = None;
            }
        }
    }

    // ---------------------------------------------------------------- files

    fn files_tab(&mut self, ui: &mut Ui, area: Rect, theme: &Theme) {
        let mut y = area.min.y;
        let x0 = area.min.x;
        let w = area.width();

        // Incoming request first: it is waiting for an answer.
        if let Some(inc) = self.incoming.first() {
            let t = inc.transfer.clone();
            let card = Rect::from_min_size(pos2(x0, y), vec2(w, 78.0));
            widgets::card(ui.painter(), card, theme);
            ui.painter().rect_stroke(
                card,
                nook_ui::theme::TILE_RADIUS,
                egui::Stroke::new(1.5, with_alpha(theme.accent, 0.8)),
                egui::StrokeKind::Inside,
            );
            paint::icon(
                ui.painter(),
                pos2(x0 + 22.0, y + 24.0),
                icons::DOWNLOAD_SIMPLE,
                18.0,
                theme.accent_fg(),
                true,
            );
            one_line(
                ui,
                pos2(x0 + 40.0, y + 14.0),
                &format!("{} wants to send you", t.peer),
                w - 220.0,
                Theme::font_semibold(SIZE_BODY),
                theme.text,
            );
            let summary = match t.files.as_slice() {
                [(name, size)] => format!("{name}  ·  {}", human_size(*size)),
                list => format!(
                    "{} files  ·  {}  ·  {}",
                    list.len(),
                    human_size(t.total),
                    list.iter()
                        .take(3)
                        .map(|f| f.0.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            };
            one_line(
                ui,
                pos2(x0 + 40.0, y + 36.0),
                &summary,
                w - 220.0,
                Theme::font(SIZE_LABEL),
                theme.text_dim,
            );
            let accept = Rect::from_min_size(pos2(card.max.x - 172.0, y + 23.0), vec2(80.0, 30.0));
            let decline = Rect::from_min_size(pos2(card.max.x - 86.0, y + 23.0), vec2(74.0, 30.0));
            if button(ui, accept, "Accept", Some(icons::CHECK), true, theme).clicked() {
                let inc = self.incoming.remove(0);
                let _ = inc.reply.send(true);
            } else if button(ui, decline, "Decline", None, false, theme).clicked() {
                let inc = self.incoming.remove(0);
                let _ = inc.reply.send(false);
            }
            y = card.max.y + GAP;
        }

        if let Some((peer, _, pin)) = &mut self.pin {
            let card = Rect::from_min_size(pos2(x0, y), vec2(w, 52.0));
            widgets::card(ui.painter(), card, theme);
            ui.painter().text(
                pos2(x0 + 14.0, card.center().y),
                Align2::LEFT_CENTER,
                format!("{} asks for a PIN", peer.alias),
                Theme::font_semibold(SIZE_LABEL + 1.0),
                theme.text,
            );
            let field = Rect::from_min_size(pos2(card.max.x - 250.0, y + 13.0), vec2(120.0, 26.0));
            ui.put(
                field,
                egui::TextEdit::singleline(pin)
                    .hint_text("PIN")
                    .password(true)
                    .desired_width(field.width()),
            );
            let send = Rect::from_min_size(pos2(card.max.x - 120.0, y + 11.0), vec2(108.0, 30.0));
            if button(ui, send, "Send", Some(icons::PAPER_PLANE_TILT), true, theme).clicked()
                && let Some((peer, paths, pin)) = self.pin.take()
                && let Some(ls) = &self.ls
            {
                ls.send_files(peer, paths, Some(pin));
            }
            y = card.max.y + GAP;
        }

        // Devices.
        let strip = Rect::from_min_size(pos2(x0, y), vec2(w, 30.0));
        self.device_strip(ui, strip, theme);
        y = strip.max.y + 10.0;

        // Drop zone.
        let zone = Rect::from_min_size(pos2(x0, y), vec2(w, 74.0));
        self.drop_zone(ui, zone, theme);
        y = zone.max.y + GAP;

        // Messages and transfers.
        let list = Rect::from_min_max(pos2(x0, y), area.max);
        self.activity(ui, list, theme);
    }

    fn device_strip(&mut self, ui: &mut Ui, strip: Rect, theme: &Theme) {
        let mut x = strip.min.x;
        let cy = strip.center().y;
        let refresh = icon_at(
            ui,
            pos2(strip.max.x - 13.0, cy),
            26.0,
            icons::ARROWS_CLOCKWISE,
            false,
            theme,
        )
        .on_hover_text("Look for devices again (also scans the whole network)");
        if refresh.clicked() {
            self.rescan();
        }
        if self.peers.is_empty() {
            ui.painter().text(
                pos2(x, cy),
                Align2::LEFT_CENTER,
                "Looking for devices with LocalSend open…",
                Theme::font(SIZE_LABEL + 1.0),
                theme.text_dim,
            );
            return;
        }
        let limit = strip.max.x - 34.0;
        let mut pick = None;
        for p in &self.peers {
            let font = Theme::font_semibold(SIZE_LABEL);
            let gw = ui
                .painter()
                .layout_no_wrap(p.alias.clone(), font.clone(), theme.text)
                .size()
                .x
                .min(140.0);
            let chip = Rect::from_min_size(pos2(x, strip.min.y + 2.0), vec2(gw + 40.0, 26.0));
            if chip.max.x > limit {
                break;
            }
            let selected = p.fingerprint == self.settings.target;
            let resp = ui.interact(chip, Id::new(("peer-chip", &p.fingerprint)), Sense::click());
            let fill = if selected {
                with_alpha(theme.accent, if theme.dark { 0.45 } else { 0.30 })
            } else if resp.hovered() {
                with_alpha(theme.text, 0.12)
            } else {
                wash(theme)
            };
            ui.painter().rect_filled(chip, 13.0, fill);
            let glyph = match p.kind.as_deref() {
                Some("mobile") => icons::DEVICE_MOBILE,
                Some("web") => icons::GLOBE,
                _ => icons::DESKTOP,
            };
            let fg = if selected {
                theme.accent_fg()
            } else {
                theme.text
            };
            paint::icon(
                ui.painter(),
                pos2(chip.min.x + 14.0, chip.center().y),
                glyph,
                13.0,
                fg,
                selected,
            );
            one_line(
                ui,
                pos2(chip.min.x + 26.0, chip.center().y - 7.5),
                &p.alias,
                gw + 2.0,
                font,
                if selected { theme.text } else { theme.text_dim },
            );
            let tip = format!(
                "{}{}\n{}:{}",
                p.alias,
                p.model
                    .as_ref()
                    .map(|m| format!(" · {m}"))
                    .unwrap_or_default(),
                p.ip,
                p.port
            );
            if resp.on_hover_text(tip).clicked() {
                pick = Some(p.fingerprint.clone());
            }
            x = chip.max.x + 6.0;
        }
        if let Some(f) = pick {
            self.set_target(f);
        }
    }

    fn drop_zone(&mut self, ui: &mut Ui, zone: Rect, theme: &Theme) {
        let hovering = ui.input(|i| !i.raw.hovered_files.is_empty());
        widgets::card(ui.painter(), zone, theme);
        if hovering {
            ui.painter().rect_filled(
                zone,
                nook_ui::theme::TILE_RADIUS,
                with_alpha(theme.accent, 0.18),
            );
            ui.painter().rect_stroke(
                zone,
                nook_ui::theme::TILE_RADIUS,
                egui::Stroke::new(1.5, theme.accent),
                egui::StrokeKind::Inside,
            );
        }
        let target = self.target().map(|p| p.alias.clone());
        let icon_c = pos2(zone.min.x + 30.0, zone.center().y);
        paint::icon(
            ui.painter(),
            icon_c,
            if hovering {
                icons::DOWNLOAD_SIMPLE
            } else {
                icons::UPLOAD_SIMPLE
            },
            22.0,
            if hovering {
                theme.accent_fg()
            } else {
                theme.text_dim
            },
            hovering,
        );
        let title = match (&target, hovering) {
            (Some(t), true) => format!("Drop to send to {t}"),
            (Some(t), false) => format!("Drop files here to send to {t}"),
            (None, _) => "Drop files here".to_owned(),
        };
        let text_w = zone.width() - 60.0 - 230.0;
        one_line(
            ui,
            pos2(zone.min.x + 52.0, zone.center().y - 17.0),
            &title,
            text_w,
            Theme::font_semibold(SIZE_BODY),
            theme.text,
        );
        let sub = if target.is_some() {
            "Folders keep their structure. Phones need the LocalSend app open."
        } else {
            "Open LocalSend on your phone, then pick it above."
        };
        lines(
            ui,
            pos2(zone.min.x + 52.0, zone.center().y + 2.0),
            sub,
            text_w,
            2,
            Theme::font(SIZE_LABEL - 1.0),
            theme.text_dim,
        );
        let choose = Rect::from_min_size(
            pos2(zone.max.x - 228.0, zone.center().y - 15.0),
            vec2(104.0, 30.0),
        );
        if button(ui, choose, "Choose files", Some(icons::FILES), true, theme).clicked() {
            self.pick_files();
        }
        let clip = Rect::from_min_size(
            pos2(zone.max.x - 118.0, zone.center().y - 15.0),
            vec2(106.0, 30.0),
        );
        if button(
            ui,
            clip,
            "Send clipboard",
            Some(icons::CLIPBOARD_TEXT),
            false,
            theme,
        )
        .on_hover_text("Send the text on your clipboard as a message")
        .clicked()
        {
            match (clipboard::read_text(), self.target().cloned(), &self.ls) {
                (Some(text), Some(peer), Some(ls)) if !text.trim().is_empty() => {
                    ls.send_text(peer, text);
                    self.say("Clipboard sent");
                }
                (_, None, _) => self.say("Choose a device to send to"),
                _ => self.say("The clipboard has no text"),
            }
        }
    }

    fn activity(&mut self, ui: &mut Ui, list: Rect, theme: &Theme) {
        if self.messages.is_empty() && self.transfers.is_empty() {
            ui.painter().text(
                pos2(list.center().x, list.min.y + 40.0),
                Align2::CENTER_CENTER,
                "Transfers and messages show up here.",
                Theme::font(SIZE_LABEL + 1.0),
                theme.text_faint,
            );
            return;
        }
        let row_h = 50.0;
        let msg_h = 58.0;
        let total = self.messages.len() as f32 * (msg_h + 6.0)
            + self.transfers.len() as f32 * (row_h + 6.0);
        let mut copy = None;
        let mut reveal = None;
        let mut dismiss_msg = None;
        let mut builder = ui.new_child(UiBuilder::new().max_rect(list));
        egui::ScrollArea::vertical()
            .id_salt("phone-activity")
            .max_height(list.height())
            .auto_shrink([false, false])
            .show(&mut builder, |ui| {
                let (area, _) = ui.allocate_exact_size(vec2(list.width(), total), Sense::hover());
                let mut y = area.min.y;
                for (i, m) in self.messages.iter().enumerate() {
                    let card =
                        Rect::from_min_size(pos2(area.min.x, y), vec2(area.width() - 8.0, msg_h));
                    y += msg_h + 6.0;
                    if !ui.is_rect_visible(card) {
                        continue;
                    }
                    widgets::card(ui.painter(), card, theme);
                    paint::icon(
                        ui.painter(),
                        pos2(card.min.x + 20.0, card.min.y + 18.0),
                        icons::CHAT_TEXT,
                        16.0,
                        theme.accent_fg(),
                        true,
                    );
                    one_line(
                        ui,
                        pos2(card.min.x + 36.0, card.min.y + 9.0),
                        &format!("Message from {}", m.from),
                        card.width() - 120.0,
                        Theme::font_semibold(SIZE_LABEL),
                        theme.text_dim,
                    );
                    lines(
                        ui,
                        pos2(card.min.x + 36.0, card.min.y + 25.0),
                        &m.text,
                        card.width() - 120.0,
                        2,
                        Theme::font(SIZE_LABEL + 1.0),
                        theme.text,
                    );
                    if icon_at(
                        ui,
                        pos2(card.max.x - 50.0, card.center().y),
                        26.0,
                        icons::COPY,
                        false,
                        theme,
                    )
                    .on_hover_text("Copy")
                    .clicked()
                    {
                        copy = Some(m.text.clone());
                    }
                    if icon_at(
                        ui,
                        pos2(card.max.x - 20.0, card.center().y),
                        24.0,
                        icons::X,
                        false,
                        theme,
                    )
                    .on_hover_text("Dismiss")
                    .clicked()
                    {
                        dismiss_msg = Some(i);
                    }
                }
                for t in &self.transfers {
                    let card =
                        Rect::from_min_size(pos2(area.min.x, y), vec2(area.width() - 8.0, row_h));
                    y += row_h + 6.0;
                    if !ui.is_rect_visible(card) {
                        continue;
                    }
                    if let Some(p) = transfer_row(ui, card, t, theme) {
                        reveal = Some(p);
                    }
                }
            });
        if let Some(text) = copy {
            match clipboard::copy_text(&text) {
                Ok(()) => self.say("Copied"),
                Err(e) => self.say(format!("Could not copy: {e}")),
            }
        }
        if let Some(i) = dismiss_msg {
            self.messages.remove(i);
        }
        if let Some(path) = reveal
            && let Err(e) = shell::reveal(&path)
        {
            tracing::warn!("phone link: reveal: {e}");
        }
    }

    // -------------------------------------------------------- notifications

    fn alerts_tab(&mut self, ui: &mut Ui, area: Rect, theme: &Theme) {
        self.unseen = 0;
        if !self.ready() {
            let c = area.center();
            paint::icon(
                ui.painter(),
                c - vec2(0.0, 30.0),
                icons::BELL_SLASH,
                30.0,
                theme.text_faint,
                false,
            );
            ui.painter().text(
                c + vec2(0.0, 10.0),
                Align2::CENTER_CENTER,
                "Connect your phone over USB or Wi-Fi to see its notifications.",
                Theme::font(SIZE_LABEL + 1.0),
                theme.text_dim,
            );
            return;
        }
        if self.notifs.is_empty() {
            let c = area.center();
            paint::icon(
                ui.painter(),
                c - vec2(0.0, 30.0),
                icons::BELL,
                30.0,
                theme.text_faint,
                false,
            );
            ui.painter().text(
                c + vec2(0.0, 10.0),
                Align2::CENTER_CENTER,
                if self.notifs_loaded {
                    "No notifications. All caught up."
                } else {
                    "Reading notifications…"
                },
                Theme::font(SIZE_LABEL + 1.0),
                theme.text_dim,
            );
            return;
        }
        let row_h = 58.0;
        let total = self.notifs.len() as f32 * (row_h + 6.0);
        let mut open = None;
        let mut builder = ui.new_child(UiBuilder::new().max_rect(area));
        egui::ScrollArea::vertical()
            .id_salt("phone-alerts")
            .max_height(area.height())
            .auto_shrink([false, false])
            .show(&mut builder, |ui| {
                let (list, _) = ui.allocate_exact_size(vec2(area.width(), total), Sense::hover());
                for (i, n) in self.notifs.iter().enumerate() {
                    let card = Rect::from_min_size(
                        pos2(list.min.x, list.min.y + i as f32 * (row_h + 6.0)),
                        vec2(list.width() - 8.0, row_h),
                    );
                    if !ui.is_rect_visible(card) {
                        continue;
                    }
                    let resp = ui.interact(card, Id::new(("phone-notif", &n.key)), Sense::click());
                    widgets::card(ui.painter(), card, theme);
                    if resp.hovered() {
                        ui.painter()
                            .rect_filled(card, nook_ui::theme::TILE_RADIUS, wash(theme));
                    }
                    let badge = pos2(card.min.x + 22.0, card.center().y);
                    ui.painter()
                        .circle_filled(badge, 13.0, with_alpha(theme.accent, 0.25));
                    let initial = n
                        .app
                        .chars()
                        .next()
                        .unwrap_or('?')
                        .to_uppercase()
                        .to_string();
                    ui.painter().text(
                        badge,
                        Align2::CENTER_CENTER,
                        initial,
                        Theme::font_semibold(SIZE_BODY),
                        theme.accent_fg(),
                    );
                    let tx = card.min.x + 44.0;
                    let tw = card.width() - 60.0;
                    let head = format!("{}  ·  {}", n.app, ago(n.when));
                    one_line(
                        ui,
                        pos2(tx, card.min.y + 8.0),
                        &head,
                        tw,
                        Theme::font(SIZE_LABEL - 1.0),
                        theme.text_faint,
                    );
                    one_line(
                        ui,
                        pos2(tx, card.min.y + 22.0),
                        &n.title,
                        tw,
                        Theme::font_semibold(SIZE_LABEL + 1.0),
                        theme.text,
                    );
                    one_line(
                        ui,
                        pos2(tx, card.min.y + 38.0),
                        &n.text,
                        tw,
                        Theme::font(SIZE_LABEL),
                        theme.text_dim,
                    );
                    if resp
                        .on_hover_text("Open on the phone and mirror it")
                        .clicked()
                    {
                        open = Some(n.package.clone());
                    }
                }
            });
        if let Some(pkg) = open {
            self.adb_send(adb::Cmd::OpenApp(pkg));
            if self.mirror.is_none() {
                self.want_mirror = true;
            }
        }
    }

    // ---------------------------------------------------------------- setup

    fn setup_tab(&mut self, ui: &mut Ui, area: Rect, theme: &Theme) {
        let mut builder = ui.new_child(UiBuilder::new().max_rect(area));
        egui::ScrollArea::vertical()
            .id_salt("phone-setup")
            .max_height(area.height())
            .auto_shrink([false, false])
            .show(&mut builder, |ui| {
                ui.set_width(area.width() - 10.0);
                ui.spacing_mut().item_spacing.y = 8.0;
                let heading = |ui: &mut Ui, text: &str| {
                    ui.add_space(2.0);
                    ui.label(egui::RichText::new(text).font(Theme::font_semibold(SIZE_LABEL)).color(theme.text_faint));
                };
                let dim = |ui: &mut Ui, text: &str| {
                    ui.label(egui::RichText::new(text).font(Theme::font(SIZE_LABEL)).color(theme.text_dim));
                };

                heading(ui, "RECEIVING FILES");
                match &self.ls {
                    Some(ls) => match &ls.server_error {
                        None => dim(ui, &format!("Visible as \"{}\" on port {}. Allow Nook through the Windows firewall if asked.", ls.alias(), ls.port())),
                        Some(e) => dim(ui, &format!("Receiving is off: {e}")),
                    },
                    None => dim(ui, "Starting…"),
                }
                let mut auto = self.settings.auto_accept;
                if setting_row(ui, "Accept incoming files without asking", &mut auto, theme) {
                    self.settings.auto_accept = auto;
                    self.settings.save();
                    if let Some(ls) = &self.ls {
                        ls.set_auto_accept(auto);
                    }
                }
                ui.horizontal(|ui| {
                    let path = self.settings.save_dir.display().to_string();
                    ui.label(egui::RichText::new(format!("Save to  {path}")).font(Theme::font(SIZE_LABEL)).color(theme.text));
                });
                ui.horizontal(|ui| {
                    if ui.button("Change folder").clicked() {
                        self.pick_folder();
                    }
                    if ui.button("Open folder").clicked() {
                        let _ = std::fs::create_dir_all(&self.settings.save_dir);
                        if let Err(e) = shell::open(&self.settings.save_dir.to_string_lossy()) {
                            tracing::warn!("phone link: open folder: {e}");
                        }
                    }
                });

                ui.add_space(4.0);
                heading(ui, "MIRRORING");
                match &self.scrcpy {
                    Some(p) => dim(ui, &format!("scrcpy: {}", p.display())),
                    None => {
                        dim(ui, "scrcpy is not installed. It is free and open source, and brings adb along.");
                        ui.horizontal(|ui| {
                            if ui.button("Install with WinGet").clicked() {
                                match mirror::install_with_winget() {
                                    Ok(()) => self.say("Installing… press Check again when it is done"),
                                    Err(e) => self.say(format!("Could not run winget: {e}")),
                                }
                            }
                            if ui.button("Check again").clicked() {
                                self.find_tools();
                            }
                        });
                    }
                }
                let mut audio = self.settings.audio;
                if setting_row(ui, "Play the phone's sound on this PC (Android 11+)", &mut audio, theme) {
                    self.settings.audio = audio;
                    self.settings.save();
                }
                let mut off = self.settings.screen_off;
                if setting_row(ui, "Turn the phone's screen off while mirroring", &mut off, theme) {
                    self.settings.screen_off = off;
                    self.settings.save();
                }
                dim(ui, "Clipboard syncs both ways while mirroring. Changes apply the next time mirroring starts.");

                ui.add_space(4.0);
                heading(ui, "WI-FI CONNECTION (ADB)");
                if self.adb_missing {
                    dim(ui, "adb is not available. Install scrcpy (above), which includes it.");
                }
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut self.connect_addr).hint_text("192.168.1.20:5555").desired_width(170.0));
                    ui.add(egui::TextEdit::singleline(&mut self.pair_code).hint_text("Pairing code").desired_width(100.0));
                    let addr = self.connect_addr.trim().to_owned();
                    if ui.add_enabled(!addr.is_empty(), egui::Button::new("Connect")).clicked() {
                        self.adb_send(adb::Cmd::Connect(addr.clone()));
                    }
                    let code = self.pair_code.trim().to_owned();
                    if ui.add_enabled(!addr.is_empty() && !code.is_empty(), egui::Button::new("Pair")).clicked() {
                        self.adb_send(adb::Cmd::Pair(addr, code));
                    }
                });
                dim(ui, "Android 11+: Developer options > Wireless debugging. Pair with the code's address, then connect to the address on the main screen.");
                let usb = self.device().is_some_and(|d| d.state == DeviceState::Ready && !d.wireless());
                if ui
                    .add_enabled(usb, egui::Button::new("Switch the USB phone to Wi-Fi"))
                    .on_hover_text("Runs adb tcpip 5555 and connects over Wi-Fi; then unplug the cable")
                    .clicked()
                {
                    self.adb_send(adb::Cmd::GoWireless);
                    self.say("Switching to Wi-Fi…");
                }
                if let Some(d) = self.device().filter(|d| d.wireless()).cloned()
                    && ui.button(format!("Disconnect {}", d.serial)).clicked()
                {
                    self.adb_send(adb::Cmd::Disconnect(d.serial));
                }
                if self.devices.len() > 1 {
                    ui.add_space(4.0);
                    heading(ui, "PHONES");
                    let mut pick = None;
                    for d in &self.devices {
                        let label = format!(
                            "{}  {}",
                            d.model.clone().unwrap_or_else(|| d.serial.clone()),
                            if d.wireless() { "(Wi-Fi)" } else { "(USB)" }
                        );
                        if ui.selectable_label(Some(&d.serial) == self.serial.as_ref(), label).clicked() {
                            pick = Some(d.serial.clone());
                        }
                    }
                    if let Some(p) = pick {
                        self.mirror = None;
                        self.select(Some(p));
                    }
                }
            });
    }

    fn find_tools(&mut self) {
        self.scrcpy = mirror::find_scrcpy();
        let exe = mirror::find_adb(self.scrcpy.as_deref());
        let restart = self.adb.as_ref().is_none_or(|a| a.exe != exe);
        if restart && let Some(ctx) = &self.ctx {
            self.adb = Some(Adb::start(exe, ctx.clone()));
            self.adb_missing = false;
            self.serial = None;
            self.devices.clear();
        }
    }
}

/// A switch on the right of a label. Returns true when flipped.
fn setting_row(ui: &mut Ui, label: &str, on: &mut bool, theme: &Theme) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(label)
                .font(Theme::font(SIZE_LABEL + 1.0))
                .color(theme.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = widgets::toggle(ui, on, theme).changed();
        });
    });
    changed
}

/// One transfer row. Returns a path to show in Explorer when clicked.
fn transfer_row(ui: &mut Ui, card: Rect, t: &Arc<Transfer>, theme: &Theme) -> Option<PathBuf> {
    widgets::card(ui.painter(), card, theme);
    let state = t.state();
    let sending = t.direction == Direction::Send;
    let glyph = if sending {
        icons::ARROW_UP
    } else {
        icons::ARROW_DOWN
    };
    let ok = matches!(state, State::Done);
    let failed = matches!(state, State::Failed(_) | State::Declined | State::Cancelled);
    let icon_color = if failed {
        Color32::from_rgb(255, 120, 110)
    } else if ok {
        Color32::from_rgb(98, 214, 132)
    } else {
        theme.accent_fg()
    };
    paint::icon(
        ui.painter(),
        pos2(card.min.x + 20.0, card.center().y),
        if ok { icons::CHECK } else { glyph },
        16.0,
        icon_color,
        true,
    );
    let title = match t.files.as_slice() {
        [(name, _)] => name.clone(),
        list => format!("{} files", list.len()),
    };
    let x = card.min.x + 38.0;
    let w = card.width() - 90.0;
    one_line(
        ui,
        pos2(x, card.min.y + 8.0),
        &title,
        w * 0.62,
        Theme::font_semibold(SIZE_LABEL + 1.0),
        theme.text,
    );
    let who = if sending {
        format!("to {}", t.peer)
    } else {
        format!("from {}", t.peer)
    };
    let done = t.done.load(std::sync::atomic::Ordering::Relaxed);
    let status = match &state {
        State::Waiting if sending => format!("{who}  ·  waiting for them to accept"),
        State::Waiting => format!("{who}  ·  waiting for you"),
        State::NeedPin => format!("{who}  ·  PIN needed"),
        State::Running => format!(
            "{who}  ·  {} of {}  ·  {}",
            human_size(done),
            human_size(t.total),
            speed(t.speed())
        ),
        State::Done => format!("{who}  ·  {}", human_size(t.total)),
        State::Declined => format!("{who}  ·  declined"),
        State::Cancelled => format!("{who}  ·  cancelled"),
        State::Failed(e) => format!("{who}  ·  {e}"),
    };
    let meta_x = x + w * 0.62 + 8.0;
    one_line(
        ui,
        pos2(meta_x, card.min.y + 9.0),
        &status_short(&state, t),
        card.max.x - 50.0 - meta_x,
        Theme::font(SIZE_LABEL - 1.0),
        theme.text_faint,
    );
    one_line(
        ui,
        pos2(x, card.min.y + 25.0),
        &status,
        w,
        Theme::font(SIZE_LABEL - 1.0),
        theme.text_dim,
    );
    if matches!(state, State::Running | State::Waiting) {
        let bar = Rect::from_min_size(pos2(x, card.max.y - 9.0), vec2(w, 3.0));
        progress_bar(ui, bar, t.progress(), theme);
    }
    let action = pos2(card.max.x - 22.0, card.center().y);
    if !state.finished() {
        if icon_at(ui, action, 26.0, icons::X, false, theme)
            .on_hover_text("Cancel")
            .clicked()
        {
            t.cancel();
        }
    } else if !sending && ok {
        let first = t.saved.lock().ok().and_then(|s| s.first().cloned());
        if let Some(p) = first
            && icon_at(ui, action, 26.0, icons::FOLDER_OPEN, false, theme)
                .on_hover_text("Show in Explorer")
                .clicked()
        {
            return Some(p);
        }
    }
    None
}

fn status_short(state: &State, t: &Transfer) -> String {
    match state {
        State::Running => format!("{:.0}%", t.progress() * 100.0),
        _ => String::new(),
    }
}

impl Module for PhoneLink {
    fn id(&self) -> &'static str {
        "phone_link"
    }

    fn name(&self) -> &str {
        "Phone link"
    }

    fn icon(&self) -> &'static str {
        icons::DEVICE_MOBILE
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn attach(&mut self, ctx: &egui::Context) {
        self.ctx = Some(ctx.clone());
        let alias = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Nook".into());
        let _ = std::fs::create_dir_all(&self.settings.save_dir);
        self.ls = Some(LocalSend::start(
            ctx.clone(),
            alias,
            self.settings.fingerprint.clone(),
            self.settings.save_dir.clone(),
            self.settings.auto_accept,
        ));
        self.find_tools();
        // Reconnect the last Wi-Fi phone, quietly.
        if !self.settings.wireless.is_empty() {
            let addr = self.settings.wireless.clone();
            self.adb_send(adb::Cmd::Connect(addr));
        }
    }

    fn is_active(&self) -> bool {
        self.mirror.is_some() || !self.running().is_empty() || !self.incoming.is_empty()
    }

    fn collapsed_badge(&self) -> Option<Badge> {
        let running = self.running();
        if !running.is_empty() {
            let total: u64 = running.iter().map(|t| t.total).sum();
            let done: u64 = running
                .iter()
                .map(|t| t.done.load(std::sync::atomic::Ordering::Relaxed))
                .sum();
            let p = if total == 0 {
                0.0
            } else {
                (done as f64 / total as f64) as f32
            };
            return Some(Badge::Ring {
                progress: p,
                label: format!("{:.0}%", p * 100.0),
            });
        }
        (self.unseen > 0 || self.attention || !self.incoming.is_empty()).then_some(Badge::Pulse)
    }

    fn on_tick(&mut self) {
        self.drain_localsend();
        self.drain_adb();
        if let Some(rx) = &self.picker
            && let Ok(p) = rx.try_recv()
        {
            self.picker = None;
            match p {
                Picked::Files(paths) => self.send_paths(paths),
                Picked::Folder(Some(dir)) => {
                    self.settings.save_dir = dir.clone();
                    self.settings.save();
                    if let Some(ls) = &self.ls {
                        ls.set_save_dir(dir);
                    }
                }
                Picked::Folder(None) => {}
            }
        }
        let visible = self.page_visible();
        if visible {
            self.attention = false;
        }
        if visible != self.fast {
            self.fast = visible;
            self.adb_send(adb::Cmd::Fast(visible));
        }
        if self.owner.is_none() && self.mirror.is_some() {
            self.owner = window::find_own("Nook");
        }
        let owner = self.owner;
        let mut ended = None;
        if let Some(m) = self.mirror.as_mut() {
            match m.tick(owner) {
                Status::Exited(msg) => ended = Some(msg),
                Status::Starting | Status::Running => {}
            }
            if m.take_esc() {
                m.set_control(false, None);
            }
            // Leaving for another app ends control too.
            if m.lost_focus(owner) {
                m.set_control(false, None);
            }
            if !visible {
                m.hide();
            }
        }
        if let Some(msg) = ended {
            tracing::info!("phone link: mirror ended: {msg}");
            self.mirror = None;
            self.mirror_error = Some(msg);
        }
        if visible || self.mirror.is_none() {
            self.hidden_since = None;
        } else if self.hidden_since.get_or_insert_with(Instant::now).elapsed() > MIRROR_IDLE {
            tracing::info!("phone link: mirror hidden for a while, stopping it");
            self.mirror = None;
            self.hidden_since = None;
        }
        if self
            .toast
            .as_ref()
            .is_some_and(|(_, t)| t.elapsed() > Duration::from_secs(5))
        {
            self.toast = None;
        }
    }

    fn next_repaint(&self, _tile_visible: bool) -> Option<Duration> {
        let mut next: Option<Duration> = None;
        let mut want = |d: Duration| next = Some(next.map_or(d, |n| n.min(d)));
        if let Some(m) = &self.mirror {
            if !m.ready() {
                want(Duration::from_millis(100));
            } else if m.controlling() {
                want(Duration::from_millis(200));
            } else if self.hidden_since.is_some() {
                want(Duration::from_secs(1));
            } else {
                // Follow rotation.
                want(Duration::from_millis(500));
            }
        }
        if let Some((_, t)) = &self.toast {
            want(Duration::from_secs(5).saturating_sub(t.elapsed()) + Duration::from_millis(20));
        }
        if self.picker.is_some() {
            want(Duration::from_millis(250));
        }
        next
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.open_page)
    }

    fn take_show_request(&mut self) -> bool {
        std::mem::take(&mut self.show)
    }

    fn accepts_files(&self) -> bool {
        self.page_visible()
    }

    fn drop_files(&mut self, paths: Vec<PathBuf>) {
        self.send_paths(paths);
    }

    fn page_size(&self) -> Option<egui::Vec2> {
        Some(PAGE)
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        let rect = ui.max_rect();
        let resp = ui.interact(rect.expand(8.0), ui.id().with("phone-tile"), Sense::click());
        let active = self.is_active();
        let color = if active {
            theme.accent_fg()
        } else if ctx.hovered {
            theme.text
        } else {
            theme.text_dim
        };
        let glyph = if self.mirror.is_some() {
            icons::SCREENCAST
        } else {
            icons::DEVICE_MOBILE
        };
        paint::icon(
            ui.painter(),
            rect.center(),
            glyph,
            SIZE_ICON + 2.0,
            color,
            active,
        );
        // Green dot: a phone is connected over adb.
        if self.ready() {
            let c = rect.center() + vec2(-11.0, 10.0);
            ui.painter()
                .circle_filled(c, 3.5, Color32::from_rgb(98, 214, 132));
        }
        let count = self.unseen + self.incoming.len();
        if count > 0 {
            let c = rect.center() + vec2(11.0, -9.0);
            ui.painter().circle_filled(c, 7.5, theme.accent);
            ui.painter().text(
                c,
                Align2::CENTER_CENTER,
                count.min(99),
                Theme::font_semibold(SIZE_LABEL - 2.0),
                Color32::WHITE,
            );
        }
        if resp.clicked() || resp.secondary_clicked() {
            self.open_page = true;
        }
        let tip = match (self.device(), &self.info) {
            (Some(d), Some(i)) if d.state == DeviceState::Ready => format!(
                "{}{}  ·  Click for files, mirroring and notifications",
                i.name(),
                i.battery.map(|b| format!(" · {b}%")).unwrap_or_default()
            ),
            _ => "Send files, mirror your phone, see its notifications".to_owned(),
        };
        resp.on_hover_text(tip);
    }

    fn ui_page(&mut self, ui: &mut Ui, header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        let ctx = ui.ctx().clone();
        self.page_pass = Some(ctx.cumulative_pass_nr());

        // Header: tabs (right to left).
        let alerts = if self.unseen > 0 {
            format!("Alerts ({})", self.unseen)
        } else if !self.notifs.is_empty() {
            format!("Alerts · {}", self.notifs.len())
        } else {
            "Alerts".to_owned()
        };
        let mut idx = match self.tab {
            Tab::Files => 0,
            Tab::Alerts => 1,
            Tab::Setup => 2,
        };
        if widgets::segmented(header, &["Files", &alerts, "Setup"], &mut idx, theme).changed() {
            self.tab = [Tab::Files, Tab::Alerts, Tab::Setup][idx];
        }

        let phone = Rect::from_min_size(body.min, vec2(PHONE_W, body.height()));
        self.phone_column(ui, phone, theme);

        let right = Rect::from_min_max(pos2(phone.max.x + GAP + 4.0, body.min.y), body.max);
        let toast_h = if self.toast.is_some() { 30.0 } else { 0.0 };
        let content = Rect::from_min_max(right.min, pos2(right.max.x, right.max.y - toast_h));
        match self.tab {
            Tab::Files => self.files_tab(ui, content, theme),
            Tab::Alerts => self.alerts_tab(ui, content, theme),
            Tab::Setup => self.setup_tab(ui, content, theme),
        }
        if let Some((msg, at)) = &self.toast {
            let a = (1.0 - (at.elapsed().as_secs_f32() - 4.5).max(0.0) * 2.0).clamp(0.0, 1.0);
            let r = Rect::from_min_max(pos2(right.min.x, right.max.y - 26.0), right.max);
            ui.painter()
                .rect_filled(r, 13.0, with_alpha(theme.text, 0.10 * a));
            one_line(
                ui,
                pos2(r.min.x + 12.0, r.center().y - 7.5),
                msg,
                r.width() - 24.0,
                Theme::font(SIZE_LABEL),
                with_alpha(theme.text, a),
            );
        }
        false
    }

    fn shutdown(&mut self) {
        self.mirror = None;
    }
}
