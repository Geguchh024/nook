//! Anonymous usage stats: how many installs there are, how many open Nook
//! each day, and how much it gets used.
//!
//! Each install has a random id (`install_id` in the config, made on first
//! run). Once per day Nook is open it sends that id, its version, the
//! Windows build number and three counts gathered since the last ping: how
//! many times the panel opened, how many minutes it was open, and how many
//! times each built-in tool was used. Plugin names are never sent. The
//! counts wait in `%APPDATA%\Nook\usage.json` until a ping goes through.
//! Settings > "Anonymous usage stats" turns all of it off, and copies Setup
//! didn't install (dev builds) never send anything.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PING_URL: &str = "https://nook-stats.gegasmixes.workers.dev/ping";
/// Wait a little after start so a crash-on-launch loop doesn't count.
const FIRST_PING: Duration = Duration::from_secs(30);
/// How often the thread saves the counts and looks whether a new (UTC) day
/// has begun.
const TICK: Duration = Duration::from_secs(5 * 60);
/// Longest stretch one open panel counts for (a panel pinned open overnight
/// would otherwise swamp the averages).
const MAX_SESSION: Duration = Duration::from_secs(4 * 60 * 60);

/// Counts gathered since the last ping that went through.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Times the panel opened.
    pub opens: u32,
    /// Seconds the panel was open.
    pub secs: u64,
    /// Uses of each built-in tool, by module id.
    pub tools: BTreeMap<String, u32>,
}

impl Usage {
    fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Removes what `sent` reported, keeping anything counted meanwhile.
    fn subtract(&mut self, sent: &Usage) {
        self.opens = self.opens.saturating_sub(sent.opens);
        self.secs = self.secs.saturating_sub(sent.secs);
        for (tool, n) in &sent.tools {
            if let Some(left) = self.tools.get_mut(tool) {
                *left = left.saturating_sub(*n);
            }
        }
        self.tools.retain(|_, n| *n > 0);
    }
}

pub struct Stats {
    enabled: Arc<AtomicBool>,
    usage: Arc<Mutex<Usage>>,
    /// Module ids that ship with Nook; only these are counted as tools.
    builtin: HashSet<&'static str>,
    /// When the panel opened, while it is open.
    open_since: Option<Instant>,
}

impl Stats {
    /// Starts the ping thread for `install_id`. `first` marks the run that
    /// created the id (a new install); `builtin` lists the tools that may
    /// be counted by name.
    pub fn start(
        install_id: String,
        first: bool,
        enabled: bool,
        builtin: HashSet<&'static str>,
    ) -> Self {
        let flag = Arc::new(AtomicBool::new(enabled));
        let usage = Arc::new(Mutex::new(load_usage().unwrap_or_default()));
        let stats = Self {
            enabled: flag.clone(),
            usage: usage.clone(),
            builtin,
            open_since: None,
        };
        if crate::update::installed_dir().is_none() {
            // Dev builds and loose exes would only skew the numbers.
            stats.enabled.store(false, Ordering::Relaxed);
            return stats;
        }
        let spawned = std::thread::Builder::new()
            .name("nook-stats".into())
            .spawn(move || run(&install_id, first, &flag, &usage));
        if let Err(e) = spawned {
            tracing::warn!("spawning stats thread: {e}");
        }
        stats
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        if !on {
            // Turning stats off drops what was waiting to be sent.
            *self.usage.lock().unwrap_or_else(|e| e.into_inner()) = Usage::default();
            save_usage(&Usage::default());
        }
    }

