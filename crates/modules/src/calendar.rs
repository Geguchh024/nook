//! Calendar: today's date on the tile, a month view with week numbers, and
//! simple events with reminders.
//!
//! Type `14:30 Dentist` (or `2pm Call Sam`, or just `Birthday`) under a
//! day to add an event. Timed events remind you: the notch chimes and opens
//! on the day, and the pill pulses until you look. The week starts on the
//! day your Windows region settings say. Events are saved in
//! `%APPDATA%\Nook\calendar.txt`, one per line.

use egui::{Align2, Color32, Id, Rect, Sense, Ui, pos2, vec2};
use nook_platform::system;
use nook_ui::module::{Badge, Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme, mix, with_alpha};
use nook_ui::{anim, icons, paint, widgets};
use std::time::Duration;

use crate::kit;

const PAGE: egui::Vec2 = vec2(700.0, 430.0);
const FIELD_ID: &str = "nook-calendar-add";

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
/// Monday first.
const WEEKDAYS: [&str; 7] = [
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
    "Sunday",
];

// ------------------------------------------------------------- dates

/// Days since 1970-01-01 (proleptic Gregorian).
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((y + i64::from(m <= 2)) as i32, m, d)
}

/// 0 = Monday ... 6 = Sunday.
pub fn weekday(days: i64) -> u32 {
    (days + 3).rem_euclid(7) as u32
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    let next = if m == 12 {
        days_from_civil(y + 1, 1, 1)
    } else {
        days_from_civil(y, m + 1, 1)
    };
    (next - days_from_civil(y, m, 1)) as u32
}

/// ISO 8601 week number.
pub fn iso_week(days: i64) -> u32 {
    let thursday = days - i64::from(weekday(days)) + 3;
    let (y, _, _) = civil_from_days(thursday);
    ((thursday - days_from_civil(y, 1, 1)) / 7 + 1) as u32
}

fn today() -> (i64, u32) {
    let t = system::local_time();
    (
        days_from_civil(t.year, t.month, t.day),
        t.hour * 3600 + t.minute * 60 + t.second,
    )
}

/// "Today", "Tomorrow", "In 12 days", "3 days ago".
fn relative(day: i64, today: i64) -> String {
    match day - today {
        0 => "Today".into(),
        1 => "Tomorrow".into(),
        -1 => "Yesterday".into(),
        n if n > 0 => format!("In {n} days"),
        n => format!("{} days ago", -n),
    }
}

/// `14:30`, `9`, `2pm`, `9:05am` as minutes after midnight.
fn parse_time(tok: &str) -> Option<u32> {
    let t = tok.trim().to_lowercase();
    let (t, pm) = if let Some(s) = t.strip_suffix("pm") {
        (s.to_owned(), Some(true))
    } else if let Some(s) = t.strip_suffix("am") {
        (s.to_owned(), Some(false))
    } else {
        (t, None)
    };
    let (h, m) = match t.split_once([':', '.']) {
        Some((h, m)) if m.len() == 2 => (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?),
        Some(_) => return None,
        None if pm.is_some() || t.len() <= 2 => (t.parse::<u32>().ok()?, 0),
        None => return None,
    };
    let h = match pm {
        Some(true) if (1..=11).contains(&h) => h + 12,
        Some(false) if h == 12 => 0,
        Some(_) if !(1..=12).contains(&h) => return None,
        _ => h,
    };
    // A bare number is only a time with am/pm (so "3 pizzas" is a title).
    if pm.is_none() && !t.contains([':', '.']) {
        return None;
    }
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Splits `14:30 Dentist` into the time and the title.
fn parse_event(text: &str) -> Option<(Option<u32>, String)> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    if let Some((first, rest)) = text.split_once(char::is_whitespace)
        && let Some(t) = parse_time(first)
        && !rest.trim().is_empty()
    {
        return Some((Some(t), rest.trim().to_owned()));
    }
    Some((None, text.to_owned()))
}

