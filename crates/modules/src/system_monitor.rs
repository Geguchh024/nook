//! System monitor: CPU and memory rings on the tile; on the page, the last
//! minute of CPU, memory and network as live charts, free space on every
//! drive and the busiest apps.
//!
//! It samples once a second, and only while the tile or the page is on
//! screen, so a folded notch still costs nothing. Processes are listed by
//! app (eight `chrome.exe` count as one Chrome) from a worker thread every
//! two seconds while the page is open.

use egui::{Align2, Color32, Id, Rect, Sense, Ui, pos2, vec2};
use nook_platform::perf::{self, CpuTimes, Disk, Memory};
use nook_platform::shell;
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_LABEL, Theme, mix, with_alpha};
use nook_ui::{icons, paint, widgets};
use std::collections::{HashMap, VecDeque};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use crate::file_shelf::human_size;
use crate::kit;

const PAGE: egui::Vec2 = vec2(740.0, 450.0);
/// Seconds of history in the charts.
const HISTORY: usize = 60;
const SAMPLE: Duration = Duration::from_secs(1);
const PROCS_EVERY: Duration = Duration::from_secs(2);
/// The tile or page counts as on screen this long after it was drawn.
const SEEN: Duration = Duration::from_millis(1500);

const MEMORY_COLOR: Color32 = Color32::from_rgb(45, 212, 191);
const UPLOAD_COLOR: Color32 = Color32::from_rgb(245, 158, 11);

/// An app's share, summed over its processes.
#[derive(Clone, Debug, PartialEq)]
struct AppUse {
    name: String,
    count: usize,
    /// Share of the whole machine, 0..1.
    cpu: f32,
    memory: u64,
}

fn app_name(exe: &str) -> String {
    let base = exe
        .strip_suffix(".exe")
        .or_else(|| exe.strip_suffix(".EXE"))
        .unwrap_or(exe);
    let mut c = base.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => exe.to_owned(),
    }
}

/// Groups two process snapshots `elapsed` apart into per-app usage.
fn group(
    prev: &HashMap<u32, u64>,
    now: &[perf::Process],
    elapsed: Duration,
    cpus: usize,
) -> Vec<AppUse> {
    let wall = elapsed.as_secs_f64() * 1e7 * cpus.max(1) as f64;
    let mut apps: HashMap<String, AppUse> = HashMap::new();
    for p in now {
        if p.pid == 0 || p.name.eq_ignore_ascii_case("System") || p.name == "Idle" {
            continue;
        }
        let cpu = prev.get(&p.pid).map_or(0.0, |&before| {
            p.cpu.saturating_sub(before) as f64 / wall.max(1.0)
        });
        let key = p.name.to_lowercase();
        let e = apps.entry(key).or_insert_with(|| AppUse {
            name: app_name(&p.name),
            count: 0,
            cpu: 0.0,
            memory: 0,
        });
        e.count += 1;
        e.cpu += cpu as f32;
        e.memory += p.memory;
    }
    apps.into_values().collect()
}

fn uptime_label(d: Duration) -> String {
    let m = d.as_secs() / 60;
    let (days, hours, mins) = (m / 1440, (m / 60) % 24, m % 60);
    if days > 0 {
        format!("{days} d {hours} h")
    } else if hours > 0 {
        format!("{hours} h {mins} min")
    } else {
        format!("{mins} min")
    }
}

fn rate(bps: f32) -> String {
    format!("{}/s", human_size(bps.max(0.0) as u64))
}

type ProcSnapshot = (Instant, Vec<perf::Process>);

pub struct SystemMonitor {
    last: Option<Instant>,
    prev_cpu: Option<CpuTimes>,
    prev_net: Option<(u64, u64, Instant)>,
    cpu: VecDeque<f32>,
    mem: VecDeque<f32>,
    down: VecDeque<f32>,
    up: VecDeque<f32>,
    memory: Memory,
    disks: Vec<Disk>,
    disks_at: Option<Instant>,
    cpus: usize,
    /// Last frame the tile or the page drew.
    drawn: Option<Instant>,
    page_drawn: Option<Instant>,
    procs_tx: Option<Sender<()>>,
    procs_rx: Option<Receiver<ProcSnapshot>>,
    procs_asked: Option<Instant>,
    prev_procs: Option<(Instant, HashMap<u32, u64>)>,
    apps: Vec<AppUse>,
    by_memory: bool,
    open_page: bool,
}

