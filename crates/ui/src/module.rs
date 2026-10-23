//! The `Module` trait every utility implements, and the registry the bento
//! layout is built from.
//!
//! Modules own their background work (threads, OS callbacks) and talk to the
//! UI through channels they drain in [`Module::on_tick`]; nothing a module
//! does from `ui_tile` may block. The trait is object-safe and keyed by a
//! stable string id, so the plugin host (`nook_modules::plugins`) registers
//! out-of-process plugins as `Box<dyn Module>`s without changes to the shell.

pub use crate::geometry::TileSize;
use crate::theme::Theme;
use std::path::PathBuf;
use std::time::Duration;

/// Live activity a module mirrors into the collapsed pill.
#[derive(Clone, Debug, PartialEq)]
pub enum Badge {
    /// Progress ring with a short label (e.g. "12:04").
    Ring { progress: f32, label: String },
    /// A small glyph (Phosphor codepoint), e.g. a coffee cup.
    Glyph { icon: &'static str },
    /// A pulsing dot: something arrived.
    Pulse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleStatus {
    Ready,
    /// Shown as a disabled tile with a "Coming soon" state.
    ComingSoon,
}

/// Per-frame context handed to `ui_tile`.
pub struct TileCtx<'a> {
    pub theme: &'a Theme,
    /// egui time in seconds.
    pub now: f64,
    /// Pointer is over this tile.
    pub hovered: bool,
    /// Files are being dragged over the notch.
    pub files_hovering: bool,
}

pub trait Module: Send {
    /// Stable identifier used in config (`"coffee"`, `"timer"`, ...).
    fn id(&self) -> &'static str;
    fn name(&self) -> &str;
    /// Phosphor glyph. Drawn "regular" when idle and "fill" when active.
    fn icon(&self) -> &'static str;
    fn size_hint(&self) -> TileSize;

    fn status(&self) -> ModuleStatus {
        ModuleStatus::Ready
    }
    /// Active tiles get the accent gradient and the filled icon.
    fn is_active(&self) -> bool {
        false
    }
    fn collapsed_badge(&self) -> Option<Badge> {
        None
    }
    /// Draws the tile body. The tile background is drawn by the shell; `ui`
    /// is already clipped and inset to the tile's content area.
    fn ui_tile(&mut self, ui: &mut egui::Ui, ctx: &mut TileCtx<'_>);
    /// Called once at start-up with the UI context, so background work can
    /// wake the UI (`ctx.request_repaint()`) when it has news.
    fn attach(&mut self, _ctx: &egui::Context) {}
    /// Called once per UI frame before drawing; drain channels here.
    fn on_tick(&mut self) {}
    /// How soon this module needs another frame while visible (e.g. a ticking
    /// timer). `None` means it is static and costs nothing when idle.
    fn next_repaint(&self, _tile_visible: bool) -> Option<Duration> {
        None
    }
    /// True while the tile uses the mouse wheel itself (e.g. setting the
    /// timer), so scrolling over it does not flip the grid's page.
    fn captures_wheel(&self) -> bool {
        false
    }
    /// Asks the shell to open this module's full-panel page. Checked after
    /// the tiles are drawn; return true once per request.
    fn take_page_request(&mut self) -> bool {
        false
    }
    /// Asks the app to open the panel (pinned) on this module's page, e.g.
    /// when results arrive from a hotkey. Return true once per request.
    fn take_show_request(&mut self) -> bool {
        false
    }
    /// Size the panel grows to while this module's page is open (points);
    /// `None` keeps the regular panel.
    fn page_size(&self) -> Option<egui::Vec2> {
        None
    }
    /// Draws the full-panel page. `header` sits right of the title (right to
    /// left) for page actions; `body` is the area below. Return true to go
    /// back to the grid.
    fn ui_page(
        &mut self,
        _ui: &mut egui::Ui,
        _header: &mut egui::Ui,
        _body: egui::Rect,
        _theme: &Theme,
    ) -> bool {
        true
    }
    /// Shows extra windows (e.g. the draw-on-screen overlay). Called every
    /// frame from the app, whatever the shell is doing.
    fn ui_overlay(&mut self, _ctx: &egui::Context, _theme: &Theme) {}
    /// True while an overlay is open: the app keeps its memory warm then.
    fn overlay_open(&self) -> bool {
        false
    }
    /// Accelerator in `global-hotkey` syntax, e.g. `"ctrl+alt+KeyC"`.
    fn default_hotkey(&self) -> Option<&'static str> {
        None
    }
    fn on_hotkey(&mut self) {}
    /// True if files dropped on the notch should go to this module. The
    /// first visible module that accepts them gets them.
    fn accepts_files(&self) -> bool {
        false
    }
    /// Files dropped on the notch.
    fn drop_files(&mut self, _paths: Vec<PathBuf>) {}
    /// Restore any system state the module changed. Called on exit.
    fn shutdown(&mut self) {}
    /// False for tiles that must stay in the grid (settings).
    fn can_hide(&self) -> bool {
        true
    }
}

/// A module as the grid draws it.
pub struct GridTile<'a> {
    pub module: &'a mut Box<dyn Module>,
    /// The size the user picked, else the module's own.
    pub size: TileSize,
    /// Hidden from the grid; only edit mode shows it (dimmed).
    pub hidden: bool,
}

