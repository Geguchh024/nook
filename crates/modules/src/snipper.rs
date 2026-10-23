//! Shared plumbing for the tools that start from the freeze-and-select
//! overlay (text extractor, lens search, color picker): open the overlay,
//! run the tool's work on the overlay thread once it closes, and bring the
//! result back to the UI. Also the small-tile look these tools share: the
//! icon, swapped for a check or a warning for a moment after each use.

use egui::{Color32, Response, Sense, Ui};
use nook_platform::snip::{Snip, SnipMode, SnipOutcome};
use nook_ui::module::TileCtx;
use nook_ui::theme::SIZE_ICON;
use nook_ui::{icons, paint};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

/// How long the check or warning replaces the icon.
const FLASH_FOR: Duration = Duration::from_millis(1600);

pub struct Snipper<T> {
    snip: Option<Snip>,
    /// The tool's work is running on the overlay thread.
    working: Arc<AtomicBool>,
    tx: Sender<T>,
    rx: Receiver<T>,
    repaint: Option<egui::Context>,
    /// Outcome of the last use, for the flash.
    flash: Option<(bool, Instant)>,
}

impl<T: Send + 'static> Default for Snipper<T> {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            snip: None,
            working: Arc::new(AtomicBool::new(false)),
            tx,
            rx,
            repaint: None,
            flash: None,
        }
    }
}

impl<T: Send + 'static> Snipper<T> {
    pub fn attach(&mut self, ctx: &egui::Context) {
        self.repaint = Some(ctx.clone());
    }

    /// The overlay is open or its result is being worked on.
    pub fn busy(&self) -> bool {
        self.snip.as_ref().is_some_and(Snip::is_open) || self.working.load(Ordering::Relaxed)
    }

    /// Opens the overlay, or closes it if it is already open. `work` runs
    /// on the overlay thread after it closes; what it returns comes back
    /// through [`Snipper::poll`].
    pub fn toggle(
        &mut self,
        mode: SnipMode,
        hint: &str,
        work: impl FnOnce(SnipOutcome) -> Option<T> + Send + 'static,
    ) {
        if self.busy() {
            self.snip = None;
            return;
        }
        let tx = self.tx.clone();
        let working = self.working.clone();
        let repaint = self.repaint.clone();
        let done = move |outcome: SnipOutcome| {
            working.store(true, Ordering::Relaxed);
            if let Some(result) = work(outcome) {
                let _ = tx.send(result);
            }
            working.store(false, Ordering::Relaxed);
            if let Some(ctx) = &repaint {
                ctx.request_repaint();
            }
        };
        match Snip::open(mode, hint, done) {
            Ok(s) => self.snip = Some(s),
            Err(e) => {
                tracing::warn!("snip overlay: {e}");
                self.flash(false);
            }
        }
    }

    /// Results that arrived since the last call. Call once per tick.
    pub fn poll(&mut self) -> Vec<T> {
        if self.snip.as_ref().is_some_and(|s| !s.is_open()) {
            self.snip = None;
        }
        self.rx.try_iter().collect()
    }

    /// Shows a check (`ok`) or a warning in place of the icon for a moment.
    pub fn flash(&mut self, ok: bool) {
        self.flash = Some((ok, Instant::now()));
        if let Some(ctx) = &self.repaint {
            ctx.request_repaint();
        }
    }

    fn flashing(&self) -> Option<bool> {
        self.flash
            .filter(|(_, at)| at.elapsed() < FLASH_FOR)
            .map(|(ok, _)| ok)
    }

    /// A frame for the end of the flash.
    pub fn next_repaint(&self) -> Option<Duration> {
        let (_, at) = self.flash?;
        FLASH_FOR
            .checked_sub(at.elapsed())
            .map(|d| d + Duration::from_millis(5))
    }

    /// Draws a small tool tile and returns its click response.
    pub fn tile(&self, ui: &mut Ui, ctx: &TileCtx<'_>, glyph: &'static str, id: &str) -> Response {
        let theme = ctx.theme;
        let rect = ui.max_rect();
        let resp = ui.interact(rect.expand(8.0), ui.id().with(id), Sense::click());
        let busy = self.busy();
        let (glyph, color, fill): (&str, Color32, bool) = match self.flashing() {
            Some(true) => (icons::CHECK, theme.accent_fg(), true),
            Some(false) => (icons::WARNING, theme.text_dim, false),
            None if busy => (glyph, theme.accent_fg(), true),
            None if ctx.hovered => (glyph, theme.text, false),
            None => (glyph, theme.text_dim, false),
        };
        paint::icon(
            ui.painter(),
            rect.center(),
            glyph,
            SIZE_ICON + 2.0,
            color,
            fill,
        );
        resp
    }
}