fn hhmm(minutes: u32) -> String {
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

// ------------------------------------------------------------- events

#[derive(Clone, Debug, PartialEq)]
struct Event {
    day: i64,
    /// Minutes after midnight.
    time: Option<u32>,
    title: String,
    /// The reminder went off (or the event was already past at start).
    notified: bool,
}

fn events_file() -> Option<std::path::PathBuf> {
    kit::settings_path("calendar.txt")
}

fn parse_events(text: &str) -> Vec<Event> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let date = parts.next()?;
            let time = parts.next()?;
            let title = parts.next()?.trim().to_owned();
            let mut d = date.split('-');
            let y = d.next()?.parse().ok()?;
            let m = d.next()?.parse().ok()?;
            let dd = d.next()?.parse().ok()?;
            Some(Event {
                day: days_from_civil(y, m, dd),
                time: parse_time(time),
                title,
                notified: false,
            })
        })
        .collect()
}

fn format_events(events: &[Event]) -> String {
    events
        .iter()
        .map(|e| {
            let (y, m, d) = civil_from_days(e.day);
            format!(
                "{y:04}-{m:02}-{d:02}\t{}\t{}\n",
                e.time.map_or("-".into(), hhmm),
                e.title.replace(['\t', '\n'], " ")
            )
        })
        .collect()
}

pub struct Calendar {
    events: Vec<Event>,
    /// First day of the week, 0 = Monday.
    week_start: u32,
    /// Month shown: (year, month).
    month: (i32, u32),
    selected: i64,
    draft: String,
    open_page: bool,
    show: bool,
    /// A reminder went off and the page has not been seen since.
    alert: Option<String>,
    /// egui time and direction of the last month change (slide in).
    flipped: (f64, f32),
    /// Wheel travel not yet turned into a month flip.
    wheel: f32,
}

impl Default for Calendar {
    fn default() -> Self {
        let (d, _) = today();
        let (y, m, _) = civil_from_days(d);
        Self {
            events: Vec::new(),
            week_start: 0,
            month: (y, m),
            selected: d,
            draft: String::new(),
            open_page: false,
            show: false,
            alert: None,
            flipped: (f64::NEG_INFINITY, 0.0),
            wheel: 0.0,
        }
    }
}

impl Calendar {
    fn save(&self) {
        let Some(path) = events_file() else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(e) = std::fs::write(&path, format_events(&self.events)) {
            tracing::warn!("calendar: saving {}: {e}", path.display());
        }
    }

    fn sort(&mut self) {
        self.events
            .sort_by_key(|e| (e.day, e.time.unwrap_or(0), e.title.clone()));
    }

    fn on_day(&self, day: i64) -> impl Iterator<Item = (usize, &Event)> {
        self.events
            .iter()
            .enumerate()
            .filter(move |(_, e)| e.day == day)
    }

    fn go_to(&mut self, day: i64, now: f64) {
        let (y, m, _) = civil_from_days(day);
        if (y, m) != self.month {
            let dir = if (y, m) > self.month { 1.0 } else { -1.0 };
            self.flipped = (now, dir);
            self.month = (y, m);
        }
        self.selected = day;
    }

    fn shift_month(&mut self, by: i32, now: f64) {
        let idx = self.month.0 * 12 + self.month.1 as i32 - 1 + by;
        self.month = (idx.div_euclid(12), idx.rem_euclid(12) as u32 + 1);
        self.flipped = (now, by.signum() as f32);
    }

    /// The next timed event that has not reminded yet: (index, seconds away).
    fn next_reminder(&self) -> Option<(usize, i64)> {
        let (day, secs) = today();
        let now = day * 86_400 + i64::from(secs);
        self.events
            .iter()
            .enumerate()
            .filter(|(_, e)| !e.notified)
            .filter_map(|(i, e)| {
                e.time
                    .map(|t| (i, e.day * 86_400 + i64::from(t) * 60 - now))
            })
            .min_by_key(|(_, s)| *s)
    }

    fn add_draft(&mut self) {
        if let Some((time, title)) = parse_event(&self.draft) {
            let (day, secs) = today();
            let past = self.selected < day
                || (self.selected == day && time.is_some_and(|t| t * 60 <= secs));
            self.events.push(Event {
                day: self.selected,
                time,
                title,
                notified: past,
            });
            self.sort();
            self.save();
            self.draft.clear();
        }
    }

    // ------------------------------------------------------- page parts

