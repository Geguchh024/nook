//! One-click installs from the website.
//!
//! The marketplace's "Install in Nook" button opens a `nook://install?...`
//! link. Windows starts `nook.exe` with it; if Nook is already running, that
//! second process hands the link to the running one over a named pipe
//! ([`forward`]) and exits. The examples built into Nook install at once;
//! anything from GitHub is only downloaded and run once the person confirms
//! on the Plugins page, which shows the plugin's name, source and the exact
//! command it will run. Sources are limited to the examples built
//! into Nook and public GitHub repositories.
//!
//! Downloads and extraction run on worker threads; the app applies a
//! finished install on the UI thread ([`super::apply_changes`]), which swaps
//! the folder and loads the plugin without a restart.

use super::examples;
use super::manifest::{Found, Manifest};
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The archive and its unpacked files may not exceed these.
const MAX_DOWNLOAD: u64 = 50 * 1024 * 1024;
const MAX_UNPACKED: u64 = 100 * 1024 * 1024;
const MAX_FILES: usize = 2000;

/// Where a plugin comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// One of the plugins built into Nook.
    Example(String),
    /// A public GitHub repository, optionally a folder in it.
    GitHub {
        owner: String,
        repo: String,
        git_ref: String,
        path: String,
    },
}

impl Source {
    pub fn describe(&self) -> String {
        match self {
            Source::Example(id) => format!("Built into Nook (example \"{id}\")"),
            Source::GitHub {
                owner,
                repo,
                path,
                git_ref,
            } => {
                let mut s = format!("github.com/{owner}/{repo}");
                if !path.is_empty() {
                    s.push('/');
                    s.push_str(path);
                }
                if git_ref != "HEAD" {
                    s.push_str(&format!(" @ {git_ref}"));
                }
                s
            }
        }
    }

    pub fn web_url(&self) -> Option<String> {
        match self {
            Source::Example(_) => None,
            Source::GitHub {
                owner,
                repo,
                git_ref,
                path,
            } if path.is_empty() && git_ref == "HEAD" => {
                Some(format!("https://github.com/{owner}/{repo}"))
            }
            Source::GitHub {
                owner,
                repo,
                git_ref,
                path,
            } => Some(format!(
                "https://github.com/{owner}/{repo}/tree/{git_ref}/{path}"
            )),
        }
    }
}

#[derive(Clone, Debug)]
pub enum State {
    /// Reading plugin.toml to show what will be installed.
    Loading,
    /// Waiting for the person to confirm.
    Ready {
        manifest: Box<Manifest>,
        /// Version already installed, when this is an update.
        installed: Option<String>,
    },
    Installing,
    Done {
        name: String,
    },
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct Request {
    pub id: u64,
    pub source: Source,
    /// The id the website expects; the manifest must match it.
    pub expect: Option<String>,
    pub state: State,
}

/// A finished download, for the UI thread to put in place.
pub(super) struct Ready {
    pub request: u64,
    pub id: String,
    pub staging: PathBuf,
}

pub(super) enum Change {
    Install(Ready),
    Uninstall { id: String },
}

#[derive(Default)]
struct Installer {
    requests: Mutex<Vec<Request>>,
    changes: Mutex<Vec<Change>>,
    /// (plugin id, why its removal failed).
    remove_errors: Mutex<Vec<(String, String)>>,
    ctx: OnceLock<egui::Context>,
    next: AtomicU64,
    arrived: AtomicBool,
}

fn installer() -> &'static Installer {
    static I: OnceLock<Installer> = OnceLock::new();
    I.get_or_init(Installer::default)
}

fn wake() {
    if let Some(ctx) = installer().ctx.get() {
        ctx.request_repaint();
    }
}

fn update(id: u64, state: State) {
    if let Ok(mut rs) = installer().requests.lock()
        && let Some(r) = rs.iter_mut().find(|r| r.id == id)
    {
        r.state = state;
    }
    wake();
}

/// Called once with the UI context: registers the `nook://` link, starts
/// the hand-off listener and takes a link Nook was started with.
pub fn start(ctx: &egui::Context) {
    if installer().ctx.set(ctx.clone()).is_err() {
        return;
    }
    if let Err(e) = register_protocol() {
        tracing::warn!("registering nook:// links: {e}");
    }
    listen();
    if let Some(url) = std::env::args().find(|a| a.to_ascii_lowercase().starts_with("nook:")) {
        submit_url(&url);
    }
}

/// Requests in arrival order (done and failed ones stay until dismissed).
pub fn requests() -> Vec<Request> {
    installer()
        .requests
        .lock()
        .map(|r| r.clone())
        .unwrap_or_default()
}

