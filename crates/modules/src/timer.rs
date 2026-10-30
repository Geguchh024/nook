//! Timer: a countdown (scroll or +/- to set the minutes, or double-click the
//! time to type one) and a stopwatch.
//! The running time and its ring mirror into the collapsed pill, and the
//! countdown plays the Windows notification sound when it ends.
//!
//! The logic ([`Clock`]) takes the current time as an argument, so it is
//! unit-tested without sleeping; the module feeds it seconds since start-up
//! from a monotonic `Instant`.

use egui::{
    Align2, Event, Id, Key, MouseWheelUnit, Rect, Response, Sense, TextEdit, Ui, UiBuilder, pos2,
    vec2,
};
use nook_platform::system;
use nook_ui::bento::tile_header;
use nook_ui::module::{Badge, Module, TileCtx, TileSize};
use nook_ui::theme::{Theme, with_alpha};
use nook_ui::{icons, paint, widgets};
use std::time::{Duration, Instant};

const DEFAULT_SECS: u32 = 25 * 60;
const MAX_SECS: u32 = 180 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Countdown,
    Stopwatch,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Idle,
    /// Running since `since`, with `banked` seconds from earlier runs.
    Running {
        since: f64,
        banked: f64,
    },
    Paused {
        banked: f64,
    },
    /// The countdown reached zero.
    Finished,
}

/// Timer state machine. Times are seconds on any monotonic clock.
#[derive(Clone, Debug, PartialEq)]
pub struct Clock {
    pub mode: Mode,
    /// Countdown length in seconds.
    pub secs: u32,
    pub phase: Phase,
}

impl Default for Clock {
    fn default() -> Self {
        Self {
            mode: Mode::Countdown,
            secs: DEFAULT_SECS,
            phase: Phase::Idle,
        }
    }
}

impl Clock {
    pub fn duration(&self) -> f64 {
        f64::from(self.secs)
    }

    /// Seconds counted so far in this run.
    pub fn elapsed(&self, now: f64) -> f64 {
        match self.phase {
            Phase::Idle => 0.0,
            Phase::Running { since, banked } => banked + (now - since).max(0.0),
            Phase::Paused { banked } => banked,
            Phase::Finished => self.duration(),
        }
    }

    /// Countdown: seconds left. Stopwatch: seconds elapsed.
    pub fn display_secs(&self, now: f64) -> f64 {
        match self.mode {
            Mode::Countdown => (self.duration() - self.elapsed(now)).max(0.0),
            Mode::Stopwatch => self.elapsed(now),
        }
    }

    /// Ring fill: the countdown drains from full; the stopwatch sweeps once
    /// a minute.
    pub fn progress(&self, now: f64) -> f32 {
        match self.mode {
            Mode::Countdown if self.phase == Phase::Idle => 1.0,
            Mode::Countdown => (self.display_secs(now) / self.duration().max(1.0)) as f32,
            Mode::Stopwatch => ((self.elapsed(now) % 60.0) / 60.0) as f32,
        }
    }