    fn month_card(&mut self, ui: &mut Ui, card: Rect, theme: &Theme) {
        widgets::card(ui.painter(), card, theme);
        let now = ui.input(|i| i.time);
        let (today, _) = today();
        let inner = card.shrink2(vec2(14.0, 12.0));
        let (y, m) = self.month;

        // Title and navigation.
        let title_y = inner.min.y + 12.0;
        ui.painter().text(
            pos2(inner.min.x + 2.0, title_y),
            Align2::LEFT_CENTER,
            MONTHS[m as usize - 1],
            Theme::font_semibold(SIZE_BODY + 5.0),
            theme.text,
        );
        let month_w = ui
            .painter()
            .layout_no_wrap(
                MONTHS[m as usize - 1].to_owned(),
                Theme::font_semibold(SIZE_BODY + 5.0),
                theme.text,
            )
            .size()
            .x;
        ui.painter().text(
            pos2(inner.min.x + month_w + 8.0, title_y + 1.0),
            Align2::LEFT_CENTER,
            y.to_string(),
            Theme::font(SIZE_BODY + 3.0),
            theme.text_faint,
        );
        if kit::icon_at(
            ui,
            pos2(inner.max.x - 12.0, title_y),
            26.0,
            icons::CARET_RIGHT,
            false,
            theme,
        )
        .on_hover_text("Next month")
        .clicked()
        {
            self.shift_month(1, now);
        }
        if kit::icon_at(
            ui,
            pos2(inner.max.x - 42.0, title_y),
            26.0,
            icons::CARET_LEFT,
            false,
            theme,
        )
        .on_hover_text("Previous month")
        .clicked()
        {
            self.shift_month(-1, now);
        }
        let (ty, tm, _) = civil_from_days(today);
        if (ty, tm) != self.month || self.selected != today {
            let r = Rect::from_center_size(pos2(inner.max.x - 92.0, title_y), vec2(52.0, 22.0));
            if kit::chip(ui, r, "Today", false, theme).clicked() {
                self.go_to(today, now);
            }
        }

        // Weekday header and the grid.
        let week_col = 26.0;
        let grid = Rect::from_min_max(pos2(inner.min.x + week_col, inner.min.y + 36.0), inner.max);
        let cw = grid.width() / 7.0;
        let head_h = 20.0;
        for i in 0..7u32 {
            let wd = (self.week_start + i) % 7;
            ui.painter().text(
                pos2(
                    grid.min.x + cw * (i as f32 + 0.5),
                    grid.min.y + head_h * 0.5,
                ),
                Align2::CENTER_CENTER,
                &WEEKDAYS[wd as usize][..2],
                Theme::font_semibold(SIZE_LABEL - 1.0),
                if wd >= 5 {
                    theme.text_faint
                } else {
                    theme.text_dim
                },
            );
        }
        let rows_area = Rect::from_min_max(pos2(grid.min.x, grid.min.y + head_h + 4.0), grid.max);
        let ch = rows_area.height() / 6.0;

        // Slide in after a month change.
        let t = ((now - self.flipped.0) as f32 / 0.28).clamp(0.0, 1.0);
        if t < 1.0 {
            ui.ctx().request_repaint();
        }
        let ease = anim::ease_out_cubic(t);
        let dx = self.flipped.1 * 24.0 * (1.0 - ease);
        let alpha = 0.25 + 0.75 * ease;

        let first = days_from_civil(y, m, 1);
        let lead = (weekday(first) + 7 - self.week_start) % 7;
        let start = first - i64::from(lead);
        // The wheel flips months, one per notch (trackpads add up).
        if ui.rect_contains_pointer(card) {
            self.wheel += kit::wheel_lines(ui.ctx());
            if self.wheel.abs() >= 1.0 && now - self.flipped.0 > 0.25 {
                self.shift_month(if self.wheel < 0.0 { 1 } else { -1 }, now);
                self.wheel = 0.0;
            }
        }
        let mut pick = None;
        for row in 0..6 {
            let row_day = start + row * 7;
            // Week number of the row's Thursday-anchored ISO week.
            ui.painter().text(
                pos2(
                    inner.min.x + week_col * 0.4,
                    rows_area.min.y + ch * (row as f32 + 0.5),
                ),
                Align2::CENTER_CENTER,
                iso_week(row_day + i64::from((7 - self.week_start) % 7).min(6)).to_string(),
                Theme::font(SIZE_LABEL - 2.0),
                with_alpha(theme.text_faint, 0.8 * alpha),
            );
            for col in 0..7 {
                let day = row_day + col;
                let (_, dm, dd) = civil_from_days(day);
                let center = pos2(
                    rows_area.min.x + cw * (col as f32 + 0.5) + dx,
                    rows_area.min.y + ch * (row as f32 + 0.5),
                );
                let cell = Rect::from_center_size(center, vec2(cw - 4.0, ch - 2.0));
                let id = Id::new(("cal-day", day));
                let resp = ui.interact(cell, id, Sense::click());
                let hover = anim::tween(ui.ctx(), id.with("h"), resp.hovered(), 0.1);
                let in_month = dm == m;
                let is_today = day == today;
                let selected = day == self.selected;
                let r = (ch * 0.5 - 3.0).min(cw * 0.5 - 2.0).min(16.0);
                let painter = ui.painter();
                if is_today {
                    painter.circle_filled(center, r, with_alpha(theme.accent, alpha));
                } else if hover > 0.0 {
                    painter.circle_filled(center, r, with_alpha(kit::wash(theme), 2.0 * hover));
                }
                if selected && !is_today {
                    painter.circle_stroke(
                        center,
                        r,
                        egui::Stroke::new(1.5, with_alpha(theme.accent, alpha)),
                    );
                }
                let weekend = weekday(day) >= 5;
                let color = if is_today {
                    Color32::WHITE
                } else if !in_month {
                    with_alpha(theme.text_faint, 0.55)
                } else if weekend {
                    theme.text_dim
                } else {
                    theme.text
                };
                painter.text(
                    center - vec2(0.0, 1.0),
                    Align2::CENTER_CENTER,
                    dd.to_string(),
                    if is_today || selected {
                        Theme::font_semibold(SIZE_BODY)
                    } else {
                        Theme::font(SIZE_BODY)
                    },
                    with_alpha(color, alpha),
                );
                let n = self.on_day(day).count().min(3);
                for k in 0..n {
                    let x = center.x + (k as f32 - (n as f32 - 1.0) * 0.5) * 5.0;
                    let dot = if is_today {
                        Color32::WHITE
                    } else {
                        theme.accent
                    };
                    painter.circle_filled(pos2(x, center.y + r - 4.5), 1.6, with_alpha(dot, alpha));
                }
                if resp.clicked() {
                    pick = Some(day);
                }
            }
        }
        if let Some(d) = pick {
            self.go_to(d, now);
        }
    }

