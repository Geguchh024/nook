//! Low-level painting: squircle fills with vertical gradients, the glass
//! surface (fill + grain + rim + top highlight), glows, icons and rings.

use crate::geometry::Squircle;
use crate::theme::{FAMILY_ICON_FILL, FAMILY_ICON_REGULAR, Theme, with_alpha};
use egui::epaint::{Mesh, PathShape, PathStroke, Vertex};
use egui::{
    Align2, Color32, ColorImage, Context, FontFamily, FontId, Painter, Pos2, Shape, Stroke,
    TextureHandle, TextureId, TextureOptions, TextureWrapMode, pos2, vec2,
};

const NOISE_SIZE: usize = 128;

/// Vertical-gradient fill of a convex outline (fan triangulation from the
/// centroid; colors are linear in y, so the interpolation is exact).
pub fn fill_gradient(painter: &Painter, outline: &[Pos2], top: Color32, bottom: Color32) {
    if outline.len() < 3 {
        return;
    }
    let (points, triangles) = triangulate(outline);
    if triangles.is_empty() {
        return;
    }
    let (min_y, max_y) = outline.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| {
        (lo.min(p.y), hi.max(p.y))
    });
    let span = (max_y - min_y).max(1.0);
    let color_at = |y: f32| lerp_color(top, bottom, (y - min_y) / span);
    let mut mesh = Mesh::default();
    for p in &points {
        mesh.colored_vertex(*p, color_at(p.y));
    }
    for [a, b, c] in triangles {
        mesh.add_triangle(a, b, c);
    }
    painter.add(Shape::mesh(mesh));
}

/// Film-grain overlay clipped to the outline, using a tiling noise texture.
///
/// The texture is mapped in physical pixels (one texel covers `grain_px`
/// screen pixels), so the grain stays fine at any display scaling instead
/// of growing into blotches at 150-200%.
pub fn fill_noise(
    painter: &Painter,
    outline: &[Pos2],
    texture: TextureId,
    alpha: u8,
    grain_px: f32,
) {
    if outline.len() < 3 || alpha == 0 {
        return;
    }
    let (points, triangles) = triangulate(outline);
    if triangles.is_empty() {
        return;
    }
    let tint = Color32::from_white_alpha(alpha);
    let scale = painter.ctx().pixels_per_point() / (NOISE_SIZE as f32 * grain_px.max(1.0));
    let uv = |p: Pos2| pos2(p.x * scale, p.y * scale);
    let mut mesh = Mesh::with_texture(texture);
    for p in &points {
        mesh.vertices.push(Vertex {
            pos: *p,
            uv: uv(*p),
            color: tint,
        });
    }
    for [a, b, c] in triangles {
        mesh.add_triangle(a, b, c);
    }
    painter.add(Shape::mesh(mesh));
}

/// Ear clipping triangulation for simple polygons. Unlike a centroid fan,
/// this keeps concave notch ears inside their outline in every mesh pass.
fn triangulate(outline: &[Pos2]) -> (Vec<Pos2>, Vec<[u32; 3]>) {
    let mut points = Vec::with_capacity(outline.len());
    for &p in outline {
        if points.last().is_none_or(|q: &Pos2| q.distance_sq(p) > 1e-6) {
            points.push(p);
        }
    }
    if points.len() > 2
        && points
            .last()
            .is_some_and(|last| points[0].distance_sq(*last) <= 1e-6)
    {
        points.pop();
    }
    if points.len() < 3 {
        return (points, Vec::new());
    }
    let area = points.iter().enumerate().fold(0.0, |sum, (i, a)| {
        let b = points[(i + 1) % points.len()];
        sum + a.x * b.y - b.x * a.y
    });
    let orientation = area.signum();
    if orientation == 0.0 {
        return (points, Vec::new());
    }
    let mut ring: Vec<usize> = (0..points.len()).collect();
    let mut triangles = Vec::with_capacity(points.len() - 2);
    let mut guard = ring.len() * ring.len();
    while ring.len() > 3 && guard > 0 {
        guard -= 1;
        let mut clipped = false;
        for at in 0..ring.len() {
            let ia = ring[(at + ring.len() - 1) % ring.len()];
            let ib = ring[at];
            let ic = ring[(at + 1) % ring.len()];
            let (a, b, c) = (points[ia], points[ib], points[ic]);
            let cross = (b - a).x * (c - b).y - (b - a).y * (c - b).x;
            if cross * orientation <= 1e-5 {
                continue;
            }
            let occupied = ring.iter().copied().any(|ip| {
                if ip == ia || ip == ib || ip == ic {
                    return false;
                }
                let p = points[ip];
                let c1 = (b - a).x * (p.y - a.y) - (b - a).y * (p.x - a.x);
                let c2 = (c - b).x * (p.y - b.y) - (c - b).y * (p.x - b.x);
                let c3 = (a - c).x * (p.y - c.y) - (a - c).y * (p.x - c.x);
                c1 * orientation >= -1e-5 && c2 * orientation >= -1e-5 && c3 * orientation >= -1e-5
            });
            if !occupied {
                triangles.push([ia as u32, ib as u32, ic as u32]);
                ring.remove(at);
                clipped = true;
                break;
            }
        }
        if !clipped {
            return (points, Vec::new());
        }
    }
    if ring.len() == 3 {
        triangles.push([ring[0] as u32, ring[1] as u32, ring[2] as u32]);
    }
    (points, triangles)
}