    fn on(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// Call every frame with whether the panel is open; counts each opening
    /// and the time it stays open.
    pub fn panel(&mut self, open: bool) {
        match (open, self.open_since) {
            (true, None) => {
                self.open_since = Some(Instant::now());
                if self.on() {
                    self.with(|u| u.opens += 1);
                }
            }
            (false, Some(since)) => {
                self.open_since = None;
                let secs = since.elapsed().min(MAX_SESSION).as_secs();
                if self.on() && secs > 0 {
                    self.with(|u| u.secs += secs);
                }
            }
            _ => {}
        }
    }

    /// A tool was used (its tile clicked or its hotkey pressed). Plugins
    /// are ignored.
    pub fn tool(&self, id: &str) {
        if self.on() && self.builtin.contains(id) {
            self.with(|u| *u.tools.entry(id.to_owned()).or_default() += 1);
        }
    }

    fn with(&self, f: impl FnOnce(&mut Usage)) {
        f(&mut self.usage.lock().unwrap_or_else(|e| e.into_inner()));
    }
}

fn run(id: &str, mut first: bool, enabled: &AtomicBool, usage: &Mutex<Usage>) {
    std::thread::sleep(FIRST_PING);
    let mut sent_day = None;
    let mut saved = Usage::default();
    loop {
        let day = utc_day();
        let snapshot = usage.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if enabled.load(Ordering::Relaxed) && sent_day != Some(day) {
            match send(id, first, &snapshot) {
                Ok(()) => {
                    sent_day = Some(day);
                    first = false;
                    let mut u = usage.lock().unwrap_or_else(|e| e.into_inner());
                    u.subtract(&snapshot);
                }
                // Offline: try again next tick.
                Err(e) => tracing::debug!("stats ping: {e}"),
            }
        }
        let now = usage.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if now != saved {
            save_usage(&now);
            saved = now;
        }
        std::thread::sleep(TICK);
    }
}

fn send(id: &str, first: bool, usage: &Usage) -> Result<(), Box<ureq::Error>> {
    let body = serde_json::json!({
        "id": id,
        "version": env!("CARGO_PKG_VERSION"),
        "os": nook_platform::system::windows_build().to_string(),
        "first": first,
        "opens": usage.opens,
        "minutes": usage.secs / 60,
        "tools": usage.tools,
    });
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(15))
        .user_agent(concat!("Nook/", env!("CARGO_PKG_VERSION")))
        .build()
        .post(PING_URL)
        .send_json(body)
        .map_err(Box::new)?;
    Ok(())
}

fn usage_path() -> Option<PathBuf> {
    crate::config::dir().ok().map(|d| d.join("usage.json"))
}

fn load_usage() -> Option<Usage> {
    let text = std::fs::read_to_string(usage_path()?).ok()?;
    serde_json::from_str(&text).ok()
}

fn save_usage(usage: &Usage) {
    let Some(path) = usage_path() else {
        return;
    };
    let result = if usage.is_empty() {
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    } else {
        serde_json::to_string(usage)
            .map_err(std::io::Error::other)
            .and_then(|text| std::fs::write(&path, text))
    };
    if let Err(e) = result {
        tracing::debug!("saving usage counts: {e}");
    }
}

/// Days since the Unix epoch, in UTC (the Worker counts UTC days too).
fn utc_day() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0)
}

/// A new random install id: 32 lowercase hex characters.
pub fn new_install_id() -> String {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    if ring::rand::SystemRandom::new().fill(&mut bytes).is_err() {
        // Practically unreachable; fall back to the clock.
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        bytes = nanos.to_le_bytes();
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Whether `id` looks like one [`new_install_id`] made.
pub fn valid_id(id: &str) -> bool {
    id.len() == 32
        && id
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_ids_are_random_hex() {
        let a = new_install_id();
        let b = new_install_id();
        assert!(valid_id(&a), "{a}");
        assert_ne!(a, b);
        assert!(!valid_id("not-an-id"));
        assert!(!valid_id(&a.to_uppercase()));
    }

    #[test]
    fn sent_usage_is_subtracted_keeping_newer_counts() {
        let sent = Usage {
            opens: 3,
            secs: 120,
            tools: [("timer".to_owned(), 2)].into(),
        };
        let mut now = sent.clone();
        now.opens += 1;
        *now.tools.entry("timer".to_owned()).or_default() += 1;
        now.tools.insert("notes".to_owned(), 1);
        now.subtract(&sent);
        assert_eq!(now.opens, 1);
        assert_eq!(now.secs, 0);
        assert_eq!(
            now.tools,
            [("timer".to_owned(), 1), ("notes".to_owned(), 1)].into()
        );
        now.subtract(&now.clone());
        assert!(now.is_empty());
    }
}
