//! Windows AppBar registration for taskbar-style overlays.

use crate::monitor::Rect;
use crate::window::Hwnd;
use crate::{PlatformError, Result};
use windows::Win32::Foundation::{LPARAM, RECT};
use windows::Win32::UI::Shell::{
    ABE_BOTTOM, ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, APPBARDATA, SHAppBarMessage,
};

/// A registered edge reservation. Dropping it releases the work-area space.
pub struct AppBar {
    data: APPBARDATA,
    rect: Rect,
}

impl AppBar {
    /// Registers `hwnd` as a top or bottom AppBar on the selected monitor.
    pub fn reserve(hwnd: Hwnd, monitor: Rect, top: bool, height: i32) -> Result<Self> {
        let edge = if top { ABE_TOP } else { ABE_BOTTOM };
        let mut data = APPBARDATA {
            cbSize: std::mem::size_of::<APPBARDATA>() as u32,
            hWnd: hwnd.get(),
            uCallbackMessage: 0x8000 + 0x4e,
            uEdge: edge,
            rc: RECT {
                left: monitor.left,
                top: monitor.top,
                right: monitor.right,
                bottom: monitor.bottom,
            },
            lParam: LPARAM(0),
        };

        // SAFETY: `data` contains the required size and a live Nook window;
        // the returned reservation is removed by `Drop`.
        if unsafe { SHAppBarMessage(ABM_NEW, &mut data) } == 0 {
            return Err(PlatformError::Other(
                "SHAppBarMessage(ABM_NEW) rejected the taskbar".into(),
            ));
        }

        // SAFETY: this AppBar was registered above and `data` remains valid.
        unsafe { SHAppBarMessage(ABM_QUERYPOS, &mut data) };
        if top {
            data.rc.bottom = data.rc.top.saturating_add(height);
        } else {
            data.rc.top = data.rc.bottom.saturating_sub(height);
        }
        // SAFETY: this AppBar was registered above and `data` remains valid.
        unsafe { SHAppBarMessage(ABM_SETPOS, &mut data) };

        let rect = Rect {
            left: data.rc.left,
            top: data.rc.top,
            right: data.rc.right,
            bottom: data.rc.bottom,
        };
        if rect.width() <= 0 || rect.height() <= 0 {
            // SAFETY: release the registration before returning an error.
            unsafe { SHAppBarMessage(ABM_REMOVE, &mut data) };
            return Err(PlatformError::Other(
                "SHAppBarMessage returned an empty taskbar rectangle".into(),
            ));
        }
        Ok(Self { data, rect })
    }

    /// Assigned screen rectangle, in physical pixels.
    pub fn rect(&self) -> Rect {
        self.rect
    }
}

impl Drop for AppBar {
    fn drop(&mut self) {
        // SAFETY: `data` describes the live registration owned by this value.
        unsafe { SHAppBarMessage(ABM_REMOVE, &mut self.data) };
    }
}