/// True once per new request, so the Plugins tile can open the notch.
pub fn take_arrived() -> bool {
    installer().arrived.swap(false, Ordering::Relaxed)
}

pub(super) fn take_changes() -> Vec<Change> {
    installer()
        .changes
        .lock()
        .map(|mut c| std::mem::take(&mut *c))
        .unwrap_or_default()
}

pub fn dismiss(id: u64) {
    if let Ok(mut rs) = installer().requests.lock() {
        rs.retain(|r| r.id != id || matches!(r.state, State::Installing));
    }
    wake();
}

/// Queues an uninstall for the UI thread.
pub fn uninstall(id: &str) {
    if let Ok(mut c) = installer().changes.lock() {
        c.push(Change::Uninstall { id: id.to_owned() });
    }
    wake();
}

pub(super) fn finish(request: u64, result: Result<String, String>) {
    update(
        request,
        match result {
            Ok(name) => State::Done { name },
            Err(e) => State::Failed(e),
        },
    );
}

/// Takes a `nook://install?...` link: queues it and reads its manifest.
pub fn submit_url(url: &str) {
    let parsed = parse_url(url);
    let id = installer().next.fetch_add(1, Ordering::Relaxed) + 1;
    let (source, expect, state) = match parsed {
        Ok((s, e)) => (s, e, State::Loading),
        Err(e) => {
            tracing::warn!("install link {url:?}: {e}");
            (Source::Example(String::new()), None, State::Failed(e))
        }
    };
    let loading = matches!(state, State::Loading);
    if let Ok(mut rs) = installer().requests.lock() {
        // The same link twice (a double click) only shows once.
        if rs
            .iter()
            .any(|r| r.source == source && matches!(r.state, State::Loading | State::Ready { .. }))
        {
            installer().arrived.store(true, Ordering::Relaxed);
            wake();
            return;
        }
        rs.push(Request {
            id,
            source: source.clone(),
            expect: expect.clone(),
            state,
        });
    }
    installer().arrived.store(true, Ordering::Relaxed);
    wake();
    if loading {
        // The examples ship inside Nook, so they install without asking;
        // code from GitHub waits on the Plugins page for the person's yes.
        let built_in = matches!(source, Source::Example(_));
        spawn(move || {
            let state = match preview(&source, expect.as_deref()) {
                Ok(manifest) => {
                    let installed = installed_version(&manifest.id);
                    State::Ready {
                        manifest: Box::new(manifest),
                        installed,
                    }
                }
                Err(e) => State::Failed(e),
            };
            update(id, state);
            if built_in {
                confirm(id);
            }
        });
    }
}

/// Starts the download and unpacking after the person said yes.
pub fn confirm(id: u64) {
    let Some(req) = requests().into_iter().find(|r| r.id == id) else {
        return;
    };
    let State::Ready { manifest, .. } = req.state else {
        return;
    };
    update(id, State::Installing);
    spawn(move || match fetch(&req.source, &manifest.id) {
        Ok(staging) => {
            if let Ok(mut c) = installer().changes.lock() {
                c.push(Change::Install(Ready {
                    request: id,
                    id: manifest.id.clone(),
                    staging,
                }));
            }
            wake();
        }
        Err(e) => update(id, State::Failed(e)),
    });
}

fn spawn(f: impl FnOnce() + Send + 'static) {
    if let Err(e) = std::thread::Builder::new()
        .name("nook-plugin-install".into())
        .spawn(f)
    {
        tracing::warn!("spawning installer thread: {e}");
    }
}

// Links ----------------------------------------------------------------

