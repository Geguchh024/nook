//! Placeholder module: final id, name, icon and tile size, shown as a
//! disabled "Coming soon" tile.

use nook_ui::module::{Module, ModuleStatus, TileCtx, TileSize};

pub struct Stub {
    id: &'static str,
    name: &'static str,
    icon: &'static str,
    size: TileSize,
}

impl Stub {
    pub fn new(id: &'static str, name: &'static str, icon: &'static str, size: TileSize) -> Self {
        Self {
            id,
            name,
            icon,
            size,
        }
    }
}

impl Module for Stub {
    fn id(&self) -> &'static str {
        self.id
    }

    fn name(&self) -> &str {
        self.name
    }

    fn icon(&self) -> &'static str {
        self.icon
    }

    fn size_hint(&self) -> TileSize {
        self.size
    }

    fn status(&self) -> ModuleStatus {
        ModuleStatus::ComingSoon
    }

    fn ui_tile(&mut self, _ui: &mut egui::Ui, _ctx: &mut TileCtx<'_>) {}
}
