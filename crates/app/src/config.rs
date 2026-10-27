//! User configuration in `%APPDATA%\Nook\config.toml`.

use anyhow::{Context, Result};
use nook_ui::module::TileSize;
use nook_ui::shell::Placement;
use nook_ui::theme::{Look, ThemeMode, parse_hex};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccentSource {
    /// Dominant color of the desktop wallpaper.
    #[default]
    Wallpaper,
    /// Windows accent color.
    System,
    /// `accent_hex`.
    Custom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MonitorMode {
    #[default]
    Primary,
    /// Follow the monitor under the mouse cursor.
    Cursor,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Material {
    /// Blurred acrylic backdrop where supported.
    #[default]
    Acrylic,
    /// Opaque glass, no backdrop blur.
    Solid,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GrainSize {
    /// One physical pixel per speck.
    #[default]
    Fine,
    Medium,
    Coarse,
}

impl GrainSize {
    pub const ALL: [GrainSize; 3] = [GrainSize::Fine, GrainSize::Medium, GrainSize::Coarse];

    pub fn pixels(self) -> f32 {
        match self {
            GrainSize::Fine => 1.0,
            GrainSize::Medium => 2.0,
            GrainSize::Coarse => 3.0,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModulesConfig {
    /// Tile order by module id. Unknown ids are ignored; unlisted modules
    /// are appended.
    pub order: Vec<String>,
    pub disabled: Vec<String>,
    /// Tile sizes picked in the editor; modules not listed use their own.
    pub sizes: std::collections::BTreeMap<String, TileSize>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub theme: ThemeMode,
    pub accent: AccentSource,
    pub accent_hex: String,
    pub monitor: MonitorMode,
    pub material: Material,
    /// Glass opacity over the blurred backdrop, in percent (Glass material).
    pub glass_opacity: u8,
    /// `#RRGGBB` the panel background is tinted toward; empty for neutral.
    pub background_hex: String,
    /// Film-grain strength in percent; 0 turns the grain off.
    pub grain: u8,
    pub grain_size: GrainSize,
    /// Top-left corner, top center or top-right corner.
    pub placement: Placement,
    /// Above all windows (true) or behind them.
    pub on_top: bool,
    /// Fade the notch out while a fullscreen app (game, video) is in front
    /// on its monitor. Off: the notch is always visible.
    pub hide_in_fullscreen: bool,
    pub hover_delay_ms: u32,
    pub collapse_grace_ms: u32,
    pub launch_at_startup: bool,
    /// Global shortcut that opens and pins the panel.
    pub toggle_hotkey: String,
    /// Plugin dev mode: plugins restart when their files are saved.
    pub plugin_dev: bool,
    /// Look for a new release on GitHub at start and every few hours.
    pub check_updates: bool,
    /// Send the anonymous daily "opened" ping (see `stats.rs`).
    pub usage_stats: bool,
    /// Random id the stats ping counts installs by; made on first run.
    pub install_id: String,
    pub modules: ModulesConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: ThemeMode::Auto,
            accent: AccentSource::Wallpaper,
            accent_hex: "#7C5CFF".into(),
            monitor: MonitorMode::Primary,
            material: Material::Acrylic,
            glass_opacity: DEFAULT_GLASS_OPACITY,
            background_hex: String::new(),
            grain: DEFAULT_GRAIN,
            grain_size: GrainSize::Fine,
            placement: Placement::Center,
            on_top: true,
            hide_in_fullscreen: false,
            hover_delay_ms: 120,
            collapse_grace_ms: 250,
            launch_at_startup: false,
            toggle_hotkey: "ctrl+alt+N".into(),
            plugin_dev: false,
            check_updates: true,
            usage_stats: true,
            install_id: String::new(),
            modules: ModulesConfig {
                order: nook_modules::DEFAULT_ORDER
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
                disabled: Vec::new(),
                sizes: Default::default(),
            },
        }
    }
}

pub const DEFAULT_GLASS_OPACITY: u8 = 70;
pub const DEFAULT_GRAIN: u8 = 35;

impl Config {
    /// The styling settings as the theme takes them.
    pub fn look(&self) -> Look {
        Look {
            glass_opacity: self.glass_opacity.clamp(20, 100) as f32 / 100.0,
            tint: parse_hex(&self.background_hex),
            grain: self.grain.min(100) as f32 / 100.0,
            grain_px: self.grain_size.pixels(),
        }
    }

    /// Puts the styling settings (not theme, accent or material) back to
    /// their defaults.
    pub fn reset_look(&mut self) {
        let d = Config::default();
        self.glass_opacity = d.glass_opacity;
        self.background_hex = d.background_hex;
        self.grain = d.grain;
        self.grain_size = d.grain_size;
    }
}

pub fn dir() -> Result<PathBuf> {
    let appdata = std::env::var_os("APPDATA").context("APPDATA is not set")?;
    Ok(PathBuf::from(appdata).join("Nook"))
}

pub fn path() -> Result<PathBuf> {
    Ok(dir()?.join("config.toml"))
}

impl Config {
    /// Loads the config, writing defaults on first run. A malformed file is
    /// kept as `config.toml.bad` and defaults are used.
    pub fn load() -> Result<Self> {
        let path = path()?;
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Config>(&text) {
                Ok(cfg) => Ok(cfg),
                Err(e) => {
                    tracing::error!("config parse error, using defaults: {e}");
                    let _ = std::fs::rename(&path, path.with_extension("toml.bad"));
                    let cfg = Config::default();
                    cfg.save()?;
                    Ok(cfg)
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let cfg = Config::default();
                cfg.save()?;
                Ok(cfg)
            }
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self) -> Result<()> {
        let dir = dir()?;
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let text = toml::to_string_pretty(self).context("serializing config")?;
        let path = path()?;
        // Write-then-rename so a crash never leaves a half-written file.
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, &path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let cfg = Config::default();
        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&text).expect("parse");
        assert_eq!(cfg, back);
    }

    #[test]
    fn missing_fields_use_defaults() {
        let cfg: Config = toml::from_str("theme = \"dark\"").expect("parse");
        assert_eq!(cfg.theme, ThemeMode::Dark);
        assert_eq!(cfg.hover_delay_ms, 120);
        assert_eq!(cfg.placement, Placement::Center);
        assert!(cfg.on_top);
    }

    #[test]
    fn module_sizes_round_trip() {
        let text = "[modules]\norder = [\"timer\"]\n[modules.sizes]\ntimer = \"wide\"\n";
        let cfg: Config = toml::from_str(text).expect("parse");
        assert_eq!(cfg.modules.sizes.get("timer"), Some(&TileSize::Wide));
        let back: Config =
            toml::from_str(&toml::to_string_pretty(&cfg).expect("ser")).expect("parse");
        assert_eq!(cfg, back);
    }

    #[test]
    fn look_settings_parse_and_map() {
        let text =
            "grain = 0\ngrain_size = \"coarse\"\nbackground_hex = \"#336699\"\nglass_opacity = 5";
        let cfg: Config = toml::from_str(text).expect("parse");
        let look = cfg.look();
        assert_eq!(look.grain, 0.0);
        assert_eq!(look.grain_px, 3.0);
        assert!(look.tint.is_some());
        // Clamped so the glass never disappears entirely.
        assert_eq!(look.glass_opacity, 0.2);
        let old: Config = toml::from_str("theme = \"dark\"").expect("parse");
        assert_eq!(old.grain, DEFAULT_GRAIN);
        assert_eq!(old.look().tint, None);
    }

    #[test]
    fn placement_is_lowercase_in_toml() {
        let cfg: Config = toml::from_str("placement = \"right\"\non_top = false").expect("parse");
        assert_eq!(cfg.placement, Placement::Right);
        assert!(!cfg.on_top);
    }
}
