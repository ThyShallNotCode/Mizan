//! Per-bubble measurement on own forms (§8.5 normalised darkness, §8.6 inner disk).
//!
//! The bubble centre is the Stage A prediction (plus the row-tick correction), never moved by
//! the marks (§8.4). With r₀ the printed radius times the local scale √|det J|:
//! - paper background B: the median gray level over the annulus 1.2–1.45 r₀ (outside the
//!   ring, inside the neighbours' quiet gap), so illumination gradients and paper tone cancel;
//! - ink black K: the tags' black-border level (Stage A);
//! - inner darkness: the mean of D = (B − I)/(B − K) over the disk r ≤ 0.8 r₀ (§8.6, locked),
//!   computed exactly in integers and stored as [`Darkness`] (1/1024); core likewise at 0.45 r₀.
//!
//! A bubble is UNREADABLE, never empty, when its disk leaves the image (CAP-009) or it is
//! smaller than 20 px (CAP-002).

use super::config::OwnMeasureConfig;
use super::ticks::TickField;
use crate::measure::{BubbleMeasure, Unreadable};
use omr_imaging::{GrayImage, Homography, Warp};
use omr_model::{Darkness, PointPx};
use serde::{Deserialize, Serialize};

/// Classical shape features of one bubble (§8.6 F4 and F6, simplified), used by the review
/// triggers that stand in for the deferred CNN router (§8.7 step 3).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BubbleFeatures {
    /// Per-mille of inner-disk pixels with D > 0.5 (a tick or X has a high dark fraction
    /// relative to its mean darkness; a light fill a low one).
    pub dark_permille: u16,
    /// Per-mille of annulus pixels (1.2–1.45 r₀) with D > 0.35: ink outside the bubble, as
    /// a cross-out's strokes leave (F6).
    pub annulus_ink_permille: u16,
}

/// A bubble to measure, in template millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BubbleSpec {
    /// Centre (mm).
    pub centre: PointPx,
    /// Printed radius r₀ (mm).
    pub radius: f64,
    /// The answer row it sits on (tick corrections), if any.
    pub question: Option<u16>,
}

/// Histogram of the annulus `r0 ≤ ρ ≤ r1` around `(cx, cy)` (pixel centres) and its size.
fn annulus_hist(
    img: &GrayImage,
    cx: f64,
    cy: f64,
    r0: f64,
    r1: f64,
    mask: &[bool],
) -> ([u32; 256], u32) {
    let (r0s, r1s) = (r0 * r0, r1 * r1);
    let x0 = (cx - r1).floor() as isize;
    let x1 = (cx + r1).ceil() as isize;
    let y0 = (cy - r1).floor() as isize;
    let y1 = (cy + r1).ceil() as isize;
    let (w, h) = (img.width() as isize, img.height() as isize);
    let mut hist = [0u32; 256];
    let mut n = 0u32;
    for y in y0.max(0)..=y1.min(h - 1) {
        let dy = y as f64 + 0.5 - cy;
        let dy2 = dy * dy;
        if dy2 > r1s {
            continue;
        }
        let row = img.row(y as usize);
        for x in x0.max(0)..=x1.min(w - 1) {
            let dx = x as f64 + 0.5 - cx;
            let d2 = dx * dx + dy2;
            if d2 >= r0s && d2 <= r1s && !mask.get(x as usize).copied().unwrap_or(false) {
                hist[usize::from(row[x as usize])] += 1;
                n += 1;
            }
        }
    }
    (hist, n)
}

fn hist_median(hist: &[u32; 256], n: u32) -> Option<u8> {
    if n == 0 {
        return None;
    }
    let half = n.div_ceil(2);
    let mut acc = 0u32;
    for (v, &c) in hist.iter().enumerate() {
        acc += c;
        if acc >= half {
            return Some(v as u8);
        }
    }
    None
}

/// Median of the annulus `r0 ≤ ρ ≤ r1` around `(cx, cy)`.
#[cfg(test)]
fn annulus_median(img: &GrayImage, cx: f64, cy: f64, r0: f64, r1: f64) -> Option<u8> {
    let (h, n) = annulus_hist(img, cx, cy, r0, r1, &[]);
    hist_median(&h, n)
}

/// Disk statistics over pixels whose centres lie within `r` of `(cx, cy)` (the same pixel
/// set as `omr_imaging::disk_sum`): ink Σ(255 − v) and the count over unmasked pixels inside
/// the image, pixels the disk would have unclipped, masked (streak) pixels, and pixels darker
/// than `dark_below`.
#[derive(Clone, Copy, Debug, Default)]
struct DiskStats {
    ink: u64,
    count: u32,
    expected: u32,
    masked: u32,
    dark: u32,
}

impl DiskStats {
    fn clipped(&self) -> bool {
        self.count + self.masked < self.expected
    }
}

fn disk_stats(
    img: &GrayImage,
    cx: f64,
    cy: f64,
    r: f64,
    dark_below: u8,
    mask: &[bool],
) -> DiskStats {
    let r2 = r * r;
    let x0 = (cx - r).floor() as i64;
    let x1 = (cx + r).ceil() as i64 + 1;
    let y0 = (cy - r).floor() as i64;
    let y1 = (cy + r).ceil() as i64 + 1;
    let (w, h) = (img.width() as i64, img.height() as i64);
    let mut s = DiskStats::default();
    for y in y0..y1 {
        let dy = y as f64 + 0.5 - cy;
        let dy2 = dy * dy;
        if dy2 > r2 {
            continue;
        }
        let row = (y >= 0 && y < h).then(|| img.row(y as usize));
        for x in x0..x1 {
            let dx = x as f64 + 0.5 - cx;
            if dx * dx + dy2 > r2 {
                continue;
            }
            s.expected += 1;
            if let Some(row) = row
                && x >= 0
                && x < w
            {
                if mask.get(x as usize).copied().unwrap_or(false) {
                    s.masked += 1;
                    continue;
                }
                let v = row[x as usize];
                s.ink += u64::from(255 - v);
                s.count += 1;
                s.dark += u32::from(v < dark_below);
            }
        }
    }
    s
}