fn safe_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 100
        && s != "."
        && s != ".."
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                match u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("zz"), 16)
                {
                    Ok(v) => {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `nook://install?example=counter` or
/// `nook://install?repo=owner/name&ref=main&path=plugins/weather&id=weather`.
pub fn parse_url(url: &str) -> Result<(Source, Option<String>), String> {
    let rest = url
        .trim()
        .strip_prefix("nook://")
        .or_else(|| url.trim().strip_prefix("nook:"))
        .ok_or("not a nook:// link")?;
    let (action, query) = rest.split_once('?').unwrap_or((rest, ""));
    if action.trim_end_matches('/') != "install" {
        return Err(format!("unknown action {action:?}"));
    }
    let mut kv = std::collections::HashMap::new();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        kv.insert(percent_decode(k), percent_decode(v));
    }
    let id_ok = |s: &str| {
        !s.is_empty()
            && s.len() <= 48
            && s.chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    };
    let expect = match kv.get("id") {
        Some(id) if id_ok(id) => Some(id.clone()),
        Some(_) => return Err("the plugin id in the link is not valid".into()),
        None => None,
    };
    if let Some(ex) = kv.get("example") {
        if examples::find(ex).is_none() {
            return Err(format!("this Nook has no example called {ex:?}"));
        }
        return Ok((Source::Example(ex.clone()), Some(ex.clone())));
    }
    let repo = kv.get("repo").ok_or("the link names no plugin")?;
    let (owner, name) = repo.split_once('/').ok_or("repo must be owner/name")?;
    let name = name.trim_end_matches(".git");
    if !safe_name(owner) || !safe_name(name) {
        return Err("the repository name in the link is not valid".into());
    }
    let git_ref = kv
        .get("ref")
        .map(String::as_str)
        .filter(|r| !r.is_empty())
        .unwrap_or("HEAD");
    if git_ref.len() > 100 || git_ref.split('/').any(|s| !safe_name(s)) {
        return Err("the branch name in the link is not valid".into());
    }
    let path = kv
        .get("path")
        .map(|p| p.trim_matches('/').to_owned())
        .unwrap_or_default();
    if !path.is_empty() && path.split('/').any(|s| !safe_name(s)) {
        return Err("the folder in the link is not valid".into());
    }
    Ok((
        Source::GitHub {
            owner: owner.into(),
            repo: name.into(),
            git_ref: git_ref.into(),
            path,
        },
        expect,
    ))
}

// Fetching ---------------------------------------------------------------

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(60))
        .user_agent(concat!(
            "Nook/",
            env!("CARGO_PKG_VERSION"),
            " plugin-installer"
        ))
        .redirects(5)
        .build()
}

fn get(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let resp = agent().get(url).call().map_err(|e| match e {
        ureq::Error::Status(404, _) => "not found on GitHub (is the repository public?)".to_owned(),
        ureq::Error::Status(code, _) => format!("GitHub answered {code}"),
        ureq::Error::Transport(t) => format!("couldn't reach GitHub: {t}"),
    })?;
    let mut buf = Vec::new();
    resp.into_reader()
        .take(limit + 1)
        .read_to_end(&mut buf)
        .map_err(|e| format!("download failed: {e}"))?;
    if buf.len() as u64 > limit {
        return Err(format!(
            "the download is larger than {} MB",
            limit / 1024 / 1024
        ));
    }
    Ok(buf)
}

fn check_id(m: &Manifest, expect: Option<&str>) -> Result<(), String> {
    match expect {
        Some(e) if e != m.id => Err(format!(
            "plugin.toml says id \"{}\", but the link asked for \"{e}\"",
            m.id
        )),
        _ => Ok(()),
    }
}

/// The manifest to show before installing.
fn preview(source: &Source, expect: Option<&str>) -> Result<Manifest, String> {
    let text = match source {
        Source::Example(id) => {
            let files = examples::find(id).ok_or("unknown example")?;
            let (_, bytes) = files
                .iter()
                .find(|(p, _)| *p == "plugin.toml")
                .ok_or("the example has no plugin.toml")?;
            String::from_utf8_lossy(bytes).into_owned()
        }
        Source::GitHub {
            owner,
            repo,
            git_ref,
            path,
        } => {
            let folder = if path.is_empty() {
                String::new()
            } else {
                format!("{path}/")
            };
            let url = format!(
                "https://raw.githubusercontent.com/{owner}/{repo}/{git_ref}/{folder}plugin.toml"
            );
            let bytes = get(&url, 64 * 1024).map_err(|e| format!("plugin.toml: {e}"))?;
            String::from_utf8(bytes).map_err(|_| "plugin.toml is not UTF-8".to_owned())?
        }
    };
    let m = Manifest::parse(&text).map_err(|e| format!("plugin.toml: {e}"))?;
    check_id(&m, expect)?;
    Ok(m)
}