/// Owns all modules; the shell iterates the enabled ones in configured order.
#[derive(Default)]
pub struct Registry {
    modules: Vec<Box<dyn Module>>,
    /// Indices into `modules`, in display order, enabled only.
    order: Vec<usize>,
    /// Indices into `modules`, in display order, hidden ones included.
    all_order: Vec<usize>,
    /// Tile sizes the user picked, by module id; others use `size_hint`.
    sizes: Vec<(String, TileSize)>,
}

impl Registry {
    pub fn register(&mut self, module: Box<dyn Module>) {
        self.order.push(self.modules.len());
        self.all_order.push(self.modules.len());
        self.modules.push(module);
    }

    /// Applies a configured order. Ids not listed keep their registration
    /// order after the listed ones; disabled ids are dropped.
    pub fn arrange(&mut self, order: &[String], disabled: &[String]) {
        let mut idx: Vec<usize> = order
            .iter()
            .filter_map(|id| self.modules.iter().position(|m| m.id() == id))
            .collect();
        for i in 0..self.modules.len() {
            if !idx.contains(&i) {
                idx.push(i);
            }
        }
        idx.dedup();
        self.all_order = idx.clone();
        idx.retain(|&i| !disabled.iter().any(|d| d == self.modules[i].id()));
        self.order = idx;
    }

