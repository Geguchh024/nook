//! Voice memo: record your microphone from the notch.
//!
//! Press the big button (or Ctrl+Alt+R from anywhere) to record and again to
//! stop; the pill shows the time while it records. Memos are WAV files in
//! `Music\Voice memos`, listed newest first with their waveform. Play one
//! with a click, drag it into a chat or an email, rename it, show it in
//! Explorer or move it to the Recycle Bin.
//!
//! Recording runs on its own thread (see `nook_platform::audio`), writing
//! to disk as it goes, so a long memo never piles up in memory.

use egui::{Align2, Color32, Id, Rect, Sense, Ui, pos2, vec2};
use nook_platform::audio::{self, SAMPLE_RATE, WavWriter};
use nook_platform::{files, shell, system};
use nook_ui::module::{Badge, Module, TileCtx, TileSize};
use nook_ui::spring::SpringParams;
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme, mix, with_alpha};
use nook_ui::{anim, icons, paint, widgets};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use crate::file_shelf::human_size;
use crate::kit;

const PAGE: egui::Vec2 = vec2(700.0, 430.0);
const RED: Color32 = Color32::from_rgb(239, 68, 68);
/// Bars in a memo's waveform.
const BARS: usize = 48;
/// Live levels kept while recording (50 ms each).
const LIVE: usize = 90;

struct Memo {
    path: PathBuf,
    secs: f32,
    bytes: u64,
    peaks: Option<Vec<f32>>,
    asked: bool,
}

fn memos_dir() -> Option<PathBuf> {
    shell::music_dir().map(|m| m.join("Voice memos"))
}

/// Duration from a WAV file's size (our files: 16-bit mono, 44-byte header).
fn wav_secs(bytes: u64) -> f32 {
    bytes.saturating_sub(44) as f32 / 2.0 / SAMPLE_RATE as f32
}

fn load_memos(dir: &Path) -> Vec<Memo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut memos: Vec<(std::time::SystemTime, Memo)> = entries
        .flatten()
        .filter(|e| {
            e.path()
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case("wav"))
        })
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            Some((
                meta.modified().ok()?,
                Memo {
                    path: e.path(),
                    secs: wav_secs(meta.len()),
                    bytes: meta.len(),
                    peaks: None,
                    asked: false,
                },
            ))
        })
        .collect();
    memos.sort_by_key(|(t, _)| std::cmp::Reverse(*t));
    memos.into_iter().map(|(_, m)| m).collect()
}

/// `Memo 2026-10-20 14-32-05` reads as `20 Oct, 14:32`; other names as is.
fn title(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parse = || {
        let rest = stem.strip_prefix("Memo ")?;
        let (date, time) = rest.split_once(' ')?;
        let mut d = date.split('-');
        let (_, m, day) = (
            d.next()?,
            d.next()?.parse::<usize>().ok()?,
            d.next()?.parse::<u32>().ok()?,
        );
        let mut t = time.split('-');
        let (h, min) = (t.next()?, t.next()?);
        const MONTHS: [&str; 12] = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        Some(format!(
            "{day} {}, {h}:{min}",
            MONTHS.get(m.wrapping_sub(1))?
        ))
    };
    parse().unwrap_or(stem)
}

fn clock(secs: f32) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Peak of each of `BARS` slices, scaled so the loudest is 1.
fn peaks(samples: &[i16]) -> Vec<f32> {
    if samples.is_empty() {
        return vec![0.0; BARS];
    }
    let per = samples.len().div_ceil(BARS).max(1);
    let raw: Vec<f32> = samples
        .chunks(per)
        .map(|c| c.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0) as f32 / 32768.0)
        .collect();
    let top = raw.iter().copied().fold(0.02, f32::max);
    let mut out: Vec<f32> = raw.iter().map(|v| (v / top).sqrt()).collect();
    out.resize(BARS, 0.0);
    out
}

enum RecMsg {
    Level(f32),
    Done(Result<PathBuf, String>),
}

