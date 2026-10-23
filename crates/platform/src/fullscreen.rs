//! "Is a fullscreen app or game in front?"

use crate::monitor::{self, Rect};
use crate::window::Hwnd;
use windows::Win32::Foundation::RECT;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Shell::{
    QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN, SHQueryUserNotificationState,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetShellWindow, GetWindowRect, GetWindowThreadProcessId,
    IsWindowVisible, IsZoomed,
};

/// True when the foreground window covers the whole monitor the notch is on
/// (borderless fullscreen) or the shell reports exclusive D3D fullscreen /
/// presentation mode. Maximized windows, windows on other monitors (a
/// maximized window on a monitor without a taskbar covers it too), the
/// desktop and our own windows (e.g. the draw-on-screen overlay) never count.
pub fn foreground_is_fullscreen(own: Hwnd) -> bool {
    // SAFETY: read-only shell query.
    if let Ok(state) = unsafe { SHQueryUserNotificationState() }
        && (state == QUNS_RUNNING_D3D_FULL_SCREEN || state == QUNS_PRESENTATION_MODE)
    {
        return true;
    }
    // SAFETY: read-only window queries; handles may be stale, which only
    // makes the calls fail.
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg == own.get() || fg == GetShellWindow() {
            return false;
        }
        if !IsWindowVisible(fg).as_bool() || IsZoomed(fg).as_bool() {
            return false;
        }
        let mut pid = 0;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        if pid == GetCurrentProcessId() {
            return false;
        }
        let mut class = [0u16; 64];
        let n = GetClassNameW(fg, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        if matches!(class.as_str(), "WorkerW" | "Progman" | "Shell_TrayWnd") {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(fg, &mut r).is_err() {
            return false;
        }
        let (Ok(mon), Ok(ours)) = (
            monitor::of_window(Hwnd::from_raw(fg.0 as isize)),
            monitor::of_window(own),
        ) else {
            return false;
        };
        mon.bounds == ours.bounds && Rect::from(r).covers(&mon.bounds)
    }
}
