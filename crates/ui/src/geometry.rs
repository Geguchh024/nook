//! Shell geometry: the continuous-corner "squircle" outline, hit-testing and
//! the bento grid packer. Pure math, no egui state, so it is unit-tested.

use egui::{Pos2, Rect, Vec2, pos2, vec2};

/// Exponent of the superellipse corners. 2.0 is a circular arc; higher values
/// give the smoother "continuous" corner where the curve eases into the edge.
pub const CORNER_EXPONENT: f32 = 4.0;

/// Points sampled per corner when building the outline polygon.
pub const CORNER_SEGMENTS: usize = 14;

/// A rounded rectangle whose corners are superellipse quadrants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Squircle {
    pub rect: Rect,
    pub radius: f32,
    /// Corner superellipse exponent (2 = circular arc).
    pub exponent: f32,
}

impl Squircle {
    pub fn new(rect: Rect, radius: f32) -> Self {
        let max_r = 0.5 * rect.width().min(rect.height());
        Self {
            rect,
            radius: radius.clamp(0.0, max_r.max(0.0)),
            exponent: CORNER_EXPONENT,
        }
    }

    /// Overrides the corner exponent (clamped to >= 2: below that the corner
    /// would pinch inward past a circular arc).
    pub fn with_exponent(mut self, exponent: f32) -> Self {
        self.exponent = exponent.max(2.0);
        self
    }

    /// Is `p` inside (or on) the outline?
    pub fn contains(&self, p: Pos2) -> bool {
        let r = self.radius;
        let half = self.rect.size() * 0.5;
        let d = (p - self.rect.center()).abs();
        if d.x > half.x || d.y > half.y {
            return false;
        }
        if r <= 0.0 {
            return true;
        }
        // Distance into the corner zone, normalized by the radius.
        let qx = ((d.x - (half.x - r)) / r).max(0.0);
        let qy = ((d.y - (half.y - r)) / r).max(0.0);
        qx.powf(self.exponent) + qy.powf(self.exponent) <= 1.0 + 1e-4
    }

    /// The outline as a convex polygon, clockwise in screen space starting at
    /// the top-right corner.
    pub fn outline(&self) -> Vec<Pos2> {
        let mut pts = Vec::with_capacity(4 * (CORNER_SEGMENTS + 1));
        self.outline_into(&mut pts);
        pts
    }

    pub fn outline_into(&self, pts: &mut Vec<Pos2>) {
        pts.clear();
        let r = self.radius;
        let Rect { min, max } = self.rect;
        // Corner centers and the quadrant each corner sweeps.
        // Corner centers in clockwise order (screen space, y down), each with
        // the start angle of its quarter sweep.
        let corners = [
            (pos2(max.x - r, min.y + r), -90.0_f32), // top-right
            (pos2(max.x - r, max.y - r), 0.0),       // bottom-right
            (pos2(min.x + r, max.y - r), 90.0),      // bottom-left
            (pos2(min.x + r, min.y + r), 180.0),     // top-left
        ];
        let e = 2.0 / self.exponent;
        let shape = |v: f32| v.signum() * v.abs().powf(e);
        for (c, start) in corners {
            for s in 0..=CORNER_SEGMENTS {
                let deg = start + 90.0 * s as f32 / CORNER_SEGMENTS as f32;
                let (sin, cos) = deg.to_radians().sin_cos();
                pts.push(c + vec2(shape(cos), shape(sin)) * r);
            }
        }
    }

    pub fn translate(self, delta: Vec2) -> Self {
        Self {
            rect: self.rect.translate(delta),
            ..self
        }
    }
}