struct Recording {
    stop: Arc<AtomicBool>,
    started: Instant,
    rx: Receiver<RecMsg>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn start_recording(dir: PathBuf) -> Recording {
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let flag = stop.clone();
    let thread = std::thread::Builder::new()
        .name("nook-voice".into())
        .spawn(move || {
            let res = (|| {
                std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let path = dir.join(format!("Memo {}.wav", system::file_timestamp()));
                let mut writer = WavWriter::create(&path).map_err(|e| e.to_string())?;
                let mut samples = 0usize;
                let mut failed = None;
                let rec = audio::record(&flag, |chunk| {
                    samples += chunk.len();
                    if let Err(e) = writer.write(chunk) {
                        failed = Some(e.to_string());
                        flag.store(true, Ordering::Relaxed);
                    }
                    let peak = chunk.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
                    let _ = tx.send(RecMsg::Level(peak as f32 / 32768.0));
                });
                let finished = writer.finish().map_err(|e| e.to_string());
                let too_short = samples < SAMPLE_RATE as usize / 2;
                let outcome = rec
                    .map_err(|e| e.to_string())
                    .and(failed.map_or(Ok(()), Err))
                    .and(finished)
                    .and(if too_short {
                        Err("Too short to keep".to_owned())
                    } else {
                        Ok(())
                    });
                if outcome.is_err() {
                    let _ = std::fs::remove_file(&path);
                }
                outcome.map(|()| path)
            })();
            let _ = tx.send(RecMsg::Done(res));
        })
        .ok();
    Recording {
        stop,
        started: Instant::now(),
        rx,
        thread,
    }
}

type Peaks = (PathBuf, Vec<f32>);

pub struct VoiceMemo {
    dir: Option<PathBuf>,
    memos: Vec<Memo>,
    recording: Option<Recording>,
    live: VecDeque<f32>,
    /// Playing: which file, since when, for how long.
    playing: Option<(PathBuf, Instant, f32)>,
    peaks_tx: Option<Sender<PathBuf>>,
    peaks_rx: Option<Receiver<Peaks>>,
    renaming: Option<(PathBuf, String)>,
    open_page: bool,
    page_seen: Option<Instant>,
    toast: Option<(String, Instant)>,
    ctx: Option<egui::Context>,
}

impl Default for VoiceMemo {
    fn default() -> Self {
        Self {
            dir: None,
            memos: Vec::new(),
            recording: None,
            live: VecDeque::with_capacity(LIVE),
            playing: None,
            peaks_tx: None,
            peaks_rx: None,
            renaming: None,
            open_page: false,
            page_seen: None,
            toast: None,
            ctx: None,
        }
    }
}

impl VoiceMemo {
    fn reload(&mut self) {
        if let Some(dir) = &self.dir {
            self.memos = load_memos(dir);
        }
    }

    fn toggle_recording(&mut self) {
        if let Some(rec) = &self.recording {
            rec.stop.store(true, Ordering::Relaxed);
            return;
        }
        let Some(dir) = self.dir.clone() else {
            return;
        };
        self.stop_playing();
        self.live.clear();
        self.recording = Some(start_recording(dir));
        tracing::info!("voice memo: recording");
    }

    fn elapsed(&self) -> Option<f32> {
        self.recording
            .as_ref()
            .map(|r| r.started.elapsed().as_secs_f32())
    }

    fn play(&mut self, path: &Path, secs: f32) {
        if self.playing.as_ref().is_some_and(|(p, ..)| p == path) {
            self.stop_playing();
            return;
        }
        match audio::play(path) {
            Ok(()) => self.playing = Some((path.to_path_buf(), Instant::now(), secs)),
            Err(e) => self.toast(e.to_string()),
        }
    }

    fn stop_playing(&mut self) {
        if self.playing.take().is_some() {
            audio::stop_playback();
        }
    }

    fn toast(&mut self, t: impl Into<String>) {
        self.toast = Some((t.into(), Instant::now()));
    }

    fn page_visible(&self) -> bool {
        self.page_seen
            .is_some_and(|t| t.elapsed().as_secs_f32() < 0.5)
    }

