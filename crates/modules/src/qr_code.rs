//! QR code: turns a link (or any text) into a branded QR code.
//!
//! Paste a link and the code draws itself as you type. Pick how its modules
//! look (dots, liquid blobs, round lines, rounded or square), how the three
//! corner eyes look, a color or gradient and a background, put your logo
//! in the middle and a label under it. Copy it as an image, save it as PNG
//! or SVG, or drag the preview straight into another app.
//!
//! The preview is the exported image itself, rendered at screen resolution
//! by the same anti-aliased rasterizer, so what you see is what you get.
//! Codes use error correction Medium, or High when a logo covers the middle
//! (High survives 30% of the code missing). Ctrl+Alt+Q makes a code from
//! the clipboard.

use egui::epaint::Mesh;
use egui::{
    Align2, Color32, ColorImage, Id, Pos2, Rect, Sense, Shape, TextureHandle, TextureOptions, Ui,
    pos2, vec2,
};
use image::RgbaImage;
use nook_platform::capture::Frame;
use nook_platform::{clipboard, files, raster, shell, system};
use nook_ui::module::{Module, TileCtx, TileSize};
use nook_ui::theme::{SIZE_BODY, SIZE_LABEL, Theme, mix};
use nook_ui::{icons, widgets};
use qrcodegen::{QrCode, QrCodeEcc};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver};
use std::time::Instant;

use crate::kit;

const PAGE: egui::Vec2 = vec2(740.0, 440.0);
/// Quiet zone around the code, in modules.
const QUIET: f32 = 3.0;
/// Share of the code's width the logo plate covers.
const LOGO_SHARE: f32 = 0.24;
/// Exported image width, pixels.
const EXPORT: u32 = 1200;
const SETTINGS: &str = "qr.txt";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Body {
    Dots,
    Liquid,
    Lines,
    Rounded,
    Square,
}

