//! Self-update from GitHub Releases.
//!
//! A background thread asks GitHub for the latest release a little after
//! start and then every few hours. When it is newer than this build, the
//! UI shows an Update button; clicking it downloads the release's
//! `Nook-Setup-*.exe`, checks its size and SHA-256, runs it silently and
//! quits. The installer replaces the files and starts Nook again (see the
//! `relaunch` parameter in `installer\nook.iss`).

use anyhow::{Context, Result, bail};
use crossbeam_channel::{Receiver, Sender, unbounded};
use nook_ui::theme::{SIZE_LABEL, Theme, mix, with_alpha};
use serde::Deserialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const LATEST_URL: &str = "https://api.github.com/repos/Geguchh024/nook/releases/latest";
pub const RELEASES_PAGE: &str = "https://github.com/Geguchh024/nook/releases/latest";
const FIRST_CHECK: Duration = Duration::from_secs(20);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    /// Version without the leading `v`.
    pub version: String,
    pub page: String,
    asset_url: String,
    asset_name: String,
    size: u64,
    /// Lowercase hex SHA-256, when GitHub lists one for the asset.
    sha256: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    /// `progress` in 0..1.
    Downloading {
        version: String,
        progress: f32,
    },
    /// The installer is running; Nook is about to quit.
    Installing,
    Failed(String),
}

impl Status {
    /// The release an Update button should offer, if any.
    pub fn offer(&self) -> Option<&Release> {
        match self {
            Status::Available(r) => Some(r),
            _ => None,
        }
    }
}

enum Cmd {
    Check,
    Install,
}

pub struct Updater {
    status: Arc<Mutex<Status>>,
    tx: Sender<Cmd>,
}

impl Updater {
    /// Starts the checker thread. With `auto` off it only checks when asked.
    pub fn start(ctx: egui::Context, auto: bool) -> Self {
        let status = Arc::new(Mutex::new(Status::Idle));
        let (tx, rx) = unbounded();
        let shared = status.clone();
        let spawned = std::thread::Builder::new()
            .name("nook-update".into())
            .spawn(move || run(rx, shared, ctx, auto));
        if let Err(e) = spawned {
            tracing::warn!("spawning update thread: {e}");
        }
        Self { status, tx }
    }

