//! The draw-on-screen canvas: a native, truly transparent ink layer.
//!
//! A second OpenGL viewport cannot be made see-through reliably (it
//! composites black), so the canvas is a classic per-pixel-alpha layered
//! window: strokes are rasterized (anti-aliased capsules) into a 32-bit
//! premultiplied DIB and pushed with `UpdateLayeredWindow`. Untouched
//! pixels have alpha 1, invisible but still hit-testable, so the canvas
//! catches the pen everywhere while the screen shows through.
//!
//! The window lives on its own thread with its own message loop. Esc closes
//! it, Ctrl+Z undoes; the toolbar (an egui window owned by the canvas)
//! changes the style through [`InkCanvas`].

use crate::{PlatformError, Result, Win32Context, wide};
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM};
use windows::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, HBITMAP, HDC,
    HGDIOBJ, SelectObject,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_ESCAPE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, IDC_CROSS,
    LoadCursorW, MSG, PostMessageW, PostQuitMessage, RegisterClassExW, SW_SHOW,
    SetForegroundWindow, ShowWindow, TranslateMessage, ULW_ALPHA, UPDATELAYEREDWINDOWINFO,
    UpdateLayeredWindowIndirect, WM_APP, WM_CLOSE, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN,
    WM_LBUTTONUP, WM_MOUSEMOVE, WNDCLASSEXW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    WS_POPUP,
};
use windows::core::PCWSTR;

/// Untouched pixels: premultiplied black at alpha 1 (invisible, clickable).
const BACKGROUND: u32 = 0x0100_0000;
/// Eraser radius in pixels.
const ERASER: f32 = 12.0;

const WM_UNDO: u32 = WM_APP + 1;
const WM_CLEAR: u32 = WM_APP + 2;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum InkTool {
    /// Draws with `color` (unpremultiplied RGBA) and `width` (pixels).
    Pen {
        color: [u8; 4],
        width: f32,
    },
    Eraser,
}

struct Shared {
    tool: Mutex<InkTool>,
    open: AtomicBool,
}

/// Handle to an open canvas. Dropping it closes the canvas.
pub struct InkCanvas {
    hwnd: isize,
    shared: Arc<Shared>,
}