const BODIES: [(Body, &str); 5] = [
    (Body::Dots, "Dots"),
    (Body::Liquid, "Liquid"),
    (Body::Lines, "Lines"),
    (Body::Rounded, "Rounded"),
    (Body::Square, "Square"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Eyes {
    Round,
    Circle,
    Leaf,
    Square,
}

const EYES: [(Eyes, &str); 4] = [
    (Eyes::Round, "Round"),
    (Eyes::Circle, "Circle"),
    (Eyes::Leaf, "Leaf"),
    (Eyes::Square, "Square"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Background {
    White,
    Cream,
    Night,
    Clear,
}

const BACKGROUNDS: [(Background, &str); 4] = [
    (Background::White, "White"),
    (Background::Cream, "Cream"),
    (Background::Night, "Night"),
    (Background::Clear, "Clear"),
];

impl Background {
    fn rgb(self) -> Option<[u8; 3]> {
        match self {
            Background::White => Some([255, 255, 255]),
            Background::Cream => Some([255, 248, 236]),
            Background::Night => Some([17, 19, 24]),
            Background::Clear => None,
        }
    }
}

/// Gradient stops; equal stops are a solid color. The accent palette is
/// filled in from the theme.
const PALETTES: [(&str, [u8; 3], [u8; 3]); 6] = [
    ("Ink", [17, 17, 20], [17, 17, 20]),
    ("Accent", [0, 0, 0], [0, 0, 0]),
    ("Violet", [124, 58, 237], [219, 39, 119]),
    ("Ocean", [6, 182, 212], [37, 99, 235]),
    ("Forest", [16, 185, 129], [6, 95, 70]),
    ("Sunset", [245, 158, 11], [225, 29, 72]),
];
const ACCENT: usize = 1;

#[derive(Clone, Debug, PartialEq)]
struct Style {
    body: Body,
    eyes: Eyes,
    palette: usize,
    background: Background,
    label: String,
    accent: [u8; 3],
}

impl Default for Style {
    fn default() -> Self {
        Self {
            body: Body::Liquid,
            eyes: Eyes::Round,
            palette: 2,
            background: Background::White,
            label: String::new(),
            accent: [124, 92, 255],
        }
    }
}

impl Style {
    /// Gradient stops for the code on this background.
    fn colors(&self) -> ([u8; 3], [u8; 3]) {
        let (_, a, b) = PALETTES[self.palette.min(PALETTES.len() - 1)];
        if self.palette == ACCENT {
            let c = Color32::from_rgb(self.accent[0], self.accent[1], self.accent[2]);
            let d = mix(c, Color32::BLACK, 0.35);
            return (self.accent, [d.r(), d.g(), d.b()]);
        }
        if self.palette == 0 && self.background == Background::Night {
            return ([244, 244, 246], [244, 244, 246]);
        }
        (a, b)
    }

    /// The label is drawn only on an opaque background.
    fn label(&self) -> Option<&str> {
        let l = self.label.trim();
        (!l.is_empty() && self.background != Background::Clear).then_some(l)
    }
}

// ------------------------------------------------------------- geometry

/// A shape in module units, origin at the code's top-left module.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Shape2 {
    /// Rounded rectangle; corner radii top-left, top-right, bottom-right,
    /// bottom-left.
    Rect {
        min: [f32; 2],
        max: [f32; 2],
        r: [f32; 4],
    },
    Circle {
        c: [f32; 2],
        r: f32,
    },
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Prim {
    shape: Shape2,
    /// Cuts out of what is drawn before it (the eye rings).
    hole: bool,
}

fn rect(x0: f32, y0: f32, x1: f32, y1: f32, r: [f32; 4]) -> Shape2 {
    Shape2::Rect {
        min: [x0, y0],
        max: [x1, y1],
        r,
    }
}

impl Shape2 {
    fn bounds(&self) -> ([f32; 2], [f32; 2]) {
        match *self {
            Shape2::Rect { min, max, .. } => (min, max),
            Shape2::Circle { c, r } => ([c[0] - r, c[1] - r], [c[0] + r, c[1] + r]),
        }
    }

    /// Signed distance from `p` (negative inside).
    fn distance(&self, p: [f32; 2]) -> f32 {
        match *self {
            Shape2::Circle { c, r } => ((p[0] - c[0]).hypot(p[1] - c[1])) - r,
            Shape2::Rect { min, max, r } => {
                let hx = (max[0] - min[0]) * 0.5;
                let hy = (max[1] - min[1]) * 0.5;
                let qx = p[0] - (min[0] + hx);
                let qy = p[1] - (min[1] + hy);
                let rr = match (qx >= 0.0, qy >= 0.0) {
                    (false, false) => r[0],
                    (true, false) => r[1],
                    (true, true) => r[2],
                    (false, true) => r[3],
                }
                .min(hx)
                .min(hy);
                let dx = qx.abs() - hx + rr;
                let dy = qy.abs() - hy + rr;
                let outside = dx.max(0.0).hypot(dy.max(0.0));
                outside + dx.max(dy).min(0.0) - rr
            }
        }
    }
}

/// Where the three finder patterns ("eyes") sit, top-left corners.
fn eye_origins(n: i32) -> [(i32, i32); 3] {
    [(0, 0), (n - 7, 0), (0, n - 7)]
}

fn in_eye(n: i32, x: i32, y: i32) -> bool {
    eye_origins(n)
        .iter()
        .any(|&(ex, ey)| x >= ex && x < ex + 7 && y >= ey && y < ey + 7)
}

/// The logo plate, in module units (min, side).
fn plate(n: i32) -> ([f32; 2], f32) {
    let side = (n as f32 * LOGO_SHARE).round() + 1.0;
    let c = n as f32 * 0.5;
    ([c - side * 0.5, c - side * 0.5], side)
}

/// The code as shapes. `logo` clears the modules under the plate.
fn shapes(qr: &QrCode, body: Body, eyes: Eyes, logo: bool) -> Vec<Prim> {
    let n = qr.size();
    let (pmin, side) = plate(n);
    let cleared = |x: i32, y: i32| {
        logo && (x as f32 + 1.0) > pmin[0]
            && (x as f32) < pmin[0] + side
            && (y as f32 + 1.0) > pmin[1]
            && (y as f32) < pmin[1] + side
    };
    let dark = |x: i32, y: i32| {
        x >= 0
            && y >= 0
            && x < n
            && y < n
            && qr.get_module(x, y)
            && !in_eye(n, x, y)
            && !cleared(x, y)
    };
    let mut out = Vec::new();
    let mut push = |shape: Shape2| out.push(Prim { shape, hole: false });
    for y in 0..n {
        for x in 0..n {
            if !dark(x, y) {
                continue;
            }
            let (fx, fy) = (x as f32, y as f32);
            match body {
                Body::Dots => push(Shape2::Circle {
                    c: [fx + 0.5, fy + 0.5],
                    r: 0.43,
                }),
                Body::Liquid => {
                    let free = |dx: i32, dy: i32| !dark(x + dx, y) && !dark(x, y + dy);
                    let r = |dx, dy| if free(dx, dy) { 0.5 } else { 0.0 };
                    push(rect(
                        fx,
                        fy,
                        fx + 1.0,
                        fy + 1.0,
                        [r(-1, -1), r(1, -1), r(1, 1), r(-1, 1)],
                    ));
                }
                Body::Lines => {
                    // One capsule per vertical run, started at its top.
                    if dark(x, y - 1) {
                        continue;
                    }
                    let mut end = y;
                    while dark(x, end + 1) {
                        end += 1;
                    }
                    push(rect(
                        fx + 0.14,
                        fy + 0.07,
                        fx + 0.86,
                        end as f32 + 0.93,
                        [0.36; 4],
                    ));
                }
                Body::Rounded => push(rect(fx + 0.06, fy + 0.06, fx + 0.94, fy + 0.94, [0.3; 4])),
                Body::Square => push(rect(fx, fy, fx + 1.0, fy + 1.0, [0.0; 4])),
            }
        }
    }
    for (ex, ey) in eye_origins(n) {
        let (x, y) = (ex as f32, ey as f32);
        let ring = |inset: f32, r: [f32; 4]| {
            rect(x + inset, y + inset, x + 7.0 - inset, y + 7.0 - inset, r)
        };
        let (outer, hole, pupil) = match eyes {
            Eyes::Round => (
                ring(0.0, [2.4; 4]),
                ring(1.0, [1.6; 4]),
                ring(2.0, [1.0; 4]),
            ),
            Eyes::Leaf => (
                ring(0.0, [3.0, 0.4, 3.0, 0.4]),
                ring(1.0, [2.1, 0.25, 2.1, 0.25]),
                ring(2.0, [1.3, 0.15, 1.3, 0.15]),
            ),
            Eyes::Square => (
                ring(0.0, [0.0; 4]),
                ring(1.0, [0.0; 4]),
                ring(2.0, [0.0; 4]),
            ),
            Eyes::Circle => {
                let c = [x + 3.5, y + 3.5];
                (
                    Shape2::Circle { c, r: 3.5 },
                    Shape2::Circle { c, r: 2.5 },
                    Shape2::Circle { c, r: 1.5 },
                )
            }
        };
        out.push(Prim {
            shape: outer,
            hole: false,
        });
        out.push(Prim {
            shape: hole,
            hole: true,
        });
        out.push(Prim {
            shape: pupil,
            hole: false,
        });
    }
    out
}

// ------------------------------------------------------------- rendering

/// Straight-alpha RGBA pixels.
struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// Image layout for a code of `n` modules drawn `size` pixels wide.
struct Layout {
    /// Pixels per module.
    scale: f32,
    /// Top-left of the first module.
    origin: [f32; 2],
    height: u32,
    /// Label baseline band: center y and text size.
    label: Option<(f32, f32)>,
}

fn layout(n: i32, size: u32, style: &Style) -> Layout {
    let scale = size as f32 / (n as f32 + 2.0 * QUIET);
    let origin = [QUIET * scale, QUIET * scale];
    let band = size as f32 * 0.11;
    let (height, label) = match style.label() {
        Some(_) => (
            (size as f32 + band).round() as u32,
            Some((
                size as f32 - QUIET * scale * 0.2 + band * 0.5,
                size as f32 * 0.058,
            )),
        ),
        None => (size, None),
    };
    Layout {
        scale,
        origin,
        height,
        label,
    }
}

/// Coverage of the shapes, one value per pixel.
fn coverage(prims: &[Prim], lay: &Layout, width: u32, height: u32) -> Vec<f32> {
    let (w, h) = (width as i32, height as i32);
    let mut mask = vec![0.0f32; (width * height) as usize];
    let s = lay.scale;
    for prim in prims {
        let (min, max) = prim.shape.bounds();
        let x0 = ((lay.origin[0] + min[0] * s).floor() as i32 - 1).max(0);
        let y0 = ((lay.origin[1] + min[1] * s).floor() as i32 - 1).max(0);
        let x1 = ((lay.origin[0] + max[0] * s).ceil() as i32 + 1).min(w);
        let y1 = ((lay.origin[1] + max[1] * s).ceil() as i32 + 1).min(h);
        for py in y0..y1 {
            let my = (py as f32 + 0.5 - lay.origin[1]) / s;
            for px in x0..x1 {
                let mx = (px as f32 + 0.5 - lay.origin[0]) / s;
                let d = prim.shape.distance([mx, my]) * s;
                let cov = (0.5 - d).clamp(0.0, 1.0);
                if cov <= 0.0 {
                    continue;
                }
                let m = &mut mask[(py * w + px) as usize];
                // Shapes tile without overlapping, so coverage adds up
                // exactly along shared edges (no seams).
                *m = if prim.hole {
                    *m * (1.0 - cov)
                } else {
                    (*m + cov).min(1.0)
                };
            }
        }
    }
    mask
}

fn lerp3(a: [u8; 3], b: [u8; 3], t: f32) -> [f32; 3] {
    let l = |x: u8, y: u8| x as f32 + (y as f32 - x as f32) * t;
    [l(a[0], b[0]), l(a[1], b[1]), l(a[2], b[2])]
}

fn render(qr: &QrCode, style: &Style, logo: Option<&RgbaImage>, size: u32) -> Image {
    let n = qr.size();
    let lay = layout(n, size, style);
    let (width, height) = (size, lay.height);
    let prims = shapes(qr, style.body, style.eyes, logo.is_some());
    let mask = coverage(&prims, &lay, width, height);
    let (ca, cb) = style.colors();
    let bg = style.background.rgb();
    let span = n as f32 * lay.scale * 2.0;
    let mut rgba = vec![0u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let i = (y * width + x) as usize;
            let m = mask[i];
            let t = ((x as f32 - lay.origin[0]) + (y as f32 - lay.origin[1])) / span;
            let fg = lerp3(ca, cb, t.clamp(0.0, 1.0));
            let px = &mut rgba[i * 4..i * 4 + 4];
            match bg {
                Some(b) => {
                    for k in 0..3 {
                        px[k] = (b[k] as f32 * (1.0 - m) + fg[k] * m).round() as u8;
                    }
                    px[3] = 255;
                }
                None => {
                    for k in 0..3 {
                        px[k] = fg[k].round() as u8;
                    }
                    px[3] = (m * 255.0).round() as u8;
                }
            }
        }
    }
    let mut img = Image {
        width,
        height,
        rgba,
    };
    if let Some(logo) = logo {
        draw_logo(&mut img, &lay, n, bg.unwrap_or([255, 255, 255]), logo);
    }
    if let (Some(text), Some((cy, px))) = (style.label(), lay.label) {
        draw_label(&mut img, text, cy, px, ca);
    }
    img
}

/// The plate in the middle and the logo fitted on it.
fn draw_logo(img: &mut Image, lay: &Layout, n: i32, plate_rgb: [u8; 3], logo: &RgbaImage) {
    let (pmin, side) = plate(n);
    let s = lay.scale;
    let shape = rect(
        pmin[0] + 0.15,
        pmin[1] + 0.15,
        pmin[0] + side - 0.15,
        pmin[1] + side - 0.15,
        [side * 0.24; 4],
    );
    let (w, h) = (img.width as i32, img.height as i32);
    let x0 = (lay.origin[0] + pmin[0] * s).floor() as i32 - 1;
    let y0 = (lay.origin[1] + pmin[1] * s).floor() as i32 - 1;
    let x1 = (lay.origin[0] + (pmin[0] + side) * s).ceil() as i32 + 1;
    let y1 = (lay.origin[1] + (pmin[1] + side) * s).ceil() as i32 + 1;
    for py in y0.max(0)..y1.min(h) {
        for px in x0.max(0)..x1.min(w) {
            let p = [
                (px as f32 + 0.5 - lay.origin[0]) / s,
                (py as f32 + 0.5 - lay.origin[1]) / s,
            ];
            let cov = (0.5 - shape.distance(p) * s).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend_px(img, px as u32, py as u32, plate_rgb, cov);
            }
        }
    }
    // The logo, fitted inside the plate with a margin.
    let inner = (side - 1.4) * s;
    let (lw, lh) = (logo.width() as f32, logo.height() as f32);
    let k = inner / lw.max(lh);
    let (tw, th) = (
        (lw * k).round().max(1.0) as u32,
        (lh * k).round().max(1.0) as u32,
    );
    let scaled = image::imageops::resize(logo, tw, th, image::imageops::FilterType::CatmullRom);
    let cx = lay.origin[0] + (pmin[0] + side * 0.5) * s;
    let cy = lay.origin[1] + (pmin[1] + side * 0.5) * s;
    let ox = (cx - tw as f32 * 0.5).round() as i32;
    let oy = (cy - th as f32 * 0.5).round() as i32;
    for (x, y, p) in scaled.enumerate_pixels() {
        let (px, py) = (ox + x as i32, oy + y as i32);
        if px < 0 || py < 0 || px >= w || py >= h || p[3] == 0 {
            continue;
        }
        blend_px(
            img,
            px as u32,
            py as u32,
            [p[0], p[1], p[2]],
            p[3] as f32 / 255.0,
        );
    }
}

fn blend_px(img: &mut Image, x: u32, y: u32, rgb: [u8; 3], a: f32) {
    let i = ((y * img.width + x) * 4) as usize;
    let px = &mut img.rgba[i..i + 4];
    let da = px[3] as f32 / 255.0;
    let oa = a + da * (1.0 - a);
    if oa <= 0.0 {
        return;
    }
    for k in 0..3 {
        px[k] = ((rgb[k] as f32 * a + px[k] as f32 * da * (1.0 - a)) / oa).round() as u8;
    }
    px[3] = (oa * 255.0).round() as u8;
}

/// The label, centered at `cy` (the background is opaque here).
fn draw_label(img: &mut Image, text: &str, cy: f32, size: f32, rgb: [u8; 3]) {
    let mut pixels: Vec<u32> = img
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| 0xFF00_0000 | (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2]))
        .collect();
    let (tw, th) = raster::text_size(text, size);
    let mut canvas = raster::Canvas {
        pixels: &mut pixels,
        width: img.width,
        height: img.height,
    };
    let x = (img.width as f32 - tw) * 0.5;
    if let Err(e) = canvas.text((x, cy - th * 0.5), text, size, rgb) {
        tracing::debug!("qr label: {e}");
        return;
    }
    for (p, c) in img.rgba.as_chunks_mut::<4>().0.iter_mut().zip(pixels) {
        p[0] = (c >> 16) as u8;
        p[1] = (c >> 8) as u8;
        p[2] = c as u8;
    }
}