impl Default for SystemMonitor {
    fn default() -> Self {
        Self {
            last: None,
            prev_cpu: None,
            prev_net: None,
            cpu: VecDeque::with_capacity(HISTORY),
            mem: VecDeque::with_capacity(HISTORY),
            down: VecDeque::with_capacity(HISTORY),
            up: VecDeque::with_capacity(HISTORY),
            memory: Memory::default(),
            disks: Vec::new(),
            disks_at: None,
            cpus: std::thread::available_parallelism().map_or(1, |n| n.get()),
            drawn: None,
            page_drawn: None,
            procs_tx: None,
            procs_rx: None,
            procs_asked: None,
            prev_procs: None,
            apps: Vec::new(),
            by_memory: false,
            open_page: false,
        }
    }
}

fn push(q: &mut VecDeque<f32>, v: f32) {
    if q.len() == HISTORY {
        q.pop_front();
    }
    q.push_back(v);
}

impl SystemMonitor {
    fn on_screen(&self) -> bool {
        self.drawn.is_some_and(|t| t.elapsed() < SEEN)
    }

    fn page_on_screen(&self) -> bool {
        self.page_drawn.is_some_and(|t| t.elapsed() < SEEN)
    }

    fn sample(&mut self) {
        let now = Instant::now();
        // After a pause the charts would show a gap as a ramp: start over.
        if self.last.is_none_or(|t| now - t > SAMPLE * 4) {
            self.prev_cpu = None;
            self.prev_net = None;
        }
        self.last = Some(now);
        if let Ok(t) = perf::cpu_times() {
            if let Some(p) = self.prev_cpu {
                push(&mut self.cpu, t.usage_since(&p));
            }
            self.prev_cpu = Some(t);
        }
        if let Ok(m) = perf::memory() {
            self.memory = m;
            push(&mut self.mem, m.used as f32 / m.total.max(1) as f32);
        }
        if let Ok((d, u)) = perf::net_bytes() {
            if let Some((pd, pu, at)) = self.prev_net {
                let s = (now - at).as_secs_f32().max(0.01);
                push(&mut self.down, d.saturating_sub(pd) as f32 / s);
                push(&mut self.up, u.saturating_sub(pu) as f32 / s);
            }
            self.prev_net = Some((d, u, now));
        }
        if self
            .disks_at
            .is_none_or(|t| t.elapsed() > Duration::from_secs(10))
        {
            self.disks = perf::disks();
            self.disks_at = Some(now);
        }
    }

    fn ensure_worker(&mut self, ctx: &egui::Context) {
        if self.procs_tx.is_some() {
            return;
        }
        let (req_tx, req_rx) = mpsc::channel::<()>();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("nook-procs".into())
            .spawn(move || {
                for () in req_rx {
                    match perf::processes() {
                        Ok(p) => {
                            if tx.send((Instant::now(), p)).is_err() {
                                break;
                            }
                            ctx.request_repaint();
                        }
                        Err(e) => tracing::debug!("processes: {e}"),
                    }
                }
            });
        if spawned.is_ok() {
            self.procs_tx = Some(req_tx);
            self.procs_rx = Some(rx);
        }
    }

    fn cpu_now(&self) -> f32 {
        self.cpu.back().copied().unwrap_or(0.0)
    }

    fn mem_now(&self) -> f32 {
        self.mem.back().copied().unwrap_or(0.0)
    }

    // ------------------------------------------------------- page parts

