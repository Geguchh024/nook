//! Screen grabs (physical pixels) for the snip overlay, OCR, lens and the
//! color picker.
//!
//! `BitBlt` from the screen DC returns what DWM composited, layered and
//! accelerated windows included. Nook's own windows are left out of the
//! grab: they are marked `WDA_EXCLUDEFROMCAPTURE` for the moment of the
//! copy, so the open panel never shows up in what you select.

use crate::monitor::Rect;
use crate::{PlatformError, Result, Win32Context};
use windows::Win32::Foundation::{HWND, LPARAM};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
    CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
    SRCCOPY, SelectObject,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowDisplayAffinity, GetWindowThreadProcessId, IsWindowVisible,
    SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WINDOW_DISPLAY_AFFINITY,
};
use windows::core::BOOL;

/// A captured image: opaque BGRA pixels (`0xFFRRGGBB`), top-down.
#[derive(Clone, Debug, PartialEq)]
pub struct Frame {
    /// Screen position of the top-left pixel.
    pub origin: (i32, i32),
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

impl Frame {
    pub fn rect(&self) -> Rect {
        Rect {
            left: self.origin.0,
            top: self.origin.1,
            right: self.origin.0 + self.width as i32,
            bottom: self.origin.1 + self.height as i32,
        }
    }

    /// `[r, g, b]` at frame coordinates, clamped to the edges.
    pub fn rgb(&self, x: i32, y: i32) -> [u8; 3] {
        let x = x.clamp(0, self.width as i32 - 1) as usize;
        let y = y.clamp(0, self.height as i32 - 1) as usize;
        let p = self.pixels[y * self.width as usize + x];
        [(p >> 16) as u8, (p >> 8) as u8, p as u8]
    }

    /// The part of the frame inside `r` (screen coordinates), or `None` if
    /// they do not overlap.
    pub fn crop(&self, r: Rect) -> Option<Frame> {
        crop_pixels(&self.pixels, self.width, self.origin, r)
    }

    /// Packed BGRA bytes (what `SoftwareBitmap` and GDI expect).
    pub fn bgra_bytes(&self) -> Vec<u8> {
        self.pixels.iter().flat_map(|p| p.to_le_bytes()).collect()
    }

    pub fn to_rgb_image(&self) -> image::RgbImage {
        let raw = self
            .pixels
            .iter()
            .flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8])
            .collect();
        image::RgbImage::from_raw(self.width, self.height, raw).unwrap_or_default()
    }

    /// PNG bytes at full size.
    pub fn encode_png(&self) -> Result<Vec<u8>> {
        let mut out = std::io::Cursor::new(Vec::new());
        self.to_rgb_image()
            .write_to(&mut out, image::ImageFormat::Png)
            .map_err(|e| PlatformError::Other(format!("encoding PNG: {e}")))?;
        Ok(out.into_inner())
    }

    /// JPEG bytes, scaled down so the longest edge is at most `max_edge`.
    pub fn encode_jpeg(&self, max_edge: u32, quality: u8) -> Result<Vec<u8>> {
        let mut img = image::DynamicImage::ImageRgb8(self.to_rgb_image());
        if img.width().max(img.height()) > max_edge {
            img = img.resize(max_edge, max_edge, image::imageops::FilterType::Triangle);
        }
        let mut out = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality)
            .encode_image(&img)
            .map_err(|e| PlatformError::Other(format!("encoding JPEG: {e}")))?;
        Ok(out)
    }
}

/// The part of a `width`-wide, top-down pixel buffer at screen position
/// `origin` that lies inside `r`, or `None` if they do not overlap.
pub fn crop_pixels(pixels: &[u32], width: u32, origin: (i32, i32), r: Rect) -> Option<Frame> {
    let height = (pixels.len() / width.max(1) as usize) as i32;
    let (l, t) = (r.left.max(origin.0), r.top.max(origin.1));
    let (rr, bb) = (
        r.right.min(origin.0 + width as i32),
        r.bottom.min(origin.1 + height),
    );
    if rr <= l || bb <= t {
        return None;
    }
    let (w, h) = ((rr - l) as usize, (bb - t) as usize);
    let (ox, oy) = ((l - origin.0) as usize, (t - origin.1) as usize);
    let stride = width as usize;
    let mut out = Vec::with_capacity(w * h);
    for y in oy..oy + h {
        out.extend_from_slice(&pixels[y * stride + ox..y * stride + ox + w]);
    }
    Some(Frame {
        origin: (l, t),
        width: w as u32,
        height: h as u32,
        pixels: out,
    })
}