    fn day_column(&mut self, ui: &mut Ui, col: Rect, theme: &Theme) {
        let (today, now_secs) = today();
        let day = self.selected;
        let (y, m, d) = civil_from_days(day);
        let mut cursor = col.min.y;

        if let Some(alert) = &self.alert {
            let banner = Rect::from_min_size(pos2(col.min.x, cursor), vec2(col.width(), 34.0));
            ui.painter()
                .rect_filled(banner, 12.0, with_alpha(theme.accent, 0.22));
            paint::icon(
                ui.painter(),
                pos2(banner.min.x + 16.0, banner.center().y),
                icons::BELL_RINGING,
                15.0,
                theme.accent_fg(),
                true,
            );
            kit::one_line(
                ui,
                pos2(banner.min.x + 32.0, banner.center().y - 8.0),
                &format!("Now: {alert}"),
                banner.width() - 64.0,
                Theme::font_semibold(SIZE_LABEL + 1.0),
                theme.text,
            );
            if kit::icon_at(
                ui,
                pos2(banner.max.x - 16.0, banner.center().y),
                22.0,
                icons::X,
                false,
                theme,
            )
            .clicked()
            {
                self.alert = None;
            }
            cursor = banner.max.y + 10.0;
        }

        ui.painter().text(
            pos2(col.min.x + 2.0, cursor),
            Align2::LEFT_TOP,
            WEEKDAYS[weekday(day) as usize].to_uppercase(),
            Theme::font_semibold(SIZE_LABEL - 1.0),
            theme.accent_fg(),
        );
        ui.painter().text(
            pos2(col.min.x + 1.0, cursor + 14.0),
            Align2::LEFT_TOP,
            format!("{d} {}", MONTHS[m as usize - 1]),
            Theme::font_semibold(26.0),
            theme.text,
        );
        let year = if y != civil_from_days(today).0 {
            format!("  \u{00B7}  {y}")
        } else {
            String::new()
        };
        ui.painter().text(
            pos2(col.min.x + 2.0, cursor + 50.0),
            Align2::LEFT_TOP,
            format!(
                "{}  \u{00B7}  Week {}{year}",
                relative(day, today),
                iso_week(day)
            ),
            Theme::font(SIZE_LABEL),
            theme.text_dim,
        );
        cursor += 76.0;

        // The day's events.
        let field_h = 34.0;
        let list = Rect::from_min_max(
            pos2(col.min.x, cursor),
            pos2(col.max.x, col.max.y - field_h - 10.0),
        );
        let todays: Vec<(usize, Event)> = self.on_day(day).map(|(i, e)| (i, e.clone())).collect();
        let row_h = 36.0;
        let mut remove = None;
        if todays.is_empty() {
            ui.painter().text(
                pos2(list.min.x + 2.0, list.min.y + 6.0),
                Align2::LEFT_TOP,
                "Nothing planned",
                Theme::font(SIZE_LABEL + 1.0),
                theme.text_faint,
            );
            // Fill the space with what is coming up.
            let upcoming: Vec<Event> = self
                .events
                .iter()
                .filter(|e| e.day > day.max(today - 1))
                .take(((list.height() - 50.0) / 22.0).max(0.0) as usize)
                .cloned()
                .collect();
            if !upcoming.is_empty() {
                kit::section(
                    ui,
                    pos2(list.min.x + 2.0, list.min.y + 34.0),
                    "COMING UP",
                    theme,
                );
                for (k, e) in upcoming.iter().enumerate() {
                    let (_, em, ed) = civil_from_days(e.day);
                    let yy = list.min.y + 54.0 + k as f32 * 22.0;
                    let when = format!(
                        "{} {ed} {}",
                        &WEEKDAYS[weekday(e.day) as usize][..3],
                        &MONTHS[em as usize - 1][..3]
                    );
                    ui.painter().text(
                        pos2(list.min.x + 2.0, yy),
                        Align2::LEFT_TOP,
                        when,
                        Theme::font_semibold(SIZE_LABEL),
                        theme.text_dim,
                    );
                    kit::one_line(
                        ui,
                        pos2(list.min.x + 84.0, yy),
                        &match e.time {
                            Some(t) => format!("{}  {}", hhmm(t), e.title),
                            None => e.title.clone(),
                        },
                        list.width() - 86.0,
                        Theme::font(SIZE_LABEL),
                        theme.text,
                    );
                }
            }
        }
        for (k, (i, e)) in todays.iter().enumerate() {
            let r = Rect::from_min_size(
                pos2(list.min.x, list.min.y + k as f32 * row_h),
                vec2(list.width(), row_h - 4.0),
            );
            if r.max.y > list.max.y {
                break;
            }
            let resp = kit::row(ui, r, Id::new(("cal-event", *i)), false, theme);
            let past = day < today || (day == today && e.time.is_some_and(|t| t * 60 < now_secs));
            let pill =
                Rect::from_min_size(pos2(r.min.x + 6.0, r.center().y - 10.0), vec2(50.0, 20.0));
            let (pill_fill, pill_fg) = if e.time.is_some() {
                (
                    with_alpha(theme.accent, if past { 0.10 } else { 0.24 }),
                    theme.accent_fg(),
                )
            } else {
                (kit::wash(theme), theme.text_dim)
            };
            ui.painter().rect_filled(pill, 10.0, pill_fill);
            ui.painter().text(
                pill.center(),
                Align2::CENTER_CENTER,
                e.time.map_or("All day".into(), hhmm),
                Theme::font_semibold(SIZE_LABEL - 1.0),
                pill_fg,
            );
            kit::one_line(
                ui,
                pos2(pill.max.x + 10.0, r.center().y - 8.0),
                &e.title,
                r.width() - 100.0,
                Theme::font(SIZE_BODY),
                if past { theme.text_dim } else { theme.text },
            );
            if (resp.hovered() || resp.contains_pointer())
                && kit::icon_at(
                    ui,
                    pos2(r.max.x - 14.0, r.center().y),
                    22.0,
                    icons::X,
                    false,
                    theme,
                )
                .on_hover_text("Delete")
                .clicked()
            {
                remove = Some(*i);
            }
        }
        if let Some(i) = remove {
            self.events.remove(i);
            self.save();
        }

        // Adding.
        let field = Rect::from_min_max(pos2(col.min.x, col.max.y - field_h), col.max);
        let id = Id::new(FIELD_ID);
        let resp = kit::field(
            ui,
            Rect::from_min_max(field.min, pos2(field.max.x - 40.0, field.max.y)),
            &mut self.draft,
            "Add: 14:30 Dentist",
            id,
            Theme::font(SIZE_BODY),
            theme,
        );
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let add = Rect::from_min_max(pos2(field.max.x - 34.0, field.min.y), field.max);
        let clicked = ui.add_enabled_ui(!self.draft.trim().is_empty(), |ui| {
            kit::button(ui, add, "", Some(icons::PLUS), true, theme).clicked()
        });
        if enter || clicked.inner {
            self.add_draft();
            if enter {
                resp.request_focus();
            }
        }
    }
}

