//! The freeze-and-select overlay shared by the text extractor, lens search
//! and the color picker.
//!
//! Every monitor is grabbed first, then shown frozen in one topmost popup
//! that spans the whole virtual screen, so menus, tooltips and video hold
//! still while you select (PowerToys Text Extractor and ShareX work the
//! same way), and a selection or the loupe can be on any monitor or across
//! them. The hint, labels and loupe follow the monitor under the cursor
//! and are sized for its DPI.
//!
//! Everything is plain GDI on the overlay's own thread: the frozen frame
//! and a dimmed copy live in memory DCs, and each `WM_PAINT` only
//! recomposes the dirty rectangle through a back buffer the size of that
//! rectangle, so dragging costs a few small blits and never flickers.
//!
//! - [`SnipMode::Region`]: drag a rectangle. Outside it the screen is
//!   dimmed; a label shows its size. Release to finish; Shift at release
//!   asks for a single line of text.
//! - [`SnipMode::Screenshot`]: like region, plus the window under the
//!   cursor is highlighted and a click takes it; F takes the monitor under
//!   the cursor, A every monitor, Enter the highlighted window.
//! - [`SnipMode::Pick`]: a loupe follows the cursor with the pixels under
//!   it magnified and the color's hex code. Scroll to zoom, arrows nudge by
//!   a pixel (Shift: ten), click or Enter picks.
//!
//! Esc or right click cancels. The result is handed to a callback on the
//! overlay thread after its window is gone, so slow work there (OCR) never
//! holds the overlay open.

use crate::capture::{self, Frame};
use crate::{PlatformError, Result, Win32Context, monitor, wide};
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::Graphics::Gdi::CLEARTYPE_QUALITY;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BeginPaint, BitBlt, COLORONCOLOR, CreateCompatibleBitmap,
    CreateCompatibleDC, CreateDIBSection, CreateFontIndirectW, CreatePen, CreateSolidBrush,
    DIB_RGB_COLORS, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DeleteDC, DeleteObject, DrawTextW,
    EndPaint, FW_SEMIBOLD, FillRect, FrameRect, GetStockObject, GetTextExtentPoint32W, HBITMAP,
    HDC, HFONT, HGDIOBJ, InvalidateRect, LOGFONTW, NULL_BRUSH, NULL_PEN, PAINTSTRUCT, PS_SOLID,
    Rectangle, RoundRect, SRCCOPY, SelectObject, SetBkMode, SetStretchBltMode, SetTextColor,
    SetWindowOrgEx, StretchBlt, TRANSPARENT,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_DOWN, VK_ESCAPE, VK_LEFT, VK_RETURN, VK_RIGHT,
    VK_SHIFT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetCursorPos, GetMessageW,
    IDC_CROSS, LoadCursorW, MSG, PostMessageW, PostQuitMessage, RegisterClassExW, SW_SHOW,
    SetCursor, SetCursorPos, SetForegroundWindow, ShowWindow, TranslateMessage, WM_CLOSE,
    WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
    WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN, WM_SETCURSOR, WNDCLASSEXW, WS_EX_TOOLWINDOW,
    WS_EX_TOPMOST, WS_POPUP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GWL_EXSTYLE, GetClassNameW, GetWindowLongPtrW, GetWindowRect,
    GetWindowThreadProcessId, IsIconic, IsWindowVisible, WS_EX_TRANSPARENT,
};
use windows::core::{BOOL, PCWSTR};

/// Loupe zoom steps: pixels shown across (odd, so one sits in the middle).
const LOUPE_CELLS: [i32; 6] = [31, 21, 15, 11, 9, 7];
const LOUPE_DEFAULT: usize = 2;
/// Loupe edge in logical pixels.
const LOUPE_SIZE: i32 = 150;
/// Selections smaller than this (physical pixels) count as a stray click.
const MIN_SELECTION: i32 = 4;
/// Brightness kept outside the selection, out of 256.
const DIM: u32 = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnipMode {
    Region,
    Screenshot,
    Pick,
}

#[derive(Debug)]
pub enum SnipOutcome {
    /// The selected part of the frozen frame. `single_line` is true when
    /// Shift was held as the selection finished.
    Region {
        frame: Frame,
        single_line: bool,
    },
    /// The picked pixel (`[r, g, b]`) and where it was on screen.
    Color {
        rgb: [u8; 3],
        at: (i32, i32),
    },
    Cancelled,
}

/// Handle to an open overlay. Dropping it cancels the overlay.
pub struct Snip {
    hwnd: isize,
    open: Arc<AtomicBool>,
}

