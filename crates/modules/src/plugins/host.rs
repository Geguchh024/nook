//! Runs a plugin's program and moves JSON lines between it and the UI.
//!
//! Live mode keeps one child process: a writer thread feeds its stdin, a
//! reader thread parses its stdout, a third logs its stderr. Poll mode runs
//! the program to completion on a worker thread, every `interval` seconds and
//! once per event (passed in `NOOK_EVENT`). Either way the UI only drains a
//! channel in `on_tick`, so a slow or stuck plugin never blocks a frame.

use super::manifest::{Found, Mode};
use super::protocol::{self, Out};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

/// No console window flashing up for console programs (python, node, ...).
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
/// A poll run that takes longer is killed.
const POLL_TIMEOUT: Duration = Duration::from_secs(30);
/// stderr lines kept for the error tile.
const STDERR_TAIL: usize = 6;

/// What the host reports to the UI thread.
pub enum Event {
    Msg(Out),
    /// A line that was not a valid message.
    Bad(String),
    /// stderr output (last lines kept for errors).
    Stderr(String),
    /// The live process ended or could not start.
    Exited(String),
    /// Files in the plugin folder changed (dev mode).
    Changed,
}

/// Environment every run gets.
fn command(found: &Found, data_dir: &Path, dark: bool, accent: &str) -> Command {
    let m = &found.manifest;
    // A program inside the plugin folder ("bin/tool.exe", "./run.cmd") is
    // resolved there; anything else ("python", "node") is looked up on PATH.
    let program = {
        let local = found.dir.join(&m.run[0]);
        let looks_local = m.run[0].contains(['/', '\\']) || local.is_file();
        if looks_local && local.exists() {
            local.into_os_string()
        } else {
            m.run[0].clone().into()
        }
    };
    let mut cmd = Command::new(program);
    cmd.args(&m.run[1..])
        .current_dir(&found.dir)
        .env("NOOK_PLUGIN_ID", &m.id)
        .env("NOOK_PLUGIN_DIR", &found.dir)
        .env("NOOK_DATA_DIR", data_dir)
        .env("NOOK_PROTOCOL", protocol::PROTOCOL.to_string())
        .env("NOOK_VERSION", env!("CARGO_PKG_VERSION"))
        .env(
            "NOOK_MODE",
            if m.mode == Mode::Poll { "poll" } else { "live" },
        )
        .env("NOOK_THEME", if dark { "dark" } else { "light" })
        .env("NOOK_ACCENT", accent)
        // Line-buffered UTF-8 output from the runtimes people reach for.
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONIOENCODING", "utf-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// `%APPDATA%\Nook\plugin-data\<id>`, created on first use.
pub fn data_dir(id: &str) -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Nook").join("plugin-data").join(id);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        tracing::warn!("plugin data dir {}: {e}", dir.display());
    }
    dir
}

/// A running plugin. Dropping it stops the program.
pub struct Host {
    to_plugin: Option<Sender<Value>>,
    child: Option<Child>,
    pub events: Receiver<Event>,
    id: String,
}

impl Host {
    pub fn start(found: &Found, dark: bool, accent: &str, wake: egui::Context) -> Self {
        let (ev_tx, events) = mpsc::channel();
        let data = data_dir(&found.manifest.id);
        let notify = {
            let wake = wake.clone();
            move |tx: &Sender<Event>, e: Event| {
                let ok = tx.send(e).is_ok();
                wake.request_repaint();
                ok
            }
        };
        let id = found.manifest.id.clone();
        match found.manifest.mode {
            Mode::Live => {
                let mut cmd = command(found, &data, dark, accent);
                match cmd.spawn() {
                    Ok(mut child) => {
                        let to_plugin = spawn_io(&id, &mut child, ev_tx, notify);
                        tracing::info!("plugin {id} started (pid {})", child.id());
                        Self {
                            to_plugin,
                            child: Some(child),
                            events,
                            id,
                        }
                    }
                    Err(e) => {
                        let msg = format!("could not start {:?}: {e}", found.manifest.run[0]);
                        tracing::warn!("plugin {id}: {msg}");
                        notify(&ev_tx, Event::Exited(msg));
                        Self {
                            to_plugin: None,
                            child: None,
                            events,
                            id,
                        }
                    }
                }
            }
            Mode::Poll => {
                let (tx, rx) = mpsc::channel::<Value>();
                let found = found.clone();
                let theme = (dark, accent.to_owned());
                let spawned = std::thread::Builder::new()
                    .name(format!("nook-plugin-{id}"))
                    .spawn(move || poll_loop(&found, &data, theme, &rx, &ev_tx, &notify));
                if let Err(e) = spawned {
                    tracing::warn!("plugin {id}: spawning poll thread: {e}");
                }
                Self {
                    to_plugin: Some(tx),
                    child: None,
                    events,
                    id,
                }
            }
        }
    }