// ------------------------------------------------------------- export

fn svg_shape(s: &Shape2, k: f32, o: [f32; 2], d: &mut String) {
    use std::fmt::Write;
    let f = |v: f32| (v * 100.0).round() / 100.0;
    match *s {
        Shape2::Circle { c, r } => {
            let (cx, cy, r) = (o[0] + c[0] * k, o[1] + c[1] * k, r * k);
            let _ = write!(
                d,
                "M{} {}a{} {} 0 1 0 {} 0a{} {} 0 1 0 {} 0Z",
                f(cx - r),
                f(cy),
                f(r),
                f(r),
                f(2.0 * r),
                f(r),
                f(r),
                f(-2.0 * r)
            );
        }
        Shape2::Rect { min, max, r } => {
            let (x0, y0) = (o[0] + min[0] * k, o[1] + min[1] * k);
            let (x1, y1) = (o[0] + max[0] * k, o[1] + max[1] * k);
            let lim = (x1 - x0).min(y1 - y0) * 0.5;
            let [tl, tr, br, bl] = r.map(|v| (v * k).min(lim));
            let arc = |d: &mut String, r: f32, x: f32, y: f32| {
                if r > 0.0 {
                    let _ = write!(d, "A{} {} 0 0 1 {} {}", f(r), f(r), f(x), f(y));
                }
            };
            let _ = write!(d, "M{} {}H{}", f(x0 + tl), f(y0), f(x1 - tr));
            arc(d, tr, x1, y0 + tr);
            let _ = write!(d, "V{}", f(y1 - br));
            arc(d, br, x1 - br, y1);
            let _ = write!(d, "H{}", f(x0 + bl));
            arc(d, bl, x0, y1 - bl);
            let _ = write!(d, "V{}", f(y0 + tl));
            arc(d, tl, x0 + tl, y0);
            d.push('Z');
        }
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

fn svg(qr: &QrCode, style: &Style, logo: Option<&RgbaImage>, size: u32) -> String {
    use std::fmt::Write;
    let n = qr.size();
    let lay = layout(n, size, style);
    let (k, o) = (lay.scale, lay.origin);
    let (ca, cb) = style.colors();
    let mut s = String::new();
    let _ = write!(
        s,
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {h}" width="{size}" height="{h}">"#,
        h = lay.height
    );
    let q = n as f32 * k;
    let _ = write!(
        s,
        r#"<defs><linearGradient id="g" gradientUnits="userSpaceOnUse" x1="{}" y1="{}" x2="{}" y2="{}"><stop offset="0" stop-color="{}"/><stop offset="1" stop-color="{}"/></linearGradient></defs>"#,
        o[0],
        o[1],
        o[0] + q,
        o[1] + q,
        hex(ca),
        hex(cb)
    );
    if let Some(bg) = style.background.rgb() {
        let _ = write!(
            s,
            r#"<rect width="{size}" height="{}" fill="{}"/>"#,
            lay.height,
            hex(bg)
        );
    }
    let mut d = String::new();
    for p in shapes(qr, style.body, style.eyes, logo.is_some()) {
        svg_shape(&p.shape, k, o, &mut d);
    }
    let _ = write!(s, r#"<path fill="url(#g)" fill-rule="evenodd" d="{d}"/>"#);
    if let Some(logo) = logo {
        let (pmin, side) = plate(n);
        let mut pd = String::new();
        svg_shape(
            &rect(
                pmin[0] + 0.15,
                pmin[1] + 0.15,
                pmin[0] + side - 0.15,
                pmin[1] + side - 0.15,
                [side * 0.24; 4],
            ),
            k,
            o,
            &mut pd,
        );
        let plate_rgb = style.background.rgb().unwrap_or([255, 255, 255]);
        let _ = write!(s, r#"<path fill="{}" d="{pd}"/>"#, hex(plate_rgb));
        let mut png = std::io::Cursor::new(Vec::new());
        if logo.write_to(&mut png, image::ImageFormat::Png).is_ok() {
            let inner = (side - 1.4) * k;
            let x = o[0] + (pmin[0] + side * 0.5) * k - inner * 0.5;
            let y = o[1] + (pmin[1] + side * 0.5) * k - inner * 0.5;
            let _ = write!(
                s,
                r#"<image x="{x}" y="{y}" width="{inner}" height="{inner}" preserveAspectRatio="xMidYMid meet" href="data:image/png;base64,{}"/>"#,
                base64(&png.into_inner())
            );
        }
    }
    if let (Some(text), Some((cy, px))) = (style.label(), lay.label) {
        let _ = write!(
            s,
            r#"<text x="{}" y="{cy}" text-anchor="middle" dominant-baseline="central" font-family="Segoe UI, Helvetica, Arial, sans-serif" font-weight="600" font-size="{px}" fill="{}">{}</text>"#,
            size as f32 * 0.5,
            hex(ca),
            xml_escape(text)
        );
    }
    s.push_str("</svg>");
    s
}

/// A short file-name-safe name for the text, e.g. `github.com`.
fn slug(text: &str) -> String {
    let t = text.trim();
    let t = t
        .strip_prefix("https://")
        .or_else(|| t.strip_prefix("http://"))
        .unwrap_or(t);
    let t = t.strip_prefix("www.").unwrap_or(t);
    let head = t.split(['/', '?', '#']).next().unwrap_or("");
    let s: String = head
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
        .take(40)
        .collect();
    if s.is_empty() { "qr".into() } else { s }
}

fn looks_like_link(s: &str) -> bool {
    let s = s.trim();
    !s.contains('\n')
        && (s.starts_with("http://") || s.starts_with("https://") || s.starts_with("www."))
}

// ------------------------------------------------------------- module

struct Logo {
    path: PathBuf,
    image: Arc<RgbaImage>,
    tex: Option<TextureHandle>,
}

fn load_logo(path: &Path) -> Result<RgbaImage, String> {
    let img = image::open(path).map_err(|_| "Use a PNG, JPG or BMP logo".to_owned())?;
    // Big logos only cost time: the plate is a few hundred pixels at most.
    let img = if img.width().max(img.height()) > 600 {
        img.resize(600, 600, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    Ok(img.to_rgba8())
}

fn is_image(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "bmp"
        )
    })
}

/// The preview texture and what it was rendered from.
struct Preview {
    key: (String, Style, Option<PathBuf>, u32),
    tex: TextureHandle,
    aspect: f32,
}

#[derive(Default)]
pub struct QrCodeModule {
    text: String,
    style: Style,
    logo: Option<Logo>,
    /// Encoded code for `text` (and whether a logo forced High).
    code: Option<(String, bool, Result<QrCode, String>)>,
    preview: Option<Preview>,
    tile: Option<Preview>,
    open_page: bool,
    show: bool,
    /// Last frame the page drew (for accepting dropped logos).
    page_seen: Option<Instant>,
    picker: Option<Receiver<Option<PathBuf>>>,
    ctx: Option<egui::Context>,
    /// Feedback under the buttons, and when it was set.
    toast: Option<(String, Instant)>,
    dirty: bool,
}

impl QrCodeModule {
    fn load_settings(&mut self) {
        for (k, v) in kit::read_settings(SETTINGS) {
            match k.as_str() {
                "text" => self.text = v,
                "label" => self.style.label = v,
                "body" => {
                    if let Some(b) = BODIES.iter().find(|(_, n)| *n == v) {
                        self.style.body = b.0;
                    }
                }
                "eyes" => {
                    if let Some(e) = EYES.iter().find(|(_, n)| *n == v) {
                        self.style.eyes = e.0;
                    }
                }
                "background" => {
                    if let Some(b) = BACKGROUNDS.iter().find(|(_, n)| *n == v) {
                        self.style.background = b.0;
                    }
                }
                "palette" => {
                    if let Some(i) = PALETTES.iter().position(|(n, ..)| *n == v) {
                        self.style.palette = i;
                    }
                }
                "logo" if !v.is_empty() => self.set_logo(PathBuf::from(v)),
                _ => {}
            }
        }
    }

    fn save_settings(&self) {
        let body = BODIES
            .iter()
            .find(|b| b.0 == self.style.body)
            .map_or("", |b| b.1);
        let eyes = EYES
            .iter()
            .find(|e| e.0 == self.style.eyes)
            .map_or("", |e| e.1);
        let bg = BACKGROUNDS
            .iter()
            .find(|b| b.0 == self.style.background)
            .map_or("", |b| b.1);
        kit::write_settings(
            SETTINGS,
            &[
                ("text", self.text.clone()),
                ("label", self.style.label.clone()),
                ("body", body.to_owned()),
                ("eyes", eyes.to_owned()),
                ("background", bg.to_owned()),
                ("palette", PALETTES[self.style.palette].0.to_owned()),
                (
                    "logo",
                    self.logo
                        .as_ref()
                        .map(|l| l.path.display().to_string())
                        .unwrap_or_default(),
                ),
            ],
        );
    }

    fn set_logo(&mut self, path: PathBuf) {
        match load_logo(&path) {
            Ok(img) => {
                self.logo = Some(Logo {
                    path,
                    image: Arc::new(img),
                    tex: None,
                });
                self.dirty = true;
            }
            Err(e) => self.toast(e),
        }
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now()));
    }

    /// The code for the current text, re-encoded when it changed.
    fn code(&mut self) -> Option<&Result<QrCode, String>> {
        let text = self.text.trim().to_owned();
        if text.is_empty() {
            self.code = None;
            return None;
        }
        let high = self.logo.is_some();
        let stale = self
            .code
            .as_ref()
            .is_none_or(|(t, h, _)| *t != text || *h != high);
        if stale {
            let ecc = if high {
                QrCodeEcc::High
            } else {
                QrCodeEcc::Medium
            };
            let res = QrCode::encode_text(&text, ecc)
                .map_err(|_| "Too long for a QR code. Try a shorter link.".to_owned());
            self.code = Some((text, high, res));
        }
        self.code.as_ref().map(|(_, _, r)| r)
    }

    fn export_image(&mut self) -> Option<Image> {
        let style = self.style.clone();
        let logo = self.logo.as_ref().map(|l| l.image.clone());
        let qr = self.code()?.as_ref().ok()?.clone();
        Some(render(&qr, &style, logo.as_deref(), EXPORT))
    }

    fn copy(&mut self) {
        let Some(img) = self.export_image() else {
            return;
        };
        // The clipboard has no transparency: clear codes go on white.
        let pixels = img
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .map(|p| {
                let a = p[3] as f32 / 255.0;
                let c = |v: u8| (v as f32 * a + 255.0 * (1.0 - a)).round() as u32;
                0xFF00_0000 | (c(p[0]) << 16) | (c(p[1]) << 8) | c(p[2])
            })
            .collect();
        let frame = Frame {
            origin: (0, 0),
            width: img.width,
            height: img.height,
            pixels,
        };
        match clipboard::copy_image(&frame) {
            Ok(()) => self.toast("Copied. Paste it anywhere."),
            Err(e) => self.toast(format!("Could not copy: {e}")),
        }
    }

    fn out_dir() -> Option<PathBuf> {
        shell::pictures_dir().map(|p| p.join("QR codes"))
    }

    fn save_png(&mut self, path: &Path) -> Result<(), String> {
        let img = self.export_image().ok_or("Nothing to save")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        RgbaImage::from_raw(img.width, img.height, img.rgba)
            .ok_or("bad image")?
            .save(path)
            .map_err(|e| e.to_string())
    }

    fn file_name(&self, ext: &str) -> String {
        format!("{} {}.{ext}", slug(&self.text), system::file_timestamp())
    }

    fn save(&mut self, svg_out: bool) {
        let Some(dir) = Self::out_dir() else {
            return;
        };
        let path = dir.join(self.file_name(if svg_out { "svg" } else { "png" }));
        let res = if svg_out {
            let style = self.style.clone();
            let logo = self.logo.as_ref().map(|l| l.image.clone());
            match self.code().and_then(|r| r.as_ref().ok()).cloned() {
                Some(qr) => std::fs::create_dir_all(&dir)
                    .and_then(|()| std::fs::write(&path, svg(&qr, &style, logo.as_deref(), EXPORT)))
                    .map_err(|e| e.to_string()),
                None => Err("Nothing to save".into()),
            }
        } else {
            self.save_png(&path)
        };
        match res {
            Ok(()) => {
                let _ = shell::reveal(&path);
                self.toast("Saved to Pictures\\QR codes");
            }
            Err(e) => self.toast(format!("Could not save: {e}")),
        }
    }

    /// Writes the PNG to a temporary file and drags it out.
    fn drag_out(&mut self) {
        let path = std::env::temp_dir()
            .join("Nook")
            .join(format!("QR {}.png", slug(&self.text)));
        match self.save_png(&path) {
            Ok(()) => {
                if let Err(e) = files::drag_out(&[path]) {
                    tracing::warn!("qr drag: {e}");
                }
            }
            Err(e) => self.toast(format!("Could not drag: {e}")),
        }
    }

    fn pick_logo(&mut self) {
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("nook-qr-logo".into())
            .spawn(move || {
                let picked = files::pick_files("Choose a logo (PNG, JPG or BMP)")
                    .ok()
                    .and_then(|v| v.into_iter().next());
                let _ = tx.send(picked);
                if let Some(c) = ctx {
                    c.request_repaint();
                }
            });
        if spawned.is_ok() {
            self.picker = Some(rx);
        }
    }

    fn page_visible(&self) -> bool {
        self.page_seen
            .is_some_and(|t| t.elapsed().as_secs_f32() < 0.5)
    }

    /// The rendered preview for `px` pixels wide, re-rendered on change.
    fn preview_tex(
        &mut self,
        ctx: &egui::Context,
        px: u32,
        tile: bool,
    ) -> Option<(TextureHandle, f32)> {
        let style = self.style.clone();
        let logo = if tile {
            None
        } else {
            self.logo
                .as_ref()
                .map(|l| (l.path.clone(), l.image.clone()))
        };
        let key = (
            self.text.trim().to_owned(),
            style.clone(),
            logo.as_ref().map(|l| l.0.clone()),
            px,
        );
        let slot = if tile { &self.tile } else { &self.preview };
        if let Some(p) = slot
            && p.key == key
        {
            return Some((p.tex.clone(), p.aspect));
        }
        let qr = self.code()?.as_ref().ok()?.clone();
        let img = render(&qr, &style, logo.as_ref().map(|l| l.1.as_ref()), px);
        let aspect = img.width as f32 / img.height as f32;
        let color = ColorImage::from_rgba_unmultiplied(
            [img.width as usize, img.height as usize],
            &img.rgba,
        );
        let tex = ctx.load_texture(
            if tile {
                "nook-qr-tile"
            } else {
                "nook-qr-preview"
            },
            color,
            TextureOptions::LINEAR,
        );
        let p = Preview {
            key,
            tex: tex.clone(),
            aspect,
        };
        if tile {
            self.tile = Some(p);
        } else {
            self.preview = Some(p);
        }
        Some((tex, aspect))
    }

    // ------------------------------------------------------- page parts

    fn preview_card(&mut self, ui: &mut Ui, card: Rect, theme: &Theme) {
        widgets::card(ui.painter(), card, theme);
        let area = card.shrink(20.0);
        let ctx = ui.ctx().clone();
        let error = match self.code() {
            None => {
                kit::empty(
                    ui,
                    card.center(),
                    icons::QR_CODE,
                    "Paste a link to begin",
                    theme,
                );
                return;
            }
            Some(Err(e)) => Some(e.clone()),
            Some(Ok(_)) => None,
        };
        if let Some(e) = error {
            kit::empty(ui, card.center(), icons::WARNING_CIRCLE, &e, theme);
            return;
        }
        let ppp = ctx.pixels_per_point();
        // Size the image to fit, label band included.
        let label_k = if self.style.label().is_some() {
            1.11
        } else {
            1.0
        };
        let side = area.width().min(area.height() / label_k);
        let px = (side * ppp).round() as u32;
        let Some((tex, aspect)) = self.preview_tex(&ctx, px, false) else {
            return;
        };
        let size = vec2(side, side / aspect);
        let img_rect = Rect::from_center_size(area.center(), size);
        if self.style.background == Background::Clear {
            checkerboard(ui.painter(), img_rect, theme);
        }
        let radius = if self.style.background == Background::Clear {
            0.0
        } else {
            14.0
        };
        egui::Image::new(&tex)
            .corner_radius(radius)
            .paint_at(ui, img_rect);
        if self.style.background == Background::Night {
            ui.painter().rect_stroke(
                img_rect,
                radius,
                egui::Stroke::new(1.0, theme.tile_rim),
                egui::StrokeKind::Outside,
            );
        }
        let resp = ui
            .interact(img_rect, Id::new("nook-qr-drag"), Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::Grab)
            .on_hover_text("Drag into any app  \u{00B7}  Click to copy");
        if resp.drag_started() {
            self.drag_out();
        } else if resp.clicked() {
            self.copy();
        }
    }

    fn controls(&mut self, ui: &mut Ui, col: Rect, theme: &Theme) {
        let mut y = col.min.y;
        let w = col.width();
        let before = (self.text.clone(), self.style.clone());

        // Content.
        let field = Rect::from_min_size(pos2(col.min.x, y), vec2(w, 36.0));
        let fid = Id::new("nook-qr-text");
        kit::field(
            ui,
            Rect::from_min_max(field.min, pos2(field.max.x - 36.0, field.max.y)),
            &mut self.text,
            "Paste a link or type any text",
            fid,
            Theme::font(SIZE_BODY),
            theme,
        );
        if kit::icon_at(
            ui,
            pos2(field.max.x - 16.0, field.center().y),
            28.0,
            icons::CLIPBOARD_TEXT,
            false,
            theme,
        )
        .on_hover_text("Paste from the clipboard")
        .clicked()
            && let Some(t) = clipboard::read_text()
        {
            self.text = t.trim().to_owned();
        }
        y += 48.0;

        let group = |ui: &mut Ui, y: &mut f32, title: &str| {
            kit::section(ui, pos2(col.min.x + 2.0, *y), title, theme);
            *y += 16.0;
            let r = Rect::from_min_size(pos2(col.min.x, *y), vec2(w, 26.0));
            *y += 26.0 + 12.0;
            r
        };

        let r = group(ui, &mut y, "STYLE");
        let labels: Vec<&str> = BODIES.iter().map(|b| b.1).collect();
        let sel = BODIES
            .iter()
            .position(|b| b.0 == self.style.body)
            .unwrap_or(0);
        if let Some(i) = kit::chips(ui, r, &labels, sel, theme) {
            self.style.body = BODIES[i].0;
        }

        let r = group(ui, &mut y, "CORNERS");
        let labels: Vec<&str> = EYES.iter().map(|e| e.1).collect();
        let sel = EYES
            .iter()
            .position(|e| e.0 == self.style.eyes)
            .unwrap_or(0);
        if let Some(i) = kit::chips(ui, r, &labels, sel, theme) {
            self.style.eyes = EYES[i].0;
        }

        let r = group(ui, &mut y, "COLOR");
        let step = w / PALETTES.len() as f32;
        for (i, (name, ..)) in PALETTES.iter().enumerate() {
            let c = pos2(r.min.x + step * (i as f32 + 0.5), r.center().y);
            let mut s = self.style.clone();
            s.palette = i;
            let (a, b) = s.colors();
            let hit = Rect::from_center_size(c, vec2(step, 28.0));
            let resp = ui
                .interact(hit, Id::new(("nook-qr-swatch", i)), Sense::click())
                .on_hover_text(*name);
            let selected = self.style.palette == i;
            let grow = if resp.hovered() { 1.0 } else { 0.0 };
            if selected {
                ui.painter()
                    .circle_stroke(c, 14.0, egui::Stroke::new(2.0, theme.text));
            }
            gradient_circle(ui.painter(), c, 10.5 + grow, a, b);
            if resp.clicked() {
                self.style.palette = i;
            }
        }

        let r = group(ui, &mut y, "BACKGROUND");
        let labels: Vec<&str> = BACKGROUNDS.iter().map(|b| b.1).collect();
        let sel = BACKGROUNDS
            .iter()
            .position(|b| b.0 == self.style.background)
            .unwrap_or(0);
        if let Some(i) = kit::chips(ui, r, &labels, sel, theme) {
            self.style.background = BACKGROUNDS[i].0;
        }

        // Logo and label.
        kit::section(ui, pos2(col.min.x + 2.0, y), "LOGO AND LABEL", theme);
        y += 16.0;
        let row = Rect::from_min_size(pos2(col.min.x, y), vec2(w, 32.0));
        let logo_r = Rect::from_min_size(row.min, vec2(118.0, row.height()));
        match &mut self.logo {
            Some(logo) => {
                let ctx = ui.ctx().clone();
                let tex = logo.tex.get_or_insert_with(|| {
                    let img = ColorImage::from_rgba_unmultiplied(
                        [logo.image.width() as usize, logo.image.height() as usize],
                        logo.image.as_raw(),
                    );
                    ctx.load_texture("nook-qr-logo", img, TextureOptions::LINEAR)
                });
                ui.painter().rect_filled(logo_r, 16.0, kit::wash(theme));
                let thumb = Rect::from_center_size(
                    pos2(logo_r.min.x + 18.0, logo_r.center().y),
                    vec2(22.0, 22.0),
                );
                egui::Image::new(&*tex)
                    .corner_radius(5.0)
                    .paint_at(ui, thumb);
                kit::one_line(
                    ui,
                    pos2(thumb.max.x + 8.0, logo_r.center().y - 8.0),
                    &logo
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    logo_r.width() - 66.0,
                    Theme::font(SIZE_LABEL),
                    theme.text,
                );
                if kit::icon_at(
                    ui,
                    pos2(logo_r.max.x - 14.0, logo_r.center().y),
                    22.0,
                    icons::X,
                    false,
                    theme,
                )
                .on_hover_text("Remove the logo")
                .clicked()
                {
                    self.logo = None;
                    self.dirty = true;
                }
            }
            None => {
                if kit::button(
                    ui,
                    logo_r,
                    "Add logo",
                    Some(icons::IMAGE_SQUARE),
                    false,
                    theme,
                )
                .on_hover_text("Choose an image, or drop one on the notch")
                .clicked()
                {
                    self.pick_logo();
                }
            }
        }
        let label_r = Rect::from_min_max(pos2(logo_r.max.x + 8.0, row.min.y), row.max);
        if self.style.background == Background::Clear {
            ui.painter().rect_filled(label_r, 10.0, kit::wash(theme));
            ui.painter().text(
                label_r.center(),
                Align2::CENTER_CENTER,
                "Labels need a background",
                Theme::font(SIZE_LABEL),
                theme.text_faint,
            );
        } else {
            kit::field(
                ui,
                label_r,
                &mut self.style.label,
                "Label, e.g. Scan to visit",
                Id::new("nook-qr-label"),
                Theme::font(SIZE_LABEL + 1.0),
                theme,
            );
        }

        // Status and export.
        let bottom = col.max.y;
        let buttons = Rect::from_min_max(pos2(col.min.x, bottom - 34.0), pos2(col.max.x, bottom));
        let details = match self.code() {
            Some(Ok(qr)) => Some(format!(
                "Version {}  \u{00B7}  {n}\u{00D7}{n} modules  \u{00B7}  error correction {}",
                qr.version().value(),
                match qr.error_correction_level() {
                    QrCodeEcc::Low => "L",
                    QrCodeEcc::Medium => "M",
                    QrCodeEcc::Quartile => "Q",
                    QrCodeEcc::High => "H",
                },
                n = qr.size()
            )),
            _ => None,
        };
        let info = match (&self.toast, details) {
            (Some((t, at)), _) if at.elapsed().as_secs_f32() < 4.0 => {
                Some((t.clone(), theme.accent_fg()))
            }
            (_, Some(d)) => Some((d, theme.text_faint)),
            _ => None,
        };
        if let Some((text, color)) = info {
            kit::one_line(
                ui,
                pos2(col.min.x + 2.0, buttons.min.y - 22.0),
                &text,
                w,
                Theme::font(SIZE_LABEL - 1.0),
                color,
            );
        }
        let ready = matches!(self.code(), Some(Ok(_)));
        ui.add_enabled_ui(ready, |ui| {
            let bw = (w - 16.0) / 3.0;
            let b = |i: usize| {
                Rect::from_min_size(
                    pos2(buttons.min.x + i as f32 * (bw + 8.0), buttons.min.y),
                    vec2(bw, buttons.height()),
                )
            };
            if kit::button(ui, b(0), "Copy", Some(icons::COPY), true, theme).clicked() {
                self.copy();
            }
            if kit::button(ui, b(1), "PNG", Some(icons::DOWNLOAD_SIMPLE), false, theme)
                .on_hover_text("Save a 1200 px PNG to Pictures\\QR codes")
                .clicked()
            {
                self.save(false);
            }
            if kit::button(ui, b(2), "SVG", Some(icons::BEZIER_CURVE), false, theme)
                .on_hover_text("Save a vector SVG to Pictures\\QR codes")
                .clicked()
            {
                self.save(true);
            }
        });

        if (self.text.clone(), self.style.clone()) != before {
            self.dirty = true;
        }
    }
}

/// A circle filled with a diagonal gradient.
fn gradient_circle(painter: &egui::Painter, c: Pos2, r: f32, a: [u8; 3], b: [u8; 3]) {
    let col = |t: f32| {
        let v = lerp3(a, b, t);
        Color32::from_rgb(v[0] as u8, v[1] as u8, v[2] as u8)
    };
    let mut mesh = Mesh::default();
    mesh.colored_vertex(c, col(0.5));
    let n = 32;
    for i in 0..n {
        let ang = std::f32::consts::TAU * i as f32 / n as f32;
        let p = c + vec2(ang.cos(), ang.sin()) * r;
        let t = ((p.x - c.x) + (p.y - c.y)) / (4.0 * r) + 0.5;
        mesh.colored_vertex(p, col(t));
    }
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    painter.add(Shape::mesh(mesh));
}

fn checkerboard(painter: &egui::Painter, rect: Rect, theme: &Theme) {
    let (a, b) = if theme.dark {
        (Color32::from_gray(58), Color32::from_gray(44))
    } else {
        (Color32::from_gray(236), Color32::from_gray(214))
    };
    painter.rect_filled(rect, 0.0, a);
    let cell = 10.0;
    let cols = (rect.width() / cell).ceil() as i32;
    let rows = (rect.height() / cell).ceil() as i32;
    for y in 0..rows {
        for x in 0..cols {
            if (x + y) % 2 == 0 {
                continue;
            }
            let r = Rect::from_min_size(
                rect.min + vec2(x as f32 * cell, y as f32 * cell),
                vec2(cell, cell),
            )
            .intersect(rect);
            painter.rect_filled(r, 0.0, b);
        }
    }
}

impl Module for QrCodeModule {
    fn id(&self) -> &'static str {
        "qr_code"
    }

    fn name(&self) -> &str {
        "QR code"
    }

    fn icon(&self) -> &'static str {
        icons::QR_CODE
    }

    fn size_hint(&self) -> TileSize {
        TileSize::Small
    }

    fn attach(&mut self, ctx: &egui::Context) {
        self.ctx = Some(ctx.clone());
        self.load_settings();
    }

    fn on_tick(&mut self) {
        if let Some(rx) = &self.picker
            && let Ok(picked) = rx.try_recv()
        {
            self.picker = None;
            if let Some(p) = picked {
                self.set_logo(p);
            }
        }
        if self.dirty {
            self.dirty = false;
            self.save_settings();
        }
    }

    fn next_repaint(&self, _tile_visible: bool) -> Option<std::time::Duration> {
        // Clear the toast when it times out.
        self.toast
            .as_ref()
            .and_then(|(_, at)| std::time::Duration::from_secs(4).checked_sub(at.elapsed()))
    }

    fn take_page_request(&mut self) -> bool {
        std::mem::take(&mut self.open_page)
    }

    fn take_show_request(&mut self) -> bool {
        std::mem::take(&mut self.show)
    }

    fn page_size(&self) -> Option<egui::Vec2> {
        Some(PAGE)
    }

    fn default_hotkey(&self) -> Option<&'static str> {
        Some("ctrl+alt+KeyQ")
    }

    /// Makes a code from the clipboard and shows it.
    fn on_hotkey(&mut self) {
        if let Some(t) = clipboard::read_text().filter(|t| !t.trim().is_empty()) {
            self.text = t.trim().to_owned();
            self.dirty = true;
        }
        self.show = true;
    }

    fn accepts_files(&self) -> bool {
        self.page_visible()
    }

    fn drop_files(&mut self, paths: Vec<PathBuf>) {
        match paths.into_iter().find(|p| is_image(p)) {
            Some(p) => self.set_logo(p),
            None => self.toast("Drop a PNG, JPG or BMP to use it as the logo"),
        }
    }

    fn ui_tile(&mut self, ui: &mut Ui, ctx: &mut TileCtx<'_>) {
        let rect = ui.max_rect();
        // Big tiles show the code itself.
        if rect.height() > 80.0 && !self.text.trim().is_empty() {
            let resp = ui.interact(rect.expand(12.0), ui.id().with("qr-tile"), Sense::click());
            let ppp = ui.ctx().pixels_per_point();
            let side = rect.width().min(rect.height());
            let egui_ctx = ui.ctx().clone();
            if let Some((tex, aspect)) =
                self.preview_tex(&egui_ctx, (side * ppp).round() as u32, true)
            {
                let r = Rect::from_center_size(rect.center(), vec2(side, side / aspect));
                egui::Image::new(&tex).corner_radius(8.0).paint_at(ui, r);
            }
            if resp.clicked() {
                self.open_page = true;
            }
            return;
        }
        if kit::tool_tile(ui, ctx, icons::QR_CODE, false)
            .on_hover_text("Make a QR code  \u{00B7}  Ctrl+Alt+Q uses the clipboard")
            .clicked()
        {
            if self.text.trim().is_empty()
                && let Some(t) = clipboard::read_text().filter(|t| looks_like_link(t))
            {
                self.text = t.trim().to_owned();
            }
            self.open_page = true;
        }
    }

    fn ui_page(&mut self, ui: &mut Ui, header: &mut Ui, body: Rect, theme: &Theme) -> bool {
        self.page_seen = Some(Instant::now());
        let a = theme.accent;
        self.style.accent = [a.r(), a.g(), a.b()];
        if !self.text.is_empty()
            && widgets::icon_button(header, icons::ARROW_COUNTER_CLOCKWISE, 26.0, false, theme)
                .on_hover_text("Start over")
                .clicked()
        {
            self.text.clear();
            self.style = Style {
                accent: self.style.accent,
                ..Style::default()
            };
            self.logo = None;
            self.dirty = true;
        }
        if widgets::icon_button(header, icons::FOLDER_OPEN, 26.0, false, theme)
            .on_hover_text("Open Pictures\\QR codes")
            .clicked()
            && let Some(dir) = Self::out_dir()
        {
            let _ = std::fs::create_dir_all(&dir);
            let _ = shell::open(&dir.display().to_string());
        }
        let side = body.height();
        let card = Rect::from_min_size(body.min, vec2(side, side));
        self.preview_card(ui, card, theme);
        let col = Rect::from_min_max(pos2(card.max.x + 18.0, body.min.y), body.max);
        self.controls(ui, col, theme);
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rounded_rect_distance() {
        let s = rect(0.0, 0.0, 2.0, 2.0, [0.0; 4]);
        assert!((s.distance([1.0, 1.0]) + 1.0).abs() < 1e-5);
        assert!((s.distance([3.0, 1.0]) - 1.0).abs() < 1e-5);
        let round = rect(0.0, 0.0, 2.0, 2.0, [1.0; 4]);
        // The corner is cut: a point near it is outside.
        assert!(round.distance([0.05, 0.05]) > 0.0);
        assert!(s.distance([0.05, 0.05]) < 0.0);
    }

    #[test]
    fn renders_every_style() {
        let qr = QrCode::encode_text("https://example.com", QrCodeEcc::High).unwrap();
        let logo = RgbaImage::from_pixel(8, 8, image::Rgba([255, 0, 0, 255]));
        for (body, _) in BODIES {
            for (eyes, _) in EYES {
                for (background, _) in BACKGROUNDS {
                    let style = Style {
                        body,
                        eyes,
                        background,
                        label: "Scan me".into(),
                        ..Style::default()
                    };
                    let img = render(&qr, &style, Some(&logo), 240);
                    assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
                    // The top-left eye's outer ring is drawn.
                    let lay = layout(qr.size(), 240, &style);
                    let x = (lay.origin[0] + 3.5 * lay.scale) as u32;
                    let y = (lay.origin[1] + 0.5 * lay.scale) as u32;
                    let i = ((y * img.width + x) * 4) as usize;
                    let px = &img.rgba[i..i + 4];
                    let bg = background.rgb().unwrap_or([0, 0, 0]);
                    assert!(px[3] > 200);
                    assert_ne!(
                        [px[0], px[1], px[2]],
                        bg,
                        "{body:?} {eyes:?} {background:?}"
                    );
                    let doc = svg(&qr, &style, Some(&logo), 240);
                    assert!(doc.starts_with("<svg") && doc.ends_with("</svg>"));
                }
            }
        }
    }

    #[test]
    fn slugs_and_links() {
        assert_eq!(slug("https://www.github.com/rust-lang?x=1"), "github.com");
        assert_eq!(slug("   "), "qr");
        assert!(looks_like_link("https://a.b"));
        assert!(!looks_like_link("hello"));
        assert_eq!(base64(b"Man"), "TWFu");
        assert_eq!(base64(b"Ma"), "TWE=");
    }
}
