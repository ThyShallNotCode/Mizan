//! Area reduction, packed 1-bit bitmaps and capture-mode detection (spec §7.2, §8.2, §8.5).
//!
//! Bitonal input (CCITT G4 from scanner PDFs, 1-bit TIFF) is reduced by an integer box factor to
//! *coverage gray*: each output pixel is `round(255 · white / n)` over its `f × f` block (`n`
//! pixels; partial blocks at the right and bottom edges average what they cover). 600 → 200 dpi
//! is `f = 3`; the result takes only the ten values `round(255·k/9)`. The same rounding as the
//! Python prototype (`PIL.Image.reduce`) is used, so reduced pages match it bit for bit.

use crate::ImagingError;
use crate::gray::GrayImage;
use serde::{Deserialize, Serialize};

/// Box-averages `img` by an integer `factor` (≥ 1), rounding half up.
///
/// The output is `ceil(w / f) × ceil(h / f)`; edge blocks average only the pixels they cover.
pub fn reduce_area(img: &GrayImage, factor: usize) -> GrayImage {
    let f = factor.max(1);
    if f == 1 {
        return img.clone();
    }
    if f == 2 {
        return half2(img);
    }
    let (w, h) = (img.width(), img.height());
    let (ow, oh) = (w.div_ceil(f), h.div_ceil(f));
    let mut out = Vec::with_capacity(ow * oh);
    let mut colsum = vec![0u32; w];
    for oy in 0..oh {
        let y0 = oy * f;
        let y1 = (y0 + f).min(h);
        colsum.iter_mut().for_each(|c| *c = 0);
        for y in y0..y1 {
            for (c, &v) in colsum.iter_mut().zip(img.row(y)) {
                *c += u32::from(v);
            }
        }
        let rows = (y1 - y0) as u32;
        for ox in 0..ow {
            let x0 = ox * f;
            let x1 = (x0 + f).min(w);
            let s: u32 = colsum[x0..x1].iter().sum();
            let n = rows * (x1 - x0) as u32;
            out.push(((s + n / 2) / n) as u8);
        }
    }
    GrayImage::from_raw(ow, oh, out).expect("sizes are consistent")
}

/// Halves an image with a 2 × 2 box (one pyramid level, §8.2).
pub fn half(img: &GrayImage) -> GrayImage {
    reduce_area(img, 2)
}

