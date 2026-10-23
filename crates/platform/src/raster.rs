//! Burns annotations into an opaque BGRA image (`0xFFRRGGBB`, top-down):
//! anti-aliased strokes, filled blocks and text. The screenshot editor
//! draws the same shapes with egui while you edit and uses these to make
//! the image you copy or save.
//!
//! Strokes are unions of capsules: each pixel takes the highest coverage of
//! any segment of the stroke, so a translucent marker does not darken where
//! its own segments overlap. Text goes through GDI (Segoe UI Semibold,
//! greyscale anti-aliasing so it has no color fringes on any background).

use crate::{PlatformError, Result};
use windows::Win32::Foundation::{COLORREF, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC,
    CreateDIBSection, CreateFontIndirectW, DIB_RGB_COLORS, DT_NOCLIP, DT_NOPREFIX, DT_SINGLELINE,
    DeleteDC, DeleteObject, DrawTextW, FW_SEMIBOLD, GetTextExtentPoint32W, LOGFONTW, SelectObject,
    SetBkMode, SetTextColor, TRANSPARENT,
};

/// An image to draw on.
pub struct Canvas<'a> {
    pub pixels: &'a mut [u32],
    pub width: u32,
    pub height: u32,
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

/// `src` (RGB) over the opaque pixel `dst` with opacity `alpha` (0..=1).
fn blend(dst: u32, [r, g, b]: [u8; 3], alpha: f32) -> u32 {
    let mix = |d: u32, s: u8| {
        let v = d as f32 * (1.0 - alpha) + f32::from(s) * alpha;
        v.round().clamp(0.0, 255.0) as u32
    };
    let (dr, dg, db) = ((dst >> 16) & 0xFF, (dst >> 8) & 0xFF, dst & 0xFF);
    0xFF00_0000 | (mix(dr, r) << 16) | (mix(dg, g) << 8) | mix(db, b)
}

/// Width and height in pixels of one line of `text` as [`Canvas::text`]
/// draws it at `size` pixels per em.
pub fn text_size(text: &str, size: f32) -> (f32, f32) {
    let wide: Vec<u16> = text.encode_utf16().collect();
    let mut face = [0u16; 32];
    for (d, s) in face.iter_mut().zip("Segoe UI".encode_utf16()) {
        *d = s;
    }
    let lf = LOGFONTW {
        lfHeight: -(size.round() as i32).max(1),
        lfWeight: FW_SEMIBOLD.0 as i32,
        lfQuality: ANTIALIASED_QUALITY,
        lfFaceName: face,
        ..Default::default()
    };
    // SAFETY: a memory DC and font created and freed here.
    unsafe {
        let dc = CreateCompatibleDC(None);
        let font = CreateFontIndirectW(&lf);
        let old = SelectObject(dc, font.into());
        let mut extent = SIZE::default();
        let _ = GetTextExtentPoint32W(dc, &wide, &mut extent);
        SelectObject(dc, old);
        let _ = DeleteObject(font.into());
        let _ = DeleteDC(dc);
        (extent.cx as f32, extent.cy as f32)
    }
}

/// Average color of the pixel rectangle `(x0, y0, x1, y1)` of a `width` x
/// `height` image, clamped to it.
pub fn average(
    pixels: &[u32],
    width: u32,
    height: u32,
    (x0, y0, x1, y1): (i32, i32, i32, i32),
) -> [u8; 3] {
    let (w, h) = (width as i32, height as i32);
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    for y in y0.clamp(0, h)..y1.clamp(0, h) {
        for x in x0.clamp(0, w)..x1.clamp(0, w) {
            let p = pixels[(y * w + x) as usize];
            sum[0] += u64::from((p >> 16) & 0xFF);
            sum[1] += u64::from((p >> 8) & 0xFF);
            sum[2] += u64::from(p & 0xFF);
            n += 1;
        }
    }
    let n = n.max(1);
    [(sum[0] / n) as u8, (sum[1] / n) as u8, (sum[2] / n) as u8]
}

