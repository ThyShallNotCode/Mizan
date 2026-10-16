//! Integer micrometre geometry (spec §6.1: "Geometry MUST be integer micrometres (`i32` µm,
//! origin top-left, y down), never pixels or floats"), with conversions to the float
//! millimetre and pixel types downstream crates use.

use omr_model::{PointMm, RectMm};
use serde::{Deserialize, Serialize};

/// A length in micrometres.
pub type Um = i32;

/// Micrometres per millimetre.
pub const UM_PER_MM: Um = 1_000;
/// Micrometres per inch.
pub const UM_PER_INCH: f64 = 25_400.0;
/// One typographic point (1/72 inch) in micrometres, rounded.
pub const PT: Um = 353;

/// Converts micrometres to millimetres.
pub fn um_to_mm(v: Um) -> f64 {
    f64::from(v) / f64::from(UM_PER_MM)
}

/// Converts micrometres to pixels at `dpi` (pixel `i` covers `[i, i + 1)`; see [`crate::render`]).
pub fn um_to_px(v: Um, dpi: f64) -> f64 {
    f64::from(v) * dpi / UM_PER_INCH
}

/// Converts a point size to micrometres (exact to the micrometre).
pub fn pt_to_um(pt: f64) -> Um {
    (pt * UM_PER_INCH / 72.0).round() as Um
}

/// A point on the page in micrometres (origin top-left, y down).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub struct PtUm {
    /// From the left edge.
    pub x: Um,
    /// From the top edge.
    pub y: Um,
}

impl PtUm {
    /// A point from coordinates.
    pub const fn new(x: Um, y: Um) -> Self {
        Self { x, y }
    }

    /// The point in millimetres.
    pub fn to_mm(self) -> PointMm {
        PointMm {
            x: um_to_mm(self.x),
            y: um_to_mm(self.y),
        }
    }

    /// Squared distance in µm² (as `i64`, exact).
    pub fn dist2(self, o: PtUm) -> i64 {
        let dx = i64::from(self.x - o.x);
        let dy = i64::from(self.y - o.y);
        dx * dx + dy * dy
    }
}

/// An axis-aligned rectangle in micrometres.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default,
)]
pub struct RectUm {
    /// Left edge.
    pub x: Um,
    /// Top edge.
    pub y: Um,
    /// Width (≥ 0).
    pub w: Um,
    /// Height (≥ 0).
    pub h: Um,
}

impl RectUm {
    /// A rectangle from its top-left corner and size.
    pub const fn new(x: Um, y: Um, w: Um, h: Um) -> Self {
        Self { x, y, w, h }
    }

    /// A rectangle from its edges.
    pub const fn from_ltrb(l: Um, t: Um, r: Um, b: Um) -> Self {
        Self {
            x: l,
            y: t,
            w: r - l,
            h: b - t,
        }
    }

    /// A square of side `2·r` centred on `c`.
    pub const fn around(c: PtUm, r: Um) -> Self {
        Self {
            x: c.x - r,
            y: c.y - r,
            w: 2 * r,
            h: 2 * r,
        }
    }

    /// Right edge.
    pub const fn right(&self) -> Um {
        self.x + self.w
    }

    /// Bottom edge.
    pub const fn bottom(&self) -> Um {
        self.y + self.h
    }

    /// Centre (rounded down to the micrometre).
    pub const fn centre(&self) -> PtUm {
        PtUm {
            x: self.x + self.w / 2,
            y: self.y + self.h / 2,
        }
    }

    /// The rectangle grown by `d` on every side.
    pub const fn inflate(&self, d: Um) -> Self {
        Self {
            x: self.x - d,
            y: self.y - d,
            w: self.w + 2 * d,
            h: self.h + 2 * d,
        }
    }

    /// True if the interiors overlap (touching edges do not count).
    pub const fn intersects(&self, o: &RectUm) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }

    /// True if `o` lies entirely inside `self` (edges included).
    pub const fn contains_rect(&self, o: &RectUm) -> bool {
        o.x >= self.x && o.y >= self.y && o.right() <= self.right() && o.bottom() <= self.bottom()
    }

    /// The smallest rectangle containing both.
    pub fn union(&self, o: &RectUm) -> RectUm {
        RectUm::from_ltrb(
            self.x.min(o.x),
            self.y.min(o.y),
            self.right().max(o.right()),
            self.bottom().max(o.bottom()),
        )
    }

    /// True if the horizontal extents overlap.
    pub const fn overlaps_x(&self, l: Um, r: Um) -> bool {
        self.x < r && l < self.right()
    }

    /// The rectangle in millimetres.
    pub fn to_mm(&self) -> RectMm {
        RectMm {
            x: um_to_mm(self.x),
            y: um_to_mm(self.y),
            w: um_to_mm(self.w),
            h: um_to_mm(self.h),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversions_are_exact_for_the_spec_constants() {
        assert_eq!(um_to_mm(6_350), 6.35);
        assert_eq!(pt_to_um(6.0), 2_117);
        assert!((um_to_px(4_200, 200.0) - 33.07).abs() < 0.01);
    }

    #[test]
    fn rect_relations() {
        let a = RectUm::new(0, 0, 10, 10);
        let b = RectUm::new(10, 0, 5, 5);
        assert!(!a.intersects(&b), "touching edges do not overlap");
        assert!(a.inflate(1).intersects(&b));
        assert!(a.inflate(1).contains_rect(&a));
        assert_eq!(a.union(&b), RectUm::new(0, 0, 15, 10));
    }
}
