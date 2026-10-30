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
//! A plugin `.zip` or `.7z` dropped on the notch ([`submit_archive`])
//! installs too: every folder in it with a `plugin.toml` is one plugin, so a
//! bundle of several installs in one drop. Dropping the file is the person's go-ahead, so
//! these install without a second confirmation.
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
    /// A `.zip` or `.7z` on this PC, and the folder in it holding
    /// `plugin.toml` (empty when it sits at the top).
    Archive { file: PathBuf, folder: String },
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
            Source::Archive { file, folder } => {
                let name = file.file_name().map_or_else(
                    || file.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                if folder.is_empty() {
                    format!("{name} (dropped on Nook)")
                } else {
                    format!("{name} \u{203a} {folder} (dropped on Nook)")
                }
            }
        }
    }

    pub fn web_url(&self) -> Option<String> {
        match self {
            Source::Example(_) | Source::Archive { .. } => None,
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
    match parse_url(url) {
        Ok((source, expect)) => submit(source, expect),
        Err(e) => {
            tracing::warn!("install link {url:?}: {e}");
            queue(Source::Example(String::new()), None, State::Failed(e));
        }
    }
}

/// True for the archive types plugins can be dropped as.
pub fn is_archive(file: &Path) -> bool {
    file.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ARCHIVES.iter().any(|a| e.eq_ignore_ascii_case(a)))
}

const ARCHIVES: [&str; 2] = ["zip", "7z"];

/// Installs the plugins in a dropped `.zip` or `.7z`: one, or every folder
/// of a bundle that has a `plugin.toml`. False (and nothing queued) when the
/// file is not an archive or holds no plugin, so the drop can go elsewhere.
pub fn submit_archive(file: &Path) -> bool {
    match plugins_in_archive(file) {
        Ok(folders) if !folders.is_empty() => {
            queue_archive(file, folders);
            true
        }
        Ok(_) => {
            tracing::info!("{}: no plugin.toml inside, not a plugin", file.display());
            false
        }
        Err(e) => {
            tracing::info!("{}: not a plugin archive ({e})", file.display());
            false
        }
    }
}

/// For a drop on the Plugins page, where every file is meant as a plugin:
/// installs it, or shows on the page why it could not.
pub fn submit_dropped(file: &Path) {
    let reason = match plugins_in_archive(file) {
        Ok(folders) if !folders.is_empty() => return queue_archive(file, folders),
        Ok(_) => "it has no plugin.toml, so it isn't a Nook plugin".to_owned(),
        Err(e) => e,
    };
    let name = file
        .file_name()
        .map_or_else(|| file.display().to_string(), |n| n.to_string_lossy().into_owned());
    tracing::info!("{}: not installed: {reason}", file.display());
    queue(
        Source::Archive {
            file: file.to_path_buf(),
            folder: String::new(),
        },
        None,
        State::Failed(format!("{name}: {reason}")),
    );
}

fn queue_archive(file: &Path, folders: Vec<String>) {
    tracing::info!(
        "{}: installing {} plugin(s) from {folders:?}",
        file.display(),
        folders.len()
    );
    for folder in folders {
        submit(
            Source::Archive {
                file: file.to_path_buf(),
                folder,
            },
            None,
        );
    }
}

/// Adds a request to the list. `None` when the same source is already
/// waiting (a double click), which only shows it again.
fn queue(source: Source, expect: Option<String>, state: State) -> Option<u64> {
    let id = installer().next.fetch_add(1, Ordering::Relaxed) + 1;
    if let Ok(mut rs) = installer().requests.lock() {
        // The same link twice (a double click) only shows once.
        if rs
            .iter()
            .any(|r| r.source == source && matches!(r.state, State::Loading | State::Ready { .. }))
        {
            installer().arrived.store(true, Ordering::Relaxed);
            wake();
            return None;
        }
        rs.push(Request {
            id,
            source,
            expect,
            state,
        });
    }
    installer().arrived.store(true, Ordering::Relaxed);
    wake();
    Some(id)
}

