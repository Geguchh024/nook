//! Nook's utility modules. Each lives in its own folder and implements
//! [`nook_ui::module::Module`]; [`register_all`] wires them into a registry in
//! the default layout order, followed by any [`plugins`] found on disk.

pub mod calculator;
pub mod calendar;
pub mod clipboard;
pub mod coffee;
pub mod color_picker;
pub mod draw;
pub mod file_shelf;
pub mod font_detector;
pub mod kit;
pub mod lens;
pub mod media;
pub mod notes;
pub mod phone_link;
pub mod plugins;
pub mod qr_code;
pub mod quick_launch;
pub mod screenshot;
mod shot_editor;
mod snipper;
pub mod stub;
pub mod system_monitor;
pub mod text_extractor;
pub mod timer;
pub mod voice_memo;
pub mod wallpaper;

use nook_ui::module::Registry;

/// Default tile order. Packs the first 8x3 grid exactly with the settings
/// tile (added by the app) in the last cell; the rest go on the next page.
pub const DEFAULT_ORDER: &[&str] = &[
    "timer",
    "coffee",
    "media",
    "notes",
    "clipboard",
    "color_picker",
    "screenshot",
    "ocr",
    "lens",
    "draw",
    "phone_link",
    "file_shelf",
    "settings",
    "font_detector",
    "wallpaper",
    "calculator",
    "system_monitor",
    "calendar",
    "quick_launch",
    "qr_code",
    "voice_memo",
    "plugins",
];

pub fn register_all(reg: &mut Registry) {
    reg.register(Box::<timer::Timer>::default());
    reg.register(Box::<coffee::Coffee>::default());
    reg.register(Box::<media::Media>::default());
    reg.register(Box::<notes::Notes>::default());
    reg.register(Box::<clipboard::Clipboard>::default());
    reg.register(Box::<color_picker::ColorPicker>::default());
    reg.register(Box::<screenshot::Screenshot>::default());
    reg.register(Box::<text_extractor::TextExtractor>::default());
    reg.register(Box::<lens::Lens>::default());
    reg.register(Box::<draw::Draw>::default());
    reg.register(Box::<phone_link::PhoneLink>::default());
    reg.register(Box::<file_shelf::FileShelf>::default());
    // Past the first grid: the second page.
    reg.register(Box::<font_detector::FontDetector>::default());
    reg.register(Box::<wallpaper::Wallpaper>::default());
    reg.register(Box::<calculator::Calculator>::default());
    reg.register(Box::<system_monitor::SystemMonitor>::default());
    reg.register(Box::<calendar::Calendar>::default());
    reg.register(Box::<quick_launch::QuickLaunch>::default());
    reg.register(Box::<qr_code::QrCodeModule>::default());
    reg.register(Box::<voice_memo::VoiceMemo>::default());
    // The plugin manager, then third-party tiles from the plugin folders.
    reg.register(Box::<plugins::PluginsTile>::default());
    plugins::register(reg);
}
