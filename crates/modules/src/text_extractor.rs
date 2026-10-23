//! Text extractor: drag over anything on screen and its text lands on the
//! clipboard, like PowerToys Text Extractor.
//!
//! The screen freezes (`nook_platform::snip`), the selection goes through
//! the OCR engine built into Windows (`nook_platform::ocr`), and the result
//! is copied with one line per row. Hold Shift as you let go to get it on a
//! single line. Recognition uses the first of your Windows display
//! languages that has an OCR pack. Ctrl+Alt+T starts it from anywhere.

use nook_platform::snip::{SnipMode, SnipOutcome};
use nook_platform::{clipboard, ocr};
use nook_ui::icons;
use nook_ui::module::{Module, TileCtx, TileSize};
use std::time::Duration;

use crate::snipper::Snipper;

const HINT: &str =
    "Drag over text to copy it  \u{00B7}  Shift for one line  \u{00B7}  Esc to cancel";

#[derive(Default)]
pub struct TextExtractor {
    snipper: Snipper<bool>,
}

/// Recognizes and copies; true when text was found and copied.
fn extract(outcome: SnipOutcome) -> Option<bool> {
    let SnipOutcome::Region { frame, single_line } = outcome else {
        return None;
    };
    let started = std::time::Instant::now();
    let text = match ocr::recognize(&frame, None) {
        Ok(t) => t,
        Err(e) => {
            tracing::warn!("text extractor: {e}");
            return Some(false);
        }
    };
    tracing::info!(
        "text extractor: {} lines ({}, {}x{}) in {:?}",
        text.lines.len(),
        text.language,
        frame.width,
        frame.height,
        started.elapsed()
    );
    if text.lines.is_empty() {
        return Some(false);
    }
    match clipboard::copy_text(&text.joined(single_line)) {
        Ok(()) => Some(true),
        Err(e) => {
            tracing::warn!("text extractor: copying: {e}");
            Some(false)
        }
    }
}

impl TextExtractor {
    fn start(&mut self) {
        self.snipper.toggle(SnipMode::Region, HINT, extract);
    }
}

impl Module for TextExtractor {
    fn id(&self) -> &'static str {
        "ocr"
    }

    fn name(&self) -> &str {
        "Text extractor"
    }

    fn icon(&self) -> &'static str {
        icons::TEXT_AA
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn attach(&mut self, ctx: &egui::Context) {
        self.snipper.attach(ctx);
    }

    fn is_active(&self) -> bool {
        self.snipper.busy()
    }

    fn overlay_open(&self) -> bool {
        self.snipper.busy()
    }

    fn on_tick(&mut self) {
        for ok in self.snipper.poll() {
            self.snipper.flash(ok);
        }
    }

    fn next_repaint(&self, _tile_visible: bool) -> Option<Duration> {
        self.snipper.next_repaint()
    }

    fn default_hotkey(&self) -> Option<&'static str> {
        Some("ctrl+alt+KeyT")
    }

    fn on_hotkey(&mut self) {
        self.start();
    }

    fn ui_tile(&mut self, ui: &mut egui::Ui, ctx: &mut TileCtx<'_>) {
        if self
            .snipper
            .tile(ui, ctx, icons::TEXT_AA, "ocr-tile")
            .clicked()
        {
            self.start();
        }
    }
}