/// Linear interpolation helper.
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Polygon outline for a top-edge notch. Ear arcs extend outside `rect` to
/// meet the screen edge; the remaining corners round the panel bottom.
pub fn notch_outline(rect: Rect, ear: f32, bottom: f32, steps: usize) -> Vec<Pos2> {
    use std::f32::consts::{FRAC_PI_2, PI};

    let (l, r, top, b) = (rect.left(), rect.right(), rect.top(), rect.bottom());
    let ear = ear.max(0.0).min(rect.width() * 0.25);
    let bottom = bottom
        .max(0.0)
        .min(rect.height() * 0.5)
        .min(rect.width() * 0.5);
    let steps = steps.max(1);
    let mut pts = Vec::with_capacity(steps * 4 + 8);
    let arc = |pts: &mut Vec<Pos2>, cx: f32, cy: f32, rad: f32, a0: f32, a1: f32| {
        if rad <= 0.0 {
            pts.push(pos2(cx, cy));
            return;
        }
        for i in 0..=steps {
            let a = a0 + (a1 - a0) * (i as f32 / steps as f32);
            pts.push(pos2(cx + rad * a.cos(), cy + rad * a.sin()));
        }
    };
    pts.push(pos2(l - ear, top));
    arc(&mut pts, l - ear, top + ear, ear, -FRAC_PI_2, 0.0);
    pts.push(pos2(l, b - bottom));
    arc(&mut pts, l + bottom, b - bottom, bottom, PI, FRAC_PI_2);
    pts.push(pos2(r - bottom, b));
    arc(&mut pts, r - bottom, b - bottom, bottom, FRAC_PI_2, 0.0);
    pts.push(pos2(r, top + ear));
    arc(&mut pts, r + ear, top + ear, ear, PI, 3.0 * FRAC_PI_2);
    pts.push(pos2(r + ear, top));
    pts
}

