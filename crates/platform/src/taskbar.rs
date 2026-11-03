//! Visibility controls for the native Windows taskbar.

use crate::monitor::Monitor;
use crate::{PlatformError, Result};
use std::path::PathBuf;
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromWindow};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetClassNameW, IsWindow, SW_HIDE, SW_SHOWNA, ShowWindow,
};
use windows::core::BOOL;

/// The taskbar windows that were visible before Dook temporarily hid them.
/// Restoring this snapshot preserves taskbars Explorer had already hidden.
#[derive(Default)]
pub struct VisibilitySnapshot(Vec<(HWND, bool)>);

/// Reads the current user's pinned taskbar shortcuts from Explorer's taskbar
/// pin directory. This mirrors Windows pins without maintaining a second list.
pub fn pinned_shortcuts() -> Result<Vec<PathBuf>> {
    let roaming = std::env::var_os("APPDATA")
        .ok_or_else(|| PlatformError::Other("APPDATA is unavailable".into()))?;
    let directory = PathBuf::from(roaming)
        .join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(PlatformError::Other(format!(
                "reading pinned taskbar shortcuts: {error}"
            )));
        }
    };
    let mut shortcuts = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            PlatformError::Other(format!("reading a pinned taskbar shortcut: {error}"))
        })?;
        let path = entry.path();
        let supported = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(extension.to_ascii_lowercase().as_str(), "lnk" | "url")
            });
        if supported && path.is_file() {
            shortcuts.push(path);
        }
    }
    shortcuts.sort_by_cached_key(|path| {
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_lowercase()
    });
    Ok(shortcuts)
}

/// Hides currently visible primary and per-monitor taskbars, retaining their
/// original visibility for restoration when Dook closes.
pub fn hide(monitor: Monitor) -> Result<VisibilitySnapshot> {
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

    let mut hidden = Vec::new();
    for (hwnd, was_visible) in taskbars.0.iter().copied() {
        // Explorer can recreate a secondary taskbar during enumeration.
        // Only replace the Windows taskbar on the monitor Dook occupies.
        // SAFETY: verify the handle before querying its monitor or visibility.
        let on_target = unsafe {
            IsWindow(Some(hwnd)).as_bool()
                && MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST).0 as isize == monitor.id
        };
        if on_target && was_visible {
            let _ = unsafe { ShowWindow(hwnd, SW_HIDE) };
            hidden.push((hwnd, was_visible));
        }
    }
    if hidden.is_empty() {
        return Err(PlatformError::Other(
            "no visible Windows taskbar found on the selected monitor".into(),
        ));
    }
    Ok(VisibilitySnapshot(hidden))
}

/// Restores exactly the taskbars that were visible before [`hide`].
pub fn restore(snapshot: VisibilitySnapshot) {
    for (hwnd, was_visible) in snapshot.0 {
        if was_visible && unsafe { IsWindow(Some(hwnd)) }.as_bool() {
            let _ = unsafe { ShowWindow(hwnd, SW_SHOWNA) };
        }
    }
}