impl InkCanvas {
    /// Opens a canvas over `bounds` (physical pixels: x, y, w, h).
    /// `on_close` runs on the canvas thread when it closes (Esc or drop).
    pub fn open(
        bounds: (i32, i32, i32, i32),
        tool: InkTool,
        on_close: impl Fn() + Send + 'static,
    ) -> Result<Self> {
        let shared = Arc::new(Shared {
            tool: Mutex::new(tool),
            open: AtomicBool::new(true),
        });
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let thread_shared = shared.clone();
        std::thread::Builder::new()
            .name("nook-ink".into())
            .spawn(move || {
                let canvas = match Canvas::create(bounds, thread_shared.clone()) {
                    Ok(c) => c,
                    Err(e) => {
                        thread_shared.open.store(false, Ordering::Relaxed);
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let hwnd = canvas.hwnd;
                canvas.present(None);
                CANVAS.with(|c| *c.borrow_mut() = Some(canvas));
                // SAFETY: our own window, on its own thread.
                unsafe {
                    let _ = ShowWindow(hwnd, SW_SHOW);
                    let _ = SetForegroundWindow(hwnd);
                }
                let _ = ready_tx.send(Ok(hwnd.0 as isize));
                pump();
                CANVAS.with(|c| c.borrow_mut().take());
                thread_shared.open.store(false, Ordering::Relaxed);
                on_close();
            })
            .map_err(|e| PlatformError::Other(format!("spawning ink thread: {e}")))?;
        let hwnd = ready_rx
            .recv()
            .map_err(|_| PlatformError::Other("ink thread exited".into()))??;
        Ok(Self { hwnd, shared })
    }

    pub fn hwnd(&self) -> crate::Hwnd {
        crate::Hwnd::from_raw(self.hwnd)
    }

    pub fn is_open(&self) -> bool {
        self.shared.open.load(Ordering::Relaxed)
    }

    pub fn set_tool(&self, tool: InkTool) {
        *self.shared.tool.lock().unwrap_or_else(|p| p.into_inner()) = tool;
    }

    pub fn undo(&self) {
        self.post(WM_UNDO);
    }

    pub fn clear(&self) {
        self.post(WM_CLEAR);
    }

    /// Brings the canvas to the foreground (so Esc reaches it).
    pub fn focus(&self) {
        // SAFETY: plain call on our own window.
        let _ = unsafe { SetForegroundWindow(self.raw()) };
    }

    fn raw(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }

    fn post(&self, msg: u32) {
        // SAFETY: posting to our canvas window; fails harmlessly once closed.
        let _ = unsafe { PostMessageW(Some(self.raw()), msg, WPARAM(0), LPARAM(0)) };
    }
}

impl Drop for InkCanvas {
    fn drop(&mut self) {
        self.post(WM_CLOSE);
    }
}

struct Stroke {
    points: Vec<(f32, f32)>,
    /// Premultiplied BGRA.
    color: u32,
    width: f32,
}

/// One undoable step.
enum Action {
    Draw,
    /// Strokes removed by one eraser drag, with the index each had when it
    /// was removed; put back in reverse order they land where they were.
    Erase(Vec<(usize, Stroke)>),
    Clear(Vec<Stroke>),
}

/// The strokes and the history that undo walks back through: drawing,
/// erasing and clearing are each one step.
#[derive(Default)]
struct Ink {
    strokes: Vec<Stroke>,
    history: Vec<Action>,
    /// Strokes erased by the drag in progress.
    erasing: Vec<(usize, Stroke)>,
}

impl Ink {
    fn draw(&mut self, stroke: Stroke) {
        self.strokes.push(stroke);
        self.history.push(Action::Draw);
    }

    /// Removes the strokes under `p`. True if any was removed.
    fn erase_at(&mut self, p: (f32, f32), radius: f32) -> bool {
        let mut removed = false;
        let mut i = 0;
        while i < self.strokes.len() {
            if self.strokes[i].hits(p, radius) {
                self.erasing.push((i, self.strokes.remove(i)));
                removed = true;
            } else {
                i += 1;
            }
        }
        removed
    }

    /// Ends an eraser drag: what it removed becomes one undo step.
    fn end_erase(&mut self) {
        if !self.erasing.is_empty() {
            self.history.push(Action::Erase(std::mem::take(&mut self.erasing)));
        }
    }

    fn clear(&mut self) -> bool {
        self.end_erase();
        if self.strokes.is_empty() {
            return false;
        }
        self.history.push(Action::Clear(std::mem::take(&mut self.strokes)));
        true
    }

    /// Reverts the last step. True if anything changed.
    fn undo(&mut self) -> bool {
        self.end_erase();
        match self.history.pop() {
            None => false,
            Some(Action::Draw) => self.strokes.pop().is_some(),
            Some(Action::Erase(removed)) => {
                for (i, s) in removed.into_iter().rev() {
                    let i = i.min(self.strokes.len());
                    self.strokes.insert(i, s);
                }
                true
            }
            Some(Action::Clear(strokes)) => {
                self.strokes = strokes;
                true
            }
        }
    }
}

impl Stroke {
    fn hits(&self, p: (f32, f32), radius: f32) -> bool {
        let r = radius + self.width * 0.5;
        if self.points.len() == 1 {
            return dist(p, self.points[0]) <= r;
        }
        self.points
            .windows(2)
            .any(|w| segment_distance(p, w[0], w[1]) <= r)
    }
}

fn dist(a: (f32, f32), b: (f32, f32)) -> f32 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

fn segment_distance(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (abx, aby) = (b.0 - a.0, b.1 - a.1);
    let len2 = abx * abx + aby * aby;
    if len2 <= f32::EPSILON {
        return dist(p, a);
    }
    let t = (((p.0 - a.0) * abx + (p.1 - a.1) * aby) / len2).clamp(0.0, 1.0);
    dist(p, (a.0 + abx * t, a.1 + aby * t))
}

/// Unpremultiplied RGBA -> premultiplied BGRA (`0xAARRGGBB`).
fn premultiply([r, g, b, a]: [u8; 4]) -> u32 {
    let m = |c: u8| (u32::from(c) * u32::from(a) + 127) / 255;
    (u32::from(a) << 24) | (m(r) << 16) | (m(g) << 8) | m(b)
}

/// `src` scaled by `cov` (0..=255), premultiplied, composited over `dst`.
fn over(dst: u32, src: u32, cov: u8) -> u32 {
    let k = u32::from(cov);
    let s = |shift: u32| ((src >> shift) & 0xFF) * k / 255;
    let sa = s(24);
    let inv = 255 - sa;
    let d = |shift: u32| (dst >> shift) & 0xFF;
    let ch = |shift: u32| (s(shift) + d(shift) * inv / 255).min(255);
    (ch(24) << 24) | (ch(16) << 16) | (ch(8) << 8) | ch(0)
}

/// Pixel surface: the DIB the window shows, plus scratch buffers for the
/// stroke in progress.
struct Surface {
    w: i32,
    h: i32,
    /// Points into the DIB section (top-down, `w * h` pixels).
    bits: *mut u32,
    /// The canvas before the current stroke began.
    base: Vec<u32>,
    /// Coverage of the current stroke (max over its segments).
    cov: Vec<u8>,
}

impl Surface {
    fn pixels(&mut self) -> &mut [u32] {
        // SAFETY: `bits` is the DIB section's buffer of exactly w * h pixels,
        // alive as long as the bitmap (owned by `Canvas`), and only touched
        // from the canvas thread.
        unsafe { std::slice::from_raw_parts_mut(self.bits, (self.w * self.h) as usize) }
    }

    fn begin_stroke(&mut self) {
        let snapshot = self.pixels().to_vec();
        self.base = snapshot;
        self.cov.iter_mut().for_each(|c| *c = 0);
    }

    /// Adds one capsule from `a` to `b` to the current stroke and repaints
    /// the pixels it covers. Returns the rectangle it touched.
    fn segment(&mut self, a: (f32, f32), b: (f32, f32), width: f32, color: u32) -> RECT {
        let r = width * 0.5;
        let pad = r + 1.0;
        let x0 = ((a.0.min(b.0) - pad).floor() as i32).clamp(0, self.w);
        let y0 = ((a.1.min(b.1) - pad).floor() as i32).clamp(0, self.h);
        let x1 = ((a.0.max(b.0) + pad).ceil() as i32).clamp(0, self.w);
        let y1 = ((a.1.max(b.1) + pad).ceil() as i32).clamp(0, self.h);
        let w = self.w;
        for y in y0..y1 {
            for x in x0..x1 {
                let d = segment_distance((x as f32 + 0.5, y as f32 + 0.5), a, b);
                let c = ((r + 0.5 - d).clamp(0.0, 1.0) * 255.0) as u8;
                if c == 0 {
                    continue;
                }
                let i = (y * w + x) as usize;
                if c > self.cov[i] {
                    self.cov[i] = c;
                    let px = over(self.base[i], color, c);
                    self.pixels()[i] = px;
                }
            }
        }
        RECT {
            left: x0,
            top: y0,
            right: x1,
            bottom: y1,
        }
    }

    fn reset(&mut self) {
        self.pixels().fill(BACKGROUND);
    }
}

struct Canvas {
    hwnd: HWND,
    origin: POINT,
    dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    surface: Surface,
    ink: Ink,
    current: Option<Stroke>,
    erasing: bool,
    shared: Arc<Shared>,
}

thread_local! {
    static CANVAS: RefCell<Option<Canvas>> = const { RefCell::new(None) };
}

impl Canvas {
    fn create((x, y, w, h): (i32, i32, i32, i32), shared: Arc<Shared>) -> Result<Self> {
        let class = wide("NookInk");
        let title = wide("Nook Ink");
        // SAFETY: class registration, window and DIB creation on this
        // thread; every handle is owned by the returned `Canvas` and freed
        // in its `Drop`.
        unsafe {
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
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
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
            .api("CreateWindowExW(ink)")?;

            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h, // top-down
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let dc = CreateCompatibleDC(None);
            let mut bits = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            {
                Ok(b) if !bits.is_null() => b,
                other => {
                    let _ = DeleteDC(dc);
                    let _ = DestroyWindow(hwnd);
                    return Err(match other {
                        Err(e) => PlatformError::Win32 {
                            call: "CreateDIBSection",
                            source: e,
                        },
                        Ok(_) => PlatformError::Other("CreateDIBSection: no bits".into()),
                    });
                }
            };
            let old = SelectObject(dc, bitmap.into());
            let n = (w * h) as usize;
            let mut surface = Surface {
                w,
                h,
                bits: bits.cast(),
                base: vec![BACKGROUND; n],
                cov: vec![0; n],
            };
            surface.reset();
            Ok(Self {
                hwnd,
                origin: POINT { x, y },
                dc,
                bitmap,
                old,
                surface,
                ink: Ink::default(),
                current: None,
                erasing: false,
                shared,
            })
        }
    }

    /// Pushes the canvas to the screen. Only `dirty` is copied when given:
    /// the canvas can span every monitor, and a pen move touches a few
    /// pixels of it.
    fn present(&self, dirty: Option<RECT>) {
        let size = SIZE {
            cx: self.surface.w,
            cy: self.surface.h,
        };
        let src = POINT { x: 0, y: 0 };
        let blend = BLENDFUNCTION {
            BlendOp: AC_SRC_OVER as u8,
            BlendFlags: 0,
            SourceConstantAlpha: 255,
            AlphaFormat: AC_SRC_ALPHA as u8,
        };
        let info = UPDATELAYEREDWINDOWINFO {
            cbSize: std::mem::size_of::<UPDATELAYEREDWINDOWINFO>() as u32,
            hdcDst: HDC::default(),
            pptDst: &self.origin,
            psize: &size,
            hdcSrc: self.dc,
            pptSrc: &src,
            crKey: COLORREF(0),
            pblend: &blend,
            dwFlags: ULW_ALPHA,
            prcDirty: dirty
                .as_ref()
                .map_or(std::ptr::null(), |r| r as *const RECT),
        };
        // SAFETY: our layered window and the memory DC holding its bitmap;
        // every pointer in `info` refers to a local that outlives the call.
        if let Err(e) = unsafe { UpdateLayeredWindowIndirect(self.hwnd, &info) }.ok() {
            tracing::warn!("UpdateLayeredWindowIndirect: {e}");
        }
    }

    fn tool(&self) -> InkTool {
        *self.shared.tool.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Repaints every stroke from scratch (after undo, erase or clear).
    fn redraw(&mut self) {
        self.surface.reset();
        for s in &self.ink.strokes {
            self.surface.begin_stroke();
            draw_points(&mut self.surface, &s.points, s.width, s.color);
        }
        self.present(None);
    }

    fn erase_at(&mut self, p: (f32, f32)) {
        if self.ink.erase_at(p, ERASER) {
            self.redraw();
        }
    }

    fn undo(&mut self) {
        if self.ink.undo() {
            self.redraw();
        }
    }

    fn down(&mut self, p: (f32, f32)) {
        // SAFETY: capture for our own window while the button is held.
        unsafe { SetCapture(self.hwnd) };
        match self.tool() {
            InkTool::Eraser => {
                self.erasing = true;
                self.erase_at(p);
            }
            InkTool::Pen { color, width } => {
                let color = premultiply(color);
                self.surface.begin_stroke();
                let dirty = self.surface.segment(p, p, width, color);
                self.current = Some(Stroke {
                    points: vec![p],
                    color,
                    width,
                });
                self.present(Some(dirty));
            }
        }
    }

    fn moved(&mut self, p: (f32, f32)) {
        if self.erasing {
            self.erase_at(p);
            return;
        }
        let Some(s) = &mut self.current else {
            return;
        };
        let Some(&last) = s.points.last() else {
            return;
        };
        if dist(last, p) < 1.0 {
            return;
        }
        s.points.push(p);
        let dirty = self.surface.segment(last, p, s.width, s.color);
        self.present(Some(dirty));
    }

    fn up(&mut self) {
        // SAFETY: releasing our own capture.
        let _ = unsafe { ReleaseCapture() };
        self.erasing = false;
        self.ink.end_erase();
        if let Some(s) = self.current.take() {
            self.ink.draw(s);
        }
    }
}

fn draw_points(surface: &mut Surface, points: &[(f32, f32)], width: f32, color: u32) {
    match points {
        [] => {}
        [p] => {
            surface.segment(*p, *p, width, color);
        }
        _ => {
            for w in points.windows(2) {
                surface.segment(w[0], w[1], width, color);
            }
        }
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        // SAFETY: freeing the GDI objects created in `create`.
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteDC(self.dc);
        }
    }
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

fn point(lparam: LPARAM) -> (f32, f32) {
    let x = (lparam.0 & 0xFFFF) as u16 as i16;
    let y = ((lparam.0 >> 16) & 0xFFFF) as u16 as i16;
    (f32::from(x), f32::from(y))
}

fn with_canvas(f: impl FnOnce(&mut Canvas)) {
    CANVAS.with(|c| {
        if let Ok(mut c) = c.try_borrow_mut()
            && let Some(canvas) = c.as_mut()
        {
            f(canvas);
        }
    });
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_LBUTTONDOWN => with_canvas(|c| c.down(point(lparam))),
        WM_MOUSEMOVE => with_canvas(|c| c.moved(point(lparam))),
        WM_LBUTTONUP => with_canvas(Canvas::up),
        WM_UNDO => with_canvas(Canvas::undo),
        WM_CLEAR => with_canvas(|c| {
            if c.ink.clear() {
                c.redraw();
            }
        }),
        WM_KEYDOWN if wparam.0 == usize::from(VK_ESCAPE.0) => {
            // SAFETY: our own window.
            let _ = unsafe { DestroyWindow(hwnd) };
        }
        WM_KEYDOWN
            if wparam.0 == usize::from(b'Z')
                // SAFETY: plain key state query.
                && unsafe { GetKeyState(i32::from(VK_CONTROL.0)) } < 0 =>
        {
            with_canvas(Canvas::undo);
        }
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
    fn premultiplies_and_composites() {
        assert_eq!(premultiply([255, 0, 0, 255]), 0xFFFF_0000);
        assert_eq!(premultiply([255, 255, 255, 0]), 0);
        // Opaque red fully covering the background.
        assert_eq!(over(BACKGROUND, 0xFFFF_0000, 255), 0xFFFF_0000);
        // Half coverage keeps some of what is below.
        let half = over(0xFF00_00FF, 0xFFFF_0000, 128);
        assert_eq!(half >> 24, 0xFF);
        assert!((half >> 16) & 0xFF > 100 && half & 0xFF > 100);
    }

    #[test]
    fn segment_coverage_is_antialiased() {
        let n = 20 * 20;
        let mut bits = vec![BACKGROUND; n];
        let mut s = Surface {
            w: 20,
            h: 20,
            bits: bits.as_mut_ptr(),
            base: vec![BACKGROUND; n],
            cov: vec![0; n],
        };
        s.begin_stroke();
        s.segment((2.0, 10.3), (18.0, 10.3), 4.0, 0xFFFF_FFFF);
        let center = s.pixels()[10 * 20 + 10];
        let edge = s.pixels()[12 * 20 + 10];
        let outside = s.pixels()[16 * 20 + 10];
        assert_eq!(center, 0xFFFF_FFFF);
        assert!(edge >> 24 > 0 && edge >> 24 < 255, "edge {edge:08X}");
        assert_eq!(outside, BACKGROUND);
    }

    fn line(y: f32) -> Stroke {
        Stroke {
            points: vec![(0.0, y), (100.0, y)],
            color: 0,
            width: 4.0,
        }
    }

    fn ys(ink: &Ink) -> Vec<f32> {
        ink.strokes.iter().map(|s| s.points[0].1).collect()
    }

    #[test]
    fn undo_after_erase_brings_the_stroke_back() {
        let mut ink = Ink::default();
        for y in [0.0, 100.0, 200.0] {
            ink.draw(line(y));
        }
        assert!(ink.erase_at((50.0, 100.0), 10.0));
        ink.end_erase();
        assert_eq!(ys(&ink), [0.0, 200.0]);
        // The erased stroke returns in its place; the last drawn one stays.
        assert!(ink.undo());
        assert_eq!(ys(&ink), [0.0, 100.0, 200.0]);
        // Then undo goes on through the drawing.
        assert!(ink.undo());
        assert_eq!(ys(&ink), [0.0, 100.0]);
    }

    #[test]
    fn one_eraser_drag_is_one_undo_step() {
        let mut ink = Ink::default();
        for y in [0.0, 100.0, 200.0] {
            ink.draw(line(y));
        }
        ink.erase_at((50.0, 0.0), 10.0);
        ink.erase_at((50.0, 200.0), 10.0);
        ink.end_erase();
        assert_eq!(ys(&ink), [100.0]);
        assert!(ink.undo());
        assert_eq!(ys(&ink), [0.0, 100.0, 200.0]);
    }

    #[test]
    fn clear_can_be_undone() {
        let mut ink = Ink::default();
        ink.draw(line(0.0));
        ink.draw(line(100.0));
        assert!(ink.clear());
        assert!(ink.strokes.is_empty());
        assert!(!ink.clear(), "clearing an empty canvas is not a step");
        assert!(ink.undo());
        assert_eq!(ys(&ink), [0.0, 100.0]);
    }

    #[test]
    fn undo_on_empty_history_does_nothing() {
        let mut ink = Ink::default();
        assert!(!ink.undo());
        assert!(!ink.erase_at((0.0, 0.0), 10.0));
        ink.end_erase();
        assert!(ink.history.is_empty());
    }

    #[test]
    fn eraser_hits_segments() {
        let s = Stroke {
            points: vec![(0.0, 0.0), (100.0, 0.0)],
            color: 0,
            width: 4.0,
        };
        assert!(s.hits((50.0, 8.0), 10.0));
        assert!(!s.hits((50.0, 30.0), 10.0));
    }
}