    /// Queues a message for the plugin. Never blocks.
    pub fn send(&self, msg: Value) {
        if let Some(tx) = &self.to_plugin {
            let _ = tx.send(msg);
        }
    }

    /// Reaps a live child after its stdout closed; kills it if it lingers.
    pub fn reap(&mut self) -> Option<i32> {
        let mut child = self.child.take()?;
        match child.try_wait() {
            Ok(Some(status)) => status.code(),
            _ => {
                let _ = child.kill();
                child.wait().ok().and_then(|s| s.code())
            }
        }
    }

    /// Asks the plugin to stop, then kills it after a short grace.
    pub fn stop(&mut self) {
        self.send(protocol::msg_simple("shutdown"));
        self.to_plugin = None;
        if let Some(mut child) = self.child.take() {
            let deadline = Instant::now() + Duration::from_millis(400);
            while Instant::now() < deadline {
                if let Ok(Some(_)) = child.try_wait() {
                    tracing::info!("plugin {} stopped", self.id);
                    return;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            let _ = child.kill();
            let _ = child.wait();
            tracing::info!("plugin {} killed", self.id);
        }
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.stop();
    }
}

type Notify = dyn Fn(&Sender<Event>, Event) -> bool + Send + Sync;

/// Wires a live child's pipes to threads; returns the stdin queue.
fn spawn_io(
    id: &str,
    child: &mut Child,
    ev_tx: Sender<Event>,
    notify: impl Fn(&Sender<Event>, Event) -> bool + Send + Sync + Clone + 'static,
) -> Option<Sender<Value>> {
    let stdin = child.stdin.take();
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let spawn = |name: &str, f: Box<dyn FnOnce() + Send>| {
        if let Err(e) = std::thread::Builder::new()
            .name(format!("nook-plugin-{id}-{name}"))
            .spawn(f)
        {
            tracing::warn!("plugin {id}: spawning {name} thread: {e}");
        }
    };

    if let Some(out) = stdout {
        let tx = ev_tx.clone();
        let notify = notify.clone();
        spawn(
            "out",
            Box::new(move || {
                for line in BufReader::new(out).lines() {
                    let Ok(line) = line else { break };
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }
                    let ev = match protocol::parse(line) {
                        Ok(m) => Event::Msg(m),
                        Err(e) => Event::Bad(e),
                    };
                    if !notify(&tx, ev) {
                        return;
                    }
                }
                notify(&tx, Event::Exited("the program exited".into()));
            }),
        );
    }
    if let Some(err) = stderr {
        let tx = ev_tx.clone();
        let notify = notify.clone();
        let id = id.to_owned();
        spawn(
            "err",
            Box::new(move || {
                for line in BufReader::new(err).lines() {
                    let Ok(line) = line else { break };
                    tracing::info!(target: "plugin", "[{id}] {line}");
                    if !notify(&tx, Event::Stderr(line)) {
                        return;
                    }
                }
            }),
        );
    }
    let mut stdin = stdin?;
    let (tx, rx) = mpsc::channel::<Value>();
    spawn(
        "in",
        Box::new(move || {
            for msg in rx {
                let line = format!("{msg}\n");
                if stdin.write_all(line.as_bytes()).is_err() || stdin.flush().is_err() {
                    break;
                }
            }
        }),
    );
    Some(tx)
}

/// Poll mode: run now, then on every event or when `interval` passes.
fn poll_loop(
    found: &Found,
    data: &Path,
    mut theme: (bool, String),
    rx: &Receiver<Value>,
    ev_tx: &Sender<Event>,
    notify: &Notify,
) {
    let interval = Duration::from_secs_f64(found.manifest.interval);
    let mut event: Option<Value> = None;
    loop {
        let mut cmd = command(found, data, theme.0, &theme.1);
        if let Some(e) = &event {
            cmd.env("NOOK_EVENT", e.to_string());
        }
        match run_once(cmd) {
            Ok((stdout, stderr)) => {
                for line in stderr.lines().filter(|l| !l.trim().is_empty()) {
                    tracing::info!(target: "plugin", "[{}] {line}", found.manifest.id);
                    notify(ev_tx, Event::Stderr(line.to_owned()));
                }
                for r in protocol::parse_output(&stdout) {
                    let ev = r.map_or_else(Event::Bad, Event::Msg);
                    if !notify(ev_tx, ev) {
                        return;
                    }
                }
            }
            Err(e) => {
                if !notify(ev_tx, Event::Exited(e)) {
                    return;
                }
            }
        }
        // Wait for the next reason to run. Theme and layout news only
        // updates the environment of the next run.
        event = None;
        let deadline = Instant::now() + interval;
        while event.is_none() {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(msg) => match msg.get("type").and_then(Value::as_str) {
                    Some("shutdown") => return,
                    Some("theme") => {
                        theme.0 = msg["dark"].as_bool().unwrap_or(theme.0);
                        if let Some(a) = msg["accent"].as_str() {
                            theme.1 = a.to_owned();
                        }
                    }
                    Some("hello" | "layout") => {}
                    _ => event = Some(msg),
                },
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

/// Runs a poll command to completion: (stdout, stderr), or why it failed.
fn run_once(mut cmd: Command) -> Result<(String, String), String> {
    let mut child = cmd.spawn().map_err(|e| format!("could not start: {e}"))?;
    drop(child.stdin.take());
    let read = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut r) = r {
                let _ = r.read_to_string(&mut s);
            }
            s
        })
    };
    let out = read(
        child
            .stdout
            .take()
            .map(|o| Box::new(o) as Box<dyn Read + Send>),
    );
    let err = read(
        child
            .stderr
            .take()
            .map(|e| Box::new(e) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + POLL_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(15)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("timed out after {} s", POLL_TIMEOUT.as_secs()));
            }
            Err(e) => return Err(e.to_string()),
        }
    };
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    if !status.success() && stdout.trim().is_empty() {
        let last = stderr.lines().rev().find(|l| !l.trim().is_empty());
        return Err(match last {
            Some(l) => format!("exit code {}: {l}", status.code().unwrap_or(-1)),
            None => format!("exit code {}", status.code().unwrap_or(-1)),
        });
    }
    Ok((stdout, stderr))
}

