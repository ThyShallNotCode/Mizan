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
}

impl CaptureKind {
    /// True for bitonal originals, reduced or not (CAP-003 `bitonal`).
    pub fn is_bitonal(self) -> bool {
        !matches!(self, CaptureKind::Gray)
    }
}

/// Classifies a page by its histogram: two levels → bitonal; only the coverage levels of an
/// `f × f` box over a bitonal original (≥ 99.9 % of pixels, ≥ 3 levels used) → reduced bitonal.
pub fn classify_capture(img: &GrayImage) -> CaptureKind {
    let hist = img.histogram();
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