/// Even-odd point-in-polygon test for a closed polygon.
pub fn polygon_contains(pts: &[Pos2], p: Pos2) -> bool {
    if pts.len() < 3 {
        return false;
    }
    let mut inside = false;
    let mut j = pts.len() - 1;
    for i in 0..pts.len() {
        let (a, b) = (pts[i], pts[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

// ---------------------------------------------------------------------------
// Bento grid

/// Tile footprint in grid cells (columns x rows).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TileSize {
    /// 1x1
    Small,
    /// 2x1
    Wide,
    /// 1x2
    Tall,
    /// 2x2
    Large,
}

/// Semantic footprint passed to tile modules for responsive layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeClass {
    Small,
    Wide,
    Tall,
    Large,
}

impl From<TileSize> for SizeClass {
    fn from(size: TileSize) -> Self {
        match size {
            TileSize::Small => Self::Small,
            TileSize::Wide => Self::Wide,
            TileSize::Tall => Self::Tall,
            TileSize::Large => Self::Large,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orient {
    Horizontal,
    Vertical,
    Square,
}

pub fn orient_of(rect: Rect) -> Orient {
    let aspect = rect.width() / rect.height().max(f32::EPSILON);
    if aspect > 1.25 {
        Orient::Horizontal
    } else if aspect < 0.8 {
        Orient::Vertical
    } else {
        Orient::Square
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TileShape {
    Rounded,
    Circle,
}

/// Conservative content bounds for a tile shape, preventing labels and
/// controls from landing in heavily rounded corners or outside a circle.
pub fn safe_rect(shape: TileShape, rect: Rect, radius: f32) -> Rect {
    match shape {
        TileShape::Circle => {
            let side = rect.width().min(rect.height()) * 0.70;
            Rect::from_center_size(rect.center(), Vec2::splat(side))
        }
        TileShape::Rounded => {
            let inset = (radius.max(0.0) * 0.29)
                .max(8.0)
                .min(rect.width().min(rect.height()) * 0.25);
            rect.shrink(inset)
        }
    }
}

impl TileSize {
    pub const ALL: [TileSize; 4] = [
        TileSize::Small,
        TileSize::Wide,
        TileSize::Tall,
        TileSize::Large,
    ];

    pub fn cells(self) -> (usize, usize) {
        match self {
            TileSize::Small => (1, 1),
            TileSize::Wide => (2, 1),
            TileSize::Tall => (1, 2),
            TileSize::Large => (2, 2),
        }
    }

    /// Cells covered; smaller means the tile has less room than another.
    pub fn area(self) -> usize {
        let (w, h) = self.cells();
        w * h
    }
}

/// Grid metrics for the bento layout.
#[derive(Clone, Copy, Debug)]
pub struct GridSpec {
    pub columns: usize,
    pub rows: usize,
    pub cell: f32,
    pub gap: f32,
}

impl GridSpec {
    pub fn size(&self) -> Vec2 {
        vec2(
            self.columns as f32 * self.cell + (self.columns.saturating_sub(1)) as f32 * self.gap,
            self.rows as f32 * self.cell + (self.rows.saturating_sub(1)) as f32 * self.gap,
        )
    }

    pub fn cell_rect(&self, origin: Pos2, col: usize, row: usize, size: TileSize) -> Rect {
        let (w, h) = size.cells();
        let step = self.cell + self.gap;
        let min = origin + vec2(col as f32 * step, row as f32 * step);
        let span = vec2(
            w as f32 * self.cell + (w - 1) as f32 * self.gap,
            h as f32 * self.cell + (h - 1) as f32 * self.gap,
        );
        Rect::from_min_size(min, span)
    }
}

/// Where a tile landed in the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub index: usize,
    pub col: usize,
    pub row: usize,
}

/// Dense first-fit packing in column-major order (fill top-to-bottom, then
/// left-to-right), so 2x2 tiles and 1x1 stacks interleave naturally in a wide,
/// short panel. Tiles that do not fit are left out of the result.
pub fn pack(spec: &GridSpec, sizes: &[TileSize]) -> Vec<Placement> {
    let mut used = vec![false; spec.columns * spec.rows];
    let mut out = Vec::with_capacity(sizes.len());
    for (index, size) in sizes.iter().enumerate() {
        let (w, h) = size.cells();
        'search: for col in 0..spec.columns {
            for row in 0..spec.rows {
                if col + w > spec.columns || row + h > spec.rows {
                    continue;
                }
                let free =
                    (col..col + w).all(|c| (row..row + h).all(|r| !used[r * spec.columns + c]));
                if free {
                    for c in col..col + w {
                        for r in row..row + h {
                            used[r * spec.columns + c] = true;
                        }
                    }
                    out.push(Placement { index, col, row });
                    break 'search;
                }
            }
        }
    }
    out
}

/// Packs `sizes` onto as many grids as it takes: each page takes what
/// [`pack`] fits of the tiles still left, in order. Indices are into
/// `sizes`. Always returns at least one (maybe empty) page.
pub fn pack_pages(spec: &GridSpec, sizes: &[TileSize]) -> Vec<Vec<Placement>> {
    let mut left: Vec<usize> = (0..sizes.len()).collect();
    let mut pages = Vec::new();
    while !left.is_empty() {
        let page_sizes: Vec<TileSize> = left.iter().map(|&i| sizes[i]).collect();
        let placed: Vec<Placement> = pack(spec, &page_sizes)
            .into_iter()
            .map(|p| Placement {
                index: left[p.index],
                ..p
            })
            .collect();
        if placed.is_empty() {
            // A tile larger than the grid; drop the rest.
            break;
        }
        left.retain(|i| !placed.iter().any(|p| p.index == *i));
        pages.push(placed);
    }
    if pages.is_empty() {
        pages.push(Vec::new());
    }
    pages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_footprints_map_to_responsive_size_classes() {
        assert_eq!(SizeClass::from(TileSize::Small), SizeClass::Small);
        assert_eq!(SizeClass::from(TileSize::Wide), SizeClass::Wide);
        assert_eq!(SizeClass::from(TileSize::Tall), SizeClass::Tall);
        assert_eq!(SizeClass::from(TileSize::Large), SizeClass::Large);
    }

    #[test]
    fn orientation_uses_tile_aspect_ratio() {
        assert_eq!(
            orient_of(Rect::from_min_size(Pos2::ZERO, vec2(120.0, 60.0))),
            Orient::Horizontal
        );
        assert_eq!(
            orient_of(Rect::from_min_size(Pos2::ZERO, vec2(60.0, 120.0))),
            Orient::Vertical
        );
        assert_eq!(
            orient_of(Rect::from_min_size(Pos2::ZERO, vec2(80.0, 80.0))),
            Orient::Square
        );
    }

    #[test]
    fn safe_rect_stays_inside_rounded_and_circular_tiles() {
        let rect = Rect::from_min_size(Pos2::ZERO, vec2(120.0, 60.0));
        let rounded = safe_rect(TileShape::Rounded, rect, 28.0);
        assert!(rect.contains(rounded.min) && rect.contains(rounded.max));
        let square = Rect::from_min_size(Pos2::ZERO, vec2(100.0, 100.0));
        let circle_safe = safe_rect(TileShape::Circle, square, 50.0);
        assert!(square.contains(circle_safe.min) && square.contains(circle_safe.max));
        let radius = 50.0;
        for p in [
            circle_safe.left_top(),
            circle_safe.right_top(),
            circle_safe.left_bottom(),
            circle_safe.right_bottom(),
        ] {
            assert!((p - square.center()).length() <= radius);
        }
    }

    fn pill() -> Squircle {
        Squircle::new(Rect::from_min_size(pos2(0.0, 0.0), vec2(140.0, 34.0)), 17.0)
    }

    #[test]
    fn radius_is_clamped_to_half_the_short_side() {
        let s = Squircle::new(Rect::from_min_size(Pos2::ZERO, vec2(100.0, 20.0)), 50.0);
        assert_eq!(s.radius, 10.0);
    }

    #[test]
    fn contains_center_and_edges() {
        let s = pill();
        assert!(s.contains(pos2(70.0, 17.0)));
        assert!(s.contains(pos2(70.0, 0.5)));
        assert!(s.contains(pos2(0.5, 17.0)));
        assert!(!s.contains(pos2(-0.5, 17.0)));
        assert!(!s.contains(pos2(70.0, 34.5)));
    }

    #[test]
    fn corners_are_cut() {
        let s = pill();
        // The bounding box corner is outside a rounded shape.
        assert!(!s.contains(pos2(0.5, 0.5)));
        assert!(!s.contains(pos2(139.5, 33.5)));
        // A point just inside the curve is inside.
        assert!(s.contains(pos2(4.0, 6.0)));
    }

    #[test]
    fn circular_exponent_makes_a_true_capsule() {
        let s = pill().with_exponent(2.0);
        // On the semicircle end at 45 degrees.
        let c = pos2(17.0, 17.0);
        let on = c + vec2(-1.0, -1.0).normalized() * 16.9;
        let off = c + vec2(-1.0, -1.0).normalized() * 17.2;
        assert!(s.contains(on));
        assert!(!s.contains(off));
        // The default squircle bulges further into the corner than a circle.
        assert!(pill().contains(off));
    }

    #[test]
    fn square_rect_with_zero_radius_contains_corner() {
        let s = Squircle::new(Rect::from_min_size(Pos2::ZERO, vec2(10.0, 10.0)), 0.0);
        assert!(s.contains(pos2(0.0, 0.0)));
        assert!(s.contains(pos2(10.0, 10.0)));
    }

    #[test]
    fn outline_points_lie_on_the_boundary() {
        let s = Squircle::new(
            Rect::from_min_size(pos2(10.0, 5.0), vec2(200.0, 80.0)),
            24.0,
        );
        let pts = s.outline();
        assert_eq!(pts.len(), 4 * (CORNER_SEGMENTS + 1));
        for p in &pts {
            assert!(s.contains(*p), "{p:?} should be on/inside");
            // Nudge outward from the center: must leave the shape.
            let out = *p + (*p - s.rect.center()).normalized() * 0.75;
            assert!(!s.contains(out), "{out:?} should be outside");
        }
    }

    #[test]
    fn outline_is_clockwise_and_convex() {
        let pts = pill().outline();
        let n = pts.len();
        let mut area2 = 0.0;
        for i in 0..n {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            area2 += a.x * b.y - b.x * a.y;
        }
        // Screen space (y down): clockwise has positive signed area.
        assert!(area2 > 0.0);
        for i in 0..n {
            let a = pts[i];
            let b = pts[(i + 1) % n];
            let c = pts[(i + 2) % n];
            let cross = (b - a).x * (c - b).y - (b - a).y * (c - b).x;
            assert!(cross >= -1e-3, "concave at {i}");
        }
    }

    #[test]
    fn notch_outline_bounds_and_hit_test_follow_the_ears() {
        let rect = Rect::from_min_size(pos2(100.0, 0.0), vec2(200.0, 100.0));
        let pts = notch_outline(rect, 12.0, 20.0, 8);
        let min_x = pts.iter().map(|p| p.x).fold(f32::INFINITY, f32::min);
        let max_x = pts.iter().map(|p| p.x).fold(f32::NEG_INFINITY, f32::max);
        let min_y = pts.iter().map(|p| p.y).fold(f32::INFINITY, f32::min);
        let max_y = pts.iter().map(|p| p.y).fold(f32::NEG_INFINITY, f32::max);
        assert_eq!((min_x, max_x, min_y, max_y), (88.0, 312.0, 0.0, 100.0));
        assert!(polygon_contains(&pts, pos2(200.0, 50.0)));
        assert!(!polygon_contains(&pts, pos2(90.0, 1.0)));
        assert!(!polygon_contains(&pts, pos2(80.0, 50.0)));
        assert!(!polygon_contains(&pts, pos2(200.0, 110.0)));
    }

    #[test]
    fn packer_fills_a_three_row_grid() {
        let spec = GridSpec {
            columns: 8,
            rows: 3,
            cell: 60.0,
            gap: 8.0,
        };
        use TileSize::*;
        let sizes = [
            Large, Large, Wide, Small, Large, Wide, Small, Small, Small, Small, Small, Small, Small,
        ];
        let placed = pack(&spec, &sizes);
        assert_eq!(placed.len(), sizes.len());
        assert_eq!(
            placed[0],
            Placement {
                index: 0,
                col: 0,
                row: 0
            }
        );
        // Second 2x2 cannot sit under the first (only one row left), so it moves right.
        assert_eq!(
            placed[1],
            Placement {
                index: 1,
                col: 2,
                row: 0
            }
        );
        // The wide tile fills the row below the first 2x2.
        assert_eq!(
            placed[2],
            Placement {
                index: 2,
                col: 0,
                row: 2
            }
        );
    }

    #[test]
    fn packer_skips_tiles_that_do_not_fit() {
        let spec = GridSpec {
            columns: 2,
            rows: 1,
            cell: 10.0,
            gap: 2.0,
        };
        let placed = pack(&spec, &[TileSize::Large, TileSize::Wide]);
        assert_eq!(
            placed,
            vec![Placement {
                index: 1,
                col: 0,
                row: 0
            }]
        );
    }

    #[test]
    fn vertical_medium_tiles_pack_as_one_column_by_two_rows() {
        let spec = GridSpec {
            columns: 2,
            rows: 3,
            cell: 10.0,
            gap: 2.0,
        };
        assert_eq!(TileSize::Tall.cells(), (1, 2));
        assert_eq!(
            spec.cell_rect(Pos2::ZERO, 1, 1, TileSize::Tall).size(),
            vec2(10.0, 22.0)
        );
        assert_eq!(pack(&spec, &[TileSize::Tall]).len(), 1);
    }

    #[test]
    fn cell_rect_spans_gaps() {
        let spec = GridSpec {
            columns: 8,
            rows: 3,
            cell: 60.0,
            gap: 8.0,
        };
        assert_eq!(spec.size(), vec2(536.0, 196.0));
        let r = spec.cell_rect(Pos2::ZERO, 1, 1, TileSize::Large);
        assert_eq!(r, Rect::from_min_size(pos2(68.0, 68.0), vec2(128.0, 128.0)));
    }

    #[test]
    fn overflow_goes_to_the_next_page() {
        let spec = GridSpec {
            columns: 2,
            rows: 1,
            cell: 60.0,
            gap: 8.0,
        };
        let sizes = [TileSize::Small, TileSize::Wide, TileSize::Small];
        let pages = pack_pages(&spec, &sizes);
        let idx: Vec<Vec<usize>> = pages
            .iter()
            .map(|p| p.iter().map(|pl| pl.index).collect())
            .collect();
        assert_eq!(idx, [vec![0, 2], vec![1]]);
        assert_eq!(pack_pages(&spec, &[]).len(), 1);
    }
}