/// Dev mode: reports when any file in the plugin folder (two levels deep)
/// changes, so the plugin restarts on save. Watches until dropped.
pub struct Watch {
    pub changes: Receiver<Event>,
    stop: Arc<AtomicBool>,
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

pub fn watch(dir: PathBuf, wake: egui::Context) -> Watch {
    let (tx, changes) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let spawned = std::thread::Builder::new()
        .name("nook-plugin-watch".into())
        .spawn(move || {
            let mut last = fingerprint(&dir);
            while !stopped.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(700));
                let now = fingerprint(&dir);
                if now != last {
                    last = now;
                    if tx.send(Event::Changed).is_err() {
                        return;
                    }
                    wake.request_repaint();
                }
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("spawning plugin watcher: {e}");
    }
    Watch { changes, stop }
}

fn fingerprint(dir: &Path) -> u64 {
    fn walk(dir: &Path, depth: u32, acc: &mut u64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for e in entries.filter_map(Result::ok) {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "node_modules" || name == "__pycache__" {
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            if meta.is_dir() {
                if depth > 0 {
                    walk(&e.path(), depth - 1, acc);
                }
                continue;
            }
            let modified = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_millis() as u64);
            *acc = acc
                .wrapping_mul(31)
                .wrapping_add(modified ^ meta.len().rotate_left(17));
        }
    }
    let mut acc = 0;
    walk(dir, 2, &mut acc);
    acc
}

/// Keeps the last few stderr lines.
#[derive(Default)]
pub struct Tail(Vec<String>);

impl Tail {
    pub fn push(&mut self, line: String) {
        if self.0.len() == STDERR_TAIL {
            self.0.remove(0);
        }
        self.0.push(line);
    }

