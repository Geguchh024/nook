//! The Plugins tile: confirms installs that come from the website and lists
//! installed plugins. A link opens the notch on this page; nothing is
//! downloaded or run until the person clicks Install.

use super::icons;
use super::install::{self, Request, State};
use super::manifest::{self, Found};
use crate::kit;
use egui::{Align2, Color32, FontId, Rect, Sense, Ui, pos2, vec2};
use nook_ui::module::{Badge, Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme, with_alpha};
use nook_ui::{icons as ph, paint, widgets};
use std::time::{Duration, Instant};

const CARD_H: f32 = 124.0;
const ROW_H: f32 = 44.0;

#[derive(Default)]
pub struct PluginsTile {
    page_request: bool,
    show_request: bool,
    installed: Vec<Found>,
    scanned: Option<Instant>,
    /// A Remove button clicked once, waiting for the second click.
    armed: Option<(String, Instant)>,
}

impl PluginsTile {
    fn rescan(&mut self) {
        if self
            .scanned
            .is_none_or(|t| t.elapsed() > Duration::from_secs(2))
        {
            self.installed = manifest::discover(&manifest::search_dirs());
            self.scanned = Some(Instant::now());
        }
    }
}

fn marketplace() -> Option<String> {
    std::env::var("NOOK_MARKETPLACE_URL")
        .ok()
        .filter(|u| u.starts_with("https://") || u.starts_with("http://localhost"))
}

impl Module for PluginsTile {
    fn id(&self) -> &'static str {
        super::PLUGINS_ID
    }

    fn name(&self) -> &str {
        "Plugins"
    }

    fn icon(&self) -> &'static str {
        ph::PUZZLE_PIECE
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn attach(&mut self, ctx: &egui::Context) {
        install::start(ctx);
    }

    fn on_tick(&mut self) {
        if install::take_arrived() {
            self.show_request = true;
            self.scanned = None;
        }
    }

    fn is_active(&self) -> bool {
        install::requests()
            .iter()
            .any(|r| matches!(r.state, State::Ready { .. } | State::Installing))
    }

    fn collapsed_badge(&self) -> Option<Badge> {
        install::requests()
            .iter()
            .any(|r| matches!(r.state, State::Ready { .. }))
            .then_some(Badge::Pulse)
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.page_request)
    }

    fn take_show_request(&mut self) -> bool {
        std::mem::take(&mut self.show_request)
    }

    fn page_size(&self) -> Option<egui::Vec2> {
        Some(vec2(620.0, 400.0))
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let active = self.is_active();
        if kit::tool_tile(ui, ctx, ph::PUZZLE_PIECE, active).clicked() {
            self.page_request = true;
        }
    }

    fn ui_page(&mut self, ui: &mut Ui, header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        self.rescan();
        if widgets::icon_button(header, ph::FOLDER_OPEN, nook_ui::page::HEADER, false, theme)
            .on_hover_text("Open the plugins folder")
            .clicked()
        {
            let _ = nook_platform::shell::open(&install::plugins_root().display().to_string());
        }
        if let Some(url) = marketplace()
            && widgets::icon_button(header, ph::STOREFRONT, nook_ui::page::HEADER, false, theme)
                .on_hover_text("Browse the plugin marketplace")
                .clicked()
        {
            let _ = nook_platform::shell::open(&url);
        }

        let requests = install::requests();
        let w = body.width();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body));
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(&mut child, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                for r in &requests {
                    let (rect, _) = ui.allocate_exact_size(vec2(w - 8.0, CARD_H), Sense::hover());
                    request_card(ui, rect, r, theme);
                }
                if !requests.is_empty() {
                    ui.add_space(6.0);
                }
                let (head, _) = ui.allocate_exact_size(vec2(w - 8.0, 18.0), Sense::hover());
                kit::section(
                    ui,
                    head.min,
                    &format!("INSTALLED · {}", self.installed.len()),
                    theme,
                );
                if self.installed.is_empty() {
                    let (rect, _) = ui.allocate_exact_size(vec2(w - 8.0, 120.0), Sense::hover());
                    kit::empty(
                        ui,
                        rect.center(),
                        ph::PUZZLE_PIECE,
                        "No plugins yet. Install one from the marketplace, or drop a .zip here.",
                        theme,
                    );
                }
                let mut remove = None;
                for f in &self.installed {
                    let (rect, _) = ui.allocate_exact_size(vec2(w - 8.0, ROW_H), Sense::hover());
                    if installed_row(ui, rect, f, theme, &self.armed) {
                        remove = Some(f.manifest.id.clone());
                    }
                }
                if let Some(id) = remove {
                    match &self.armed {
                        Some((a, t)) if *a == id && t.elapsed() < Duration::from_secs(3) => {
                            install::uninstall(&id);
                            self.armed = None;
                            self.scanned = None;
                        }
                        _ => self.armed = Some((id, Instant::now())),
                    }
                }
            });
        // While files are dragged over, the whole page is the drop zone:
        // everything dropped here goes to the installer (see the app's
        // drop routing), never to the file shelf.
        if ui.input(|i| !i.raw.hovered_files.is_empty()) {
            drop_zone(ui, body, theme);
        }
        false
    }
}