/// The shell's glass surface at shell `progress` (0 = pill, 1 = panel).
/// With `fill = false` the compositor draws the tint, rim and highlight (see
/// `nook_platform::backdrop`) and only the grain is painted here.
pub fn glass(
    painter: &Painter,
    shape: &Squircle,
    outline: &[Pos2],
    theme: &Theme,
    opacity: f32,
    progress: f32,
    fill: bool,
) {
    let o = |c: Color32| with_alpha(c, opacity);
    if fill && theme.shell_shadow > 0.01 {
        glow(painter, shape, Color32::BLACK, theme.shell_shadow * opacity);
    }
    if fill {
        let (top, bottom) = theme.glass_at(progress);
        fill_gradient(painter, outline, o(top), o(bottom));
    }
    let noise = noise_texture(painter.ctx());
    fill_noise(
        painter,
        outline,
        noise.id(),
        (theme.noise_alpha as f32 * opacity) as u8,
        theme.grain_px,
    );
    if !fill {
        return;
    }
    painter.add(PathShape::closed_line(
        outline.to_vec(),
        PathStroke::new(1.0, o(theme.rim_at(progress))),
    ));
    top_highlight(painter, shape, o(theme.highlight_at(progress)));
}

/// 1 px highlight along the straight part of the top edge, fading out
/// before the corners begin (so it never pokes outside the curve).
pub fn top_highlight(painter: &Painter, shape: &Squircle, color: Color32) {
    let r = shape.radius;
    let y = shape.rect.min.y + 0.75;
    let x0 = shape.rect.min.x + r;
    let x1 = shape.rect.max.x - r;
    if x1 <= x0 {
        return;
    }
    let mid = 0.5 * (x0 + x1);
    let fade = Color32::TRANSPARENT;
    let mut mesh = Mesh::default();
    let h = 1.0;
    for (x, c) in [(x0, fade), (mid, color), (x1, fade)] {
        mesh.colored_vertex(pos2(x, y), c);
        mesh.colored_vertex(pos2(x, y + h), c);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 2, 3);
    mesh.add_triangle(2, 3, 4);
    mesh.add_triangle(3, 4, 5);
    painter.add(Shape::mesh(mesh));
}

/// Soft glow: restrained, expanding outline strokes under a shape.
/// Strokes avoid the large filled wedges produced by convex polygon
/// tessellation on layered, transformed overlay windows.
pub fn glow(painter: &Painter, shape: &Squircle, color: Color32, strength: f32) {
    if strength <= 0.01 {
        return;
    }
    for (grow, width, alpha) in [(6.0, 7.0, 0.025), (3.5, 4.0, 0.06), (1.5, 2.0, 0.12)] {
        let corners = crate::geometry::Corners {
            tl: shape.corners.tl + grow,
            tr: shape.corners.tr + grow,
            br: shape.corners.br + grow,
            bl: shape.corners.bl + grow,
        };
        let s = Squircle::new(shape.rect.expand(grow), shape.radius + grow)
            .with_corners(corners)
            .with_exponent(shape.exponent);
        painter.add(Shape::Path(PathShape::closed_line(
            s.outline(),
            PathStroke::new(width, with_alpha(color, alpha * strength)),
        )));
    }
}