    pub fn status(&self) -> Status {
        self.status.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn check_now(&self) {
        let _ = self.tx.send(Cmd::Check);
    }

    /// Downloads and runs the offered release. A copy of Nook that Setup
    /// did not install (a dev build, an unzipped exe) can't be updated in
    /// place, so the release page opens instead.
    pub fn install(&self) {
        if installed_dir().is_none() {
            if let Err(e) = nook_platform::shell::open(RELEASES_PAGE) {
                tracing::warn!("opening the release page: {e}");
            }
            return;
        }
        let _ = self.tx.send(Cmd::Install);
    }
}

fn run(rx: Receiver<Cmd>, status: Arc<Mutex<Status>>, ctx: egui::Context, auto: bool) {
    let set = |s: Status| {
        if let Ok(mut cur) = status.lock() {
            *cur = s;
        }
        ctx.request_repaint();
    };
    // Dev builds don't nag; the manual check still works.
    let auto = auto && installed_dir().is_some();
    let mut wait = if auto { Some(FIRST_CHECK) } else { None };
    loop {
        let cmd = match wait {
            Some(w) => match rx.recv_timeout(w) {
                Ok(c) => c,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => Cmd::Check,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
            },
            None => match rx.recv() {
                Ok(c) => c,
                Err(_) => return,
            },
        };
        match cmd {
            Cmd::Check => {
                // Don't replace an offer or a download with "Checking".
                let busy = matches!(
                    status.lock().map(|s| s.clone()).unwrap_or_default(),
                    Status::Downloading { .. } | Status::Installing
                );
                if !busy {
                    set(Status::Checking);
                    match latest() {
                        Ok(Some(r)) => {
                            tracing::info!("update available: {} -> {}", CURRENT, r.version);
                            set(Status::Available(r));
                        }
                        Ok(None) => set(Status::UpToDate),
                        Err(e) => {
                            tracing::warn!("update check: {e:#}");
                            set(Status::Failed(short(&e)));
                        }
                    }
                }
                if auto {
                    wait = Some(CHECK_EVERY);
                }
            }
            Cmd::Install => {
                let release = match status.lock().map(|s| s.clone()) {
                    Ok(Status::Available(r)) => r,
                    _ => continue,
                };
                let progress = |p: f32| {
                    set(Status::Downloading {
                        version: release.version.clone(),
                        progress: p,
                    })
                };
                progress(0.0);
                match download(&release, progress).and_then(|path| launch(&path)) {
                    Ok(()) => {
                        tracing::info!("installer started for {}", release.version);
                        set(Status::Installing);
                        return;
                    }
                    Err(e) => {
                        tracing::warn!("update install: {e:#}");
                        set(Status::Failed(short(&e)));
                        // Offer it again so the user can retry.
                        std::thread::sleep(Duration::from_secs(4));
                        set(Status::Available(release));
                    }
                }
            }
        }
    }
}

fn short(e: &anyhow::Error) -> String {
    let s = e.to_string();
    if s.chars().count() > 60 {
        format!("{}…", s.chars().take(59).collect::<String>())
    } else {
        s
    }
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .user_agent(concat!("Nook/", env!("CARGO_PKG_VERSION"), " updater"))
        .build()
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    #[serde(default)]
    digest: Option<String>,
}

/// The latest release if it is newer than this build and has an installer.
fn latest() -> Result<Option<Release>> {
    let resp = agent()
        .get(LATEST_URL)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::Status(404, _) => anyhow::anyhow!("no releases published yet"),
            ureq::Error::Status(403 | 429, _) => {
                anyhow::anyhow!("GitHub rate limit, try again later")
            }
            ureq::Error::Status(code, _) => anyhow::anyhow!("GitHub answered {code}"),
            ureq::Error::Transport(t) => anyhow::anyhow!("couldn't reach GitHub: {t}"),
        })?;
    let api: ApiRelease = resp.into_json().context("reading the release")?;
    Ok(pick(api, CURRENT))
}

fn pick(api: ApiRelease, current: &str) -> Option<Release> {
    if api.draft || api.prerelease {
        return None;
    }
    let version = api.tag_name.trim_start_matches(['v', 'V']).to_owned();
    if !is_newer(&version, current) {
        return None;
    }
    let asset = api.assets.into_iter().find(|a| {
        let n = a.name.to_ascii_lowercase();
        n.starts_with("nook-setup") && n.ends_with(".exe")
    })?;
    let sha256 = asset
        .digest
        .as_deref()
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(str::to_ascii_lowercase);
    Some(Release {
        version,
        page: api.html_url,
        asset_url: asset.browser_download_url,
        asset_name: asset.name,
        size: asset.size,
        sha256,
    })
}

/// `a > b` for dotted numeric versions; a pre-release suffix (`-rc.1`)
/// sorts before the plain version.
pub fn is_newer(a: &str, b: &str) -> bool {
    fn parse(v: &str) -> (Vec<u64>, bool) {
        let (core, pre) = match v.split_once(['-', '+']) {
            Some((c, rest)) => (c, !rest.is_empty() && v.contains('-')),
            None => (v, false),
        };
        let mut nums: Vec<u64> = core
            .split('.')
            .map(|p| p.trim().parse().unwrap_or(0))
            .collect();
        while nums.len() < 3 {
            nums.push(0);
        }
        (nums, pre)
    }
    let (na, pa) = parse(a.trim());
    let (nb, pb) = parse(b.trim());
    match na.cmp(&nb) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => pb && !pa,
    }
}

