//! Sampling: integer bilinear interpolation (coordinates quantised to 1/256 px, 8-bit weights,
//! so samples are bit-identical on every ISA, §8.6), affine resampling, and disk sums for bubble
//! darkness.

use crate::gray::GrayImage;
use crate::warp::Affine;

/// Bilinear sample at point `(x, y)` (corner convention: pixel `i` has its centre at `i + 0.5`);
/// pixels beyond the image read as `outside`.
#[inline]
pub fn bilinear(img: &GrayImage, x: f64, y: f64, outside: u8) -> u8 {
    let fx = ((x - 0.5) * 256.0).round();
    let fy = ((y - 0.5) * 256.0).round();
    if !(fx.is_finite() && fy.is_finite()) || fx.abs() > 1e12 || fy.abs() > 1e12 {
        return outside;
    }
    let (fx, fy) = (fx as i64, fy as i64);
    let (ix, iy) = (fx >> 8, fy >> 8);
    let (wx, wy) = ((fx & 255) as u32, (fy & 255) as u32);
    let w = img.width() as i64;
    let h = img.height() as i64;
    if ix >= 0 && iy >= 0 && ix + 1 < w && iy + 1 < h {
        let i = (iy * w + ix) as usize;
        let d = img.as_raw();
        let (p00, p10) = (u32::from(d[i]), u32::from(d[i + 1]));
        let (p01, p11) = (
            u32::from(d[i + w as usize]),
            u32::from(d[i + w as usize + 1]),
        );
        let top = p00 * (256 - wx) + p10 * wx;
        let bot = p01 * (256 - wx) + p11 * wx;
        return ((top * (256 - wy) + bot * wy + 32_768) >> 16) as u8;
    }
    let g = |xx: i64, yy: i64| u32::from(img.get_or(xx as isize, yy as isize, outside));
    let top = g(ix, iy) * (256 - wx) + g(ix + 1, iy) * wx;
    let bot = g(ix, iy + 1) * (256 - wx) + g(ix + 1, iy + 1) * wx;
    ((top * (256 - wy) + bot * wy + 32_768) >> 16) as u8
}

/// Resamples `src` into a `w × h` image: output pixel centre `q` takes `src(map(q))`, where
/// `map` (output → source) is affine.
///
/// Source coordinates step along each row in 1/65,536-px fixed point (error < 0.01 px over
/// 600 px) and are truncated to 1/256 px for the 8-bit bilinear weights, so the result is
/// identical on every platform.
pub fn resample_affine(
    src: &GrayImage,
    w: usize,
    h: usize,
    map: &Affine,
    outside: u8,
) -> GrayImage {
    resample_affine_offset(src, w, h, map, 0.0, 0.0, outside)
}

/// Like [`resample_affine`], but output pixel `(i, j)` stands for the point
/// `(ox + i + 0.5, oy + j + 0.5)` before `map` is applied.
pub fn resample_affine_offset(
    src: &GrayImage,
    w: usize,
    h: usize,
    map: &Affine,
    ox: f64,
    oy: f64,
    outside: u8,
) -> GrayImage {
    const ONE: f64 = 65_536.0;
    let [a, b, tx, c, d, ty] = map.m;
    let mut out = GrayImage::new(w, h, outside);
    let (sw, sh) = (src.width() as i64, src.height() as i64);
    let data = src.as_raw();
    let lim = 1i64 << 40;
    let dxs = (a * ONE).round() as i64;
    let dys = (c * ONE).round() as i64;
    for j in 0..h {
        let qx = ox + 0.5;
        let qy = oy + j as f64 + 0.5;
        let x0 = a * qx + b * qy + tx - 0.5;
        let y0 = c * qx + d * qy + ty - 0.5;
        if !(x0.is_finite() && y0.is_finite())
            || x0.abs() * ONE > lim as f64
            || y0.abs() * ONE > lim as f64
        {
            continue;
        }
        let mut fx = (x0 * ONE).round() as i64;
        let mut fy = (y0 * ONE).round() as i64;
        let row = out.row_mut(j);
        for o in row.iter_mut() {
            let (px, py) = (fx >> 8, fy >> 8);
            let (ix, iy) = (px >> 8, py >> 8);
            let (wx, wy) = ((px & 255) as u32, (py & 255) as u32);
            *o = if ix >= 0 && iy >= 0 && ix + 1 < sw && iy + 1 < sh {
                let i = (iy * sw + ix) as usize;
                let s = sw as usize;
                let top = u32::from(data[i]) * (256 - wx) + u32::from(data[i + 1]) * wx;
                let bot = u32::from(data[i + s]) * (256 - wx) + u32::from(data[i + s + 1]) * wx;
                ((top * (256 - wy) + bot * wy + 32_768) >> 16) as u8
            } else if ix < -1 || iy < -1 || ix >= sw || iy >= sh {
                outside
            } else {
                let g = |xx: i64, yy: i64| u32::from(src.get_or(xx as isize, yy as isize, outside));
                let top = g(ix, iy) * (256 - wx) + g(ix + 1, iy) * wx;
                let bot = g(ix, iy + 1) * (256 - wx) + g(ix + 1, iy + 1) * wx;
                ((top * (256 - wy) + bot * wy + 32_768) >> 16) as u8
            };
            fx += dxs;
            fy += dys;
        }
    }
    out
}

