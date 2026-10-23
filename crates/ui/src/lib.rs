//! Nook UI: design tokens, the spring animator, shell geometry and rendering,
//! the bento grid, and the `Module` trait.

pub mod anim;
pub mod bento;
pub mod geometry;
pub mod module;
pub mod page;
pub mod paint;
pub mod shell;
pub mod shell_state;
pub mod spring;
pub mod theme;
pub mod widgets;

pub use egui_phosphor::{fill as icons_fill, regular as icons};