/// Queues `source` and reads its manifest.
fn submit(source: Source, expect: Option<String>) {
    let Some(id) = queue(source.clone(), expect.clone(), State::Loading) else {
        return;
    };
    // The examples ship inside Nook and a dropped archive was the person's
    // own choice, so both install without asking; code from GitHub waits on
    // the Plugins page for the person's yes.
    let built_in = matches!(source, Source::Example(_) | Source::Archive { .. });
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
        Source::Archive { file, folder } => {
            let name = if folder.is_empty() {
                "plugin.toml".to_owned()
            } else {
                format!("{folder}/plugin.toml")
            };
            let entries = read_archive(file)?;
            let entry = entries
                .iter()
                .find(|e| e.name == name)
                .ok_or_else(|| format!("the archive has no {name}"))?;
            String::from_utf8(entry.data.clone())
                .map_err(|_| "plugin.toml is not UTF-8".to_owned())?
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
                unpack(&zip, path, &staging, true)?;
            }
            Source::Archive { file, folder } => {
                write_folder(&read_archive(file)?, folder, &staging)?;
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

/// One file from a dropped archive. `name` uses `/` whatever the tool that
/// made it wrote (Windows' own tools store `a\b`).
struct Entry {
    name: String,
    data: Vec<u8>,
}

/// Reads every file of a dropped `.zip` or `.7z` into memory, within the
/// same limits as a GitHub download.
fn read_archive(file: &Path) -> Result<Vec<Entry>, String> {
    let len = std::fs::metadata(file)
        .map_err(|e| format!("reading {}: {e}", file.display()))?
        .len();
    if len > MAX_DOWNLOAD {
        return Err(format!(
            "the archive is larger than {} MB",
            MAX_DOWNLOAD / 1024 / 1024
        ));
    }
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut out: Vec<Entry> = Vec::new();
    let mut total = 0u64;
    let mut add = |name: &str, reader: &mut dyn Read| -> Result<(), String> {
        if out.len() >= MAX_FILES {
            return Err("the archive has too many files to be a plugin".into());
        }
        let mut data = Vec::new();
        reader
            .take(MAX_UNPACKED - total + 1)
            .read_to_end(&mut data)
            .map_err(|e| format!("reading {name}: {e}"))?;
        total += data.len() as u64;
        if total > MAX_UNPACKED {
            return Err("the plugin is too large to install".into());
        }
        out.push(Entry {
            name: name.replace('\\', "/"),
            data,
        });
        Ok(())
    };
    match ext.as_str() {
        "zip" => {
            let bytes =
                std::fs::read(file).map_err(|e| format!("reading {}: {e}", file.display()))?;
            let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
                .map_err(|e| format!("not a valid zip: {e}"))?;
            for i in 0..archive.len() {
                let mut entry = archive
                    .by_index(i)
                    .map_err(|e| format!("reading the zip: {e}"))?;
                if entry.is_dir() {
                    continue;
                }
                let name = entry.name().to_owned();
                add(&name, &mut entry)?;
            }
        }
        "7z" => {
            let mut reader =
                sevenz_rust2::ArchiveReader::open(file, sevenz_rust2::Password::empty())
                    .map_err(|e| format!("not a valid 7z (or it has a password): {e}"))?;
            // Refuse a decompression bomb before inflating anything.
            let declared: u64 = reader.archive().files.iter().map(|f| f.size()).sum();
            if declared > MAX_UNPACKED {
                return Err("the plugin is too large to install".into());
            }
            let mut failed = None;
            reader
                .for_each_entries(|entry, data| {
                    if entry.is_directory() {
                        return Ok(true);
                    }
                    match add(entry.name(), data) {
                        Ok(()) => Ok(true),
                        Err(e) => {
                            failed = Some(e);
                            Ok(false)
                        }
                    }
                })
                .map_err(|e| format!("reading the 7z: {e}"))?;
            if let Some(e) = failed {
                return Err(e);
            }
        }
        _ => return Err("only .zip and .7z files can hold plugins".into()),
    }
    Ok(out)
}

/// The folders in an archive that hold a `plugin.toml` (`""` for the top),
/// each one plugin.
pub fn plugins_in_archive(file: &Path) -> Result<Vec<String>, String> {
    if !is_archive(file) {
        return Err("only .zip and .7z files can hold plugins".into());
    }
    let entries = read_archive(file)?;
    Ok(plugin_folders(entries.iter().map(|e| e.name.as_str())))
}

/// Writes the files under `folder` (all of them for `""`) into `into`.
/// Names that could leave `into` (`..`, drive letters, rooted paths) are
/// refused.
fn write_folder(entries: &[Entry], folder: &str, into: &Path) -> Result<(), String> {
    let prefix = if folder.is_empty() {
        String::new()
    } else {
        format!("{folder}/")
    };
    let mut written = 0usize;
    for e in entries {
        let Some(rel) = e.name.strip_prefix(&prefix) else {
            continue;
        };
        let parts: Vec<&str> = rel.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
        if parts.is_empty() {
            continue;
        }
        if e.name.starts_with('/') || parts.iter().any(|p| *p == ".." || p.contains(':')) {
            return Err(format!("the archive has an unsafe path: {}", e.name));
        }
        let out = parts.iter().fold(into.to_path_buf(), |dir, p| dir.join(p));
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| format!("creating {}: {err}", parent.display()))?;
        }
        std::fs::write(&out, &e.data).map_err(|err| format!("writing {rel}: {err}"))?;
        written += 1;
    }
    if written == 0 {
        return Err(format!("the archive has no folder \"{folder}\""));
    }
    Ok(())
}