/// Downloads the installer to `%TEMP%\Nook-Update`, verifying size and
/// hash. Returns its path.
fn download(release: &Release, mut progress: impl FnMut(f32)) -> Result<PathBuf> {
    let dir = std::env::temp_dir().join("Nook-Update");
    // Old installers from earlier updates are no use any more.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let name = Path::new(&release.asset_name)
        .file_name()
        .context("bad asset name")?;
    let path = dir.join(name);
    let part = path.with_extension("part");

    let resp = agent()
        .get(&release.asset_url)
        .call()
        .map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(release.size)
        .max(1);
    let mut reader = resp.into_reader();
    let mut file =
        std::fs::File::create(&part).with_context(|| format!("creating {}", part.display()))?;
    let mut hash = ring::digest::Context::new(&ring::digest::SHA256);
    let mut buf = vec![0u8; 64 * 1024];
    let mut got: u64 = 0;
    let mut last = 0.0;
    loop {
        let n = reader.read(&mut buf).context("download interrupted")?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).context("writing the installer")?;
        hash.update(&buf[..n]);
        got += n as u64;
        let p = (got as f32 / total as f32).min(1.0);
        if p - last >= 0.01 {
            last = p;
            progress(p);
        }
    }
    file.flush()?;
    drop(file);

    if release.size > 0 && got != release.size {
        bail!("download incomplete ({got} of {} bytes)", release.size);
    }
    if let Some(want) = &release.sha256 {
        let have = hex(hash.finish().as_ref());
        if &have != want {
            bail!("installer checksum mismatch");
        }
    }
    std::fs::rename(&part, &path).with_context(|| format!("renaming {}", part.display()))?;
    progress(1.0);
    Ok(path)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Folder Nook was installed to by Setup (it holds the uninstaller), or
/// `None` for a dev build or a loose exe.
pub fn installed_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?.to_path_buf();
    dir.join("unins000.exe").exists().then_some(dir)
}

/// Runs the installer silently. Setup stops Nook, replaces the files and,
/// with `/RELAUNCH=1`, starts it again. An install for all users (under
/// Program Files) needs `/ALLUSERS`, which brings up the UAC prompt.
fn launch(installer: &Path) -> Result<()> {
    let dir = installed_dir().context("Nook was not installed by Setup")?;
    let all_users = std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .is_some_and(|pf| dir.starts_with(pf));
    let mut cmd = std::process::Command::new(installer);
    cmd.args([
        "/SILENT",
        "/SUPPRESSMSGBOXES",
        "/NORESTART",
        "/NOCANCEL",
        // Keep the user's choices: never re-add the startup entry or the
        // desktop shortcut on update.
        "/MERGETASKS=!startup,!desktopicon",
        "/RELAUNCH=1",
    ])
    .arg(format!("/DIR={}", dir.display()))
    .arg(if all_users {
        "/ALLUSERS"
    } else {
        "/CURRENTUSER"
    });
    cmd.spawn().context("starting the installer")?;
    Ok(())
}

pub struct Chip {
    pub rect: egui::Rect,
    pub clicked: bool,
}

