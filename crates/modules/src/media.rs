//! Media: now playing from any app that reports to Windows (Spotify,
//! browsers, the Media Player...) through SMTC, with album art, progress and
//! previous / play-pause / next. A note glyph shows in the pill while
//! something plays.
//!
//! The SMTC work runs on `nook_platform::media`'s worker thread; this module
//! only drains its snapshots in `on_tick` and sends commands back.

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{
    Align2, Color32, ColorImage, Pos2, Rect, Response, TextureHandle, TextureOptions, Ui,
    UiBuilder, pos2, vec2,
};
use nook_platform::media::{MediaCommand, MediaWatcher, NowPlaying};
use nook_ui::module::{Badge, BadgeArtwork, Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme, with_alpha};
use nook_ui::{icons, paint, widgets};
use std::sync::Arc;
use std::time::Duration;

const ART: f32 = 44.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MediaTileMode {
    Small,
    Medium,
    Large,
}

fn tile_mode(width: f32, height: f32) -> MediaTileMode {
    if width < 90.0 && height < 90.0 {
        MediaTileMode::Small
    } else if width < 90.0 || height < 90.0 {
        MediaTileMode::Medium
    } else {
        MediaTileMode::Large
    }
}

#[derive(Default)]
pub struct Media {
    watcher: Option<MediaWatcher>,
    now: Option<NowPlaying>,
    /// Texture for `now.art`, keyed by the art's allocation.
    art: Option<(usize, TextureHandle)>,
    badge_art: Option<(usize, BadgeArtwork)>,
    page_request: bool,
    seek_preview: Option<f64>,
}

impl Media {
    fn send(&self, cmd: MediaCommand) {
        if let Some(w) = &self.watcher {
            w.send(cmd);
        }
    }

    fn art_texture(&mut self, ctx: &egui::Context) -> Option<egui::TextureId> {
        let art = self.now.as_ref()?.art.as_ref()?;
        let key = Arc::as_ptr(art) as usize;
        if self.art.as_ref().map(|(k, _)| *k) != Some(key) {
            let image =
                ColorImage::from_rgba_unmultiplied([art.width, art.height], art.rgba.as_slice());
            let tex = ctx.load_texture("nook-media-art", image, TextureOptions::LINEAR);
            self.art = Some((key, tex));
        }
        self.art.as_ref().map(|(_, t)| t.id())
    }
}

/// Single-line text, elided with "…" to `width`.
fn text_line(ui: &Ui, pos: Pos2, text: &str, width: f32, font: egui::FontId, color: Color32) {
    let mut job = LayoutJob::single_section(text.to_owned(), TextFormat::simple(font, color));
    job.wrap = TextWrapping {
        max_width: width,
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(pos, galley, color);
}

fn button_at(
    ui: &mut Ui,
    center: Pos2,
    size: f32,
    glyph: &'static str,
    active: bool,
    theme: &Theme,
) -> Response {
    let rect = Rect::from_center_size(center, vec2(size, size));
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect).id_salt(glyph));
    widgets::icon_button(&mut child, glyph, size, active, theme)
}

fn mmss(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Rounded image: a squircle-ish clip is overkill at 44 px, a rounded rect
/// mesh reads the same.
fn rounded_image(ui: &Ui, rect: Rect, tex: egui::TextureId, radius: f32) {
    ui.painter().add(
        egui::epaint::RectShape::filled(rect, radius, Color32::WHITE)
            .with_texture(tex, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0))),
    );
}

