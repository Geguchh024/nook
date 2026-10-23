//! Clipboard watching and writing for the clipboard history.
//!
//! A worker thread owns a message-only window registered with
//! `AddClipboardFormatListener`, so Windows wakes it on every copy and it
//! sleeps otherwise. It reads Unicode text or a device-independent bitmap,
//! skips content that apps flag as private (password managers set
//! `ExcludeClipboardContentFromMonitorProcessing` or
//! `CanIncludeInClipboardHistory = 0`), and sends entries to the UI. Writes
//! go through the same thread, since the clipboard is opened per window.
//!
//! Nothing is persisted: the history lives in memory only.

use crate::{PlatformError, Result, Win32Context, wide};
use std::cell::RefCell;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    RemoveClipboardFormatListener, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::{CF_DIB, CF_UNICODETEXT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE,
    KillTimer, MSG, PostMessageW, PostQuitMessage, RegisterClassExW, SetTimer, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WM_CLIPBOARDUPDATE, WM_CLOSE, WM_DESTROY, WM_TIMER, WNDCLASSEXW,
};
use windows::core::PCWSTR;

/// Longest text kept, in UTF-16 units (about 1 MB).
const MAX_TEXT: usize = 512 * 1024;
/// Largest thumbnail edge, in pixels.
pub const THUMB_SIZE: u32 = 240;
/// Largest bitmap kept, in bytes (a 4K screenshot is about 33 MB).
const MAX_DIB: usize = 48 * 1024 * 1024;

const WM_WRITE: u32 = WM_APP + 1;
/// Timer that defers reading after a change: apps copying through OLE
/// reopen the clipboard right after announcing it, and reading at once
/// makes their copy fail ("clipboard busy").
const READ_TIMER: usize = 1;
const READ_DELAY_MS: u32 = 120;

/// A copied bitmap: the original DIB (for pasting back) and a thumbnail.
#[derive(Debug)]
pub struct ClipImage {
    pub width: u32,
    pub height: u32,
    /// Unpremultiplied RGBA8, at most [`THUMB_SIZE`] on its longest edge.
    pub thumb: Vec<u8>,
    pub thumb_size: [usize; 2],
    dib: Vec<u8>,
}

