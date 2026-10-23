//! Window styling, placement, click-through and z-order.
//!
//! The overlay window never uses a window region. Every `SetWindowRgn` on the
//! OpenGL window made DWM paint the newly included area pale for a frame or
//! two before the GL content caught up, which flashed white corners while the
//! shell sprang open (see docs/NOTES.md). The window is layered instead, and
//! `WS_EX_TRANSPARENT` is toggled by [`crate::hittest`] so clicks outside the
//! shape fall through to the app behind.

use crate::{Result, Win32Context};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumThreadWindows, FindWindowW, GW_OWNER, GWL_EXSTYLE, GWL_STYLE, GWLP_HWNDPARENT, GetWindow,
    GetWindowLongPtrW, GetWindowRect, GetWindowThreadProcessId, HWND_BOTTOM, HWND_NOTOPMOST,
    HWND_TOPMOST, IsWindowVisible, LWA_ALPHA, SW_SHOWNOACTIVATE, SWP_FRAMECHANGED, SWP_NOACTIVATE,
    SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER, SetLayeredWindowAttributes, SetWindowLongPtrW,
    SetWindowPos, ShowWindow, WM_NCCALCSIZE, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_LAYERED,
    WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_MAXIMIZEBOX,
    WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
};
use windows::core::BOOL;

/// A window handle that can cross threads. Win32 handles are plain integers
/// owned by the window manager; the wrapper only forgets the pointer type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Hwnd(isize);

impl Hwnd {
    pub fn from_raw(raw: isize) -> Self {
        Self(raw)
    }

    pub fn raw(self) -> isize {
        self.0
    }

    pub(crate) fn get(self) -> HWND {
        HWND(self.0 as *mut core::ffi::c_void)
    }
}

/// Frame bits winit keeps on an undecorated window (for the DWM shadow). On
/// a layered window DWM then draws that frame: a shadowed, tinted box the
/// size of the whole window, with a tiny caption close button.
const FRAME_STYLE: u32 =
    WS_CAPTION.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0 | WS_MAXIMIZEBOX.0 | WS_THICKFRAME.0;

/// Makes the client area cover the whole window. Without the caption bits
/// winit's `WM_NCCALCSIZE` handling leaves a 1 px non-client strip at the
/// top, which DWM fills pale (a white line along the top of the screen).
/// Call once, on the window's thread.
pub fn fill_client(hwnd: Hwnd) -> Result<()> {
    unsafe extern "system" fn proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
        _id: usize,
        _data: usize,
    ) -> LRESULT {
        if msg == WM_NCCALCSIZE && wparam.0 != 0 {
            // Keep the proposed window rect as the client rect: no frame.
            return LRESULT(0);
        }
        // SAFETY: forwarding to the next window procedure in the chain.
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }
    // SAFETY: subclassing our own window from its thread; `proc` is 'static.
    if !unsafe { SetWindowSubclass(hwnd.get(), Some(proc), 1, 0) }.as_bool() {
        return Err(crate::PlatformError::Other(
            "SetWindowSubclass failed".into(),
        ));
    }
    refresh_frame(hwnd)
}

/// Frameless, no-taskbar popup that may take focus, without the layered
/// style (used by full-screen overlays such as draw-on-screen). Cheap to
/// call every frame: it only writes when bits drifted.
pub fn ensure_popup(hwnd: Hwnd) -> Result<bool> {
    let changed = update_ex_style(hwnd, |ex| {
        (ex | WS_EX_TOOLWINDOW.0 as isize)
            & !((WS_EX_APPWINDOW.0 | WS_EX_LAYERED.0 | WS_EX_NOACTIVATE.0) as isize)
    })?;
    let restyled = update_style(hwnd);
    if changed.is_none() && !restyled {
        return Ok(false);
    }
    refresh_frame(hwnd)?;
    Ok(true)
}