impl Snip {
    /// Freezes every monitor and opens the overlay over all of them.
    /// `hint` shows at the top until the pointer starts working.
    /// `on_done` runs on the overlay thread once the overlay has closed.
    pub fn open(
        mode: SnipMode,
        hint: impl Into<String>,
        on_done: impl FnOnce(SnipOutcome) + Send + 'static,
    ) -> Result<Self> {
        let hint = hint.into();
        let open = Arc::new(AtomicBool::new(true));
        let thread_open = open.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("nook-snip".into())
            .spawn(move || {
                let created = monitor::virtual_bounds()
                    .and_then(capture::grab)
                    .and_then(|frame| Overlay::create(mode, frame, &hint, &monitor::all()));
                let overlay = match created {
                    Ok(o) => o,
                    Err(e) => {
                        thread_open.store(false, Ordering::Relaxed);
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let hwnd = overlay.hwnd;
                OVERLAY.with(|o| *o.borrow_mut() = Some(overlay));
                // SAFETY: our own window, on its own thread.
                unsafe {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                    let _ = SetForegroundWindow(hwnd);
                }
                let _ = ready_tx.send(Ok(hwnd.0 as isize));
                pump();
                let outcome = OVERLAY
                    .with(|o| o.borrow_mut().take())
                    .and_then(|mut o| o.outcome.take())
                    .unwrap_or(SnipOutcome::Cancelled);
                thread_open.store(false, Ordering::Relaxed);
                on_done(outcome);
            })
            .map_err(|e| PlatformError::Other(format!("spawning snip thread: {e}")))?;
        let hwnd = ready_rx
            .recv()
            .map_err(|_| PlatformError::Other("snip thread exited".into()))??;
        Ok(Self { hwnd, open })
    }

    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::Relaxed)
    }
}

impl Drop for Snip {
    fn drop(&mut self) {
        let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);
        // SAFETY: posting to our overlay window; fails harmlessly once closed.
        let _ = unsafe { PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    }
}

/// A memory DC with a 32-bit top-down DIB selected into it.
struct Surface {
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    bits: *mut u32,
    len: usize,
}

impl Surface {
    fn new(w: i32, h: i32) -> Result<Self> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: a fresh memory DC and DIB section, owned by the returned
        // value and freed in its `Drop`.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let mut bits = std::ptr::null_mut();
            match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
                Ok(bitmap) if !bits.is_null() => {
                    let old = SelectObject(dc, bitmap.into());
                    Ok(Self {
                        dc,
                        bitmap,
                        old,
                        bits: bits.cast(),
                        len: (w * h) as usize,
                    })
                }
                other => {
                    let _ = DeleteDC(dc);
                    Err(match other {
                        Err(source) => PlatformError::Win32 {
                            call: "CreateDIBSection(snip)",
                            source,
                        },
                        Ok(_) => PlatformError::Other("CreateDIBSection: no bits".into()),
                    })
                }
            }
        }
    }

    fn pixels_ref(&self) -> &[u32] {
        // SAFETY: as in `pixels`.
        unsafe { std::slice::from_raw_parts(self.bits, self.len) }
    }

    fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: `bits` is the DIB section's buffer of `len` pixels, alive
        // as long as `bitmap`, and only touched from the overlay thread.
        unsafe { std::slice::from_raw_parts_mut(self.bits, self.len) }
    }
}

impl Drop for Surface {
    fn drop(&mut self) {
        // SAFETY: freeing what `new` created.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
}

/// One monitor under the overlay: where it is (client coordinates), its
/// scale, and the label font at that scale.
struct Screen {
    rect: RECT,
    scale: f32,
    font: HFONT,
}

impl Screen {
    fn new(m: &monitor::Monitor, origin: (i32, i32)) -> Self {
        let scale = m.dpi as f32 / 96.0;
        let mut face = [0u16; 32];
        for (d, s) in face.iter_mut().zip("Segoe UI".encode_utf16()) {
            *d = s;
        }
        let lf = LOGFONTW {
            lfHeight: -(13.0 * scale).round() as i32,
            lfWeight: FW_SEMIBOLD.0 as i32,
            lfQuality: CLEARTYPE_QUALITY,
            lfFaceName: face,
            ..Default::default()
        };
        let b = m.bounds;
        Self {
            rect: rect(
                b.left - origin.0,
                b.top - origin.1,
                b.right - origin.0,
                b.bottom - origin.1,
            ),
            scale,
            // SAFETY: a font owned by this screen, freed in `Drop`.
            font: unsafe { CreateFontIndirectW(&lf) },
        }
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        // SAFETY: our own font.
        let _ = unsafe { DeleteObject(self.font.into()) };
    }
}

fn contains(r: RECT, (x, y): (i32, i32)) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

struct Overlay {
    hwnd: HWND,
    mode: SnipMode,
    /// Screen position and size of the frozen frame (its pixels live in
    /// `bright`).
    origin: (i32, i32),
    size: (i32, i32),
    /// The frozen frame.
    bright: Surface,
    /// The frozen frame dimmed (region mode only).
    dim: Option<Surface>,
    screens: Vec<Screen>,
    /// Index of the screen under the cursor.
    screen: usize,
    /// Visible top-level windows, front to back, client coordinates
    /// (screenshot mode only).
    windows: Vec<RECT>,
    hint: Vec<u16>,
    /// Drag anchor and current corner, client coordinates.
    anchor: Option<(i32, i32)>,
    corner: (i32, i32),
    /// Last known cursor position (client coordinates).
    cursor: (i32, i32),
    zoom: usize,
    /// What was last painted besides the frame (to invalidate on change).
    decor: Vec<RECT>,
    outcome: Option<SnipOutcome>,
}

thread_local! {
    static OVERLAY: RefCell<Option<Overlay>> = const { RefCell::new(None) };
}

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF(u32::from(r) | (u32::from(g) << 8) | (u32::from(b) << 16))
}