/// Ink sum over a disk: `ink = Σ (255 − v)` over pixels whose centres lie within `r` of `(cx, cy)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskSum {
    /// Σ (255 − v).
    pub ink: u64,
    /// Pixels inside the image and the disk.
    pub count: u32,
    /// Pixels the disk would have if the image were unbounded.
    pub expected: u32,
}

impl DiskSum {
    /// True if part of the disk falls outside the image.
    pub fn clipped(&self) -> bool {
        self.count < self.expected
    }

    /// Mean darkness `ink / (255 · count)` in 1/1024 units, rounded half up (0 if empty).
    pub fn darkness_q10(&self) -> u16 {
        self.darkness_q10_of(255)
    }

    /// Mean darkness relative to a paper level, `ink / (paper · count)` in 1/1024 units,
    /// rounded half up, at most 1024 (0 if empty or `paper` is 0). Pairs with
    /// [`disk_sum_paper`]; `darkness_q10_of(255)` is [`Self::darkness_q10`].
    pub fn darkness_q10_of(&self, paper: u8) -> u16 {
        if self.count == 0 || paper == 0 {
            return 0;
        }
        let p = u64::from(paper);
        let num = 2 * 1024 * self.ink + p * u64::from(self.count);
        let den = 2 * p * u64::from(self.count);
        (num / den).min(1024) as u16
    }
}

/// Sums ink over the disk of radius `r` centred at `(cx, cy)`; the pixel set is
/// `{(x, y) : (x + 0.5 − cx)² + (y + 0.5 − cy)² ≤ r²}` (the prototype's definition).
///
/// A non-finite or negative radius, a non-finite centre, or a centre more than 10⁹ px away
/// yields an empty, [`DiskSum::clipped`] sum (never a reading).
pub fn disk_sum(img: &GrayImage, cx: f64, cy: f64, r: f64) -> DiskSum {
    disk_sum_paper(img, cx, cy, r, 255)
}

