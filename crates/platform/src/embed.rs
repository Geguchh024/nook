//! Hosting another process's window on top of the notch: phone link shows
//! scrcpy's mirror window over its page as if it were part of the panel.
//!
//! The guest window is made a borderless tool window *owned* by Nook's main
//! window, so it always stays directly above the panel in the z-order
//! (whether Nook is on top or behind other windows) and never gets a taskbar
//! button. It is layered at full opacity so `WS_EX_TRANSPARENT` can make it
//! view-only: clicks then fall through to the panel underneath. A rounded
//! window region gives it the same corners as the cards around it.
//!
//! While the guest has the keyboard, [`EscHook`] (a low-level keyboard hook)
//! takes Esc for Nook, so the user can always get out of a mirrored phone
//! that would otherwise swallow every key.

use crate::window::Hwnd;
use crate::{Result, Win32Context};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::Input::KeyboardAndMouse::VK_ESCAPE;
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, CallNextHookEx, DispatchMessageW, EnumWindows, GWL_EXSTYLE, GWL_STYLE,
    GWLP_HWNDPARENT, GetForegroundWindow, GetMessageW, GetWindowLongPtrW, GetWindowTextW,
    GetWindowThreadProcessId, HC_ACTION, HHOOK, IsWindow, IsWindowVisible, KBDLLHOOKSTRUCT,
    LWA_ALPHA, MSG, PostThreadMessageW, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetForegroundWindow, SetLayeredWindowAttributes,
    SetWindowLongPtrW, SetWindowPos, SetWindowsHookExW, ShowWindow, TranslateMessage,
    UnhookWindowsHookEx, WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
    WS_CAPTION, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
    WS_THICKFRAME,
};
use windows::core::BOOL;

/// A visible top-level window of process `pid` whose title is `title`.
pub fn find_process_window(pid: u32, title: &str) -> Option<Hwnd> {
    struct Search {
        pid: u32,
        title: Vec<u16>,
        found: Option<HWND>,
    }
    extern "system" fn visit(h: HWND, l: LPARAM) -> BOOL {
        // SAFETY: `l` is the `Search` below, alive for the whole enumeration.
        let s = unsafe { &mut *(l.0 as *mut Search) };
        let mut pid = 0;
        // SAFETY: read-only queries on a window handed to us by EnumWindows.
        unsafe { GetWindowThreadProcessId(h, Some(&mut pid)) };
        if pid != s.pid || !unsafe { IsWindowVisible(h) }.as_bool() {
            return true.into();
        }
        let mut buf = [0u16; 256];
        // SAFETY: the buffer is large enough for the length passed.
        let n = unsafe { GetWindowTextW(h, &mut buf) } as usize;
        if buf[..n] == s.title[..] {
            s.found = Some(h);
            return false.into();
        }
        true.into()
    }
    let mut search = Search {
        pid,
        title: title.encode_utf16().collect(),
        found: None,
    };
    // SAFETY: the callback only touches `search`. EnumWindows reports an
    // error when the callback stops early, which is the success case here.
    let _ = unsafe { EnumWindows(Some(visit), LPARAM(&mut search as *mut Search as isize)) };
    search.found.map(|h| Hwnd::from_raw(h.0 as isize))
}

/// Whether the handle still names a window (the guest may have exited).
pub fn is_alive(hwnd: Hwnd) -> bool {
    // SAFETY: IsWindow accepts any value.
    unsafe { IsWindow(Some(hwnd.get())) }.as_bool()
}

/// Turns `guest` into a frameless, owned tool window above `owner`, layered
/// at full opacity so it can become click-through later.
pub fn adopt(guest: Hwnd, owner: Hwnd) -> Result<()> {
    // SAFETY: style and owner changes on a live top-level window. Owning a
    // window of another process is allowed; it ties the two input queues,
    // which is what keeps the guest's focus and z-order with the panel.
    unsafe {
        let style = GetWindowLongPtrW(guest.get(), GWL_STYLE);
        let frame = (WS_CAPTION.0 | WS_THICKFRAME.0) as isize;
        SetWindowLongPtrW(
            guest.get(),
            GWL_STYLE,
            (style | WS_POPUP.0 as isize) & !frame,
        );
        let ex = GetWindowLongPtrW(guest.get(), GWL_EXSTYLE);
        let want =
            (ex | (WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED.0) as isize) & !(WS_EX_APPWINDOW.0 as isize);
        SetWindowLongPtrW(guest.get(), GWL_EXSTYLE, want);
        SetLayeredWindowAttributes(guest.get(), Default::default(), 255, LWA_ALPHA)
            .api("SetLayeredWindowAttributes(guest)")?;
        SetWindowLongPtrW(guest.get(), GWLP_HWNDPARENT, owner.raw());
        SetWindowPos(
            guest.get(),
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )
        .api("SetWindowPos(adopt)")
    }
}