fn rect(l: i32, t: i32, r: i32, b: i32) -> RECT {
    RECT {
        left: l,
        top: t,
        right: r,
        bottom: b,
    }
}

fn inflate(r: RECT, by: i32) -> RECT {
    rect(r.left - by, r.top - by, r.right + by, r.bottom + by)
}

/// Normalized rectangle between two corners.
fn span(a: (i32, i32), b: (i32, i32)) -> RECT {
    rect(a.0.min(b.0), a.1.min(b.1), a.0.max(b.0), a.1.max(b.1))
}

/// Darkens an opaque BGRA pixel.
fn dimmed(p: u32) -> u32 {
    let ch = |shift: u32| (((p >> shift) & 0xFF) * DIM / 256) << shift;
    0xFF00_0000 | ch(16) | ch(8) | ch(0)
}

/// Where the loupe goes for a cursor at `c`: below right, flipped to stay
/// inside `bounds` (the cursor's monitor).
fn loupe_rect(c: (i32, i32), size: i32, label: i32, gap: i32, bounds: RECT) -> RECT {
    let total_h = size + label;
    let x = if c.0 + gap + size <= bounds.right {
        c.0 + gap
    } else {
        c.0 - gap - size
    };
    let y = if c.1 + gap + total_h <= bounds.bottom {
        c.1 + gap
    } else {
        c.1 - gap - total_h
    };
    rect(x, y, x + size, y + total_h)
}

