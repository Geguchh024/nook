//! Screen mirroring through scrcpy, shown inside the notch.
//!
//! scrcpy does the hard parts (the on-device server, H.264 decoding, input
//! injection, clipboard sync). Nook starts it borderless, finds its window,
//! adopts it ([`nook_platform::embed`]) and keeps it placed over the phone
//! area of the page, with rounded corners, every frame the page is shown.
//!
//! The mirror is view-only by default: the window is click-through, so the
//! panel underneath gets the clicks. **Control** hands it the mouse and
//! keyboard; Esc (caught by a low-level hook before scrcpy sees it) or the
//! Release button gives them back.

use nook_platform::Hwnd;
use nook_platform::embed::{self, EscHook};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Options picked on the page.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub audio: bool,
    pub screen_off: bool,
}

pub struct Mirror {
    child: Child,
    title: String,
    hwnd: Option<Hwnd>,
    /// Nook's main window, which owns the mirror.
    owner: Option<Hwnd>,
    started: Instant,
    /// The last error scrcpy printed.
    log: Arc<Mutex<Option<String>>>,
    hook: Option<EscHook>,
    esc: Arc<AtomicBool>,
    /// Who had the keyboard before control mode took it.
    prev_focus: Option<Hwnd>,
    placed: Option<(i32, i32, i32, i32)>,
    visible: bool,
    /// The phone had the keyboard at least once since control started.
    had_focus: bool,
    /// Content width / height; follows the phone's rotation.
    pub aspect: f32,
    pub serial: String,
}

pub enum Status {
    Starting,
    Running,
    Exited(String),
}

/// scrcpy, wherever it was installed: Nook's own folder, next to nook.exe,
/// PATH (WinGet links), WinGet's package folder, Scoop, Chocolatey.
pub fn find_scrcpy() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(a) = std::env::var_os("APPDATA") {
        candidates.push(
            PathBuf::from(a)
                .join("Nook")
                .join("scrcpy")
                .join("scrcpy.exe"),
        );
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        candidates.push(dir.join("scrcpy").join("scrcpy.exe"));
        candidates.push(dir.join("scrcpy.exe"));
    }
    candidates.extend(on_path("scrcpy.exe"));
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        let pkgs = PathBuf::from(l)
            .join("Microsoft")
            .join("WinGet")
            .join("Packages");
        candidates.extend(find_below(&pkgs, "Genymobile.scrcpy", "scrcpy.exe"));
    }
    if let Some(u) = std::env::var_os("USERPROFILE") {
        candidates.push(
            PathBuf::from(u)
                .join("scoop")
                .join("apps")
                .join("scrcpy")
                .join("current")
                .join("scrcpy.exe"),
        );
    }
    candidates.extend(find_below(
        Path::new(r"C:\ProgramData\chocolatey\lib"),
        "scrcpy",
        "scrcpy.exe",
    ));
    candidates
        .into_iter()
        .find(|p| p.is_file())
        // WinGet links are symlinks; the real folder also holds adb.exe.
        .map(|p| std::fs::canonicalize(&p).map(strip_verbatim).unwrap_or(p))
}

/// The adb to use: the one shipped with scrcpy (scrcpy restarts a server of
/// another version), else one on PATH or in the Android SDK.
pub fn find_adb(scrcpy: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = scrcpy.and_then(Path::parent) {
        candidates.push(dir.join("adb.exe"));
    }
    candidates.extend(on_path("adb.exe"));
    if let Some(l) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(l)
                .join("Android")
                .join("Sdk")
                .join("platform-tools")
                .join("adb.exe"),
        );
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p,
    }
}

fn on_path(exe: &str) -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join(exe)).collect())
        .unwrap_or_default()
}

/// `root/<prefix>*/**/file`, two levels deep.
fn find_below(root: &Path, prefix: &str, file: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for e in entries.flatten() {
        if !e.file_name().to_string_lossy().starts_with(prefix) {
            continue;
        }
        let dir = e.path();
        out.push(dir.join(file));
        if let Ok(sub) = std::fs::read_dir(&dir) {
            for s in sub.flatten() {
                out.push(s.path().join(file));
                out.push(s.path().join("tools").join(file));
            }
        }
    }
    out
}