/// Covers the page while files are dragged over it.
fn drop_zone(ui: &Ui, body: Rect, theme: &Theme) {
    let rect = body.shrink(2.0);
    let painter = ui.painter();
    painter.rect_filled(rect, 14.0, with_alpha(theme.glass_bottom.to_opaque(), 0.96));
    painter.rect_filled(rect, 14.0, with_alpha(theme.accent, 0.10));
    painter.rect_stroke(
        rect,
        14.0,
        egui::Stroke::new(1.5, with_alpha(theme.accent, 0.7)),
        egui::StrokeKind::Inside,
    );
    let c = rect.center();
    painter.text(
        c - vec2(0.0, 26.0),
        Align2::CENTER_CENTER,
        ph::ARCHIVE_BOX,
        FontId::proportional(34.0),
        theme.accent_fg(),
    );
    painter.text(
        c + vec2(0.0, 14.0),
        Align2::CENTER_CENTER,
        "Drop a plugin .zip or .7z to install",
        Theme::font_semibold(SIZE_BODY + 2.0),
        theme.text,
    );
    painter.text(
        c + vec2(0.0, 36.0),
        Align2::CENTER_CENTER,
        "One plugin or a bundle of several",
        Theme::font(SIZE_LABEL),
        theme.text_dim,
    );
}

fn request_card(ui: &mut Ui, rect: Rect, r: &Request, theme: &Theme) {
    widgets::card(ui.painter(), rect, theme);
    let inner = rect.shrink(14.0);
    let (icon, name, version) = match &r.state {
        State::Ready { manifest, .. } => (
            icons::glyph(&manifest.icon),
            manifest.name.clone(),
            manifest.version.clone(),
        ),
        State::Done { name } => (ph::CHECK_CIRCLE, name.clone(), String::new()),
        State::Failed(_) => (
            ph::WARNING_CIRCLE,
            "Couldn't install".to_owned(),
            String::new(),
        ),
        _ => (
            ph::PUZZLE_PIECE,
            r.expect.clone().unwrap_or_else(|| "Plugin".into()),
            String::new(),
        ),
    };
    // Icon well
    let well = Rect::from_min_size(inner.min, vec2(44.0, 44.0));
    ui.painter().rect_filled(well, 12.0, kit::wash(theme));
    let icon_color = match r.state {
        State::Failed(_) => Color32::from_rgb(0xF0, 0x5A, 0x5A),
        State::Done { .. } => Color32::from_rgb(0x3D, 0xC9, 0x7A),
        _ => theme.text,
    };
    paint::icon(ui.painter(), well.center(), icon, 22.0, icon_color, false);

    let x = well.max.x + 12.0;
    let buttons_w = 196.0;
    let text_w = inner.max.x - x - buttons_w - 8.0;
    let title = if version.is_empty() {
        name
    } else {
        format!("{name}  v{version}")
    };
    kit::one_line(
        ui,
        pos2(x, inner.min.y - 1.0),
        &title,
        text_w,
        Theme::font_semibold(SIZE_BODY + 1.0),
        theme.text,
    );
    kit::one_line(
        ui,
        pos2(x, inner.min.y + 19.0),
        &r.source.describe(),
        text_w,
        Theme::font(SIZE_LABEL),
        theme.text_dim,
    );

    let line3 = pos2(x, inner.min.y + 38.0);
    let line4 = pos2(x, inner.min.y + 56.0);
    match &r.state {
        State::Loading => {
            kit::one_line(
                ui,
                line3,
                "Reading plugin.toml…",
                text_w,
                Theme::font(SIZE_LABEL),
                theme.text_dim,
            );
        }
        State::Ready {
            manifest,
            installed,
        } => {
            let desc = if manifest.description.is_empty() {
                "No description."
            } else {
                &manifest.description
            };
            kit::one_line(
                ui,
                line3,
                desc,
                text_w,
                Theme::font(SIZE_LABEL),
                theme.text_dim,
            );
            kit::one_line(
                ui,
                line4,
                &format!("Runs: {}", manifest.run.join(" ")),
                text_w,
                FontId::monospace(SIZE_LABEL - 0.5),
                theme.text,
            );
            if let Some(v) = installed {
                kit::one_line(
                    ui,
                    pos2(x, inner.min.y + 74.0),
                    &format!("Replaces the version you have ({v}). Its saved data is kept."),
                    text_w,
                    Theme::font(SIZE_LABEL - 1.0),
                    theme.accent_fg(),
                );
            }
        }
        State::Installing => {
            kit::one_line(
                ui,
                line3,
                "Downloading and checking…",
                text_w,
                Theme::font(SIZE_LABEL),
                theme.text_dim,
            );
            let bar = Rect::from_min_size(line4 + vec2(0.0, 4.0), vec2(text_w.min(220.0), 4.0));
            let t = (ui.input(|i| i.time) as f32 * 0.8).fract();
            kit::meter(ui.painter(), bar, t, theme.accent, theme);
            ui.ctx().request_repaint_after(Duration::from_millis(33));
        }
        State::Done { .. } => {
            kit::one_line(
                ui,
                line3,
                "Installed. Its tile is in the grid now, at the end.",
                text_w,
                Theme::font(SIZE_LABEL),
                theme.text_dim,
            );
        }
        State::Failed(e) => {
            let galley = ui.painter().layout(
                e.clone(),
                Theme::font(SIZE_LABEL),
                theme.text_dim,
                text_w + buttons_w - 90.0,
            );
            ui.painter().galley(line3, galley, theme.text_dim);
        }
    }

    // Safety line along the bottom of a pending card.
    if matches!(r.state, State::Ready { .. }) {
        ui.painter().text(
            pos2(inner.min.x, inner.max.y),
            Align2::LEFT_BOTTOM,
            "Plugins run with your rights, like any app. Install only code you trust.",
            Theme::font(SIZE_LABEL - 1.0),
            theme.text_faint,
        );
    }

    // Buttons, right-aligned on the first row.
    let bh = 30.0;
    let right = inner.max.x;
    let top = inner.min.y + 4.0;
    match &r.state {
        State::Ready { installed, .. } => {
            let go = Rect::from_min_size(pos2(right - 104.0, top), vec2(104.0, bh));
            let no = Rect::from_min_size(pos2(go.min.x - 84.0, top), vec2(78.0, bh));
            let label = if installed.is_some() {
                "Update"
            } else {
                "Install"
            };
            if kit::button(ui, go, label, Some(ph::DOWNLOAD_SIMPLE), true, theme).clicked() {
                install::confirm(r.id);
            }
            if kit::button(ui, no, "Cancel", None, false, theme).clicked() {
                install::dismiss(r.id);
            }
            if let Some(url) = r.source.web_url() {
                let code =
                    Rect::from_min_size(pos2(right - 104.0, top + bh + 8.0), vec2(104.0, bh));
                if kit::button(ui, code, "Read code", Some(ph::GITHUB_LOGO), false, theme).clicked()
                {
                    let _ = nook_platform::shell::open(&url);
                }
            }
        }
        State::Done { .. } | State::Failed(_) => {
            let ok = Rect::from_min_size(pos2(right - 84.0, top), vec2(84.0, bh));
            if kit::button(ui, ok, "Dismiss", None, false, theme).clicked() {
                install::dismiss(r.id);
            }
        }
        State::Loading | State::Installing => {}
    }
}