/// `%APPDATA%\Nook\plugins`, created if needed.
pub fn plugins_root() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Nook").join("plugins");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// `%APPDATA%\Nook\plugin-staging`: downloads are unpacked here, beside the
/// plugins folder (same volume, so moving them in is a rename) but never in
/// it, where a half-written plugin could be discovered.
fn staging_root() -> PathBuf {
    let dir = plugins_root().with_file_name("plugin-staging");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

fn stamp() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

fn installed_version(id: &str) -> Option<String> {
    let found = super::manifest::discover(&super::manifest::search_dirs());
    found.into_iter().find(|f| f.manifest.id == id).map(|f| {
        if f.manifest.version.is_empty() {
            "installed".to_owned()
        } else {
            f.manifest.version
        }
    })
}

/// Downloads (or copies) the plugin into a staging folder next to the
/// plugins and checks its manifest again.
fn fetch(source: &Source, id: &str) -> Result<PathBuf, String> {
    let staging = staging_root().join(format!("{id}-{}", stamp()));
    let result = (|| {
        std::fs::create_dir_all(&staging)
            .map_err(|e| format!("creating {}: {e}", staging.display()))?;
        match source {
            Source::Example(ex) => {
                for (rel, bytes) in examples::find(ex).ok_or("unknown example")? {
                    std::fs::write(staging.join(rel), bytes)
                        .map_err(|e| format!("writing {rel}: {e}"))?;
                }
            }
            Source::GitHub {
                owner,
                repo,
                git_ref,
                path,
            } => {
                let zip = get(
                    &format!("https://github.com/{owner}/{repo}/archive/{git_ref}.zip"),
                    MAX_DOWNLOAD,
                )?;
                unpack(&zip, path, &staging)?;
            }
        }
        let text = std::fs::read_to_string(staging.join("plugin.toml"))
            .map_err(|_| "the download has no plugin.toml".to_owned())?;
        let m = Manifest::parse(&text).map_err(|e| format!("plugin.toml: {e}"))?;
        check_id(&m, Some(id))?;
        Ok(())
    })();
    match result {
        Ok(()) => Ok(staging),
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            Err(e)
        }
    }
}

/// Extracts `path` (a folder inside the repository, or all of it) from a
/// GitHub archive, whose entries sit under one top folder `repo-ref/`.
fn unpack(zip: &[u8], path: &str, into: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip))
        .map_err(|e| format!("the download isn't a zip: {e}"))?;
    let mut files = 0usize;
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| format!("reading the zip: {e}"))?;
        // enclosed_name refuses absolute paths and `..`.
        let Some(name) = entry.enclosed_name() else {
            continue;
        };
        let mut parts = name.components();
        parts.next(); // the archive's top folder
        let rel: PathBuf = parts.collect();
        let rel = if path.is_empty() {
            rel
        } else {
            match rel.strip_prefix(path) {
                Ok(r) => r.to_path_buf(),
                Err(_) => continue,
            }
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = into.join(&rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&out)
                .map_err(|e| format!("creating {}: {e}", rel.display()))?;
            continue;
        }
        files += 1;
        total += entry.size();
        if files > MAX_FILES || total > MAX_UNPACKED {
            return Err("the plugin is too large to install".into());
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("creating {}: {e}", parent.display()))?;
        }
        let mut f =
            std::fs::File::create(&out).map_err(|e| format!("writing {}: {e}", rel.display()))?;
        std::io::copy(&mut (&mut entry).take(MAX_UNPACKED), &mut f)
            .map_err(|e| format!("writing {}: {e}", rel.display()))?;
    }
    if files == 0 {
        return Err(if path.is_empty() {
            "the repository is empty".into()
        } else {
            format!("the repository has no folder \"{path}\"")
        });
    }
    Ok(())
}

/// Puts a staged plugin in place of any older copy and loads its manifest.
/// Runs on the UI thread after the old plugin's process has stopped.
pub(super) fn commit(staging: &Path, id: &str) -> Result<Found, String> {
    let root = plugins_root();
    let dest = root.join(id);
    if dest.exists() {
        let old = set_aside(&dest, id).map_err(|e| format!("replacing the old version: {e}"))?;
        if nook_platform::shell::recycle(&old).is_err() {
            let _ = std::fs::remove_dir_all(&old);
        }
    }
    std::fs::rename(staging, &dest).map_err(|e| format!("moving the plugin into place: {e}"))?;
    let text = std::fs::read_to_string(dest.join("plugin.toml")).map_err(|e| e.to_string())?;
    let manifest = Manifest::parse(&text)?;
    Ok(Found {
        manifest,
        dir: dest,
    })
}

/// Moves an installed plugin's folder to the Recycle Bin. The folder is
/// first moved aside in one step, so a locked folder stays whole.
pub(super) fn trash(id: &str) -> Result<(), String> {
    let dir = plugins_root().join(id);
    if !dir.exists() {
        return Ok(());
    }
    let old = set_aside(&dir, id)?;
    if nook_platform::shell::recycle(&old).is_err() {
        let _ = std::fs::remove_dir_all(&old);
    }
    Ok(())
}