impl Module for Calendar {
    fn id(&self) -> &'static str {
        "calendar"
    }

    fn name(&self) -> &str {
        "Calendar"
    }

    fn icon(&self) -> &'static str {
        icons::CALENDAR_BLANK
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn attach(&mut self, _ctx: &egui::Context) {
        self.week_start = system::first_day_of_week();
        if let Some(text) = events_file().and_then(|p| std::fs::read_to_string(p).ok()) {
            self.events = parse_events(&text);
            self.sort();
        }
        // Reminders that went by while Nook was not running stay quiet.
        let (day, secs) = today();
        for e in &mut self.events {
            e.notified = e.day < day || (e.day == day && e.time.is_none_or(|t| t * 60 <= secs));
        }
    }

    fn is_active(&self) -> bool {
        self.alert.is_some()
    }

    fn collapsed_badge(&self) -> Option<Badge> {
        self.alert.as_ref().map(|_| Badge::Pulse)
    }

    fn on_tick(&mut self) {
        if let Some((i, secs)) = self.next_reminder()
            && secs <= 0
        {
            let e = &mut self.events[i];
            e.notified = true;
            // Only ring for reminders that are due now, not long gone.
            if secs > -600 {
                self.alert = Some(match e.time {
                    Some(t) => format!("{} {}", hhmm(t), e.title),
                    None => e.title.clone(),
                });
                let day = e.day;
                self.go_to(day, 0.0);
                self.show = true;
                system::alert_sound();
            }
        }
    }

    fn next_repaint(&self, _tile_visible: bool) -> Option<Duration> {
        let (_, secs) = self.next_reminder()?;
        // Re-check at least hourly: sleep and clock changes move "now".
        Some(Duration::from_secs(secs.clamp(0, 3600) as u64) + Duration::from_millis(50))
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.open_page)
    }

    fn take_show_request(&mut self) -> bool {
        std::mem::take(&mut self.show)
    }

    fn page_size(&self) -> Option<egui::Vec2> {
        Some(PAGE)
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        let rect = ui.max_rect();
        let resp = ui.interact(rect.expand(10.0), ui.id().with("cal-tile"), Sense::click());
        let (day, secs) = today();
        let (_, m, d) = civil_from_days(day);
        let wd = &WEEKDAYS[weekday(day) as usize][..3];
        let painter = ui.painter();
        let accent = if self.alert.is_some() || ctx.hovered {
            theme.accent_fg()
        } else {
            mix(theme.accent_fg(), theme.text_dim, 0.25)
        };
        let next_today = self
            .events
            .iter()
            .find(|e| e.day == day && e.time.is_none_or(|t| t * 60 >= secs));
        if rect.width() > rect.height() * 1.5 {
            // Wide: the date on the left, the next event on the right.
            let c = pos2(rect.min.x + 20.0, rect.center().y);
            painter.text(
                c - vec2(0.0, 10.0),
                Align2::CENTER_CENTER,
                wd.to_uppercase(),
                Theme::font_semibold(SIZE_LABEL - 2.0),
                accent,
            );
            painter.text(
                c + vec2(0.0, 6.0),
                Align2::CENTER_CENTER,
                d.to_string(),
                Theme::font_semibold(22.0),
                theme.text,
            );
            let x = rect.min.x + 46.0;
            let (top, bottom) = match next_today {
                Some(e) => (e.time.map_or("Today".into(), hhmm), e.title.clone()),
                None => (
                    MONTHS[m as usize - 1].to_owned(),
                    "No more events today".into(),
                ),
            };
            kit::one_line(
                ui,
                pos2(x, rect.center().y - 16.0),
                &top,
                rect.max.x - x,
                Theme::font_semibold(SIZE_LABEL),
                theme.text_dim,
            );
            kit::one_line(
                ui,
                pos2(x, rect.center().y),
                &bottom,
                rect.max.x - x,
                Theme::font(SIZE_LABEL + 1.0),
                theme.text,
            );
        } else {
            let c = rect.center();
            painter.text(
                c - vec2(0.0, 11.0),
                Align2::CENTER_CENTER,
                wd.to_uppercase(),
                Theme::font_semibold(SIZE_LABEL - 2.0),
                accent,
            );
            painter.text(
                c + vec2(0.0, 5.0),
                Align2::CENTER_CENTER,
                d.to_string(),
                Theme::font_semibold(24.0),
                if ctx.hovered {
                    theme.text
                } else {
                    mix(theme.text, theme.text_dim, 0.2)
                },
            );
            if next_today.is_some() {
                painter.circle_filled(c + vec2(0.0, 21.0), 2.0, theme.accent);
            }
        }
        let tip = match next_today {
            Some(e) => format!(
                "Next: {}{}",
                e.time.map_or(String::new(), |t| format!("{} ", hhmm(t))),
                e.title
            ),
            None => format!(
                "{} {d} {}",
                WEEKDAYS[weekday(day) as usize],
                MONTHS[m as usize - 1]
            ),
        };
        if resp.on_hover_text(tip).clicked() {
            let now = ui.input(|i| i.time);
            self.go_to(day, now);
            self.open_page = true;
        }
    }

    fn ui_page(&mut self, ui: &mut Ui, _header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        let card = Rect::from_min_size(body.min, vec2(372.0, body.height()));
        self.month_card(ui, card, theme);
        let col = Rect::from_min_max(pos2(card.max.x + 18.0, body.min.y + 4.0), body.max);
        self.day_column(ui, col, theme);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        for z in [-800_000, -1, 0, 59, 11_016, 20_746, 100_000] {
            let (y, m, d) = civil_from_days(z);
            assert_eq!(days_from_civil(y, m, d), z);
        }
        // 2026-10-20 is a Tuesday in ISO week 43.
        let d = days_from_civil(2026, 10, 20);
        assert_eq!(weekday(d), 1);
        assert_eq!(iso_week(d), 43);
        // 2021-01-03 (Sunday) belongs to 2020's week 53.
        assert_eq!(iso_week(days_from_civil(2021, 1, 3)), 53);
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2026, 12), 31);
    }

    #[test]
    fn times_and_events() {
        assert_eq!(parse_time("14:30"), Some(14 * 60 + 30));
        assert_eq!(parse_time("9.05"), Some(9 * 60 + 5));
        assert_eq!(parse_time("2pm"), Some(14 * 60));
        assert_eq!(parse_time("12am"), Some(0));
        assert_eq!(parse_time("9:05am"), Some(9 * 60 + 5));
        assert_eq!(parse_time("3"), None);
        assert_eq!(parse_time("25:00"), None);
        assert_eq!(
            parse_event("14:30 Dentist"),
            Some((Some(870), "Dentist".into()))
        );
        assert_eq!(parse_event("3 pizzas"), Some((None, "3 pizzas".into())));
        assert_eq!(parse_event("  "), None);
        assert_eq!(relative(10, 10), "Today");
        assert_eq!(relative(15, 10), "In 5 days");
        assert_eq!(relative(7, 10), "3 days ago");
    }

    #[test]
    fn events_file_round_trip() {
        let events = vec![
            Event {
                day: days_from_civil(2026, 10, 20),
                time: Some(870),
                title: "Dentist".into(),
                notified: false,
            },
            Event {
                day: days_from_civil(2026, 12, 24),
                time: None,
                title: "Eve".into(),
                notified: false,
            },
        ];
        let text = format_events(&events);
        assert!(text.starts_with("2026-10-20\t14:30\tDentist\n"));
        assert_eq!(parse_events(&text), events);
    }
}