/// Turns on per-pixel alpha for an OpenGL window: DWM blur-behind with an
/// empty region composites the GL alpha channel instead of treating the
/// surface as opaque. winit does this for the main window but not for
/// secondary egui viewports, which otherwise come out black.
pub fn enable_alpha(hwnd: Hwnd) -> Result<()> {
    use windows::Win32::Graphics::Dwm::{
        DWM_BB_BLURREGION, DWM_BB_ENABLE, DWM_BLURBEHIND, DwmEnableBlurBehindWindow,
        DwmExtendFrameIntoClientArea,
    };
    use windows::Win32::Graphics::Gdi::{CreateRectRgn, DeleteObject};
    use windows::Win32::UI::Controls::MARGINS;
    // SAFETY: plain DWM call on our own window; the region is freed after
    // DWM has copied it.
    unsafe {
        let region = CreateRectRgn(0, 0, -1, -1);
        let bb = DWM_BLURBEHIND {
            dwFlags: DWM_BB_ENABLE | DWM_BB_BLURREGION,
            fEnable: BOOL(1),
            hRgnBlur: region,
            fTransitionOnMaximized: BOOL(0),
        };
        let res = DwmEnableBlurBehindWindow(hwnd.get(), &bb).api("DwmEnableBlurBehindWindow");
        let _ = DeleteObject(region.into());
        res?;
        // "Sheet of glass": the whole client area is composited with its
        // alpha channel. The window has no caption, so no frame is drawn.
        let glass = MARGINS {
            cxLeftWidth: -1,
            cxRightWidth: -1,
            cyTopHeight: -1,
            cyBottomHeight: -1,
        };
        DwmExtendFrameIntoClientArea(hwnd.get(), &glass).api("DwmExtendFrameIntoClientArea")
    }
}

/// Makes `owner` own `hwnd`: an owned window always stays above its owner
/// (the draw toolbar above the ink canvas).
pub fn set_owner(hwnd: Hwnd, owner: Hwnd) {
    // SAFETY: re-parenting the owner of one of our own windows.
    unsafe {
        SetWindowLongPtrW(hwnd.get(), GWLP_HWNDPARENT, owner.raw());
    }
}

/// Asks DWM for Windows 11 rounded corners on a small window.
pub fn round_corners(hwnd: Hwnd) {
    use windows::Win32::Graphics::Dwm::{
        DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
    };
    let pref = DWMWCP_ROUND;
    // SAFETY: attribute and size match; cosmetic, so failures are ignored.
    let _ = unsafe {
        DwmSetWindowAttribute(
            hwnd.get(),
            DWMWA_WINDOW_CORNER_PREFERENCE,
            &pref as *const _ as *const _,
            std::mem::size_of_val(&pref) as u32,
        )
    };
}

/// A top-level window of this process with exactly this title (eframe does
/// not hand out the handles of secondary viewports).
pub fn find_own(title: &str) -> Option<Hwnd> {
    let title = crate::wide(title);
    // SAFETY: read-only lookups; the title outlives the call.
    unsafe {
        let hwnd = FindWindowW(
            windows::core::PCWSTR::null(),
            windows::core::PCWSTR(title.as_ptr()),
        )
        .ok()?;
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        (pid == GetCurrentProcessId()).then(|| Hwnd::from_raw(hwnd.0 as isize))
    }
}

/// Read-modify-write of the window style: a bare popup, no frame.
fn update_style(hwnd: Hwnd) -> bool {
    // SAFETY: plain style query/update on a window we own.
    unsafe {
        let style = GetWindowLongPtrW(hwnd.get(), GWL_STYLE);
        let want = (style | WS_POPUP.0 as isize) & !(FRAME_STYLE as isize);
        if want == style {
            return false;
        }
        SetWindowLongPtrW(hwnd.get(), GWL_STYLE, want);
        true
    }
}

/// Read-modify-write of the extended style. Call it on the window's own
/// thread only (see `hittest`).
fn update_ex_style(hwnd: Hwnd, f: impl FnOnce(isize) -> isize) -> Result<Option<isize>> {
    // SAFETY: plain style query/update on a window we own.
    unsafe {
        let ex = GetWindowLongPtrW(hwnd.get(), GWL_EXSTYLE);
        let want = f(ex);
        if want == ex {
            return Ok(None);
        }
        SetWindowLongPtrW(hwnd.get(), GWL_EXSTYLE, want);
        Ok(Some(ex))
    }
}

/// Makes the window a frameless, layered, no-taskbar tool window that does not take
/// focus unless `activatable` (text input needs focus). winit re-applies its
/// own styles on some state changes, so call this every frame: it only
/// touches the window when the bits drifted. Leaves `WS_EX_TRANSPARENT` and
/// `WS_EX_TOPMOST` alone (see [`set_click_through`] and [`set_layer`]).
/// Returns whether it changed anything.
pub fn ensure_overlay(hwnd: Hwnd, activatable: bool) -> Result<bool> {
    let changed = update_ex_style(hwnd, |ex| {
        let mut want =
            (ex | (WS_EX_TOOLWINDOW.0 | WS_EX_LAYERED.0) as isize) & !(WS_EX_APPWINDOW.0 as isize);
        if activatable {
            want &= !(WS_EX_NOACTIVATE.0 as isize);
        } else {
            want |= WS_EX_NOACTIVATE.0 as isize;
        }
        want
    })?;
    let restyled = update_style(hwnd);
    if changed.is_none() && !restyled {
        return Ok(false);
    }
    if let Some(before) = changed
        && before & WS_EX_LAYERED.0 as isize == 0
    {
        // A layered window shows nothing until its attributes are set.
        set_opacity(hwnd, 255)?;
    }
    refresh_frame(hwnd)?;
    Ok(true)
}