/// [`disk_sum`] against a local paper level `B` (§8.5 normalised darkness with `K = 0`):
/// `ink = Σ max(0, B − v)`, so pixels brighter than the paper add nothing; read it with
/// [`DiskSum::darkness_q10_of`]`(B)`. `paper = 255` is exactly [`disk_sum`].
pub fn disk_sum_paper(img: &GrayImage, cx: f64, cy: f64, r: f64, paper: u8) -> DiskSum {
    const FAR: f64 = 1e9;
    if !(cx.is_finite() && cy.is_finite() && r.is_finite())
        || r < 0.0
        || cx.abs() > FAR
        || cy.abs() > FAR
        || r > FAR
    {
        return DiskSum {
            ink: 0,
            count: 0,
            expected: 1,
        };
    }
    let r2 = r * r;
    let x0 = (cx - r).floor() as i64;
    let x1 = (cx + r).ceil() as i64 + 1;
    let y0 = (cy - r).floor() as i64;
    let y1 = (cy + r).ceil() as i64 + 1;
    let (w, h) = (img.width() as i64, img.height() as i64);
    let mut s = DiskSum {
        ink: 0,
        count: 0,
        expected: 0,
    };
    for y in y0..y1 {
        let dy = y as f64 + 0.5 - cy;
        let dy2 = dy * dy;
        if dy2 > r2 {
            continue;
        }
        let inside_row = y >= 0 && y < h;
        let row = if inside_row {
            Some(img.row(y as usize))
        } else {
            None
        };
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
                s.ink += u64::from(paper.saturating_sub(row[x as usize]));
                s.count += 1;
            }
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear_at_centres_and_midpoints() {
        let img = GrayImage::from_fn(3, 2, |x, y| (x * 100 + y * 10) as u8);
        assert_eq!(bilinear(&img, 1.5, 0.5, 255), 100);
        assert_eq!(bilinear(&img, 1.0, 0.5, 255), 50);
        assert_eq!(bilinear(&img, 1.5, 1.0, 255), 105);
        assert_eq!(bilinear(&img, -5.0, 0.5, 7), 7);
    }

    #[test]
    fn resample_identity_and_agrees_with_bilinear() {
        let img = GrayImage::from_fn(9, 7, |x, y| (x * 20 + y * 3) as u8);
        assert_eq!(resample_affine(&img, 9, 7, &Affine::IDENTITY, 0), img);
        let big = GrayImage::from_fn(300, 200, |x, y| ((x * 7 + y * 13 + x * y) % 256) as u8);
        let m = Affine::similarity(0.93, 0.03, 11.3, -4.7);
        let r = resample_affine_offset(&big, 120, 90, &m, 10.0, 20.0, 255);
        let mut maxd = 0i32;
        for j in 0..90 {
            for i in 0..120 {
                let p = (10.0 + i as f64 + 0.5, 20.0 + j as f64 + 0.5);
                let (x, y) = (
                    m.m[0] * p.0 + m.m[1] * p.1 + m.m[2],
                    m.m[3] * p.0 + m.m[4] * p.1 + m.m[5],
                );
                let e = i32::from(bilinear(&big, x, y, 255));
                maxd = maxd.max((e - i32::from(r.get(i, j))).abs());
            }
        }
        assert!(maxd <= 3, "{maxd}");
    }

    #[test]
    fn disk_sum_counts_and_clips() {
        let img = GrayImage::new(40, 40, 0);
        let s = disk_sum(&img, 20.0, 20.0, 5.0);
        assert_eq!(s.count, s.expected);
        assert_eq!(s.darkness_q10(), 1024);
        assert!((70..=90).contains(&s.count));
        let c = disk_sum(&img, 1.0, 20.0, 5.0);
        assert!(c.clipped());
        let white = GrayImage::new(40, 40, 255);
        assert_eq!(disk_sum(&white, 20.0, 20.0, 5.0).darkness_q10(), 0);
        // Against a local paper level: paper reads 0 even when gray, half-paper reads 0.5.
        let gray = GrayImage::new(40, 40, 160);
        let s = disk_sum_paper(&gray, 20.0, 20.0, 5.0, 160);
        assert_eq!(s.darkness_q10_of(160), 0);
        let half = GrayImage::new(40, 40, 80);
        assert_eq!(
            disk_sum_paper(&half, 20.0, 20.0, 5.0, 160).darkness_q10_of(160),
            512
        );
        // Brighter than the paper adds nothing; 255 is disk_sum.
        assert_eq!(
            disk_sum_paper(&white, 20.0, 20.0, 5.0, 160).darkness_q10_of(160),
            0
        );
        assert_eq!(
            disk_sum_paper(&half, 20.0, 20.0, 5.0, 255),
            disk_sum(&half, 20.0, 20.0, 5.0)
        );
        // Degenerate geometry is clipped (unreadable), never a reading.
        for (x, y, r) in [
            (f64::NAN, 20.0, 5.0),
            (20.0, f64::INFINITY, 5.0),
            (20.0, 20.0, f64::NAN),
            (20.0, 20.0, -1.0),
            (1e300, 20.0, 5.0),
        ] {
            let s = disk_sum(&img, x, y, r);
            assert!(s.clipped() && s.count == 0, "{x} {y} {r}: {s:?}");
        }
    }
}