    fn ensure_peaks_worker(&mut self) {
        if self.peaks_tx.is_some() {
            return;
        }
        let (req_tx, req_rx) = mpsc::channel::<PathBuf>();
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("nook-voice-peaks".into())
            .spawn(move || {
                for path in req_rx {
                    if let Ok((_, samples)) = audio::read_wav(&path) {
                        if tx.send((path, peaks(&samples))).is_err() {
                            break;
                        }
                        if let Some(c) = &ctx {
                            c.request_repaint();
                        }
                    }
                }
            });
        if spawned.is_ok() {
            self.peaks_tx = Some(req_tx);
            self.peaks_rx = Some(rx);
        }
    }

    fn rename(&mut self, from: &Path, to: &str) {
        let to = to.trim();
        let clean: String = to
            .chars()
            .filter(|c| !matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
            .collect();
        if clean.is_empty() {
            return;
        }
        let target = from.with_file_name(format!("{clean}.wav"));
        if target == from {
            return;
        }
        if target.exists() {
            self.toast("A memo with that name exists");
            return;
        }
        if self.playing.as_ref().is_some_and(|(p, ..)| p == from) {
            self.stop_playing();
        }
        match std::fs::rename(from, &target) {
            Ok(()) => {
                if let Some(m) = self.memos.iter_mut().find(|m| m.path == from) {
                    m.path = target;
                }
            }
            Err(e) => self.toast(format!("Could not rename: {e}")),
        }
    }

    // ------------------------------------------------------- page parts

    fn recorder(&mut self, ui: &mut Ui, card: Rect, theme: &Theme) {
        widgets::card(ui.painter(), card, theme);
        let ctx = ui.ctx().clone();
        let recording = self.recording.is_some();
        let inner = card.shrink2(vec2(18.0, 16.0));

        // Time.
        let elapsed = self.elapsed().unwrap_or(0.0);
        let tenths = ((elapsed * 10.0) as u32) % 10;
        let time = format!("{}.{tenths}", clock(elapsed));
        ui.painter().text(
            pos2(inner.center().x, inner.min.y + 26.0),
            Align2::CENTER_CENTER,
            time,
            Theme::font_semibold(36.0),
            if recording {
                theme.text
            } else {
                theme.text_faint
            },
        );
        let status = if recording {
            "Recording".to_owned()
        } else {
            "Ready".to_owned()
        };
        let pulse = if recording {
            0.5 + 0.5 * (ui.input(|i| i.time) as f32 * 4.0).sin()
        } else {
            0.0
        };
        let sw = ui
            .painter()
            .layout_no_wrap(status.clone(), Theme::font_semibold(SIZE_LABEL), theme.text)
            .size()
            .x;
        let sy = inner.min.y + 56.0;
        if recording {
            ui.painter().circle_filled(
                pos2(inner.center().x - sw * 0.5 - 8.0, sy),
                3.5,
                with_alpha(RED, 0.4 + 0.6 * pulse),
            );
        }
        ui.painter().text(
            pos2(inner.center().x + if recording { 4.0 } else { 0.0 }, sy),
            Align2::CENTER_CENTER,
            status,
            Theme::font_semibold(SIZE_LABEL),
            if recording { RED } else { theme.text_dim },
        );

        // Live waveform: newest on the right.
        let wave = Rect::from_min_size(
            pos2(inner.min.x, inner.min.y + 80.0),
            vec2(inner.width(), 96.0),
        );
        ui.painter().rect_filled(wave, 12.0, kit::wash(theme));
        let n = LIVE;
        let bw = wave.width() / n as f32;
        let mid = wave.center().y;
        let levels: Vec<f32> = self.live.iter().copied().collect();
        for k in 0..n {
            let idx = levels.len() as isize - (n - k) as isize;
            let level = if idx >= 0 { levels[idx as usize] } else { 0.0 };
            let h = (level.sqrt() * (wave.height() - 16.0)).max(2.0);
            let x = wave.min.x + bw * (k as f32 + 0.5);
            let fade = (k as f32 / n as f32).powf(0.6);
            let color = if recording {
                with_alpha(mix(theme.accent, RED, 0.35), 0.35 + 0.65 * fade)
            } else {
                with_alpha(theme.text_faint, 0.6)
            };
            ui.painter().rect_filled(
                Rect::from_center_size(pos2(x, mid), vec2((bw - 1.5).max(1.0), h)),
                1.0,
                color,
            );
        }

        // The button: a red disc whose dot morphs into a stop square.
        let c = pos2(inner.center().x, inner.max.y - 54.0);
        let id = Id::new("voice-record");
        let resp = ui.interact(
            Rect::from_center_size(c, vec2(72.0, 72.0)),
            id,
            Sense::click(),
        );
        let hover = anim::tween(&ctx, id.with("h"), resp.hovered(), 0.12);
        let t = anim::spring(
            &ctx,
            id.with("morph"),
            if recording { 1.0 } else { 0.0 },
            SpringParams::with_overshoot(420.0, 0.08),
        );
        let painter = ui.painter();
        painter.circle_stroke(
            c,
            33.0,
            egui::Stroke::new(3.0, with_alpha(theme.text, 0.18 + 0.1 * hover)),
        );
        if recording {
            painter.circle_filled(c, 33.0 + 5.0 * pulse, with_alpha(RED, 0.10 * pulse));
        }
        let size = 44.0 - 18.0 * t;
        let radius = (size * 0.5) * (1.0 - t) + 5.0 * t;
        let press = if resp.is_pointer_button_down_on() {
            0.94
        } else {
            1.0
        };
        painter.rect_filled(
            Rect::from_center_size(c, vec2(size, size) * press),
            radius,
            mix(RED, Color32::WHITE, 0.08 * hover),
        );
        let hint = if recording {
            "Click to stop and save"
        } else {
            "Click to record  \u{00B7}  Ctrl+Alt+R anywhere"
        };
        ui.painter().text(
            pos2(c.x, inner.max.y - 4.0),
            Align2::CENTER_CENTER,
            hint,
            Theme::font(SIZE_LABEL - 1.0),
            theme.text_faint,
        );
        if resp.clicked() {
            self.toggle_recording();
        }
    }

    fn list(&mut self, ui: &mut Ui, area: Rect, theme: &Theme) {
        kit::section(ui, area.min + vec2(4.0, 0.0), "MEMOS", theme);
        if !self.memos.is_empty() {
            let total: f32 = self.memos.iter().map(|m| m.secs).sum();
            ui.painter().text(
                pos2(area.max.x - 4.0, area.min.y),
                Align2::RIGHT_TOP,
                format!("{}  \u{00B7}  {}", self.memos.len(), clock(total)),
                Theme::font(SIZE_LABEL - 1.0),
                theme.text_faint,
            );
        }
        let list = Rect::from_min_max(area.min + vec2(0.0, 20.0), area.max);
        if self.memos.is_empty() {
            kit::empty(
                ui,
                list.center(),
                icons::MICROPHONE,
                "Your memos will show up here",
                theme,
            );
            return;
        }
        // Waveforms load lazily.
        self.ensure_peaks_worker();
        if let Some(tx) = &self.peaks_tx {
            for m in self.memos.iter_mut().filter(|m| !m.asked) {
                m.asked = true;
                let _ = tx.send(m.path.clone());
            }
        }

        let row_h = 56.0;
        let mut action: Option<(usize, &str)> = None;
        let mut lui = ui.new_child(egui::UiBuilder::new().max_rect(list).id_salt("voice-list"));
        let renaming = self.renaming.clone();
        let mut rename_text = renaming.as_ref().map(|(_, t)| t.clone());
        let mut rename_done = None;
        egui::ScrollArea::vertical()
            .id_salt("nook-voice-list")
            .max_height(list.height())
            .auto_shrink([false, false])
            .show(&mut lui, |ui| {
                let (rows, _) = ui.allocate_exact_size(
                    vec2(list.width(), self.memos.len() as f32 * row_h),
                    Sense::hover(),
                );
                for (i, m) in self.memos.iter().enumerate() {
                    let r = Rect::from_min_size(
                        pos2(rows.min.x, rows.min.y + i as f32 * row_h),
                        vec2(rows.width() - 4.0, row_h - 6.0),
                    );
                    if !ui.is_rect_visible(r) {
                        continue;
                    }
                    let playing = self.playing.as_ref().filter(|(p, ..)| *p == m.path);
                    widgets::card(ui.painter(), r, theme);
                    let resp = ui.interact(r, Id::new(("voice-row", i)), Sense::click_and_drag());
                    if resp.drag_started() {
                        action = Some((i, "drag"));
                    }

                    // Play button.
                    let pc = pos2(r.min.x + 26.0, r.center().y);
                    let pid = Id::new(("voice-play", i));
                    let presp = ui.interact(
                        Rect::from_center_size(pc, vec2(34.0, 34.0)),
                        pid,
                        Sense::click(),
                    );
                    let ph = anim::tween(ui.ctx(), pid.with("h"), presp.hovered(), 0.1);
                    ui.painter().circle_filled(
                        pc,
                        16.0,
                        mix(theme.accent, Color32::WHITE, 0.12 * ph),
                    );
                    paint::icon(
                        ui.painter(),
                        pc + vec2(if playing.is_some() { 0.0 } else { 1.0 }, 0.0),
                        if playing.is_some() {
                            icons::STOP
                        } else {
                            icons::PLAY
                        },
                        14.0,
                        Color32::WHITE,
                        true,
                    );
                    if presp.clicked() {
                        action = Some((i, "play"));
                    }

                    // Name and details, or the rename field.
                    let text_x = pc.x + 26.0;
                    let wave = Rect::from_min_max(
                        pos2(r.max.x - 120.0, r.min.y + 12.0),
                        pos2(r.max.x - 12.0, r.max.y - 12.0),
                    );
                    let name_w = wave.min.x - text_x - 10.0;
                    if let (Some((path, _)), Some(text)) = (&renaming, rename_text.as_mut())
                        && *path == m.path
                    {
                        let field = Rect::from_min_size(
                            pos2(text_x, r.center().y - 15.0),
                            vec2(name_w + 100.0, 30.0),
                        );
                        let fid = Id::new("nook-voice-rename");
                        let fr =
                            kit::field(ui, field, text, "Name", fid, Theme::font(SIZE_BODY), theme);
                        if !fr.has_focus() && !fr.lost_focus() {
                            fr.request_focus();
                        }
                        if fr.lost_focus() {
                            rename_done = Some(ui.input(|i| i.key_pressed(egui::Key::Enter)));
                        }
                        continue;
                    }
                    kit::one_line(
                        ui,
                        pos2(text_x, r.min.y + 9.0),
                        &title(&m.path),
                        name_w,
                        Theme::font_semibold(SIZE_BODY),
                        theme.text,
                    );
                    let detail = match playing {
                        Some((_, at, secs)) => format!(
                            "{} of {}",
                            clock(at.elapsed().as_secs_f32().min(*secs)),
                            clock(*secs)
                        ),
                        None => format!("{}  \u{00B7}  {}", clock(m.secs), human_size(m.bytes)),
                    };
                    ui.painter().text(
                        pos2(text_x, r.max.y - 13.0),
                        Align2::LEFT_CENTER,
                        detail,
                        Theme::font(SIZE_LABEL - 1.0),
                        theme.text_dim,
                    );

                    // Waveform with the play position, or actions on hover.
                    let hovered = ui.rect_contains_pointer(r);
                    if hovered && !resp.dragged() {
                        let icons_at = [
                            (icons::PENCIL_SIMPLE, "rename", "Rename"),
                            (icons::FOLDER_OPEN, "reveal", "Show in Explorer"),
                            (icons::TRASH, "delete", "Move to the Recycle Bin"),
                        ];
                        for (k, (glyph, what, tip)) in icons_at.iter().enumerate() {
                            let c = pos2(wave.max.x - 12.0 - k as f32 * 32.0, r.center().y);
                            if kit::icon_at(ui, c, 28.0, glyph, false, theme)
                                .on_hover_text(*tip)
                                .clicked()
                            {
                                action = Some((i, what));
                            }
                        }
                    } else if let Some(p) = &m.peaks {
                        let progress = playing.map(|(_, at, secs)| {
                            (at.elapsed().as_secs_f32() / secs.max(0.01)).min(1.0)
                        });
                        let bw = wave.width() / p.len() as f32;
                        for (k, v) in p.iter().enumerate() {
                            let x = wave.min.x + bw * (k as f32 + 0.5);
                            let h = (v * wave.height()).max(2.0);
                            let played =
                                progress.is_some_and(|t| (k as f32 + 0.5) / p.len() as f32 <= t);
                            let color = if played {
                                theme.accent
                            } else {
                                with_alpha(theme.text_dim, 0.55)
                            };
                            ui.painter().rect_filled(
                                Rect::from_center_size(
                                    pos2(x, wave.center().y),
                                    vec2((bw - 1.0).max(1.0), h),
                                ),
                                1.0,
                                color,
                            );
                        }
                    }
                    if !hovered {
                        continue;
                    }
                    resp.on_hover_text("Drag into a chat or an email");
                }
            });

        if let (Some(done), Some((path, _))) = (rename_done, renaming) {
            if done && let Some(text) = rename_text {
                self.rename(&path, &text);
            }
            self.renaming = None;
        } else if let (Some((path, _)), Some(text)) = (&self.renaming, rename_text) {
            self.renaming = Some((path.clone(), text));
        }

        if let Some((i, what)) = action {
            let (path, secs) = (self.memos[i].path.clone(), self.memos[i].secs);
            match what {
                "play" => self.play(&path, secs),
                "drag" => {
                    if let Err(e) = files::drag_out(std::slice::from_ref(&path)) {
                        tracing::warn!("voice drag: {e}");
                    }
                }
                "rename" => {
                    let stem = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    self.renaming = Some((path, stem));
                }
                "reveal" => {
                    let _ = shell::reveal(&path);
                }
                "delete" => {
                    if self.playing.as_ref().is_some_and(|(p, ..)| *p == path) {
                        self.stop_playing();
                    }
                    match shell::recycle(&path) {
                        Ok(()) => {
                            self.memos.remove(i);
                        }
                        Err(e) => self.toast(format!("Could not delete: {e}")),
                    }
                }
                _ => {}
            }
        }
    }
}

impl Module for VoiceMemo {
    fn id(&self) -> &'static str {
        "voice_memo"
    }

