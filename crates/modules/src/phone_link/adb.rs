//! A small client for the adb server (the one `adb` and scrcpy share), over
//! its socket protocol on `127.0.0.1:5037` rather than by running `adb.exe`
//! for every command.
//!
//! - `host:track-devices-l` keeps one connection open that the server
//!   writes to whenever a phone is plugged in, authorized, or goes away: no
//!   polling for devices.
//! - A worker thread runs shell commands on the selected phone: device info
//!   (model, Android version, battery, screen size), the notification list
//!   (`dumpsys notification --noredact`), key presses and app launches. It
//!   polls notifications every few seconds while a phone is connected and
//!   only wakes the UI when something changed.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

const SERVER: ([u8; 4], u16) = ([127, 0, 0, 1], 5037);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeviceState {
    Ready,
    Unauthorized,
    Offline,
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    pub serial: String,
    pub state: DeviceState,
    pub model: Option<String>,
}

impl Device {
    /// Connected over Wi-Fi (`ip:port`, or an mDNS TLS name).
    pub fn wireless(&self) -> bool {
        self.serial.contains(':') || self.serial.contains("._adb-tls-connect")
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct DeviceInfo {
    pub manufacturer: String,
    pub model: String,
    pub android: String,
    pub battery: Option<u8>,
    pub charging: bool,
    /// Screen size in pixels, portrait (width, height).
    pub screen: Option<(u32, u32)>,
}

impl DeviceInfo {
    pub fn name(&self) -> String {
        let m = self.model.trim();
        let brand = self.manufacturer.trim();
        // Pixel models already name the product.
        let skip = brand.is_empty()
            || brand.eq_ignore_ascii_case("google")
            || m.to_lowercase().starts_with(&brand.to_lowercase());
        if skip {
            m.to_owned()
        } else {
            format!("{} {m}", capitalize(brand))
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notif {
    pub key: String,
    pub package: String,
    pub app: String,
    pub title: String,
    pub text: String,
    /// Post time, ms since the Unix epoch (0 if unknown).
    pub when: i64,
}

pub enum Cmd {
    /// Follow this device (or none).
    Select(Option<String>),
    /// Poll faster while the page is open.
    Fast(bool),
    /// Re-read device info and notifications now.
    Refresh,
    /// `input keyevent`.
    Key(u32),
    /// Launch an app by package name.
    OpenApp(String),
    /// `adb connect host:port`.
    Connect(String),
    /// Android 11+ wireless debugging: `adb pair host:port code`.
    Pair(String, String),
    /// `adb tcpip 5555` on the USB device, then connect to it over Wi-Fi.
    GoWireless,
    Disconnect(String),
}

pub enum Event {
    Devices(Vec<Device>),
    Info(String, DeviceInfo),
    Notifications(String, Vec<Notif>),
    /// A one-line result to show the user ("Connected to ...").
    Message(String),
    /// The adb server is not reachable and could not be started.
    Unavailable,
}

pub struct Adb {
    pub exe: Option<PathBuf>,
    tx: Sender<Cmd>,
    pub events: Receiver<Event>,
}

impl Adb {
    /// Starts the device tracker and the worker. `exe` is the adb to start
    /// the server with if it is not running.
    pub fn start(exe: Option<PathBuf>, ctx: egui::Context) -> Self {
        let (ev_tx, events) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let emit = Emitter {
            tx: ev_tx,
            ctx: ctx.clone(),
        };
        let e = emit.clone();
        let adb = exe.clone();
        spawn("nook-adb-track", move || track(adb.as_deref(), &e));
        spawn("nook-adb-work", move || work(rx, &emit));
        Self { exe, tx, events }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

#[derive(Clone)]
struct Emitter {
    tx: Sender<Event>,
    ctx: egui::Context,
}

impl Emitter {
    fn emit(&self, e: Event) -> bool {
        let ok = self.tx.send(e).is_ok();
        if ok {
            self.ctx.request_repaint();
        }
        ok
    }
}

fn spawn(name: &str, f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new().name(name.into()).spawn(f) {
        tracing::warn!("spawning {name}: {e}");
    }
}

/// Runs `adb start-server` with no console window.
fn start_server(exe: &Path) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(exe)
        .arg("start-server")
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn connect() -> std::io::Result<TcpStream> {
    let addr = SocketAddr::from(SERVER);
    let s = TcpStream::connect_timeout(&addr, Duration::from_millis(800))?;
    s.set_nodelay(true)?;
    Ok(s)
}

/// Sends one request and reads OKAY, or the server's FAIL message.
fn request(s: &mut TcpStream, payload: &str) -> std::io::Result<()> {
    write!(s, "{:04x}{payload}", payload.len())?;
    let mut status = [0u8; 4];
    s.read_exact(&mut status)?;
    match &status {
        b"OKAY" => Ok(()),
        b"FAIL" => {
            let msg = read_block(s).unwrap_or_default();
            Err(std::io::Error::other(msg))
        }
        other => Err(std::io::Error::other(format!(
            "adb: unexpected reply {:?}",
            String::from_utf8_lossy(other)
        ))),
    }
}

/// A hex-length-prefixed block.
fn read_block(s: &mut TcpStream) -> std::io::Result<String> {
    let mut len = [0u8; 4];
    s.read_exact(&mut len)?;
    let len = usize::from_str_radix(std::str::from_utf8(&len).unwrap_or("0"), 16)
        .map_err(std::io::Error::other)?;
    let mut buf = vec![0u8; len];
    s.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

/// A `host:` query that answers with one block (`host:connect:...`).
fn host(query: &str) -> std::io::Result<String> {
    let mut s = connect()?;
    s.set_read_timeout(Some(Duration::from_secs(20)))?;
    request(&mut s, query)?;
    read_block(&mut s)
}

/// Runs a service on a device and returns everything it printed.
fn device_service(serial: &str, service: &str) -> std::io::Result<String> {
    let mut s = connect()?;
    s.set_read_timeout(Some(Duration::from_secs(15)))?;
    request(&mut s, &format!("host:transport:{serial}"))?;
    request(&mut s, service)?;
    let mut out = Vec::new();
    s.read_to_end(&mut out)?;
    Ok(String::from_utf8_lossy(&out).replace('\r', ""))
}

pub fn shell(serial: &str, cmd: &str) -> std::io::Result<String> {
    device_service(serial, &format!("shell:{cmd}"))
}

fn parse_devices(block: &str) -> Vec<Device> {
    block
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let serial = parts.next()?.to_owned();
            let state = match parts.next()? {
                "device" => DeviceState::Ready,
                "unauthorized" => DeviceState::Unauthorized,
                "offline" => DeviceState::Offline,
                other => DeviceState::Other(other.to_owned()),
            };
            let model = parts
                .find_map(|p| p.strip_prefix("model:"))
                .map(|m| m.replace('_', " "));
            Some(Device {
                serial,
                state,
                model,
            })
        })
        .collect()
}

/// Follows devices for as long as the app runs. Reconnects (and restarts
/// the server) when the server goes away, e.g. when another adb version
/// replaced it.
fn track(exe: Option<&Path>, emit: &Emitter) {
    let mut backoff = Duration::from_millis(500);
    loop {
        let mut stream = match connect() {
            Ok(s) => s,
            Err(_) => {
                let started = exe.is_some_and(start_server);
                if !started {
                    emit.emit(Event::Unavailable);
                    // Nothing to talk to: try again rarely.
                    std::thread::sleep(Duration::from_secs(30));
                    continue;
                }
                match connect() {
                    Ok(s) => s,
                    Err(_) => {
                        std::thread::sleep(backoff);
                        backoff = (backoff * 2).min(Duration::from_secs(10));
                        continue;
                    }
                }
            }
        };
        if request(&mut stream, "host:track-devices-l").is_err() {
            std::thread::sleep(backoff);
            continue;
        }
        backoff = Duration::from_millis(500);
        while let Ok(block) = read_block(&mut stream) {
            if !emit.emit(Event::Devices(parse_devices(&block))) {
                return;
            }
        }
        emit.emit(Event::Devices(Vec::new()));
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn work(rx: Receiver<Cmd>, emit: &Emitter) {
    let mut serial: Option<String> = None;
    let mut fast = false;
    let mut last_notifs: Option<Vec<Notif>> = None;
    let mut last_info: Option<DeviceInfo> = None;
    let mut info_at = Instant::now() - Duration::from_secs(3600);
    loop {
        let interval = if fast {
            Duration::from_secs(3)
        } else {
            Duration::from_secs(8)
        };
        let cmd = match rx.recv_timeout(interval) {
            Ok(c) => Some(c),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let mut refresh = cmd.is_none();
        match cmd {
            None => {}
            Some(Cmd::Select(s)) => {
                if s != serial {
                    serial = s;
                    last_notifs = None;
                    last_info = None;
                    info_at = Instant::now() - Duration::from_secs(3600);
                    refresh = true;
                }
            }
            Some(Cmd::Fast(f)) => {
                refresh = f && !fast;
                fast = f;
            }
            Some(Cmd::Refresh) => {
                info_at = Instant::now() - Duration::from_secs(3600);
                refresh = true;
            }
            Some(Cmd::Key(code)) => {
                if let Some(s) = &serial {
                    let _ = shell(s, &format!("input keyevent {code}"));
                }
            }
            Some(Cmd::OpenApp(pkg)) => {
                if let Some(s) = &serial
                    && pkg
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_')
                {
                    let _ = shell(
                        s,
                        &format!("monkey -p {pkg} -c android.intent.category.LAUNCHER 1"),
                    );
                }
            }
            Some(Cmd::Connect(addr)) => {
                let msg = match host(&format!("host:connect:{}", with_port(&addr))) {
                    Ok(m) => m,
                    Err(e) => e.to_string(),
                };
                emit.emit(Event::Message(msg.trim().to_owned()));
            }
            Some(Cmd::Pair(addr, code)) => {
                let msg = match host(&format!("host:pair:{}:{addr}", code.trim())) {
                    Ok(m) => m,
                    Err(e) => e.to_string(),
                };
                emit.emit(Event::Message(msg.trim().to_owned()));
            }
            Some(Cmd::Disconnect(addr)) => {
                let _ = host(&format!("host:disconnect:{addr}"));
            }
            Some(Cmd::GoWireless) => {
                let Some(s) = serial.clone() else {
                    continue;
                };
                let msg = go_wireless(&s);
                emit.emit(Event::Message(msg));
            }
        }
        let Some(s) = serial.clone() else {
            continue;
        };
        if !refresh {
            continue;
        }
        if info_at.elapsed() > Duration::from_secs(60) {
            info_at = Instant::now();
            if let Ok(out) = shell(&s, INFO_CMD) {
                let info = parse_info(&out);
                if last_info.as_ref() != Some(&info) {
                    last_info = Some(info.clone());
                    emit.emit(Event::Info(s.clone(), info));
                }
            }
        }
        if let Ok(out) = shell(&s, "dumpsys notification --noredact") {
            let list = parse_notifications(&out);
            if last_notifs.as_ref() != Some(&list) {
                last_notifs = Some(list.clone());
                emit.emit(Event::Notifications(s, list));
            }
        }
    }
}

const INFO_CMD: &str = "getprop ro.product.manufacturer; getprop ro.product.model; \
    getprop ro.build.version.release; dumpsys battery; wm size";

fn with_port(addr: &str) -> String {
    let addr = addr.trim();
    if addr.contains(':') {
        addr.to_owned()
    } else {
        format!("{addr}:5555")
    }
}

fn go_wireless(serial: &str) -> String {
    let ip = shell(serial, "ip -f inet addr show wlan0")
        .ok()
        .and_then(|out| {
            out.split_whitespace()
                .skip_while(|w| *w != "inet")
                .nth(1)
                .and_then(|a| a.split('/').next())
                .map(str::to_owned)
        });
    let Some(ip) = ip else {
        return "The phone is not on Wi-Fi".into();
    };
    if let Err(e) = device_service(serial, "tcpip:5555") {
        return format!("Could not switch to Wi-Fi: {e}");
    }
    // adbd restarts in TCP mode; give it a moment.
    std::thread::sleep(Duration::from_millis(1500));
    for _ in 0..3 {
        if let Ok(m) = host(&format!("host:connect:{ip}:5555"))
            && m.contains("connected")
        {
            return format!("Wireless: {ip}:5555. You can unplug the cable");
        }
        std::thread::sleep(Duration::from_secs(1));
    }
    format!("Could not reach {ip}:5555")
}

fn parse_info(out: &str) -> DeviceInfo {
    let mut lines = out.lines();
    let mut info = DeviceInfo {
        manufacturer: lines.next().unwrap_or("").trim().to_owned(),
        model: lines.next().unwrap_or("").trim().to_owned(),
        android: lines.next().unwrap_or("").trim().to_owned(),
        ..Default::default()
    };
    let mut physical = None;
    let mut override_size = None;
    for line in lines {
        let l = line.trim();
        if let Some(v) = l.strip_prefix("level:") {
            info.battery = v.trim().parse().ok();
        } else if let Some(v) = l.strip_prefix("status:") {
            // BatteryManager: 2 charging, 5 full.
            info.charging = matches!(v.trim(), "2" | "5");
        } else if let Some(v) = l.strip_prefix("Physical size:") {
            physical = parse_size(v);
        } else if let Some(v) = l.strip_prefix("Override size:") {
            override_size = parse_size(v);
        }
    }
    info.screen = override_size
        .or(physical)
        .map(|(w, h)| (w.min(h), w.max(h)));
    info
}

fn parse_size(v: &str) -> Option<(u32, u32)> {
    let (w, h) = v.trim().split_once('x')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

/// Value of an `extras` line: `android.title=String (Hello)` -> `Hello`.
fn extra_value(v: &str) -> Option<String> {
    let v = v.trim();
    if v == "null" {
        return None;
    }
    let text = match (v.find(" ("), v.ends_with(')')) {
        (Some(i), true) => &v[i + 2..v.len() - 1],
        _ => v,
    };
    let text = text.trim();
    (!text.is_empty() && text != "null").then(|| text.to_owned())
}

/// The posted notifications in `dumpsys notification --noredact` output,
/// newest first, without ongoing ones (media players, services) and group
/// summaries.
pub fn parse_notifications(out: &str) -> Vec<Notif> {
    const ONGOING: u32 = 0x2;
    const FOREGROUND_SERVICE: u32 = 0x40;
    const GROUP_SUMMARY: u32 = 0x200;

    struct Raw {
        key: String,
        package: String,
        flags: u32,
        title: Option<String>,
        text: Option<String>,
        big: Option<String>,
        when: i64,
    }
    let indent = |l: &str| l.len() - l.trim_start().len();
    let mut list: Vec<Raw> = Vec::new();
    let mut section: Option<usize> = None;
    for line in out.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        match section {
            None => {
                if t == "Notification List:" {
                    section = Some(indent(line));
                }
                continue;
            }
            Some(depth) if indent(line) <= depth => break,
            Some(_) => {}
        }
        if let Some(rest) = t.strip_prefix("NotificationRecord(") {
            let field = |name: &str| {
                rest.split_whitespace()
                    .find_map(|w| w.strip_prefix(name))
                    .map(|v| v.trim_end_matches(':').to_owned())
            };
            list.push(Raw {
                key: field("key=").unwrap_or_default(),
                package: field("pkg=").unwrap_or_default(),
                flags: 0,
                title: None,
                text: None,
                big: None,
                when: 0,
            });
            continue;
        }
        let Some(cur) = list.last_mut() else {
            continue;
        };
        if let Some(v) = t.strip_prefix("flags=") {
            if cur.flags == 0 {
                cur.flags = u32::from_str_radix(v.trim_start_matches("0x").trim(), 16).unwrap_or(0);
            }
        } else if let Some(v) = t.strip_prefix("android.title=") {
            cur.title = extra_value(v);
        } else if let Some(v) = t.strip_prefix("android.text=") {
            cur.text = extra_value(v);
        } else if let Some(v) = t.strip_prefix("android.bigText=") {
            cur.big = extra_value(v);
        } else if let Some(v) = t.strip_prefix("when=")
            && cur.when == 0
        {
            cur.when = v.trim().parse().unwrap_or(0);
        }
    }
    let mut out: Vec<Notif> = list
        .into_iter()
        .filter(|r| r.flags & (ONGOING | FOREGROUND_SERVICE | GROUP_SUMMARY) == 0)
        .filter(|r| r.title.is_some() || r.text.is_some())
        .map(|r| Notif {
            app: app_name(&r.package),
            key: r.key,
            package: r.package,
            title: r.title.unwrap_or_default(),
            text: r.big.or(r.text).unwrap_or_default(),
            when: r.when,
        })
        .collect();
    out.sort_by_key(|n| std::cmp::Reverse(n.when));
    out.dedup_by(|a, b| a.key == b.key);
    out
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// A readable app name for a package (no label lookup over adb is cheap
/// enough to run per notification).
pub fn app_name(package: &str) -> String {
    let known = [
        ("com.whatsapp", "WhatsApp"),
        ("com.whatsapp.w4b", "WhatsApp Business"),
        ("org.telegram.messenger", "Telegram"),
        ("org.thoughtcrime.securesms", "Signal"),
        ("com.google.android.gm", "Gmail"),
        ("com.google.android.apps.messaging", "Messages"),
        ("com.samsung.android.messaging", "Messages"),
        ("com.google.android.dialer", "Phone"),
        ("com.samsung.android.dialer", "Phone"),
        ("com.google.android.calendar", "Calendar"),
        ("com.google.android.youtube", "YouTube"),
        ("com.instagram.android", "Instagram"),
        ("com.facebook.orca", "Messenger"),
        ("com.facebook.katana", "Facebook"),
        ("com.discord", "Discord"),
        ("com.Slack", "Slack"),
        ("com.microsoft.teams", "Teams"),
        ("com.microsoft.office.outlook", "Outlook"),
        ("com.twitter.android", "X"),
        ("com.snapchat.android", "Snapchat"),
        ("com.spotify.music", "Spotify"),
        ("com.viber.voip", "Viber"),
        ("com.google.android.apps.photos", "Photos"),
        ("org.localsend.localsend_app", "LocalSend"),
        ("android", "Android"),
        ("com.android.systemui", "System"),
    ];
    if let Some((_, name)) = known.iter().find(|(p, _)| *p == package) {
        return (*name).to_owned();
    }
    let generic = [
        "com",
        "org",
        "net",
        "android",
        "app",
        "apps",
        "google",
        "mobile",
        "client",
        "messenger",
        "lite",
    ];
    package
        .split('.')
        .rev()
        .find(|p| !generic.contains(&p.to_lowercase().as_str()) && p.len() > 1)
        .map(capitalize)
        .unwrap_or_else(|| package.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = "\
Current Notification Manager state:
  Notification List:
    NotificationRecord(0x0a1b: pkg=com.whatsapp user=UserHandle{0} id=1 tag=null importance=4 key=0|com.whatsapp|1|null|10123: Notification(channel=x flags=0x10))
      uid=10123 userId=0
      flags=0x10
      notification=
          when=1700000000000
          extras={
              android.title=String (Alice)
              android.text=SpannableString (See you at 8?)
          }
    NotificationRecord(0x0c2d: pkg=com.spotify.music user=UserHandle{0} id=2 tag=null importance=2 key=0|com.spotify.music|2|null|10200: Notification(channel=y flags=0x62))
      flags=0x62
      notification=
          when=1700000005000
          extras={
              android.title=String (Song)
              android.text=String (Artist)
          }
    NotificationRecord(0x0e3f: pkg=com.google.android.gm user=UserHandle{0} id=3 tag=null importance=3 key=0|com.google.android.gm|3|null|10154: Notification(channel=z flags=0x18))
      flags=0x18
      notification=
          when=1700000009000
          extras={
              android.title=String (Bob)
              android.text=String (Invoice)
              android.bigText=String (Invoice attached for October)
          }
  Snoozed notifications:
    NotificationRecord(0x0fff: pkg=com.snoozed user=UserHandle{0} id=4 key=0|com.snoozed|4|null|1: Notification())
      extras={
          android.title=String (Hidden)
      }
";

    #[test]
    fn notifications_parse_newest_first_without_ongoing() {
        let n = parse_notifications(DUMP);
        assert_eq!(n.len(), 2);
        assert_eq!(n[0].app, "Gmail");
        assert_eq!(n[0].title, "Bob");
        assert_eq!(n[0].text, "Invoice attached for October");
        assert_eq!(n[1].package, "com.whatsapp");
        assert_eq!(n[1].text, "See you at 8?");
        assert_eq!(n[1].when, 1_700_000_000_000);
        assert_eq!(n[1].key, "0|com.whatsapp|1|null|10123");
    }

    #[test]
    fn device_lists_parse() {
        let d = parse_devices(
            "R58M12345\tdevice product:a51 model:SM_A515F device:a51 transport_id:3\n\
             192.168.1.20:5555\tunauthorized transport_id:4\n",
        );
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].state, DeviceState::Ready);
        assert_eq!(d[0].model.as_deref(), Some("SM A515F"));
        assert!(!d[0].wireless() && d[1].wireless());
        assert_eq!(d[1].state, DeviceState::Unauthorized);
    }

    #[test]
    fn device_info_parses() {
        let info = parse_info(
            "Google\nPixel 8\n15\nCurrent Battery Service state:\n  AC powered: false\n  \
             status: 2\n  level: 77\nPhysical size: 1080x2400\n",
        );
        assert_eq!(info.name(), "Pixel 8");
        assert_eq!(info.battery, Some(77));
        assert!(info.charging);
        assert_eq!(info.screen, Some((1080, 2400)));
        let samsung = DeviceInfo {
            manufacturer: "samsung".into(),
            model: "SM-S918B".into(),
            ..Default::default()
        };
        assert_eq!(samsung.name(), "Samsung SM-S918B");
    }

    #[test]
    fn app_names_read_well() {
        assert_eq!(app_name("org.telegram.messenger"), "Telegram");
        assert_eq!(app_name("com.example.weatherapp"), "Weatherapp");
        assert_eq!(app_name("io.homeassistant.companion.android"), "Companion");
    }
}