    fn stat_card(
        ui: &mut Ui,
        card: Rect,
        title: &str,
        big: &str,
        sub: &str,
        theme: &Theme,
    ) -> Rect {
        widgets::card(ui.painter(), card, theme);
        let inner = card.shrink2(vec2(14.0, 12.0));
        kit::section(ui, inner.min, title, theme);
        ui.painter().text(
            pos2(inner.min.x, inner.min.y + 16.0),
            Align2::LEFT_TOP,
            big,
            Theme::font_semibold(26.0),
            theme.text,
        );
        kit::one_line(
            ui,
            pos2(inner.min.x, inner.min.y + 50.0),
            sub,
            inner.width(),
            Theme::font(SIZE_LABEL),
            theme.text_dim,
        );
        Rect::from_min_max(
            pos2(card.min.x + 1.0, inner.min.y + 72.0),
            pos2(card.max.x - 1.0, card.max.y - 8.0),
        )
    }

    fn cards(&mut self, ui: &mut Ui, area: Rect, theme: &Theme) {
        let gap = 10.0;
        let w = (area.width() - gap) * 0.5;
        let h = (area.height() - gap) * 0.5;
        let cell = |c: usize, r: usize| {
            Rect::from_min_size(
                pos2(
                    area.min.x + c as f32 * (w + gap),
                    area.min.y + r as f32 * (h + gap),
                ),
                vec2(w, h),
            )
        };

        let chart = Self::stat_card(
            ui,
            cell(0, 0),
            "CPU",
            &format!("{:.0}%", self.cpu_now() * 100.0),
            &format!(
                "{} threads  \u{00B7}  up {}",
                self.cpus,
                uptime_label(perf::uptime())
            ),
            theme,
        );
        let v: Vec<f32> = self.cpu.iter().copied().collect();
        chart_grid(ui.painter(), chart, theme);
        kit::sparkline(ui.painter(), chart, &v, 1.0, theme.accent);

        let m = self.memory;
        let chart = Self::stat_card(
            ui,
            cell(1, 0),
            "MEMORY",
            &format!("{:.0}%", self.mem_now() * 100.0),
            &format!("{} of {} in use", human_size(m.used), human_size(m.total)),
            theme,
        );
        let v: Vec<f32> = self.mem.iter().copied().collect();
        chart_grid(ui.painter(), chart, theme);
        kit::sparkline(ui.painter(), chart, &v, 1.0, MEMORY_COLOR);

        let down = self.down.back().copied().unwrap_or(0.0);
        let up = self.up.back().copied().unwrap_or(0.0);
        let chart = Self::stat_card(
            ui,
            cell(0, 1),
            "NETWORK",
            &format!("\u{2193} {}", rate(down)),
            &format!("\u{2191} {} upload", rate(up)),
            theme,
        );
        let d: Vec<f32> = self.down.iter().copied().collect();
        let u: Vec<f32> = self.up.iter().copied().collect();
        let peak = d.iter().chain(&u).copied().fold(64.0 * 1024.0, f32::max) * 1.15;
        chart_grid(ui.painter(), chart, theme);
        kit::sparkline(ui.painter(), chart, &u, peak, UPLOAD_COLOR);
        kit::sparkline(ui.painter(), chart, &d, peak, theme.accent);

        // Drives.
        let card = cell(1, 1);
        widgets::card(ui.painter(), card, theme);
        let inner = card.shrink2(vec2(14.0, 12.0));
        kit::section(ui, inner.min, "STORAGE", theme);
        let mut y = inner.min.y + 22.0;
        for disk in &self.disks {
            if y + 30.0 > inner.max.y {
                break;
            }
            let used = 1.0 - disk.free as f32 / disk.total.max(1) as f32;
            let row = Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), 30.0));
            let resp = ui
                .interact(
                    row,
                    Id::new(("sysmon-disk", disk.name.as_str())),
                    Sense::click(),
                )
                .on_hover_text("Open in Explorer");
            if resp.clicked() {
                let _ = shell::open(&format!("{}\\", disk.name));
            }
            ui.painter().text(
                pos2(row.min.x, row.min.y + 6.0),
                Align2::LEFT_CENTER,
                &disk.name,
                Theme::font_semibold(SIZE_LABEL + 1.0),
                if resp.hovered() {
                    theme.accent_fg()
                } else {
                    theme.text
                },
            );
            ui.painter().text(
                pos2(row.max.x, row.min.y + 6.0),
                Align2::RIGHT_CENTER,
                format!(
                    "{} free of {}",
                    human_size(disk.free),
                    human_size(disk.total)
                ),
                Theme::font(SIZE_LABEL - 1.0),
                theme.text_dim,
            );
            let color = if used > 0.9 {
                Color32::from_rgb(239, 68, 68)
            } else {
                theme.accent
            };
            kit::meter(
                ui.painter(),
                Rect::from_min_size(pos2(row.min.x, row.min.y + 16.0), vec2(row.width(), 6.0)),
                used,
                color,
                theme,
            );
            y += 36.0;
        }
    }

    fn processes(&mut self, ui: &mut Ui, col: Rect, theme: &Theme) {
        widgets::card(ui.painter(), col, theme);
        let inner = col.shrink2(vec2(12.0, 12.0));
        kit::section(ui, inner.min + vec2(2.0, 0.0), "TOP APPS", theme);
        let chips = Rect::from_min_size(
            pos2(inner.max.x - 110.0, inner.min.y - 4.0),
            vec2(110.0, 22.0),
        );
        if let Some(i) = kit::chips(
            ui,
            chips,
            &["CPU", "Memory"],
            usize::from(self.by_memory),
            theme,
        ) {
            self.by_memory = i == 1;
        }
        let mut apps = self.apps.clone();
        if self.by_memory {
            apps.sort_by_key(|a| std::cmp::Reverse(a.memory));
        } else {
            apps.sort_by(|a, b| {
                b.cpu
                    .partial_cmp(&a.cpu)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(b.memory.cmp(&a.memory))
            });
        }
        if apps.is_empty() {
            ui.painter().text(
                inner.center(),
                Align2::CENTER_CENTER,
                "Measuring\u{2026}",
                Theme::font(SIZE_LABEL + 1.0),
                theme.text_faint,
            );
            return;
        }
        let row_h = 34.0;
        let top = inner.min.y + 26.0;
        let rows = ((inner.max.y - top) / row_h).floor() as usize;
        let max_mem = apps.iter().map(|a| a.memory).max().unwrap_or(1).max(1);
        for (i, a) in apps.iter().take(rows).enumerate() {
            let r = Rect::from_min_size(
                pos2(inner.min.x, top + i as f32 * row_h),
                vec2(inner.width(), row_h - 4.0),
            );
            // A bar behind the row shows its share.
            let share = if self.by_memory {
                a.memory as f32 / max_mem as f32
            } else {
                (a.cpu * 4.0).min(1.0)
            };
            let bar = Rect::from_min_size(r.min, vec2(r.width() * share.max(0.0), r.height()));
            let color = if self.by_memory {
                MEMORY_COLOR
            } else {
                theme.accent
            };
            ui.painter().rect_filled(
                bar,
                8.0,
                with_alpha(color, if theme.dark { 0.16 } else { 0.12 }),
            );
            let name = if a.count > 1 {
                format!("{}  \u{00D7}{}", a.name, a.count)
            } else {
                a.name.clone()
            };
            kit::one_line(
                ui,
                pos2(r.min.x + 8.0, r.center().y - 8.0),
                &name,
                r.width() - 112.0,
                Theme::font(SIZE_LABEL + 1.0),
                theme.text,
            );
            ui.painter().text(
                pos2(r.max.x - 66.0, r.center().y),
                Align2::RIGHT_CENTER,
                format!("{:.1}%", a.cpu * 100.0),
                Theme::font_semibold(SIZE_LABEL),
                if self.by_memory {
                    theme.text_dim
                } else {
                    theme.text
                },
            );
            ui.painter().text(
                pos2(r.max.x - 6.0, r.center().y),
                Align2::RIGHT_CENTER,
                human_size(a.memory),
                Theme::font_semibold(SIZE_LABEL),
                if self.by_memory {
                    theme.text
                } else {
                    theme.text_dim
                },
            );
        }
    }
}