impl PartialEq for ClipImage {
    fn eq(&self, other: &Self) -> bool {
        self.dib == other.dib
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ClipContent {
    Text(String),
    Image(Arc<ClipImage>),
}

/// Handle to the watcher thread. Dropping it stops the thread.
pub struct ClipboardWatcher {
    rx: Receiver<ClipContent>,
    writes: Sender<ClipContent>,
    hwnd: isize,
}

impl ClipboardWatcher {
    /// Starts watching. `on_change` runs (on the worker thread) after each
    /// new entry.
    pub fn spawn(on_change: impl Fn() + Send + 'static) -> Result<Self> {
        let (tx, rx) = mpsc::channel();
        let (writes, write_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        std::thread::Builder::new()
            .name("nook-clipboard".into())
            .spawn(move || {
                let hwnd = match create_window() {
                    Ok(h) => h,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                STATE.with(|s| {
                    *s.borrow_mut() = Some(State {
                        tx,
                        writes: write_rx,
                        on_change: Box::new(on_change),
                    })
                });
                let _ = ready_tx.send(Ok(hwnd.0 as isize));
                pump();
                // SAFETY: our own window, on its own thread.
                unsafe {
                    let _ = RemoveClipboardFormatListener(hwnd);
                }
            })
            .map_err(|e| PlatformError::Other(format!("spawning clipboard thread: {e}")))?;
        let hwnd = ready_rx
            .recv()
            .map_err(|_| PlatformError::Other("clipboard thread exited".into()))??;
        Ok(Self { rx, writes, hwnd })
    }

    /// Entries copied since the last call, oldest first.
    pub fn poll(&self) -> Vec<ClipContent> {
        self.rx.try_iter().collect()
    }

    /// Puts `content` back on the clipboard.
    pub fn set(&self, content: ClipContent) {
        if self.writes.send(content).is_ok() {
            // SAFETY: posting to our worker's window; fails harmlessly if gone.
            let _ = unsafe { PostMessageW(Some(self.hwnd()), WM_WRITE, WPARAM(0), LPARAM(0)) };
        }
    }

    fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut core::ffi::c_void)
    }
}

impl Drop for ClipboardWatcher {
    fn drop(&mut self) {
        // SAFETY: as above.
        let _ = unsafe { PostMessageW(Some(self.hwnd()), WM_CLOSE, WPARAM(0), LPARAM(0)) };
    }
}

/// Puts `text` on the clipboard from any thread, without a watcher. A
/// short-lived message-only window owns the write (the clipboard refuses
/// writes opened without one).
pub fn copy_text(text: &str) -> Result<()> {
    with_writer(|hwnd| write(hwnd, &ClipContent::Text(text.to_owned())))
}

/// The text on the clipboard, if any, from any thread.
pub fn read_text() -> Option<String> {
    let mut out = None;
    let _ = with_writer(|hwnd| {
        if let Some(ClipContent::Text(t)) = read(hwnd)? {
            out = Some(t);
        }
        Ok(())
    });
    out
}

/// Puts an image on the clipboard as a DIB (what most apps paste) and as
/// PNG (what browsers and chat apps prefer), from any thread.
pub fn copy_image(frame: &crate::capture::Frame) -> Result<()> {
    let png = frame.encode_png()?;
    let dib = frame_dib(frame);
    with_writer(|hwnd| {
        let _open = Open::new(hwnd)?;
        // SAFETY: the clipboard is open for our window.
        unsafe { EmptyClipboard() }.api("EmptyClipboard")?;
        put(u32::from(CF_DIB.0), &dib)?;
        let png_format = format_id("PNG");
        if png_format != 0 {
            put(png_format, &png)?;
        }
        Ok(())
    })
}

/// A packed bottom-up 32-bit DIB of `frame`.
fn frame_dib(frame: &crate::capture::Frame) -> Vec<u8> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    let mut dib = Vec::with_capacity(40 + w * h * 4);
    dib.extend_from_slice(&40u32.to_le_bytes());
    dib.extend_from_slice(&(w as i32).to_le_bytes());
    dib.extend_from_slice(&(h as i32).to_le_bytes());
    dib.extend_from_slice(&1u16.to_le_bytes());
    dib.extend_from_slice(&32u16.to_le_bytes());
    dib.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    dib.extend_from_slice(&((w * h * 4) as u32).to_le_bytes());
    dib.extend_from_slice(&[0; 16]); // resolution, palette
    for row in frame.pixels.chunks_exact(w.max(1)).rev() {
        for p in row {
            dib.extend_from_slice(&p.to_le_bytes());
        }
    }
    dib
}

/// Runs `f` with a short-lived message-only window to own clipboard writes.
fn with_writer(f: impl FnOnce(HWND) -> Result<()>) -> Result<()> {
    let class = wide("STATIC");
    // SAFETY: a message-only window of a system class, created and
    // destroyed on this thread around the write.
    unsafe {
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            None,
            None,
        )
        .api("CreateWindowExW(clipboard writer)")?;
        let res = f(hwnd);
        let _ = DestroyWindow(hwnd);
        res
    }
}

