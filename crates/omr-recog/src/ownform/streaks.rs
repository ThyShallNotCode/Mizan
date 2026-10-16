//! Scanner streaks (§8.6 "Streaks", §8.9, CAP-006): a dirty-glass streak is a 1–4 px column
//! band that is darker (or lighter) than its surroundings over the whole page height. A column
//! is a streak when the median over sampled rows of `v(x) − (v(x−4) + v(x+4))/2` is large
//! and at least 1.5× that of the columns 4 px either side (which see the streak as a
//! neighbour with half the effect). Streak columns are excluded from every disk and annulus
//! sample, so a streak through a bubble no longer reads as a mark.
//!
//! The spec's batch rule (same x on ≥ 3 pages before alerting) belongs to `omr-app`; here a
//! page-level CAP-006 WARN (Region) records the columns.

use omr_imaging::GrayImage;
use serde::{Deserialize, Serialize};

/// One detected streak band.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Streak {
    /// First column.
    pub x: u32,
    /// Width in columns.
    pub width: u32,
    /// Median intensity change (gray levels; negative = darker).
    pub delta: i16,
}

/// Streak columns of a page.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Streaks {
    /// Bands, left to right.
    pub bands: Vec<Streak>,
    /// Column mask (true = excluded), empty when there is no streak.
    #[serde(skip)]
    pub mask: Vec<bool>,
}

impl Streaks {
    /// True if column `x` is masked.
    pub fn masked(&self, x: usize) -> bool {
        self.mask.get(x).copied().unwrap_or(false)
    }
}

/// Detects streak bands inside the image region `[x0, x1) × [y0, y1)` (the page's printed
/// area, so the paper edge against the backing is never taken for a streak); `min_delta` gray
/// levels, bands at most `max_width` columns, rows sampled every `row_step`.
pub(crate) fn detect(
    img: &GrayImage,
    region: [usize; 4],
    min_delta: u8,
    max_width: u32,
    row_step: usize,
) -> Streaks {
    let (w, h) = (img.width(), img.height());
    const OFF: usize = 4;
    let [rx0, ry0, rx1, ry1] = region;
    let (rx0, rx1) = (rx0.max(OFF), rx1.min(w.saturating_sub(OFF)));
    let (ry0, ry1) = (ry0, ry1.min(h));
    if rx1 <= rx0 + OFF || ry1 < ry0 + 16 {
        return Streaks::default();
    }
    let rows: Vec<&[u8]> = (ry0 + row_step / 2..ry1)
        .step_by(row_step.max(1))
        .map(|y| img.row(y))
        .collect();
    let mut med = vec![0i32; w];
    let mut buf: Vec<i32> = Vec::with_capacity(rows.len());
    for (x, m) in med.iter_mut().enumerate().take(rx1).skip(rx0) {
        buf.clear();
        for r in &rows {
            buf.push(2 * i32::from(r[x]) - i32::from(r[x - OFF]) - i32::from(r[x + OFF]));
        }
        let mid = buf.len() / 2;
        let (_, v, _) = buf.select_nth_unstable(mid);
        *m = *v; // twice the difference, keeps integers
    }
    let thr = 2 * i32::from(min_delta);
    let is_streak = |x: usize| -> bool {
        let d = med[x];
        if d.abs() < thr {
            return false;
        }
        let side = |o: Option<usize>| o.map_or(0, |i| med.get(i).copied().unwrap_or(0).abs());
        let (l, r) = (side(x.checked_sub(OFF)), side(Some(x + OFF)));
        2 * d.abs() >= 3 * l && 2 * d.abs() >= 3 * r
    };
    let mut bands = Vec::new();
    let mut x = rx0;
    while x < rx1 {
        if !is_streak(x) {
            x += 1;
            continue;
        }
        let sign = med[x].signum();
        let start = x;
        while x < rx1 && is_streak(x) && med[x].signum() == sign {
            x += 1;
        }
        let width = (x - start) as u32;
        if width <= max_width {
            let mut d: Vec<i32> = med[start..x].to_vec();
            d.sort_unstable();
            bands.push(Streak {
                x: start as u32,
                width,
                delta: (d[d.len() / 2] / 2) as i16,
            });
        }
    }
    if bands.is_empty() {
        return Streaks::default();
    }
    let mut mask = vec![false; w];
    for b in &bands {
        for m in mask.iter_mut().skip(b.x as usize).take(b.width as usize) {
            *m = true;
        }
    }
    Streaks { bands, mask }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_dark_and_light_streaks_but_not_content() {
        let mut img = GrayImage::from_fn(200, 300, |x, y| {
            // Paper with some content: a dark box (not full height) and a slanted line.
            if (40..60).contains(&x) && (20..80).contains(&y) {
                30
            } else if x == 120 + y / 10 {
                20
            } else {
                235
            }
        });
        for y in 0..300 {
            for x in 80..83 {
                let v = img.get(x, y);
                img.set(x, y, v.saturating_sub(70));
            }
            let v = img.get(160, y);
            img.set(160, y, v.saturating_add(15));
        }
        let s = detect(&img, [0, 0, 200, 300], 12, 4, 8);
        assert_eq!(s.bands.len(), 2, "{:?}", s.bands);
        assert_eq!((s.bands[0].x, s.bands[0].width), (80, 3));
        assert!(s.bands[0].delta <= -60);
        assert_eq!((s.bands[1].x, s.bands[1].width), (160, 1));
        assert!(s.masked(81) && !s.masked(83) && !s.masked(50));
    }
}