    /// Replaces the user's tile sizes.
    pub fn set_sizes<'a>(&mut self, sizes: impl IntoIterator<Item = (&'a String, &'a TileSize)>) {
        self.sizes = sizes.into_iter().map(|(k, v)| (k.clone(), *v)).collect();
    }

    /// The size `module` is shown at.
    pub fn size_of(&self, module: &dyn Module) -> TileSize {
        self.sizes
            .iter()
            .find(|(id, _)| id == module.id())
            .map_or_else(|| module.size_hint(), |(_, s)| *s)
    }

    /// Tiles for the grid in display order: the enabled ones, or with
    /// `with_hidden` every module (for edit mode).
    pub fn grid_mut(&mut self, with_hidden: bool) -> Vec<GridTile<'_>> {
        let idx = if with_hidden {
            self.all_order.clone()
        } else {
            self.order.clone()
        };
        let meta: Vec<(TileSize, bool)> = idx
            .iter()
            .map(|&i| {
                let m = self.modules[i].as_ref();
                (self.size_of(m), !self.order.contains(&i))
            })
            .collect();
        let mut slots: Vec<Option<&mut Box<dyn Module>>> =
            self.modules.iter_mut().map(Some).collect();
        idx.iter()
            .zip(meta)
            .filter_map(|(&i, (size, hidden))| {
                slots
                    .get_mut(i)
                    .and_then(Option::take)
                    .map(|module| GridTile {
                        module,
                        size,
                        hidden,
                    })
            })
            .collect()
    }

    /// Enabled modules in display order.
    pub fn visible_mut(&mut self) -> Vec<&mut Box<dyn Module>> {
        let mut slots: Vec<Option<&mut Box<dyn Module>>> =
            self.modules.iter_mut().map(Some).collect();
        self.order
            .iter()
            .filter_map(|&i| slots.get_mut(i).and_then(Option::take))
            .collect()
    }

    pub fn visible(&self) -> impl Iterator<Item = &dyn Module> {
        self.order
            .iter()
            .filter_map(|&i| self.modules.get(i).map(|m| m.as_ref()))
    }

    pub fn all(&self) -> impl Iterator<Item = &dyn Module> {
        self.modules.iter().map(|m| m.as_ref())
    }

    pub fn all_mut(&mut self) -> impl Iterator<Item = &mut Box<dyn Module>> {
        self.modules.iter_mut()
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Box<dyn Module>> {
        self.modules.iter_mut().find(|m| m.id() == id)
    }

    /// Takes a module out (a plugin being uninstalled or replaced). The
    /// others keep their order.
    pub fn remove(&mut self, id: &str) -> Option<Box<dyn Module>> {
        let at = self.modules.iter().position(|m| m.id() == id)?;
        let fix = |idx: &mut Vec<usize>| {
            idx.retain(|&i| i != at);
            idx.iter_mut().filter(|i| **i > at).for_each(|i| *i -= 1);
        };
        fix(&mut self.order);
        fix(&mut self.all_order);
        Some(self.modules.remove(at))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dummy(&'static str);
    impl Module for Dummy {
        fn id(&self) -> &'static str {
            self.0
        }
        fn name(&self) -> &str {
            self.0
        }
        fn icon(&self) -> &'static str {
            ""
        }
        fn size_hint(&self) -> TileSize {
            TileSize::Small
        }
        fn ui_tile(&mut self, _: &mut egui::Ui, _: &mut TileCtx<'_>) {}
    }

    fn reg() -> Registry {
        let mut r = Registry::default();
        for id in ["a", "b", "c", "d"] {
            r.register(Box::new(Dummy(id)));
        }
        r
    }

    #[test]
    fn arrange_orders_and_filters() {
        let mut r = reg();
        r.arrange(&["c".into(), "a".into(), "zzz".into()], &["b".into()]);
        let ids: Vec<_> = r.visible().map(|m| m.id()).collect();
        assert_eq!(ids, ["c", "a", "d"]);
        let ids: Vec<_> = r.visible_mut().into_iter().map(|m| m.id()).collect();
        assert_eq!(ids, ["c", "a", "d"]);
    }

    #[test]
    fn remove_keeps_the_rest_in_order() {
        let mut r = reg();
        r.arrange(
            &["d".into(), "b".into(), "a".into(), "c".into()],
            &["a".into()],
        );
        assert!(r.remove("b").is_some());
        assert!(r.remove("zzz").is_none());
        let ids: Vec<_> = r.visible().map(|m| m.id()).collect();
        assert_eq!(ids, ["d", "c"]);
        let all: Vec<_> = r.grid_mut(true).iter().map(|t| t.module.id()).collect();
        assert_eq!(all, ["d", "a", "c"]);
        r.register(Box::new(Dummy("e")));
        let ids: Vec<_> = r.visible().map(|m| m.id()).collect();
        assert_eq!(ids, ["d", "c", "e"]);
    }

    #[test]
    fn grid_tiles_carry_sizes_and_hidden_ones_on_request() {
        let mut r = reg();
        r.arrange(&["c".into(), "a".into()], &["b".into()]);
        let picked = [("c".to_owned(), TileSize::Large)];
        r.set_sizes(picked.iter().map(|(k, v)| (k, v)));
        let shown: Vec<_> = r
            .grid_mut(false)
            .iter()
            .map(|t| (t.module.id(), t.size, t.hidden))
            .collect();
        assert_eq!(
            shown,
            [
                ("c", TileSize::Large, false),
                ("a", TileSize::Small, false),
                ("d", TileSize::Small, false)
            ]
        );
        let all: Vec<_> = r
            .grid_mut(true)
            .iter()
            .map(|t| (t.module.id(), t.hidden))
            .collect();
        assert_eq!(all, [("c", false), ("a", false), ("b", true), ("d", false)]);
    }
}