    fn name(&self) -> &str {
        "Voice memo"
    }

    fn icon(&self) -> &'static str {
        icons::MICROPHONE
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn attach(&mut self, ctx: &egui::Context) {
        self.ctx = Some(ctx.clone());
        self.dir = memos_dir();
        self.reload();
    }

    fn is_active(&self) -> bool {
        self.recording.is_some()
    }

    fn collapsed_badge(&self) -> Option<Badge> {
        let secs = self.elapsed()?;
        Some(Badge::Ring {
            progress: (secs % 60.0) / 60.0,
            label: clock(secs),
        })
    }

    fn on_tick(&mut self) {
        let mut done = None;
        if let Some(rec) = &self.recording {
            for msg in rec.rx.try_iter() {
                match msg {
                    RecMsg::Level(l) => {
                        if self.live.len() == LIVE {
                            self.live.pop_front();
                        }
                        self.live.push_back(l);
                    }
                    RecMsg::Done(r) => done = Some(r),
                }
            }
        }
        if let Some(res) = done {
            if let Some(mut rec) = self.recording.take()
                && let Some(t) = rec.thread.take()
            {
                let _ = t.join();
            }
            match res {
                Ok(path) => {
                    tracing::info!("voice memo: saved {}", path.display());
                    self.reload();
                    self.toast("Saved to Music\\Voice memos");
                }
                Err(e) => self.toast(e),
            }
        }
        if let Some(rx) = &self.peaks_rx {
            for (path, p) in rx.try_iter() {
                if let Some(m) = self.memos.iter_mut().find(|m| m.path == path) {
                    m.peaks = Some(p);
                }
            }
        }
        if let Some((_, at, secs)) = &self.playing
            && at.elapsed().as_secs_f32() > *secs + 0.2
        {
            self.playing = None;
        }
    }

