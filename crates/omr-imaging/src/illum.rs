//! Paper background and illumination flattening for gray and camera captures (§8.5).
//!
//! The paper background `B` is a grayscale **closing** (max filter, then min filter, square
//! element) followed by a box blur of the same size: dark features smaller than the element
//! (bubbles, letters, marks, rules) disappear and the slowly varying paper level — shadows,
//! vignetting, a colour cast converted to luma — remains. [`flatten`] divides it out
//! (`255 · I / B`, i.e. `D = (B − I)/(B − K)` with `K = 0`), which the legacy path uses for
//! its low-resolution correlation levels, and the measurement reads bubbles against `B`
//! (`omr_imaging::disk_sum_paper`). Bitonal captures never use it (§8.5 bitonal mode:
//! `B = 255`, `K = 0`).
//!
//! The max/min filters are separable van Herk / Gil-Werman passes (O(1) per pixel, windows
//! clipped at the image border); the blur uses exact integer sums with round-half-up division,
//! so every result is bit-identical on every platform (§8.11).

use crate::gray::GrayImage;

/// Extreme (max or min) of `src[i − r ..= i + r]` (clipped to the slice) for every `i`.
fn extreme_1d(src: &[u8], r: usize, is_max: bool, out: &mut [u8], pad: &mut Vec<u8>) {
    let n = src.len();
    let w = 2 * r + 1;
    let neutral = if is_max { 0 } else { 255 };
    let pick = |a: u8, b: u8| if is_max { a.max(b) } else { a.min(b) };
    let m = n + 2 * r;
    pad.clear();
    pad.resize(m, neutral);
    pad[r..r + n].copy_from_slice(src);
    // g: running extreme from the start of each block of w; h: to the end of each block.
    let mut g = vec![neutral; m];
    let mut h = vec![neutral; m];
    for i in 0..m {
        g[i] = if i % w == 0 {
            pad[i]
        } else {
            pick(g[i - 1], pad[i])
        };
    }
    for i in (0..m).rev() {
        h[i] = if i % w == w - 1 || i == m - 1 {
            pad[i]
        } else {
            pick(h[i + 1], pad[i])
        };
    }
    for (i, o) in out.iter_mut().enumerate().take(n) {
        *o = pick(h[i], g[i + w - 1]);
    }
}

fn extreme_2d(img: &GrayImage, r: usize, is_max: bool) -> GrayImage {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 || r == 0 {
        return img.clone();
    }
    let mut pad = Vec::new();
    let mut rows = GrayImage::new(w, h, 0);
    for y in 0..h {
        let mut line = vec![0u8; w];
        extreme_1d(img.row(y), r, is_max, &mut line, &mut pad);
        rows.row_mut(y).copy_from_slice(&line);
    }
    let mut out = GrayImage::new(w, h, 0);
    let mut col = vec![0u8; h];
    let mut res = vec![0u8; h];
    for x in 0..w {
        for (y, c) in col.iter_mut().enumerate() {
            *c = rows.get(x, y);
        }
        extreme_1d(&col, r, is_max, &mut res, &mut pad);
        for (y, &v) in res.iter().enumerate() {
            out.set(x, y, v);
        }
    }
    out
}

/// Maximum over the `(2r + 1)²` square around each pixel (clipped at the border).
pub fn max_filter(img: &GrayImage, r: usize) -> GrayImage {
    extreme_2d(img, r, true)
}

/// Minimum over the `(2r + 1)²` square around each pixel (clipped at the border).
pub fn min_filter(img: &GrayImage, r: usize) -> GrayImage {
    extreme_2d(img, r, false)
}

/// Mean over the `(2r + 1)²` square around each pixel (clipped at the border: the mean of the
/// pixels inside the image), exact integer sums, rounded half up.
pub fn box_blur(img: &GrayImage, r: usize) -> GrayImage {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 || r == 0 {
        return img.clone();
    }
    // Integral image with a zero first row and column.
    let iw = w + 1;
    let mut ii = vec![0u64; iw * (h + 1)];
    for y in 0..h {
        let mut run = 0u64;
        let row = img.row(y);
        for x in 0..w {
            run += u64::from(row[x]);
            ii[(y + 1) * iw + x + 1] = ii[y * iw + x + 1] + run;
        }
    }
    GrayImage::from_fn(w, h, |x, y| {
        let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        let s = ii[y1 * iw + x1] + ii[y0 * iw + x0] - ii[y0 * iw + x1] - ii[y1 * iw + x0];
        let n = ((x1 - x0) * (y1 - y0)) as u64;
        ((s + n / 2) / n) as u8
    })
}

/// The paper background `B` of `img` (§8.5): closing with a `(2r + 1)²` square, then a box
/// blur of the same size. Choose `r` so the square is about twice the bubble diameter.
pub fn paper_background(img: &GrayImage, r: usize) -> GrayImage {
    box_blur(&min_filter(&max_filter(img, r), r), r)
}

