//! Font detection: which installed font does the text in a screenshot look
//! like?
//!
//! There is no font recognizer in Windows, so this does it the plain way:
//!
//! 1. The selection is turned into an ink map (distance from the background
//!    color, taken from the border), split into text lines, and the line
//!    with the most ink is kept.
//! 2. The OCR engine (`crate::ocr`) reads that line, so we know what it
//!    says.
//! 3. The same text is drawn with every installed font family (regular,
//!    bold and italic) through GDI, cropped to its ink and scaled to the
//!    sample's size.
//! 4. Each rendering is scored by how well its pixels line up with the
//!    sample (normalized cross-correlation), times how close its proportions
//!    are. The best families win.
//!
//! It only finds fonts installed on this machine; a web or app font that
//! is not installed comes back as its closest installed look-alike.

use crate::capture::Frame;
use crate::{PlatformError, Result, fontnet, ocr};
use image::{GrayImage, Luma, imageops};
use windows::Win32::Foundation::COLORREF;
use windows::Win32::Foundation::{LPARAM, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    ANTIALIASED_QUALITY, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC,
    CreateDIBSection, CreateFontIndirectW, DEFAULT_CHARSET, DIB_RGB_COLORS, DT_NOCLIP, DT_NOPREFIX,
    DT_SINGLELINE, DeleteDC, DeleteObject, DrawTextW, EnumFontFamiliesExW, FW_BOLD, FW_NORMAL,
    GetTextExtentPoint32W, LOGFONTW, SYMBOL_CHARSET, SelectObject, SetBkMode, SetTextColor,
    TEXTMETRICW, TRANSPARENT,
};

/// Height every sample and rendering is scaled to before comparing.
const NORM_H: u32 = 40;
/// Longest normalized width compared (long lines are cut to it).
const MAX_NORM_W: u32 = 900;
/// Ink coverage above which a pixel counts for bounding boxes.
const INK: f32 = 0.35;
/// Em size the candidates are drawn at.
const RENDER_PX: i32 = 64;
/// Characters of the sample line compared at most.
const MAX_CHARS: usize = 48;
/// Lens predictions merged in.
const LENS_TOP: usize = 30;
/// Best installed score below which the font is taken not to be installed,
/// and above which it surely is. Measured (`eval_fonts`): exact renders of
/// installed fonts score 0.94-1.00 (one 18 px serif 0.79); the closest
/// installed look-alike of a Google font that is not installed 0.54-0.87.
const INSTALLED_LO: f32 = 0.84;
const INSTALLED_HI: f32 = 0.94;
/// Weight each side keeps whatever the other says, so a hit there still
/// makes the list.
const MIN_SHARE: f32 = 0.05;
/// Softmax temperature for installed scores.
const INSTALLED_TEMP: f32 = 0.02;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontStyle {
    Regular,
    Bold,
    Italic,
}