/// Faint quarter lines behind a chart.
fn chart_grid(painter: &egui::Painter, rect: Rect, theme: &Theme) {
    for k in 1..4 {
        let y = rect.min.y + rect.height() * k as f32 / 4.0;
        painter.hline(
            rect.x_range(),
            y,
            egui::Stroke::new(1.0, with_alpha(theme.tile_rim, 0.6)),
        );
    }
}

/// A ring gauge with its value in the middle.
fn gauge(
    painter: &egui::Painter,
    c: egui::Pos2,
    r: f32,
    t: f32,
    color: Color32,
    label: &str,
    theme: &Theme,
) {
    paint::ring(painter, c, r, 3.5, t, kit::wash(theme), color);
    painter.text(
        c,
        Align2::CENTER_CENTER,
        label,
        Theme::font_semibold(if r > 16.0 {
            SIZE_LABEL
        } else {
            SIZE_LABEL - 2.0
        }),
        theme.text,
    );
}

impl Module for SystemMonitor {
    fn id(&self) -> &'static str {
        "system_monitor"
    }

    fn name(&self) -> &str {
        "System monitor"
    }

    fn icon(&self) -> &'static str {
        icons::CPU
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn on_tick(&mut self) {
        if self.on_screen() && self.last.is_none_or(|t| t.elapsed() >= SAMPLE) {
            self.sample();
        }
        if let Some(rx) = &self.procs_rx
            && let Some((at, procs)) = rx.try_iter().last()
        {
            let map: HashMap<u32, u64> = procs.iter().map(|p| (p.pid, p.cpu)).collect();
            if let Some((prev_at, prev)) = &self.prev_procs {
                self.apps = group(prev, &procs, at - *prev_at, self.cpus);
            }
            self.prev_procs = Some((at, map));
        }
        if self.page_on_screen()
            && self.procs_asked.is_none_or(|t| t.elapsed() >= PROCS_EVERY)
            && let Some(tx) = &self.procs_tx
        {
            let _ = tx.send(());
            self.procs_asked = Some(Instant::now());
        }
    }

    fn next_repaint(&self, tile_visible: bool) -> Option<Duration> {
        if !(tile_visible && self.on_screen()) && !self.page_on_screen() {
            return None;
        }
        let since = self.last.map_or(SAMPLE, |t| t.elapsed());
        Some(SAMPLE.saturating_sub(since) + Duration::from_millis(5))
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.open_page)
    }

    fn page_size(&self) -> Option<egui::Vec2> {
        Some(PAGE)
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        self.drawn = Some(Instant::now());
        if self.last.is_none() {
            self.sample();
        }
        let rect = ui.max_rect();
        let resp = ui.interact(
            rect.expand(10.0),
            ui.id().with("sysmon-tile"),
            Sense::click(),
        );
        let painter = ui.painter();
        let cpu = self.cpu_now();
        let mem = self.mem_now();
        if rect.width() > rect.height() * 1.5 || rect.height() > 80.0 {
            // Wide and large: two gauges with names; large adds the chart.
            let large = rect.height() > 80.0;
            let w = rect.width() * 0.5;
            // Large: each ring centered in its half, its name under it.
            // Wide: a smaller ring with its name beside it.
            let r = if large {
                (w * 0.5 - 6.0).min(20.0)
            } else {
                (rect.height() * 0.5 - 4.0).min(13.0)
            };
            let cy = rect.min.y + r + 2.0;
            let mut bottom = cy + r;
            for (i, (t, color, name)) in [(cpu, theme.accent, "CPU"), (mem, MEMORY_COLOR, "RAM")]
                .into_iter()
                .enumerate()
            {
                let half_x = rect.min.x + w * i as f32;
                let c = if large {
                    pos2(half_x + w * 0.5, cy)
                } else {
                    pos2(half_x + r + 2.0, rect.center().y)
                };
                gauge(painter, c, r, t, color, &format!("{:.0}", t * 100.0), theme);
                let (pos, align) = if large {
                    (pos2(c.x, c.y + r + 9.0), Align2::CENTER_CENTER)
                } else {
                    (pos2(c.x + r + 5.0, c.y), Align2::LEFT_CENTER)
                };
                let label = painter.text(
                    pos,
                    align,
                    name,
                    Theme::font_semibold(SIZE_LABEL - 2.0),
                    theme.text_dim,
                );
                bottom = bottom.max(label.max.y);
            }
            if large {
                let chart = Rect::from_min_max(pos2(rect.min.x, bottom + 6.0), rect.max);
                let v: Vec<f32> = self.cpu.iter().copied().collect();
                kit::sparkline(ui.painter(), chart, &v, 1.0, theme.accent);
            }
        } else {
            let c = rect.center();
            let r = rect.width().min(rect.height()) * 0.5 - 2.0;
            paint::ring(painter, c, r, 3.0, cpu, kit::wash(theme), theme.accent);
            paint::ring(
                painter,
                c,
                r - 5.5,
                2.5,
                mem,
                kit::wash(theme),
                MEMORY_COLOR,
            );
            painter.text(
                c,
                Align2::CENTER_CENTER,
                format!("{:.0}%", cpu * 100.0),
                Theme::font_semibold(SIZE_LABEL - 1.0),
                if ctx.hovered {
                    theme.text
                } else {
                    mix(theme.text, theme.text_dim, 0.3)
                },
            );
        }
        let tip = format!(
            "CPU {:.0}%  \u{00B7}  Memory {:.0}% ({} of {})",
            cpu * 100.0,
            mem * 100.0,
            human_size(self.memory.used),
            human_size(self.memory.total)
        );
        if resp.on_hover_text(tip).clicked() {
            self.open_page = true;
        }
    }

    fn ui_page(&mut self, ui: &mut Ui, header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        self.page_drawn = Some(Instant::now());
        self.drawn = self.page_drawn;
        self.ensure_worker(ui.ctx());
        if self.last.is_none() {
            self.sample();
        }
        if widgets::icon_button(header, icons::ARROW_SQUARE_OUT, 26.0, false, theme)
            .on_hover_text("Open Task Manager")
            .clicked()
        {
            let _ = shell::open("taskmgr.exe");
        }
        let left = Rect::from_min_size(body.min, vec2(450.0, body.height()));
        self.cards(ui, left, theme);
        let col = Rect::from_min_max(pos2(left.max.x + 10.0, body.min.y), body.max);
        self.processes(ui, col, theme);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_processes_by_app() {
        let p = |pid, name: &str, cpu, memory| perf::Process {
            pid,
            name: name.into(),
            cpu,
            memory,
        };
        let prev: HashMap<u32, u64> = [(1, 0), (2, 0), (3, 0)].into_iter().collect();
        let now = [
            p(1, "chrome.exe", 5_000_000, 100),
            p(2, "chrome.exe", 5_000_000, 50),
            p(3, "code.exe", 0, 10),
            p(0, "[System Process]", 99, 0),
        ];
        // One second on two CPUs: 2e7 ticks in all.
        let mut apps = group(&prev, &now, Duration::from_secs(1), 2);
        apps.sort_by(|a, b| a.name.cmp(&b.name));
        assert_eq!(apps.len(), 2);
        assert_eq!(apps[0].name, "Chrome");
        assert_eq!(apps[0].count, 2);
        assert!((apps[0].cpu - 0.5).abs() < 1e-6);
        assert_eq!(apps[0].memory, 150);
        assert_eq!(apps[1].name, "Code");
    }

    #[test]
    fn labels() {
        assert_eq!(uptime_label(Duration::from_secs(90 * 60)), "1 h 30 min");
        assert_eq!(
            uptime_label(Duration::from_secs(3 * 86_400 + 4 * 3600)),
            "3 d 4 h"
        );
        assert_eq!(uptime_label(Duration::from_secs(300)), "5 min");
        assert_eq!(rate(1536.0), "1.5 KB/s");
    }
}