/// Moves and sizes the guest (physical pixels) and rounds its corners with
/// `radius` pixels.
pub fn place(guest: Hwnd, x: i32, y: i32, w: i32, h: i32, radius: i32) -> Result<()> {
    // SAFETY: plain placement of a live window; the region is owned by the
    // system once SetWindowRgn succeeds.
    unsafe {
        SetWindowPos(guest.get(), None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE)
            .api("SetWindowPos(guest)")?;
        let d = radius * 2;
        let rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, d, d);
        if SetWindowRgn(guest.get(), Some(rgn), true) == 0 {
            let _ = windows::Win32::Graphics::Gdi::DeleteObject(rgn.into());
        }
    }
    Ok(())
}

/// Puts `guest` directly above `owner` if it is not above it already.
/// Owning a window that already exists does not reorder it: Windows only
/// enforces "owned above owner" on later z-order changes, so the mirror
/// started out under the panel (rendering, but hidden behind it).
pub fn raise_above(guest: Hwnd, owner: Hwnd) {
    use windows::Win32::UI::WindowsAndMessaging::{
        GW_HWNDNEXT, GW_HWNDPREV, GetWindow, HWND_TOP, HWND_TOPMOST,
    };
    // SAFETY: read-only z-order walk and one placement of a live window;
    // stale handles only make calls fail.
    unsafe {
        let mut cur = guest.get();
        while let Ok(next) = GetWindow(cur, GW_HWNDNEXT) {
            if next == owner.get() {
                return;
            }
            cur = next;
        }
        let topmost = crate::window::is_topmost(owner);
        // Insert right under whatever sits on top of the owner, or at the
        // top of the owner's band.
        let after = match GetWindow(owner.get(), GW_HWNDPREV) {
            Ok(prev)
                if prev != guest.get()
                    && crate::window::is_topmost(Hwnd::from_raw(prev.0 as isize)) == topmost =>
            {
                prev
            }
            _ if topmost => HWND_TOPMOST,
            _ => HWND_TOP,
        };
        if topmost {
            let _ = SetWindowPos(
                guest.get(),
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        let _ = SetWindowPos(
            guest.get(),
            Some(after),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        );
    }
}

/// Shows or hides the guest through its layered opacity. It is never
/// really hidden: scrcpy's OpenGL surface stops presenting after a
/// `SW_HIDE`/`SW_SHOW` round trip and stays black (measured), while an
/// opacity change is invisible to it.
pub fn set_visible(guest: Hwnd, visible: bool) {
    // SAFETY: attribute and visibility changes of a live layered window,
    // never activating it.
    unsafe {
        let _ = SetLayeredWindowAttributes(
            guest.get(),
            Default::default(),
            if visible { 255 } else { 0 },
            LWA_ALPHA,
        );
        if visible && !IsWindowVisible(guest.get()).as_bool() {
            let _ = ShowWindow(guest.get(), SW_SHOWNOACTIVATE);
        }
    }
}

/// Click-through (view only) or interactive.
pub fn set_pass_through(guest: Hwnd, through: bool) {
    // SAFETY: style read-modify-write of a live window, then a frame
    // refresh so hit testing picks it up.
    unsafe {
        let ex = GetWindowLongPtrW(guest.get(), GWL_EXSTYLE);
        let bit = WS_EX_TRANSPARENT.0 as isize;
        let want = if through { ex | bit } else { ex & !bit };
        if want != ex {
            SetWindowLongPtrW(guest.get(), GWL_EXSTYLE, want);
            let _ = SetWindowPos(
                guest.get(),
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
        }
    }
}

pub fn foreground() -> Option<Hwnd> {
    // SAFETY: plain query.
    let h = unsafe { GetForegroundWindow() };
    (!h.is_invalid()).then(|| Hwnd::from_raw(h.0 as isize))
}

/// Hands the keyboard to `hwnd` even though Nook (which never activates)
/// is not the foreground app: joins the foreground thread's input state for
/// the moment of the switch, which lifts the foreground lock.
pub fn give_focus(hwnd: Hwnd) {
    // SAFETY: input attachment is undone right after the switch; every call
    // tolerates stale handles.
    unsafe {
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let me = GetCurrentThreadId();
        let attached =
            fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
        let _ = BringWindowToTop(hwnd.get());
        let _ = SetForegroundWindow(hwnd.get());
        if attached {
            let _ = AttachThreadInput(me, fg_thread, false);
        }
    }
}

/// Window that Esc belongs to while the hook is installed, and the callback.
static ESC_TARGET: AtomicIsize = AtomicIsize::new(0);
static ESC_SWALLOWING: AtomicBool = AtomicBool::new(false);
static ESC_CALLBACK: Mutex<Option<Box<dyn Fn() + Send>>> = Mutex::new(None);

unsafe extern "system" fn esc_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for WH_KEYBOARD_LL, lparam points at a KBDLLHOOKSTRUCT.
        let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if info.vkCode == u32::from(VK_ESCAPE.0) {
            let msg = wparam.0 as u32;
            let target = ESC_TARGET.load(Ordering::Relaxed);
            // SAFETY: plain query.
            let fg = unsafe { GetForegroundWindow() }.0 as isize;
            if (msg == WM_KEYDOWN || msg == WM_SYSKEYDOWN) && target != 0 && fg == target {
                if !ESC_SWALLOWING.swap(true, Ordering::Relaxed)
                    && let Ok(cb) = ESC_CALLBACK.lock()
                    && let Some(cb) = cb.as_ref()
                {
                    cb();
                }
                return LRESULT(1);
            }
            if (msg == WM_KEYUP || msg == WM_SYSKEYUP)
                && ESC_SWALLOWING.swap(false, Ordering::Relaxed)
            {
                return LRESULT(1);
            }
        }
    }
    // SAFETY: passing the event on down the hook chain.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

/// Catches Esc while `target` is the foreground window: the key never
/// reaches it, and `on_esc` runs instead (on the hook's thread). Dropping
/// the hook removes it. Only one exists at a time.
pub struct EscHook {
    thread: u32,
    join: Option<std::thread::JoinHandle<()>>,
}

impl EscHook {
    pub fn install(target: Hwnd, on_esc: impl Fn() + Send + 'static) -> Result<Self> {
        ESC_TARGET.store(target.raw(), Ordering::Relaxed);
        ESC_SWALLOWING.store(false, Ordering::Relaxed);
        if let Ok(mut cb) = ESC_CALLBACK.lock() {
            *cb = Some(Box::new(on_esc));
        }
        let (tx, rx) = std::sync::mpsc::channel::<std::result::Result<u32, String>>();
        let join = std::thread::Builder::new()
            .name("nook-esc-hook".into())
            .spawn(move || {
                // SAFETY: the hook lives on this thread, which pumps messages
                // until WM_QUIT and then removes it.
                unsafe {
                    let module = GetModuleHandleW(None).ok();
                    let hook: HHOOK = match SetWindowsHookExW(
                        WH_KEYBOARD_LL,
                        Some(esc_proc),
                        module.map(Into::into),
                        0,
                    ) {
                        Ok(h) => h,
                        Err(e) => {
                            let _ = tx.send(Err(e.to_string()));
                            return;
                        }
                    };
                    let _ = tx.send(Ok(GetCurrentThreadId()));
                    let mut msg = MSG::default();
                    while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                    let _ = UnhookWindowsHookEx(hook);
                }
            })
            .map_err(|e| crate::PlatformError::Other(format!("spawning hook thread: {e}")))?;
        match rx.recv() {
            Ok(Ok(thread)) => Ok(Self {
                thread,
                join: Some(join),
            }),
            Ok(Err(e)) => Err(crate::PlatformError::Other(format!(
                "SetWindowsHookExW: {e}"
            ))),
            Err(_) => Err(crate::PlatformError::Other("hook thread died".into())),
        }
    }
}

impl Drop for EscHook {
    fn drop(&mut self) {
        ESC_TARGET.store(0, Ordering::Relaxed);
        // SAFETY: posting WM_QUIT to the hook's own message loop.
        let _ = unsafe { PostThreadMessageW(self.thread, WM_QUIT, WPARAM(0), LPARAM(0)) };
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
        if let Ok(mut cb) = ESC_CALLBACK.lock() {
            *cb = None;
        }
    }
}
