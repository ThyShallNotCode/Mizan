//! The §6.3 geometry specification table, in integer micrometres.
//!
//! Everything a recognizer needs at run time is in the per-page layout; these constants are
//! exposed so downstream crates can check invariants and size their sampling windows.

use crate::units::{RectUm, Um};

/// A4 width.
pub const A4_W: Um = 210_000;
/// A4 height.
pub const A4_H: Um = 297_000;
/// The A4 safe box (§6.3): x 7–203 mm, y 24.6–272.4 mm. Every printed element lies inside it.
pub const SAFE_BOX_A4: RectUm = RectUm::from_ltrb(7_000, 24_600, 203_000, 272_400);

/// Bubble diameter Ø (4.2 mm; 33 px at 200 dpi), measured on the outline's centre line.
pub const BUBBLE_DIAMETER: Um = 4_200;
/// Bubble radius r₀.
pub const BUBBLE_RADIUS: Um = BUBBLE_DIAMETER / 2;
/// Bubble outline weight: 0.6 pt, 100 % K.
pub const BUBBLE_STROKE: Um = 212;
/// Measured interior: inner 80 % of the radius (§6.3, §8.6), in per mille.
pub const INTERIOR_RATIO_PERMILLE: u16 = 800;
/// Horizontal bubble pitch (≥ 1.6 × Ø; edge gap ≥ 2 mm).
pub const PITCH_X: Um = 7_000;
/// Answer row pitch.
pub const PITCH_Y: Um = 6_350;
/// A gap is inserted after every this many answer rows.
pub const BAND_ROWS: u16 = 5;
/// The gap inserted every [`BAND_ROWS`] rows.
pub const BAND_GAP: Um = 2_000;
/// ID-grid column pitch.
pub const ID_PITCH_X: Um = 7_000;
/// ID-grid row pitch (5.5 mm, the low end of the §6.3 5.5–7.0 mm range, so a 100-item,
/// 10-digit-ID sheet fits one A4 side; see the crate README).
pub const ID_PITCH_Y: Um = 5_500;
/// Quiet zone around blocks, tags and codes: nothing else is printed inside it.
pub const QUIET_ZONE: Um = 3_000;

/// AprilTag 36h11 overall size including the one-module white border.
pub const TAG_SIZE: Um = 8_000;
/// AprilTag module (10 × 10 modules).
pub const TAG_MODULE: Um = 800;
/// Side of the tag's black square (8 modules, 6.4 mm); its corners are the registration points.
pub const TAG_BLACK: Um = 6_400;
/// AprilTag IDs per tag set (§6.4: set k uses IDs 4k … 4k+3 for TL, TR, BR, BL).
pub const TAGS_PER_SET: u16 = 4;
/// Tag set of answer pages.
pub const TAG_SET_ANSWER: u16 = 0;
/// Tag set of proofs (drafts that can never be graded).
pub const TAG_SET_PROOF: u16 = 3;

/// Row tick width.
pub const TICK_W: Um = 1_200;
/// Row tick height.
pub const TICK_H: Um = 600;

/// QR version of the page code.
pub const QR_VERSION: i16 = 3;
/// QR modules per side (version 3).
pub const QR_MODULES: u16 = 29;
/// QR module size.
pub const QR_MODULE: Um = 650;
/// QR symbol size without quiet zone (18.85 mm).
pub const QR_SIZE: Um = QR_MODULE * QR_MODULES as Um;

/// Option-label font size inside bubbles (6 pt).
pub const LABEL_FONT: Um = 2_117;
/// Option-label gray level (45 % K → 255 × 0.55).
pub const LABEL_GRAY: u8 = 140;
/// Question-number font size (7 pt, bold).
pub const QNUM_FONT: Um = 2_469;
/// Echo-line font size (5 pt).
pub const ECHO_FONT: Um = 1_764;
/// Echo-line line height.
pub const ECHO_LINE_H: Um = 2_300;
/// Echo-line box width.
pub const ECHO_W: Um = 36_500;
/// Echo-line box height (two lines: friendly prefix, then the code).
pub const ECHO_H: Um = 2 * ECHO_LINE_H;

/// Answer row: logical offset of the first bubble centre from the column's start edge
/// (tick 1.2 + gap 1.0 + number box 5.0 + gap 1.8 + r₀).
pub const ROW_FIRST_CENTRE: Um = 11_100;
/// Answer row: logical offset of the question number's inner edge.
pub const ROW_QNUM_EDGE: Um = 7_200;
/// Answer row: gap from the last bubble edge to the end tick.
pub const ROW_END_GAP: Um = 1_800;
/// Smallest gutter between answer columns (the block quiet zone).
pub const MIN_GUTTER: Um = QUIET_ZONE;
/// Largest gutter between answer columns; wider layouts are centred instead.
pub const MAX_GUTTER: Um = 8_000;

/// Width of an answer column whose widest row has `options` bubbles.
pub const fn column_width(options: u8) -> Um {
    ROW_FIRST_CENTRE + (options as Um - 1) * PITCH_X + BUBBLE_RADIUS + ROW_END_GAP + TICK_W
}

/// Vertical extent (first bubble top to last bubble bottom) of `n` answer rows.
pub const fn rows_extent(n: u16) -> Um {
    if n == 0 {
        0
    } else {
        (n as Um - 1) * PITCH_Y + ((n as Um - 1) / BAND_ROWS as Um) * BAND_GAP + BUBBLE_DIAMETER
    }
}

/// Offset of row `i`'s centre below the column top.
pub const fn row_centre_offset(i: u16) -> Um {
    BUBBLE_RADIUS + i as Um * PITCH_Y + (i as Um / BAND_ROWS as Um) * BAND_GAP
}

/// How many answer rows fit in a column of height `h`.
pub fn rows_capacity(h: Um) -> u16 {
    let mut n = 0u16;
    while rows_extent(n + 1) <= h {
        n += 1;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_table_values() {
        assert_eq!(QR_SIZE, 18_850);
        assert_eq!(TAG_MODULE * 10, TAG_SIZE);
        assert_eq!(TAG_MODULE * 8, TAG_BLACK);
        assert!(PITCH_X * 10 >= 16 * BUBBLE_DIAMETER, "pitch ≥ 1.6 × Ø");
        assert!(PITCH_X - BUBBLE_DIAMETER >= 2_000, "edge gap ≥ 2 mm");
        assert_eq!(SAFE_BOX_A4.w, 196_000);
        assert_eq!(column_width(5), 44_200);
    }

    #[test]
    fn rows_and_bands() {
        assert_eq!(rows_extent(1), BUBBLE_DIAMETER);
        assert_eq!(rows_extent(5), 4 * PITCH_Y + BUBBLE_DIAMETER);
        assert_eq!(rows_extent(6), 5 * PITCH_Y + BAND_GAP + BUBBLE_DIAMETER);
        assert_eq!(row_centre_offset(5) - row_centre_offset(4), PITCH_Y + BAND_GAP);
        let h = rows_extent(28);
        assert_eq!(rows_capacity(h), 28);
        assert_eq!(rows_capacity(h - 1), 27);
    }
}