/// Greatest common divisor (u64).
fn gcd(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// One output pixel's footprint along an axis: `(first input index, integer weights)`.
#[derive(Clone, Debug)]
struct Span {
    start: usize,
    weights: Vec<u64>,
}

/// Exact overlaps of `n_out` output pixels of width `den` with `n_src` input pixels of width
/// `num` (common units of 1/num input pixel); weights are integers ≤ min(num, den).
fn area_spans(n_src: usize, num: u64, den: u64) -> Vec<Span> {
    let total = n_src as u64 * num;
    let n_out = total.div_ceil(den) as usize;
    (0..n_out as u64)
        .map(|j| {
            let (a, b) = (j * den, ((j + 1) * den).min(total));
            let start = a / num;
            let mut weights = Vec::with_capacity((den / num + 2) as usize);
            let mut i = start;
            while i * num < b {
                let lo = (i * num).max(a);
                let hi = ((i + 1) * num).min(b);
                weights.push(hi - lo);
                i += 1;
            }
            Span {
                start: start as usize,
                weights,
            }
        })
        .collect()
}

/// Resamples `img` by the rational scale `num / den` (output pixels per input pixel) with
/// **exact area coverage**: each output pixel is the coverage-weighted mean of the input pixels
/// its footprint `[j·den/num, (j+1)·den/num)` overlaps, rounded half up. Weights are integers
/// (units of 1/num input pixel) and every sum is an exact `u64`, so the result is identical on
/// every machine (§8.11).
///
/// - The output is `ceil(w · num / den) × ceil(h · num / den)`; a partial footprint at the right
///   and bottom edges averages only what it covers (as [`reduce_area`] does).
/// - `num / den` is reduced first; `1/f` is exactly [`reduce_area`] with factor `f`, `1/1` a
///   copy. Any ratio works (300 → 200 dpi is `2/3`, 400 → 200 is `1/2`, 240 → 200 is `5/6`);
///   ratios above 1 enlarge by box sampling.
/// - `num` or `den` equal to 0 returns a copy (degenerate input, never a panic).
pub fn resample_area(img: &GrayImage, num: u32, den: u32) -> GrayImage {
    if num == 0 || den == 0 || img.is_empty() {
        return img.clone();
    }
    let g = gcd(u64::from(num), u64::from(den));
    let (num, den) = (u64::from(num) / g, u64::from(den) / g);
    if num == den {
        return img.clone();
    }
    if num == 1 {
        return reduce_area(img, den as usize);
    }
    let (w, h) = (img.width(), img.height());
    let xs = area_spans(w, num, den);
    let ys = area_spans(h, num, den);
    let (ow, oh) = (xs.len(), ys.len());
    let wx: Vec<u64> = xs.iter().map(|s| s.weights.iter().sum()).collect();
    let mut out = Vec::with_capacity(ow * oh);
    let mut hrow = vec![0u64; ow];
    let mut acc = vec![0u64; ow];
    for ys_j in &ys {
        acc.iter_mut().for_each(|a| *a = 0);
        let wy: u64 = ys_j.weights.iter().sum();
        for (k, &wyk) in ys_j.weights.iter().enumerate() {
            let row = img.row(ys_j.start + k);
            for (hv, s) in hrow.iter_mut().zip(&xs) {
                *hv = s
                    .weights
                    .iter()
                    .zip(&row[s.start..s.start + s.weights.len()])
                    .map(|(&wt, &v)| wt * u64::from(v))
                    .sum();
            }
            for (a, &hv) in acc.iter_mut().zip(&hrow) {
                *a += wyk * hv;
            }
        }
        for (&a, &wxj) in acc.iter().zip(&wx) {
            let area = wxj * wy;
            out.push(((a + area / 2) / area) as u8);
        }
    }
    GrayImage::from_raw(ow, oh, out).expect("sizes are consistent")
}

/// Resamples a page captured at `src_dpi` to `dst_dpi` by exact area coverage
/// ([`resample_area`] with `dst_dpi / src_dpi`); a copy when either is 0.
pub fn resample_to_dpi(img: &GrayImage, src_dpi: u32, dst_dpi: u32) -> GrayImage {
    resample_area(img, dst_dpi, src_dpi)
}

/// The output size of [`resample_area`] for an input of `width × height`.
pub fn resampled_size(width: usize, height: usize, num: u32, den: u32) -> (usize, usize) {
    if num == 0 || den == 0 {
        return (width, height);
    }
    let g = gcd(u64::from(num), u64::from(den));
    let (num, den) = (u64::from(num) / g, u64::from(den) / g);
    (
        (width as u64 * num).div_ceil(den) as usize,
        (height as u64 * num).div_ceil(den) as usize,
    )
}

/// The 2 × 2 fast path of [`reduce_area`] (identical results).
fn half2(img: &GrayImage) -> GrayImage {
    let (w, h) = (img.width(), img.height());
    let (ow, oh) = (w.div_ceil(2), h.div_ceil(2));
    let mut out = vec![0u8; ow * oh];
    let full = w / 2;
    let mut pair = vec![0u16; w];
    for oy in 0..oh {
        let y0 = 2 * oy;
        let r0 = img.row(y0);
        let o = &mut out[oy * ow..(oy + 1) * ow];
        if y0 + 1 < h {
            let r1 = img.row(y0 + 1);
            for ((p, &a), &b) in pair.iter_mut().zip(r0).zip(r1) {
                *p = u16::from(a) + u16::from(b);
            }
            for (x, v) in o[..full].iter_mut().enumerate() {
                *v = ((pair[2 * x] + pair[2 * x + 1] + 2) >> 2) as u8;
            }
            if full < ow {
                *o.last_mut().expect("odd width") = ((pair[w - 1] + 1) >> 1) as u8;
            }
        } else {
            for (x, v) in o[..full].iter_mut().enumerate() {
                *v = ((u16::from(r0[2 * x]) + u16::from(r0[2 * x + 1]) + 1) >> 1) as u8;
            }
            if full < ow {
                *o.last_mut().expect("odd width") = r0[w - 1];
            }
        }
    }
    GrayImage::from_raw(ow, oh, out).expect("sizes are consistent")
}

/// A packed 1-bit image: rows of `stride` bytes, most significant bit first.
#[derive(Clone, PartialEq, Eq)]
pub struct Bitmap {
    width: usize,
    height: usize,
    stride: usize,
    bits: Vec<u8>,
    black_is_one: bool,
}

impl std::fmt::Debug for Bitmap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Bitmap({}x{}, stride {}, black_is_one {})",
            self.width, self.height, self.stride, self.black_is_one
        )
    }
}