    pub fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Running { .. })
    }

    /// Start, pause or resume. A finished countdown starts over.
    pub fn toggle(&mut self, now: f64) {
        self.phase = match self.phase {
            Phase::Idle | Phase::Finished => Phase::Running {
                since: now,
                banked: 0.0,
            },
            Phase::Running { .. } => Phase::Paused {
                banked: self.elapsed(now),
            },
            Phase::Paused { banked } => Phase::Running { since: now, banked },
        };
    }

    pub fn reset(&mut self) {
        self.phase = Phase::Idle;
    }

    /// Changes the countdown length by `delta` minutes (1 min to 3 h). While
    /// a countdown runs this extends or shortens the current run.
    pub fn adjust(&mut self, delta: i32) {
        self.secs = self
            .secs
            .saturating_add_signed(delta.saturating_mul(60))
            .clamp(60, MAX_SECS);
    }

    /// Sets a typed countdown length and goes back to idle.
    pub fn set_secs(&mut self, secs: u32) {
        self.secs = secs.clamp(1, MAX_SECS);
        self.phase = Phase::Idle;
    }

    /// Restarts a finished countdown for one more minute.
    pub fn snooze(&mut self, now: f64) {
        self.secs = 60;
        self.phase = Phase::Running {
            since: now,
            banked: 0.0,
        };
    }

    pub fn set_mode(&mut self, mode: Mode) {
        if self.mode != mode {
            self.mode = mode;
            self.phase = Phase::Idle;
        }
    }

    /// Advances the state; returns true exactly once, when a countdown ends.
    pub fn poll(&mut self, now: f64) -> bool {
        if self.mode == Mode::Countdown && self.is_running() && self.elapsed(now) >= self.duration()
        {
            self.phase = Phase::Finished;
            return true;
        }
        false
    }

    /// Seconds until the displayed value next changes, while running.
    pub fn until_next_second(&self, now: f64) -> Option<f64> {
        if !self.is_running() {
            return None;
        }
        let e = self.elapsed(now);
        let wait = match self.mode {
            // The display rounds remaining time up, so it ticks when the
            // elapsed time crosses a whole second.
            Mode::Countdown => {
                let next = e.floor() + 1.0;
                (next - e).min(self.duration() - e)
            }
            Mode::Stopwatch => e.floor() + 1.0 - e,
        };
        Some(wait.max(0.0))
    }
}

/// `m:ss`, or `h:mm:ss` from an hour up. The countdown rounds up so it
/// shows 0:00 only at the end.
pub fn format_secs(secs: f64, round_up: bool) -> String {
    let s = if round_up { secs.ceil() } else { secs.floor() };
    let s = s.max(0.0) as u64;
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Parses a typed countdown length into seconds: `25` (minutes), `5:30`
/// (m:ss, like the display), `1:05:00` (h:mm:ss), or units such as `90s`,
/// `45m`, `1h30m`, `1.5` (minutes). `None` if it is not a time or is out of
/// range (1 s to 3 h).
pub fn parse_duration(text: &str) -> Option<u32> {
    let t: String = text.trim().to_ascii_lowercase().split_whitespace().collect();
    if t.is_empty() {
        return None;
    }
    let secs = if t.contains(':') {
        let parts: Vec<u32> = t
            .split(':')
            .map(|p| p.parse().ok())
            .collect::<Option<_>>()?;
        match parts[..] {
            [m, s] if s < 60 => m.checked_mul(60)?.checked_add(s)?,
            [h, m, s] if m < 60 && s < 60 => h.checked_mul(3600)?.checked_add(m * 60 + s)?,
            _ => return None,
        }
    } else if let Ok(minutes) = t.parse::<f64>() {
        if !minutes.is_finite() || minutes < 0.0 {
            return None;
        }
        (minutes * 60.0).round().min(f64::from(u32::MAX)) as u32
    } else {
        // Number + unit pairs: 1h30m, 90s, 2min.
        let mut total: u32 = 0;
        let mut rest = t.as_str();
        while !rest.is_empty() {
            let digits = rest.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest.len());
            if digits == 0 {
                return None;
            }
            let n: u32 = rest[..digits].parse().ok()?;
            rest = &rest[digits..];
            let unit_len = rest.find(|c: char| c.is_ascii_digit()).unwrap_or(rest.len());
            let scale = match &rest[..unit_len] {
                "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
                "m" | "min" | "mins" | "minute" | "minutes" | "" => 60,
                "s" | "sec" | "secs" | "second" | "seconds" => 1,
                _ => return None,
            };
            total = total.checked_add(n.checked_mul(scale)?)?;
            rest = &rest[unit_len..];
        }
        total
    };
    (1..=MAX_SECS).contains(&secs).then_some(secs)
}

pub struct Timer {
    clock: Clock,
    epoch: Instant,
    /// Unspent wheel movement, in notches.
    wheel: f32,
    /// Text typed into the time after a double-click, while editing.
    editing: Option<String>,
}

impl Default for Timer {
    fn default() -> Self {
        Self {
            clock: Clock::default(),
            epoch: Instant::now(),
            wheel: 0.0,
            editing: None,
        }
    }
}

impl Timer {
    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    fn label(&self, now: f64) -> String {
        format_secs(
            self.clock.display_secs(now),
            self.clock.mode == Mode::Countdown,
        )
    }

