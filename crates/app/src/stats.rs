//! Anonymous usage stats: how many installs there are and how many open
//! Nook each day.
//!
//! Each install has a random id (`install_id` in the config, made on first
//! run). Once per day Nook is open it sends that id, its version and the
//! Windows build number to the stats Worker (`telemetry/` in this repo).
//! Nothing else is sent. Settings > "Send anonymous usage stats" turns it
//! off, and copies Setup didn't install (dev builds) never send anything.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const PING_URL: &str = "https://nook-stats.gegasmixes.workers.dev/ping";
/// Wait a little after start so a crash-on-launch loop doesn't count.
const FIRST_PING: Duration = Duration::from_secs(30);
/// How often the thread looks whether a new (UTC) day has begun.
const TICK: Duration = Duration::from_secs(60 * 60);

pub struct Stats {
    enabled: Arc<AtomicBool>,
}

impl Stats {
    /// Starts the ping thread for `install_id`. `first` marks the run that
    /// created the id (a new install).
    pub fn start(install_id: String, first: bool, enabled: bool) -> Self {
        let flag = Arc::new(AtomicBool::new(enabled));
        if crate::update::installed_dir().is_none() {
            // Dev builds and loose exes would only skew the numbers.
            return Self { enabled: flag };
        }
        let on = flag.clone();
        let spawned = std::thread::Builder::new()
            .name("nook-stats".into())
            .spawn(move || run(&install_id, first, &on));
        if let Err(e) = spawned {
            tracing::warn!("spawning stats thread: {e}");
        }
        Self { enabled: flag }
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }
}

fn run(id: &str, mut first: bool, enabled: &AtomicBool) {
    std::thread::sleep(FIRST_PING);
    let mut sent_day = None;
    loop {
        let day = utc_day();
        if enabled.load(Ordering::Relaxed) && sent_day != Some(day) {
            match send(id, first) {
                Ok(()) => {
                    sent_day = Some(day);
                    first = false;
                }
                // Offline: try again next tick.
                Err(e) => tracing::debug!("stats ping: {e}"),
            }
        }
        std::thread::sleep(TICK);
    }
}

fn send(id: &str, first: bool) -> Result<(), Box<ureq::Error>> {
    let body = serde_json::json!({
        "id": id,
        "version": env!("CARGO_PKG_VERSION"),
        "os": nook_platform::system::windows_build().to_string(),
        "first": first,
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
}