    fn next_repaint(&self, _tile_visible: bool) -> Option<Duration> {
        let busy = self.recording.is_some() || self.playing.is_some();
        if busy {
            // Smooth while the page shows it; the pill's clock needs less.
            return Some(if self.page_visible() {
                Duration::from_millis(50)
            } else {
                Duration::from_millis(500)
            });
        }
        self.toast
            .as_ref()
            .and_then(|(_, at)| Duration::from_secs(3).checked_sub(at.elapsed()))
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.open_page)
    }

    fn page_size(&self) -> Option<egui::Vec2> {
        Some(PAGE)
    }

    fn default_hotkey(&self) -> Option<&'static str> {
        Some("ctrl+alt+KeyR")
    }

    fn on_hotkey(&mut self) {
        self.toggle_recording();
    }

    fn shutdown(&mut self) {
        // Let the recorder finish the file.
        if let Some(mut rec) = self.recording.take() {
            rec.stop.store(true, Ordering::Relaxed);
            if let Some(t) = rec.thread.take() {
                let _ = t.join();
            }
        }
        self.stop_playing();
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        let rect = ui.max_rect();
        if let Some(secs) = self.elapsed() {
            let resp = ui.interact(rect.expand(8.0), ui.id().with("voice-tile"), Sense::click());
            let pulse = 0.5 + 0.5 * (ctx.now as f32 * 4.0).sin();
            let c = rect.center() - vec2(0.0, 6.0);
            ui.painter()
                .circle_filled(c, 11.0 + 3.0 * pulse, with_alpha(RED, 0.18));
            ui.painter().circle_filled(c, 6.0, RED);
            ui.painter().text(
                rect.center() + vec2(0.0, 14.0),
                Align2::CENTER_CENTER,
                clock(secs),
                Theme::font_semibold(SIZE_LABEL - 1.0),
                theme.text,
            );
            if resp
                .on_hover_text("Recording  \u{00B7}  click to stop")
                .clicked()
            {
                self.toggle_recording();
            }
            return;
        }
        if kit::tool_tile(ui, ctx, icons::MICROPHONE, false)
            .on_hover_text("Record a voice memo  \u{00B7}  Ctrl+Alt+R")
            .clicked()
        {
            self.reload();
            self.open_page = true;
        }
    }

    fn ui_page(&mut self, ui: &mut Ui, header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        self.page_seen = Some(Instant::now());
        if widgets::icon_button(header, icons::FOLDER_OPEN, 26.0, false, theme)
            .on_hover_text("Open Music\\Voice memos")
            .clicked()
            && let Some(dir) = &self.dir
        {
            let _ = std::fs::create_dir_all(dir);
            let _ = shell::open(&dir.display().to_string());
        }
        if let Some((t, at)) = &self.toast
            && at.elapsed().as_secs_f32() < 3.0
        {
            header.label(
                egui::RichText::new(t.as_str())
                    .font(Theme::font(SIZE_LABEL))
                    .color(theme.accent_fg()),
            );
        }
        let card = Rect::from_min_size(body.min, vec2(290.0, body.height()));
        self.recorder(ui, card, theme);
        let list = Rect::from_min_max(pos2(card.max.x + 16.0, body.min.y), body.max);
        self.list(ui, list, theme);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_clock() {
        assert_eq!(
            title(Path::new("C:\\m\\Memo 2026-10-20 14-32-05.wav")),
            "20 Oct, 14:32"
        );
        assert_eq!(title(Path::new("C:\\m\\Groceries.wav")), "Groceries");
        assert_eq!(clock(75.4), "1:15");
        assert_eq!(wav_secs(44 + 88_200), 1.0);
    }

    #[test]
    fn peaks_are_normalized() {
        let samples: Vec<i16> = (0..4800)
            .map(|i| if i < 2400 { 1000 } else { 4000 })
            .collect();
        let p = peaks(&samples);
        assert_eq!(p.len(), BARS);
        assert!((p[BARS - 1] - 1.0).abs() < 1e-6);
        assert!(p[0] < 1.0 && p[0] > 0.0);
        assert_eq!(peaks(&[]), vec![0.0; BARS]);
    }
}