impl Bitmap {
    /// Wraps packed rows. `black_is_one` follows the PDF `BlackIs1` flag (false: 0 bits are
    /// black, the CCITT default).
    pub fn new(
        width: usize,
        height: usize,
        stride: usize,
        bits: Vec<u8>,
        black_is_one: bool,
    ) -> Result<Self, ImagingError> {
        if stride * 8 < width || stride.checked_mul(height) != Some(bits.len()) {
            return Err(ImagingError::BadDimensions {
                width,
                height,
                len: bits.len(),
            });
        }
        Ok(Self {
            width,
            height,
            stride,
            bits,
            black_is_one,
        })
    }

    /// Width in pixels.
    pub fn width(&self) -> usize {
        self.width
    }

    /// Height in pixels.
    pub fn height(&self) -> usize {
        self.height
    }

    /// True if pixel `(x, y)` is black.
    pub fn is_black(&self, x: usize, y: usize) -> bool {
        let b = (self.bits[y * self.stride + x / 8] >> (7 - (x % 8))) & 1;
        (b == 1) == self.black_is_one
    }

    /// Adds the black count of row `y` into `acc` (one counter per pixel).
    fn add_black_row(&self, y: usize, acc: &mut [u16]) {
        let row = &self.bits[y * self.stride..(y + 1) * self.stride];
        let flip = if self.black_is_one { 0u8 } else { 0xFF };
        for (bx, &byte) in row.iter().enumerate() {
            let b = byte ^ flip; // now 1 = black
            if b == 0 {
                continue;
            }
            let x0 = bx * 8;
            for k in 0..8 {
                let x = x0 + k;
                if x >= self.width {
                    break;
                }
                acc[x] += u16::from((b >> (7 - k)) & 1);
            }
        }
    }
}

/// Reduces a 1-bit image by `factor` to coverage gray (`round(255 · white / n)` per block).
pub fn reduce_bitmap(bm: &Bitmap, factor: usize) -> GrayImage {
    let f = factor.max(1);
    let (w, h) = (bm.width, bm.height);
    let (ow, oh) = (w.div_ceil(f), h.div_ceil(f));
    let mut out = Vec::with_capacity(ow * oh);
    let mut black = vec![0u16; w];
    for oy in 0..oh {
        let y0 = oy * f;
        let y1 = (y0 + f).min(h);
        black.iter_mut().for_each(|c| *c = 0);
        for y in y0..y1 {
            bm.add_black_row(y, &mut black);
        }
        let rows = (y1 - y0) as u32;
        for ox in 0..ow {
            let x0 = ox * f;
            let x1 = (x0 + f).min(w);
            let n = rows * (x1 - x0) as u32;
            let blacks: u32 = black[x0..x1].iter().map(|&c| u32::from(c)).sum();
            let white_sum = 255 * (n - blacks);
            out.push(((white_sum + n / 2) / n) as u8);
        }
    }
    GrayImage::from_raw(ow, oh, out).expect("sizes are consistent")
}

/// How a page was captured, as far as the pixels tell (§7.2, §8.9 bitonal gate).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CaptureKind {
    /// Ordinary 8-bit gray (or colour converted to gray).
    Gray,
    /// Two-level (1-bit or thresholded) pixels at native resolution.
    Bitonal,
    /// Coverage gray from a bitonal original reduced by `factor` (only `round(255·k/f²)` levels).
    ReducedBitonal {
        /// The box factor that produced it (2, 3 or 4).
        factor: u8,
    },
    /// A camera (phone) photo of the sheet, gray or colour converted to gray: perspective,
    /// anisotropic scale and uneven light are expected (§7.6, §8.10). The pixels alone do not
    /// tell a photo from a gray scan, so [`classify_capture`] never returns it: the caller sets
    /// it from the source (device profile, EXIF), and recognition also detects camera geometry
    /// itself.
    Camera,
}

