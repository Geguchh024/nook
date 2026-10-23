//! Coffee: keeps the PC (and the screen) awake, forever or for a while.
//! Click the text to pick how long, flip the switch to start. A cup shows
//! in the pill while it is on.
//!
//! `SetThreadExecutionState` is per thread and Windows drops it when the
//! thread ends, so one long-lived worker owns it; even a crash cannot leave
//! the machine stuck awake.

use egui::{Align2, Rect, Sense, Ui, UiBuilder, pos2};
use nook_platform::power;
use nook_ui::module::{Badge, Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_ICON, SIZE_LABEL, Theme};
use nook_ui::{icons, paint, widgets};
use std::sync::mpsc::{self, Sender};
use std::time::{Duration, Instant};

/// Choices for how long to stay awake (`None` = until turned off).
const DURATIONS: [Option<u64>; 5] = [None, Some(15), Some(30), Some(60), Some(120)];

fn duration_label(minutes: Option<u64>) -> String {
    match minutes {
        None => "Always".into(),
        Some(m) if m >= 60 => format!("{} h", m / 60),
        Some(m) => format!("{m} min"),
    }
}

/// "1:05 left" / "12m left" (the tile is narrow).
fn remaining_label(left: Duration) -> String {
    let mins = left.as_secs().div_ceil(60);
    if mins >= 60 {
        format!("{}:{:02} left", mins / 60, mins % 60)
    } else {
        format!("{mins}m left")
    }
}

pub struct Coffee {
    /// Index into [`DURATIONS`].
    choice: usize,
    /// When keep-awake was switched on, and when it ends.
    on: Option<(Instant, Option<Instant>)>,
    worker: Option<Sender<bool>>,
}

impl Default for Coffee {
    fn default() -> Self {
        Self {
            choice: 0,
            on: None,
            worker: spawn_worker(),
        }
    }
}

/// The thread that holds the execution state: `true` = stay awake.
fn spawn_worker() -> Option<Sender<bool>> {
    let (tx, rx) = mpsc::channel::<bool>();
    let spawned = std::thread::Builder::new()
        .name("nook-coffee".into())
        .spawn(move || {
            for awake in rx {
                let res = if awake {
                    power::keep_awake(true)
                } else {
                    power::release()
                };
                if let Err(e) = res {
                    tracing::warn!("keep-awake: {e}");
                }
            }
            let _ = power::release();
        });
    match spawned {
        Ok(_) => Some(tx),
        Err(e) => {
            tracing::warn!("spawning keep-awake thread: {e}");
            None
        }
    }
}

impl Coffee {
    fn set(&mut self, on: bool) {
        if on == self.on.is_some() {
            return;
        }
        self.on = on.then(|| {
            let now = Instant::now();
            let until = DURATIONS[self.choice].map(|m| now + Duration::from_secs(m * 60));
            (now, until)
        });
        if let Some(w) = &self.worker {
            let _ = w.send(on);
        }
        tracing::info!("keep awake: {on}");
    }

    fn left(&self) -> Option<Duration> {
        let (_, until) = self.on?;
        until.map(|u| u.saturating_duration_since(Instant::now()))
    }
}

impl Module for Coffee {
    fn id(&self) -> &'static str {
        "coffee"
    }

    fn name(&self) -> &str {
        "Coffee"
    }

    fn icon(&self) -> &'static str {
        icons::COFFEE
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Wide
    }

    fn is_active(&self) -> bool {
        self.on.is_some()
    }

    fn collapsed_badge(&self) -> Option<Badge> {
        self.is_active().then_some(Badge::Glyph {
            icon: icons::COFFEE,
        })
    }

    fn on_tick(&mut self) {
        if self.left() == Some(Duration::ZERO) {
            self.set(false);
        }
    }

    fn next_repaint(&self, tile_visible: bool) -> Option<Duration> {
        let left = self.left()?;
        // Wake for the end; tick the "min left" text while visible.
        let tick = if tile_visible {
            Duration::from_secs(1 + left.as_secs() % 60)
        } else {
            left
        };
        Some(tick.min(left) + Duration::from_millis(5))
    }

    fn default_hotkey(&self) -> Option<&'static str> {
        Some("ctrl+alt+KeyC")
    }

    fn on_hotkey(&mut self) {
        let on = self.on.is_none();
        self.set(on);
    }

    fn shutdown(&mut self) {
        self.set(false);
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        let rect = ui.max_rect();
        let active = self.is_active();
        let cy = rect.center().y;
        let painter = ui.painter().clone();

        let icon_color = if active {
            theme.accent_fg()
        } else {
            theme.text_dim
        };
        paint::icon(
            &painter,
            pos2(rect.min.x + 8.0, cy),
            icons::COFFEE,
            SIZE_ICON,
            icon_color,
            active,
        );

        // Name and status; clicking them cycles the duration while off.
        let text = Rect::from_min_max(
            pos2(rect.min.x + 22.0, rect.min.y),
            pos2(rect.max.x - 38.0, rect.max.y),
        );
        let resp = ui.interact(text, ui.id().with("coffee-duration"), Sense::click());
        if resp.clicked() && !active {
            self.choice = (self.choice + 1) % DURATIONS.len();
        }
        let status = match (self.on, self.left()) {
            (Some(_), Some(left)) => remaining_label(left),
            (Some(_), None) => "Awake".to_owned(),
            (None, _) => duration_label(DURATIONS[self.choice]),
        };
        painter.text(
            pos2(text.min.x, cy - 7.0),
            Align2::LEFT_CENTER,
            "Coffee",
            Theme::font_semibold(SIZE_LABEL + 1.0),
            theme.text,
        );
        let status_color = if resp.hovered() && !active {
            theme.text
        } else {
            theme.text_dim
        };
        painter.text(
            pos2(text.min.x, cy + 8.0),
            Align2::LEFT_CENTER,
            status,
            Theme::font(SIZE_LABEL - 1.0),
            status_color,
        );
        if !active {
            resp.on_hover_text("Click to change how long");
        }

        // Switch on the right.
        let sw = Rect::from_center_size(
            pos2(rect.max.x - widgets::TOGGLE_SIZE.x * 0.5, cy),
            widgets::TOGGLE_SIZE,
        );
        let mut sui = ui.new_child(UiBuilder::new().max_rect(sw));
        let mut on = active;
        if widgets::toggle(&mut sui, &mut on, theme).changed() {
            self.set(on);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(duration_label(None), "Always");
        assert_eq!(duration_label(Some(30)), "30 min");
        assert_eq!(duration_label(Some(120)), "2 h");
        assert_eq!(remaining_label(Duration::from_secs(61)), "2m left");
        assert_eq!(remaining_label(Duration::from_secs(3900)), "1:05 left");
    }
}
