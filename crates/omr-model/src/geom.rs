//! Geometry in template (millimetre) and image (pixel) space.

use serde::{Deserialize, Serialize};

/// A point on the printed page, in millimetres from the top-left corner (x right, y down).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct PointMm {
    /// Millimetres from the left edge.
    pub x: f64,
    /// Millimetres from the top edge.
    pub y: f64,
}

/// A point in a captured image, in pixels from the top-left corner.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct PointPx {
    /// Pixels from the left edge.
    pub x: f64,
    /// Pixels from the top edge.
    pub y: f64,
}

/// An axis-aligned rectangle on the page, in millimetres.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, Default)]
pub struct RectMm {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
}

impl RectMm {
    /// Centre of the rectangle.
    pub fn centre(&self) -> PointMm {
        PointMm {
            x: self.x + self.w / 2.0,
            y: self.y + self.h / 2.0,
        }
    }

    /// True if the two rectangles overlap (touching edges do not count).
    pub fn intersects(&self, o: &RectMm) -> bool {
        self.x < o.x + o.w && o.x < self.x + self.w && self.y < o.y + o.h && o.y < self.y + self.h
    }
}

/// Millimetres per inch.
pub const MM_PER_INCH: f64 = 25.4;

/// Converts millimetres to pixels at `dpi`.
pub fn mm_to_px(mm: f64, dpi: f64) -> f64 {
    mm * dpi / MM_PER_INCH
}