/// The gray level below which D = (B − I)/(B − K) exceeds `frac_permille`/1000.
fn level_for(paper: u8, black: u8, frac_permille: u32) -> u8 {
    let c = u32::from(paper.saturating_sub(black));
    let drop = (c * frac_permille + 500) / 1000;
    (u32::from(paper).saturating_sub(drop)) as u8
}

/// Normalised darkness of a disk: round(1024 · (ink − (255 − B)·n) / ((B − K)·n)), clamped.
fn normalised(ink: u64, n: u32, paper: u8, black: u8) -> u16 {
    if n == 0 {
        return 0;
    }
    let contrast = i64::from(paper.saturating_sub(black).max(1));
    let n = i64::from(n);
    let num = ink as i64 - i64::from(255 - paper) * n;
    if num <= 0 {
        return 0;
    }
    let den = contrast * n;
    ((2 * 1024 * num + den) / (2 * den)).min(1024) as u16
}

/// Measures `specs` on `img` under the map `h` (template mm → image px). Returns the
/// measurements, the paper level B per bubble and the shape features.
pub(crate) fn measure_bubbles(
    img: &GrayImage,
    h: &Homography,
    ticks: Option<&TickField>,
    specs: &[BubbleSpec],
    black: u8,
    mask: &[bool],
    cfg: &OwnMeasureConfig,
) -> (Vec<BubbleMeasure>, Vec<u8>, Vec<BubbleFeatures>) {
    let fi = f64::from(cfg.inner_permille) / 1000.0;
    let fc = f64::from(cfg.core_permille) / 1000.0;
    let [a0, a1] = cfg.annulus_permille.map(|v| f64::from(v) / 1000.0);
    let min_d = f64::from(cfg.min_diameter_mpx) / 1000.0;
    let mut papers = Vec::with_capacity(specs.len());
    let mut feats = Vec::with_capacity(specs.len());
    let out = specs
        .iter()
        .map(|s| {
            let mut p = h.map(s.centre);
            if let (Some(t), Some(q)) = (ticks, s.question) {
                let c = t.correction(q, s.centre);
                p.x += c.x;
                p.y += c.y;
            }
            let r = s.radius * h.local_scale(s.centre);
            let unread = |cause| BubbleMeasure {
                centre: p,
                snapped: false,
                radius_px: r,
                inner: Darkness(0),
                core: Darkness(0),
                unreadable: Some(cause),
            };
            let cause = if !(r.is_finite() && p.x.is_finite() && p.y.is_finite()) {
                Some(Unreadable::Registration)
            } else if 2.0 * r < min_d {
                Some(Unreadable::TooSmall)
            } else {
                None
            };
            if let Some(c) = cause {
                papers.push(0);
                feats.push(BubbleFeatures::default());
                return unread(c);
            }
            let (hist, n_ann) = annulus_hist(img, p.x, p.y, a0 * r, a1 * r, mask);
            let paper = hist_median(&hist, n_ann).unwrap_or(255);
            let dark_inner = level_for(paper, black, 500);
            let inner = disk_stats(img, p.x, p.y, fi * r, dark_inner, mask);
            if inner.clipped() || inner.count == 0 {
                papers.push(0);
                feats.push(BubbleFeatures::default());
                return unread(Unreadable::OutsideImage);
            }
            let core = disk_stats(img, p.x, p.y, fc * r, 0, mask);
            let dark_ann = level_for(paper, black, 350);
            let ann_dark: u32 = hist[..usize::from(dark_ann)].iter().sum();
            papers.push(paper);
            feats.push(BubbleFeatures {
                dark_permille: ((u64::from(inner.dark) * 1000 + u64::from(inner.count) / 2)
                    / u64::from(inner.count)) as u16,
                annulus_ink_permille: if n_ann == 0 {
                    0
                } else {
                    ((u64::from(ann_dark) * 1000 + u64::from(n_ann) / 2) / u64::from(n_ann)) as u16
                },
            });
            BubbleMeasure {
                centre: p,
                snapped: false,
                radius_px: r,
                inner: Darkness(normalised(inner.ink, inner.count, paper, black)),
                core: Darkness(normalised(core.ink, core.count, paper, black)),
                unreadable: None,
            }
        })
        .collect();
    (out, papers, feats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalised_darkness_is_exact_and_clamped() {
        // Paper 240, black 40: a pixel at 140 is half way.
        assert_eq!(normalised(u64::from(255u32 - 140) * 10, 10, 240, 40), 512);
        assert_eq!(normalised(u64::from(255u32 - 245) * 10, 10, 240, 40), 0);
        assert_eq!(normalised(u64::from(255u32 - 10) * 10, 10, 240, 40), 1024);
        let img = GrayImage::from_fn(40, 40, |x, y| {
            let d = ((x as f64 + 0.5 - 20.0).powi(2) + (y as f64 + 0.5 - 20.0).powi(2)).sqrt();
            if d < 5.0 { 60 } else { 230 }
        });
        assert_eq!(annulus_median(&img, 20.0, 20.0, 7.0, 9.0), Some(230));
    }
}