impl Canvas<'_> {
    /// Strokes the polyline `points` (a single point draws a dot) `width`
    /// wide in `rgb` at `opacity`.
    pub fn stroke(&mut self, points: &[(f32, f32)], width: f32, rgb: [u8; 3], opacity: f32) {
        if points.is_empty() || width <= 0.0 {
            return;
        }
        let r = width * 0.5;
        let pad = r + 1.0;
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in points {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        let (w, h) = (self.width as i32, self.height as i32);
        let bx0 = ((x0 - pad).floor() as i32).clamp(0, w);
        let by0 = ((y0 - pad).floor() as i32).clamp(0, h);
        let bx1 = ((x1 + pad).ceil() as i32).clamp(0, w);
        let by1 = ((y1 + pad).ceil() as i32).clamp(0, h);
        let bw = (bx1 - bx0) as usize;
        if bw == 0 || by1 <= by0 {
            return;
        }
        let mut cov = vec![0f32; bw * (by1 - by0) as usize];
        let segments: Vec<((f32, f32), (f32, f32))> = if points.len() == 1 {
            vec![(points[0], points[0])]
        } else {
            points.windows(2).map(|s| (s[0], s[1])).collect()
        };
        for (a, b) in segments {
            let sx0 = ((a.0.min(b.0) - pad).floor() as i32).clamp(bx0, bx1);
            let sy0 = ((a.1.min(b.1) - pad).floor() as i32).clamp(by0, by1);
            let sx1 = ((a.0.max(b.0) + pad).ceil() as i32).clamp(bx0, bx1);
            let sy1 = ((a.1.max(b.1) + pad).ceil() as i32).clamp(by0, by1);
            for y in sy0..sy1 {
                for x in sx0..sx1 {
                    let d = segment_distance((x as f32 + 0.5, y as f32 + 0.5), a, b);
                    let c = (r + 0.5 - d).clamp(0.0, 1.0);
                    let i = (y - by0) as usize * bw + (x - bx0) as usize;
                    if c > cov[i] {
                        cov[i] = c;
                    }
                }
            }
        }
        for y in by0..by1 {
            for x in bx0..bx1 {
                let c = cov[(y - by0) as usize * bw + (x - bx0) as usize];
                if c > 0.0 {
                    let i = (y * w + x) as usize;
                    self.pixels[i] = blend(self.pixels[i], rgb, c * opacity);
                }
            }
        }
    }

    /// Fills the pixel rectangle `[x0, x1) x [y0, y1)` with `rgb`.
    pub fn fill(&mut self, (x0, y0, x1, y1): (i32, i32, i32, i32), rgb: [u8; 3]) {
        let (w, h) = (self.width as i32, self.height as i32);
        let color =
            0xFF00_0000 | (u32::from(rgb[0]) << 16) | (u32::from(rgb[1]) << 8) | u32::from(rgb[2]);
        for y in y0.clamp(0, h)..y1.clamp(0, h) {
            let row = (y * w) as usize;
            self.pixels[row + x0.clamp(0, w) as usize..row + x1.clamp(0, w) as usize].fill(color);
        }
    }

    /// Average color of the pixel rectangle (clamped to the image).
    pub fn average(&self, r: (i32, i32, i32, i32)) -> [u8; 3] {
        average(self.pixels, self.width, self.height, r)
    }

    /// Draws one line of `text` with its top-left at `(x, y)`, `size` pixels
    /// per em.
    pub fn text(&mut self, (x, y): (f32, f32), text: &str, size: f32, rgb: [u8; 3]) -> Result<()> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        if wide.is_empty() {
            return Ok(());
        }
        let mut face = [0u16; 32];
        for (d, s) in face.iter_mut().zip("Segoe UI".encode_utf16()) {
            *d = s;
        }
        let lf = LOGFONTW {
            lfHeight: -(size.round() as i32).max(1),
            lfWeight: FW_SEMIBOLD.0 as i32,
            lfQuality: ANTIALIASED_QUALITY,
            lfFaceName: face,
            ..Default::default()
        };
        // SAFETY: a memory DC, font and DIB section created and freed here;
        // the DIB is `bw * bh` pixels and only touched within that.
        unsafe {
            let dc = CreateCompatibleDC(None);
            let font = CreateFontIndirectW(&lf);
            let old_font = SelectObject(dc, font.into());
            let mut extent = SIZE::default();
            let _ = GetTextExtentPoint32W(dc, &wide, &mut extent);
            // A margin for overhanging glyphs (italics, accents).
            let margin = (size * 0.3).ceil() as i32;
            let (bw, bh) = (extent.cx + 2 * margin, extent.cy + 2 * margin);
            let (ox, oy) = (x.round() as i32 - margin, y.round() as i32 - margin);
            let info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: bw,
                    biHeight: -bh,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0)
            {
                Ok(b) if !bits.is_null() => b,
                _ => {
                    SelectObject(dc, old_font);
                    let _ = DeleteObject(font.into());
                    let _ = DeleteDC(dc);
                    return Err(PlatformError::Other("CreateDIBSection(text) failed".into()));
                }
            };
            let old_bitmap = SelectObject(dc, bitmap.into());
            let buf = std::slice::from_raw_parts_mut(bits.cast::<u32>(), (bw * bh) as usize);
            // Copy the image under the text box in, draw, copy back.
            let (w, h) = (self.width as i32, self.height as i32);
            let inside = |px: i32, py: i32| px >= 0 && py >= 0 && px < w && py < h;
            for by in 0..bh {
                for bx in 0..bw {
                    let (px, py) = (ox + bx, oy + by);
                    if inside(px, py) {
                        buf[(by * bw + bx) as usize] = self.pixels[(py * w + px) as usize];
                    }
                }
            }
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(
                dc,
                COLORREF(u32::from(rgb[0]) | (u32::from(rgb[1]) << 8) | (u32::from(rgb[2]) << 16)),
            );
            let mut rect = RECT {
                left: margin,
                top: margin,
                right: bw - margin,
                bottom: bh - margin,
            };
            let mut copy = wide.clone();
            DrawTextW(
                dc,
                &mut copy,
                &mut rect,
                DT_SINGLELINE | DT_NOPREFIX | DT_NOCLIP,
            );
            for by in 0..bh {
                for bx in 0..bw {
                    let (px, py) = (ox + bx, oy + by);
                    if inside(px, py) {
                        self.pixels[(py * w + px) as usize] =
                            buf[(by * bw + bx) as usize] | 0xFF00_0000;
                    }
                }
            }
            SelectObject(dc, old_bitmap);
            let _ = DeleteObject(bitmap.into());
            SelectObject(dc, old_font);
            let _ = DeleteObject(font.into());
            let _ = DeleteDC(dc);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas(px: &mut [u32]) -> Canvas<'_> {
        Canvas {
            pixels: px,
            width: 20,
            height: 20,
        }
    }

    #[test]
    fn strokes_are_antialiased_and_clipped() {
        let mut px = vec![0xFF00_0000; 400];
        let mut c = canvas(&mut px);
        // Runs off the left edge without panicking.
        c.stroke(&[(-10.0, 10.3), (15.0, 10.3)], 4.0, [255, 255, 255], 1.0);
        assert_eq!(px[10 * 20 + 5], 0xFFFF_FFFF);
        let edge = px[12 * 20 + 5] & 0xFF;
        assert!(edge > 0 && edge < 255, "edge {edge}");
        assert_eq!(px[16 * 20 + 5], 0xFF00_0000);
        assert_eq!(px[10 * 20 + 19], 0xFF00_0000);
    }

    #[test]
    fn translucent_stroke_does_not_stack_on_itself() {
        let mut px = vec![0xFF00_0000; 400];
        let mut c = canvas(&mut px);
        c.stroke(
            &[(2.0, 10.0), (10.0, 10.0), (2.0, 10.0)],
            6.0,
            [255, 255, 255],
            0.5,
        );
        assert_eq!(px[10 * 20 + 6] & 0xFF, 128);
    }

    #[test]
    fn fills_and_averages() {
        let mut px = vec![0xFF00_0000; 400];
        let mut c = canvas(&mut px);
        c.fill((0, 0, 10, 20), [200, 100, 0]);
        assert_eq!(c.average((0, 0, 20, 20)), [100, 50, 0]);
        assert_eq!(c.average((-5, -5, 5, 5)), [200, 100, 0]);
    }

    #[test]
    fn draws_text() {
        let mut px = vec![0xFF00_0000; 40 * 20];
        let mut c = Canvas {
            pixels: &mut px,
            width: 40,
            height: 20,
        };
        c.text((2.0, 2.0), "Hi", 14.0, [255, 255, 255])
            .expect("text");
        assert!(px.iter().any(|&p| p & 0xFF > 200));
        assert!(px.iter().all(|&p| p >> 24 == 0xFF));
    }
}