/// The Update chip: an accent pill placed at `pos` by `align`, showing
/// download progress once started. Draws nothing (and returns `None`)
/// when there is no update to offer.
pub fn chip(
    ui: &mut egui::Ui,
    pos: egui::Pos2,
    align: egui::Align2,
    status: &Status,
    theme: &Theme,
) -> Option<Chip> {
    use egui::{Align2, Color32, Rect, Sense, vec2};
    let (label, progress) = match status {
        Status::Available(r) => (format!("Update to {}", r.version), None),
        Status::Downloading { progress, .. } => (
            format!("Downloading {}%", (progress * 100.0).round()),
            Some(*progress),
        ),
        Status::Installing => ("Installing…".to_owned(), Some(1.0)),
        _ => return None,
    };
    let font = Theme::font_semibold(SIZE_LABEL);
    let text_w = ui
        .painter()
        .layout_no_wrap(label.clone(), font.clone(), Color32::WHITE)
        .size()
        .x;
    let rect = align.anchor_size(pos, vec2(text_w + 34.0, 20.0));
    let center = rect.center();
    let resp = ui.interact(rect, egui::Id::new("nook-update-chip"), Sense::click());
    let hover = nook_ui::anim::tween(ui.ctx(), resp.id.with("h"), resp.hovered(), 0.12);
    let painter = ui.painter();
    let fill = mix(theme.accent, Color32::WHITE, 0.12 * hover);
    match progress {
        None => {
            painter.rect_filled(rect, 10.0, fill);
        }
        Some(p) => {
            painter.rect_filled(rect, 10.0, with_alpha(theme.accent, 0.35));
            let done = Rect::from_min_size(rect.min, vec2(rect.width() * p, rect.height()));
            painter.with_clip_rect(done).rect_filled(rect, 10.0, fill);
        }
    }
    nook_ui::paint::icon(
        painter,
        egui::pos2(rect.min.x + 13.0, center.y),
        nook_ui::icons::ARROW_CIRCLE_UP,
        12.0,
        Color32::WHITE,
        true,
    );
    painter.text(
        egui::pos2(rect.min.x + 23.0, center.y),
        Align2::LEFT_CENTER,
        label,
        font,
        Color32::WHITE,
    );
    if matches!(status, Status::Downloading { .. }) {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
    let clicked = matches!(status, Status::Available(_))
        && resp.on_hover_text("Download and install").clicked();
    Some(Chip { rect, clicked })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_order() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(is_newer("0.2.0", "0.2.0-rc.1"));
        assert!(!is_newer("0.2.0-rc.1", "0.2.0"));
    }

    fn api(tag: &str, assets: &[&str]) -> ApiRelease {
        ApiRelease {
            tag_name: tag.into(),
            html_url: "https://example.invalid/r".into(),
            draft: false,
            prerelease: false,
            assets: assets
                .iter()
                .map(|n| ApiAsset {
                    name: (*n).into(),
                    browser_download_url: format!("https://example.invalid/{n}"),
                    size: 10,
                    digest: Some("sha256:ABCD".into()),
                })
                .collect(),
        }
    }

    #[test]
    fn picks_the_installer_of_a_newer_release() {
        let r = pick(
            api("v0.2.0", &["notes.txt", "Nook-Setup-0.2.0.exe"]),
            "0.1.0",
        )
        .expect("newer release");
        assert_eq!(r.version, "0.2.0");
        assert_eq!(r.asset_name, "Nook-Setup-0.2.0.exe");
        assert_eq!(r.sha256.as_deref(), Some("abcd"));
        assert!(pick(api("v0.1.0", &["Nook-Setup-0.1.0.exe"]), "0.1.0").is_none());
        assert!(pick(api("v0.2.0", &["nook.zip"]), "0.1.0").is_none());
        let mut pre = api("v0.3.0", &["Nook-Setup-0.3.0.exe"]);
        pre.prerelease = true;
        assert!(pick(pre, "0.1.0").is_none());
    }

    /// Talks to GitHub: `cargo test -p nook-app -- --ignored`.
    #[test]
    #[ignore]
    fn downloads_and_verifies_the_latest_installer() {
        let api: ApiRelease = agent()
            .get(LATEST_URL)
            .call()
            .expect("GitHub")
            .into_json()
            .expect("json");
        let r = pick(api, "0.0.0").expect("a release with an installer");
        let path = download(&r, |_| {}).expect("download");
        assert_eq!(std::fs::metadata(&path).expect("file").len(), r.size);
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn parses_github_json() {
        let json = r#"{"tag_name":"v9.9.9","html_url":"https://x","draft":false,
            "prerelease":false,"assets":[{"name":"Nook-Setup-9.9.9.exe",
            "browser_download_url":"https://x/a.exe","size":5,"digest":null}]}"#;
        let api: ApiRelease = serde_json::from_str(json).expect("parse");
        let r = pick(api, "0.1.0").expect("release");
        assert_eq!(r.sha256, None);
    }
}