/// Opens a console that installs scrcpy (and its adb) with WinGet.
pub fn install_with_winget() -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    Command::new("cmd.exe")
        .args([
            "/c",
            "winget install --id Genymobile.scrcpy -e --accept-source-agreements & pause",
        ])
        .creation_flags(CREATE_NEW_CONSOLE)
        .spawn()
        .map(|_| ())
}

impl Mirror {
    /// Starts scrcpy for `serial`. `rect` is where the mirror goes, in
    /// physical pixels; `aspect` the phone's screen width / height.
    pub fn start(
        scrcpy: &Path,
        adb: Option<&Path>,
        serial: &str,
        opts: Options,
        rect: (i32, i32, i32, i32),
        aspect: f32,
        ctx: &egui::Context,
    ) -> std::io::Result<Self> {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let title = format!("Nook mirror {}", super::localsend::random_hex(4));
        let (x, y, w, h) = rect;
        let mut cmd = Command::new(scrcpy);
        cmd.arg(format!("--serial={serial}"))
            .arg(format!("--window-title={title}"))
            .arg("--window-borderless")
            .arg(format!("--window-x={x}"))
            .arg(format!("--window-y={y}"))
            .arg(format!("--window-width={}", w.max(64)))
            .arg(format!("--window-height={}", h.max(64)))
            // A layered window is how the mirror becomes click-through; GL
            // presents fine into one (Nook's own window is the proof).
            .arg("--render-driver=opengl")
            .arg("--max-size=1600")
            .arg("--video-bit-rate=8M")
            .arg("--max-fps=60")
            .arg("--stay-awake");
        if !opts.audio {
            cmd.arg("--no-audio");
        }
        if opts.screen_off {
            cmd.arg("--turn-screen-off");
        }
        if let Some(adb) = adb {
            cmd.env("ADB", adb);
        }
        // Physical pixels everywhere, like Nook (no bitmap-stretched mirror
        // at 125% scaling).
        cmd.env("SDL_WINDOWS_DPI_AWARENESS", "permonitorv2")
            .env("SDL_WINDOWS_DPI_SCALING", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NO_WINDOW);
        if let Some(dir) = scrcpy.parent() {
            cmd.current_dir(dir);
        }
        let mut child = cmd.spawn()?;
        let log = Arc::new(Mutex::new(None));
        for stream in [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let log = log.clone();
            let ctx = ctx.clone();
            let _ = std::thread::Builder::new()
                .name("nook-scrcpy-log".into())
                .spawn(move || {
                    for line in BufReader::new(stream).lines().map_while(Result::ok) {
                        tracing::debug!("scrcpy: {line}");
                        if line.contains("ERROR") {
                            let msg = line.split_once("ERROR:").map_or(line.as_str(), |(_, m)| m);
                            if let Ok(mut l) = log.lock() {
                                *l = Some(msg.trim().to_owned());
                            }
                            ctx.request_repaint();
                        }
                    }
                });
        }
        tracing::info!("phone link: mirroring {serial}");
        Ok(Self {
            child,
            title,
            hwnd: None,
            owner: None,
            started: Instant::now(),
            log,
            hook: None,
            esc: Arc::new(AtomicBool::new(false)),
            prev_focus: None,
            placed: None,
            visible: false,
            had_focus: false,
            aspect,
            serial: serial.to_owned(),
        })
    }