/// Draws one installed plugin; returns true when Remove was clicked.
fn installed_row(
    ui: &mut Ui,
    rect: Rect,
    f: &Found,
    theme: &Theme,
    armed: &Option<(String, Instant)>,
) -> bool {
    let id = ui.id().with(("installed", &f.manifest.id));
    kit::row(ui, rect, id, false, theme);
    paint::icon(
        ui.painter(),
        pos2(rect.min.x + 22.0, rect.center().y),
        icons::glyph(&f.manifest.icon),
        18.0,
        theme.text_dim,
        false,
    );
    let x = rect.min.x + 44.0;
    let w = rect.width() - 44.0 - 120.0;
    let title = if f.manifest.version.is_empty() {
        f.manifest.name.clone()
    } else {
        format!("{}  v{}", f.manifest.name, f.manifest.version)
    };
    kit::one_line(
        ui,
        pos2(x, rect.min.y + 5.0),
        &title,
        w,
        Theme::font(SIZE_BODY),
        theme.text,
    );
    let (line, color) = match install::remove_error(&f.manifest.id) {
        Some(e) => (
            format!("Couldn't remove: {e}"),
            Color32::from_rgb(0xF0, 0x5A, 0x5A),
        ),
        None => (f.dir.display().to_string(), theme.text_faint),
    };
    kit::one_line(
        ui,
        pos2(x, rect.min.y + 23.0),
        &line,
        w,
        Theme::font(SIZE_LABEL - 1.0),
        color,
    );
    if install::removable(&f.dir) {
        let b = Rect::from_min_size(
            pos2(rect.max.x - 112.0, rect.center().y - 14.0),
            vec2(104.0, 28.0),
        );
        let confirm = armed
            .as_ref()
            .is_some_and(|(a, t)| *a == f.manifest.id && t.elapsed() < Duration::from_secs(3));
        if confirm {
            ui.painter().rect_filled(
                b,
                14.0,
                with_alpha(Color32::from_rgb(0xF0, 0x5A, 0x5A), 0.18),
            );
            ui.ctx().request_repaint_after(Duration::from_millis(500));
        }
        return kit::button(
            ui,
            b,
            if confirm { "Sure?" } else { "Remove" },
            Some(ph::TRASH),
            false,
            theme,
        )
        .clicked();
    }
    false
}
