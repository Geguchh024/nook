//! Text recognition with the OCR engine built into Windows
//! (`Windows.Media.Ocr`), the one PowerToys Text Extractor and Text Grab
//! use. It runs on the device, needs no package identity (tested from the
//! plain exe), and reads a typical selection in well under 100 ms.
//!
//! It only knows the languages whose OCR packs are installed (Settings,
//! Time & language, Language & region). Two tweaks from those projects make
//! it noticeably better on screen text:
//!
//! - **Upscaling.** The engine is tuned for text larger than most UI fonts,
//!   so small selections are scaled up (bilinear, up to 3x) first.
//! - **Padding.** Glyphs touching the edge of the image are often dropped;
//!   the image gets a margin filled with its own border color.
//!
//! Lines are joined with line breaks (or spaces in single-line mode).
//! Chinese and Japanese results come back with a space between every
//! character, so those spaces are removed.

use crate::capture::Frame;
use crate::{Result, Win32Context};
use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapAlphaMode, BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::core::HSTRING;

/// Selections whose longer edge is under this get scaled up.
const UPSCALE_BELOW: u32 = 1200;
const MAX_UPSCALE: u32 = 3;
/// Margin added around the image, in pixels (after scaling).
const PADDING: u32 = 24;

/// An installed OCR language: BCP-47 tag and display name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OcrLanguage {
    pub tag: String,
    pub name: String,
}

/// Languages the engine can read on this machine.
pub fn languages() -> Vec<OcrLanguage> {
    let Ok(list) = OcrEngine::AvailableRecognizerLanguages() else {
        return Vec::new();
    };
    list.into_iter()
        .filter_map(|l| {
            Some(OcrLanguage {
                tag: l.LanguageTag().ok()?.to_string(),
                name: l.DisplayName().ok()?.to_string(),
            })
        })
        .collect()
}

/// A recognized word and its box in the input frame's pixels.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OcrWord {
    pub text: String,
    /// Index into [`OcrText::lines`].
    pub line: usize,
    /// `(x, y, width, height)`.
    pub rect: (f32, f32, f32, f32),
}

/// Recognized text, one entry per line, top to bottom.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OcrText {
    pub lines: Vec<String>,
    /// Every word, in reading order.
    pub words: Vec<OcrWord>,
    /// The language the engine used.
    pub language: String,
}

impl OcrText {
    /// The text for the clipboard: lines on their own rows (CRLF), or all
    /// on one line.
    pub fn joined(&self, single_line: bool) -> String {
        let sep = if single_line { " " } else { "\r\n" };
        self.lines.join(sep)
    }
}

/// Reads the text in `frame`. `language` is a BCP-47 tag; `None` uses the
/// first of the user's display languages the engine supports.
pub fn recognize(frame: &Frame, language: Option<&str>) -> Result<OcrText> {
    let engine = match language {
        Some(tag) => {
            let lang = Language::CreateLanguage(&HSTRING::from(tag)).api("Language")?;
            OcrEngine::TryCreateFromLanguage(&lang).api("OcrEngine::TryCreateFromLanguage")?
        }
        None => OcrEngine::TryCreateFromUserProfileLanguages()
            .api("OcrEngine::TryCreateFromUserProfileLanguages")?,
    };
    let tag = engine
        .RecognizerLanguage()
        .and_then(|l| l.LanguageTag())
        .map(|t| t.to_string())
        .unwrap_or_default();

    let max = OcrEngine::MaxImageDimension().unwrap_or(10_000);
    let prepared = prepare(frame, max);
    let writer = DataWriter::new().api("DataWriter")?;
    writer
        .WriteBytes(&prepared.bgra_bytes())
        .api("DataWriter::WriteBytes")?;
    let buffer = writer.DetachBuffer().api("DataWriter::DetachBuffer")?;
    let bitmap = SoftwareBitmap::CreateCopyWithAlphaFromBuffer(
        &buffer,
        BitmapPixelFormat::Bgra8,
        prepared.width as i32,
        prepared.height as i32,
        BitmapAlphaMode::Premultiplied,
    )
    .api("SoftwareBitmap::CreateCopyWithAlphaFromBuffer")?;
    let result = engine
        .RecognizeAsync(&bitmap)
        .api("OcrEngine::RecognizeAsync")?
        .join()
        .api("OcrEngine::RecognizeAsync(join)")?;

    // Word boxes come back in the prepared image: undo padding and scale.
    let sx = (prepared.width - 2 * PADDING) as f32 / frame.width.max(1) as f32;
    let sy = (prepared.height - 2 * PADDING) as f32 / frame.height.max(1) as f32;
    let pad = PADDING as f32;
    let squeeze = is_cjk(&tag);
    let mut lines = Vec::new();
    let mut words = Vec::new();
    for line in result.Lines().api("OcrResult::Lines")? {
        let text = line.Text().api("OcrLine::Text")?.to_string();
        let text = if squeeze { squeeze_cjk(&text) } else { text };
        if text.trim().is_empty() {
            continue;
        }
        for word in line.Words().api("OcrLine::Words")? {
            let r = word.BoundingRect().api("OcrWord::BoundingRect")?;
            words.push(OcrWord {
                text: word.Text().api("OcrWord::Text")?.to_string(),
                line: lines.len(),
                rect: (
                    (r.X - pad) / sx,
                    (r.Y - pad) / sy,
                    r.Width / sx,
                    r.Height / sy,
                ),
            });
        }
        lines.push(text);
    }
    Ok(OcrText {
        lines,
        words,
        language: tag,
    })
}