/// Posts a left-button release at the cursor to `hwnd`, for when a modal
/// loop (a shell drag) swallowed the real one and the UI still thinks the
/// button is held.
pub fn post_left_button_up(hwnd: Hwnd) {
    use windows::Win32::Foundation::{LPARAM, POINT, WPARAM};
    use windows::Win32::Graphics::Gdi::ScreenToClient;
    use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, PostMessageW, WM_LBUTTONUP};
    let mut p = POINT::default();
    // SAFETY: plain queries and a posted message to our own window.
    unsafe {
        let _ = GetCursorPos(&mut p);
        let _ = ScreenToClient(hwnd.get(), &mut p);
        let lparam = ((p.y as u16 as isize) << 16) | (p.x as u16 as isize);
        let _ = PostMessageW(Some(hwnd.get()), WM_LBUTTONUP, WPARAM(0), LPARAM(lparam));
    }
}

/// Brings the window to the foreground so it gets keyboard input. Works
/// right after the user clicked it (the click grants the right).
pub fn activate(hwnd: Hwnd) {
    // SAFETY: plain call on our own window; failure only means no focus.
    let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd.get()) };
}

/// Whole-window opacity of a layered window (0 hides it, 255 shows the
/// per-pixel alpha content as is).
pub fn set_opacity(hwnd: Hwnd, alpha: u8) -> Result<()> {
    // SAFETY: attribute update on a layered window we own.
    unsafe { SetLayeredWindowAttributes(hwnd.get(), Default::default(), alpha, LWA_ALPHA) }
        .api("SetLayeredWindowAttributes")
}

/// Shows the window without activating it, if it is hidden.
pub fn ensure_shown(hwnd: Hwnd) {
    // SAFETY: visibility query/show on a window we own.
    unsafe {
        if !IsWindowVisible(hwnd.get()).as_bool() {
            let _ = ShowWindow(hwnd.get(), SW_SHOWNOACTIVATE);
        }
    }
}

/// Lets mouse input fall through the (layered) window to whatever is below.
pub fn set_click_through(hwnd: Hwnd, through: bool) -> Result<()> {
    let changed = update_ex_style(hwnd, |ex| {
        if through {
            ex | WS_EX_TRANSPARENT.0 as isize
        } else {
            ex & !(WS_EX_TRANSPARENT.0 as isize)
        }
    })?;
    if changed.is_some() {
        // Hit-testing keeps using the old style until the frame is refreshed.
        refresh_frame(hwnd)?;
    }
    Ok(())
}

fn refresh_frame(hwnd: Hwnd) -> Result<()> {
    // SAFETY: frame refresh on our own window.
    unsafe {
        SetWindowPos(
            hwnd.get(),
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
        )
    }
    .api("SetWindowPos(FRAMECHANGED)")
}

/// Whether the window lost its place at the top: the topmost bit was
/// dropped, or a visible topmost window of another process sits above it
/// and overlaps it. Read-only, safe from any thread.
pub fn is_covered(hwnd: Hwnd) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::GW_HWNDPREV;
    if !is_topmost(hwnd) {
        return true;
    }
    let Ok(ours) = window_rect(hwnd) else {
        return false;
    };
    // SAFETY: read-only z-order walk; stale handles only make calls fail.
    unsafe {
        let mut own_pid = 0;
        GetWindowThreadProcessId(hwnd.get(), Some(&mut own_pid));
        let mut cur = hwnd.get();
        while let Ok(prev) = GetWindow(cur, GW_HWNDPREV) {
            cur = prev;
            let other = Hwnd(prev.0 as isize);
            if !is_topmost(other) || !IsWindowVisible(prev).as_bool() {
                continue;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(prev, Some(&mut pid));
            if pid == own_pid {
                continue;
            }
            // A guest window hosted on the panel (phone mirror) is meant
            // to sit above it.
            if GetWindow(prev, GW_OWNER).is_ok_and(|o| o == hwnd.get()) {
                continue;
            }
            let Ok((x, y, w, h)) = window_rect(other) else {
                continue;
            };
            let overlaps =
                x < ours.0 + ours.2 && ours.0 < x + w && y < ours.1 + ours.3 && ours.1 < y + h;
            if overlaps && w > 0 && h > 0 {
                return true;
            }
        }
    }
    false
}

pub fn is_topmost(hwnd: Hwnd) -> bool {
    // SAFETY: style query on a window we own.
    let ex = unsafe { GetWindowLongPtrW(hwnd.get(), GWL_EXSTYLE) };
    ex & WS_EX_TOPMOST.0 as isize != 0
}