/// A Phosphor glyph. `fill` selects the filled variant (active state).
pub fn icon(painter: &Painter, center: Pos2, glyph: &str, size: f32, color: Color32, fill: bool) {
    let family = if fill {
        FAMILY_ICON_FILL
    } else {
        FAMILY_ICON_REGULAR
    };
    painter.text(
        center,
        Align2::CENTER_CENTER,
        glyph,
        FontId::new(size, FontFamily::Name(family.into())),
        color,
    );
}

/// Progress ring; `progress` in 0..1 runs clockwise from 12 o'clock.
pub fn ring(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    width: f32,
    progress: f32,
    track: Color32,
    color: Color32,
) {
    painter.circle_stroke(center, radius, Stroke::new(width, track));
    let progress = progress.clamp(0.0, 1.0);
    if progress <= 0.0 {
        return;
    }
    let segments = ((64.0 * progress).ceil() as usize).max(2);
    let start = -std::f32::consts::FRAC_PI_2;
    let points: Vec<Pos2> = (0..=segments)
        .map(|i| {
            let a = start + std::f32::consts::TAU * progress * i as f32 / segments as f32;
            center + vec2(a.cos(), a.sin()) * radius
        })
        .collect();
    painter.add(PathShape::line(points, PathStroke::new(width, color)));
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    // Premultiplied components interpolate correctly for alpha blending.
    let t = t.clamp(0.0, 1.0);
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(
        l(a.r(), b.r()),
        l(a.g(), b.g()),
        l(a.b(), b.b()),
        l(a.a(), b.a()),
    )
}

/// Lazily created tiling grain texture (deterministic, generated once).
///
/// Each texel is a light or a dark speck (premultiplied white or black)
/// whose alpha follows a triangular distribution around zero, so the grain
/// adds texture without washing the glass lighter.
pub fn noise_texture(ctx: &Context) -> TextureHandle {
    let key = egui::Id::NULL.with("nook-noise");
    if let Some(tex) = ctx.data(|d| d.get_temp::<TextureHandle>(key)) {
        return tex;
    }
    let mut seed: u32 = 0x9E37_79B9;
    let mut next = || {
        // xorshift32
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed >> 24) as i32
    };
    let mut pixels = Vec::with_capacity(NOISE_SIZE * NOISE_SIZE);
    for _ in 0..NOISE_SIZE * NOISE_SIZE {
        // Difference of two uniforms: -255..=255, mostly near zero.
        let v = next() - next();
        let a = v.unsigned_abs().min(255) as u8;
        pixels.push(if v >= 0 {
            Color32::from_rgba_premultiplied(a, a, a, a)
        } else {
            Color32::from_rgba_premultiplied(0, 0, 0, a)
        });
    }
    let image = ColorImage::new([NOISE_SIZE, NOISE_SIZE], pixels);
    let tex = ctx.load_texture(
        "nook-noise",
        image,
        TextureOptions {
            wrap_mode: TextureWrapMode::Repeat,
            ..TextureOptions::NEAREST
        },
    );
    ctx.data_mut(|d| d.insert_temp(key, tex.clone()));
    tex
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{notch_outline, polygon_contains};
    use egui::{Rect, pos2, vec2};

    #[test]
    fn concave_notch_mesh_stays_inside_the_outline() {
        let outline = notch_outline(
            Rect::from_min_size(pos2(100.0, 0.0), vec2(200.0, 100.0)),
            12.0,
            20.0,
            8,
        );
        let (points, triangles) = triangulate(&outline);
        assert_eq!(triangles.len(), points.len() - 2);
        for [a, b, c] in triangles {
            let center = (points[a as usize].to_vec2()
                + points[b as usize].to_vec2()
                + points[c as usize].to_vec2())
                / 3.0;
            assert!(polygon_contains(&outline, center.to_pos2()));
        }
    }
}
