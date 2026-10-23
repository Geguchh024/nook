//! Monitor lookup (physical pixels).

use crate::window::Hwnd;
use crate::{PlatformError, Result, Win32Context};
use windows::Win32::Foundation::{LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITOR_DEFAULTTONEAREST,
    MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint, MonitorFromWindow,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
use windows::core::BOOL;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    /// `self` fully covers `other`.
    pub fn covers(&self, other: &Rect) -> bool {
        self.left <= other.left
            && self.top <= other.top
            && self.right >= other.right
            && self.bottom >= other.bottom
    }
}

impl From<RECT> for Rect {
    fn from(r: RECT) -> Self {
        Self {
            left: r.left,
            top: r.top,
            right: r.right,
            bottom: r.bottom,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Monitor {
    /// Opaque id, stable while the monitor stays connected.
    pub id: isize,
    pub bounds: Rect,
    pub work: Rect,
    /// Effective DPI (96 = 100%).
    pub dpi: u32,
}

fn info(h: HMONITOR) -> Result<Monitor> {
    let mut mi = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: cbSize set, valid out pointer.
    if !unsafe { GetMonitorInfoW(h, &mut mi) }.as_bool() {
        return Err(PlatformError::Other("GetMonitorInfoW failed".into()));
    }
    let (mut dpi_x, mut dpi_y) = (0, 0);
    // SAFETY: valid out pointers; on failure the DPI falls back to 96.
    let dpi = match unsafe { GetDpiForMonitor(h, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) } {
        Ok(()) if dpi_x > 0 => dpi_x,
        _ => 96,
    };
    Ok(Monitor {
        id: h.0 as isize,
        bounds: mi.rcMonitor.into(),
        work: mi.rcWork.into(),
        dpi,
    })
}

/// Every connected monitor, primary first.
pub fn all() -> Vec<Monitor> {
    unsafe extern "system" fn collect(h: HMONITOR, _: HDC, _: *mut RECT, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `Vec` passed to `EnumDisplayMonitors`
        // below, alive for the whole enumeration.
        let list = unsafe { &mut *(lparam.0 as *mut Vec<HMONITOR>) };
        list.push(h);
        BOOL(1)
    }
    let mut handles: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only touches `handles` through `lparam`.
    let _ = unsafe {
        EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut handles as *mut _ as isize),
        )
    };
    let mut out: Vec<Monitor> = handles.into_iter().filter_map(|h| info(h).ok()).collect();
    out.sort_by_key(|m| {
        (
            m.bounds.left != 0 || m.bounds.top != 0,
            m.bounds.left,
            m.bounds.top,
        )
    });
    out
}

/// The rectangle around all monitors (the virtual screen).
pub fn virtual_bounds() -> Result<Rect> {
    union(all().iter().map(|m| m.bounds)).ok_or_else(|| PlatformError::Other("no monitors".into()))
}

fn union(rects: impl Iterator<Item = Rect>) -> Option<Rect> {
    rects.reduce(|a, b| Rect {
        left: a.left.min(b.left),
        top: a.top.min(b.top),
        right: a.right.max(b.right),
        bottom: a.bottom.max(b.bottom),
    })
}

pub fn cursor_pos() -> Result<(i32, i32)> {
    let mut p = POINT::default();
    // SAFETY: valid out pointer.
    unsafe { GetCursorPos(&mut p) }.api("GetCursorPos")?;
    Ok((p.x, p.y))
}

pub fn primary() -> Result<Monitor> {
    // SAFETY: (0,0) always lies on the primary monitor.
    info(unsafe { MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY) })
}

pub fn under_cursor() -> Result<Monitor> {
    let (x, y) = cursor_pos()?;
    // SAFETY: plain lookup.
    info(unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) })
}

pub fn of_window(hwnd: Hwnd) -> Result<Monitor> {
    // SAFETY: plain lookup.
    info(unsafe { MonitorFromWindow(hwnd.get(), MONITOR_DEFAULTTONEAREST) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unions_side_by_side_monitors() {
        let a = Rect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let b = Rect {
            left: -1600,
            top: 200,
            right: 0,
            bottom: 1100,
        };
        let u = union([a, b].into_iter()).expect("two rects");
        assert_eq!((u.left, u.top, u.right, u.bottom), (-1600, 0, 1920, 1100));
        assert!(union(std::iter::empty()).is_none());
    }
}
