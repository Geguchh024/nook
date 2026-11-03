//! Visibility controls for the native Windows taskbar.

use crate::{PlatformError, Result};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, IsWindow, SW_HIDE, SW_SHOWNA, ShowWindow,
};
use windows::core::BOOL;

/// The taskbar windows that were visible before Dook temporarily hid them.
/// Restoring this snapshot preserves taskbars Explorer had already hidden.
#[derive(Default)]
pub struct VisibilitySnapshot(Vec<(HWND, bool)>);

/// Hides currently visible primary and per-monitor taskbars, retaining their
/// original visibility for restoration when Dook closes.
pub fn hide() -> Result<VisibilitySnapshot> {
    struct Taskbars(Vec<(HWND, bool)>);

    extern "system" fn collect(hwnd: HWND, state: LPARAM) -> BOOL {
        let mut class = [0u16; 64];
        // SAFETY: a valid fixed-size output buffer and a live window handle.
        let len = unsafe { GetClassNameW(hwnd, &mut class) };
        if len <= 0 {
            return true.into();
        }
        let class = String::from_utf16_lossy(&class[..len as usize]);
        if class == "Shell_TrayWnd" || class == "Shell_SecondaryTrayWnd" {
            // SAFETY: `state` points to the `Taskbars` value passed to EnumWindows.
            // SAFETY: callback state points at the live collection passed to
            // EnumWindows and remains valid for the entire enumeration.
            unsafe { &mut *(state.0 as *mut Taskbars) }.0.push((
                hwnd,
                unsafe { windows::Win32::UI::WindowsAndMessaging::IsWindowVisible(hwnd) }.as_bool(),
            ));
        }
        true.into()
    }

    let mut taskbars = Taskbars(Vec::new());
    // SAFETY: callback only writes to `taskbars`, which remains alive throughout.
    unsafe {
        EnumWindows(
            Some(collect),
            LPARAM(&mut taskbars as *mut Taskbars as isize),
        )
    }
    .map_err(|error| PlatformError::Win32 {
        call: "EnumWindows(taskbars)",
        source: error,
    })?;
    if taskbars.0.is_empty() {
        return Err(PlatformError::Other(
            "Windows taskbar windows were not found".into(),
        ));
    }

    for (hwnd, was_visible) in taskbars.0.iter().copied() {
        // Explorer can recreate a secondary taskbar during enumeration.
        // SAFETY: verify the handle is still a window before changing visibility.
        if unsafe { IsWindow(Some(hwnd)) }.as_bool() && was_visible {
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
        }
    }
    Ok(VisibilitySnapshot(taskbars.0))
}

/// Restores exactly the taskbars that were visible before [`hide`].
pub fn restore(snapshot: VisibilitySnapshot) {
    for (hwnd, was_visible) in snapshot.0 {
        if was_visible && unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNA) };
        }
    }
}