impl CaptureKind {
    /// True for bitonal originals, reduced or not (CAP-003 `bitonal`).
    pub fn is_bitonal(self) -> bool {
        matches!(
            self,
            CaptureKind::Bitonal | CaptureKind::ReducedBitonal { .. }
        )
    }

    /// True for a camera capture.
    pub fn is_camera(self) -> bool {
        matches!(self, CaptureKind::Camera)
    }
}

/// Ink statistics of a page for the blank-page test (§7.5, §8.9 CAP-012).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InkCoverage {
    /// Paper level: the 90th percentile of the page's pixels.
    pub paper: u8,
    /// Pixels darker than half the paper level, per million, inside the page without a
    /// 3 % margin on each side (scanner borders and edge shadows are not content).
    pub dark_ppm: u32,
}

impl InkCoverage {
    /// True below `max_ppm` dark pixels per million (§8.9: < 0.2 % = 2,000 ppm for CAP-012).
    pub fn is_blank(&self, max_ppm: u32) -> bool {
        self.dark_ppm < max_ppm
    }
}

/// Dark-pixel coverage of a page for the blank-page test: a blank back or a page showing only
/// bleed-through from its other side has (almost) no pixel darker than half the paper level,
/// since printing and pencil marks seen through the sheet stay light gray. Every row and every
/// second column is sampled (≈ 2 M pixels at 200 dpi).
pub fn ink_coverage(img: &GrayImage) -> InkCoverage {
    let (w, h) = (img.width(), img.height());
    let mut hist = [0u64; 256];
    for y in 0..h {
        for &v in img.row(y).iter().step_by(2) {
            hist[usize::from(v)] += 1;
        }
    }
    let n: u64 = hist.iter().sum();
    let mut acc = 0u64;
    let mut paper = 255u8;
    for (v, &c) in hist.iter().enumerate() {
        acc += c;
        if acc * 10 >= n * 9 {
            paper = v as u8;
            break;
        }
    }
    let thr = paper / 2;
    let (mx, my) = (w * 3 / 100, h * 3 / 100);
    let (mut dark, mut total) = (0u64, 0u64);
    for y in my..h.saturating_sub(my) {
        let row = img.row(y);
        for &v in row[mx..w.saturating_sub(mx).max(mx)].iter().step_by(2) {
            total += 1;
            dark += u64::from(v < thr);
        }
    }
    InkCoverage {
        paper,
        dark_ppm: (dark * 1_000_000).checked_div(total).unwrap_or(0) as u32,
    }
}

/// Classifies a page by its histogram: two levels → bitonal; only the coverage levels of an
/// `f × f` box over a bitonal original (≥ 99.9 % of pixels, ≥ 3 levels used) → reduced bitonal.
pub fn classify_capture(img: &GrayImage) -> CaptureKind {
    classify_histogram(&img.histogram())
}

/// [`classify_capture`] on every `row_step`-th row only (rows 0, `row_step`, …): a cheap test
/// for the page stage, which must tell a bitonal original (exact `B = 255, K = 0` mode, §8.5)
/// from a gray capture without a full histogram pass. A step of 0 or 1 is the full histogram.
pub fn classify_capture_sampled(img: &GrayImage, row_step: usize) -> CaptureKind {
    // Four interleaved counters: runs of equal pixels (paper) would otherwise serialise every
    // increment on the same counter.
    let mut h4 = [[0u64; 256]; 4];
    for y in (0..img.height()).step_by(row_step.max(1)) {
        let row = img.row(y);
        let mut quads = row.chunks_exact(4);
        for q in &mut quads {
            h4[0][usize::from(q[0])] += 1;
            h4[1][usize::from(q[1])] += 1;
            h4[2][usize::from(q[2])] += 1;
            h4[3][usize::from(q[3])] += 1;
        }
        for &v in quads.remainder() {
            h4[0][usize::from(v)] += 1;
        }
    }
    let mut hist = [0u64; 256];
    for (v, a) in hist.iter_mut().enumerate() {
        *a = h4[0][v] + h4[1][v] + h4[2][v] + h4[3][v];
    }
    classify_histogram(&hist)
}