struct State {
    tx: Sender<ClipContent>,
    writes: Receiver<ClipContent>,
    on_change: Box<dyn Fn()>,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

fn create_window() -> Result<HWND> {
    let class = wide("NookClipboard");
    // SAFETY: registering a class and creating a message-only window owned
    // by this thread; the strings outlive the calls.
    unsafe {
        let instance = GetModuleHandleW(None).api("GetModuleHandleW")?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: PCWSTR(class.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(class.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
        .api("CreateWindowExW(clipboard)")?;
        AddClipboardFormatListener(hwnd).api("AddClipboardFormatListener")?;
        Ok(hwnd)
    }
}

fn pump() {
    let mut msg = MSG::default();
    // SAFETY: standard message loop for this thread's windows.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            DispatchMessageW(&msg);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_CLIPBOARDUPDATE => {
            // SAFETY: (re)arming a timer on our own window coalesces bursts.
            unsafe { SetTimer(Some(hwnd), READ_TIMER, READ_DELAY_MS, None) };
            LRESULT(0)
        }
        WM_TIMER if wparam.0 == READ_TIMER => {
            // SAFETY: our own timer.
            let _ = unsafe { KillTimer(Some(hwnd), READ_TIMER) };
            match read(hwnd) {
                Ok(Some(content)) => STATE.with(|s| {
                    if let Some(s) = &*s.borrow()
                        && s.tx.send(content).is_ok()
                    {
                        (s.on_change)();
                    }
                }),
                Ok(None) => {}
                Err(e) => tracing::debug!("reading clipboard: {e}"),
            }
            LRESULT(0)
        }
        WM_WRITE => {
            let pending: Vec<ClipContent> = STATE.with(|s| {
                s.borrow()
                    .as_ref()
                    .map(|s| s.writes.try_iter().collect())
                    .unwrap_or_default()
            });
            if let Some(content) = pending.into_iter().last()
                && let Err(e) = write(hwnd, &content)
            {
                tracing::warn!("writing clipboard: {e}");
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            // SAFETY: our own window.
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            // SAFETY: ends this thread's message loop.
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        // SAFETY: default handling for everything else.
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Holds the clipboard open; closes it on drop.
struct Open;

impl Open {
    /// Another app may hold the clipboard for a moment; retry briefly.
    fn new(hwnd: HWND) -> Result<Self> {
        for attempt in 0..10 {
            // SAFETY: opening for our own window.
            if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
                return Ok(Open);
            }
            std::thread::sleep(std::time::Duration::from_millis(5 + attempt * 5));
        }
        Err(PlatformError::Other("clipboard is busy".into()))
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        // SAFETY: we opened it.
        let _ = unsafe { CloseClipboard() };
    }
}

fn format_id(name: &str) -> u32 {
    let w = wide(name);
    // SAFETY: null-terminated name.
    unsafe { RegisterClipboardFormatW(PCWSTR(w.as_ptr())) }
}

fn available(format: u32) -> bool {
    // SAFETY: plain query.
    format != 0 && unsafe { IsClipboardFormatAvailable(format) }.is_ok()
}

/// Copies the bytes behind a clipboard handle (clipboard must be open).
fn global_bytes(format: u32, max: usize) -> Option<Vec<u8>> {
    // SAFETY: the clipboard is open; the handle stays valid until it is
    // closed, and we only read within `GlobalSize`.
    unsafe {
        let handle = GetClipboardData(format).ok()?;
        let global = HGLOBAL(handle.0);
        let size = GlobalSize(global);
        if size == 0 || size > max {
            return None;
        }
        let ptr = GlobalLock(global) as *const u8;
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr, size).to_vec();
        let _ = GlobalUnlock(global);
        Some(bytes)
    }
}

/// Private or "do not record" content, per the documented clipboard formats.
fn excluded() -> bool {
    if available(format_id("ExcludeClipboardContentFromMonitorProcessing"))
        || available(format_id("Clipboard Viewer Ignore"))
    {
        return true;
    }
    let history = format_id("CanIncludeInClipboardHistory");
    available(history)
        && global_bytes(history, 64).is_some_and(|b| b.len() >= 4 && b[..4] == [0, 0, 0, 0])
}

/// Raw clipboard payload, copied out while the clipboard is open.
enum Raw {
    Text(Vec<u8>),
    Dib(Vec<u8>),
}

fn read(hwnd: HWND) -> Result<Option<ClipContent>> {
    // Copy the bytes and close the clipboard right away; decoding happens
    // after, so other apps are never kept waiting.
    let raw = {
        let _open = Open::new(hwnd)?;
        if excluded() {
            return Ok(None);
        }
        if available(u32::from(CF_UNICODETEXT.0)) {
            global_bytes(u32::from(CF_UNICODETEXT.0), MAX_TEXT * 2).map(Raw::Text)
        } else if available(u32::from(CF_DIB.0)) {
            global_bytes(u32::from(CF_DIB.0), MAX_DIB).map(Raw::Dib)
        } else {
            None
        }
    };
    Ok(match raw {
        Some(Raw::Text(bytes)) => {
            let units: Vec<u16> = bytes
                .as_chunks::<2>()
                .0
                .iter()
                .map(|&c| u16::from_le_bytes(c))
                .take_while(|&u| u != 0)
                .collect();
            let text = String::from_utf16_lossy(&units);
            (!text.trim().is_empty()).then_some(ClipContent::Text(text))
        }
        Some(Raw::Dib(dib)) => decode_dib(dib).map(|img| ClipContent::Image(Arc::new(img))),
        None => None,
    })
}

fn write(hwnd: HWND, content: &ClipContent) -> Result<()> {
    let (format, bytes): (u32, Vec<u8>) = match content {
        ClipContent::Text(t) => (
            u32::from(CF_UNICODETEXT.0),
            t.encode_utf16()
                .chain(std::iter::once(0))
                .flat_map(u16::to_le_bytes)
                .collect(),
        ),
        ClipContent::Image(img) => (u32::from(CF_DIB.0), img.dib.clone()),
    };
    let _open = Open::new(hwnd)?;
    // SAFETY: the clipboard is open for our window.
    unsafe { EmptyClipboard() }.api("EmptyClipboard")?;
    put(format, &bytes)
}

/// Adds one format to the open clipboard.
fn put(format: u32, bytes: &[u8]) -> Result<()> {
    // SAFETY: the caller holds the clipboard open. The global block is
    // filled within its size; on success the system owns it, otherwise we
    // free it.
    unsafe {
        let global = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).api("GlobalAlloc")?;
        let ptr = GlobalLock(global) as *mut u8;
        if ptr.is_null() {
            let _ = GlobalFree(Some(global));
            return Err(PlatformError::Other("GlobalLock failed".into()));
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(global);
        if let Err(e) = SetClipboardData(format, Some(HANDLE(global.0))) {
            let _ = GlobalFree(Some(global));
            return Err(PlatformError::Win32 {
                call: "SetClipboardData",
                source: e,
            });
        }
    }
    Ok(())
}

fn le_u32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn le_i32(b: &[u8], at: usize) -> Option<i32> {
    le_u32(b, at).map(|v| v as i32)
}

/// Wraps a packed DIB in a BMP file header so the `image` crate decodes it,
/// then builds the thumbnail.
fn decode_dib(dib: Vec<u8>) -> Option<ClipImage> {
    let header = le_u32(&dib, 0)? as usize;
    let width = le_i32(&dib, 4)?.unsigned_abs();
    let height = le_i32(&dib, 8)?.unsigned_abs();
    let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?);
    let compression = le_u32(&dib, 16)?;
    let colors = le_u32(&dib, 32)? as usize;
    // BI_BITFIELDS / BI_ALPHABITFIELDS masks follow a plain 40-byte header.
    let masks = match (header, compression) {
        (40, 3) => 12,
        (40, 6) => 16,
        _ => 0,
    };
    let palette = if colors > 0 {
        colors
    } else if bits <= 8 {
        1 << bits
    } else {
        0
    };
    let offset = 14 + header + masks + palette * 4;
    let mut file = Vec::with_capacity(14 + dib.len());
    file.extend_from_slice(b"BM");
    file.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(&(offset as u32).to_le_bytes());
    file.extend_from_slice(&dib);
    let decoded = image::load_from_memory_with_format(&file, image::ImageFormat::Bmp)
        .inspect_err(|e| tracing::debug!("decoding clipboard bitmap: {e}"))
        .ok()?;
    let decoded = if decoded.width().max(decoded.height()) > THUMB_SIZE {
        decoded.thumbnail(THUMB_SIZE, THUMB_SIZE)
    } else {
        decoded
    };
    let mut thumb = decoded.to_rgba8();
    // Many apps leave the alpha byte of 32-bit DIBs at zero.
    if thumb.pixels().all(|p| p.0[3] == 0) {
        thumb.pixels_mut().for_each(|p| p.0[3] = 255);
    }
    Some(ClipImage {
        width,
        height,
        thumb_size: [thumb.width() as usize, thumb.height() as usize],
        thumb: thumb.into_raw(),
        dib,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 bottom-up 24-bit DIB: red, green / blue, white.
    fn dib_2x2() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&40u32.to_le_bytes());
        d.extend_from_slice(&2i32.to_le_bytes());
        d.extend_from_slice(&2i32.to_le_bytes());
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&24u16.to_le_bytes());
        d.extend_from_slice(&[0; 24]); // compression .. important colors
        // Rows are bottom-up, BGR, padded to 4 bytes.
        d.extend_from_slice(&[255, 0, 0, 255, 255, 255, 0, 0]); // blue, white
        d.extend_from_slice(&[0, 0, 255, 0, 255, 0, 0, 0]); // red, green
        d
    }

    #[test]
    fn decodes_a_packed_dib() {
        let img = decode_dib(dib_2x2()).expect("decodes");
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(img.thumb_size, [2, 2]);
        assert_eq!(&img.thumb[0..4], &[255, 0, 0, 255]); // top-left red
        assert_eq!(&img.thumb[12..16], &[255, 255, 255, 255]); // bottom-right white
    }

    #[test]
    fn frame_dib_round_trips() {
        let frame = crate::capture::Frame {
            origin: (0, 0),
            width: 2,
            height: 2,
            pixels: vec![0xFFFF_0000, 0xFF00_FF00, 0xFF00_00FF, 0xFFFF_FFFF],
        };
        let img = decode_dib(frame_dib(&frame)).expect("decodes");
        assert_eq!((img.width, img.height), (2, 2));
        assert_eq!(&img.thumb[0..4], &[255, 0, 0, 255]);
        assert_eq!(&img.thumb[12..16], &[255, 255, 255, 255]);
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode_dib(vec![1, 2, 3]).is_none());
    }
}
