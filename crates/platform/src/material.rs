//! DWM window attributes for the overlay, and which backdrop is in use.
//!
//! The blur itself comes from [`crate::backdrop::Backdrop`]. The older
//! `SetWindowCompositionAttribute` acrylic accent was tried first and
//! dropped: on current Windows 11 builds it ignores the window region and
//! blurs the whole window rectangle (see docs/NOTES.md).

use crate::window::Hwnd;
use windows::Win32::Graphics::Dwm::{
    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND,
    DwmSetWindowAttribute,
};
use windows::core::BOOL;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Applied {
    /// Shape-clipped blurred backdrop; draw the glass translucent.
    Acrylic,
    /// No backdrop; draw the glass nearly opaque.
    Opaque,
}

impl Applied {
    pub fn translucent(self) -> bool {
        matches!(self, Applied::Acrylic)
    }
}

/// Square DWM corners (the region does the rounding) and dark-mode hint.
/// Both are cosmetic, so failures are only logged.
pub fn prepare(hwnd: Hwnd, dark: bool) {
    let pref = DWMWCP_DONOTROUND;
    // SAFETY: attribute and size match.
    if let Err(e) = unsafe {
        DwmSetWindowAttribute(
            hwnd.get(),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        )
    } {
        tracing::debug!("DWMWA_WINDOW_CORNER_PREFERENCE: {e}");
    }
    let dark = BOOL::from(dark);
    // SAFETY: attribute and size match.
    if let Err(e) = unsafe {
        DwmSetWindowAttribute(
            hwnd.get(),
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const _,
            std::mem::size_of::<BOOL>() as u32,
        )
    } {
        tracing::debug!("DWMWA_USE_IMMERSIVE_DARK_MODE: {e}");
    }
}