/// Renames a plugin folder into the staging area. Retries briefly, since
/// antivirus scanners hold new files for a moment.
fn set_aside(dir: &Path, id: &str) -> Result<PathBuf, String> {
    let old = staging_root().join(format!("old-{id}-{}", stamp()));
    let mut tries = 0;
    loop {
        match std::fs::rename(dir, &old) {
            Ok(()) => return Ok(old),
            // 32: in use by another process; 5: access denied (same cause
            // for folders).
            Err(e) if matches!(e.raw_os_error(), Some(32 | 5)) => {
                tries += 1;
                if tries == 5 {
                    return Err("another program has the plugin's folder open (an editor, \
                         terminal or Explorer window). Close it and try again."
                        .into());
                }
                std::thread::sleep(Duration::from_millis(80));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Why the last Remove of a plugin failed, until it's removed or retried.
pub fn remove_error(id: &str) -> Option<String> {
    let errs = installer().remove_errors.lock().ok()?;
    errs.iter().find(|(i, _)| i == id).map(|(_, e)| e.clone())
}

pub(super) fn set_remove_error(id: &str, error: Option<String>) {
    if let Ok(mut errs) = installer().remove_errors.lock() {
        errs.retain(|(i, _)| i != id);
        if let Some(e) = error {
            errs.push((id.to_owned(), e));
        }
    }
    wake();
}

/// True if the plugin lives in the folder installs go to (so the Plugins
/// page may remove it).
pub fn removable(dir: &Path) -> bool {
    dir.parent().is_some_and(|p| p == plugins_root())
}

// The nook:// link -------------------------------------------------------

/// Named pipe the running Nook takes links on (see `nook_platform::ipc`).
const PIPE: &str = "Nook.Install";

fn register_protocol() -> nook_platform::Result<()> {
    nook_platform::url_scheme::register("nook", "Nook plugin installer")
}

/// Takes links from later `nook.exe` processes.
fn listen() {
    if let Err(e) = nook_platform::ipc::serve(PIPE, |url| submit_url(&url)) {
        tracing::warn!("install hand-off: {e}");
    }
}

/// From a second `nook.exe`: passes the link to the running Nook.
pub fn forward(url: &str) -> bool {
    match nook_platform::ipc::send(PIPE, url) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("install hand-off: {e}");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_links() {
        let (s, e) = parse_url("nook://install?example=counter").unwrap();
        assert_eq!(s, Source::Example("counter".into()));
        assert_eq!(e.as_deref(), Some("counter"));
        let (s, e) = parse_url(
            "nook://install/?repo=you%2Fnook-weather&ref=main&path=plugins/weather&id=weather",
        )
        .unwrap();
        assert_eq!(
            s,
            Source::GitHub {
                owner: "you".into(),
                repo: "nook-weather".into(),
                git_ref: "main".into(),
                path: "plugins/weather".into()
            }
        );
        assert_eq!(e.as_deref(), Some("weather"));
        let (s, _) = parse_url("nook://install?repo=a/b").unwrap();
        assert!(
            matches!(s, Source::GitHub { ref git_ref, ref path, .. } if git_ref == "HEAD" && path.is_empty())
        );
    }

    #[test]
    fn refuses_bad_links() {
        for bad in [
            "https://evil.example/x",
            "nook://uninstall?example=counter",
            "nook://install?example=nope",
            "nook://install?repo=a/b&path=../../Windows",
            "nook://install?repo=a/..&id=x",
            "nook://install?repo=a/b&ref=..%2F..",
            "nook://install?repo=a/b&id=Bad%20Id",
            "nook://install?repo=a",
            "nook://install",
        ] {
            assert!(parse_url(bad).is_err(), "{bad} should be refused");
        }
    }

    #[test]
    fn unpacks_a_folder_and_refuses_escapes() {
        use std::io::Write as _;
        let mut buf = Cursor::new(Vec::new());
        {
            let mut z = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            for (name, body) in [
                ("repo-main/README.md", "x"),
                (
                    "repo-main/plugins/w/plugin.toml",
                    "id = \"w\"\nname = \"W\"\nrun = [\"a\"]",
                ),
                ("repo-main/plugins/w/lib/util.py", "pass"),
                ("repo-main/plugins/other/plugin.toml", "nope"),
            ] {
                z.start_file(name, o).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        let dir = std::env::temp_dir().join(format!("nook-unpack-test-{}", stamp()));
        std::fs::create_dir_all(&dir).unwrap();
        unpack(buf.get_ref(), "plugins/w", &dir).unwrap();
        assert!(dir.join("plugin.toml").is_file());
        assert!(dir.join("lib").join("util.py").is_file());
        assert!(!dir.join("README.md").exists());
        assert!(unpack(buf.get_ref(), "plugins/missing", &dir).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