/// Copies `r` from the screen, leaving Nook's own windows out.
pub fn grab(r: Rect) -> Result<Frame> {
    let hidden = hide_own_windows();
    let frame = blit(r);
    for (hwnd, affinity) in hidden {
        // SAFETY: restoring the affinity we read from our own window.
        let _ = unsafe { SetWindowDisplayAffinity(hwnd, affinity) };
    }
    frame
}

fn blit(r: Rect) -> Result<Frame> {
    let (w, h) = (r.width(), r.height());
    if w <= 0 || h <= 0 {
        return Err(PlatformError::Other("empty capture rectangle".into()));
    }
    // SAFETY: screen DC, a compatible memory DC and bitmap, all released
    // before returning; `GetDIBits` writes exactly w * h 32-bit pixels.
    unsafe {
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, w, h);
        let old = SelectObject(mem, bitmap.into());
        let copied = BitBlt(
            mem,
            0,
            0,
            w,
            h,
            Some(screen),
            r.left,
            r.top,
            SRCCOPY | CAPTUREBLT,
        );
        let mut info = BITMAPINFO {
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
        let mut pixels = vec![0u32; (w * h) as usize];
        SelectObject(mem, old);
        let lines = GetDIBits(
            mem,
            bitmap,
            0,
            h as u32,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        let _ = DeleteObject(bitmap.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        copied.api("BitBlt(screen)")?;
        if lines != h {
            return Err(PlatformError::Other("GetDIBits(screen) failed".into()));
        }
        // GDI leaves the alpha byte undefined.
        pixels.iter_mut().for_each(|p| *p |= 0xFF00_0000);
        Ok(Frame {
            origin: (r.left, r.top),
            width: w as u32,
            height: h as u32,
            pixels,
        })
    }
}

/// Marks every visible top-level window of this process as excluded from
/// capture and waits for DWM to compose a frame without them. Returns the
/// previous affinities to restore.
fn hide_own_windows() -> Vec<(HWND, WINDOW_DISPLAY_AFFINITY)> {
    unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: `lparam` is the `Vec` passed to `EnumWindows` below, alive
        // for the whole enumeration.
        let list = unsafe { &mut *(lparam.0 as *mut Vec<HWND>) };
        let mut pid = 0;
        // SAFETY: plain queries on a window handle from the enumeration.
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            if pid == GetCurrentProcessId() && IsWindowVisible(hwnd).as_bool() {
                list.push(hwnd);
            }
        }
        BOOL(1)
    }
    let mut own: Vec<HWND> = Vec::new();
    // SAFETY: the callback only touches `own` through `lparam`.
    let _ = unsafe { EnumWindows(Some(collect), LPARAM(&mut own as *mut _ as isize)) };
    let mut changed = Vec::new();
    for hwnd in own {
        let mut affinity = 0u32;
        // SAFETY: our own top-level windows.
        unsafe {
            if GetWindowDisplayAffinity(hwnd, &mut affinity).is_ok()
                && SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE).is_ok()
            {
                changed.push((hwnd, WINDOW_DISPLAY_AFFINITY(affinity)));
            }
        }
    }
    if !changed.is_empty() {
        // Two compositions: the first may already have been under way.
        // SAFETY: plain waits on DWM.
        unsafe {
            let _ = DwmFlush();
            let _ = DwmFlush();
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Frame {
        Frame {
            origin: (100, 50),
            width: 4,
            height: 3,
            pixels: (0..12).map(|i| 0xFF00_0000 | i).collect(),
        }
    }

    #[test]
    fn crops_in_screen_coordinates() {
        let f = frame();
        let c = f
            .crop(Rect {
                left: 101,
                top: 51,
                right: 200,
                bottom: 200,
            })
            .expect("overlaps");
        assert_eq!((c.origin, c.width, c.height), ((101, 51), 3, 2));
        assert_eq!(c.pixels[0] & 0xFF, 5);
        assert_eq!(c.pixels[5] & 0xFF, 11);
        assert!(
            f.crop(Rect {
                left: 0,
                top: 0,
                right: 100,
                bottom: 50
            })
            .is_none()
        );
    }

    #[test]
    fn reads_rgb_clamped() {
        let f = frame();
        assert_eq!(f.rgb(-5, -5), [0, 0, 0]);
        assert_eq!(f.rgb(99, 99), [0, 0, 11]);
    }
}
