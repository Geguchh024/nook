//! Handing files and URLs to the shell.

use crate::{PlatformError, Result, wide};
use std::os::windows::ffi::OsStrExt;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW,
    SHFileOperationW, ShellExecuteW,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Screenshots, KF_FLAG_DEFAULT,
    SHGetKnownFolderPath,
};
use windows::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow, SW_SHOWNORMAL};
use windows::core::PCWSTR;

/// Opens the Windows emoji and symbols picker with its native shortcut.
pub fn show_emoji_picker() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_LWIN, VK_OEM_PERIOD};
    send_key_chord(&[VK_LWIN, VK_OEM_PERIOD]);
}

/// Invokes a Windows shell shortcut using the user's existing key bindings.
pub fn windows_shortcut(key: u8) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_LWIN};
    send_key_chord(&[VK_LWIN, VIRTUAL_KEY(u16::from(key))]);
}

/// Opens the Windows Start menu without pressing a second key.
pub fn show_start_menu() {
    use windows::Win32::UI::Input::KeyboardAndMouse::VK_LWIN;
    send_key_chord(&[VK_LWIN]);
}

/// Opens Windows Task View.
pub fn show_task_view() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VK_LWIN, VK_TAB};
    send_key_chord(&[VK_LWIN, VK_TAB]);
}

/// Opens the Windows notifications and calendar flyout.
pub fn show_notifications() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_LWIN};
    send_key_chord(&[VK_LWIN, VIRTUAL_KEY(u16::from(b'N'))]);
}

/// Shows or hides the desktop using the native Windows shortcut.
pub fn toggle_desktop() {
    use windows::Win32::UI::Input::KeyboardAndMouse::{VIRTUAL_KEY, VK_LWIN};
    send_key_chord(&[VK_LWIN, VIRTUAL_KEY(u16::from(b'D'))]);
}

fn send_key_chord(keys: &[windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY]) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    };
    let mut events = Vec::with_capacity(keys.len() * 2);
    events.extend(keys.iter().copied().map(|key| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                ..Default::default()
            },
        },
    }));
    events.extend(keys.iter().rev().copied().map(|key| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: key,
                dwFlags: KEYEVENTF_KEYUP,
                ..Default::default()
            },
        },
    }));
    // SAFETY: all events are initialized keyboard inputs and each pressed key
    // receives a matching release in reverse chord order.
    let sent = unsafe { SendInput(&events, std::mem::size_of::<INPUT>() as i32) };
    if sent != events.len() as u32 {
        tracing::debug!(
            "SendInput accepted {sent} of {} keyboard events",
            events.len()
        );
    }
}

/// The user's Screenshots folder (Pictures\Screenshots unless moved).
pub fn screenshots_dir() -> Option<std::path::PathBuf> {
    // SAFETY: the returned string is freed with CoTaskMemFree after copying.
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Screenshots, KF_FLAG_DEFAULT, None) {
            let path = p.to_string().ok().map(std::path::PathBuf::from);
            CoTaskMemFree(Some(p.0 as *const _));
            if path.is_some() {
                return path;
            }
        }
    }
    std::env::var_os("USERPROFILE").map(|u| {
        std::path::PathBuf::from(u)
            .join("Pictures")
            .join("Screenshots")
    })
}

/// The user's Downloads folder.
pub fn downloads_dir() -> Option<std::path::PathBuf> {
    // SAFETY: the returned string is freed with CoTaskMemFree after copying.
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None) {
            let path = p.to_string().ok().map(std::path::PathBuf::from);
            CoTaskMemFree(Some(p.0 as *const _));
            if path.is_some() {
                return path;
            }
        }
    }
    std::env::var_os("USERPROFILE").map(|u| std::path::PathBuf::from(u).join("Downloads"))
}

/// A known folder's path, or `fallback` under the user profile.
fn known_folder(id: &windows::core::GUID, fallback: &str) -> Option<std::path::PathBuf> {
    // SAFETY: the returned string is freed with CoTaskMemFree after copying.
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None) {
            let path = p.to_string().ok().map(std::path::PathBuf::from);
            CoTaskMemFree(Some(p.0 as *const _));
            if path.is_some() {
                return path;
            }
        }
    }
    std::env::var_os("USERPROFILE").map(|u| std::path::PathBuf::from(u).join(fallback))
}

/// The user's Pictures folder.
pub fn pictures_dir() -> Option<std::path::PathBuf> {
    known_folder(&FOLDERID_Pictures, "Pictures")
}

/// The user's Music folder.
pub fn music_dir() -> Option<std::path::PathBuf> {
    known_folder(&FOLDERID_Music, "Music")
}

/// Moves a file or folder to the Recycle Bin, silently.
pub fn recycle(path: &std::path::Path) -> Result<()> {
    // SHFileOperation wants a double-null-terminated list.
    let mut from: Vec<u16> = path.as_os_str().encode_wide().collect();
    from.extend([0, 0]);
    let mut op = SHFILEOPSTRUCTW {
        wFunc: FO_DELETE,
        pFrom: PCWSTR(from.as_ptr()),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI).0 as u16,
        ..Default::default()
    };
    // SAFETY: `from` outlives the call; no window, no progress UI.
    let code = unsafe { SHFileOperationW(&mut op) };
    if code == 0 {
        Ok(())
    } else {
        Err(PlatformError::Code {
            call: "SHFileOperationW",
            code: code as u32,
        })
    }
}

/// Shows `path` selected in an Explorer window.
pub fn reveal(path: &std::path::Path) -> Result<()> {
    let verb = wide("open");
    let exe = wide("explorer.exe");
    let args = wide(&format!("/select,\"{}\"", path.display()));
    // SAFETY: null-terminated strings that outlive the call.
    let code = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(exe.as_ptr()),
            PCWSTR(args.as_ptr()),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    if code.0 as isize > 32 {
        Ok(())
    } else {
        Err(PlatformError::Code {
            call: "ShellExecuteW(explorer)",
            code: code.0 as u32,
        })
    }
}

/// Opens `target` (a path or URL) with its default handler, like a double
/// click in Explorer.
pub fn open(target: &str) -> Result<()> {
    let verb = wide("open");
    let file = wide(target);
    // Pass our foreground right on (we usually hold it right after the
    // snip overlay closed), so the browser comes to the front instead of
    // opening a tab behind the current window.
    // SAFETY: plain call; failure only means the handler opens behind.
    let _ = unsafe { AllowSetForegroundWindow(ASFW_ANY) };
    // SAFETY: null-terminated strings that outlive the call.
    let code = unsafe {
        ShellExecuteW(
            None,
            PCWSTR(verb.as_ptr()),
            PCWSTR(file.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        )
    };
    // Values above 32 mean success.
    if code.0 as isize > 32 {
        Ok(())
    } else {
        Err(PlatformError::Code {
            call: "ShellExecuteW",
            code: code.0 as u32,
        })
    }
}
