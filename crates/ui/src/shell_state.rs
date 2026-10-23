//! Open/close logic for the shell: hover debounce, leave grace, pinning and
//! the "content fades out before the shell shrinks" sequencing.
//!
//! Time is passed in explicitly (seconds), so the logic is deterministic and
//! testable without a window.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShellTimings {
    /// Hover must last this long before the shell expands.
    pub hover_delay: f64,
    /// After the pointer leaves, wait this long before collapsing.
    pub collapse_grace: f64,
    /// On collapse, content fades for this long before the shell shrinks.
    pub content_fade_out: f64,
}

impl Default for ShellTimings {
    fn default() -> Self {
        Self {
            hover_delay: 0.120,
            collapse_grace: 0.250,
            content_fade_out: 0.100,
        }
    }
}

/// What the renderer should animate towards this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShellTargets {
    /// Shell size target: `true` = expanded panel, `false` = pill.
    pub shell_open: bool,
    /// Content visibility target.
    pub content_open: bool,
    /// Absolute time at which the state machine needs another look, if any.
    pub wake_at: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct ShellState {
    open: bool,
    pinned: bool,
    hover_since: Option<f64>,
    left_since: Option<f64>,
    closed_at: Option<f64>,
}

impl ShellState {
    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn is_pinned(&self) -> bool {
        self.pinned
    }

    /// Pointer is resting on the closed pill (during the hover debounce).
    pub fn is_hovering_closed(&self) -> bool {
        !self.open && self.hover_since.is_some()
    }

    /// Pin toggles; pinning also opens.
    pub fn toggle_pin(&mut self, now: f64) {
        self.set_pinned(!self.pinned, now);
    }

    pub fn set_pinned(&mut self, pinned: bool, now: f64) {
        self.pinned = pinned;
        if pinned {
            self.open_now();
        } else {
            // Unpinning while the pointer is elsewhere starts the grace period.
            self.left_since = Some(now);
        }
    }

    /// Explicit open/close (hotkey, tray, Escape).
    pub fn set_open(&mut self, open: bool, now: f64) {
        if open {
            self.open_now();
        } else {
            self.pinned = false;
            self.close_now(now);
        }
    }

    fn open_now(&mut self) {
        self.open = true;
        self.closed_at = None;
        self.left_since = None;
    }

    fn close_now(&mut self, now: f64) {
        if self.open {
            self.open = false;
            self.closed_at = Some(now);
        }
        self.hover_since = None;
        self.left_since = None;
    }

    /// Advances the state machine. `hovered` is whether the pointer is over
    /// the visible shell this frame.
    pub fn update(&mut self, now: f64, hovered: bool, t: &ShellTimings) -> ShellTargets {
        let mut wake_at = None;
        if hovered {
            self.left_since = None;
            if !self.open {
                let since = *self.hover_since.get_or_insert(now);
                if now - since >= t.hover_delay {
                    self.open_now();
                } else {
                    wake_at = Some(since + t.hover_delay);
                }
            }
        } else {
            self.hover_since = None;
            if self.open && !self.pinned {
                let since = *self.left_since.get_or_insert(now);
                if now - since >= t.collapse_grace {
                    self.close_now(now);
                } else {
                    wake_at = Some(since + t.collapse_grace);
                }
            }
        }

        let shell_open = if self.open {
            true
        } else if let Some(closed) = self.closed_at {
            if now - closed < t.content_fade_out {
                wake_at = Some(closed + t.content_fade_out);
                true
            } else {
                self.closed_at = None;
                false
            }
        } else {
            false
        };

        ShellTargets {
            shell_open,
            content_open: self.open,
            wake_at,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: ShellTimings = ShellTimings {
        hover_delay: 0.12,
        collapse_grace: 0.25,
        content_fade_out: 0.10,
    };

    #[test]
    fn brief_hover_does_not_open() {
        let mut s = ShellState::default();
        let out = s.update(0.0, true, &T);
        assert!(!out.shell_open);
        assert_eq!(out.wake_at, Some(0.12));
        let out = s.update(0.05, false, &T);
        assert!(!out.shell_open);
        let out = s.update(0.2, true, &T);
        assert!(!out.shell_open, "debounce restarts after leaving");
    }

    #[test]
    fn hover_past_delay_opens() {
        let mut s = ShellState::default();
        s.update(0.0, true, &T);
        let out = s.update(0.13, true, &T);
        assert!(out.shell_open && out.content_open);
    }

    #[test]
    fn leaving_waits_for_grace_then_fades_content_first() {
        let mut s = ShellState::default();
        s.update(0.0, true, &T);
        s.update(0.2, true, &T);
        let out = s.update(1.0, false, &T);
        assert!(out.shell_open && out.content_open);
        assert_eq!(out.wake_at, Some(1.25));
        // Back within the grace period: stays open.
        let out = s.update(1.1, true, &T);
        assert!(out.content_open);
        s.update(2.0, false, &T);
        let out = s.update(2.3, false, &T);
        assert!(!out.content_open, "content fades out first");
        assert!(out.shell_open, "shell waits for the content fade");
        let out = s.update(2.45, false, &T);
        assert!(!out.shell_open);
        assert_eq!(out.wake_at, None, "fully idle once collapsed");
    }

    #[test]
    fn pinned_stays_open_without_hover() {
        let mut s = ShellState::default();
        s.toggle_pin(0.0);
        assert!(s.is_open());
        let out = s.update(5.0, false, &T);
        assert!(out.shell_open && out.content_open);
        assert_eq!(out.wake_at, None);
        s.toggle_pin(6.0);
        let out = s.update(6.3, false, &T);
        assert!(!out.content_open);
    }

    #[test]
    fn explicit_close_unpins() {
        let mut s = ShellState::default();
        s.set_pinned(true, 0.0);
        s.set_open(false, 1.0);
        assert!(!s.is_pinned());
        let out = s.update(1.2, false, &T);
        assert!(!out.shell_open);
    }

    #[test]
    fn rehover_during_content_fade_reopens() {
        let mut s = ShellState::default();
        s.set_open(true, 0.0);
        s.update(1.0, false, &T);
        let out = s.update(1.3, false, &T);
        assert!(!out.content_open);
        // Hover again: debounced like a fresh hover.
        s.update(1.32, true, &T);
        let out = s.update(1.45, true, &T);
        assert!(out.shell_open && out.content_open);
    }
}