impl Module for Media {
    fn id(&self) -> &'static str {
        "media"
    }

    fn name(&self) -> &str {
        "Media"
    }

    fn icon(&self) -> &'static str {
        icons::MUSIC_NOTES
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn is_active(&self) -> bool {
        self.now.as_ref().is_some_and(|n| n.playing)
    }

    fn collapsed_badge(&self) -> Option<Badge> {
        self.now
            .as_ref()
            .is_some_and(|now| now.playing)
            .then_some(Badge::Media {
                icon: icons::MUSIC_NOTES,
                artwork: self.badge_art.as_ref().map(|(_, art)| art.clone()),
            })
    }

    fn attach(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        match MediaWatcher::spawn(move || ctx.request_repaint()) {
            Ok(w) => self.watcher = Some(w),
            Err(e) => tracing::warn!("media: {e}"),
        }
    }

    fn on_tick(&mut self) {
        if let Some(snapshot) = self.watcher.as_ref().and_then(MediaWatcher::poll) {
            self.now = snapshot;
            self.seek_preview = None;
            let art = self.now.as_ref().and_then(|now| now.art.as_ref());
            let key = art.map(|art| Arc::as_ptr(art) as usize);
            if self.badge_art.as_ref().map(|(key, _)| *key) != key {
                self.badge_art = art.map(|art| {
                    (
                        Arc::as_ptr(art) as usize,
                        BadgeArtwork {
                            width: art.width,
                            height: art.height,
                            rgba: Arc::from(art.rgba.clone()),
                        },
                    )
                });
            }
            if self.now.as_ref().and_then(|n| n.art.as_ref()).is_none() {
                self.art = None;
            }
        }
    }

    fn next_repaint(&self, tile_visible: bool) -> Option<Duration> {
        // The progress bar and time advance once a second while visible.
        let n = self.now.as_ref()?;
        (tile_visible && n.playing && n.timeline.is_some()).then(|| Duration::from_secs(1))
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let theme = ctx.theme;
        let rect = ui.max_rect();
        let mode = tile_mode(rect.width(), rect.height());
        let compact = mode == MediaTileMode::Small;
        let medium = mode == MediaTileMode::Medium;

        let Some(now) = self.now.clone() else {
            let response = ui.interact(rect, ui.id().with("media-page"), egui::Sense::click());
            if response.clicked() {
                self.page_request = true;
            }
            let c = rect.center();
            paint::icon(
                ui.painter(),
                if compact { c } else { c - vec2(0.0, 8.0) },
                icons::MUSIC_NOTES,
                if compact { 25.0 } else { 24.0 },
                theme.text_faint,
                false,
            );
            if !compact {
                ui.painter().text(
                    c + vec2(0.0, 18.0),
                    Align2::CENTER_CENTER,
                    if medium { "Media" } else { "Nothing playing" },
                    Theme::font(SIZE_LABEL),
                    theme.text_dim,
                );
            }
            return;
        };

        if compact {
            let response = ui.interact(rect, ui.id().with("media-page"), egui::Sense::click());
            if response.clicked() {
                self.page_request = true;
            }
            match self.art_texture(ui.ctx()) {
                Some(tex) => rounded_image(ui, rect.shrink(2.0), tex, 11.0),
                None => {
                    paint::icon(
                        ui.painter(),
                        rect.center(),
                        icons::MUSIC_NOTES,
                        27.0,
                        theme.accent,
                        now.playing,
                    );
                }
            }
            return;
        }

        if medium {
            let vertical = rect.width() < 110.0;
            let art_size = if vertical {
                rect.width().min(44.0).min(rect.height() * 0.38).max(28.0)
            } else {
                rect.height().min(48.0).min(rect.width() * 0.33).max(32.0)
            };
            let art_origin = if vertical {
                pos2(rect.center().x - art_size * 0.5, rect.min.y)
            } else {
                rect.min
            };
            let art_rect = Rect::from_min_size(art_origin, vec2(art_size, art_size));
            match self.art_texture(ui.ctx()) {
                Some(tex) => rounded_image(ui, art_rect, tex, 9.0),
                None => {
                    widgets::card(ui.painter(), art_rect, theme);
                    paint::icon(
                        ui.painter(),
                        art_rect.center(),
                        icons::MUSIC_NOTES,
                        18.0,
                        theme.text_faint,
                        false,
                    );
                }
            }

            let controls_y = rect.max.y - 14.0;
            let control_center = if vertical {
                rect.center().x
            } else {
                rect.max.x - 45.0
            };
            let control_gap = if vertical { 19.0 } else { 23.0 };
            let control_size = if vertical { 16.0 } else { 19.0 };
            let play_icon = if now.playing {
                icons::PAUSE
            } else {
                icons::PLAY
            };
            let hit_top = if vertical {
                Rect::from_min_max(
                    pos2(rect.min.x, art_rect.max.y + 2.0),
                    pos2(rect.max.x, controls_y - 15.0),
                )
            } else {
                Rect::from_min_max(
                    pos2(art_rect.max.x + 5.0, rect.min.y),
                    pos2(rect.max.x, controls_y - 13.0),
                )
            };
            if ui
                .interact(
                    hit_top,
                    ui.id().with("media-page-info"),
                    egui::Sense::click(),
                )
                .clicked()
                || ui
                    .interact(
                        if vertical {
                            art_rect
                        } else {
                            Rect::from_min_max(rect.min, pos2(art_rect.max.x, rect.max.y))
                        },
                        ui.id().with("media-page-art"),
                        egui::Sense::click(),
                    )
                    .clicked()
            {
                self.page_request = true;
            }
            if vertical {
                text_line(
                    ui,
                    pos2(rect.min.x + 3.0, art_rect.max.y + 7.0),
                    if now.title.is_empty() {
                        &now.source
                    } else {
                        &now.title
                    },
                    rect.width() - 6.0,
                    Theme::font(SIZE_LABEL - 1.0),
                    theme.text,
                );
            } else {
                text_line(
                    ui,
                    pos2(art_rect.max.x + 6.0, rect.min.y + 7.0),
                    if now.title.is_empty() {
                        &now.source
                    } else {
                        &now.title
                    },
                    (rect.max.x - art_rect.max.x - 8.0).max(8.0),
                    Theme::font(SIZE_LABEL),
                    theme.text,
                );
            }
            if now.can_previous
                && button_at(
                    ui,
                    pos2(control_center - control_gap, controls_y),
                    control_size,
                    icons::SKIP_BACK,
                    false,
                    theme,
                )
                .on_hover_text("Previous")
                .clicked()
            {
                self.send(MediaCommand::Previous);
            }
            if now.can_toggle
                && button_at(
                    ui,
                    pos2(control_center, controls_y),
                    control_size + 2.0,
                    play_icon,
                    true,
                    theme,
                )
                .on_hover_text(if now.playing { "Pause" } else { "Play" })
                .clicked()
            {
                self.send(MediaCommand::PlayPause);
                if let Some(n) = &mut self.now {
                    if let Some((pos, dur)) = n.position() {
                        n.timeline = Some((pos, dur));
                        n.captured = std::time::Instant::now();
                    }
                    n.playing = !n.playing;
                }
            }
            if now.can_next
                && button_at(
                    ui,
                    pos2(control_center + control_gap, controls_y),
                    control_size,
                    icons::SKIP_FORWARD,
                    false,
                    theme,
                )
                .on_hover_text("Next")
                .clicked()
            {
                self.send(MediaCommand::Next);
            }
            return;
        }

        let page_hit = Rect::from_min_max(rect.min, pos2(rect.max.x, rect.max.y - 36.0));
        if ui
            .interact(page_hit, ui.id().with("media-page"), egui::Sense::click())
            .clicked()
        {
            self.page_request = true;
        }

        // Album art, or a placeholder.
        let art_rect = Rect::from_min_size(rect.min, vec2(ART, ART));
        match self.art_texture(ui.ctx()) {
            Some(tex) => rounded_image(ui, art_rect, tex, 10.0),
            None => {
                widgets::card(ui.painter(), art_rect, theme);
                paint::icon(
                    ui.painter(),
                    art_rect.center(),
                    icons::MUSIC_NOTES,
                    20.0,
                    theme.text_faint,
                    false,
                );
            }
        }

        // Right of the art: source app, time, progress bar.
        let col = rect.min.x + ART + 8.0;
        let col_w = rect.max.x - col;
        text_line(
            ui,
            pos2(col, rect.min.y + 2.0),
            &now.source,
            col_w,
            Theme::font(SIZE_LABEL - 1.0),
            theme.text_faint,
        );
        if let Some((pos, dur)) = now.position() {
            ui.painter().text(
                pos2(col, rect.min.y + 24.0),
                Align2::LEFT_CENTER,
                format!("{} / {}", mmss(pos), mmss(dur)),
                Theme::font(SIZE_LABEL - 1.0),
                theme.text_dim,
            );
            let bar = Rect::from_min_size(pos2(col, rect.min.y + 36.0), vec2(col_w, 3.0));
            ui.painter()
                .rect_filled(bar, 1.5, with_alpha(theme.text_faint, 0.3));
            let frac = (pos / dur.max(1.0)).clamp(0.0, 1.0) as f32;
            let mut fill = bar;
            fill.set_width(bar.width() * frac);
            ui.painter().rect_filled(fill, 1.5, theme.accent_fg());
        } else {
            ui.painter().text(
                pos2(col, rect.min.y + 24.0),
                Align2::LEFT_CENTER,
                if now.playing { "Playing" } else { "Paused" },
                Theme::font(SIZE_LABEL - 1.0),
                theme.text_dim,
            );
        }

        // Title and artist.
        let title = if now.title.is_empty() {
            "Unknown title"
        } else {
            &now.title
        };
        text_line(
            ui,
            pos2(rect.min.x, rect.min.y + ART + 6.0),
            title,
            rect.width(),
            Theme::font_semibold(SIZE_LABEL + 1.0),
            theme.text,
        );
        text_line(
            ui,
            pos2(rect.min.x, rect.min.y + ART + 21.0),
            &now.artist,
            rect.width(),
            Theme::font(SIZE_LABEL),
            theme.text_dim,
        );

        // Transport.
        let y = rect.max.y - 11.0;
        let cx = rect.center().x;
        let gap = 34.0;
        if now.can_previous
            && button_at(ui, pos2(cx - gap, y), 24.0, icons::SKIP_BACK, false, theme)
                .on_hover_text("Previous")
                .clicked()
        {
            self.send(MediaCommand::Previous);
        }
        let (glyph, tip) = if now.playing {
            (icons::PAUSE, "Pause")
        } else {
            (icons::PLAY, "Play")
        };
        if now.can_toggle
            && button_at(ui, pos2(cx, y), 28.0, glyph, true, theme)
                .on_hover_text(tip)
                .clicked()
        {
            self.send(MediaCommand::PlayPause);
            // Flip right away; the next snapshot confirms it.
            if let Some(n) = &mut self.now {
                if let Some((pos, dur)) = n.position() {
                    n.timeline = Some((pos, dur));
                    n.captured = std::time::Instant::now();
                }
                n.playing = !n.playing;
            }
        }
        if now.can_next
            && button_at(
                ui,
                pos2(cx + gap, y),
                24.0,
                icons::SKIP_FORWARD,
                false,
                theme,
            )
            .on_hover_text("Next")
            .clicked()
        {
            self.send(MediaCommand::Next);
        }
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.page_request)
    }

    fn ui_page(&mut self, ui: &mut Ui, _header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        let Some(now) = self.now.clone() else {
            ui.painter().text(
                body.center(),
                Align2::CENTER_CENTER,
                "Nothing is playing",
                Theme::font(SIZE_BODY),
                theme.text_dim,
            );
            return false;
        };

        let art_size = body.height().min(76.0).min(body.width() * 0.22).max(40.0);
        let art_rect = Rect::from_min_size(body.min, vec2(art_size, art_size));
        match self.art_texture(ui.ctx()) {
            Some(texture) => rounded_image(ui, art_rect, texture, 12.0),
            None => {
                widgets::card(ui.painter(), art_rect, theme);
                paint::icon(
                    ui.painter(),
                    art_rect.center(),
                    icons::MUSIC_NOTES,
                    art_size * 0.42,
                    theme.text_faint,
                    false,
                );
            }
        }

        let info_x = art_rect.max.x + 12.0;
        let info_width = (body.max.x - info_x).max(1.0);
        text_line(
            ui,
            pos2(info_x, body.min.y + 2.0),
            &now.source,
            info_width,
            Theme::font(SIZE_LABEL - 1.0),
            theme.text_faint,
        );
        text_line(
            ui,
            pos2(info_x, body.min.y + 26.0),
            if now.title.is_empty() {
                "Unknown title"
            } else {
                &now.title
            },
            info_width,
            Theme::font_semibold(SIZE_BODY),
            theme.text,
        );
        text_line(
            ui,
            pos2(info_x, body.min.y + 49.0),
            &now.artist,
            info_width,
            Theme::font(SIZE_LABEL),
            theme.text_dim,
        );

        if let Some((position, duration)) = now.position() {
            let mut fraction = (self.seek_preview.unwrap_or(position) / duration.max(1.0)) as f32;
            let bar_rect = Rect::from_min_size(
                pos2(body.min.x, body.min.y + art_size + 12.0),
                vec2(body.width(), 22.0),
            );
            let response = ui
                .scope_builder(egui::UiBuilder::new().max_rect(bar_rect), |ui| {
                    widgets::slider(ui, &mut fraction, bar_rect.width(), 0, theme)
                })
                .inner;
            if response.changed() {
                nook_ui::sound::scrub(ui.ctx(), response.id);
                self.seek_preview = Some(fraction as f64 * duration);
            }
            if response.drag_stopped() || response.clicked() {
                let ticks = (fraction as f64 * duration * 10_000_000.0).max(0.0) as u64;
                self.send(MediaCommand::Seek(ticks));
            }
            let current = self.seek_preview.unwrap_or(position);
            ui.painter().text(
                pos2(body.min.x, bar_rect.max.y + 5.0),
                egui::Align2::LEFT_TOP,
                mmss(current),
                Theme::font(SIZE_LABEL - 1.0),
                theme.text_dim,
            );
            ui.painter().text(
                pos2(body.max.x, bar_rect.max.y + 5.0),
                egui::Align2::RIGHT_TOP,
                mmss(duration),
                Theme::font(SIZE_LABEL - 1.0),
                theme.text_dim,
            );
        }

        let center = pos2(body.center().x, body.max.y - 13.0);
        let gap = 38.0;
        if now.can_previous
            && button_at(
                ui,
                center - vec2(gap, 0.0),
                28.0,
                icons::SKIP_BACK,
                false,
                theme,
            )
            .on_hover_text("Previous")
            .clicked()
        {
            nook_ui::sound::play(ui.ctx(), nook_ui::sound::Cue::Media);
            self.send(MediaCommand::Previous);
        }
        if now.can_toggle {
            let (glyph, tip) = if now.playing {
                (icons::PAUSE, "Pause")
            } else {
                (icons::PLAY, "Play")
            };
            if button_at(ui, center, 32.0, glyph, true, theme)
                .on_hover_text(tip)
                .clicked()
            {
                nook_ui::sound::play(ui.ctx(), nook_ui::sound::Cue::Media);
                self.send(MediaCommand::PlayPause);
            }
        }
        if now.can_next
            && button_at(
                ui,
                center + vec2(gap, 0.0),
                28.0,
                icons::SKIP_FORWARD,
                false,
                theme,
            )
            .on_hover_text("Next")
            .clicked()
        {
            nook_ui::sound::play(ui.ctx(), nook_ui::sound::Cue::Media);
            self.send(MediaCommand::Next);
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{MediaTileMode, tile_mode};

    #[test]
    fn media_tile_mode_follows_each_layout_footprint() {
        assert_eq!(tile_mode(52.0, 52.0), MediaTileMode::Small);
        assert_eq!(tile_mode(116.0, 52.0), MediaTileMode::Medium);
        assert_eq!(tile_mode(52.0, 116.0), MediaTileMode::Medium);
        assert_eq!(tile_mode(116.0, 116.0), MediaTileMode::Large);
    }
}