    /// Whole wheel notches over the tile this frame (touchpads accumulate).
    fn wheel_steps(&mut self, ui: &Ui) -> i32 {
        let delta: f32 = ui.input(|i| {
            i.events
                .iter()
                .map(|e| match e {
                    Event::MouseWheel { unit, delta, .. } => match unit {
                        MouseWheelUnit::Line => delta.y,
                        MouseWheelUnit::Point => delta.y / 40.0,
                        MouseWheelUnit::Page => delta.y * 3.0,
                    },
                    _ => 0.0,
                })
                .sum()
        });
        self.wheel += delta;
        let steps = self.wheel.trunc();
        self.wheel -= steps;
        steps as i32
    }
}

/// A round icon button centered at `center`.
fn button_at(
    ui: &mut Ui,
    center: egui::Pos2,
    size: f32,
    glyph: &'static str,
    active: bool,
    theme: &Theme,
) -> Response {
    let rect = Rect::from_center_size(center, vec2(size, size));
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(glyph));
    widgets::icon_button(&mut child, glyph, size, active, theme)
}

impl Module for Timer {
    fn id(&self) -> &'static str {
        "timer"
    }

    fn captures_wheel(&self) -> bool {
        self.clock.mode == Mode::Countdown && self.clock.phase != Phase::Finished
    }

    fn name(&self) -> &str {
        match self.clock.mode {
            Mode::Countdown => "Timer",
            Mode::Stopwatch => "Stopwatch",
        }
    }

    fn icon(&self) -> &'static str {
        icons::TIMER
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Large
    }

    fn is_active(&self) -> bool {
        self.clock.phase != Phase::Idle
    }

    fn collapsed_badge(&self) -> Option<Badge> {
        let now = self.now();
        match self.clock.phase {
            Phase::Idle => None,
            Phase::Finished => Some(Badge::Glyph {
                icon: icons::BELL_RINGING,
            }),
            Phase::Running { .. } | Phase::Paused { .. } => Some(Badge::Ring {
                progress: self.clock.progress(now),
                label: self.label(now),
            }),
        }
    }

    fn on_tick(&mut self) {
        let now = self.now();
        if self.clock.poll(now) {
            tracing::info!("timer finished");
            system::alert_sound();
        }
    }

    fn next_repaint(&self, tile_visible: bool) -> Option<Duration> {
        let now = self.now();
        if let Some(wait) = self.clock.until_next_second(now) {
            // The pill shows the time too, so tick even when collapsed.
            return Some(Duration::from_secs_f64(wait + 0.005));
        }
        // The finished ring pulses while the panel is open.
        (tile_visible && self.clock.phase == Phase::Finished).then(|| Duration::from_millis(33))
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        let now = self.now();
        let rect = ui.max_rect();
        let active = self.is_active();
        let glyph = match self.clock.mode {
            Mode::Countdown => icons::HOURGLASS,
            Mode::Stopwatch => icons::TIMER,
        };
        tile_header(ui, glyph, self.name(), active, theme);

        // Mode switch, top right: shows the other mode's glyph.
        let other = match self.clock.mode {
            Mode::Countdown => (icons::TIMER, Mode::Stopwatch),
            Mode::Stopwatch => (icons::HOURGLASS, Mode::Countdown),
        };
        if !active
            && button_at(
                ui,
                pos2(rect.max.x - 8.0, rect.min.y + 9.0),
                20.0,
                other.0,
                false,
                theme,
            )
            .on_hover_text(match other.1 {
                Mode::Countdown => "Countdown",
                Mode::Stopwatch => "Stopwatch",
            })
            .clicked()
        {
            self.clock.set_mode(other.1);
        }

        // Ring with the time inside.
        let center = pos2(rect.center().x, rect.min.y + 50.0);
        let radius = 25.0;
        let finished = self.clock.phase == Phase::Finished;
        let pulse = if finished {
            ((ctx.now * 3.0).sin() * 0.5 + 0.5) as f32
        } else {
            1.0
        };
        let painter = ui.painter();
        paint::ring(
            painter,
            center,
            radius,
            3.0,
            if finished {
                1.0
            } else {
                self.clock.progress(now)
            },
            with_alpha(theme.text_faint, 0.35),
            with_alpha(theme.accent_fg(), 0.4 + 0.6 * pulse),
        );
        let (text, color) = if finished {
            ("Done".to_owned(), theme.accent_fg())
        } else {
            let dim = matches!(self.clock.phase, Phase::Paused { .. });
            (
                self.label(now),
                if dim { theme.text_dim } else { theme.text },
            )
        };

        // Double-click the time to type one (a countdown that is not running).
        let editable = self.clock.mode == Mode::Countdown && !self.clock.is_running();
        if !editable {
            self.editing = None;
        }
        let edit_id = Id::new("nook-timer-edit");
        let mut just_opened = false;
        if editable && self.editing.is_none() {
            let time_rect = Rect::from_center_size(center, vec2(radius * 2.0, radius * 2.0));
            if ui
                .interact(time_rect, Id::new("nook-timer-time"), Sense::click())
                .on_hover_text("Double-click to type a time: 5, 5:30, 90s or 1h30m")
                .double_clicked()
            {
                self.editing = Some(format_secs(self.clock.duration(), true));
                ui.memory_mut(|m| m.request_focus(edit_id));
                just_opened = true;
            }
        }
        if let Some(input) = &mut self.editing {
            let field = Rect::from_center_size(center, vec2(44.0, 22.0));
            let mut child = ui.new_child(UiBuilder::new().max_rect(field));
            let mut out = TextEdit::singleline(input)
                .id(edit_id)
                .font(Theme::font_semibold(14.0))
                .text_color(theme.text)
                .horizontal_align(egui::Align::Center)
                .desired_width(field.width())
                .char_limit(10)
                .show(&mut child);
            if just_opened {
                // Select it all so typing replaces the old time.
                let all = egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(input.chars().count()),
                );
                out.state.cursor.set_char_range(Some(all));
                out.state.store(ui.ctx(), edit_id);
            }
            if out.response.lost_focus() {
                // Enter or a click elsewhere applies; Esc (or nonsense) keeps
                // the old time.
                let cancelled = ui.input(|i| i.key_pressed(Key::Escape));
                if !cancelled && let Some(secs) = parse_duration(input) {
                    self.clock.set_secs(secs);
                }
                self.editing = None;
            }
        } else {
            let size = if text.len() > 5 { 12.0 } else { 15.0 };
            ui.painter().text(
                center,
                Align2::CENTER_CENTER,
                text,
                Theme::font_semibold(size),
                color,
            );
        }

        // Scroll over the tile to set the countdown.
        if ctx.hovered && self.clock.mode == Mode::Countdown && !finished && self.editing.is_none() {
            let steps = self.wheel_steps(ui);
            if steps != 0 {
                self.clock.adjust(steps);
            }
        } else {
            self.wheel = 0.0;
        }

        // Controls: [minus | reset]  [play/pause | dismiss]  [plus | snooze].
        let y = rect.max.y - 11.0;
        let cx = rect.center().x;
        let gap = 34.0;
        let idle = self.clock.phase == Phase::Idle;
        let countdown = self.clock.mode == Mode::Countdown;

        let (left, left_tip) = if idle && countdown {
            (Some(icons::MINUS), "1 minute less")
        } else if idle {
            (None, "")
        } else {
            (Some(icons::ARROW_COUNTER_CLOCKWISE), "Reset")
        };
        if let Some(g) = left
            && button_at(ui, pos2(cx - gap, y), 24.0, g, false, theme)
                .on_hover_text(left_tip)
                .clicked()
        {
            if idle {
                self.clock.adjust(-1);
            } else {
                self.clock.reset();
            }
        }

        let (mid, mid_tip) = match self.clock.phase {
            Phase::Running { .. } => (icons::PAUSE, "Pause"),
            Phase::Finished => (icons::CHECK, "Dismiss"),
            Phase::Paused { .. } => (icons::PLAY, "Resume"),
            Phase::Idle => (icons::PLAY, "Start"),
        };
        if button_at(ui, pos2(cx, y), 28.0, mid, true, theme)
            .on_hover_text(mid_tip)
            .clicked()
        {
            if finished {
                self.clock.reset();
            } else {
                self.clock.toggle(now);
            }
        }

        if countdown {
            let tip = if finished {
                "One more minute"
            } else {
                "1 minute more"
            };
            if button_at(ui, pos2(cx + gap, y), 24.0, icons::PLUS, false, theme)
                .on_hover_text(tip)
                .clicked()
            {
                if finished {
                    self.clock.snooze(now);
                } else {
                    self.clock.adjust(1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countdown_runs_pauses_and_finishes() {
        let mut c = Clock {
            secs: 60,
            ..Clock::default()
        };
        assert_eq!(c.display_secs(0.0), 60.0);
        c.toggle(10.0);
        assert_eq!(c.display_secs(25.0), 45.0);
        c.toggle(25.0); // pause
        assert_eq!(c.display_secs(100.0), 45.0);
        c.toggle(100.0); // resume
        assert!(!c.poll(140.0));
        assert!(c.poll(145.0));
        assert_eq!(c.phase, Phase::Finished);
        assert!(!c.poll(150.0), "finishes only once");
        assert_eq!(c.display_secs(150.0), 0.0);
    }

    #[test]
    fn stopwatch_counts_up_and_never_finishes() {
        let mut c = Clock::default();
        c.set_mode(Mode::Stopwatch);
        c.toggle(0.0);
        assert!(!c.poll(10_000.0));
        assert_eq!(c.display_secs(90.0), 90.0);
        assert!((c.progress(90.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn adjust_clamps_and_mode_switch_resets() {
        let mut c = Clock::default();
        c.adjust(-1000);
        assert_eq!(c.secs, 60);
        c.adjust(1000);
        assert_eq!(c.secs, MAX_SECS);
        c.toggle(0.0);
        c.set_mode(Mode::Stopwatch);
        assert_eq!(c.phase, Phase::Idle);
    }

    #[test]
    fn snooze_restarts_for_a_minute() {
        let mut c = Clock::default();
        c.toggle(0.0);
        assert!(c.poll(c.duration()));
        c.snooze(2000.0);
        assert_eq!(c.display_secs(2030.0), 30.0);
    }

    #[test]
    fn next_tick_lands_on_the_second() {
        let mut c = Clock::default();
        c.toggle(0.0);
        let wait = c.until_next_second(3.25).unwrap_or_default();
        assert!((wait - 0.75).abs() < 1e-9);
        assert_eq!(Clock::default().until_next_second(1.0), None);
    }

    #[test]
    fn typed_durations() {
        assert_eq!(parse_duration("25"), Some(25 * 60));
        assert_eq!(parse_duration(" 5:30 "), Some(330));
        assert_eq!(parse_duration("0:45"), Some(45));
        assert_eq!(parse_duration("1:05:00"), Some(3900));
        assert_eq!(parse_duration("1.5"), Some(90));
        assert_eq!(parse_duration("90s"), Some(90));
        assert_eq!(parse_duration("45m"), Some(45 * 60));
        assert_eq!(parse_duration("1h30m"), Some(5400));
        assert_eq!(parse_duration("1h 30m 15s"), Some(5415));
        assert_eq!(parse_duration("2 min"), Some(120));
        // What the field shows when it opens parses back unchanged.
        assert_eq!(parse_duration(&format_secs(1500.0, true)), Some(1500));
        for bad in ["", "abc", "5:75", "0", "0:00", "4h", "-3", "1x", "1:2:3:4", "::"] {
            assert_eq!(parse_duration(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn typed_time_resets_a_paused_countdown() {
        let mut c = Clock::default();
        c.toggle(0.0);
        c.toggle(10.0);
        c.set_secs(330);
        assert_eq!(c.phase, Phase::Idle);
        assert_eq!(c.display_secs(50.0), 330.0);
        // +/- still step whole minutes from a typed time.
        c.adjust(1);
        assert_eq!(c.secs, 390);
    }

    #[test]
    fn formatting() {
        assert_eq!(format_secs(59.2, true), "1:00");
        assert_eq!(format_secs(59.2, false), "0:59");
        assert_eq!(format_secs(25.0 * 60.0, true), "25:00");
        assert_eq!(format_secs(3_725.0, false), "1:02:05");
    }
}