    pub fn text(&self) -> String {
        self.0.join("\n")
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::manifest::Manifest;

    /// A plugin folder in the temp dir with `plugin.toml` and `echo.ps1`.
    fn plugin(name: &str, toml: &str, script: &str) -> Found {
        let dir =
            std::env::temp_dir().join(format!("nook-plugin-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("echo.ps1"), script).unwrap();
        Found {
            manifest: Manifest::parse(toml).unwrap(),
            dir,
        }
    }

    fn next_msg(host: &Host) -> Out {
        loop {
            match host.events.recv_timeout(Duration::from_secs(20)) {
                Ok(Event::Msg(m)) => return m,
                Ok(Event::Bad(e) | Event::Exited(e)) => panic!("plugin failed: {e}"),
                Ok(_) => {}
                Err(e) => panic!("no message: {e}"),
            }
        }
    }

    fn text_of(m: Out) -> String {
        let Out::Tile { view } = m else {
            panic!("not a tile: {m:?}")
        };
        match view.kind {
            super::super::protocol::Kind::Text { text, .. } => text,
            k => panic!("not text: {k:?}"),
        }
    }

    const RUN: &str =
        r#"run = ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "echo.ps1"]"#;

    #[test]
    fn live_plugin_round_trip() {
        // Answers every message with a text tile naming its type (and id).
        let script = r#"
            while ($null -ne ($line = [Console]::In.ReadLine())) {
                $m = $line | ConvertFrom-Json
                $t = "$($m.type) $($m.id)".Trim()
                Write-Output (@{ type = 'text'; text = $t } | ConvertTo-Json -Compress)
                if ($m.type -eq 'shutdown') { break }
            }
        "#;
        let found = plugin(
            "live",
            &format!("id = \"echo\"\nname = \"Echo\"\n{RUN}"),
            script,
        );
        let host = Host::start(&found, true, "#7C5CFF", egui::Context::default());
        host.send(protocol::msg_simple("hello"));
        assert_eq!(text_of(next_msg(&host)), "hello");
        host.send(protocol::msg_event("inc", "click", Value::Null));
        assert_eq!(text_of(next_msg(&host)), "event inc");
    }

    #[test]
    fn poll_plugin_reruns_with_event() {
        let script = r#"
            $e = if ($env:NOOK_EVENT) { ($env:NOOK_EVENT | ConvertFrom-Json).id } else { 'none' }
            Write-Output (@{ type = 'text'; text = "$env:NOOK_PLUGIN_ID $e" } | ConvertTo-Json)
        "#;
        let toml =
            format!("id = \"poller\"\nname = \"P\"\nmode = \"poll\"\ninterval = 3600\n{RUN}");
        let found = plugin("poll", &toml, script);
        let host = Host::start(&found, true, "#7C5CFF", egui::Context::default());
        assert_eq!(text_of(next_msg(&host)), "poller none");
        host.send(protocol::msg_event("refresh", "click", Value::Null));
        assert_eq!(text_of(next_msg(&host)), "poller refresh");
    }
}
