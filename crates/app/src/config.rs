//! User configuration in `%APPDATA%\Nook\config.toml`.

use anyhow::{Context, Result};
use nook_ui::geometry::Corners;
use nook_ui::module::{RadioRestore, TileSize};
use nook_ui::shell::Placement;
use nook_ui::shell_state::MotionEasing;
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
    /// Use the connected display selected in Settings.
    Display,
    /// Follow the cursor, but only on selected displays.
    SelectedDisplays,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Material {
    /// Blurred acrylic backdrop where supported.
    #[default]
    Acrylic,
    /// Opaque glass, no backdrop blur.
    Solid,
    /// Pure black OLED surface with subtle elevated controls.
    Oled,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Renderer {
    /// DirectX 12, falling back to OpenGL when it cannot start.
    #[default]
    Auto,
    /// DirectX 12 through a DirectComposition swapchain.
    DirectX,
    /// OpenGL. Some drivers drop its alpha and the window comes out black.
    OpenGl,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellStyle {
    #[default]
    Pill,
    Notch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DookPosition {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DookStyle {
    #[default]
    Pill,
    Notch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DookTarget {
    #[default]
    Nook,
    Windows,
}

/// A compact, user-configurable dock that can replace the Windows taskbar.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DookConfig {
    pub enabled: bool,
    pub hide_windows_taskbar: bool,
    pub auto_hide: bool,
    pub style: DookStyle,
    pub apps_position: DookPosition,
    pub oled: bool,
    pub monochrome_icons: bool,
    pub background_hex: String,
    pub icon_hex: String,
    pub radius: u8,
    pub button_radius: u8,
    pub start_target: DookTarget,
    pub search_target: DookTarget,
    pub quick_settings_target: DookTarget,
    pub show_start: bool,
    pub show_search: bool,
    pub show_quick_settings: bool,
    pub show_clock: bool,
    pub show_date: bool,
    pub show_wifi: bool,
    pub show_volume: bool,
    pub show_battery: bool,
    pub show_emoji: bool,
}

impl Default for DookConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            hide_windows_taskbar: false,
            auto_hide: false,
            style: DookStyle::Pill,
            apps_position: DookPosition::Center,
            oled: false,
            monochrome_icons: true,
            background_hex: "#17171A".into(),
            icon_hex: "#D7D7DE".into(),
            radius: 28,
            button_radius: 12,
            start_target: DookTarget::Nook,
            search_target: DookTarget::Windows,
            quick_settings_target: DookTarget::Windows,
            show_start: true,
            show_search: true,
            show_quick_settings: true,
            show_clock: true,
            show_date: true,
            show_wifi: true,
            show_volume: true,
            show_battery: true,
            show_emoji: true,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotchSize {
    Compact,
    #[default]
    Standard,
    Large,
}

impl NotchSize {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Standard, Self::Large];

    pub fn points(self) -> egui::Vec2 {
        match self {
            Self::Compact => egui::vec2(160.0, 28.0),
            Self::Standard => egui::vec2(200.0, 32.0),
            Self::Large => egui::vec2(250.0, 40.0),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClockStyle {
    #[default]
    Classic,
    Digital,
    DotMatrix,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionPeriod {
    OneDay,
    ThreeDays,
    FiveDays,
    SevenDays,
    TenDays,
    FifteenDays,
    OneMonth,
    #[default]
    Never,
}

impl RetentionPeriod {
    pub const ALL: [Self; 8] = [
        Self::OneDay,
        Self::ThreeDays,
        Self::FiveDays,
        Self::SevenDays,
        Self::TenDays,
        Self::FifteenDays,
        Self::OneMonth,
        Self::Never,
    ];

    pub const fn days(self) -> Option<u32> {
        match self {
            Self::OneDay => Some(1),
            Self::ThreeDays => Some(3),
            Self::FiveDays => Some(5),
            Self::SevenDays => Some(7),
            Self::TenDays => Some(10),
            Self::FifteenDays => Some(15),
            Self::OneMonth => Some(30),
            Self::Never => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::OneDay => "1 day",
            Self::ThreeDays => "3 days",
            Self::FiveDays => "5 days",
            Self::SevenDays => "7 days",
            Self::TenDays => "10 days",
            Self::FifteenDays => "15 days",
            Self::OneMonth => "1 month",
            Self::Never => "Never",
        }
    }
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
    /// Per-tile continuous-corner radius overrides in logical points.
    pub radii: std::collections::BTreeMap<String, u8>,
    /// Per-tile top-left, top-right, bottom-right, bottom-left radii.
    pub corners: std::collections::BTreeMap<String, Corners>,
    /// Spacing between layout tiles, in logical points.
    #[serde(default = "default_layout_gap")]
    pub layout_gap: u8,
    /// Radios that were on before the experimental Airplane tile disabled them.
    pub airplane_restore: Option<RadioRestore>,
}

fn default_layout_gap() -> u8 {
    8
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub shell_style: ShellStyle,
    pub dook: DookConfig,
    pub notch_size: NotchSize,
    pub clock_style: ClockStyle,
    pub clipboard_retention: RetentionPeriod,
    pub notes_retention: RetentionPeriod,
    /// Play the short UI sound cues.
    pub sound_effects: bool,
    pub theme: ThemeMode,
    pub accent: AccentSource,
    pub accent_hex: String,
    pub monitor: MonitorMode,
    /// Zero-based entry in the sorted connected-display list.
    pub monitor_display: u8,
    /// Displays the notch may follow when `monitor` is `selected_displays`.
    pub monitor_displays: Vec<u8>,
    pub material: Material,
    /// Glass opacity over the blurred backdrop, in percent (Glass material).
    pub glass_opacity: u8,
    /// `#RRGGBB` the panel background is tinted toward; empty for neutral.
    pub background_hex: String,
    /// `#RRGGBB` tint for cards, sections and tile surfaces.
    pub surface_hex: String,
    /// `#RRGGBB` tint for buttons and compact controls.
    pub control_hex: String,
    /// Top offset of the closed pill, in logical points. Notch mode stays attached to the edge.
    pub pill_margin: u8,
    /// Opacity of the single outer shell outline, in percent.
    pub shell_border: u8,
    /// Strength of the outer shell shadow, in percent.
    pub shell_shadow: u8,
    /// Hover glow around the shell, in percent.
    pub hover_glow: u8,
    /// Shell open/close animation duration, milliseconds.
    pub motion_duration_ms: u16,
    pub motion_easing: MotionEasing,
    /// Show a small media glyph in the collapsed notch when audio is playing.
    pub media_in_closed_notch: bool,
    /// Radius of shared component buttons, in logical points.
    pub component_radius: u8,
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
    /// GPU backend; read at startup only.
    pub renderer: Renderer,
    /// Random id the stats ping counts installs by; made on first run.
    pub install_id: String,
    pub modules: ModulesConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shell_style: ShellStyle::Pill,
            dook: DookConfig::default(),
            notch_size: NotchSize::Standard,
            clock_style: ClockStyle::Classic,
            clipboard_retention: RetentionPeriod::Never,
            notes_retention: RetentionPeriod::Never,
            sound_effects: true,
            theme: ThemeMode::Auto,
            accent: AccentSource::Wallpaper,
            accent_hex: "#7C5CFF".into(),
            monitor: MonitorMode::Primary,
            monitor_display: 0,
            monitor_displays: vec![0],
            material: Material::Acrylic,
            glass_opacity: DEFAULT_GLASS_OPACITY,
            background_hex: String::new(),
            surface_hex: String::new(),
            control_hex: String::new(),
            pill_margin: 6,
            shell_border: 100,
            shell_shadow: 0,
            hover_glow: 12,
            motion_duration_ms: 370,
            motion_easing: MotionEasing::Spring,
            media_in_closed_notch: true,
            component_radius: 13,
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
            renderer: Renderer::Auto,
            install_id: String::new(),
            modules: ModulesConfig {
                order: nook_modules::DEFAULT_ORDER
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
                disabled: Vec::new(),
                sizes: Default::default(),
                radii: Default::default(),
                corners: Default::default(),
                layout_gap: default_layout_gap(),
                airplane_restore: None,
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
            surface_tint: parse_hex(&self.surface_hex),
            control_tint: parse_hex(&self.control_hex),
            oled: self.material == Material::Oled,
            shell_border: self.shell_border.min(100) as f32 / 100.0,
            shell_shadow: self.shell_shadow.min(100) as f32 / 100.0,
            hover_glow: self.hover_glow.min(100) as f32 / 100.0,
            component_radius: self.component_radius.min(24) as f32,
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
        self.surface_hex = d.surface_hex;
        self.control_hex = d.control_hex;
        self.shell_border = d.shell_border;
        self.shell_shadow = d.shell_shadow;
        self.component_radius = d.component_radius;
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
                Ok(mut cfg) => {
                    cfg.migrate_legacy_corners();
                    Ok(cfg)
                }
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

    fn migrate_legacy_corners(&mut self) {
        for (id, radius) in &self.modules.radii {
            self.modules
                .corners
                .entry(id.clone())
                .or_insert_with(|| Corners::uniform(*radius as f32));
        }
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
        assert!(cfg.sound_effects);
        assert_eq!(cfg.hover_delay_ms, 120);
        assert_eq!(cfg.placement, Placement::Center);
        assert!(cfg.on_top);
        assert!(!cfg.dook.enabled);
        assert!(cfg.dook.show_clock);
        assert_eq!(cfg.dook.apps_position, DookPosition::Center);
        assert_eq!(cfg.dook.style, DookStyle::Pill);
    }

    #[test]
    fn module_sizes_round_trip() {
        let text = "[modules]\norder = [\"timer\"]\n[modules.sizes]\ntimer = \"wide\"\n[modules.radii]\ntimer = 24\n";
        let cfg: Config = toml::from_str(text).expect("parse");
        assert_eq!(cfg.modules.sizes.get("timer"), Some(&TileSize::Wide));
        assert_eq!(cfg.modules.radii.get("timer"), Some(&24));
        let back: Config =
            toml::from_str(&toml::to_string_pretty(&cfg).expect("ser")).expect("parse");
        assert_eq!(cfg, back);
    }

    #[test]
    fn old_uniform_radius_is_migrated_to_four_equal_corners() {
        let mut cfg: Config = toml::from_str("[modules.radii]\ntimer = 24\n").expect("parse");
        cfg.migrate_legacy_corners();
        assert_eq!(
            cfg.modules.corners.get("timer"),
            Some(&Corners::uniform(24.0))
        );
    }

    #[test]
    fn per_corner_values_round_trip() {
        let mut cfg = Config::default();
        cfg.modules.corners.insert(
            "timer".into(),
            Corners {
                tl: 2.0,
                tr: 8.0,
                br: 14.0,
                bl: 20.0,
            },
        );
        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&text).expect("parse");
        assert_eq!(back.modules.corners, cfg.modules.corners);
    }

    #[test]
    fn old_module_config_gets_default_layout_gap() {
        let cfg: Config = toml::from_str("[modules]\norder = [\"timer\"]\n")
            .expect("legacy module config should parse");
        assert_eq!(cfg.modules.layout_gap, 8);
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
    fn renderer_parses_and_defaults_to_auto() {
        let cfg: Config = toml::from_str("renderer = \"opengl\"").expect("parse");
        assert_eq!(cfg.renderer, Renderer::OpenGl);
        let old: Config = toml::from_str("theme = \"dark\"").expect("parse");
        assert_eq!(old.renderer, Renderer::Auto);
    }

    #[test]
    fn placement_is_lowercase_in_toml() {
        let cfg: Config = toml::from_str("placement = \"right\"\non_top = false").expect("parse");
        assert_eq!(cfg.placement, Placement::Right);
        assert!(!cfg.on_top);
    }

    #[test]
    fn shell_style_is_snake_case_and_defaults_to_pill() {
        assert_eq!(Config::default().shell_style, ShellStyle::Pill);
        let cfg: Config = toml::from_str("shell_style = \"notch\"").expect("parse");
        assert_eq!(cfg.shell_style, ShellStyle::Notch);
    }

    #[test]
    fn sound_effects_default_on_and_round_trip() {
        let mut cfg = Config::default();
        assert!(cfg.sound_effects);
        cfg.sound_effects = false;
        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&text).expect("parse");
        assert!(!back.sound_effects);
    }

    #[test]
    fn notch_clock_and_retention_settings_round_trip() {
        let cfg = Config {
            notch_size: NotchSize::Large,
            clock_style: ClockStyle::DotMatrix,
            clipboard_retention: RetentionPeriod::SevenDays,
            notes_retention: RetentionPeriod::OneMonth,
            ..Config::default()
        };
        let text = toml::to_string_pretty(&cfg).expect("serialize");
        let back: Config = toml::from_str(&text).expect("parse");
        assert_eq!(back.notch_size, NotchSize::Large);
        assert_eq!(back.clock_style, ClockStyle::DotMatrix);
        assert_eq!(back.clipboard_retention, RetentionPeriod::SevenDays);
        assert_eq!(back.notes_retention, RetentionPeriod::OneMonth);
    }
}