/// Puts the overlay above all windows (`on_top`) or behind all normal
/// windows. `owner` is the backdrop window `main` is owned by; it always
/// stays directly under `main`.
pub fn set_layer(main: Hwnd, owner: Option<Hwnd>, on_top: bool) -> Result<()> {
    let flags = SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE;
    let place = |h: Hwnd, after: HWND| {
        // SAFETY: z-order change on windows we own.
        unsafe { SetWindowPos(h.get(), Some(after), 0, 0, 0, 0, flags) }.api("SetWindowPos(layer)")
    };
    if on_top {
        // Windows silently refuses to make a window topmost while it owns a
        // window that is not, such as the hidden IME window that belongs to
        // winit's window (and `main` itself, owned by the backdrop). Detach
        // the owned windows, promote, then restore ownership; a topmost owner
        // keeps its non-topmost owned windows once they are re-attached.
        let root = owner.unwrap_or(main);
        let tree = owned_by(root.get());
        // SAFETY: owner (GWLP_HWNDPARENT) changes on windows of this thread,
        // restored right below.
        let owners: Vec<(HWND, isize)> = tree
            .iter()
            .map(|&h| unsafe { (h, SetWindowLongPtrW(h, GWLP_HWNDPARENT, 0)) })
            .collect();
        let promoted = place(root, HWND_TOPMOST).and_then(|()| {
            if owner.is_some() {
                place(main, HWND_TOPMOST)
            } else {
                Ok(())
            }
        });
        for (h, o) in owners.into_iter().rev() {
            // SAFETY: see above.
            unsafe { SetWindowLongPtrW(h, GWLP_HWNDPARENT, o) };
        }
        promoted?;
        if !is_topmost(main) {
            return Err(crate::PlatformError::Other(
                "window refused to become topmost".into(),
            ));
        }
    } else {
        if let Some(o) = owner {
            place(o, HWND_NOTOPMOST)?;
        }
        place(main, HWND_NOTOPMOST)?;
        // Main first, then its owner below it.
        place(main, HWND_BOTTOM)?;
        if let Some(o) = owner {
            place(o, HWND_BOTTOM)?;
        }
    }
    Ok(())
}

/// Windows of the calling thread owned by `root`, directly or through other
/// owned windows, deepest first (restoring in reverse rebuilds the tree).
fn owned_by(root: HWND) -> Vec<HWND> {
    extern "system" fn collect(h: HWND, l: LPARAM) -> BOOL {
        // SAFETY: `l` is the `Vec` passed below, alive for the whole call.
        unsafe { (*(l.0 as *mut Vec<HWND>)).push(h) };
        true.into()
    }
    let mut all: Vec<HWND> = Vec::new();
    // SAFETY: the callback only pushes into `all`.
    unsafe {
        let _ = EnumThreadWindows(
            GetCurrentThreadId(),
            Some(collect),
            LPARAM(&mut all as *mut Vec<HWND> as isize),
        );
    }
    let depth = |h: HWND| -> Option<usize> {
        let mut cur = h;
        for d in 1..8 {
            // SAFETY: owner query; fails at the top of the chain.
            cur = unsafe { GetWindow(cur, GW_OWNER) }.ok()?;
            if cur == root {
                return Some(d);
            }
        }
        None
    };
    let mut owned: Vec<(usize, HWND)> = all
        .into_iter()
        .filter_map(|h| depth(h).map(|d| (d, h)))
        .collect();
    owned.sort_by_key(|&(d, _)| std::cmp::Reverse(d));
    owned.into_iter().map(|(_, h)| h).collect()
}

/// Window rectangle in physical pixels: (left, top, width, height).
pub fn window_rect(hwnd: Hwnd) -> Result<(i32, i32, i32, i32)> {
    let mut r = RECT::default();
    // SAFETY: r is a valid out pointer.
    unsafe { GetWindowRect(hwnd.get(), &mut r) }.api("GetWindowRect")?;
    Ok((r.left, r.top, r.right - r.left, r.bottom - r.top))
}

/// Moves and resizes the window (physical pixels) without touching z-order.
pub fn set_rect(hwnd: Hwnd, x: i32, y: i32, w: i32, h: i32) -> Result<()> {
    // SAFETY: SetWindowPos on our own window.
    unsafe { SetWindowPos(hwnd.get(), None, x, y, w, h, SWP_NOZORDER | SWP_NOACTIVATE) }
        .api("SetWindowPos(rect)")
}

pub fn dpi(hwnd: Hwnd) -> u32 {
    // SAFETY: returns 0 for an invalid window, which we map to 96.
    match unsafe { GetDpiForWindow(hwnd.get()) } {
        0 => 96,
        d => d,
    }
}