impl Overlay {
    fn create(
        mode: SnipMode,
        frame: Frame,
        hint: &str,
        monitors: &[monitor::Monitor],
    ) -> Result<Self> {
        let class = wide("NookSnip");
        let title = wide("Nook Snip");
        let (x, y) = frame.origin;
        let (w, h) = (frame.width as i32, frame.height as i32);
        // SAFETY: class registration and window creation on this thread;
        // the window is owned by the returned overlay's thread.
        let hwnd = unsafe {
            let instance = GetModuleHandleW(None).api("GetModuleHandleW")?;
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(wndproc),
                hInstance: instance.into(),
                hCursor: LoadCursorW(None, IDC_CROSS).unwrap_or_default(),
                lpszClassName: PCWSTR(class.as_ptr()),
                ..Default::default()
            };
            RegisterClassExW(&wc);
            CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                PCWSTR(class.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_POPUP,
                x,
                y,
                w,
                h,
                None,
                None,
                Some(instance.into()),
                None,
            )
            .api("CreateWindowExW(snip)")?
        };
        let (bright, dim) = match Self::surfaces(mode, frame) {
            Ok(s) => s,
            Err(e) => {
                // SAFETY: our own window.
                let _ = unsafe { DestroyWindow(hwnd) };
                return Err(e);
            }
        };
        let mut screens: Vec<Screen> = monitors.iter().map(|m| Screen::new(m, (x, y))).collect();
        if screens.is_empty() {
            // No monitor list: treat the whole frame as one 100% screen.
            let all = monitor::Rect {
                left: x,
                top: y,
                right: x + w,
                bottom: y + h,
            };
            let m = monitor::Monitor {
                id: 0,
                bounds: all,
                work: all,
                dpi: 96,
            };
            screens.push(Screen::new(&m, (x, y)));
        }
        let cursor = cursor_client((x, y));
        let mut overlay = Self {
            hwnd,
            mode,
            origin: (x, y),
            size: (w, h),
            bright,
            dim,
            screens,
            screen: 0,
            windows: if mode == SnipMode::Screenshot {
                visible_windows((x, y))
            } else {
                Vec::new()
            },
            hint: hint.encode_utf16().collect(),
            anchor: None,
            corner: cursor,
            cursor,
            zoom: LOUPE_DEFAULT,
            decor: Vec::new(),
            outcome: None,
        };
        overlay.screen = overlay.screen_at(cursor);
        Ok(overlay)
    }

    /// Moves the frame's pixels into a memory DC, plus a dimmed copy for
    /// region mode.
    fn surfaces(mode: SnipMode, frame: Frame) -> Result<(Surface, Option<Surface>)> {
        let (w, h) = (frame.width as i32, frame.height as i32);
        let mut bright = Surface::new(w, h)?;
        bright.pixels().copy_from_slice(&frame.pixels);
        let dim = match mode {
            SnipMode::Region | SnipMode::Screenshot => {
                let mut d = Surface::new(w, h)?;
                for (o, &p) in d.pixels().iter_mut().zip(&frame.pixels) {
                    *o = dimmed(p);
                }
                Some(d)
            }
            SnipMode::Pick => None,
        };
        Ok((bright, dim))
    }

    /// The screen containing `p`, or the nearest one.
    fn screen_at(&self, p: (i32, i32)) -> usize {
        if let Some(i) = self.screens.iter().position(|s| contains(s.rect, p)) {
            return i;
        }
        let dist = |r: RECT| {
            let dx = (r.left - p.0).max(p.0 - r.right + 1).max(0);
            let dy = (r.top - p.1).max(p.1 - r.bottom + 1).max(0);
            i64::from(dx) * i64::from(dx) + i64::from(dy) * i64::from(dy)
        };
        (0..self.screens.len())
            .min_by_key(|&i| dist(self.screens[i].rect))
            .unwrap_or(0)
    }

    fn current(&self) -> &Screen {
        &self.screens[self.screen.min(self.screens.len() - 1)]
    }

    /// Logical pixels to physical, at the scale of the cursor's monitor.
    fn px(&self, logical: f32) -> i32 {
        (logical * self.current().scale).round() as i32
    }

    fn size(&self) -> (i32, i32) {
        self.size
    }

    fn selection(&self) -> Option<RECT> {
        self.anchor.map(|a| span(a, self.corner))
    }

    fn text_size(&self, dc: HDC, text: &[u16]) -> (i32, i32) {
        let mut s = SIZE::default();
        // SAFETY: measuring with the font selected into `dc`.
        let _ = unsafe { GetTextExtentPoint32W(dc, text, &mut s) };
        (s.cx, s.cy)
    }

    /// Rectangles of everything drawn over the frame, for the current state.
    fn decorations(&self) -> Vec<RECT> {
        let (w, h) = self.size();
        let mut out = Vec::new();
        let pad = self.px(4.0);
        match self.mode {
            SnipMode::Region | SnipMode::Screenshot => {
                if let Some(sel) = self.selection().or_else(|| self.target()) {
                    out.push(inflate(sel, pad));
                    out.push(self.size_label_rect(sel));
                }
                if self.anchor.is_none() {
                    out.push(self.hint_rect());
                }
            }
            SnipMode::Pick => {
                out.push(inflate(self.loupe(), pad));
                out.push(self.hint_rect());
            }
        }
        out.into_iter()
            .map(|r| rect(r.left.max(0), r.top.max(0), r.right.min(w), r.bottom.min(h)))
            .collect()
    }

    fn hint_rect(&self) -> RECT {
        // Generous fixed width: measured text is centered inside.
        let m = self.current().rect;
        let cx = (m.left + m.right) / 2;
        let half = self.px(260.0).min((m.right - m.left) / 2);
        let top = m.top + self.px(24.0);
        rect(cx - half, top, cx + half, top + self.px(34.0))
    }

    fn size_label_rect(&self, sel: RECT) -> RECT {
        let (lw, lh) = (self.px(96.0), self.px(24.0));
        let gap = self.px(8.0);
        let m = self.current().rect;
        let top = if sel.bottom + gap + lh <= m.bottom {
            sel.bottom + gap
        } else {
            (sel.top - gap - lh).max(m.top)
        };
        rect(sel.left, top, sel.left + lw, top + lh)
    }

    fn loupe(&self) -> RECT {
        let cells = LOUPE_CELLS[self.zoom];
        let cell = (self.px(LOUPE_SIZE as f32) / cells).max(1);
        loupe_rect(
            self.cursor,
            cell * cells,
            self.px(30.0),
            self.px(20.0),
            self.current().rect,
        )
    }

    /// Repaints whatever changed since the last call.
    fn refresh(&mut self) {
        let now = self.decorations();
        for r in self.decor.iter().chain(&now) {
            // SAFETY: invalidating part of our own window.
            let _ = unsafe { InvalidateRect(Some(self.hwnd), Some(r), false) };
        }
        self.decor = now;
    }

    fn paint(&mut self) {
        let mut ps = PAINTSTRUCT::default();
        // SAFETY: standard paint cycle on our own window. The back buffer
        // covers only the dirty rectangle; its window origin is moved so
        // everything still draws in client coordinates. Every GDI object is
        // deselected and freed before returning.
        unsafe {
            let dc = BeginPaint(self.hwnd, &mut ps);
            let r = ps.rcPaint;
            let (rw, rh) = (r.right - r.left, r.bottom - r.top);
            if rw > 0 && rh > 0 {
                let back = CreateCompatibleDC(Some(dc));
                let bitmap = CreateCompatibleBitmap(dc, rw, rh);
                let old_bitmap = SelectObject(back, bitmap.into());
                let _ = SetWindowOrgEx(back, r.left, r.top, None);
                let base = self.dim.as_ref().map_or(self.bright.dc, |d| d.dc);
                let _ = BitBlt(
                    back,
                    r.left,
                    r.top,
                    rw,
                    rh,
                    Some(base),
                    r.left,
                    r.top,
                    SRCCOPY,
                );
                let old_font = SelectObject(back, self.current().font.into());
                SetBkMode(back, TRANSPARENT);
                match self.mode {
                    SnipMode::Region | SnipMode::Screenshot => self.paint_region(back),
                    SnipMode::Pick => self.paint_pick(back),
                }
                SelectObject(back, old_font);
                let _ = BitBlt(
                    dc,
                    r.left,
                    r.top,
                    rw,
                    rh,
                    Some(back),
                    r.left,
                    r.top,
                    SRCCOPY,
                );
                SelectObject(back, old_bitmap);
                let _ = DeleteObject(bitmap.into());
                let _ = DeleteDC(back);
            }
            let _ = EndPaint(self.hwnd, &ps);
        }
    }

    /// A dark rounded label with centered white text.
    fn pill(&self, dc: HDC, r: RECT, text: &[u16]) {
        let radius = r.bottom - r.top;
        // SAFETY: drawing into our back buffer with objects created and
        // freed here.
        unsafe {
            let brush = CreateSolidBrush(rgb(24, 24, 28));
            let old_brush = SelectObject(dc, brush.into());
            let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
            let _ = RoundRect(dc, r.left, r.top, r.right, r.bottom, radius, radius);
            SelectObject(dc, old_pen);
            SelectObject(dc, old_brush);
            let _ = DeleteObject(brush.into());
            // An empty slice reaches DrawTextW as a dangling pointer, and
            // user32 reads it anyway.
            if text.is_empty() {
                return;
            }
            SetTextColor(dc, rgb(255, 255, 255));
            let mut buf = text.to_vec();
            let mut tr = r;
            DrawTextW(
                dc,
                &mut buf,
                &mut tr,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
        }
    }

    /// The hint, in a pill sized to the text, centered in `hint_rect`.
    fn paint_hint(&self, dc: HDC) {
        let area = self.hint_rect();
        let (tw, _) = self.text_size(dc, &self.hint);
        let half = (tw / 2 + self.px(16.0)).min((area.right - area.left) / 2);
        let cx = (area.left + area.right) / 2;
        self.pill(
            dc,
            rect(cx - half, area.top, cx + half, area.bottom),
            &self.hint,
        );
    }

    fn paint_region(&self, dc: HDC) {
        if let Some(sel) = self.selection().or_else(|| self.target()) {
            self.paint_box(dc, sel);
        }
        if self.anchor.is_none() {
            self.paint_hint(dc);
        }
    }

    /// The selection (or the window it would take): bright inside, framed,
    /// with its size.
    fn paint_box(&self, dc: HDC, sel: RECT) {
        let (sw, sh) = (sel.right - sel.left, sel.bottom - sel.top);
        // SAFETY: drawing into our back buffer; brushes freed here.
        unsafe {
            if sw > 0 && sh > 0 {
                let _ = BitBlt(
                    dc,
                    sel.left,
                    sel.top,
                    sw,
                    sh,
                    Some(self.bright.dc),
                    sel.left,
                    sel.top,
                    SRCCOPY,
                );
            }
            let white = CreateSolidBrush(rgb(255, 255, 255));
            let edge = inflate(sel, 1);
            FrameRect(dc, &edge, white);
            let _ = DeleteObject(white.into());
        }
        let label: Vec<u16> = format!("{sw} \u{00D7} {sh}").encode_utf16().collect();
        let lr = self.size_label_rect(sel);
        let (tw, _) = self.text_size(dc, &label);
        let lr = rect(lr.left, lr.top, lr.left + tw + self.px(20.0), lr.bottom);
        self.pill(dc, lr, &label);
    }

    fn paint_pick(&self, dc: HDC) {
        self.paint_hint(dc);
        let cells = LOUPE_CELLS[self.zoom];
        let lr = self.loupe();
        let size = lr.right - lr.left;
        let cell = size / cells;
        let half = cells / 2;
        let (cx, cy) = self.cursor;
        // SAFETY: drawing into our back buffer; every created object is
        // deselected and freed here.
        unsafe {
            SetStretchBltMode(dc, COLORONCOLOR);
            let _ = StretchBlt(
                dc,
                lr.left,
                lr.top,
                size,
                size,
                Some(self.bright.dc),
                cx - half,
                cy - half,
                cells,
                cells,
                SRCCOPY,
            );
            let old_brush = SelectObject(dc, GetStockObject(NULL_BRUSH));
            // Outline: dark outside, white inside, readable on any color.
            let frame_box = rect(lr.left, lr.top, lr.right, lr.top + size);
            for (inset, color) in [(-2, rgb(20, 20, 24)), (-1, rgb(255, 255, 255))] {
                let pen = CreatePen(PS_SOLID, 1, color);
                let old = SelectObject(dc, pen.into());
                let b = inflate(frame_box, -inset - 1);
                let _ = Rectangle(dc, b.left, b.top, b.right + 1, b.bottom + 1);
                SelectObject(dc, old);
                let _ = DeleteObject(pen.into());
            }
            // The picked cell.
            let x = lr.left + half * cell;
            let y = lr.top + half * cell;
            for (inset, color) in [(0, rgb(0, 0, 0)), (1, rgb(255, 255, 255))] {
                let pen = CreatePen(PS_SOLID, 1, color);
                let old = SelectObject(dc, pen.into());
                let _ = Rectangle(
                    dc,
                    x - 1 + inset,
                    y - 1 + inset,
                    x + cell + 1 - inset,
                    y + cell + 1 - inset,
                );
                SelectObject(dc, old);
                let _ = DeleteObject(pen.into());
            }
            SelectObject(dc, old_brush);
        }
        // Label: swatch and hex.
        let [r, g, b] = self.picked();
        let text: Vec<u16> = format!("#{r:02X}{g:02X}{b:02X}").encode_utf16().collect();
        let label = rect(lr.left, lr.top + size + self.px(6.0), lr.right, lr.bottom);
        self.pill(dc, label, &[]);
        let sw = self.px(12.0);
        let mid = (label.top + label.bottom) / 2;
        let swatch = rect(
            label.left + self.px(12.0),
            mid - sw / 2,
            label.left + self.px(12.0) + sw,
            mid + sw / 2,
        );
        // SAFETY: as above.
        unsafe {
            let brush = CreateSolidBrush(rgb(r, g, b));
            FillRect(dc, &swatch, brush);
            let _ = DeleteObject(brush.into());
            let rim = CreateSolidBrush(rgb(255, 255, 255));
            FrameRect(dc, &swatch, rim);
            let _ = DeleteObject(rim.into());
            SetTextColor(dc, rgb(255, 255, 255));
            let mut buf = text;
            let mut tr = rect(swatch.right, label.top, label.right, label.bottom);
            DrawTextW(
                dc,
                &mut buf,
                &mut tr,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE,
            );
        }
    }

    /// Screenshot mode, before dragging: the window under the cursor
    /// (clipped to the screen), or the cursor's monitor over the desktop.
    fn target(&self) -> Option<RECT> {
        if self.mode != SnipMode::Screenshot || self.anchor.is_some() {
            return None;
        }
        let (w, h) = self.size;
        let hit = self
            .windows
            .iter()
            .find(|r| contains(**r, self.cursor))
            .map(|r| rect(r.left.max(0), r.top.max(0), r.right.min(w), r.bottom.min(h)))
            .filter(|r| r.right - r.left >= MIN_SELECTION && r.bottom - r.top >= MIN_SELECTION);
        Some(hit.unwrap_or(self.current().rect))
    }

    fn picked(&self) -> [u8; 3] {
        let (w, h) = self.size;
        let x = self.cursor.0.clamp(0, w - 1) as usize;
        let y = self.cursor.1.clamp(0, h - 1) as usize;
        let p = self.bright.pixels_ref()[y * w as usize + x];
        [(p >> 16) as u8, (p >> 8) as u8, p as u8]
    }

    fn finish(&mut self, outcome: SnipOutcome) {
        self.outcome = Some(outcome);
        // SAFETY: our own window; ends the message loop.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }

    fn moved(&mut self, p: (i32, i32)) {
        self.cursor = p;
        self.screen = self.screen_at(p);
        if self.anchor.is_some() {
            self.corner = p;
        }
        self.refresh();
    }

    fn down(&mut self, p: (i32, i32)) {
        match self.mode {
            SnipMode::Region | SnipMode::Screenshot => {
                // SAFETY: capture for our own window while dragging.
                unsafe { SetCapture(self.hwnd) };
                self.anchor = Some(p);
                self.corner = p;
                self.refresh();
            }
            SnipMode::Pick => self.pick(),
        }
    }

    fn up(&mut self, p: (i32, i32)) {
        // SAFETY: releasing our own capture.
        let _ = unsafe { ReleaseCapture() };
        let Some(a) = self.anchor else {
            return;
        };
        self.corner = p;
        let sel = span(a, p);
        if sel.right - sel.left < MIN_SELECTION || sel.bottom - sel.top < MIN_SELECTION {
            // A click, not a drag: take the highlighted window, or start over.
            self.anchor = None;
            match self.target() {
                Some(t) => self.finish_rect(t),
                None => self.refresh(),
            }
            return;
        }
        self.finish_rect(sel);
    }

    /// Finishes with the part of the frozen frame inside `sel` (client
    /// coordinates).
    fn finish_rect(&mut self, sel: RECT) {
        let (ox, oy) = self.origin;
        let crop = capture::crop_pixels(
            self.bright.pixels_ref(),
            self.size.0 as u32,
            self.origin,
            monitor::Rect {
                left: ox + sel.left,
                top: oy + sel.top,
                right: ox + sel.right,
                bottom: oy + sel.bottom,
            },
        );
        match crop {
            Some(frame) => self.finish(SnipOutcome::Region {
                frame,
                single_line: shift_held(),
            }),
            None => self.finish(SnipOutcome::Cancelled),
        }
    }

    fn pick(&mut self) {
        let (ox, oy) = self.origin;
        let at = (ox + self.cursor.0, oy + self.cursor.1);
        let rgb = self.picked();
        self.finish(SnipOutcome::Color { rgb, at });
    }

    fn wheel(&mut self, delta: i16) {
        if self.mode != SnipMode::Pick {
            return;
        }
        self.zoom = if delta > 0 {
            (self.zoom + 1).min(LOUPE_CELLS.len() - 1)
        } else {
            self.zoom.saturating_sub(1)
        };
        self.refresh();
    }

    fn key(&mut self, vk: u16) {
        if vk == VK_ESCAPE.0 {
            self.finish(SnipOutcome::Cancelled);
            return;
        }
        if self.mode == SnipMode::Screenshot && self.anchor.is_none() {
            let (w, h) = self.size;
            match vk {
                v if v == u16::from(b'F') => {
                    let r = self.current().rect;
                    self.finish_rect(r);
                }
                v if v == u16::from(b'A') => self.finish_rect(rect(0, 0, w, h)),
                v if v == VK_RETURN.0 => {
                    if let Some(t) = self.target() {
                        self.finish_rect(t);
                    }
                }
                _ => {}
            }
            return;
        }
        if self.mode != SnipMode::Pick {
            return;
        }
        if vk == VK_RETURN.0 {
            self.pick();
            return;
        }
        let step = if shift_held() { 10 } else { 1 };
        let (dx, dy) = match vk {
            v if v == VK_LEFT.0 => (-step, 0),
            v if v == VK_RIGHT.0 => (step, 0),
            v if v == VK_UP.0 => (0, -step),
            v if v == VK_DOWN.0 => (0, step),
            _ => return,
        };
        let (w, h) = self.size();
        let (ox, oy) = self.origin;
        let x = (self.cursor.0 + dx).clamp(0, w - 1);
        let y = (self.cursor.1 + dy).clamp(0, h - 1);
        // SAFETY: plain cursor move; the WM_MOUSEMOVE it causes updates us.
        let _ = unsafe { SetCursorPos(ox + x, oy + y) };
        self.moved((x, y));
    }
}

/// Visible, uncloaked top-level windows of other processes, front to back,
/// as their visible frames (without the invisible resize borders), in
/// client coordinates of an overlay at `origin`.
fn visible_windows(origin: (i32, i32)) -> Vec<RECT> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `Vec` passed to `EnumWindows` below,
        // alive for the whole enumeration.
        let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
        list.push(hwnd);
        BOOL(1)
    }
    let mut all: Vec<HWND> = Vec::new();
    // SAFETY: the callback only touches `all` through `lparam`.
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut all as *mut _ as isize)) };
    // SAFETY: plain query.
    let own = unsafe { GetCurrentProcessId() };
    let mut out = Vec::new();
    for hwnd in all {
        // SAFETY: queries on window handles from the enumeration; a window
        // that vanished meanwhile just fails them.
        unsafe {
            if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
                continue;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == own {
                continue;
            }
            let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TRANSPARENT.0 != 0 {
                continue;
            }
            let mut cloaked = 0u32;
            if DwmGetWindowAttribute(
                hwnd,
                DWMWA_CLOAKED,
                (&mut cloaked as *mut u32).cast(),
                std::mem::size_of::<u32>() as u32,
            )
            .is_ok()
                && cloaked != 0
            {
                continue;
            }
            let mut class = [0u16; 32];
            let n = GetClassNameW(hwnd, &mut class) as usize;
            let class = String::from_utf16_lossy(&class[..n.min(class.len())]);
            if class == "Progman" || class == "WorkerW" {
                continue;
            }
            let mut r = RECT::default();
            let framed = DwmGetWindowAttribute(
                hwnd,
                DWMWA_EXTENDED_FRAME_BOUNDS,
                (&mut r as *mut RECT).cast(),
                std::mem::size_of::<RECT>() as u32,
            )
            .is_ok();
            if !framed && GetWindowRect(hwnd, &mut r).is_err() {
                continue;
            }
            if r.right - r.left < MIN_SELECTION || r.bottom - r.top < MIN_SELECTION {
                continue;
            }
            out.push(rect(
                r.left - origin.0,
                r.top - origin.1,
                r.right - origin.0,
                r.bottom - origin.1,
            ));
        }
    }
    out
}