    /// Finds and adopts the window once it exists; reports whether scrcpy
    /// is still running.
    pub fn tick(&mut self, owner: Option<Hwnd>) -> Status {
        self.owner = owner;
        if let Ok(Some(code)) = self.child.try_wait() {
            self.release_hook();
            let msg = self
                .log
                .lock()
                .ok()
                .and_then(|l| l.clone())
                .unwrap_or_else(|| format!("scrcpy stopped ({code})"));
            return Status::Exited(msg);
        }
        if self.hwnd.is_some_and(|h| !embed::is_alive(h)) {
            self.hwnd = None;
        }
        if self.hwnd.is_none() {
            let pid = self.child.id();
            if let Some(h) = embed::find_process_window(pid, &self.title) {
                if let Some(owner) = owner
                    && let Err(e) = embed::adopt(h, owner)
                {
                    tracing::warn!("phone link: adopting the mirror window: {e}");
                }
                embed::set_pass_through(h, true);
                self.hwnd = Some(h);
                self.placed = None;
                self.visible = true;
            } else if self.started.elapsed() > Duration::from_secs(25) {
                return Status::Exited("The mirror window never appeared".into());
            } else {
                return Status::Starting;
            }
        }
        // scrcpy resizes its own window when the phone rotates: follow the
        // new shape.
        if let (Some(h), Some(placed)) = (self.hwnd, self.placed)
            && let Ok((_, _, w, hh)) = nook_platform::window::window_rect(h)
            && (w, hh) != (placed.2, placed.3)
            && w > 0
            && hh > 0
        {
            let a = w as f32 / hh as f32;
            if (a > 1.0) != (self.aspect > 1.0) {
                self.aspect = a;
            }
            self.placed = None;
        }
        Status::Running
    }

    pub fn ready(&self) -> bool {
        self.hwnd.is_some()
    }

    /// Places the mirror (physical pixels) and shows it.
    pub fn place(&mut self, rect: (i32, i32, i32, i32), radius: i32) {
        let Some(h) = self.hwnd else {
            return;
        };
        if self.placed != Some(rect) {
            let (x, y, w, hh) = rect;
            if let Err(e) = embed::place(h, x, y, w, hh, radius) {
                tracing::debug!("phone link: placing the mirror: {e}");
            }
            self.placed = Some(rect);
        }
        if let Some(owner) = self.owner {
            embed::raise_above(h, owner);
        }
        if !self.visible {
            embed::set_visible(h, true);
            self.visible = true;
        }
    }

    pub fn hide(&mut self) {
        if self.controlling() {
            self.set_control(false, None);
        }
        if let Some(h) = self.hwnd
            && self.visible
        {
            embed::set_visible(h, false);
            self.visible = false;
        }
    }

    pub fn controlling(&self) -> bool {
        self.hook.is_some()
    }

    /// Hands the mouse and keyboard to the phone, or takes them back.
    pub fn set_control(&mut self, on: bool, ctx: Option<&egui::Context>) {
        let Some(h) = self.hwnd else {
            return;
        };
        if on == self.controlling() {
            return;
        }
        if on {
            self.prev_focus = embed::foreground();
            embed::set_pass_through(h, false);
            embed::give_focus(h);
            self.esc.store(false, Ordering::Relaxed);
            self.had_focus = false;
            let esc = self.esc.clone();
            let ctx = ctx.cloned();
            match EscHook::install(h, move || {
                esc.store(true, Ordering::Relaxed);
                if let Some(c) = &ctx {
                    c.request_repaint();
                }
            }) {
                Ok(hook) => self.hook = Some(hook),
                Err(e) => {
                    tracing::warn!("phone link: Esc hook: {e}");
                    embed::set_pass_through(h, true);
                }
            }
        } else {
            self.release_hook();
            embed::set_pass_through(h, true);
            if embed::foreground() == Some(h)
                && let Some(prev) = self.prev_focus.take()
                && embed::is_alive(prev)
            {
                embed::give_focus(prev);
            }
        }
    }

    fn release_hook(&mut self) {
        self.hook = None;
    }

    /// Esc was pressed in control mode since the last call.
    pub fn take_esc(&self) -> bool {
        self.esc.swap(false, Ordering::Relaxed)
    }

    /// In control mode, the keyboard went to another app (not the mirror,
    /// not Nook) after the phone had it.
    pub fn lost_focus(&mut self, owner: Option<Hwnd>) -> bool {
        if !self.controlling() {
            return false;
        }
        let fg = embed::foreground();
        if fg.is_some() && fg == self.hwnd {
            self.had_focus = true;
            return false;
        }
        self.had_focus && fg.is_some() && fg != owner
    }
}

impl Drop for Mirror {
    fn drop(&mut self) {
        self.release_hook();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