/// Scale factor for a selection of `w` x `h`, keeping under `max`.
fn upscale_factor(w: u32, h: u32, max: u32) -> u32 {
    let long = w.max(h).max(1);
    if long >= UPSCALE_BELOW {
        return 1;
    }
    let want = UPSCALE_BELOW.div_ceil(long).clamp(1, MAX_UPSCALE);
    let fits = (max.saturating_sub(2 * PADDING) / long).max(1);
    want.min(fits)
}

/// Upscales and pads `frame` for the engine, and keeps it under `max`.
fn prepare(frame: &Frame, max: u32) -> Frame {
    let factor = upscale_factor(frame.width, frame.height, max);
    let mut img = image::DynamicImage::ImageRgb8(frame.to_rgb_image());
    let room = max.saturating_sub(2 * PADDING).max(1);
    if factor > 1 {
        img = img.resize_exact(
            frame.width * factor,
            frame.height * factor,
            image::imageops::FilterType::Triangle,
        );
    } else if img.width().max(img.height()) > room {
        img = img.resize(room, room, image::imageops::FilterType::Triangle);
    }
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    let fill = border_color(&rgb);
    let (pw, ph) = (w + 2 * PADDING, h + 2 * PADDING);
    let mut pixels = vec![fill; (pw * ph) as usize];
    for (x, y, p) in rgb.enumerate_pixels() {
        let [r, g, b] = p.0;
        pixels[((y + PADDING) * pw + x + PADDING) as usize] =
            0xFF00_0000 | (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b);
    }
    Frame {
        origin: frame.origin,
        width: pw,
        height: ph,
        pixels,
    }
}

/// Average color of the outermost pixels, as opaque BGRA.
fn border_color(img: &image::RgbImage) -> u32 {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return 0xFFFF_FFFF;
    }
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    let mut add = |x: u32, y: u32| {
        let p = img.get_pixel(x, y).0;
        for (s, c) in sum.iter_mut().zip(p) {
            *s += u64::from(c);
        }
        n += 1;
    };
    for x in 0..w {
        add(x, 0);
        add(x, h - 1);
    }
    for y in 0..h {
        add(0, y);
        add(w - 1, y);
    }
    let avg = |i: usize| (sum[i] / n.max(1)) as u32;
    0xFF00_0000 | (avg(0) << 16) | (avg(1) << 8) | avg(2)
}

fn is_cjk(tag: &str) -> bool {
    let lang = tag.split('-').next().unwrap_or_default();
    matches!(lang, "zh" | "ja")
}

fn is_cjk_char(c: char) -> bool {
    matches!(c,
        '\u{3000}'..='\u{30FF}' // punctuation, hiragana, katakana
        | '\u{3400}'..='\u{4DBF}'
        | '\u{4E00}'..='\u{9FFF}'
        | '\u{F900}'..='\u{FAFF}'
        | '\u{FF00}'..='\u{FFEF}') // full-width forms
}

/// Drops spaces that sit between two CJK characters.
fn squeeze_cjk(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    for (i, &c) in chars.iter().enumerate() {
        if c == ' '
            && i > 0
            && i + 1 < chars.len()
            && is_cjk_char(chars[i - 1])
            && is_cjk_char(chars[i + 1])
        {
            continue;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upscales_small_selections_only() {
        assert_eq!(upscale_factor(200, 40, 10_000), 3);
        assert_eq!(upscale_factor(700, 100, 10_000), 2);
        assert_eq!(upscale_factor(1600, 300, 10_000), 1);
        // Never past the engine's limit.
        assert_eq!(upscale_factor(900, 10, 1000), 1);
    }

    #[test]
    fn pads_with_the_border_color() {
        let frame = Frame {
            origin: (0, 0),
            width: 2000,
            height: 2,
            pixels: vec![0xFF10_2030; 4000],
        };
        let p = prepare(&frame, 10_000);
        assert_eq!((p.width, p.height), (2000 + 2 * PADDING, 2 + 2 * PADDING));
        assert_eq!(p.pixels[0], 0xFF10_2030);
    }

    #[test]
    fn squeezes_cjk_spaces() {
        assert_eq!(squeeze_cjk("日 本 語 の text 文"), "日本語の text 文");
        assert!(is_cjk("ja-JP") && is_cjk("zh-Hans-CN") && !is_cjk("ko"));
    }

    #[test]
    fn joins_lines() {
        let t = OcrText {
            lines: vec!["a b".into(), "c".into()],
            language: "en".into(),
            ..Default::default()
        };
        assert_eq!(t.joined(false), "a b\r\nc");
        assert_eq!(t.joined(true), "a b c");
    }
}