fn classify_histogram(hist: &[u64; 256]) -> CaptureKind {
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return CaptureKind::Gray;
    }
    let used = hist.iter().filter(|&&c| c > 0).count();
    if used <= 2 && hist[1..255].iter().all(|&c| c == 0) {
        return CaptureKind::Bitonal;
    }
    for f in 2u32..=4 {
        let n = f * f;
        let mut inside = 0u64;
        let mut levels = 0;
        for k in 0..=n {
            let v = ((255 * k + n / 2) / n) as usize;
            inside += hist[v];
            if hist[v] > 0 {
                levels += 1;
            }
        }
        if levels >= 3 && (total - inside) * 1000 <= total {
            return CaptureKind::ReducedBitonal { factor: f as u8 };
        }
    }
    CaptureKind::Gray
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_coverage_tells_blank_and_bleed_through_from_content() {
        // White paper with light gray bleed-through blobs and a dark scanner border.
        let mut img = GrayImage::from_fn(400, 300, |x, y| {
            if x < 4 || y < 4 {
                0
            } else if (100..160).contains(&x) && (100..140).contains(&y) {
                200
            } else {
                240
            }
        });
        let c = ink_coverage(&img);
        assert_eq!(c.paper, 240);
        assert!(c.is_blank(2_000), "{c:?}");
        // Printed content: a dark grid of lines covers far more than 0.2 %.
        for y in (20..280).step_by(20) {
            for x in 20..380 {
                img.set(x, y, 30);
            }
        }
        assert!(!ink_coverage(&img).is_blank(2_000));
    }

    #[test]
    fn reduce_area_rounds_like_pil() {
        // 3x3 blocks with k white pixels give round(255k/9).
        for k in 0..=9usize {
            let img = GrayImage::from_fn(3, 3, |x, y| if y * 3 + x < k { 255 } else { 0 });
            let r = reduce_area(&img, 3);
            assert_eq!(r.get(0, 0) as usize, (255 * k * 2 + 9) / 18);
        }
    }

    #[test]
    fn half_fast_path_matches_generic() {
        for (w, h) in [(7usize, 5usize), (8, 6), (1, 1), (9, 2)] {
            let img = GrayImage::from_fn(w, h, |x, y| ((x * 53 + y * 101 + x * y * 7) % 256) as u8);
            // generic path via factor 2 on a 2x2 tiling equals the explicit block mean
            let fast = half(&img);
            for oy in 0..h.div_ceil(2) {
                for ox in 0..w.div_ceil(2) {
                    let (mut s, mut n) = (0u32, 0u32);
                    for y in 2 * oy..(2 * oy + 2).min(h) {
                        for x in 2 * ox..(2 * ox + 2).min(w) {
                            s += u32::from(img.get(x, y));
                            n += 1;
                        }
                    }
                    assert_eq!(u32::from(fast.get(ox, oy)), (s + n / 2) / n);
                }
            }
        }
    }

    /// Exact (rational) area mean, rounded half up: the oracle for `resample_area`.
    fn naive_resample(img: &GrayImage, num: u32, den: u32) -> GrayImage {
        let (num, den) = (u64::from(num), u64::from(den));
        let (w, h) = (img.width() as u64, img.height() as u64);
        let (ow, oh) = ((w * num).div_ceil(den), (h * num).div_ceil(den));
        GrayImage::from_fn(ow as usize, oh as usize, |ox, oy| {
            let (ox, oy) = (ox as u64, oy as u64);
            let (mut s, mut a) = (0u64, 0u64);
            for y in 0..h {
                let ovy = ((y + 1) * num).min((oy + 1) * den).min(h * num) as i64
                    - (y * num).max(oy * den) as i64;
                if ovy <= 0 {
                    continue;
                }
                for x in 0..w {
                    let ovx = ((x + 1) * num).min((ox + 1) * den).min(w * num) as i64
                        - (x * num).max(ox * den) as i64;
                    if ovx <= 0 {
                        continue;
                    }
                    let wt = (ovx * ovy) as u64;
                    s += wt * u64::from(img.get(x as usize, y as usize));
                    a += wt;
                }
            }
            ((s + a / 2) / a) as u8
        })
    }

    #[test]
    fn resample_area_matches_exact_area_oracle() {
        for (w, h) in [(9usize, 7usize), (10, 10), (1, 1), (13, 4), (31, 17)] {
            let img = GrayImage::from_fn(w, h, |x, y| ((x * 53 + y * 101 + x * y * 7) % 256) as u8);
            for (n, d) in [
                (2u32, 3u32),
                (3, 4),
                (5, 6),
                (4, 3),
                (3, 2),
                (7, 10),
                (1, 3),
            ] {
                assert_eq!(
                    resample_area(&img, n, d),
                    naive_resample(&img, n, d),
                    "{w}x{h} by {n}/{d}"
                );
                let r = resample_area(&img, n, d);
                assert_eq!((r.width(), r.height()), resampled_size(w, h, n, d));
            }
        }
    }

    #[test]
    fn resample_area_reduces_to_box_and_copy() {
        let img = GrayImage::from_fn(23, 19, |x, y| ((x * 31 + y * 7) % 251) as u8);
        for f in 1..=5u32 {
            assert_eq!(resample_area(&img, 1, f), reduce_area(&img, f as usize));
            assert_eq!(resample_area(&img, 2, 2 * f), reduce_area(&img, f as usize));
        }
        assert_eq!(resample_area(&img, 3, 3), img);
        assert_eq!(resample_area(&img, 0, 3), img);
        assert_eq!(resample_to_dpi(&img, 600, 200), reduce_area(&img, 3));
    }

    #[test]
    fn resample_area_keeps_flat_images_flat_and_is_deterministic() {
        let flat = GrayImage::new(301, 211, 137);
        let r = resample_area(&flat, 2, 3);
        assert_eq!((r.width(), r.height()), (201, 141));
        assert!(r.as_raw().iter().all(|&v| v == 137));
        // Output pixel (0, 0) of a checkerboard at 2/3 covers 1.5 × 1.5 input pixels:
        // black 1 + 0.25, white 0.5 + 0.5 → round(255 · 1 / 2.25) = 113; the mean is kept.
        let cb = GrayImage::from_fn(6, 6, |x, y| if (x + y) % 2 == 0 { 0 } else { 255 });
        let r = resample_area(&cb, 2, 3);
        assert_eq!((r.width(), r.height()), (4, 4));
        assert_eq!(r.get(0, 0), 113);
        let mean = r.as_raw().iter().map(|&v| f64::from(v)).sum::<f64>() / 16.0;
        assert!((mean - 127.5).abs() < 1.0, "mean {mean}");
        assert_eq!(resample_area(&cb, 2, 3), resample_area(&cb, 4, 6));
    }

    #[test]
    fn reduce_area_partial_edges() {
        let img = GrayImage::from_fn(5, 4, |x, _| if x < 4 { 0 } else { 255 });
        let r = reduce_area(&img, 2);
        assert_eq!((r.width(), r.height()), (3, 2));
        assert_eq!(r.get(2, 0), 255);
        assert_eq!(r.get(0, 1), 0);
    }

    #[test]
    fn bitmap_reduction_matches_gray_reduction() {
        let (w, h) = (13usize, 7usize);
        let stride = w.div_ceil(8);
        let mut bits = vec![0u8; stride * h];
        let mut gray = GrayImage::new(w, h, 255);
        for y in 0..h {
            for x in 0..w {
                let black = (x * 7 + y * 3) % 5 < 2;
                if black {
                    gray.set(x, y, 0);
                } else {
                    bits[y * stride + x / 8] |= 1 << (7 - x % 8); // 1 = white
                }
            }
        }
        let bm = Bitmap::new(w, h, stride, bits, false).unwrap();
        assert!(bm.is_black(0, 0));
        assert_eq!(reduce_bitmap(&bm, 3), reduce_area(&gray, 3));
        assert_eq!(classify_capture(&gray), CaptureKind::Bitonal);
    }

    #[test]
    fn classify_reduced_and_gray() {
        let bi = GrayImage::from_fn(
            90,
            90,
            |x, y| if (x * 31 + y * 17) % 7 < 3 { 0 } else { 255 },
        );
        let r = reduce_area(&bi, 3);
        assert_eq!(
            classify_capture(&r),
            CaptureKind::ReducedBitonal { factor: 3 }
        );
        let g = GrayImage::from_fn(64, 64, |x, y| ((x * 4 + y) % 256) as u8);
        assert_eq!(classify_capture(&g), CaptureKind::Gray);
        assert!(!CaptureKind::Gray.is_bitonal());
    }
}