impl FontStyle {
    pub fn label(self) -> &'static str {
        match self {
            FontStyle::Regular => "Regular",
            FontStyle::Bold => "Bold",
            FontStyle::Italic => "Italic",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FontMatch {
    pub family: String,
    pub style: FontStyle,
    /// 0..=1, higher is closer.
    pub score: f32,
    pub source: Source,
}

/// Which matcher found a family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Rendered with an installed font.
    Installed,
    /// Recognized by Lens; not necessarily installed.
    Lens,
    /// Both agree.
    Both,
}

#[derive(Clone, Debug)]
pub struct Detection {
    /// What the sample line says.
    pub text: String,
    /// Best first, one entry per family.
    pub matches: Vec<FontMatch>,
    /// The sample line that was compared.
    pub sample: Frame,
}

fn err(msg: &str) -> PlatformError {
    PlatformError::Other(msg.to_owned())
}

/// Ink coverage (0..=1) of every pixel: distance from the border color,
/// stretched so the strongest ink is 1.
fn ink_map(frame: &Frame) -> Option<Vec<f32>> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    if w < 4 || h < 4 {
        return None;
    }
    let lum = |p: u32| {
        let (r, g, b) = ((p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF);
        0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32
    };
    let mut border = Vec::with_capacity(2 * (w + h));
    for x in 0..w {
        border.push(lum(frame.pixels[x]));
        border.push(lum(frame.pixels[(h - 1) * w + x]));
    }
    for y in 0..h {
        border.push(lum(frame.pixels[y * w]));
        border.push(lum(frame.pixels[y * w + w - 1]));
    }
    // Median of the border: robust to a glyph touching the edge.
    border.sort_by(f32::total_cmp);
    let bg = border[border.len() / 2];
    let mut ink: Vec<f32> = frame.pixels.iter().map(|&p| (lum(p) - bg).abs()).collect();
    let max = ink.iter().copied().fold(0.0, f32::max);
    if max < 40.0 {
        return None;
    }
    for v in &mut ink {
        *v = (*v / max).clamp(0.0, 1.0);
    }
    Some(ink)
}

/// Rows `[top, bottom)` of each text line: runs of rows with ink, merged
/// across gaps of a pixel or two.
fn line_bands(ink: &[f32], w: usize, h: usize) -> Vec<(usize, usize)> {
    let inked: Vec<bool> = (0..h)
        .map(|y| ink[y * w..(y + 1) * w].iter().any(|&v| v > INK))
        .collect();
    let mut bands: Vec<(usize, usize)> = Vec::new();
    let mut y = 0;
    while y < h {
        if !inked[y] {
            y += 1;
            continue;
        }
        let top = y;
        while y < h && inked[y] {
            y += 1;
        }
        match bands.last_mut() {
            Some(last) if top - last.1 <= 2 => last.1 = y,
            _ => bands.push((top, y)),
        }
    }
    bands
}

/// Bounding box `(x0, y0, x1, y1)` of the ink in a `w`-wide map.
fn ink_bbox(ink: &[f32], w: usize, h: usize) -> Option<(usize, usize, usize, usize)> {
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for y in 0..h {
        for x in 0..w {
            if ink[y * w + x] > INK {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x0 < x1 && y0 < y1).then_some((x0, y0, x1, y1))
}

/// The `(x0, y0, x1, y1)` part of an ink map as a grey image.
fn to_gray(ink: &[f32], w: usize, (x0, y0, x1, y1): (usize, usize, usize, usize)) -> GrayImage {
    GrayImage::from_fn((x1 - x0) as u32, (y1 - y0) as u32, |x, y| {
        let v = ink[(y0 + y as usize) * w + x0 + x as usize];
        Luma([(v * 255.0).round() as u8])
    })
}

/// Scales to `NORM_H` high, `width` wide.
fn normalize(img: &GrayImage, width: u32) -> GrayImage {
    imageops::resize(img, width.max(1), NORM_H, imageops::FilterType::Triangle)
}

/// Pearson correlation of two same-sized grey images.
fn correlation(a: &GrayImage, b: &GrayImage) -> f32 {
    let n = a.as_raw().len().min(b.as_raw().len());
    if n == 0 {
        return 0.0;
    }
    let (a, b) = (&a.as_raw()[..n], &b.as_raw()[..n]);
    let mean = |s: &[u8]| s.iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64;
    let (ma, mb) = (mean(a), mean(b));
    let (mut cov, mut va, mut vb) = (0.0, 0.0, 0.0);
    for (&x, &y) in a.iter().zip(b) {
        let (dx, dy) = (f64::from(x) - ma, f64::from(y) - mb);
        cov += dx * dy;
        va += dx * dx;
        vb += dy * dy;
    }
    if va <= 0.0 || vb <= 0.0 {
        return 0.0;
    }
    (cov / (va * vb).sqrt()) as f32
}

/// Installed font families that can draw text (no vertical `@` faces, no
/// symbol fonts), sorted and without duplicates.
pub fn families() -> Vec<String> {
    unsafe extern "system" fn collect(
        lf: *const LOGFONTW,
        _tm: *const TEXTMETRICW,
        _kind: u32,
        out: LPARAM,
    ) -> i32 {
        // SAFETY: GDI passes a valid LOGFONTW; `out` is the Vec below,
        // alive for the whole enumeration.
        unsafe {
            let lf = &*lf;
            let names = &mut *(out.0 as *mut Vec<String>);
            if lf.lfCharSet != SYMBOL_CHARSET {
                let len = lf.lfFaceName.iter().position(|&c| c == 0).unwrap_or(32);
                let name = String::from_utf16_lossy(&lf.lfFaceName[..len]);
                if !name.is_empty() && !name.starts_with('@') {
                    names.push(name);
                }
            }
        }
        1
    }
    let mut names: Vec<String> = Vec::new();
    let lf = LOGFONTW {
        lfCharSet: DEFAULT_CHARSET,
        ..Default::default()
    };
    // SAFETY: a memory DC created and freed here; the callback only writes
    // to `names`.
    unsafe {
        let dc = CreateCompatibleDC(None);
        EnumFontFamiliesExW(
            dc,
            &lf,
            Some(collect),
            LPARAM(&mut names as *mut Vec<String> as isize),
            0,
        );
        let _ = DeleteDC(dc);
    }
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    names
}

/// Draws `text` in `family` (white on black) and returns its ink, cropped
/// to the glyphs.
fn render(text: &[u16], family: &str, style: FontStyle) -> Option<GrayImage> {
    let mut face = [0u16; 32];
    for (d, s) in face.iter_mut().take(31).zip(family.encode_utf16()) {
        *d = s;
    }
    let lf = LOGFONTW {
        lfHeight: -RENDER_PX,
        lfWeight: if style == FontStyle::Bold {
            FW_BOLD.0 as i32
        } else {
            FW_NORMAL.0 as i32
        },
        lfItalic: u8::from(style == FontStyle::Italic),
        lfCharSet: DEFAULT_CHARSET,
        lfQuality: ANTIALIASED_QUALITY,
        lfFaceName: face,
        ..Default::default()
    };
    // SAFETY: a memory DC, font and DIB section created and freed here; the
    // DIB is `bw * bh` pixels and only read within that.
    unsafe {
        let dc = CreateCompatibleDC(None);
        let font = CreateFontIndirectW(&lf);
        let old_font = SelectObject(dc, font.into());
        let mut extent = SIZE::default();
        let _ = GetTextExtentPoint32W(dc, text, &mut extent);
        let margin = RENDER_PX / 2;
        let (bw, bh) = (extent.cx + 2 * margin, extent.cy + 2 * margin);
        let cleanup = |dc, font: windows::Win32::Graphics::Gdi::HFONT| {
            SelectObject(dc, old_font);
            let _ = DeleteObject(font.into());
            let _ = DeleteDC(dc);
        };
        if extent.cx <= 0 || bw > 8000 {
            cleanup(dc, font);
            return None;
        }
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
        let bitmap = match CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) if !bits.is_null() => b,
            _ => {
                cleanup(dc, font);
                return None;
            }
        };
        let old_bitmap = SelectObject(dc, bitmap.into());
        SetBkMode(dc, TRANSPARENT);
        SetTextColor(dc, COLORREF(0x00FF_FFFF));
        let mut rect = RECT {
            left: margin,
            top: margin,
            right: bw - margin,
            bottom: bh - margin,
        };
        let mut copy = text.to_vec();
        DrawTextW(
            dc,
            &mut copy,
            &mut rect,
            DT_SINGLELINE | DT_NOPREFIX | DT_NOCLIP,
        );
        let buf = std::slice::from_raw_parts(bits.cast::<u32>(), (bw * bh) as usize);
        let ink: Vec<f32> = buf
            .iter()
            .map(|&p| ((p >> 8) & 0xFF) as f32 / 255.0)
            .collect();
        SelectObject(dc, old_bitmap);
        let _ = DeleteObject(bitmap.into());
        cleanup(dc, font);

        let (w, h) = (bw as usize, bh as usize);
        let bbox = ink_bbox(&ink, w, h)?;
        Some(to_gray(&ink, w, bbox))
    }
}

/// Finds the installed fonts that look most like the text in `frame`.
pub fn detect(frame: &Frame, top: usize) -> Result<Detection> {
    let ink = ink_map(frame).ok_or_else(|| err("no text in the selection"))?;
    let (w, h) = (frame.width as usize, frame.height as usize);

    // The line with the most ink.
    let (top_y, bottom_y) = line_bands(&ink, w, h)
        .into_iter()
        .max_by(|a, b| {
            let mass = |(t, b): (usize, usize)| ink[t * w..b * w].iter().sum::<f32>();
            mass(*a).total_cmp(&mass(*b))
        })
        .ok_or_else(|| err("no text in the selection"))?;
    let band = &ink[top_y * w..bottom_y * w];
    let (x0, y0, x1, y1) =
        ink_bbox(band, w, bottom_y - top_y).ok_or_else(|| err("no text in the selection"))?;
    if y1 - y0 < 6 {
        return Err(err("text is too small to compare"));
    }

    // Read it: OCR the line alone (with a little air around it).
    let pad = (y1 - y0) / 2;
    let sx0 = x0.saturating_sub(pad);
    let sx1 = (x1 + pad).min(w);
    let sy0 = (top_y + y0).saturating_sub(pad);
    let sy1 = (top_y + y1 + pad).min(h);
    let sample = Frame {
        origin: (frame.origin.0 + sx0 as i32, frame.origin.1 + sy0 as i32),
        width: (sx1 - sx0) as u32,
        height: (sy1 - sy0) as u32,
        pixels: (sy0..sy1)
            .flat_map(|y| frame.pixels[y * w + sx0..y * w + sx1].iter().copied())
            .collect(),
    };
    // The line alone can fail to read (a logo, a big heading cut oddly):
    // then read the whole selection and take its longest line.
    let mut read = ocr::recognize(&sample, None)?;
    let mut words_in = &sample;
    let mut line_box = (x0, top_y + y0, x1, top_y + y1);
    if longest_line(&read).is_none() {
        let whole = ocr::recognize(frame, None)?;
        if let Some(i) = longest_line(&whole)
            && let Some(b) = words_box(&whole.words, i).and_then(|r| ink_bbox_in(&ink, w, h, r))
        {
            line_box = b;
            words_in = frame;
            read = whole;
        }
    }
    let crops = word_crops(words_in, &read.words);
    let Some(text) = longest_line(&read).map(|i| read.lines[i].trim().to_owned()) else {
        // Nothing readable: Lens still works on the image alone.
        save_failed_sample(&sample);
        return lens_only(&crops, sample, top);
    };
    let (x0, y0, x1, y1) = line_box;
    if y1 - y0 < 6 {
        return Err(err("text is too small to compare"));
    }

    // Long lines: compare only the start, cut at the same fraction of the
    // sample's width.
    let chars: Vec<char> = text.chars().collect();
    let (used, frac) = if chars.len() > MAX_CHARS {
        let cut: String = chars[..MAX_CHARS].iter().collect();
        let cut = cut.trim_end().to_owned();
        let f = cut.chars().count() as f32 / chars.len() as f32;
        (cut, f)
    } else {
        (text.clone(), 1.0)
    };
    let line = to_gray(&ink, w, (x0, y0, x1, y1));
    let line_w = ((line.width() as f32 * frac).round() as u32).max(1);
    let line = imageops::crop_imm(&line, 0, 0, line_w, line.height()).to_image();
    let aspect = line.width() as f32 / line.height() as f32;
    let norm_w = ((aspect * NORM_H as f32).round() as u32).clamp(8, MAX_NORM_W);
    let target = normalize(&line, norm_w);

    let wide: Vec<u16> = used.encode_utf16().collect();
    let fams = families();
    let styles = [FontStyle::Regular, FontStyle::Bold, FontStyle::Italic];
    let jobs: Vec<(&str, FontStyle)> = fams
        .iter()
        .flat_map(|f| styles.iter().map(move |&s| (f.as_str(), s)))
        .collect();
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(1, 8);
    let chunk = jobs.len().div_ceil(threads).max(1);
    let (scored, lens) = std::thread::scope(|s| {
        // Lens runs alongside the installed-font renders.
        let lens = s.spawn(|| {
            let dir = fontnet::model_dir()?;
            match fontnet::classify(&dir, &crops, LENS_TOP) {
                Ok(p) => Some(p),
                Err(e) => {
                    tracing::warn!("font detector: {e}");
                    None
                }
            }
        });
        let handles: Vec<_> = jobs
            .chunks(chunk)
            .map(|part| {
                let (wide, target) = (&wide, &target);
                s.spawn(move || {
                    part.iter()
                        .filter_map(|&(family, style)| {
                            let img = render(wide, family, style)?;
                            let a = img.width() as f32 / img.height() as f32;
                            let fit = (a.min(aspect) / a.max(aspect)).powi(2);
                            let corr = correlation(&normalize(&img, target.width()), target);
                            Some(FontMatch {
                                family: family.to_owned(),
                                style,
                                score: (corr.max(0.0) * fit).clamp(0.0, 1.0),
                                source: Source::Installed,
                            })
                        })
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        let scored: Vec<FontMatch> = handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect();
        (scored, lens.join().ok().flatten())
    });

    let installed = best_per_family(scored);
    let matches = match lens {
        Some(lens) => merge(installed, &lens, top),
        None => installed.into_iter().take(top).collect(),
    };
    if matches.is_empty() {
        return Err(err("no installed font could draw this text"));
    }
    Ok(Detection {
        text,
        matches,
        sample,
    })
}

/// Index of the longest non-empty line.
fn longest_line(read: &ocr::OcrText) -> Option<usize> {
    (0..read.lines.len())
        .filter(|&i| !read.lines[i].trim().is_empty())
        .max_by_key(|&i| read.lines[i].trim().chars().count())
}

/// Union of the boxes of line `line`'s words.
fn words_box(words: &[ocr::OcrWord], line: usize) -> Option<Box4> {
    words
        .iter()
        .filter(|wd| wd.line == line)
        .map(|wd| wd.rect)
        .reduce(union)
}

/// Ink bounding box inside `rect` of a `w` x `h` ink map, in map pixels.
fn ink_bbox_in(
    ink: &[f32],
    w: usize,
    h: usize,
    (x, y, rw, rh): Box4,
) -> Option<(usize, usize, usize, usize)> {
    let x0 = (x.floor().max(0.0) as usize).min(w);
    let y0 = (y.floor().max(0.0) as usize).min(h);
    let x1 = ((x + rw).ceil().max(0.0) as usize).min(w);
    let y1 = ((y + rh).ceil().max(0.0) as usize).min(h);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let sub: Vec<f32> = (y0..y1)
        .flat_map(|yy| ink[yy * w + x0..yy * w + x1].iter().copied())
        .collect();
    let (bx0, by0, bx1, by1) = ink_bbox(&sub, x1 - x0, y1 - y0)?;
    Some((x0 + bx0, y0 + by0, x0 + bx1, y0 + by1))
}

/// Lens on the image alone, when OCR read nothing.
fn lens_only(crops: &[Frame], sample: Frame, top: usize) -> Result<Detection> {
    let dir = fontnet::model_dir().ok_or_else(|| err("could not read the text"))?;
    let matches: Vec<FontMatch> = fontnet::classify(&dir, crops, top)?
        .into_iter()
        .map(|p| FontMatch {
            family: p.family,
            style: FontStyle::Regular,
            score: p.prob,
            source: Source::Lens,
        })
        .collect();
    if matches.is_empty() {
        return Err(err("could not read the text"));
    }
    Ok(Detection {
        text: String::new(),
        matches,
        sample,
    })
}

/// Keeps the sample OCR could not read, for a look afterwards.
fn save_failed_sample(sample: &Frame) {
    let path = std::env::temp_dir().join("nook-font-sample.png");
    match sample.to_rgb_image().save(&path) {
        Ok(()) => tracing::info!(
            "font detector: unreadable sample saved to {}",
            path.display()
        ),
        Err(e) => tracing::debug!("font detector: saving sample: {e}"),
    }
}

/// Best style of each family, best family first.
fn best_per_family(mut scored: Vec<FontMatch>) -> Vec<FontMatch> {
    scored.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut out: Vec<FontMatch> = Vec::new();
    for m in scored {
        if !out.iter().any(|k| k.family == m.family) {
            out.push(m);
        }
    }
    out
}

/// Crops for Lens, in `sample` pixels: the largest word (Lens's own choice,
/// 3 px of air), and the run of words around it on its line that stays
/// within the network's 3:1 input shape. Lens averages over both.
fn word_crops(sample: &Frame, words: &[ocr::OcrWord]) -> Vec<Frame> {
    let usable: Vec<&ocr::OcrWord> = words
        .iter()
        .filter(|w| w.text.chars().any(char::is_alphanumeric) && w.rect.2 > 2.0 && w.rect.3 > 2.0)
        .collect();
    let area = |w: &ocr::OcrWord| w.rect.2 * w.rect.3;
    let Some(best) = (0..usable.len()).max_by(|&a, &b| area(usable[a]).total_cmp(&area(usable[b])))
    else {
        // No words: the whole line, as Lens does without OCR.
        return vec![sample.clone()];
    };
    let mut crops = vec![crop(sample, usable[best].rect, 3.0)];
    // Grow to the neighbors on the same line while the run fits 3:1.
    let same = |j: usize| usable[j].line == usable[best].line;
    let (mut lo, mut hi, mut run) = (best, best, usable[best].rect);
    loop {
        let right = (hi + 1 < usable.len() && same(hi + 1)).then(|| hi + 1);
        let left = (lo > 0 && same(lo - 1)).then(|| lo - 1);
        let grown = [right, left].into_iter().flatten().find_map(|j| {
            let r = union(run, usable[j].rect);
            (r.2 <= r.3 * 3.0).then_some((j, r))
        });
        match grown {
            Some((j, r)) => {
                lo = lo.min(j);
                hi = hi.max(j);
                run = r;
            }
            None => break,
        }
    }
    if hi > lo {
        crops.push(crop(sample, run, 3.0));
    }
    crops
}

type Box4 = (f32, f32, f32, f32);

fn union(a: Box4, b: Box4) -> Box4 {
    let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
    let (x1, y1) = ((a.0 + a.2).max(b.0 + b.2), (a.1 + a.3).max(b.1 + b.3));
    (x0, y0, x1 - x0, y1 - y0)
}

/// `rect` of `frame` grown by `pad`, clamped to the frame.
fn crop(frame: &Frame, (x, y, w, h): Box4, pad: f32) -> Frame {
    let (fw, fh) = (frame.width as usize, frame.height as usize);
    let x0 = ((x - pad).floor().max(0.0) as usize).min(fw - 1);
    let y0 = ((y - pad).floor().max(0.0) as usize).min(fh - 1);
    let x1 = ((x + w + pad).ceil().max(0.0) as usize).clamp(x0 + 1, fw);
    let y1 = ((y + h + pad).ceil().max(0.0) as usize).clamp(y0 + 1, fh);
    Frame {
        origin: (frame.origin.0 + x0 as i32, frame.origin.1 + y0 as i32),
        width: (x1 - x0) as u32,
        height: (y1 - y0) as u32,
        pixels: (y0..y1)
            .flat_map(|y| frame.pixels[y * fw + x0..y * fw + x1].iter().copied())
            .collect(),
    }
}

/// How sure the installed matcher is that the text is one of its fonts,
/// from its best score: renders of the exact font score near 1, look-alikes
/// lower. Lens gets the rest of the weight.
fn installed_confidence(best: f32) -> f32 {
    let ramp = ((best - INSTALLED_LO) / (INSTALLED_HI - INSTALLED_LO)).clamp(0.0, 1.0);
    MIN_SHARE + (1.0 - 2.0 * MIN_SHARE) * ramp
}

/// Combines the two rankings. Lens knows open-source families (mostly
/// Google Fonts), the renderer knows what is installed, and the two barely
/// overlap, so each side's ranking is weighted by how sure the renderer is
/// that the font is installed. A family both name gets both shares.
fn merge(installed: Vec<FontMatch>, lens: &[fontnet::Prediction], top: usize) -> Vec<FontMatch> {
    let Some(best) = installed.first().map(|m| m.score) else {
        return Vec::new();
    };
    let c = installed_confidence(best);
    // Installed scores to a distribution: sharp, since exact renders stand
    // well clear of look-alikes.
    let exps: Vec<f32> = installed
        .iter()
        .map(|m| ((m.score - best) / INSTALLED_TEMP).exp())
        .collect();
    let total: f32 = exps.iter().sum();
    let mut out: Vec<FontMatch> = installed
        .into_iter()
        .zip(exps)
        .map(|(m, e)| FontMatch {
            score: c * e / total,
            ..m
        })
        .collect();
    for p in lens {
        let share = (1.0 - c) * p.prob;
        match out
            .iter_mut()
            .find(|m| m.family.eq_ignore_ascii_case(&p.family))
        {
            Some(m) => {
                m.score += share;
                m.source = Source::Both;
            }
            None => out.push(FontMatch {
                family: p.family.clone(),
                style: FontStyle::Regular,
                score: share,
                source: Source::Lens,
            }),
        }
    }
    out.sort_by(|a, b| b.score.total_cmp(&a.score));
    out.truncate(top);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_lines_and_merges_tiny_gaps() {
        let w = 3;
        let rows = [0.0, 1.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0];
        let ink: Vec<f32> = rows.iter().flat_map(|&v| [v; 3]).collect();
        assert_eq!(line_bands(&ink, w, rows.len()), [(1, 5), (8, 10)]);
    }

    #[test]
    fn correlation_of_an_image_with_itself_is_one() {
        let img = GrayImage::from_fn(10, 4, |x, y| Luma([((x * 7 + y * 13) % 255) as u8]));
        assert!((correlation(&img, &img) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn finds_a_font_it_drew() {
        // Draw "Hamburgefonstiv" in Arial (black on white) and expect Arial
        // (or one of its metric clones) near the top.
        let wide: Vec<u16> = "Hamburgefonstiv".encode_utf16().collect();
        let Some(img) = render(&wide, "Arial", FontStyle::Regular) else {
            return; // No Arial on this machine.
        };
        let target = normalize(&img, (img.width() * NORM_H) / img.height());
        let mut best = ("".to_owned(), -1.0f32);
        for fam in ["Arial", "Times New Roman", "Courier New", "Georgia"] {
            if let Some(r) = render(&wide, fam, FontStyle::Regular) {
                let c = correlation(&normalize(&r, target.width()), &target);
                if c > best.1 {
                    best = (fam.to_owned(), c);
                }
            }
        }
        assert_eq!(best.0, "Arial");
    }

    /// End to end (OCR plus every installed font). Slow in debug builds:
    /// `cargo test -p nook-platform --release -- --ignored --nocapture`.
    /// `text` drawn in `family`, black on white with a margin, its ink
    /// scaled to `ink_h` pixels high (screen text is small).
    fn screenshot(text: &str, family: &str, ink_h: u32) -> Option<Frame> {
        let wide: Vec<u16> = text.encode_utf16().collect();
        let img = render(&wide, family, FontStyle::Regular)?;
        let w = (img.width() * ink_h).div_ceil(img.height());
        let img = imageops::resize(&img, w, ink_h, imageops::FilterType::Triangle);
        let m = ink_h;
        let (fw, fh) = (img.width() + 2 * m, img.height() + 2 * m);
        let pixels = (0..fh)
            .flat_map(|y| (0..fw).map(move |x| (x, y)))
            .map(|(x, y)| {
                let inside = x >= m && y >= m && x < fw - m && y < fh - m;
                let v = if inside {
                    255 - u32::from(img.get_pixel(x - m, y - m).0[0])
                } else {
                    255
                };
                0xFF00_0000 | (v << 16) | (v << 8) | v
            })
            .collect();
        Some(Frame {
            origin: (0, 0),
            width: fw,
            height: fh,
            pixels,
        })
    }

    #[test]
    #[ignore]
    fn detects_georgia_end_to_end() {
        let frame = screenshot("The quick brown fox jumps", "Georgia", 64).expect("Georgia");
        let started = std::time::Instant::now();
        let d = detect(&frame, 5).expect("detect");
        println!("{:?} in {:?}", d.text, started.elapsed());
        for m in &d.matches {
            println!("  {:.3} {} {}", m.score, m.family, m.style.label());
        }
        assert_eq!(d.matches[0].family, "Georgia");
    }

    /// Accuracy report over installed fonts and, optionally, font files
    /// that are not installed (`NOOK_EVAL_FONTS="path.ttf=Family;..."`: each
    /// is loaded privately to draw the sample, then unloaded before
    /// detection). Point `NOOK_FONTNET_DIR` at the Lens model to include it.
    /// `cargo test -p nook-platform --release eval_fonts -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn eval_fonts() {
        use windows::Win32::Graphics::Gdi::{
            AddFontResourceExW, FR_PRIVATE, RemoveFontResourceExW,
        };
        use windows::core::PCWSTR;
        const TEXT: &str = "Sphinx of black quartz judge my vow";
        let mut cases: Vec<(String, Option<String>)> = [
            "Segoe UI",
            "Arial",
            "Georgia",
            "Calibri",
            "Consolas",
            "Times New Roman",
            "Verdana",
            "Cascadia Code",
        ]
        .iter()
        .map(|f| ((*f).to_owned(), None))
        .collect();
        if let Ok(list) = std::env::var("NOOK_EVAL_FONTS") {
            for item in list.split(';').filter(|s| !s.is_empty()) {
                if let Some((path, fam)) = item.split_once('=') {
                    cases.push((fam.to_owned(), Some(path.to_owned())));
                }
            }
        }
        let (mut hits, mut top3, mut n) = (0, 0, 0);
        for (family, file) in &cases {
            let wide_path = file.as_ref().map(|p| crate::wide(p));
            for ink_h in [18, 40] {
                // SAFETY: private font for this process, removed right after.
                if let Some(p) = &wide_path {
                    unsafe { AddFontResourceExW(PCWSTR(p.as_ptr()), FR_PRIVATE, None) };
                }
                let frame = screenshot(TEXT, family, ink_h);
                if let Some(p) = &wide_path {
                    let _ =
                        unsafe { RemoveFontResourceExW(PCWSTR(p.as_ptr()), FR_PRIVATE.0, None) };
                }
                let Some(frame) = frame else {
                    println!("{family}: cannot draw");
                    continue;
                };
                match detect(&frame, 5) {
                    Ok(d) => {
                        n += 1;
                        let rank = d.matches.iter().position(|m| m.family == *family);
                        hits += usize::from(rank == Some(0));
                        top3 += usize::from(rank.is_some_and(|r| r < 3));
                        let list: Vec<String> = d
                            .matches
                            .iter()
                            .take(3)
                            .map(|m| format!("{} {:.2} {:?}", m.family, m.score, m.source))
                            .collect();
                        println!("{family} @{ink_h}px -> {}", list.join(" | "));
                    }
                    Err(e) => println!("{family} @{ink_h}px: {e}"),
                }
            }
        }
        println!("top-1 {hits}/{n}, top-3 {top3}/{n}");
    }
}

#[cfg(test)]
mod fallback_tests {
    use super::*;

    /// The image-only path (OCR read nothing). Needs `NOOK_FONTNET_DIR`.
    #[test]
    #[ignore]
    fn lens_only_names_a_font() {
        let wide: Vec<u16> = "Oswald".encode_utf16().collect();
        let img = render(&wide, "Bahnschrift", FontStyle::Regular).expect("Bahnschrift");
        let frame = Frame {
            origin: (0, 0),
            width: img.width(),
            height: img.height(),
            pixels: img
                .pixels()
                .map(|p| {
                    let v = 255 - u32::from(p.0[0]);
                    0xFF00_0000 | (v << 16) | (v << 8) | v
                })
                .collect(),
        };
        let d = lens_only(std::slice::from_ref(&frame), frame.clone(), 5).expect("lens");
        println!(
            "{:?}",
            d.matches
                .iter()
                .map(|m| (&m.family, m.score))
                .collect::<Vec<_>>()
        );
        assert!(d.text.is_empty() && d.matches[0].source == Source::Lens);
    }
}