/// `255 · I / B`, rounded half up and clamped to 255 (`B` = 0 reads as 1). `bg` must have the
/// size of `img`; a paper level of 255 leaves the pixel unchanged.
///
/// # Panics
/// If the sizes differ (a programming error).
pub fn flatten(img: &GrayImage, bg: &GrayImage) -> GrayImage {
    assert_eq!(
        (img.width(), img.height()),
        (bg.width(), bg.height()),
        "flatten: background size"
    );
    let data: Vec<u8> = img
        .as_raw()
        .iter()
        .zip(bg.as_raw())
        .map(|(&v, &b)| {
            let b = u32::from(b.max(1));
            ((255 * u32::from(v) + b / 2) / b).min(255) as u8
        })
        .collect();
    GrayImage::from_raw(img.width(), img.height(), data).expect("same size")
}

/// Illumination statistics of paper levels sampled over the sheet (§8.9 lamp CV).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Illumination {
    /// Mean paper level (0–255).
    pub mean: f64,
    /// Coefficient of variation (standard deviation / mean); 0 for uniform light.
    pub cv: f64,
    /// Darkest sampled paper level.
    pub min: u8,
}

/// Mean, coefficient of variation and minimum of paper levels (e.g. `B` sampled at the bubble
/// centres of a registered page).
pub fn illumination(levels: &[u8]) -> Illumination {
    if levels.is_empty() {
        return Illumination::default();
    }
    let n = levels.len() as f64;
    let s: u64 = levels.iter().map(|&v| u64::from(v)).sum();
    let s2: u64 = levels.iter().map(|&v| u64::from(v) * u64::from(v)).sum();
    let mean = s as f64 / n;
    let var = (s2 as f64 / n - mean * mean).max(0.0);
    Illumination {
        mean,
        cv: if mean > 0.0 { var.sqrt() / mean } else { 0.0 },
        min: levels.iter().copied().min().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn brute(img: &GrayImage, r: usize, is_max: bool) -> GrayImage {
        let (w, h) = (img.width() as isize, img.height() as isize);
        let r = r as isize;
        GrayImage::from_fn(img.width(), img.height(), |x, y| {
            let mut e = if is_max { 0 } else { 255 };
            for yy in (y as isize - r).max(0)..=(y as isize + r).min(h - 1) {
                for xx in (x as isize - r).max(0)..=(x as isize + r).min(w - 1) {
                    let v = img.get(xx as usize, yy as usize);
                    e = if is_max { e.max(v) } else { e.min(v) };
                }
            }
            e
        })
    }

    #[test]
    fn van_herk_matches_brute_force() {
        let img = GrayImage::from_fn(37, 23, |x, y| ((x * 37 + y * 101 + x * y * 7) % 256) as u8);
        for r in [1, 2, 3, 5, 9, 30] {
            assert_eq!(max_filter(&img, r), brute(&img, r, true), "max r={r}");
            assert_eq!(min_filter(&img, r), brute(&img, r, false), "min r={r}");
        }
    }

    #[test]
    fn box_blur_is_the_clipped_mean() {
        let img = GrayImage::from_fn(9, 7, |x, y| (x * 20 + y * 3) as u8);
        let b = box_blur(&img, 1);
        // Corner: mean of the 2 × 2 block (0, 20, 3, 23) = 11.5 → 12.
        assert_eq!(b.get(0, 0), 12);
        // Interior: the 3 × 3 mean of a linear ramp is the centre value.
        assert_eq!(b.get(4, 3), img.get(4, 3));
    }

    #[test]
    fn background_removes_marks_and_flattening_removes_a_shadow() {
        // Paper with a left-to-right shadow (255 → 155) and dark 8-px marks on a 20-px grid.
        let paper = |x: usize| 255 - (x * 100 / 199) as u8;
        let img = GrayImage::from_fn(200, 120, |x, y| {
            if x % 20 < 8 && y % 20 < 8 {
                40
            } else {
                paper(x)
            }
        });
        let bg = paper_background(&img, 8);
        for x in [10usize, 60, 100, 150, 190] {
            let d = i32::from(bg.get(x, 60)) - i32::from(paper(x));
            assert!(
                d.abs() <= 8,
                "x={x}: background {} vs paper {}",
                bg.get(x, 60),
                paper(x)
            );
        }
        let flat = flatten(&img, &bg);
        // Paper reads white everywhere; marks stay dark.
        for x in [12usize, 70, 130, 190] {
            assert!(flat.get(x, 15) >= 245, "paper at {x}: {}", flat.get(x, 15));
        }
        assert!(flat.get(4, 4) < 60 && flat.get(184, 4) < 90);
        // A white page is unchanged.
        let white = GrayImage::new(50, 40, 255);
        assert_eq!(flatten(&white, &paper_background(&white, 5)), white);
        let ill = illumination(&[200, 200, 200]);
        assert_eq!((ill.mean, ill.cv, ill.min), (200.0, 0.0, 200));
        assert!(illumination(&[100, 200]).cv > 0.3);
    }
}