fn shift_held() -> bool {
    // SAFETY: plain key state query.
    unsafe { GetKeyState(i32::from(VK_SHIFT.0)) < 0 }
}

fn cursor_client(origin: (i32, i32)) -> (i32, i32) {
    let mut p = POINT::default();
    // SAFETY: valid out pointer.
    let _ = unsafe { GetCursorPos(&mut p) };
    (p.x - origin.0, p.y - origin.1)
}

fn pump() {
    let mut msg = MSG::default();
    // SAFETY: standard message loop for this thread's windows.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

fn point(lparam: LPARAM) -> (i32, i32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16;
    (i32::from(x), i32::from(y))
}

fn with_overlay(f: impl FnOnce(&mut Overlay)) {
    OVERLAY.with(|o| {
        if let Ok(mut o) = o.try_borrow_mut()
            && let Some(overlay) = o.as_mut()
        {
            f(overlay);
        }
    });
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_PAINT => {
            let mut painted = false;
            with_overlay(|o| {
                o.paint();
                painted = true;
            });
            if !painted {
                // Before the overlay is stored: validate with a default paint.
                // SAFETY: default handling.
                return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
            }
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_SETCURSOR => {
            // SAFETY: the shared system cross cursor.
            unsafe { SetCursor(LoadCursorW(None, IDC_CROSS).ok()) };
            return LRESULT(1);
        }
        WM_MOUSEMOVE => with_overlay(|o| o.moved(point(lparam))),
        WM_LBUTTONDOWN => with_overlay(|o| o.down(point(lparam))),
        WM_LBUTTONUP => with_overlay(|o| o.up(point(lparam))),
        WM_RBUTTONDOWN => with_overlay(|o| o.finish(SnipOutcome::Cancelled)),
        WM_MOUSEWHEEL => with_overlay(|o| o.wheel((wparam.0 >> 16) as u16 as i16)),
        WM_KEYDOWN => with_overlay(|o| o.key(wparam.0 as u16)),
        WM_CLOSE => {
            // SAFETY: our own window.
            let _ = unsafe { DestroyWindow(hwnd) };
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
        }
        // SAFETY: default handling for everything else.
        _ => return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
    LRESULT(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spans_any_drag_direction() {
        let r = span((50, 40), (10, 90));
        assert_eq!((r.left, r.top, r.right, r.bottom), (10, 40, 50, 90));
    }

    #[test]
    fn dims_each_channel() {
        assert_eq!(dimmed(0xFFFF_FFFF), 0xFF77_7777);
        assert_eq!(dimmed(0xFF00_0000), 0xFF00_0000);
    }

    #[test]
    fn loupe_stays_on_screen() {
        // Room below right.
        let left = rect(0, 0, 1920, 1080);
        let r = loupe_rect((100, 100), 150, 30, 20, left);
        assert_eq!((r.left, r.top), (120, 120));
        // Bottom-right corner: flips up and left.
        let r = loupe_rect((1900, 1070), 150, 30, 20, left);
        assert_eq!((r.right, r.bottom), (1880, 1050));
        // At the right edge of the left monitor it flips rather than
        // straddling the seam to the next monitor.
        let r = loupe_rect((1850, 100), 150, 30, 20, left);
        assert!(r.right <= 1920);
        // On a monitor to the right it stays on that monitor.
        let r = loupe_rect((2000, 100), 150, 30, 20, rect(1920, 0, 3520, 900));
        assert_eq!(r.left, 2020);
    }
}