/// Plugin folders among archive entry names. A `plugin.toml` inside another
/// plugin's folder is part of that plugin, not a plugin of its own.
fn plugin_folders<'a>(names: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut folders: Vec<String> = names
        .map(|n| n.replace('\\', "/"))
        .filter(|n| !n.starts_with("__MACOSX/"))
        .filter_map(|n| {
            let folder = n.strip_suffix("plugin.toml")?;
            // Only a file named exactly plugin.toml, not "my-plugin.toml".
            if !(folder.is_empty() || folder.ends_with('/')) {
                return None;
            }
            let folder = folder.trim_end_matches('/');
            (!folder.split('/').any(|p| p == "..")).then(|| folder.to_owned())
        })
        .collect();
    folders.sort_by_key(|f| (f.matches('/').count(), f.clone()));
    let mut out: Vec<String> = Vec::new();
    for f in folders {
        let nested = out
            .iter()
            .any(|o| o.is_empty() || *o == f || f.starts_with(&format!("{o}/")));
        if !nested {
            out.push(f);
        }
    }
    out
}

/// Extracts `path` (a folder inside the archive, or all of it). GitHub
/// archives (`strip_top`) put every entry under one top folder `repo-ref/`.
fn unpack(zip: &[u8], path: &str, into: &Path, strip_top: bool) -> Result<(), String> {
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
        if strip_top {
            parts.next(); // the archive's top folder
        }
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
        unpack(buf.get_ref(), "plugins/w", &dir, true).unwrap();
        assert!(dir.join("plugin.toml").is_file());
        assert!(dir.join("lib").join("util.py").is_file());
        assert!(!dir.join("README.md").exists());
        assert!(unpack(buf.get_ref(), "plugins/missing", &dir, true).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_one_plugin_or_a_bundle() {
        let folders = |names: &[&str]| plugin_folders(names.iter().copied());
        // A single plugin zipped from inside its folder, or as its folder.
        assert_eq!(folders(&["plugin.toml", "main.py"]), [""]);
        assert_eq!(folders(&["timer/plugin.toml", "timer/main.py"]), ["timer"]);
        // A bundle, at any depth, with Windows separators and macOS junk.
        assert_eq!(
            folders(&[
                "bundle/a/plugin.toml",
                "bundle\\b\\plugin.toml",
                "c/plugin.toml",
                "__MACOSX/c/plugin.toml",
                "README.md",
            ]),
            ["c", "bundle/a", "bundle/b"]
        );
        // A plugin.toml inside a plugin (a template, say) is its own file.
        assert_eq!(
            folders(&["a/plugin.toml", "a/templates/plugin.toml"]),
            ["a"]
        );
        assert_eq!(folders(&["plugin.toml", "x/plugin.toml"]), [""]);
        // Not plugins.
        assert!(folders(&["my-plugin.toml", "notes.txt"]).is_empty());
        assert!(folders(&["../evil/plugin.toml"]).is_empty());
    }

    #[test]
    fn installs_each_plugin_of_a_dropped_bundle() {
        use std::io::Write as _;
        let file = std::env::temp_dir().join(format!("nook-bundle-test-{}.zip", stamp()));
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&file).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            for (name, body) in [
                ("pack/one/plugin.toml", "id = \"one\"\nname = \"One\"\nrun = [\"a\"]"),
                ("pack/one/main.py", "print(1)"),
                // Windows' Compress-Archive writes backslash paths.
                ("pack\\two\\plugin.toml", "id = \"two\"\nname = \"Two\"\nrun = [\"b\"]"),
                ("pack\\two\\lib\\x.py", "pass"),
            ] {
                z.start_file(name, o).unwrap();
                z.write_all(body.as_bytes()).unwrap();
            }
            z.finish().unwrap();
        }
        let found = plugins_in_archive(&file).unwrap();
        assert_eq!(found, ["pack/one", "pack/two"]);
        for (folder, id, file_in_it) in [("pack/one", "one", "main.py"), ("pack/two", "two", "lib/x.py")] {
            let source = Source::Archive {
                file: file.clone(),
                folder: folder.into(),
            };
            assert_eq!(preview(&source, None).unwrap().id, id);
            let staging = fetch(&source, id).unwrap();
            assert!(staging.join("plugin.toml").is_file());
            assert!(staging.join(file_in_it).is_file(), "{id}: {file_in_it}");
            // Only this plugin's files, not its neighbour's.
            assert!(!staging.join("one").exists() && !staging.join("two").exists());
            let _ = std::fs::remove_dir_all(&staging);
        }
        // An id that does not match what the folder was read as is refused.
        let source = Source::Archive {
            file: file.clone(),
            folder: "pack/one".into(),
        };
        assert!(fetch(&source, "two").is_err());
        let _ = std::fs::remove_file(&file);
    }

    #[test]
    fn installs_a_7z_bundle() {
        let root = std::env::temp_dir().join(format!("nook-7z-test-{}", stamp()));
        let src = root.join("bundle");
        for (rel, body) in [
            ("one/plugin.toml", "id = \"one\"\nname = \"One\"\nrun = [\"a\"]"),
            ("one/main.py", "print(1)"),
            ("two/plugin.toml", "id = \"two\"\nname = \"Two\"\nrun = [\"b\"]"),
            ("two/lib/x.py", "pass"),
        ] {
            let path = src.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, body).unwrap();
        }
        let file = root.join("bundle.7z");
        sevenz_rust2::compress_to_path(&src, &file).unwrap();
        assert!(is_archive(&file));
        let found = plugins_in_archive(&file).unwrap();
        assert_eq!(found.len(), 2, "{found:?}");
        for (folder, id) in found.iter().zip(["one", "two"]) {
            let source = Source::Archive {
                file: file.clone(),
                folder: folder.clone(),
            };
            assert_eq!(preview(&source, None).unwrap().id, id);
            let staging = fetch(&source, id).unwrap();
            assert!(staging.join("plugin.toml").is_file());
            let _ = std::fs::remove_dir_all(&staging);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Checks a real archive: `NOOK_PLUGIN_ARCHIVE=path cargo test -- --ignored`.
    #[test]
    #[ignore = "reads the archive named by NOOK_PLUGIN_ARCHIVE"]
    fn reads_a_real_archive() {
        let file = PathBuf::from(std::env::var("NOOK_PLUGIN_ARCHIVE").expect("set NOOK_PLUGIN_ARCHIVE"));
        let folders = plugins_in_archive(&file).unwrap();
        println!("plugin folders: {folders:?}");
        assert!(!folders.is_empty(), "no plugin.toml in {}", file.display());
        for folder in folders {
            let m = preview(&Source::Archive { file: file.clone(), folder }, None).unwrap();
            println!("  {} {} ({})", m.id, m.version, m.name);
        }
    }

    #[test]
    fn refuses_unsafe_archive_paths() {
        let into = std::env::temp_dir().join(format!("nook-unsafe-test-{}", stamp()));
        let entry = |name: &str| Entry {
            name: name.into(),
            data: b"x".to_vec(),
        };
        for bad in ["p/../../evil.txt", "p/C:/evil.txt"] {
            assert!(write_folder(&[entry(bad)], "p", &into).is_err(), "{bad}");
        }
        assert!(write_folder(&[entry("p/ok.txt")], "missing", &into).is_err());
        write_folder(&[entry("p/./sub//ok.txt")], "p", &into).unwrap();
        assert!(into.join("sub").join("ok.txt").is_file());
        let _ = std::fs::remove_dir_all(&into);
        // Not an archive at all.
        assert!(!is_archive(Path::new("notes.txt")));
        assert!(plugins_in_archive(Path::new("notes.txt")).is_err());
    }
}
