//! `plugin.toml`: what a plugin is called, how its tile looks and how to
//! start it. Parsed once at start-up.

use nook_ui::module::TileSize;
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// How Nook runs the plugin's program.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// One long-lived process: JSON lines in on stdin, out on stdout.
    #[default]
    Live,
    /// Run to completion every `interval` seconds (and on every event);
    /// whatever it prints replaces the view.
    Poll,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    /// Unique, lowercase: `a-z`, `0-9`, `-`, `_`. Nook prefixes it with
    /// `plugin:` in its config.
    pub id: String,
    pub name: String,
    /// Phosphor icon name, e.g. `"cloud-sun"`.
    #[serde(default = "default_icon")]
    pub icon: String,
    #[serde(default = "default_size")]
    pub size: TileSize,
    /// The program and its arguments, run from the plugin's folder.
    pub run: Vec<String>,
    #[serde(default)]
    pub mode: Mode,
    /// Seconds between runs in poll mode.
    #[serde(default = "default_interval")]
    pub interval: f64,
    /// Global shortcut in `global-hotkey` syntax, e.g. `"ctrl+alt+KeyW"`.
    pub hotkey: Option<String>,
    /// Files dropped on the notch go to this plugin while its page is open.
    #[serde(default)]
    pub accepts_files: bool,
    /// Size the panel grows to while the page is open, in points.
    pub page_size: Option<[f32; 2]>,
    /// Informational; shown in logs and the docs' plugin list.
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub version: String,
}

fn default_icon() -> String {
    "puzzle-piece".into()
}

fn default_size() -> TileSize {
    TileSize::Small
}

fn default_interval() -> f64 {
    60.0
}

/// A manifest plus the folder it came from.
#[derive(Clone, Debug)]
pub struct Found {
    pub manifest: Manifest,
    pub dir: PathBuf,
}

impl Manifest {
    pub fn parse(text: &str) -> Result<Self, String> {
        let m: Manifest = toml::from_str(text).map_err(|e| e.to_string())?;
        m.validate()?;
        Ok(m)
    }

    fn validate(&self) -> Result<(), String> {
        let ok = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_';
        if self.id.is_empty() || self.id.len() > 48 || !self.id.chars().all(ok) {
            return Err(format!(
                "id {:?} must be 1-48 characters of a-z, 0-9, - and _",
                self.id
            ));
        }
        if self.run.is_empty() || self.run[0].trim().is_empty() {
            return Err("run must name a program, e.g. run = [\"python\", \"main.py\"]".into());
        }
        if self.mode == Mode::Poll && self.interval < 1.0 {
            return Err("interval must be at least 1 second".into());
        }
        Ok(())
    }
}

/// Folders searched for plugins, in order: `%APPDATA%\Nook\plugins`,
/// `plugins` next to `nook.exe`, then each folder in `NOOK_PLUGIN_PATH`
/// (separated by `;`).
pub fn search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(a) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(a).join("Nook").join("plugins"));
    }
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
    {
        dirs.push(exe_dir.join("plugins"));
    }
    if let Some(extra) = std::env::var_os("NOOK_PLUGIN_PATH") {
        dirs.extend(std::env::split_paths(&extra));
    }
    dirs
}

/// Every valid plugin under `dirs` (one level deep). The first plugin with a
/// given id wins; broken manifests are logged and skipped.
pub fn discover(dirs: &[PathBuf]) -> Vec<Found> {
    let mut found: Vec<Found> = Vec::new();
    for root in dirs {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        let mut folders: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            // Hidden folders (".git", leftovers) are never plugins.
            .filter(|p| {
                !p.file_name()
                    .is_some_and(|n| n.to_string_lossy().starts_with('.'))
            })
            .collect();
        folders.sort();
        for dir in folders {
            let file = dir.join("plugin.toml");
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            match Manifest::parse(&text) {
                Ok(manifest) if found.iter().any(|f| f.manifest.id == manifest.id) => {
                    tracing::warn!(
                        "plugin {} in {} skipped: that id is already loaded",
                        manifest.id,
                        dir.display()
                    );
                }
                Ok(manifest) => found.push(Found { manifest, dir }),
                Err(e) => tracing::warn!("plugin {}: {e}", file.display()),
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_and_full() {
        let m =
            Manifest::parse("id = \"hello\"\nname = \"Hello\"\nrun = [\"python\", \"main.py\"]")
                .unwrap();
        assert_eq!(m.mode, Mode::Live);
        assert_eq!(m.size, TileSize::Small);
        assert_eq!(m.icon, "puzzle-piece");

        let m = Manifest::parse(
            r#"
            id = "disk"
            name = "Disk"
            icon = "hard-drives"
            size = "wide"
            run = ["powershell", "-File", "disk.ps1"]
            mode = "poll"
            interval = 30
            hotkey = "ctrl+alt+KeyY"
            page_size = [640, 360]
            "#,
        )
        .unwrap();
        assert_eq!(m.mode, Mode::Poll);
        assert_eq!(m.size, TileSize::Wide);
        assert_eq!(m.page_size, Some([640.0, 360.0]));
    }

    #[test]
    fn rejects_bad_ids_and_empty_run() {
        assert!(Manifest::parse("id = \"Bad Id\"\nname = \"x\"\nrun = [\"a\"]").is_err());
        assert!(Manifest::parse("id = \"ok\"\nname = \"x\"\nrun = []").is_err());
        assert!(Manifest::parse("id = \"ok\"\nname = \"x\"\nrun = [\"a\"]\nwat = 1").is_err());
    }
}
